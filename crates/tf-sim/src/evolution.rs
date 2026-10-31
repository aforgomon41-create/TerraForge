//! Kinematic crust reconstruction with conservative forward material remapping.
//! Rigid Euler rotations follow the GPlates convention. The procedural interaction
//! architecture follows Cortial et al. (2019); this is not a mantle-dynamics solver.
use crate::{fields::WorldFields, plates::TectonicState};
use glam::DVec3;
use serde::{Deserialize, Serialize};
use tf_core::{geo::UnitVec3, params::GenerationParams, DeterministicRng};
use tf_grid::{FlatGrid, GridOps, MapGrid, SphereHexGrid};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    pub model: String,
    pub duration_myr: f64,
    pub steps: u32,
    pub initial_crust_m3: f64,
    pub new_ocean_crust_m3: f64,
    pub subducted_crust_m3: f64,
    pub exported_crust_m3: f64,
    pub imported_crust_m3: f64,
    pub final_crust_m3: f64,
    pub relative_budget_error: f64,
    pub output_crust_m3: f64,
    pub output_projection_error: Option<f64>,
    pub volcanoes: Vec<Volcano>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Volcano {
    /// Metres in the root coordinate system; a sphere uses a radius-length vector.
    pub position_m: [f64; 3],
    pub plate_id: u16,
    pub age_myr: f64,
    pub radius_m: f64,
    pub summit_growth_m: f64,
    pub origin: String,
    #[serde(default)]
    pub along_arc: [f64; 3],
    #[serde(default)]
    pub arc_platform_fraction: f64,
    #[serde(default)]
    pub shape_seed: u64,
    /// Time since the last growth episode; distinct from the edifice's age.
    #[serde(default)]
    pub quiet_myr: f64,
    /// Present relief after decay, allowing new growth on an old edifice.
    #[serde(default)]
    pub preserved_growth_m: Option<f64>,
}

fn volcano_amplitude(v: &Volcano) -> f64 {
    v.preserved_growth_m.unwrap_or_else(|| {
        let lifetime = if v.origin == "hotspot" { 75. } else { 28. };
        v.summit_growth_m * (-v.age_myr / lifetime).exp()
    })
}

struct Hotspot {
    position: DVec3,
    next_eruption: f64,
    episode: u64,
    seed: u64,
}

fn hotspot_events(
    grid: &MapGrid,
    f: &WorldFields,
    state: &TectonicState,
    history: &mut History,
    hotspots: &mut [Hotspot],
    elapsed: f64,
) {
    for source in hotspots {
        while elapsed >= source.next_eruption {
            let mut rng = DeterministicRng::derive(source.seed ^ source.episode, "plume-episode");
            let age = elapsed - source.next_eruption;
            if let Some(cell) = locate(grid, source.position) {
                let plate_id = f.plate_id[cell];
                let (east, north) = if grid.field_is_planar() {
                    (DVec3::X, DVec3::Y)
                } else {
                    UnitVec3::from_vec(source.position).unwrap().tangent_frame()
                };
                let bearing = rng.range_f64(0., std::f64::consts::TAU);
                let along = east * bearing.cos() + north * bearing.sin();
                history.volcanoes.push(Volcano {
                    position_m: move_point(state, plate_id as usize, source.position, age)
                        .to_array(),
                    plate_id,
                    age_myr: age,
                    radius_m: rng.range_f64(22_000., 90_000.),
                    summit_growth_m: rng.range_f64(2400., 8800.),
                    origin: "hotspot".into(),
                    along_arc: along.to_array(),
                    arc_platform_fraction: 0.,
                    shape_seed: source.seed ^ source.episode,
                    quiet_myr: age,
                    preserved_growth_m: None,
                });
            }
            source.episode += 1;
            // Each mantle source has its own episodic clock and quiet periods.
            // Event spacing follows plate travel during these unequal intervals.
            source.next_eruption += rng.range_f64(1.2, 9.5);
        }
    }
}

pub fn rotate(point: DVec3, axis: DVec3, radians: f64) -> DVec3 {
    point * radians.cos()
        + axis.cross(point) * radians.sin()
        + axis * axis.dot(point) * (1. - radians.cos())
}

pub fn velocity(state: &TectonicState, plate: usize, position_m: DVec3) -> DVec3 {
    if let Some(v) = &state.planar_velocities {
        return v[plate];
    }
    let p = &state.plates[plate];
    DVec3::from_array(p.euler_pole).cross(position_m) * p.omega
}

fn move_point(state: &TectonicState, plate: usize, p: DVec3, dt: f64) -> DVec3 {
    if let Some(v) = &state.planar_velocities {
        p + v[plate] * dt
    } else {
        let plate = &state.plates[plate];
        rotate(p, DVec3::from_array(plate.euler_pole), plate.omega * dt)
    }
}

fn locate(grid: &MapGrid, p: DVec3) -> Option<usize> {
    match grid {
        MapGrid::Plane(g) => g.cell_at_xy(p.x, p.y),
        _ => grid.cell_at(UnitVec3::from_vec(p)?),
    }
}

/// Piecewise linear nodal stencil; weights are positive and sum to one.
/// Forward application conserves transported volume even for unequal cell areas.
pub(crate) fn stencil(grid: &MapGrid, p: DVec3) -> Option<[(usize, f64); 3]> {
    let i = locate(grid, p)?;
    let a = grid.field_position(i);
    let delta = p - a;
    if delta.length_squared() < 1e-12 {
        return Some([(i, 1.), (i, 0.), (i, 0.)]);
    }
    let neighbors = grid.neighbors(i);
    for &j in neighbors {
        for &k in neighbors {
            if k <= j || !grid.neighbors(j as usize).contains(&k) {
                continue;
            }
            let b = grid.field_position(j as usize) - a;
            let c = grid.field_position(k as usize) - a;
            let (bb, bc, cc) = (b.dot(b), b.dot(c), c.dot(c));
            let den = bb * cc - bc * bc;
            if den.abs() < 1e-12 {
                continue;
            }
            let v = (cc * delta.dot(b) - bc * delta.dot(c)) / den;
            let w = (bb * delta.dot(c) - bc * delta.dot(b)) / den;
            let u = 1. - v - w;
            if u.min(v).min(w) >= -0.002 {
                let weights = [u.max(0.), v.max(0.), w.max(0.)];
                let total = weights.iter().sum::<f64>();
                return Some([
                    (i, weights[0] / total),
                    (j as usize, weights[1] / total),
                    (k as usize, weights[2] / total),
                ]);
            }
        }
    }
    Some([(i, 1.), (i, 0.), (i, 0.)])
}

fn solver_grid<G: GridOps>(fine: &G) -> MapGrid {
    if let Some((width, height)) = fine.extent_m() {
        let mid = (0..fine.cell_count())
            .min_by(|&a, &b| {
                fine.local_center_m(a)
                    .unwrap_or_default()
                    .0
                    .hypot(fine.local_center_m(a).unwrap_or_default().1)
                    .total_cmp(
                        &fine
                            .local_center_m(b)
                            .unwrap_or_default()
                            .0
                            .hypot(fine.local_center_m(b).unwrap_or_default().1),
                    )
            })
            .unwrap_or(0);
        let (lat, lon) = fine.center_dir(mid).to_lat_lon_deg();
        let cols = ((width / 80_000.).round() as u32).clamp(16, 160);
        let rows =
            ((height / width * cols as f64 * 2. / 3_f64.sqrt()).ceil() as u32 + 2).clamp(4, 164);
        MapGrid::Plane(FlatGrid::new(
            cols + 2,
            rows,
            width * (cols + 2) as f64 / cols as f64,
            lat,
            lon,
            fine.radius_m(),
        ))
    } else {
        let freq = ((fine.cell_count() as f64 / 10.).sqrt() as u32).clamp(8, 64);
        MapGrid::Sphere(SphereHexGrid::build(freq, fine.radius_m(), (DVec3::Z, 0.)))
    }
}

