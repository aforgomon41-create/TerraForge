//! Continuous lithologic provinces and kinematic fault relief.
//! Plate identity controls motion, never an entire continent's elevation.
use crate::{BoundaryCell, BoundarySubtype, WorldFields};
use glam::DVec3;
use rayon::prelude::*;
use tf_core::{noise::SimplexNoise, params::GenerationParams};
use tf_grid::GridOps;

fn smooth(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
fn fbm(noise: &SimplexNoise, p: DVec3, f: f64, oct: u32) -> f64 {
    noise.fbm(p.x * f, p.y * f, p.z * f, oct, 2.03, 0.52)
}
fn warped(noise: &SimplexNoise, p: DVec3) -> DVec3 {
    p + DVec3::new(
        fbm(noise, p + DVec3::X * 7., 2., 3),
        fbm(noise, p + DVec3::Y * 13., 2., 3),
        fbm(noise, p + DVec3::Z * 19., 2., 3),
    ) * 0.22
}

/// A map framing constraint, independent of the rock-volume evolution. Closing
/// a flat root reserves water at its edge; it does not wrap or stop the plates.
pub fn apply_planar_coast_constraint<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    params: &GenerationParams,
) {
    if params.allow_land_at_edge || params.is_local() {
        return;
    }
    let Some((width, height)) = grid.extent_m() else {
        return;
    };
    let spacing = (grid.area_m2(0) * 2. / 3_f64.sqrt()).sqrt();
    let minimum_span = width.min(height);
    let margin = minimum_span * (0.01 + 0.04 * ((1. - params.land_fraction) / 0.7).clamp(0., 1.));
    let border_noise = SimplexNoise::new(params.seed ^ 0xB04D_3E12);
    let boundary = grid.boundary_mask();
    let mut reserved = vec![false; grid.cell_count()];
    let mut ocean_floor = vec![-10.; grid.cell_count()];
    let mut candidates = Vec::new();
    for i in 0..grid.cell_count() {
        let (x, y) = grid.local_center_m(i).unwrap();
        let d = (width * 0.5 - x.abs()).min(height * 0.5 - y.abs()).max(0.);
        let p = grid.field_position(i) / (minimum_span * 0.2).max(1.);
        let coastal_shape = smooth(-0.55, 0.55, fbm(&border_noise, p, 1., 3));
        let local_margin = (margin * (0.4 + 2. * coastal_shape)).max(spacing * 2.);
        let transition = minimum_span * (0.05 + 0.16 * coastal_shape);
        let t = smooth(local_margin, local_margin + transition, d);
        fields.elevation[i] = (fields.elevation[i] as f64 * t - 4500. * (1. - t)) as f32;
        reserved[i] = boundary[i] || d <= local_margin;
        ocean_floor[i] = (-10. - 2800. * (1. - smooth(0., local_margin, d))) as f32;
        if !reserved[i] {
            candidates.push(fields.elevation[i]);
        }
    }
    if candidates.is_empty() {
        fields.elevation.fill(-10.);
        return;
    }
    candidates.sort_unstable_by(f32::total_cmp);
    // Keep the requested total land area when the finite grid has enough
    // non-boundary cells. A fully oceanic rim takes priority on tiny grids.
    let land_cells = ((params.land_fraction * grid.cell_count() as f64).round() as usize)
        .min(candidates.len().saturating_sub(1));
    let sea = candidates[candidates.len() - 1 - land_cells];
    for i in 0..grid.cell_count() {
        fields.elevation[i] -= sea;
        if reserved[i] {
            fields.elevation[i] = fields.elevation[i].min(ocean_floor[i]);
        }
    }
}

