//! 完整生成流水线。
//!
//! 把各个阶段串起来，按顺序推进，并向外报告进度。

use std::time::Instant;
use tf_core::params::GenerationParams;
use tf_core::DeterministicRng;
use tf_grid::GridOps;

use crate::climate::{run_climate_cancellable, ClimateParams};
use crate::ecology::{find_minerals, run_ecology_with_diversity, EcologyResult, MineralDeposit};
use crate::erosion::{compute_distance_to_sea, ErosionParams, ErosionState};
use crate::fields::WorldFields;
use crate::landforms::{apply_hotspots, generate_hotspots, LandformParams};
use crate::plates::{
    apply_airy_isostasy, apply_seafloor_age, build_plates, find_boundaries, init_crust,
    step_tectonics, TectonicState,
};
use crate::Stage;

/// 一次生成的完整产物。
pub struct GeneratedWorld {
    pub fields: WorldFields,
    pub ecology: EcologyResult,
    pub minerals: Vec<MineralDeposit>,
    pub tectonic: TectonicState,
    pub erosion: ErosionState,
    pub stage_timings: Vec<(Stage, u128)>,
}

/// 进度回调。stage 是当前阶段，progress 是 0-1 的总进度。
pub type ProgressFn<'a> = &'a mut dyn FnMut(Stage, f64);

/// 阶段快照回调。每个阶段结束时调用一次，传入当前的物理场。
///
/// 上层用它把中间结果降采样成可视化帧推给前端，
/// 这样用户能看到星球一步步演化的过程，而不是等待一个最终结果。
pub type FrameFn<'a> = &'a mut dyn FnMut(Stage, &WorldFields);

/// 跑完整流水线。
pub fn generate<G: GridOps>(
    grid: &G,
    params: &GenerationParams,
    on_progress: ProgressFn,
) -> GeneratedWorld {
    generate_with_frames(grid, params, on_progress, &mut |_, _| {})
}

/// 带阶段快照的生成入口。
///
/// 每个阶段结束时会调用 on_frame，传入当时的物理场。
/// 前端靠这个实现「看着星球诞生」的效果。
pub fn generate_with_frames<G: GridOps>(
    grid: &G,
    params: &GenerationParams,
    on_progress: ProgressFn,
    on_frame: FrameFn,
) -> GeneratedWorld {
    generate_cancellable(grid, params, on_progress, on_frame, &|| false)
        .expect("uncancelled valid simulation")
}