fn crust_volume<G: GridOps>(grid: &G, fields: &WorldFields) -> f64 {
    fields
        .crust_thickness
        .iter()
        .enumerate()
        .map(|(i, &h)| h as f64 * grid.area_m2(i))
        .sum()
}

/// Shared dual-face flux geometry. For a rigid spherical rotation, each cell's
/// signed fluxes telescope to zero: a uniform crust does not develop false rifts.
fn faces(grid: &MapGrid) -> Vec<Vec<(usize, DVec3)>> {
    (0..grid.cell_count())
        .map(|i| {
            let p = grid.field_position(i);
            grid.neighbors(i)
                .iter()
                .map(|&j| {
                    let j = j as usize;
                    let q = grid.field_position(j);
                    let metric = if grid.field_is_planar() {
                        let d = grid.distance_m(i, j);
                        (q - p).normalize() * (grid.area_m2(i) + grid.area_m2(j)) / (3. * d)
                    } else {
                        let common: Vec<_> = grid
                            .neighbors(i)
                            .iter()
                            .copied()
                            .filter(|k| grid.neighbors(j).contains(k))
                            .collect();
                        let a = (p + q + grid.field_position(common[0] as usize)).normalize();
                        let b = (p + q + grid.field_position(common[1] as usize)).normalize();
                        let mut edge = b - a;
                        if edge.cross((p + q).normalize()).dot(q - p) < 0. {
                            edge = -edge;
                        }
                        edge * grid.radius_m().powi(2)
                    };
                    (j, metric)
                })
                .collect()
        })
        .collect()
}

fn transport_parameter(state: &TectonicState, id: usize) -> DVec3 {
    state.planar_velocities.as_ref().map_or_else(
        || DVec3::from_array(state.plates[id].euler_pole) * state.plates[id].omega,
        |v| v[id],
    )
}

/// Transfer crustal volume and tracer moments to the moved positions, then account
/// explicitly for newly exposed mantle and sinking oceanic material.
struct Reconstruction {
    value: f64,
    gradient: DVec3,
    low: f64,
    high: f64,
}
impl Reconstruction {
    fn sample(&self, offset: DVec3) -> f64 {
        (self.value + self.gradient.dot(offset)).clamp(self.low, self.high)
    }
}

/// Least-squares gradients with a local extrema limiter on the physical mesh.
fn reconstruction(grid: &MapGrid, values: &[f64]) -> Vec<Reconstruction> {
    (0..grid.cell_count())
        .map(|i| {
            let (east, north) = if grid.field_is_planar() {
                (DVec3::X, DVec3::Y)
            } else {
                grid.center_dir(i).tangent_frame()
            };
            let p = grid.field_position(i);
            let (mut xx, mut xy, mut yy, mut vx, mut vy) = (0., 0., 0., 0., 0.);
            let (mut low, mut high) = (values[i], values[i]);
            for &j in grid.neighbors(i) {
                let j = j as usize;
                let d = grid.field_position(j) - p;
                let (x, y) = (d.dot(east), d.dot(north));
                let weight = 1. / d.length_squared().max(1.);
                xx += x * x * weight;
                xy += x * y * weight;
                yy += y * y * weight;
                vx += x * (values[j] - values[i]) * weight;
                vy += y * (values[j] - values[i]) * weight;
                low = low.min(values[j]);
                high = high.max(values[j]);
            }
            let determinant = xx * yy - xy * xy;
            let mut gradient = if determinant > 1e-12 {
                east * ((yy * vx - xy * vy) / determinant)
                    + north * ((xx * vy - xy * vx) / determinant)
            } else {
                DVec3::ZERO
            };
            let mut limit = 1_f64;
            for &j in grid.neighbors(i) {
                let delta = gradient.dot((grid.field_position(j as usize) - p) * 0.5);
                if delta > 1e-20 {
                    limit = limit.min((high - values[i]) / delta);
                } else if delta < -1e-20 {
                    limit = limit.min((low - values[i]) / delta);
                }
            }
            gradient *= limit.clamp(0., 1.);
            Reconstruction {
                value: values[i],
                gradient,
                low,
                high,
            }
        })
        .collect()
}

fn flux_limit(flat: f64, proposed: f64, low: f64, high: f64) -> f64 {
    let delta = proposed - flat;
    if delta > 1e-20 {
        ((high - flat) / delta).clamp(0., 1.)
    } else if delta < -1e-20 {
        ((low - flat) / delta).clamp(0., 1.)
    } else {
        1.
    }
}

