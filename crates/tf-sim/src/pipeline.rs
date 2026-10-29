//! 完整生成流水线。
//!
//! 把各个阶段串起来，按顺序推进，并向外报告进度。

use std::time::Instant;
use tf_core::params::GenerationParams;
use tf_core::DeterministicRng;
use tf_grid::GridOps;

use crate::climate::{run_climate, ClimateParams};
use crate::ecology::{find_minerals, run_ecology, EcologyResult, MineralDeposit};
use crate::erosion::{compute_distance_to_sea, run_erosion, ErosionParams, ErosionState};
use crate::fields::WorldFields;
use crate::landforms::{apply_boundary_landforms, apply_hotspots, apply_volcanic_arcs, generate_hotspots, LandformParams};
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
    let mut rng = DeterministicRng::from_seed(params.seed);
    let mut fields = WorldFields::with_capacity(grid.cell_count());
    let mut timings = Vec::new();
    let mut cumulative = 0.0;

    // ---- 阶段 1：板块构造 ----
    let t = Instant::now();
    on_progress(Stage::Tectonics, cumulative);
    let (tectonic, plate_id) = build_plates(grid, params, &mut rng);
    init_crust(grid, &plate_id, &tectonic, &mut fields, &mut rng, params);

    // 板块构造演化：跑若干个百万年的步长，让地壳厚度在边界处增厚/减薄。
    // 步数由精度档位决定。
    let tectonic_steps = (8.0 * params.quality.iteration_scale()).round() as u32;
    for _ in 0..tectonic_steps {
        step_tectonics(grid, &tectonic, &mut fields, params, 12.0);
    }

    // 关键一步：Airy 均衡把地壳厚度变成高程。
    // 不做这一步，高程场就是空的，整个地形都不存在。
    apply_airy_isostasy(grid, &mut fields, params);

    let mut boundaries = find_boundaries(grid, &plate_id, &tectonic, &fields);
    cumulative += Stage::Tectonics.weight();
    timings.push((Stage::Tectonics, t.elapsed().as_millis()));
    on_frame(Stage::Tectonics, &fields);
    on_progress(Stage::Tectonics, cumulative);

    // ---- 阶段 2：构造地貌 ----
    let t = Instant::now();
    on_progress(Stage::Landforms, cumulative);
    let lf = LandformParams::default();
    // 顺序很重要：先让构造地貌把大陆抬起来，再用热沉降覆盖洋底。
    //
    // 如果先热沉降再构造地貌，构造形变会把洋底也抬高几公里，
    // 结果整个海洋变成浅水区（之前版本就是这样）。
    apply_boundary_landforms(grid, &mut fields, &boundaries, &lf);
    apply_volcanic_arcs(grid, &mut fields, &boundaries, &lf);
    let hotspots = generate_hotspots(params.hotspot_count, &mut rng);
    apply_hotspots(grid, &mut fields, &hotspots, &lf);
    // 最后覆盖洋底：热沉降直接设定深度，不受构造形变影响。
    apply_seafloor_age(grid, &mut fields, &tectonic, &boundaries, params.spreading_rate);
    cumulative += Stage::Landforms.weight();
    timings.push((Stage::Landforms, t.elapsed().as_millis()));
    on_frame(Stage::Landforms, &fields);
    on_progress(Stage::Landforms, cumulative);

    // ---- 阶段 3：侵蚀水文 ----
    let t = Instant::now();
    on_progress(Stage::Erosion, cumulative);
    compute_distance_to_sea(grid, &mut fields);
    let mut ero_params = ErosionParams::default();
    ero_params.iterations = (ero_params.iterations as f64 * params.quality.iteration_scale()) as u32;
    ero_params.k_river *= params.erosion_strength;
    let erosion = run_erosion(grid, &mut fields, &ero_params);
    cumulative += Stage::Erosion.weight();
    timings.push((Stage::Erosion, t.elapsed().as_millis()));
    on_frame(Stage::Erosion, &fields);
    on_progress(Stage::Erosion, cumulative);

    // 地形细节合成。
    //
    // 到这一步，构造和侵蚀给出的仍然是「大尺度骨架」——
    // 分位数映射之后陆地会显得非常平坦，因为原始分布的中间段被压平了。
    // 真实地形在各个尺度上都有起伏：山脉、丘陵、沟壑。
    //
    // 这里按「噪声只做细节补充，不主导地形」的原则，
    // 用多倍频噪声叠加出小于格宽的起伏。幅度按海拔调制：
    // 高山起伏大，平原起伏小。
    add_terrain_detail(grid, &mut fields, params.seed);
    eprintln!("[DIAG] 细节合成后粗糙度 {:.1} m", roughness(grid, &fields.elevation));

    // 高程稳健化。
    // 侵蚀和构造叠加可能产生远超物理量级的数值。真实地球的最高点是
    // 珠峰 8.8 km，最深处是马里亚纳海沟 -11 km。这里把分布裁剪到
    // 这个范围，同时保持分位数关系不被破坏。
    normalize_elevation(&mut fields, params.land_fraction);
    eprintln!("[DIAG] 分位数映射后粗糙度 {:.1} m", roughness(grid, &fields.elevation));

    // ---- 阶段 4：气候 ----
    let t = Instant::now();
    on_progress(Stage::Climate, cumulative);
    let mut clim = ClimateParams::default();
    clim.greenhouse = params.planet.greenhouse_factor;
    clim.precipitation_scale = params.precipitation_scale;
    clim.monsoon = params.monsoon_enabled;
    clim.advection_iterations = (clim.advection_iterations as f64 * params.quality.iteration_scale()) as u32;
    run_climate(grid, &mut fields, &params.planet, &clim);
    cumulative += Stage::Climate.weight();
    timings.push((Stage::Climate, t.elapsed().as_millis()));
    on_frame(Stage::Climate, &fields);
    on_progress(Stage::Climate, cumulative);

    // ---- 阶段 5：生态矿藏 ----
    let t = Instant::now();
    on_progress(Stage::Ecology, cumulative);
    let ecology = run_ecology(grid, &fields);
    let minerals = find_minerals(grid, &fields, &boundaries, &ecology, &mut rng);
    cumulative += Stage::Ecology.weight();
    timings.push((Stage::Ecology, t.elapsed().as_millis()));
    on_frame(Stage::Ecology, &fields);
    on_progress(Stage::Ecology, cumulative);

    // 重新算一次边界（地壳类型可能变了）
    boundaries = find_boundaries(grid, &fields.plate_id.clone(), &tectonic, &fields);

    on_progress(Stage::Finalize, 1.0);

    GeneratedWorld {
        fields,
        ecology,
        minerals,
        tectonic,
        erosion,
        stage_timings: timings,
    }
}
/// 把高程分布裁剪并重标定到物理合理范围。
///
/// 目的：侵蚀迭代和构造叠加会让少数格子跑到 ±70 km，
/// 这不是物理可能的地形。这里用分位数裁剪把分布压回地球量级，
/// 同时保持高低关系不变——所以地貌形态不受影响。