pub fn generate_cancellable<G: GridOps>(
    grid: &G,
    params: &GenerationParams,
    on_progress: ProgressFn,
    on_frame: FrameFn,
    cancel: &dyn Fn() -> bool,
) -> Option<GeneratedWorld> {
    if cancel() {
        return None;
    }
    let mut rng = DeterministicRng::derive(params.seed, "tectonics");
    let mut fields = WorldFields::with_capacity(grid.cell_count());
    let mut timings = Vec::new();
    let mut cumulative = 0.0;

    let local = params.is_local();
    let tectonic = if local {
        let start = Instant::now();
        on_progress(Stage::Landforms, 0.);
        let context = crate::scopes::local_terrain(grid, &mut fields, params);
        cumulative = Stage::Tectonics.weight() + Stage::Landforms.weight();
        timings.push((Stage::Landforms, start.elapsed().as_millis()));
        on_frame(Stage::Landforms, &fields);
        on_progress(Stage::Landforms, cumulative);
        context
    } else {
        // ---- 阶段 1：板块构造 ----
        let t = Instant::now();
        on_progress(Stage::Tectonics, cumulative);
        let (mut tectonic, plate_id) = build_plates(grid, params, &mut rng);
        init_crust(grid, &plate_id, &tectonic, &mut fields, &mut rng, params);
        for plate in &mut tectonic.plates {
            let mut total = 0.;
            let mut continental = 0.;
            for (i, &id) in plate_id.iter().enumerate() {
                if id == plate.id {
                    total += 1.;
                    continental += fields.crust_type[i] as f64;
                }
            }
            plate.oceanic = continental < total * 0.5;
        }

        // 板块构造演化：跑若干个百万年的步长，让地壳厚度在边界处增厚/减薄。
        // 步数由精度档位决定。
        let tectonic_steps = (8.0 * params.quality.iteration_scale()).round() as u32;
        for tick in 0..tectonic_steps {
            if cancel() {
                return None;
            }
            step_tectonics(
                grid,
                &tectonic,
                &mut fields,
                params,
                2.0 / tectonic_steps.max(1) as f64,
            );
            on_progress(
                Stage::Tectonics,
                Stage::Tectonics.weight() * (tick + 1) as f64 / tectonic_steps.max(1) as f64,
            );
        }

        // 相对初始平衡地壳的厚度变化产生隆起，保留大陆原有的低地基面。
        apply_airy_isostasy(grid, &mut fields, params);

        let boundaries = find_boundaries(grid, &plate_id, &tectonic, &fields);
        cumulative += Stage::Tectonics.weight();
        timings.push((Stage::Tectonics, t.elapsed().as_millis()));
        on_frame(Stage::Tectonics, &fields);
        on_progress(Stage::Tectonics, cumulative);

        // ---- 阶段 2：构造地貌 ----
        let t = Instant::now();
        on_progress(Stage::Landforms, cumulative);
        let lf = LandformParams::default();
        // Apply thermal ocean subsidence before adding ridges, trenches and arcs.
        if cancel() {
            return None;
        }
        apply_seafloor_age(
            grid,
            &mut fields,
            &tectonic,
            &boundaries,
            params.spreading_rate,
        );
        crate::terrain::apply_relief(grid, &mut fields, &boundaries, params);
        let hotspots = generate_hotspots(
            params.hotspot_count,
            &mut DeterministicRng::derive(params.seed, "hotspots"),
        );
        apply_hotspots(grid, &mut fields, &hotspots, &lf);

        crate::plates::adjust_sea_level(grid, &mut fields, params.land_fraction);
        cumulative += Stage::Landforms.weight();
        timings.push((Stage::Landforms, t.elapsed().as_millis()));
        on_frame(Stage::Landforms, &fields);
        on_progress(Stage::Landforms, cumulative);

        tectonic
    };
    let boundaries = find_boundaries(grid, &fields.plate_id, &tectonic, &fields);

    // ---- 阶段 3：侵蚀水文 ----
    let t = Instant::now();
    on_progress(Stage::Erosion, cumulative);
    compute_distance_to_sea(grid, &mut fields);
    let mut ero_params = ErosionParams::default();
    ero_params.iterations =
        (ero_params.iterations as f64 * params.quality.iteration_scale()) as u32;
    ero_params.k_river *= params.erosion_strength;
    let mut erosion =
        crate::erosion::run_erosion_cancellable(grid, &mut fields, &ero_params, cancel)?;
    cumulative += Stage::Erosion.weight();
    timings.push((Stage::Erosion, t.elapsed().as_millis()));
    on_frame(Stage::Erosion, &fields);
    on_progress(Stage::Erosion, cumulative);

    // The physical elevation remains in metres. Do not remap a completed
    // drainage network through a new global quantile curve.
    if fields.elevation.iter().any(|v| !v.is_finite()) {
        return None;
    }
    compute_distance_to_sea(grid, &mut fields);
    if cancel() {
        return None;
    }

    // ---- 阶段 4：气候 ----
    let t = Instant::now();
    on_progress(Stage::Climate, cumulative);
    let mut clim = ClimateParams::default();
    clim.greenhouse = params.planet.greenhouse_factor;
    clim.precipitation_scale = params.precipitation_scale;
    clim.monsoon = params.monsoon_enabled;
    clim.wind_speed_m_s = params.wind_speed_m_s;
    clim.orographic_strength = params.orographic_strength;
    clim.moisture_recycling = params.moisture_recycling;
    if !(if local {
        crate::scopes::local_climate(grid, &mut fields, params, &clim, cancel)
    } else {
        run_climate_cancellable(grid, &mut fields, &params.planet, &clim, cancel)
    }) {
        return None;
    }
    if params.erosion_strength > 0. {
        let feedback = ErosionParams {
            iterations: 6,
            k_river: ErosionParams::default().k_river * params.erosion_strength,
            ..Default::default()
        };
        erosion = crate::erosion::run_erosion_cancellable(grid, &mut fields, &feedback, cancel)?;
        compute_distance_to_sea(grid, &mut fields);
        if !(if local {
            crate::scopes::local_climate(grid, &mut fields, params, &clim, cancel)
        } else {
            run_climate_cancellable(grid, &mut fields, &params.planet, &clim, cancel)
        }) {
            return None;
        }
    }
    cumulative += Stage::Climate.weight();
    timings.push((Stage::Climate, t.elapsed().as_millis()));
    on_frame(Stage::Climate, &fields);
    on_progress(Stage::Climate, cumulative);

    // ---- 阶段 5：生态矿藏 ----
    let t = Instant::now();
    on_progress(Stage::Ecology, cumulative);
    if params.glacier_enabled {
        // Cold, wet slopes undergo a small additional valley-erosion step.
        // This is an annual-mean approximation, not an ice-sheet flow model.
        let old = fields.elevation.clone();
        for i in 0..grid.cell_count() {
            if old[i] <= 0. || fields.temperature[i] > -2. || fields.precipitation[i] < 200. {
                continue;
            }
            let slope = grid
                .neighbors(i)
                .iter()
                .map(|&j| {
                    ((old[i] - old[j as usize]) as f64 / grid.distance_m(i, j as usize).max(1.))
                        .max(0.)
                })
                .fold(0., f64::max);
            let loss = (slope
                * 2000.
                * params.erosion_strength
                * (fields.precipitation[i] as f64 / 1000.).min(2.))
            .min(100.) as f32;
            fields.elevation[i] = (old[i] - loss).max(0.001);
            fields.temperature[i] += loss * 0.0065;
        }
        erosion = crate::erosion::refresh_hydrology(grid, &mut fields);
        compute_distance_to_sea(grid, &mut fields);
    }
    let ecology = run_ecology_with_diversity(grid, &fields, params.biome_diversity);
    let minerals = find_minerals(
        grid,
        &fields,
        &boundaries,
        &ecology,
        &mut DeterministicRng::derive(params.seed, "minerals"),
    );
    cumulative += Stage::Ecology.weight();
    timings.push((Stage::Ecology, t.elapsed().as_millis()));
    on_frame(Stage::Ecology, &fields);
    on_progress(Stage::Ecology, cumulative);

    if cancel() {
        return None;
    }
    on_progress(Stage::Finalize, 1.0);
    Some(GeneratedWorld {
        fields,
        ecology,
        minerals,
        tectonic,
        erosion,
        stage_timings: timings,
    })
}