fn remap(
    grid: &MapGrid,
    geometry: &[Vec<(usize, DVec3)>],
    f: &mut WorldFields,
    state: &TectonicState,
    dt: f64,
    history: &mut History,
) {
    let n = grid.cell_count();
    let count = state.plates.len();
    let mut coverage = vec![0.; n];
    let mut volume = vec![0.; n];
    let mut continental_volume = vec![0.; n];
    let mut reference = vec![0.; n];
    let mut basement = vec![0.; n];
    let mut age = vec![0.; n];
    let mut strain = vec![0.; n];
    let mut orogen_age = vec![0.; n];
    let mut shear_strain = vec![0.; n];
    let mut votes = vec![0_f64; n * count];
    let thickness: Vec<_> = f.crust_thickness.iter().map(|v| *v as f64).collect();
    let reference_height: Vec<_> = f
        .reference_crust_thickness
        .iter()
        .map(|v| *v as f64)
        .collect();
    let continental: Vec<_> = thickness
        .iter()
        .zip(&f.crust_type)
        .map(|(h, c)| h * (*c as f64))
        .collect();
    let rh = reconstruction(grid, &thickness);
    let rr = reconstruction(grid, &reference_height);
    let rc = reconstruction(grid, &continental);
    for i in 0..n {
        let area = grid.area_m2(i);
        let plate = f.plate_id[i] as usize;
        let motion = transport_parameter(state, plate);
        // Face fluxes carry reconstructed rock densities, with a half-step
        // predictor. The retained mass is the exact donor remainder.
        let p = grid.field_position(i);
        let predictor = velocity(state, plate, p) * dt * 0.5;
        let mut shares: Vec<_> = geometry[i]
            .iter()
            .map(|&(j, metric)| {
                (
                    Some(j),
                    motion.dot(metric).max(0.) * dt,
                    (grid.field_position(j) - p) * 0.5 - predictor,
                )
            })
            .filter(|(_, a, _)| *a > 0.)
            .collect();
        let mut inward = 0.;
        if grid.field_is_planar() && grid.neighbors(i).len() < 6 {
            let spacing = grid.distance_m(i, grid.neighbors(i)[0] as usize);
            let p = grid.field_position(i);
            for (dx, dy) in [
                (1., 0.),
                (-1., 0.),
                (0.5, 3_f64.sqrt() / 2.),
                (-0.5, 3_f64.sqrt() / 2.),
                (0.5, -3_f64.sqrt() / 2.),
                (-0.5, -3_f64.sqrt() / 2.),
            ] {
                let direction = DVec3::new(dx, dy, 0.);
                if !geometry[i].iter().any(|&(j, _)| {
                    (grid.field_position(j) - p - direction * spacing).length() < spacing * 0.1
                }) {
                    let flux = motion.dot(direction) * area * 2. / (3. * spacing) * dt;
                    if flux > 0. {
                        shares.push((None, flux, direction * spacing * 0.5 - predictor));
                    }
                    inward += (-flux).max(0.);
                }
            }
        }
        // Open boundaries import an explicitly prescribed old oceanic column.
        // Incoming material is not labelled as newly created ridge/mantle crust.
        coverage[i] += inward;
        volume[i] += inward * 7000.;
        reference[i] += inward * 7000.;
        basement[i] -= inward * 2800.;
        age[i] += inward * 70.;
        votes[i * count + plate] += inward;
        history.imported_crust_m3 += inward * 7000.;
        let outgoing = shares.iter().map(|(_, a, _)| a).sum::<f64>();
        assert!(outgoing <= area * 0.51, "tectonic CFL violated");
        let retained_area = area - outgoing;
        let sum_h = shares
            .iter()
            .map(|(_, a, d)| rh[i].sample(*d) * a)
            .sum::<f64>();
        let sum_r = shares
            .iter()
            .map(|(_, a, d)| rr[i].sample(*d) * a)
            .sum::<f64>();
        let lh = flux_limit(
            thickness[i] * outgoing,
            sum_h,
            thickness[i] * area - rh[i].high * retained_area,
            thickness[i] * area - rh[i].low * retained_area,
        );
        let lr = flux_limit(
            reference_height[i] * outgoing,
            sum_r,
            reference_height[i] * area - rr[i].high * retained_area,
            reference_height[i] * area - rr[i].low * retained_area,
        );
        let hface = |d| thickness[i] + lh * (rh[i].sample(d) - thickness[i]);
        let rface = |d| reference_height[i] + lr * (rr[i].sample(d) - reference_height[i]);
        let out_h = shares.iter().map(|(_, a, d)| hface(*d) * a).sum::<f64>();
        let retained_h = (thickness[i] * area - out_h).max(0.);
        let fraction_c = f.crust_type[i] as f64;
        let out_c = shares
            .iter()
            .map(|(_, a, d)| rc[i].sample(*d).clamp(0., hface(*d)) * a)
            .sum::<f64>();
        let lc = flux_limit(
            fraction_c * out_h,
            out_c,
            (continental[i] * area - retained_h).max(0.),
            (continental[i] * area).min(out_h),
        );
        let mut kept_c = continental[i] * area;
        let mut kept_r = reference_height[i] * area;
        let mut transfers = Vec::with_capacity(shares.len() + 1);
        for (j, a, d) in shares {
            let h = hface(d) * a;
            let r = rface(d) * a;
            let c = (fraction_c * hface(d)
                + lc * (rc[i].sample(d).clamp(0., hface(d)) - fraction_c * hface(d)))
                * a;
            kept_c -= c;
            kept_r -= r;
            if let Some(j) = j {
                transfers.push((j, a, h, c, r));
            } else {
                history.exported_crust_m3 += h;
            }
        }
        transfers.push((i, retained_area, retained_h, kept_c.max(0.), kept_r.max(0.)));
        for (j, share, h, c, r) in transfers {
            coverage[j] += share;
            volume[j] += h;
            continental_volume[j] += c;
            reference[j] += r;
            basement[j] += f.elevation[i] as f64 * share;
            age[j] += f.crust_age[i] as f64 * share;
            strain[j] += f.crustal_strain[i] as f64 * share;
            orogen_age[j] += f.orogen_age[i] as f64 * share;
            shear_strain[j] += f.shear_strain[i] as f64 * share;
            votes[j * count + plate] += share;
        }
    }
    for i in 0..n {
        let area = grid.area_m2(i);
        let fraction = coverage[i] / area;
        let missing = (area - coverage[i]).max(0.);
        let new_volume = missing * 7000.;
        history.new_ocean_crust_m3 += new_volume;
        volume[i] += new_volume;
        reference[i] += new_volume;
        basement[i] -= missing * 2800.;
        let total_area = coverage[i] + missing;
        let ref_h = reference[i] / total_area.max(1.);
        let ocean_volume = (volume[i] - continental_volume[i]).max(0.);
        let excess = (volume[i] - ref_h * area).max(0.);
        let sinking = ocean_volume.min(excess);
        volume[i] -= sinking;
        history.subducted_crust_m3 += sinking;
        f.crust_thickness[i] = (volume[i] / area) as f32;
        f.reference_crust_thickness[i] = ref_h as f32;
        f.elevation[i] = (basement[i] / total_area.max(1.)) as f32;
        f.crust_type[i] = (continental_volume[i] / volume[i].max(1.)).clamp(0., 1.) as f32;
        f.shear_strain[i] = (shear_strain[i] / total_area.max(1.)) as f32;
        f.crust_age[i] = (age[i] / total_area.max(1.) + dt * fraction.min(1.)) as f32;
        f.crustal_strain[i] = (strain[i] / total_area.max(1.)
            + fraction.max(1.).ln() * f.crust_type[i] as f64) as f32;
        f.orogen_age[i] = if fraction > 1.015 && f.crust_type[i] > 0.5 {
            0.
        } else {
            (orogen_age[i] / total_area.max(1.) + dt) as f32
        };
        f.plate_id[i] = votes[i * count..(i + 1) * count]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(p, _)| p as u16);
        if coverage[i] < 1. {
            let position = grid.field_position(i);
            f.plate_id[i] = state
                .centers
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    (position - **a)
                        .length_squared()
                        .total_cmp(&(position - **b).length_squared())
                })
                .unwrap()
                .0 as u16;
        }
    }
    // Conservative lower-crust redistribution: broadens a thick root without
    // creating extra rock. The relaxation length increases with accumulated strain.
    for _ in 0..8 {
        let mut delta = vec![0_f64; n];
        let mut continental_delta = vec![0_f64; n];
        for i in 0..n {
            for &j in grid.neighbors(i) {
                let j = j as usize;
                if j <= i {
                    continue;
                }
                let excess_i =
                    (f.crust_thickness[i] - f.reference_crust_thickness[i]).max(0.) as f64;
                let excess_j =
                    (f.crust_thickness[j] - f.reference_crust_thickness[j]).max(0.) as f64;
                let difference = excess_i - excess_j;
                let (donor, receiver) = if difference >= 0. { (i, j) } else { (j, i) };
                let mobility = (f.crust_type[donor] as f64).powi(2)
                    * (f.crust_thickness[donor] as f64 / 35000.).max(1.).powi(3);
                let transport = 1.5e10 * mobility * dt / 8. * grid.area_m2(i).min(grid.area_m2(j))
                    / grid.distance_m(i, j).powi(2)
                    * difference.abs();
                let available = (f.crust_thickness[donor]
                    - f.reference_crust_thickness[donor] * 0.5)
                    .max(0.) as f64
                    * grid.area_m2(donor)
                    * 0.025;
                let flow = transport.min(available);
                delta[donor] -= flow;
                delta[receiver] += flow;
                let continental_flow = flow * f.crust_type[donor] as f64;
                continental_delta[donor] -= continental_flow;
                continental_delta[receiver] += continental_flow;
            }
        }
        for i in 0..n {
            let old_volume = f.crust_thickness[i] as f64 * grid.area_m2(i);
            let volume = old_volume + delta[i];
            f.crust_type[i] = ((old_volume * f.crust_type[i] as f64 + continental_delta[i])
                / volume.max(1.))
            .clamp(0., 1.) as f32;
            f.crust_thickness[i] = (volume / grid.area_m2(i)) as f32;
        }
    }
}