/// 给地形叠加多尺度细节。
///
/// 原则：噪声只负责「小于格宽的起伏」，不改变大尺度地貌格局。
/// 幅度按海拔调制——高山崎岖、平原平缓，这是真实地形的特征。
fn add_terrain_detail<G: GridOps>(grid: &G, fields: &mut WorldFields, seed: u64) {
    let noise = tf_core::noise::SimplexNoise::new(seed ^ 0x5DEECE66D);
    let n = grid.cell_count();

    // 频率必须按「每格几个噪声周期」来定，而不是固定的球面频率。
    //
    // 之前用固定频率 6/18/45，在 f=256 时是每 1000 km 一个周期（看不出来），
    // 在 f=512 时是每 500 km 一个周期。结果就是不同分辨率下地形完全不同，
    // 而且高频分量在低分辨率下会混叠成条纹伪影。
    //
    // 正确做法：目标是「每个周期覆盖 3-8 个格子」，这样在任何分辨率下
    // 细节尺度相对于格子是一致的，也不会混叠。
    let cell_count = n as f64;
    // 球面上一条大圆的格数约等于 sqrt(n / 4π) * 2π = sqrt(n*π)
    let cells_per_circumference = (std::f64::consts::PI * cell_count).sqrt();
    // 基础频率：每 6 格一个周期 → 1/(6/cells_per_circumference)
    let base_freq = cells_per_circumference / 6.0;

    for i in 0..n {
        let p = grid.center_dir(i).as_vec();
        let e = fields.elevation[i];

        // 幅度按海拔调制：低地小，高山大
        let amp = if e > 0.0 {
            let t = (e / 4000.0).clamp(0.0, 1.0);
            40.0 + t * 520.0
        } else {
            15.0
        };

        // 三个倍频：1x、2.5x、6x 基础频率
        let f1 = base_freq;
        let f2 = base_freq * 2.5;
        let f3 = base_freq * 6.0;
        let s = noise.fbm(p.x * f1, p.y * f1, p.z * f1, 3, 2.0, 0.5) * 0.60
            + noise.fbm(p.x * f2, p.y * f2, p.z * f2, 2, 2.0, 0.5) * 0.28
            + noise.fbm(p.x * f3, p.y * f3, p.z * f3, 2, 2.0, 0.5) * 0.12;

        fields.elevation[i] = e + (s as f32) * amp;
    }
}