#[cfg(test)]
mod regression {
    use super::*;
    #[test]
    fn seeded_worlds_have_finite_fields_and_do_not_become_high_plateaus() {
        let grid = tf_grid::SphereHexGrid::build(24, 6371000., (glam::DVec3::Z, 0.));
        for seed in [0, 1, 42, 2024] {
            let params = GenerationParams {
                seed,
                quality: tf_core::params::SimQuality::Economy,
                ..Default::default()
            };
            let result = generate(&grid, &params, &mut |_, _| {});
            let f = &result.fields;
            for channel in [
                &f.elevation,
                &f.temperature,
                &f.precipitation,
                &f.flow_accumulation,
                &f.lake_depth,
                &f.distance_to_sea,
            ] {
                assert!(channel.iter().all(|v| v.is_finite()), "seed {seed}");
            }
            let mut land: Vec<_> = f.elevation.iter().copied().filter(|&v| v > 0.).collect();
            land.sort_by(f32::total_cmp);
            assert!(!land.is_empty());
            assert!(
                land[land.len() / 2] < 1800.,
                "seed {seed}: median {}",
                land[land.len() / 2]
            );
            let mut seen = vec![false; grid.cell_count()];
            let mut areas = Vec::new();
            for i in 0..grid.cell_count() {
                if seen[i] || f.elevation[i] <= 0. {
                    continue;
                }
                seen[i] = true;
                let mut queue = vec![i];
                let mut cursor = 0;
                let mut area = 0.;
                while cursor < queue.len() {
                    let j = queue[cursor];
                    cursor += 1;
                    area += grid.area_m2(j);
                    for &k in grid.neighbors(j) {
                        let k = k as usize;
                        if !seen[k] && f.elevation[k] > 0. {
                            seen[k] = true;
                            queue.push(k);
                        }
                    }
                }
                areas.push(area);
            }
            areas.sort_by(|a, b| b.total_cmp(a));
            let total = areas.iter().sum::<f64>();
            let top = areas.iter().take(3).sum::<f64>() / total;
            let largest = areas[0] / total;
            assert!(
                top > 0.70 && largest > 0.20 && largest < 0.85,
                "seed {seed}: largest {largest}, top3 {top}"
            );
            let second = generate(&grid, &params, &mut |_, _| {});
            assert_eq!(f.elevation, second.fields.elevation);
            assert_eq!(f.precipitation, second.fields.precipitation);
        }
    }
    #[test]
    fn display_grid_size_does_not_change_the_generated_fields() {
        let grid = tf_grid::FlatGrid::new(24, 16, 2_400_000., 25., 0., 6371000.);
        let mut params = GenerationParams {
            seed: 42,
            quality: tf_core::params::SimQuality::Economy,
            ..Default::default()
        };
        let a = generate(&grid, &params, &mut |_, _| {});
        params.display_grid_m = 500_000.;
        let b = generate(&grid, &params, &mut |_, _| {});
        assert_eq!(a.fields.elevation, b.fields.elevation);
        assert_eq!(a.fields.precipitation, b.fields.precipitation);
    }
    #[test]
    fn cancellation_does_not_return_a_completed_world() {
        let grid = tf_grid::SphereHexGrid::build(4, 6371000., (glam::DVec3::Z, 0.));
        assert!(generate_cancellable(
            &grid,
            &GenerationParams::default(),
            &mut |_, _| {},
            &mut |_, _| {},
            &|| true
        )
        .is_none());
    }
}