/// Assembled irregular crustal blocks, rather than buffered lines with round caps.
struct CrustProvince {
    center: DVec3,
    east: DVec3,
    north: DVec3,
    vertices: Vec<glam::DVec2>,
}
impl CrustProvince {
    fn radial_extent(&self, p: DVec3, planetary_radius: f64, planar: bool) -> (f64, f64) {
        let delta = if planar {
            p - self.center
        } else {
            let direction = p.normalize();
            let center = self.center.normalize();
            let cosine = direction.dot(center).clamp(-1., 1.);
            let tangent = direction - center * cosine;
            if cosine < 0. && tangent.length_squared() < 1e-20 {
                return (std::f64::consts::PI * planetary_radius, 0.);
            }
            tangent.normalize_or_zero() * cosine.acos() * planetary_radius
        };
        let p = glam::DVec2::new(delta.dot(self.east), delta.dot(self.north));
        let radius = p.length();
        let direction = if radius > 1e-9 {
            p / radius
        } else {
            glam::DVec2::X
        };
        let mut boundary = 0_f64;
        for k in 0..self.vertices.len() {
            let a = self.vertices[k];
            let b = self.vertices[(k + 1) % self.vertices.len()];
            let ab = b - a;
            let determinant = direction.perp_dot(ab);
            if determinant.abs() > 1e-12 {
                let distance = a.perp_dot(ab) / determinant;
                let along = a.perp_dot(direction) / determinant;
                if distance >= 0. && (-1e-10..=1. + 1e-10).contains(&along) {
                    // Blocks are star-shaped about their centre. Use the last
                    // hit so a ray grazing a concave vertex does not cut a hole.
                    boundary = boundary.max(distance);
                }
            }
        }
        (radius, boundary)
    }
}
fn continental_provinces<G: GridOps>(grid: &G, params: &GenerationParams) -> Vec<CrustProvince> {
    use tf_core::{geo::UnitVec3, params::LevelKind, DeterministicRng};
    let mut rng = DeterministicRng::derive(params.seed, "continental-terrane-assembly-v2");
    let count = if params.start_level == LevelKind::Continent {
        1
    } else {
        params.continent_count as usize
    };
    let planar = grid.field_is_planar();
    let radius = grid.radius_m();
    let (width, height) = grid.extent_m().unwrap_or((radius * 2., radius * 2.));
    let area = if planar {
        width * height
    } else {
        4. * std::f64::consts::PI * radius * radius
    };
    let scale = (area * (params.land_fraction + 0.08).clamp(0.15, 0.8) / (count as f64 * 13.))
        .sqrt()
        * params.continent_scale.sqrt();
    let mut anchors: Vec<DVec3> = Vec::new();
    let mut provinces = Vec::new();
    let block = |center: DVec3, size: f64, bearing: f64, rng: &mut DeterministicRng| {
        let (east, north) = if planar {
            (DVec3::X, DVec3::Y)
        } else {
            UnitVec3::from_vec(center).unwrap().tangent_frame()
        };
        let count = rng.range_i32(6, 10) as usize;
        let aspect = rng.range_f64(0.6, 1.5);
        let vertices = (0..count)
            .map(|k| {
                let angle =
                    (k as f64 + rng.range_f64(-0.22, 0.22)) * std::f64::consts::TAU / count as f64;
                let radius = size * rng.range_f64(0.60, 1.25);
                let x = angle.cos() * radius * aspect;
                let y = angle.sin() * radius / aspect.sqrt();
                glam::DVec2::new(
                    x * bearing.cos() - y * bearing.sin(),
                    x * bearing.sin() + y * bearing.cos(),
                )
            })
            .collect();
        CrustProvince {
            center,
            east,
            north,
            vertices,
        }
    };
    for _group in 0..count {
        let mut center = DVec3::ZERO;
        let mut best = -1.;
        for _ in 0..32 {
            let candidate = if planar {
                DVec3::new(
                    rng.range_f64(-0.34, 0.34) * width,
                    rng.range_f64(-0.34, 0.34) * height,
                    0.,
                )
            } else {
                rng.unit_sphere().as_vec() * radius
            };
            let clearance = anchors
                .iter()
                .map(|a| (candidate - *a).length_squared())
                .fold(f64::INFINITY, f64::min);
            if clearance > best {
                center = candidate;
                best = clearance;
            }
        }
        if planar && count == 1 {
            center = DVec3::ZERO;
        }
        anchors.push(center);
        let scale = scale * rng.range_f64(0.60, 1.30);
        let bearing = rng.range_f64(0., std::f64::consts::TAU);
        let mut nodes = Vec::new();
        for step in -2_i32..=2 {
            let distance = step as f64 * scale * rng.range_f64(0.45, 0.9);
            let angle = bearing + rng.range_f64(-0.65, 0.65);
            let p = if planar {
                center + DVec3::new(angle.sin(), angle.cos(), 0.) * distance
            } else {
                UnitVec3::from_vec(center)
                    .unwrap()
                    .offset(angle, distance / radius)
                    .as_vec()
                    * radius
            };
            nodes.push((
                p,
                scale * (1.15 - 0.18 * step.unsigned_abs() as f64) * rng.range_f64(0.7, 1.25),
            ));
        }
        for pair in nodes.windows(2) {
            let center = if planar {
                (pair[0].0 + pair[1].0) * 0.5
            } else {
                (pair[0].0 + pair[1].0).normalize() * radius
            };
            let (east, north) = if planar {
                (DVec3::X, DVec3::Y)
            } else {
                UnitVec3::from_vec(center).unwrap().tangent_frame()
            };
            let project =
                |p: DVec3| glam::DVec2::new((p - center).dot(east), (p - center).dot(north));
            let (a, b) = (project(pair[0].0), project(pair[1].0));
            let axis = (b - a).normalize_or_zero();
            let side = glam::DVec2::new(-axis.y, axis.x);
            provinces.push(CrustProvince {
                center,
                east,
                north,
                vertices: vec![
                    a + side * pair[0].1 * 0.6,
                    b + side * pair[1].1 * 0.6,
                    b - side * pair[1].1 * 0.6,
                    a - side * pair[0].1 * 0.6,
                ],
            });
        }
        for &(p, size) in &nodes {
            provinces.push(block(p, size, bearing + rng.range_f64(-1., 1.), &mut rng));
        }
        // Asymmetric branches leave large embayments between terranes. Their
        // positions and sizes vary independently instead of tracing one arc.
        for branch in 0..rng.range_i32(2, 4) {
            let root = nodes[rng.below(nodes.len() as u32) as usize];
            let angle = bearing + if branch % 2 == 0 { 1. } else { -1. } * rng.range_f64(0.8, 2.1);
            let distance = scale * rng.range_f64(0.65, 1.6);
            let tip = if planar {
                root.0 + DVec3::new(angle.sin(), angle.cos(), 0.) * distance
            } else {
                UnitVec3::from_vec(root.0)
                    .unwrap()
                    .offset(angle, distance / radius)
                    .as_vec()
                    * radius
            };
            let size = scale * rng.range_f64(0.35, 0.8);
            provinces.push(block(tip, size, angle, &mut rng));
        }
        // Initial continental terranes are not all joined to the same spine.
        // Their subsequent movement and collision are handled by the crust solver.
        for _ in 0..3 {
            let angle = rng.range_f64(0., std::f64::consts::TAU);
            let distance = scale * rng.range_f64(1.8, 3.5);
            let a = if planar {
                center + DVec3::new(angle.sin(), angle.cos(), 0.) * distance
            } else {
                UnitVec3::from_vec(center)
                    .unwrap()
                    .offset(angle, distance / radius)
                    .as_vec()
                    * radius
            };
            let r = scale * rng.range_f64(0.13, 0.40);
            provinces.push(block(a, r, angle, &mut rng));
        }
    }
    provinces
}
pub fn init_continents<G: GridOps>(grid: &G, fields: &mut WorldFields, params: &GenerationParams) {
    let noise = SimplexNoise::new(params.seed ^ 0xC071_1E17);
    let provinces = continental_provinces(grid, params);
    let radius = grid.radius_m();
    let characteristic_length = provinces
        .iter()
        .map(|province| {
            province.vertices.iter().map(|v| v.length()).sum::<f64>()
                / province.vertices.len() as f64
        })
        .sum::<f64>()
        / provinces.len() as f64;
    let values: Vec<f64> = (0..grid.cell_count())
        .into_par_iter()
        .map(|i| {
            let p = grid.field_position(i);
            let coast = (fbm(&noise, p / 800_000., 1., 3) * 300_000.
                + fbm(&noise, p / 180_000., 1., 2) * 70_000.)
                * params.coast_complexity;
            // Match land area by scaling the blocks, rather than adding a
            // constant-radius buffer that rounds every tip and fills every bay.
            provinces
                .iter()
                .map(|province| {
                    let (distance, boundary) =
                        province.radial_extent(p, radius, grid.field_is_planar());
                    (boundary + coast).max(0.) / distance.max(1.)
                })
                .max_by(f64::total_cmp)
                .unwrap()
        })
        .collect();
    let mut sorted = values.clone();
    sorted.sort_unstable_by(f64::total_cmp);
    let threshold = sorted
        [(((1. - params.land_fraction) * sorted.len() as f64) as usize).min(sorted.len() - 1)];
    let transition = 400_000_f64.min(grid.extent_m().map_or(400_000., |(w, h)| w.min(h) * 0.08));
    for i in 0..grid.cell_count() {
        let inland = (values[i] / threshold.max(1e-12)).max(1e-12).ln() * characteristic_length;
        let p = grid.field_position(i);
        let continental = smooth(-0.466 * transition, 0.534 * transition, inland);
        let basement = fbm(&noise, p / 1_200_000. + DVec3::splat(53.), 1., 3)
            * 1800.
            * smooth(0., transition * 0.6, inland);
        fields.crust_type[i] = continental as f32;
        fields.crust_thickness[i] = (7000. + continental * (29000. + basement)) as f32;
        fields.reference_crust_thickness[i] = fields.crust_thickness[i];
        // Stable lowland/shelf background is the reference column, not a 2 km continental plateau.
        let foundation = if inland >= 0. {
            450. * (1. - (-inland / 600_000.).exp())
        } else {
            2000. * ((inland / 400_000.).exp() - 1.)
        };
        fields.elevation[i] = (foundation
            + fbm(&noise, p / 1_200_000. + DVec3::splat(71.), 1., 3)
                * 180.
                * smooth(0., 400_000., inland)) as f32;
    }
}