/// 诊断用：陆地格子的平均邻居高差。
fn roughness<G: tf_grid::GridOps>(grid: &G, elev: &[f32]) -> f64 {
    let n = grid.cell_count();
    let mut total = 0.0f64;
    let mut count = 0usize;
    for i in 0..n {
        if elev[i] <= 0.0 {
            continue;
        }
        let nb = grid.neighbors(i);
        if nb.is_empty() {
            continue;
        }
        let mean = nb.iter().map(|&j| elev[j as usize] as f64).sum::<f64>() / nb.len() as f64;
        total += (elev[i] as f64 - mean).abs();
        count += 1;
    }
    if count > 0 { total / count as f64 } else { 0.0 }
}

fn normalize_elevation(fields: &mut WorldFields, target_land_fraction: f64) {
    let n = fields.elevation.len();
    if n == 0 {
        return;
    }

    // 归一化的目标：
    // 1. 陆地占比精确等于目标值
    // 2. 陆地高程分布接近真实地球（低地占大部分面积）
    // 3. 保留局部起伏（否则地形会变成平板）
    let mut sorted: Vec<f32> = fields.elevation.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let target = target_land_fraction.clamp(0.02, 0.95);
    let sea_idx = (((1.0 - target) * n as f64).round() as usize).min(n - 1);
    let sea_level = sorted[sea_idx];

    // 关键修正：land_base 必须是「严格大于海平面的最小陆地值」。
    //
    // 之前直接用 sorted[sea_idx + 1]，但量化后的高程有大量格子恰好等于
    // 海平面值，导致 land_base 取到了海平面本身（-2495），
    // 于是所有陆地都被当成「离海平面 2.5 公里以上」，整块大陆被抬成高原。
    let mut land_base = sea_level;
    for &v in &sorted[sea_idx + 1..] {
        if v > sea_level {
            land_base = v;
            break;
        }
    }

    let land_top = sorted[n - 1];
    let land_span = (land_top - land_base).max(1.0);
    let ocean_deep = sorted[0];
    let ocean_span = (sea_level - ocean_deep).max(1.0);

    for e in fields.elevation.iter_mut() {
        let v = *e;
        if v <= sea_level {
            // 海洋：幂函数让大陆架平缓、深海陡降
            let norm = ((sea_level - v) / ocean_span).clamp(0.0, 1.0);
            let shaped = norm.powf(1.0 / 2.2);
            *e = -(shaped * 7_000.0);
        } else {
            // 陆地：从 land_base 起算，对数压缩让低地占大面积
            let rel = ((v - land_base) / land_span).clamp(0.0, 1.0);
            let k = 5.0;
            let shaped = ((1.0 + rel * k).ln() / (1.0 + k).ln()) as f32;
            *e = shaped * 8_800.0;
        }
    }
}





