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
    /// Collision footprint in the terrane's moving, radius-normalized frame.
    pub uplift_offset: [f64; 2],
    pub deformation_age_myr: f64,
    pub base_m: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvinceRecord {
    pub center_m: [f64; 3],
    pub kind: ProvinceKind,
    pub age_myr: f64,
    pub inherited_uplift_m: f64,
    pub deformation_age_myr: f64,
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
                uplift_offset: [0., 0.],
                deformation_age_myr: 0.,
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
            deformation_age_myr: p.deformation_age_myr,
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
                p.base_m + 850. * (-p.age_myr / 1400.).exp() * (-0.5 * (y / 0.30).powi(2)).exp(),
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
        // A collision raises a broad local footprint. It does not turn an
        // entire inherited basin/terrane into a rejuvenated mountain belt.
        let collision = p.uplift_m
            * (-0.5 * ((x - p.uplift_offset[0]).powi(2) + (y - p.uplift_offset[1]).powi(2))
                / 0.30_f64.powi(2))
            .exp();
        elevation += (h + collision) * w;
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
        p.deformation_age_myr += dt;
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
            if provinces[i].shape.group == provinces[j].shape.group
                || provinces[i].plate == provinces[j].plate
            {
                continue;
            }
            let a = &provinces[i].shape;
            let b = &provinces[j].shape;
            let (_, ra) = a.radial_extent(b.center, g.radius_m(), g.field_is_planar());
            let (_, rb) = b.radial_extent(a.center, g.radius_m(), g.field_is_planar());
            let reach = ra + rb;
            let distance = |a: DVec3, b: DVec3| {
                if g.field_is_planar() {
                    a.distance(b)
                } else {
                    a.normalize().angle_between(b.normalize()) * g.radius_m()
                }
            };
            let before = distance(initial[i], initial[j]);
            let after = distance(a.center, b.center);
            if after < reach && before > after {
                let shortening = ((before - after) / reach.max(1.)).min(0.1);
                let contact = a.center.lerp(b.center, ra / reach.max(1.));
                let contact = if g.field_is_planar() {
                    contact
                } else {
                    contact.normalize() * g.radius_m()
                };
                for k in [i, j] {
                    if provinces[k].kind != ProvinceKind::Craton {
                        let province = &mut provinces[k];
                        let b = if k == i { ra } else { rb }.max(1.);
                        let delta = contact - province.shape.center;
                        let add = shortening * 3000.;
                        let blend = add / (province.uplift_m + add).max(1e-12);
                        province.uplift_offset[0] += (delta.dot(province.shape.east) / b
                            - province.uplift_offset[0])
                            * blend;
                        province.uplift_offset[1] += (delta.dot(province.shape.north) / b
                            - province.uplift_offset[1])
                            * blend;
                        province.uplift_m = (province.uplift_m + add).min(1600.);
                        province.deformation_age_myr = 0.;
                    }
                }
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
    initial_ids: &[u16],
    duration: f64,
    progress: &mut dyn FnMut(f64),
    cancel: &dyn Fn() -> bool,
    observer: &mut dyn crate::preview::FrameObserver,
) -> Option<(
    Vec<GeologicProvince>,
    u32,
    u32,
    Option<(Vec<u16>, Vec<f32>, Vec<f32>, Vec<f32>)>,
)> {
    let mut provinces = build(g, p);
    if duration <= 0. {
        return Some((provinces, 0, 0, None));
    }
    let coarse = if let Some((w, h)) = g.extent_m() {
        let MapGrid::Plane(flat) = g else {
            unreachable!()
        };
        let cols = 40;
        let rows = (h / w * cols as f64 * 2. / 3_f64.sqrt()).ceil().max(4.) as u32;
        MapGrid::Plane(tf_grid::FlatGrid::with_reference_lat(
            cols,
            rows,
            w,
            flat.lat,
            flat.lon,
            g.radius_m(),
            flat.reference_lat,
        ))
    } else {
        tf_grid::GridDescriptor::sphere(12, g.radius_m()).build()
    };
    let mut material = WorldFields::with_capacity(coarse.cell_count());
    initialize(&coarse, &mut material, p, &provinces);
    for i in 0..coarse.cell_count() {
        let j = g.cell_at(coarse.center_dir(i))?;
        material.plate_id[i] = initial_ids[j];
        material.crust_age[i] = if material.crust_type[i] > 0.5 {
            (600. - duration).max(0.) as f32
        } else {
            70.
        };
    }
    crate::lifecycle::initialize(&coarse, &material, state);
    for province in &mut provinces {
        let i = if coarse.field_is_planar() {
            coarse.locate_local(province.shape.center.x, province.shape.center.y)
        } else {
            coarse.locate(tf_core::geo::UnitVec3::from_vec(province.shape.center)?)
        };
        if let Some(i) = i {
            province.plate = material.plate_id[i] as usize;
        }
    }
    state.provinces = provinces;
    let (mut elapsed, mut steps) = (0., 0_u32);
    let mut topology_elapsed = 0.;
    let minimum_spacing = (0..coarse.cell_count())
        .flat_map(|i| coarse.neighbors(i).iter().map(move |&j| (i, j as usize)))
        .map(|(i, j)| coarse.distance_m(i, j))
        .fold(f64::INFINITY, f64::min);
    while elapsed < duration {
        if cancel() {
            return None;
        }
        crate::plates::limit_speeds(state, &coarse, p);
        let next_output =
            ((elapsed / p.tectonic_step_myr + 1e-8).floor() + 1.) * p.tectonic_step_myr;
        let dt = p
            .tectonic_step_myr
            .min(duration - elapsed)
            .min(next_output - elapsed)
            .min(1.5)
            .min(minimum_spacing / (p.plate_speed_max_cm_yr * 10_000.) * 0.3);
        crate::lifecycle::fit_motion(&coarse, &material, state, p, elapsed, dt);
        crate::evolution::collision_resistance(&coarse, &mut material, state, dt);
        if !crate::lifecycle::advance_labels(&coarse, &mut material, state, dt, elapsed + dt, true)
        {
            return None;
        }
        let mut provinces = std::mem::take(&mut state.provinces);
        advance(g, &mut provinces, state, dt);
        state.provinces = provinces;
        for i in 0..state.centers.len() {
            state.centers[i] = crate::evolution::move_point(state, i, state.centers[i], dt);
        }
        elapsed += dt;
        steps += 1;
        // Movement remains continuous at CFL substeps. Resolve accumulated
        // damage/topology on history intervals (at most 10 Myr), like the
        // detailed window; a mesh substep is not a new independent rift event.
        if elapsed - topology_elapsed >= p.tectonic_step_myr.min(10.) - 1e-8
            || elapsed >= duration - 1e-8
        {
            crate::lifecycle::update(
                &coarse,
                &mut material,
                state,
                p,
                elapsed,
                elapsed - topology_elapsed,
                &mut [],
            );
            topology_elapsed = elapsed;
        }
        progress(elapsed / p.tectonic_age_myr.max(1e-12));
        let mut info = crate::preview::FrameInfo::stage(
            crate::Stage::Tectonics,
            elapsed / p.tectonic_age_myr.max(1e-12) * crate::Stage::Tectonics.weight(),
        );
        info.elapsed_myr = Some(elapsed);
        info.step = steps;
        info.stage_complete = false;
        info.detail = "地块迁移、弱带拉张与板块生灭";
        if observer.wants_frame(info) {
            let mut f = WorldFields::with_capacity(g.cell_count());
            initialize(g, &mut f, p, &state.provinces);
            for i in 0..g.cell_count() {
                if let Some(j) = coarse.cell_at(g.center_dir(i)) {
                    f.plate_id[i] = material.plate_id[j];
                }
            }
            observer.frame(g, &f, info);
        }
    }
    let mut ids = Vec::with_capacity(g.cell_count());
    let mut damage = Vec::with_capacity(g.cell_count());
    let mut ages = Vec::with_capacity(g.cell_count());
    let mut strength = Vec::with_capacity(g.cell_count());
    for i in 0..g.cell_count() {
        let j = coarse.cell_at(g.center_dir(i))?;
        ids.push(material.plate_id[j]);
        damage.push(material.rift_damage[j]);
        ages.push(material.crust_age[j]);
        strength.push(material.craton_strength[j]);
    }
    let count = state.lifecycle.events.len() as u32;
    Some((
        std::mem::take(&mut state.provinces),
        steps,
        count,
        Some((ids, damage, ages, strength)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tf_grid::GridDescriptor;
    #[test]
    fn collision_keeps_an_old_basin_and_raises_a_local_footprint() {
        let g = GridDescriptor::plane(25., 0., 8e6, 40, 24, 6371000.).build();
        let mut provinces = build(&g, &GenerationParams::default());
        provinces.truncate(2);
        for (k, p) in provinces.iter_mut().enumerate() {
            p.shape.center = DVec3::X * (if k == 0 { -250000. } else { 250000. });
            p.shape.east = DVec3::X;
            p.shape.north = DVec3::Y;
            p.shape.vertices = vec![
                glam::DVec2::new(-600000., -600000.),
                glam::DVec2::new(600000., -600000.),
                glam::DVec2::new(600000., 600000.),
                glam::DVec2::new(-600000., 600000.),
            ];
            p.shape.group = k as u32;
            p.plate = k;
            p.kind = ProvinceKind::Basin;
            p.age_myr = 1200.;
        }
        let state = TectonicState {
            planar_velocities: Some(vec![DVec3::X * 15000., -DVec3::X * 15000.]),
            centers: vec![provinces[0].shape.center, provinces[1].shape.center],
            ..Default::default()
        };
        advance(&g, &mut provinces, &state, 1.);
        assert!(provinces
            .iter()
            .all(|p| p.kind == ProvinceKind::Basin && p.age_myr == 1201. && p.uplift_m > 0.));
        let p = &provinces[0];
        let (_, b) = p
            .shape
            .radial_extent(provinces[1].shape.center, g.radius_m(), true);
        let contact = p.shape.center
            + p.shape.east * p.uplift_offset[0] * b
            + p.shape.north * p.uplift_offset[1] * b;
        let mut reference = p.clone();
        reference.uplift_m = 0.;
        let uplift = |point| {
            sample(&[p.clone()], point, g.radius_m(), true).0
                - sample(&[reference.clone()], point, g.radius_m(), true).0
        };
        assert!(uplift(contact) > 1.);
        assert!(uplift(p.shape.center - DVec3::X * b * 0.8) < uplift(contact) * 0.2);
    }
    #[test]
    fn ancient_history_obeys_the_requested_clock_and_retains_spherical_geometry() {
        let g = GridDescriptor::sphere(4, 6371000.).build();
        let p = GenerationParams::default();
        let (mut state, ids) =
            crate::plates::build_plates(&g, &p, &mut DeterministicRng::derive(p.seed, "tectonics"));
        let mut progress = 0.;
        let (provinces, steps, stages, _) = ancient_history(
            &g,
            &p,
            &mut state,
            &ids,
            4480.,
            &mut |t| progress = t,
            &|| false,
            &mut crate::preview::NoFrames,
        )
        .unwrap();
        assert!(steps >= 448);
        assert_eq!(stages, state.lifecycle.events.len() as u32);
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