/// Old fold belts, dissected uplands and sedimentary basins have independent wavelengths.
pub fn interior_detail(noise: &SimplexNoise, position: DVec3, spacing: f64) -> f64 {
    // Absolute metre wavelengths; only the numerical sampling filter changes.
    let p = position / 900_000.;
    let q = warped(noise, p * 0.3) * 3.3;
    let basin = fbm(noise, q, 0.8, 4) * 420.;
    // A signed, broad field has hills and valleys. Raising the zero contour of
    // |noise| to a power instead drew closed C-shaped mountain belts unrelated
    // to any accumulated shortening.
    let mut value = basin + fbm(noise, q + DVec3::splat(37.), 1.7, 4) * 500.;
    let mut wavelength = 160_000.;
    for octave in 0..18 {
        let sigma = spacing / 12_f64.sqrt();
        let attenuation = (-0.5 * (std::f64::consts::TAU * sigma / wavelength).powi(2)).exp();
        if attenuation < 0.0001 {
            break;
        }
        let amplitude = 250. * (wavelength / 160000_f64).powf(0.65);
        let v = position / wavelength + DVec3::splat(71. + octave as f64 * 17.);
        value += noise.sample(v.x, v.y, v.z) * amplitude * attenuation;
        wavelength *= 0.5;
    }
    value
}

