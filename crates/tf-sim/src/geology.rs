//! Lagrangian terrane history and inherited basement, distinct from recent rock flux.
//! Ancient history is a procedural kinematic model, not a reconstruction of early Earth.
use crate::{plates::TectonicState, terrain::CrustProvince, WorldFields};
use glam::DVec3;
use serde::{Deserialize, Serialize};
use tf_core::{params::GenerationParams, DeterministicRng};
use tf_grid::{GridOps, MapGrid};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvinceKind {
    Craton,
    OldOrogen,
    Basin,
    Rift,
}

#[derive(Debug, Clone)]
pub struct GeologicProvince {
    pub shape: CrustProvince,
    pub plate: usize,
    pub kind: ProvinceKind,
    pub age_myr: f64,
    pub uplift_m: f64,
    pub base_m: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvinceRecord {
    pub center_m: [f64; 3],
    pub kind: ProvinceKind,
    pub age_myr: f64,
    pub inherited_uplift_m: f64,
    pub plate_id: usize,
}
pub(crate) fn build(g: &MapGrid, p: &GenerationParams) -> Vec<GeologicProvince> {
    let mut rng = DeterministicRng::derive(p.seed, "inherited-geologic-provinces-v1");
    crate::terrain::continental_provinces(g, p)
        .into_iter()
        .map(|shape| {
            let mut kind = match rng.below(10) {
                0..=3 => ProvinceKind::Craton,
                4..=6 => ProvinceKind::OldOrogen,
                7..=8 => ProvinceKind::Basin,
                _ => ProvinceKind::Rift,
            };
            if shape.vertices.len() == 4 && kind == ProvinceKind::Craton {
                kind = ProvinceKind::OldOrogen;
            }
            GeologicProvince {
                plate: shape.group as usize % p.plate_count as usize,
                shape,
                kind,
                // Backdate the inherited basement continuously across the boundary
                // between the ancient and detailed windows; no jump at 120 Myr.
                age_myr: (rng.range_f64(150., 1800.) - (p.tectonic_age_myr - 120.).max(0.)).max(0.),
                uplift_m: 0.,
                base_m: rng.range_f64(220., 480.),
            }
        })
        .collect()
}
pub(crate) fn records(provinces: &[GeologicProvince]) -> Vec<ProvinceRecord> {
    provinces
        .iter()
        .map(|p| ProvinceRecord {
            center_m: p.shape.center.to_array(),
            kind: p.kind,
            age_myr: p.age_myr,
            inherited_uplift_m: p.uplift_m,
            plate_id: p.plate,
        })
        .collect()
}
pub(crate) fn sample(
    provinces: &[GeologicProvince],
    point: DVec3,
    radius: f64,
    planar: bool,
) -> (f64, f64, f64) {
    let (mut elevation, mut strength, mut support) = (0., 0., 0.);
    for p in provinces {
        let (d, b) = p.shape.radial_extent(point, radius, planar);
        if b < 1. || d > b * 1.3 {
            continue;
        }
        let q = (d / b / 1.3).clamp(0., 1.);
        let w = (1. - q * q).powi(2);
        let delta = point - p.shape.center;
        let x = delta.dot(p.shape.east) / b;
        let y = delta.dot(p.shape.north) / b;
        let (h, s) = match p.kind {
            ProvinceKind::Craton => (
                p.base_m + 120. * x,
                0.25 + 0.70 * (1. - (-p.age_myr / 500.).exp()),
            ),
            ProvinceKind::OldOrogen => (
                p.base_m
                    + (850. * (-p.age_myr / 1400.).exp() + p.uplift_m)
                        * (-0.5 * (y / 0.30).powi(2)).exp(),
                0.35,
            ),
            ProvinceKind::Basin => (
                p.base_m
                    - 650.
                        * (1. - (-p.age_myr / 180.).exp())
                        * (-0.5 * (d / b / 0.55).powi(2)).exp(),
                0.60,
            ),
            ProvinceKind::Rift => (
                p.base_m - 650. * (-0.5 * (y / 0.12).powi(2)).exp()
                    + 220. * (-0.5 * ((y.abs() - 0.30) / 0.12).powi(2)).exp(),
                0.12,
            ),
        };
        elevation += h * w;
        strength += s * w;
        support += w;
    }
    if support > 1e-10 {
        (elevation / support, strength / support, support.min(1.))
    } else {
        (350., 0.5, 0.)
    }
}
pub(crate) fn initialize(
    g: &MapGrid,
    f: &mut WorldFields,
    p: &GenerationParams,
    provinces: &[GeologicProvince],
) {
    let shapes: Vec<_> = provinces.iter().map(|p| p.shape.clone()).collect();
    crate::terrain::init_from_provinces(g, f, p, &shapes);
    for i in 0..g.cell_count() {
        let (basement, strength, support) = sample(
            provinces,
            g.field_position(i),
            g.radius_m(),
            g.field_is_planar(),
        );
        let land = f.elevation[i].max(0.) as f64 / 450.;
        let envelope = land.clamp(0., 1.) * f.crust_type[i] as f64 * support;
        f.elevation[i] += ((basement - 350.) * envelope * p.interior_relief) as f32;
        f.craton_strength[i] = (strength * f.crust_type[i] as f64) as f32;
    }
}

pub(crate) fn advance(
    g: &MapGrid,
    provinces: &mut [GeologicProvince],
    state: &TectonicState,
    dt: f64,
) {
    let initial: Vec<_> = provinces.iter().map(|p| p.shape.center).collect();
    for p in provinces.iter_mut() {
        p.age_myr += dt;
        p.uplift_m *= (-dt / 450.).exp();
        p.shape.center = crate::evolution::move_point(state, p.plate, p.shape.center, dt);
        let (axis, angle) = if state.planar_velocities.is_some() {
            (
                DVec3::Z,
                state
                    .planar_spin_rad_myr
                    .as_ref()
                    .map_or(0., |s| s[p.plate])
                    * dt,
            )
        } else {
            let plate = &state.plates[p.plate];
            (DVec3::from_array(plate.euler_pole), plate.omega * dt)
        };
        p.shape.east = crate::evolution::rotate(p.shape.east, axis, angle);
        p.shape.north = crate::evolution::rotate(p.shape.north, axis, angle);
    }
    // Distinct terranes record actual approach, rather than drawing every present
    // plate boundary as a ridge. Old relief then decays on geological timescales.
    for i in 0..provinces.len() {
        for j in i + 1..provinces.len() {
            if provinces[i].shape.group == provinces[j].shape.group {
                continue;
            }
            let a = &provinces[i].shape;
            let b = &provinces[j].shape;
            let reach = a.vertices.iter().map(|v| v.length()).fold(0_f64, f64::max)
                + b.vertices.iter().map(|v| v.length()).fold(0_f64, f64::max);
            let before = initial[i].distance(initial[j]);
            let after = a.center.distance(b.center);
            if after < reach && before > after {
                let shortening = ((before - after) / reach.max(1.)).min(0.1);
                for k in [i, j] {
                    if provinces[k].kind != ProvinceKind::Craton {
                        provinces[k].uplift_m =
                            (provinces[k].uplift_m + shortening * 3000.).min(1600.);
                        if shortening > 0.001 {
                            provinces[k].kind = ProvinceKind::OldOrogen;
                            provinces[k].age_myr = 0.;
                        }
                    }
                }
            }
        }
    }
    // A finite flat world's ancient kinematic groups stay within its extent. Do not
    // flush its entire continental history through an open boundary 20+ times.
    if let Some((w, h)) = g.extent_m() {
        for group in 0..provinces.iter().map(|p| p.shape.group).max().unwrap_or(0) + 1 {
            let members: Vec<_> = provinces
                .iter()
                .enumerate()
                .filter(|(_, p)| p.shape.group == group)
                .map(|(i, _)| i)
                .collect();
            if members.is_empty() {
                continue;
            }
            let mean = members
                .iter()
                .map(|&i| provinces[i].shape.center)
                .sum::<DVec3>()
                / members.len() as f64;
            let offset = DVec3::new(
                mean.x.clamp(-w * 0.30, w * 0.30) - mean.x,
                mean.y.clamp(-h * 0.30, h * 0.30) - mean.y,
                0.,
            );
            for i in members {
                provinces[i].shape.center += offset;
            }
        }
    }
}

/// Low-cost ancient block history. The recent finite-volume window remains
/// separately accounted in History; this function does not claim a rock budget.
pub(crate) fn ancient_history(
    g: &MapGrid,
    p: &GenerationParams,
    state: &mut TectonicState,
    duration: f64,
    progress: &mut dyn FnMut(f64),
    cancel: &dyn Fn() -> bool,
    observer: &mut dyn crate::preview::FrameObserver,
) -> Option<(Vec<GeologicProvince>, u32, u32, Option<Vec<u16>>)> {
    let mut provinces = build(g, p);
    let (mut elapsed, mut steps, mut cycles) = (0., 0_u32, 0_u32);
    let mut next_reorganization = 200.;
    let mut latest_ids = None;
    while elapsed < duration {
        if cancel() {
            return None;
        }
        crate::plates::limit_speeds(state, g, p);
        let dt = p
            .tectonic_step_myr
            .min(duration - elapsed)
            .min(next_reorganization - elapsed);
        advance(g, &mut provinces, state, dt);
        for i in 0..state.centers.len() {
            state.centers[i] = crate::evolution::move_point(state, i, state.centers[i], dt);
        }
        elapsed += dt;
        steps += 1;
        if elapsed >= next_reorganization - 1e-9 {
            cycles += 1;
            let mut rng =
                DeterministicRng::derive(p.seed ^ cycles as u64, "ancient-mantle-stage-poles");
            let (replacement, ids) = crate::plates::build_plates(g, p, &mut rng);
            state.plates = replacement.plates;
            state.planar_velocities = replacement.planar_velocities;
            state.planar_spin_rad_myr = replacement.planar_spin_rad_myr;
            state.centers = replacement.centers;
            state.partition_weights = replacement.partition_weights;
            latest_ids = Some(ids);
            crate::plates::limit_speeds(state, g, p);
            // A motion-stage change is not a plate birth or a terrane transfer.
            // Keep each inherited rigid group attached to its motion carrier;
            // actual split/absorption needs a topology event, still unsupported.
            next_reorganization += 200.;
        }
        progress(elapsed / p.tectonic_age_myr.max(1e-12));
        let mut info = crate::preview::FrameInfo::stage(
            crate::Stage::Tectonics,
            elapsed / p.tectonic_age_myr.max(1e-12) * crate::Stage::Tectonics.weight(),
        );
        info.elapsed_myr = Some(elapsed);
        info.step = steps;
        info.stage_complete = false;
        info.detail = "古老地块迁移与继承地貌";
        if observer.wants_frame(info) {
            let mut f = WorldFields::with_capacity(g.cell_count());
            initialize(g, &mut f, p, &provinces);
            observer.frame(g, &f, info);
        }
    }
    Some((provinces, steps, cycles, latest_ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tf_grid::GridDescriptor;
    #[test]
    fn ancient_history_obeys_the_requested_clock_and_retains_spherical_geometry() {
        let g = GridDescriptor::sphere(4, 6371000.).build();
        let p = GenerationParams::default();
        let (mut state, _) =
            crate::plates::build_plates(&g, &p, &mut DeterministicRng::derive(p.seed, "tectonics"));
        let mut progress = 0.;
        let (provinces, steps, stages, _) = ancient_history(
            &g,
            &p,
            &mut state,
            4480.,
            &mut |t| progress = t,
            &|| false,
            &mut crate::preview::NoFrames,
        )
        .unwrap();
        assert_eq!(steps, 448);
        assert_eq!(stages, 22);
        assert!((progress - 4480. / 4600.).abs() < 1e-12);
        assert!(provinces.iter().all(
            |p| p.age_myr.is_finite() && (p.shape.center.length() - g.radius_m()).abs() < 1e-5
        ));
        let a = build(
            &g,
            &GenerationParams {
                tectonic_age_myr: 120.,
                ..p.clone()
            },
        );
        let b = build(
            &g,
            &GenerationParams {
                tectonic_age_myr: 120.01,
                ..p
            },
        );
        for (a, b) in a.iter().zip(b) {
            assert!((a.age_myr - b.age_myr - 0.01).abs() < 1e-9);
        }
    }
    #[test]
    fn geological_provinces_have_distinct_stability_and_inherited_relief() {
        let g = GridDescriptor::sphere(16, 6371000.).build();
        let p = GenerationParams {
            tectonic_age_myr: 80.,
            ..Default::default()
        };
        let provinces = build(&g, &p);
        assert!(provinces.iter().any(|p| p.kind == ProvinceKind::Craton));
        assert!(provinces.iter().any(|p| p.kind == ProvinceKind::Basin));
        let mut f = WorldFields::with_capacity(g.cell_count());
        initialize(&g, &mut f, &p, &provinces);
        let land: Vec<_> = (0..g.cell_count())
            .filter(|&i| f.crust_type[i] > 0.99 && f.elevation[i] > 0.)
            .collect();
        let low = land
            .iter()
            .map(|&i| f.elevation[i])
            .fold(f32::INFINITY, f32::min);
        let high = land
            .iter()
            .map(|&i| f.elevation[i])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(high - low > 400., "inherited basement range {}", high - low);
        assert!(land.iter().any(|&i| f.craton_strength[i] > 0.7));
        assert!(land.iter().any(|&i| f.craton_strength[i] < 0.5));
    }
}