fn volcano_events(
    grid: &MapGrid,
    f: &WorldFields,
    state: &TectonicState,
    history: &mut History,
    elapsed: f64,
    seed: u64,
) {
    let mut rng = DeterministicRng::derive(seed ^ elapsed.to_bits(), "volcanic-events");
    let boundaries = crate::plates::find_boundaries(grid, &f.plate_id, state, f);
    let mut candidates = Vec::new();
    let productivity = tf_core::noise::SimplexNoise::new(seed ^ 0xA7C0_1440);
    for b in boundaries {
        if b.kind != crate::BoundaryKind::Convergent || b.normal_velocity < 8000. {
            continue;
        }
        let (i, j) = (b.cell as usize, b.other as usize);
        if f.crust_type[i] > 0.6 && f.crust_type[j] > 0.6 {
            continue;
        }
        let upper = if f.crust_type[i] > f.crust_type[j] + 0.1 {
            i
        } else if f.crust_type[j] > f.crust_type[i] + 0.1 {
            j
        } else if f.crust_age[i] < f.crust_age[j] {
            i
        } else {
            j
        };
        let sign = if upper == i { -1. } else { 1. };
        let midpoint = (grid.field_position(i) + grid.field_position(j)) * 0.5;
        let mut p = midpoint + DVec3::from_array(b.normal) * sign * rng.range_f64(100000., 220000.);
        if !grid.field_is_planar() {
            p = p.normalize() * grid.radius_m();
        }
        if locate(grid, p).is_none() {
            continue;
        }
        // Along-strike magma supply varies coherently; not every grid face grows
        // an identical cone at every epoch. This is an explicit stochastic proxy.
        let supply = productivity.fbm(
            p.x / 900000. + elapsed / 100.,
            p.y / 900000.,
            p.z / 900000.,
            3,
            2.,
            0.5,
        );
        if supply < -0.06 {
            continue;
        }
        let normal = DVec3::from_array(b.normal);
        let along = if grid.field_is_planar() {
            DVec3::Z.cross(normal)
        } else {
            p.normalize().cross(normal)
        }
        .normalize_or_zero();
        let radius = rng.range_f64(18_000., 65_000.) * (1. + supply * 0.4);
        let priority = supply + (b.normal_velocity / 60_000.).min(1.) * 0.25;
        candidates.push((
            priority,
            Volcano {
                position_m: p.to_array(),
                plate_id: f.plate_id[upper],
                age_myr: 0.,
                radius_m: radius,
                summit_growth_m: if f.crust_type[upper] < 0.25 {
                    rng.range_f64(2800., 8500.)
                } else {
                    rng.range_f64(700., 1800.)
                },
                origin: "subduction_arc".into(),
                along_arc: along.to_array(),
                arc_platform_fraction: 0.35,
                shape_seed: seed ^ elapsed.to_bits() ^ b.cell as u64,
                quiet_myr: 0.,
                preserved_growth_m: None,
            },
        ));
    }
    // Select productive segments first, rather than filling the front in cell
    // order. Edifice footprints set the spacing; no fixed 700-km exclusion belt.
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut chosen: Vec<usize> = Vec::new();
    for (_, candidate) in candidates {
        let p = DVec3::from_array(candidate.position_m);
        let overlaps = |v: &Volcano| {
            v.origin == "subduction_arc"
                && v.plate_id == candidate.plate_id
                && (DVec3::from_array(v.position_m) - p).length()
                    < (v.radius_m + candidate.radius_m) * 2.5
        };
        if chosen.iter().any(|&i| overlaps(&history.volcanoes[i])) {
            continue;
        }
        let nearby = history
            .volcanoes
            .iter()
            .enumerate()
            .filter(|(_, v)| overlaps(v))
            .min_by(|(_, a), (_, b)| {
                (DVec3::from_array(a.position_m) - p)
                    .length_squared()
                    .total_cmp(&(DVec3::from_array(b.position_m) - p).length_squared())
            })
            .map(|(i, _)| i);
        if let Some(i) = nearby {
            let v = &mut history.volcanoes[i];
            // Recharge an existing centre, retaining its original birth and
            // plate trajectory. Growth is a bounded procedural magma proxy.
            if v.quiet_myr >= 4. {
                v.preserved_growth_m =
                    Some((volcano_amplitude(v) + candidate.summit_growth_m * 0.12).min(9500.));
                v.summit_growth_m =
                    (v.summit_growth_m + candidate.summit_growth_m * 0.12).min(9500.);
                v.quiet_myr = 0.;
            }
            chosen.push(i);
        } else {
            chosen.push(history.volcanoes.len());
            history.volcanoes.push(candidate);
        }
    }
}

/// Continental collision resists normal motion; oceanic subduction remains mobile.
/// This kinematic resistance is explicit, not a force-balanced mantle model.
fn collision_resistance(grid: &MapGrid, f: &mut WorldFields, state: &mut TectonicState, dt: f64) {
    let boundaries = crate::plates::find_boundaries(grid, &f.plate_id, state, f);
    let mut change = vec![DVec3::ZERO; state.plates.len()];
    let mut weight = vec![0.; state.plates.len()];
    let factor = 1. - (-dt / 12.).exp();
    for b in boundaries {
        let (i, j) = (b.cell as usize, b.other as usize);
        if b.kind == crate::BoundaryKind::Transform {
            let strain = (b.tangential_velocity * dt / grid.distance_m(i, j) * 0.5) as f32;
            f.shear_strain[i] += strain;
            f.shear_strain[j] += strain;
        }
        if b.normal_velocity <= 0. || f.crust_type[i].min(f.crust_type[j]) < 0.65 {
            continue;
        }
        let a = f.plate_id[i] as usize;
        let c = f.plate_id[j] as usize;
        let normal = DVec3::from_array(b.normal);
        let correction = if grid.field_is_planar() {
            normal * b.normal_velocity * 0.5 * factor
        } else {
            ((grid.field_position(i) + grid.field_position(j))
                .normalize()
                .cross(normal))
                * b.normal_velocity
                / grid.radius_m()
                * 0.5
                * factor
        };
        change[a] -= correction;
        change[c] += correction;
        weight[a] += 1.;
        weight[c] += 1.;
    }
    if let Some(v) = &mut state.planar_velocities {
        for i in 0..v.len() {
            if weight[i] > 0. {
                v[i] += change[i] / weight[i];
            }
        }
    } else {
        for i in 0..state.plates.len() {
            if weight[i] > 0. {
                let p = &mut state.plates[i];
                let omega = DVec3::from_array(p.euler_pole) * p.omega + change[i] / weight[i];
                p.omega = omega.length();
                if p.omega > 1e-12 {
                    p.euler_pole = omega.normalize().to_array();
                }
            }
        }
    }
}