/// Locally anisotropic roughness following an existing thickened-root trend.
/// Symmetric samples are invariant to reversing the strike, avoiding phase seams.
pub fn orogenic_detail(noise: &SimplexNoise, position: DVec3, strike: DVec3, spacing: f64) -> f64 {
    let mut value = 0.;
    for (octave, wavelength) in [320_000_f64, 160_000., 80_000.].into_iter().enumerate() {
        let attenuation =
            (-0.5 * (std::f64::consts::TAU * spacing / 12_f64.sqrt() / wavelength).powi(2)).exp();
        for (offset, weight) in [(-2., 0.1), (-1., 0.2), (0., 0.4), (1., 0.2), (2., 0.1)] {
            let p = (position + strike * offset * wavelength * 0.8) / wavelength
                + DVec3::splat(127. + octave as f64 * 41.);
            // Rotate the slice away from the simplex lattice's coordinate planes.
            let p = glam::DQuat::from_axis_angle(
                DVec3::new(1., 2., 3.).normalize(),
                0.71 + octave as f64,
            )
            .mul_vec3(p);
            value += noise.sample(p.x, p.y, p.z)
                * weight
                * attenuation
                * 1500.
                * (wavelength / 320000.).powf(0.65);
        }
    }
    value
}
fn gaussian(d: f64, center: f64, width: f64, spacing: f64) -> f64 {
    let filtered = (width * width + spacing * spacing / 12.).sqrt();
    width / filtered * (-0.5 * ((d - center) / filtered).powi(2)).exp()
}

