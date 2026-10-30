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
fn field_scale<G: GridOps>(grid: &G, params: &GenerationParams) -> f64 {
    if grid.field_is_planar() {
        params.map_width_m.max(3_000_000.) * 0.45
    } else {
        grid.radius_m()
    }
}
fn warped(noise: &SimplexNoise, p: DVec3) -> DVec3 {
    p + DVec3::new(
        fbm(noise, p + DVec3::X * 7., 2., 3),
        fbm(noise, p + DVec3::Y * 13., 2., 3),
        fbm(noise, p + DVec3::Z * 19., 2., 3),
    ) * 0.22
}

pub fn init_continents<G: GridOps>(grid: &G, fields: &mut WorldFields, params: &GenerationParams) {
    let noise = SimplexNoise::new(params.seed ^ 0xC071_1E17);
    let scale = field_scale(grid, params);
    let values: Vec<f64> = (0..grid.cell_count())
        .into_par_iter()
        .map(|i| {
            let p = warped(&noise, grid.field_position(i) / scale);
            fbm(&noise, p, 1.7 / params.continent_scale, 5)
                + 0.18
                    * fbm(
                        &noise,
                        p + DVec3::splat(31.),
                        4. / params.continent_scale,
                        3,
                    )
        })
        .collect();
    let mut sorted = values.clone();
    sorted.sort_unstable_by(f64::total_cmp);
    let coverage = (params.land_fraction * 1.1 + 0.15).clamp(0.05, 0.98);
    let threshold =
        sorted[(((1. - coverage) * sorted.len() as f64) as usize).min(sorted.len() - 1)];
    let transition = 0.09;
    for i in 0..grid.cell_count() {
        let p = grid.field_position(i) / scale;
        let continental = smooth(threshold - transition, threshold + transition, values[i]);
        fields.crust_type[i] = continental as f32;
        let basement = fbm(&noise, p + DVec3::splat(53.), 3.5, 4) * 5000.
            + fbm(&noise, p + DVec3::splat(73.), 11., 3) * 1800.;
        fields.crust_thickness[i] = (7000. + continental * (29000. + basement)) as f32;
    }
}

/// Old fold belts, dissected uplands and sedimentary basins have independent wavelengths.
pub fn interior_detail(noise: &SimplexNoise, position: DVec3, spacing: f64) -> f64 {
    // Absolute metre wavelengths; only the numerical sampling filter changes.
    let p = position / 900_000.;
    let q = warped(noise, p * 0.3) * 3.3;
    let basin = fbm(noise, q, 0.8, 4) * 420.;
    let ridge = (1. - fbm(noise, q + DVec3::splat(37.), 2.6, 4).abs()).powi(5);
    let province = smooth(-0.1, 0.35, fbm(noise, q + DVec3::splat(11.), 0.6, 3));
    let mut value = basin + province * ridge * 1500.;
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