pub fn generate<G: GridOps>(
    fine: &G,
    params: &GenerationParams,
    fields: &mut WorldFields,
    progress: &mut dyn FnMut(f64),
    cancel: &dyn Fn() -> bool,
) -> Option<TectonicState> {
    let grid = solver_grid(fine);
    let mut rng = DeterministicRng::derive(params.seed, "tectonics");
    let (mut state, ids) = crate::plates::build_plates(&grid, params, &mut rng);
    let mut material = WorldFields::with_capacity(grid.cell_count());
    crate::plates::init_crust(&grid, &ids, &state, &mut material, &mut rng, params);
    let age_noise = tf_core::noise::SimplexNoise::new(params.seed ^ 0xA6E0_0410);
    for i in 0..grid.cell_count() {
        let p = grid.field_position(i) / 1_800_000.;
        let ocean_age = 70. + 50. * age_noise.fbm(p.x, p.y, p.z, 3, 2., 0.5);
        material.crust_age[i] = (600. * material.crust_type[i] as f64
            + ocean_age * (1. - material.crust_type[i] as f64))
            as f32;
    }
    for plate in &mut state.plates {
        let mut crust = 0.;
        let mut area = 0.;
        for i in 0..grid.cell_count() {
            if material.plate_id[i] == plate.id {
                let a = grid.area_m2(i);
                crust += material.crust_type[i] as f64 * a;
                area += a;
            }
        }
        plate.oceanic = crust < area * 0.5;
        if plate.oceanic {
            plate.omega *= params.spreading_rate;
            if let Some(v) = &mut state.planar_velocities {
                v[plate.id as usize] *= params.spreading_rate;
            }
        }
    }
    material.orogen_age.fill(150.);
    let mut history = History {
        model: "muscl-conservative-terrane+episodic-volcanism-v2".into(),
        initial_crust_m3: crust_volume(&grid, &material),
        duration_myr: params.tectonic_age_myr,
        ..Default::default()
    };
    let geometry = faces(&grid);
    // A norm bound covers every plate orientation, including resistance updates.
    let geometry_rate = (0..grid.cell_count())
        .map(|i| {
            if grid.field_is_planar() {
                3. / grid.distance_m(i, grid.neighbors(i)[0] as usize)
            } else {
                geometry[i].iter().map(|(_, m)| m.length()).sum::<f64>() / grid.area_m2(i)
            }
        })
        .fold(0., f64::max);
    let mut hot_rng = DeterministicRng::derive(params.seed, "mantle-hotspots");
    let mut hotspots: Vec<_> = (0..params.hotspot_count)
        .map(|id| {
            let position = if let Some((w, h)) = grid.extent_m() {
                DVec3::new(
                    hot_rng.range_f64(-0.45, 0.45) * w,
                    hot_rng.range_f64(-0.45, 0.45) * h,
                    0.,
                )
            } else {
                hot_rng.unit_sphere().as_vec() * grid.radius_m()
            };
            Hotspot {
                position,
                next_eruption: hot_rng.range_f64(0.5, 10.),
                episode: 1,
                seed: params.seed ^ (id as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15),
            }
        })
        .collect();
    let mut arc_rng = DeterministicRng::derive(params.seed, "arc-epochs");
    let mut next_event = arc_rng.range_f64(3., 10.);
    let mut elapsed = 0.;
    while elapsed < params.tectonic_age_myr {
        if history.steps >= 4096 {
            return None;
        }
        let rate = (0..state.plates.len())
            .map(|id| transport_parameter(&state, id).length())
            .fold(0., f64::max)
            * geometry_rate;
        let dt = (0.32 / rate.max(1e-12))
            .min(1.5)
            .min(params.tectonic_age_myr - elapsed);
        if cancel() {
            return None;
        }
        for v in &mut history.volcanoes {
            if state.planar_velocities.is_none() {
                let plate = &state.plates[v.plate_id as usize];
                v.along_arc = rotate(
                    DVec3::from_array(v.along_arc),
                    DVec3::from_array(plate.euler_pole),
                    plate.omega * dt,
                )
                .to_array();
            }
            v.position_m = move_point(
                &state,
                v.plate_id as usize,
                DVec3::from_array(v.position_m),
                dt,
            )
            .to_array();
            v.age_myr += dt;
            v.quiet_myr += dt;
            if let Some(height) = &mut v.preserved_growth_m {
                *height *= (-dt / 28.).exp();
            }
        }
        remap(&grid, &geometry, &mut material, &state, dt, &mut history);
        collision_resistance(&grid, &mut material, &mut state, dt);
        for i in 0..state.centers.len() {
            state.centers[i] = move_point(&state, i, state.centers[i], dt);
        }
        elapsed += dt;
        history.steps += 1;
        hotspot_events(
            &grid,
            &material,
            &state,
            &mut history,
            &mut hotspots,
            elapsed,
        );
        if elapsed >= next_event {
            volcano_events(
                &grid,
                &material,
                &state,
                &mut history,
                next_event,
                params.seed,
            );
            next_event += arc_rng.range_f64(4., 11.);
        }
        progress(elapsed / params.tectonic_age_myr);
    }
    history.final_crust_m3 = crust_volume(&grid, &material);
    let expected =
        history.initial_crust_m3 + history.imported_crust_m3 + history.new_ocean_crust_m3
            - history.subducted_crust_m3
            - history.exported_crust_m3;
    history.relative_budget_error =
        (history.final_crust_m3 - expected).abs() / history.initial_crust_m3.max(1.);
    for i in 0..fine.cell_count() {
        if i % 2048 == 0 && cancel() {
            return None;
        }
        let weights = stencil(&grid, fine.field_position(i))?;
        let interpolate = |values: &[f32]| {
            weights
                .iter()
                .map(|&(j, w)| values[j] as f64 * w)
                .sum::<f64>() as f32
        };
        fields.elevation[i] = interpolate(&material.elevation);
        fields.crust_thickness[i] = interpolate(&material.crust_thickness);
        fields.reference_crust_thickness[i] = interpolate(&material.reference_crust_thickness);
        fields.crust_type[i] = interpolate(&material.crust_type);
        fields.crust_age[i] = interpolate(&material.crust_age);
        fields.crustal_strain[i] = interpolate(&material.crustal_strain);
        fields.orogen_age[i] = interpolate(&material.orogen_age);
        fields.shear_strain[i] = interpolate(&material.shear_strain);
        let nearest = weights.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap().0;
        fields.plate_id[i] = material.plate_id[nearest];
    }
    // Spherical nodal interpolation is not conservative by itself. Correct each
    // material component's integral, preserving continental and oceanic volume.
    if !fine.field_is_planar() && fine.extent_m().is_none() {
        let target_c = (0..grid.cell_count())
            .map(|i| {
                material.crust_thickness[i] as f64 * material.crust_type[i] as f64 * grid.area_m2(i)
            })
            .sum::<f64>();
        let target_o = history.final_crust_m3 - target_c;
        let actual_c = (0..fine.cell_count())
            .map(|i| {
                fields.crust_thickness[i] as f64 * fields.crust_type[i] as f64 * fine.area_m2(i)
            })
            .sum::<f64>();
        let actual_o = crust_volume(fine, fields) - actual_c;
        let (sc, so) = (target_c / actual_c.max(1.), target_o / actual_o.max(1.));
        for i in 0..fine.cell_count() {
            let c = fields.crust_thickness[i] as f64 * fields.crust_type[i] as f64 * sc;
            let o = fields.crust_thickness[i] as f64 * (1. - fields.crust_type[i] as f64) * so;
            fields.crust_thickness[i] = (c + o) as f32;
            fields.crust_type[i] = (c / (c + o).max(1.)) as f32;
        }
        history.output_projection_error = Some(
            (crust_volume(fine, fields) - history.final_crust_m3).abs()
                / history.final_crust_m3.max(1.),
        );
    }
    history.output_crust_m3 = crust_volume(fine, fields);
    state.history = history;
    for plate in &mut state.plates {
        let mut crust = 0.;
        let mut total = 0.;
        for (i, &id) in fields.plate_id.iter().enumerate() {
            if id == plate.id {
                crust += fields.crust_type[i] as f64 * fine.area_m2(i);
                total += fine.area_m2(i);
            }
        }
        plate.oceanic = crust < total * 0.5;
    }
    Some(state)
}