/// Subtypes have different cross sections, rather than a single Gaussian ribbon.
fn fault_profile(kind: BoundarySubtype, d: f64, signed: f64, wave: f64, spacing: f64) -> f64 {
    match kind {
        BoundarySubtype::ContinentContinentConvergent => {
            gaussian(d, 0., 220_000., spacing) * (1300. + 2200. * wave)
        }
        BoundarySubtype::ContinentContinentDivergent => {
            -1600. * gaussian(d, 0., 24_000., spacing) + 900. * gaussian(d, 65000., 35000., spacing)
        }
        BoundarySubtype::OceanOceanDivergent => {
            1800. * gaussian(d, 0., 110_000., spacing) - 450. * gaussian(d, 0., 12000., spacing)
        }
        BoundarySubtype::Transform => {
            (signed / 15000.).tanh() * (300. + 350. * wave) * gaussian(d, 0., 28000., spacing)
        }
        _ => 0.,
    }
}

pub fn apply_relief<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    boundaries: &[BoundaryCell],
    params: &GenerationParams,
) {
    use std::collections::HashMap;
    let bin = 800_000.;
    let key = |p: DVec3| {
        (
            (p.x / bin).floor() as i32,
            (p.y / bin).floor() as i32,
            (p.z / bin).floor() as i32,
        )
    };
    let mut bins: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
    let mut geometry = Vec::with_capacity(boundaries.len());
    for (index, b) in boundaries.iter().enumerate() {
        let a = grid.field_position(b.cell as usize);
        let c = grid.field_position(b.other as usize);
        let mid = (a + c) * 0.5;
        let normal = DVec3::from_array(b.normal);
        let outward = if grid.field_is_planar() {
            DVec3::Z
        } else {
            mid.normalize()
        };
        let tangent = outward.cross(normal).normalize_or_zero();
        geometry.push((
            mid,
            normal,
            tangent,
            grid.distance_m(b.cell as usize, b.other as usize),
        ));
        bins.entry(key(mid)).or_default().push(index);
    }
    let noise = SimplexNoise::new(params.seed ^ 0x0ED0_6E17);
    let crust = fields.crust_type.clone();
    let ages = &fields.crust_age;
    let sampling = (grid.area_m2(0) * 2. / 3_f64.sqrt()).sqrt();
    fields
        .elevation
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, e)| {
            let position = grid.field_position(i);
            let (kx, ky, kz) = key(position);
            let mut signal_sum = 0.;
            let mut weights = 0.;
            let mut slip_sum = DVec3::ZERO;
            let phase_warp = fbm(&noise, position / 220000., 1., 3) * 2.;
            let along_variation =
                (0.8 + 0.45 * fbm(&noise, position / 350000. + DVec3::splat(91.), 1., 3)).max(0.2);
            for x in kx - 1..=kx + 1 {
                for y in ky - 1..=ky + 1 {
                    for z in kz - 1..=kz + 1 {
                        let Some(indices) = bins.get(&(x, y, z)) else {
                            continue;
                        };
                        for &index in indices {
                            let b = &boundaries[index];
                            let (mid, normal, tangent, spacing) = geometry[index];
                            let delta = position - mid;
                            let center_d2 = delta.length_squared();
                            if center_d2 > bin * bin {
                                continue;
                            }
                            let signed = delta.dot(normal);
                            let along = delta.dot(tangent);
                            let distance = signed.hypot((along.abs() - spacing * 0.65).max(0.));
                            let weight = (-0.5 * center_d2 / (160000_f64.powi(2))).exp()
                                / (center_d2 + 40000_f64.powi(2));
                            let phase = signed / 90000. * std::f64::consts::TAU + phase_warp;
                            let sigma = sampling / 12_f64.sqrt();
                            let k = std::f64::consts::TAU / 90000.;
                            let wave = 0.375
                                + 0.5 * (-0.5 * (k * sigma).powi(2)).exp() * phase.cos()
                                + 0.125 * (-2. * (k * sigma).powi(2)).exp() * (2. * phase).cos();
                            let strength = (b.normal_velocity.abs() / 60000.).clamp(0., 1.5);
                            let shear = (b.tangential_velocity / 60000.).clamp(0., 1.5);
                            let (mut signal, multiplier) = match b.subtype {
                                BoundarySubtype::ContinentContinentConvergent => (
                                    fault_profile(b.subtype, distance, signed, wave, sampling),
                                    params.orogeny_strength * strength,
                                ),
                                BoundarySubtype::ContinentContinentDivergent
                                | BoundarySubtype::OceanOceanDivergent => (
                                    fault_profile(b.subtype, distance, signed, wave, sampling),
                                    params.rift_strength * strength,
                                ),
                                BoundarySubtype::Transform => {
                                    slip_sum += tangent
                                        * (signed / 15000.).tanh()
                                        * 50000.
                                        * shear
                                        * params.shear_strength
                                        * gaussian(distance, 0., 80000., sampling)
                                        * weight;
                                    (
                                        fault_profile(b.subtype, distance, signed, wave, sampling),
                                        params.shear_strength * shear,
                                    )
                                }
                                _ => {
                                    let (a, c) = (b.cell as usize, b.other as usize);
                                    let upper_a = if crust[a] >= 0.5 || crust[c] >= 0.5 {
                                        crust[a] >= crust[c]
                                    } else {
                                        ages[a] <= ages[c]
                                    };
                                    let upper_coordinate = if upper_a { -signed } else { signed };
                                    let amplitude =
                                        if b.subtype == BoundarySubtype::OceanOceanConvergent {
                                            6500.
                                        } else {
                                            3000.
                                        };
                                    let arc = amplitude
                                        * gaussian(upper_coordinate, 135000., 70000., sampling)
                                        * (0.55 + 0.45 * wave);
                                    let trench = -3500.
                                        * gaussian(upper_coordinate, -30000., 40000., sampling);
                                    (arc + trench, params.orogeny_strength * strength)
                                }
                            };
                            signal *= multiplier * along_variation;
                            signal_sum += signal * weight;
                            weights += weight;
                        }
                    }
                }
            }
            let shifted = position
                + if weights > 0. {
                    slip_sum / weights
                } else {
                    DVec3::ZERO
                };
            let deformation = if weights > 0. {
                signal_sum / weights
            } else {
                0.
            };
            let relief = interior_detail(&noise, shifted, sampling) * params.interior_relief;
            *e += (relief * (0.12 + 0.88 * crust[i] as f64) + deformation) as f32;
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closing_a_plane_reserves_ocean_at_every_boundary_without_changing_the_rock_ledger() {
        let g = tf_grid::FlatGrid::new(64, 40, 6_400_000., 25., 0., 6_371_000.);
        let mut f = WorldFields::with_capacity(g.cell_count());
        for i in 0..g.cell_count() {
            let (x, y) = g.local_center_m(i).unwrap();
            f.elevation[i] = (800. + x * 0.0002 + y * 0.00001) as f32;
            f.crust_thickness[i] = 35000.;
        }
        let mut p = GenerationParams::default();
        let original = f.clone();
        apply_planar_coast_constraint(&g, &mut f, &p);
        assert_eq!(f.elevation, original.elevation);
        assert!(g.boundary_mask().iter_ones().any(|i| f.elevation[i] > 0.));
        p.allow_land_at_edge = false;
        apply_planar_coast_constraint(&g, &mut f, &p);
        assert!(g.boundary_mask().iter_ones().all(|i| f.elevation[i] < 0.));
        assert_eq!(f.crust_thickness, original.crust_thickness);
        let share = f.elevation.iter().filter(|h| **h > 0.).count() as f64 / g.cell_count() as f64;
        assert!((share - p.land_fraction).abs() < 0.005, "{share}");
    }
    #[test]
    fn fault_types_have_distinct_folds_rift_shoulders_and_slip() {
        let collision = fault_profile(
            BoundarySubtype::ContinentContinentConvergent,
            60000.,
            60000.,
            1.,
            10000.,
        );
        let trough = fault_profile(
            BoundarySubtype::ContinentContinentDivergent,
            0.,
            0.,
            0.,
            10000.,
        );
        let shoulder = fault_profile(
            BoundarySubtype::ContinentContinentDivergent,
            65000.,
            65000.,
            0.,
            10000.,
        );
        assert!(collision > 2500. && trough < -1000. && shoulder > 500.);
        let left = fault_profile(BoundarySubtype::Transform, 10000., -10000., 0.5, 10000.);
        let right = fault_profile(BoundarySubtype::Transform, 10000., 10000., 0.5, 10000.);
        assert!(left < -100. && right > 100.);
    }
    fn landmass_shares<G: GridOps>(g: &G, heights: &[f32]) -> Vec<f64> {
        let mut seen = vec![false; g.cell_count()];
        let mut areas = Vec::new();
        for i in 0..g.cell_count() {
            if seen[i] || heights[i] <= 0. {
                continue;
            }
            seen[i] = true;
            let mut queue = vec![i];
            let mut cursor = 0;
            let mut area = 0.;
            while cursor < queue.len() {
                let j = queue[cursor];
                cursor += 1;
                area += g.area_m2(j);
                for &k in g.neighbors(j) {
                    let k = k as usize;
                    if !seen[k] && heights[k] > 0. {
                        seen[k] = true;
                        queue.push(k);
                    }
                }
            }
            areas.push(area);
        }
        areas.sort_by(|a, b| b.total_cmp(a));
        let total = areas.iter().sum::<f64>();
        for a in &mut areas {
            *a /= total;
        }
        areas
    }
    #[test]
    fn increasing_a_crustal_block_does_not_fill_its_large_embayment() {
        let province = CrustProvince {
            center: DVec3::ZERO,
            east: DVec3::X,
            north: DVec3::Y,
            vertices: [
                (-2., -2.),
                (2., -2.),
                (2., 2.),
                (0.2, 2.),
                (0.2, 0.2),
                (-2., 0.2),
            ]
            .into_iter()
            .map(|(x, y)| glam::DVec2::new(x, y) * 1_000_000.)
            .collect(),
        };
        for scale in [1., 2., 3.] {
            let inside = DVec3::new(1., 1., 0.) * scale * 1_000_000.;
            let bay = DVec3::new(-1., 1., 0.) * scale * 1_000_000.;
            let (r, bound) = province.radial_extent(inside, 6371000., true);
            assert!(
                bound / r > 1. / scale,
                "scale {scale}, bound {bound}, r {r}"
            );
            let (r, bound) = province.radial_extent(bay, 6371000., true);
            assert!(bound / r < 1. / scale);
        }
    }
    #[test]
    fn continental_core_control_changes_large_scale_structure() {
        let g = tf_grid::SphereHexGrid::build(64, 6371000., (DVec3::Z, 0.));
        let mut p = GenerationParams {
            seed: 42,
            continent_count: 1,
            ..Default::default()
        };
        let mut a = WorldFields::with_capacity(g.cell_count());
        init_continents(&g, &mut a, &p);
        let one = landmass_shares(&g, &a.elevation);
        p.continent_count = 8;
        let mut b = WorldFields::with_capacity(g.cell_count());
        init_continents(&g, &mut b, &p);
        let several = landmass_shares(&g, &b.elevation);
        assert!(
            one[0] > 0.85 && several[0] < one[0] * 0.8,
            "one {one:?}, several {several:?}"
        );
    }
    #[test]
    fn major_continents_survive_a_change_in_sampling_density() {
        for seed in [0, 42] {
            let p = GenerationParams {
                seed,
                ..Default::default()
            };
            let mut shares = Vec::new();
            for frequency in [48, 96] {
                let g = tf_grid::SphereHexGrid::build(frequency, 6371000., (DVec3::Z, 0.));
                let mut f = WorldFields::with_capacity(g.cell_count());
                init_continents(&g, &mut f, &p);
                let parts = landmass_shares(&g, &f.elevation);
                // Four requested continental cores may now remain separate.
                let top = parts.iter().take(p.continent_count as usize).sum::<f64>();
                assert!(top > 0.85, "seed {seed}, f{frequency}: {parts:?}");
                shares.push(parts[0]);
            }
            assert!(
                (shares[0] - shares[1]).abs() < 0.08,
                "seed {seed}: {shares:?}"
            );
        }
    }
    #[test]
    fn crust_varies_inside_a_plate_without_a_plate_height_step() {
        let g = tf_grid::SphereHexGrid::build(24, 6371000., (DVec3::Z, 0.));
        let mut f = WorldFields::with_capacity(g.cell_count());
        init_continents(&g, &mut f, &GenerationParams::default());
        assert!(f.crust_type.iter().any(|&x| x < 0.1));
        assert!(f.crust_type.iter().any(|&x| x > 0.9));
        assert!(f.crust_type.iter().any(|&x| x > 0.2 && x < 0.8));
        let noise = SimplexNoise::new(42);
        assert!(
            (interior_detail(&noise, DVec3::new(1e6, 2e6, 0.), 1000.)
                - interior_detail(&noise, DVec3::new(1e6 + 1., 2e6, 0.), 1000.))
            .abs()
                < 1.
        );
    }
}