/// End-state terrain uses distributed roots and preserved events, not current-border ribbons.
pub fn landforms<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    state: &TectonicState,
    params: &GenerationParams,
) {
    let noise = tf_core::noise::SimplexNoise::new(params.seed ^ 0x0ED0_6E17);
    let sampling = (grid.area_m2(0) * 2. / 3_f64.sqrt()).sqrt();
    let axis_ratio = |v: &Volcano| {
        if v.shape_seed == 0 {
            1.
        } else {
            0.7 + (v.shape_seed % 1009) as f64 / 1009. * 0.8
        }
    };
    let support = |v: &Volcano| {
        let ratio = axis_ratio(v);
        (v.radius_m * ratio.max(1. / ratio)).max(if v.arc_platform_fraction > 0. {
            80000.
        } else {
            0.
        }) * 4.
    };
    let bin_size = state
        .history
        .volcanoes
        .iter()
        .map(support)
        .fold(400000., f64::max);
    let key = |p: DVec3| {
        (
            (p.x / bin_size).floor() as i32,
            (p.y / bin_size).floor() as i32,
            (p.z / bin_size).floor() as i32,
        )
    };
    let mut bins: std::collections::HashMap<_, Vec<&Volcano>> = std::collections::HashMap::new();
    for v in &state.history.volcanoes {
        bins.entry(key(DVec3::from_array(v.position_m)))
            .or_default()
            .push(v);
    }
    for i in 0..grid.cell_count() {
        let p = grid.field_position(i);
        let c = fields.crust_type[i] as f64;
        let age = fields.crust_age[i] as f64;
        let excess = fields.crust_thickness[i] as f64 - fields.reference_crust_thickness[i] as f64;
        let uplift = excess.max(0.) * (1. - 2800. / 3300.) * params.orogeny_strength;
        let rift = excess.min(0.) * (1. - 2800. / 3300.) * params.rift_strength;
        let ocean = -2700. - 330. * age.max(0.).min(180.).sqrt();
        let basement = fields.elevation[i] as f64;
        let strain = fields.crustal_strain[i] as f64;
        let detail = crate::terrain::interior_detail(&noise, p, sampling) * params.interior_relief;
        let mut gradient = DVec3::ZERO;
        for &j in grid.neighbors(i) {
            let j = j as usize;
            let root =
                fields.crust_thickness[j] as f64 - fields.reference_crust_thickness[j] as f64;
            gradient += (grid.field_position(j) - p).normalize_or_zero() * (root - excess)
                / grid.distance_m(i, j).max(1.);
        }
        let up = if grid.field_is_planar() {
            DVec3::Z
        } else {
            p.normalize()
        };
        let strike = up.cross(gradient).normalize_or_zero();
        let folded =
            crate::terrain::orogenic_detail(&noise, p, strike, sampling) * params.interior_relief;
        // Old orogens lose relief gradually. Width emerges from transported material
        // and lateral crustal redistribution; local folds have no global fixed wavelength.
        let old_decay = (-fields.orogen_age[i] as f64 / 180.).exp();
        let folds = detail * 0.35
            + folded
                * ((strain * 2.).min(1.5) * old_decay
                    + (fields.shear_strain[i] as f64).min(1.) * 0.35 * params.shear_strength);
        let mut height = ocean * (1. - c) + (basement + uplift + rift + folds) * c;
        let (kx, ky, kz) = key(p);
        for x in kx - 1..=kx + 1 {
            for y in ky - 1..=ky + 1 {
                for z in kz - 1..=kz + 1 {
                    if let Some(volcanoes) = bins.get(&(x, y, z)) {
                        for v in volcanoes {
                            let q = DVec3::from_array(v.position_m);
                            let distance = if grid.field_is_planar() {
                                (p - q).length()
                            } else {
                                UnitVec3::from_vec(p).unwrap().arc_distance_m(
                                    &UnitVec3::from_vec(q).unwrap(),
                                    grid.radius_m(),
                                )
                            };
                            let radius = v.radius_m;
                            if distance > support(v) {
                                continue;
                            }
                            let amplitude = volcano_amplitude(v);
                            let along = DVec3::from_array(v.along_arc);
                            let delta = p - q;
                            let longitudinal = delta.dot(along);
                            let transverse2 =
                                (distance * distance - longitudinal * longitudinal).max(0.);
                            let ratio = axis_ratio(v);
                            let longitudinal_sigma =
                                (radius * ratio).powi(2) + sampling.powi(2) / 12.;
                            let transverse_sigma =
                                (radius / ratio).powi(2) + sampling.powi(2) / 12.;
                            let sigma2 = radius * radius + sampling * sampling / 12.;
                            let phase = (v.shape_seed % 65521) as f64 * 0.013;
                            // Unequal construction and erosion on the flanks. The
                            // edifice still follows its transported geological centre.
                            let flank = if v.shape_seed == 0 {
                                1.
                            } else {
                                1. + 0.25
                                    * noise.fbm(
                                        delta.x / radius + phase,
                                        delta.y / radius,
                                        delta.z / radius,
                                        2,
                                        2.,
                                        0.5,
                                    )
                            };
                            let cone = amplitude * radius * radius
                                / (longitudinal_sigma * transverse_sigma).sqrt()
                                * (-0.5
                                    * (longitudinal.powi(2) / longitudinal_sigma
                                        + transverse2 / transverse_sigma))
                                    .exp()
                                * flank;
                            let long_radius = (radius * 2.5).min(80000.);
                            let platform = amplitude
                                * (-0.5
                                    * (longitudinal * longitudinal / (long_radius * long_radius)
                                        + transverse2 / sigma2))
                                    .exp();
                            height += cone * (1. - v.arc_platform_fraction)
                                + platform * v.arc_platform_fraction;
                            // Lithospheric flexure beneath the volcanic load, adapted from Landlab.
                            let alpha = crate::scientific::flexure_length(25000., 7e10, 33000.);
                            let line_load = 2800. * 9.81 * amplitude * radius * 0.35;
                            height -= crate::scientific::line_load_deflection(
                                line_load, distance, alpha, 33000.,
                            ) * 0.20;
                        }
                    }
                }
            }
        }
        fields.elevation[i] = height as f32;
    }
    crate::plates::adjust_sea_level(grid, fields, params.land_fraction);
    crate::terrain::apply_planar_coast_constraint(grid, fields, params);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn episodic_hotspot_chain_has_irregular_ages_and_keeps_its_motion_at_finer_steps() {
        let g = MapGrid::Plane(FlatGrid::new(32, 16, 10_000_000., 25., 0., 6371000.));
        let f = WorldFields::with_capacity(g.cell_count());
        let state = TectonicState {
            plates: vec![crate::Plate {
                id: 0,
                euler_pole: DVec3::Y.to_array(),
                omega: 0.,
                oceanic: true,
            }],
            planar_velocities: Some(vec![DVec3::X * 40_000.]),
            ..Default::default()
        };
        let run = |dt: f64| {
            let mut sources = vec![Hotspot {
                position: DVec3::ZERO,
                next_eruption: 2.3,
                episode: 1,
                seed: 42,
            }];
            let mut history = History::default();
            for step in 1..=(80. / dt) as usize {
                for v in &mut history.volcanoes {
                    v.age_myr += dt;
                    v.position_m[0] += 40_000. * dt;
                }
                hotspot_events(&g, &f, &state, &mut history, &mut sources, step as f64 * dt);
            }
            history.volcanoes
        };
        let (coarse, fine) = (run(1.), run(0.5));
        assert!(coarse.len() > 8);
        assert_eq!(coarse.len(), fine.len());
        let intervals: Vec<_> = coarse
            .windows(2)
            .map(|v| v[0].age_myr - v[1].age_myr)
            .collect();
        assert!(
            intervals.iter().copied().fold(0_f64, f64::max)
                > intervals.iter().copied().fold(f64::INFINITY, f64::min) * 2.
        );
        for (a, b) in coarse.iter().zip(&fine) {
            assert!((a.position_m[0] - a.age_myr * 40_000.).abs() < 1e-7);
            assert!((a.position_m[0] - b.position_m[0]).abs() < 1e-7);
            assert!((a.age_myr - b.age_myr).abs() < 1e-9);
            assert_eq!(a.radius_m, b.radius_m);
            assert_eq!(a.summit_growth_m, b.summit_growth_m);
        }
    }
    #[test]
    fn an_active_subduction_front_recharges_its_existing_centres() {
        let g = MapGrid::Plane(FlatGrid::new(48, 36, 4_800_000., 25., 0., 6371000.));
        let mut f = WorldFields::with_capacity(g.cell_count());
        f.crust_type.fill(0.);
        f.crust_age.fill(70.);
        for i in 0..g.cell_count() {
            f.plate_id[i] = if g.field_position(i).x < 0. { 0 } else { 1 };
        }
        let state = TectonicState {
            plates: (0..2)
                .map(|id| crate::Plate {
                    id,
                    euler_pole: DVec3::Y.to_array(),
                    omega: 0.,
                    oceanic: true,
                })
                .collect(),
            planar_velocities: Some(vec![DVec3::X * 30_000., -DVec3::X * 30_000.]),
            centers: vec![-DVec3::X * 1_000_000., DVec3::X * 1_000_000.],
            ..Default::default()
        };
        let mut history = History::default();
        volcano_events(&g, &f, &state, &mut history, 10., 42);
        assert!(history.volcanoes.len() > 3);
        let original = history.volcanoes.clone();
        for v in &mut history.volcanoes {
            v.age_myr = 20.;
            v.quiet_myr = 10.;
        }
        // Hold the boundary and supply fixed to isolate centre reuse.
        volcano_events(&g, &f, &state, &mut history, 10., 42);
        assert_eq!(history.volcanoes.len(), original.len());
        assert!(history
            .volcanoes
            .iter()
            .zip(&original)
            .any(|(a, b)| a.summit_growth_m > b.summit_growth_m));
        for (a, b) in history.volcanoes.iter().zip(&original) {
            assert_eq!(a.position_m, b.position_m);
            assert_eq!(a.age_myr, 20.);
            assert!(a.summit_growth_m <= 9500.);
            assert!(volcano_amplitude(a) > b.summit_growth_m * (-20_f64 / 28.).exp());
        }
        let widths: Vec<_> = history.volcanoes.iter().map(|v| v.radius_m).collect();
        assert!(
            widths.iter().copied().fold(0., f64::max)
                > widths.iter().copied().fold(f64::INFINITY, f64::min) * 1.5
        );
    }
    #[test]
    fn smooth_crust_transport_converges_when_the_mesh_and_time_step_are_refined() {
        let error = |columns: usize, dt: f64| {
            let rows = columns / 4;
            let g = MapGrid::Plane(FlatGrid::new(
                columns as u32,
                rows as u32,
                10_000_000.,
                25.,
                0.,
                6371000.,
            ));
            let mut f = WorldFields::with_capacity(g.cell_count());
            let profile = |x: f64| 7000. + 1000. * (-(x / 500_000.).powi(2)).exp();
            for i in 0..g.cell_count() {
                f.crust_thickness[i] = profile(g.field_position(i).x + 1_000_000.) as f32;
                f.reference_crust_thickness[i] = f.crust_thickness[i] + 500.;
                f.crust_type[i] =
                    (0.2 + 0.6 * ((f.crust_thickness[i] as f64 - 7000.) / 1000.)) as f32;
            }
            let state = TectonicState {
                plates: vec![crate::Plate {
                    id: 0,
                    euler_pole: DVec3::Y.to_array(),
                    omega: 0.,
                    oceanic: false,
                }],
                planar_velocities: Some(vec![DVec3::X * 40_000.]),
                centers: vec![DVec3::ZERO],
                ..Default::default()
            };
            let geometry = faces(&g);
            let initial = crust_volume(&g, &f);
            let mut history = History::default();
            for _ in 0..(20. / dt).round() as usize {
                remap(&g, &geometry, &mut f, &state, dt, &mut history);
            }
            let expected_volume = initial + history.imported_crust_m3 + history.new_ocean_crust_m3
                - history.subducted_crust_m3
                - history.exported_crust_m3;
            assert!((crust_volume(&g, &f) - expected_volume).abs() / initial < 1e-6);
            let (mut difference, mut reference_error, mut phase_error, mut signal) =
                (0., 0., 0., 0.);
            for i in (rows / 2 - 2) * columns..(rows / 2 + 2) * columns {
                let x = g.field_position(i).x;
                if x.abs() < 3_000_000. {
                    let exact = profile(x + 200_000.);
                    difference += (f.crust_thickness[i] as f64 - exact).abs();
                    reference_error += (f.reference_crust_thickness[i] as f64 - exact - 500.).abs();
                    phase_error +=
                        ((f.crust_type[i] as f64 - 0.2) * (1000. / 0.6) - (exact - 7000.)).abs();
                    signal += exact - 7000.;
                }
            }
            [
                difference / signal,
                reference_error / signal,
                phase_error / signal,
            ]
        };
        let coarse = error(100, 0.25);
        let fine = error(200, 0.125);
        let half_dt = error(100, 0.125);
        for channel in 0..3 {
            assert!(
                fine[channel] < coarse[channel] * 0.7,
                "channel {channel}, coarse {coarse:?}, fine {fine:?}"
            );
            assert!(
                half_dt[channel] < coarse[channel] * 1.1,
                "channel {channel}, coarse {coarse:?}, half dt {half_dt:?}"
            );
        }
    }
    #[test]
    fn a_translated_coast_does_not_diffuse_into_a_wide_mixed_crust_belt() {
        let g = MapGrid::Plane(FlatGrid::new(100, 24, 10_000_000., 25., 0., 6371000.));
        let mut f = WorldFields::with_capacity(g.cell_count());
        f.crust_thickness.fill(35000.);
        f.reference_crust_thickness.fill(35000.);
        for i in 0..g.cell_count() {
            f.crust_type[i] = if g.field_position(i).x.abs() < 500_000. {
                1.
            } else {
                0.
            };
        }
        let state = TectonicState {
            plates: vec![crate::Plate {
                id: 0,
                euler_pole: DVec3::Y.to_array(),
                omega: 0.,
                oceanic: false,
            }],
            planar_velocities: Some(vec![DVec3::X * 40_000.]),
            centers: vec![DVec3::ZERO],
            ..Default::default()
        };
        let geometry = faces(&g);
        let mut history = History::default();
        for _ in 0..80 {
            remap(&g, &geometry, &mut f, &state, 0.25, &mut history);
        }
        for row in 6..18 {
            let mixed = (row * 100..(row + 1) * 100)
                .filter(|&i| f.crust_type[i] > 0.1 && f.crust_type[i] < 0.9)
                .count();
            assert!(
                mixed <= 6,
                "row {row}: translation smeared two sharp coasts across {mixed} cells"
            );
        }
        assert!(f.crust_type.iter().all(|v| *v >= 0. && *v <= 1.));
    }
    #[test]
    fn rigid_euler_rotation_keeps_radius_and_arc_distance() {
        let p = DVec3::X * 6371000.;
        let q = DVec3::Z * 6371000.;
        let a = rotate(p, DVec3::Y, 0.4);
        let b = rotate(q, DVec3::Y, 0.4);
        assert!((a.length() - p.length()).abs() < 1e-8);
        assert!((a.distance(b) - p.distance(q)).abs() < 1e-8);
    }
    #[test]
    fn forward_remap_has_an_explicit_closed_crust_budget() {
        let grid = MapGrid::Sphere(SphereHexGrid::build(12, 6371000., (DVec3::Z, 0.)));
        let mut p = GenerationParams::default();
        p.tectonic_age_myr = 12.;
        let mut f = WorldFields::with_capacity(grid.cell_count());
        let state = generate(&grid, &p, &mut f, &mut |_| {}, &|| false).unwrap();
        assert!(
            state.history.relative_budget_error < 1e-6,
            "{}",
            state.history.relative_budget_error
        );
        assert!(state.history.new_ocean_crust_m3 > 0. && state.history.subducted_crust_m3 > 0.);
        assert!(state.history.output_projection_error.unwrap() < 1e-6);
        assert!(f.crust_thickness.iter().all(|h| h.is_finite() && *h > 0.));
    }
    #[test]
    fn uniform_rigid_plate_does_not_create_fake_mountains_or_new_crust() {
        let grid = MapGrid::Sphere(SphereHexGrid::build(16, 6371000., (DVec3::Z, 0.)));
        let mut fields = WorldFields::with_capacity(grid.cell_count());
        fields.crust_thickness.fill(35000.);
        fields.reference_crust_thickness.fill(35000.);
        fields.crust_type.fill(1.);
        let state = TectonicState {
            plates: vec![crate::Plate {
                id: 0,
                euler_pole: DVec3::Y.to_array(),
                omega: 0.01,
                oceanic: false,
            }],
            ..Default::default()
        };
        let mut history = History::default();
        let geometry = faces(&grid);
        for _ in 0..20 {
            remap(&grid, &geometry, &mut fields, &state, 0.1, &mut history);
        }
        assert!(fields
            .crust_thickness
            .iter()
            .all(|&h| (h - 35000.).abs() < 0.1));
        assert!(history.new_ocean_crust_m3 < 1e-10 * crust_volume(&grid, &fields));
        assert!(history.subducted_crust_m3 < 1e-10 * crust_volume(&grid, &fields));
    }
    #[test]
    fn zero_elapsed_time_has_no_motion_or_volcanic_events() {
        let g = MapGrid::Sphere(SphereHexGrid::build(8, 6371000., (DVec3::Z, 0.)));
        let mut p = GenerationParams::default();
        p.tectonic_age_myr = 0.;
        let mut f = WorldFields::with_capacity(g.cell_count());
        let s = generate(&g, &p, &mut f, &mut |_| {}, &|| false).unwrap();
        assert_eq!(s.history.steps, 0);
        assert!(s.history.volcanoes.is_empty());
        assert_eq!(s.history.new_ocean_crust_m3, 0.);
    }
    #[test]
    fn open_plane_accounts_for_export_and_new_ocean_crust() {
        let g = MapGrid::Plane(FlatGrid::new(24, 18, 2400000., 25., 0., 6371000.));
        let mut f = WorldFields::with_capacity(g.cell_count());
        f.crust_thickness.fill(35000.);
        f.reference_crust_thickness.fill(35000.);
        f.crust_type.fill(1.);
        let state = TectonicState {
            plates: vec![crate::Plate {
                id: 0,
                euler_pole: DVec3::Y.to_array(),
                omega: 0.,
                oceanic: false,
            }],
            planar_velocities: Some(vec![DVec3::X * 30000.]),
            centers: vec![DVec3::ZERO],
            ..Default::default()
        };
        let mut h = History {
            initial_crust_m3: crust_volume(&g, &f),
            ..Default::default()
        };
        let geometry = faces(&g);
        for _ in 0..20 {
            remap(&g, &geometry, &mut f, &state, 0.2, &mut h);
        }
        let expected = h.initial_crust_m3 + h.imported_crust_m3 + h.new_ocean_crust_m3
            - h.subducted_crust_m3
            - h.exported_crust_m3;
        assert!(h.imported_crust_m3 > 0. && h.exported_crust_m3 > 0.);
        assert!(h.new_ocean_crust_m3 / h.initial_crust_m3 < 1e-10);
        assert!((crust_volume(&g, &f) - expected).abs() / expected < 1e-6);
        assert!(f.crust_thickness.iter().all(|h| *h > 0.));
        // Incoming old ocean replaces outgoing rock; it must not be counted
        // on top of a full retained cell or create an artificial coastal root.
        f.crust_thickness.fill(7000.);
        f.reference_crust_thickness.fill(7000.);
        f.crust_type.fill(0.);
        let mut ocean_history = History::default();
        for _ in 0..20 {
            remap(&g, &geometry, &mut f, &state, 0.2, &mut ocean_history);
        }
        assert!(f
            .crust_thickness
            .iter()
            .zip(&f.reference_crust_thickness)
            .all(|(h, r)| (*h - 7000.).abs() < 0.1 && (*h - *r).abs() < 0.1));
        assert!(ocean_history.subducted_crust_m3 < 1e-10 * crust_volume(&g, &f));
    }
    #[test]
    fn continental_collision_thickens_a_region_while_oceanic_overlap_subducts() {
        let g = MapGrid::Plane(FlatGrid::new(32, 16, 3200000., 25., 0., 6371000.));
        let state = TectonicState {
            plates: (0..2)
                .map(|id| crate::Plate {
                    id,
                    euler_pole: DVec3::Y.to_array(),
                    omega: 0.,
                    oceanic: false,
                })
                .collect(),
            planar_velocities: Some(vec![DVec3::X * 20000., -DVec3::X * 20000.]),
            centers: vec![DVec3::X * -500000., DVec3::X * 500000.],
            ..Default::default()
        };
        let evolve = |continental: bool| {
            let mut f = WorldFields::with_capacity(g.cell_count());
            let thickness = if continental { 35000. } else { 7000. };
            f.crust_thickness.fill(thickness);
            f.reference_crust_thickness.fill(thickness);
            f.crust_type.fill(if continental { 1. } else { 0. });
            for i in 0..g.cell_count() {
                f.plate_id[i] = if g.field_position(i).x < 0. { 0 } else { 1 };
            }
            let mut h = History::default();
            let geometry = faces(&g);
            for _ in 0..40 {
                remap(&g, &geometry, &mut f, &state, 0.2, &mut h);
            }
            (f, h)
        };
        let (land, _) = evolve(true);
        let (ocean, sinking) = evolve(false);
        let roots: Vec<_> = (0..32)
            .map(|col| 8 * 32 + col)
            .filter(|&i| land.crust_thickness[i] - land.reference_crust_thickness[i] > 1000.)
            .collect();
        assert!(
            roots.len() >= 4,
            "root occupies {} 100-km samples",
            roots.len()
        );
        assert!(ocean
            .crust_thickness
            .iter()
            .all(|h| (*h - 7000.).abs() < 0.1));
        assert!(sinking.subducted_crust_m3 > 0.);
    }
    #[test]
    fn transported_tracer_follows_the_plate_euler_motion() {
        let g = MapGrid::Sphere(SphereHexGrid::build(16, 6371000., (DVec3::Z, 0.)));
        let mut f = WorldFields::with_capacity(g.cell_count());
        f.crust_type.fill(1.);
        for i in 0..g.cell_count() {
            f.crust_thickness[i] =
                7000. + (2000. * ((g.center_dir(i).as_vec().dot(DVec3::X) - 1.) * 8.).exp()) as f32;
        }
        f.reference_crust_thickness = f.crust_thickness.clone();
        let centroid = |f: &WorldFields| {
            (0..g.cell_count())
                .map(|i| {
                    g.center_dir(i).as_vec() * (f.crust_thickness[i] as f64 - 7000.) * g.area_m2(i)
                })
                .sum::<DVec3>()
                .normalize()
        };
        let initial = centroid(&f);
        let state = TectonicState {
            plates: vec![crate::Plate {
                id: 0,
                euler_pole: DVec3::Y.to_array(),
                omega: 0.03,
                oceanic: false,
            }],
            ..Default::default()
        };
        let mut h = History::default();
        let geometry = faces(&g);
        for _ in 0..50 {
            remap(&g, &geometry, &mut f, &state, 0.1, &mut h);
        }
        let expected = rotate(initial, DVec3::Y, 0.15);
        assert!(
            centroid(&f).distance(expected) < 0.025,
            "actual {:?}, expected {:?}",
            centroid(&f),
            expected
        );
    }
}
