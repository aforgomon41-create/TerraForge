//! Annual-mean climate approximation. All rainfall values are millimetres/year.
use crate::fields::WorldFields;
use rayon::prelude::*;
use tf_core::params::PlanetParams;
use tf_grid::GridOps;

#[derive(Debug, Clone, Copy)]
pub struct ClimateParams {
    pub greenhouse: f64,
    pub precipitation_scale: f64,
    pub monsoon: bool,
    pub advection_iterations: u32,
}
impl Default for ClimateParams {
    fn default() -> Self {
        Self {
            greenhouse: 1.,
            precipitation_scale: 1.,
            monsoon: true,
            advection_iterations: 80,
        }
    }
}
pub fn run_climate<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    planet: &PlanetParams,
    params: &ClimateParams,
) {
    run_climate_cancellable(grid, fields, planet, params, &|| false);
}
pub fn run_climate_cancellable<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    planet: &PlanetParams,
    params: &ClimateParams,
    cancel: &dyn Fn() -> bool,
) -> bool {
    if cancel() {
        return false;
    }
    compute_temperature(grid, fields, planet, params);
    apply_ocean_currents(grid, fields);
    if cancel() {
        return false;
    }
    compute_precipitation(grid, fields, planet, params, cancel)
}
// Daily mean insolation, averaged over equally spaced orbital seasons.
fn annual_insolation(latitude: f64, tilt: f64) -> f64 {
    (0..24)
        .map(|season| {
            let declination =
                (tilt.sin() * (std::f64::consts::TAU * season as f64 / 24.).sin()).asin();
            let hour = (-latitude.tan() * declination.tan()).clamp(-1., 1.).acos();
            (hour * latitude.sin() * declination.sin()
                + latitude.cos() * declination.cos() * hour.sin())
            .max(0.)
                / std::f64::consts::PI
        })
        .sum::<f64>()
        / 24.
}
fn compute_temperature<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    planet: &PlanetParams,
    params: &ClimateParams,
) {
    let equator_reference = annual_insolation(0., 23.44_f64.to_radians());
    fields
        .temperature
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, t)| {
            let q = annual_insolation(
                grid.latitude_deg(i).to_radians(),
                planet.axial_tilt_deg.to_radians(),
            );
            // A small heat-transport term avoids a zero-temperature pole at zero tilt.
            let effective =
                ((q + 0.08) / (equator_reference + 0.08) * planet.solar_constant / 1361.).max(0.01);
            let sea_level = 300.15 * effective.powf(0.25) - 273.15;
            let greenhouse = (params.greenhouse - 1.) * 20. + planet.atmosphere_density.ln() * 5.;
            let lapse = fields.elevation[i].max(0.) as f64 * tf_core::constants::ENV_LAPSE_RATE;
            let inland = (fields.distance_to_sea[i] as f64 / 3_000_000.).clamp(0., 1.) * 3.;
            *t = (sea_level + greenhouse - lapse - inland).clamp(-100., 90.) as f32;
        });
}
fn compute_wind<G: GridOps>(
    grid: &G,
    fields: &WorldFields,
    planet: &PlanetParams,
    monsoon: bool,
) -> Vec<(f64, f64)> {
    let hadley = (30. * (planet.rotation_period_hours / 24.).powf(0.35)).clamp(10., 60.);
    (0..grid.cell_count())
        .into_par_iter()
        .map(|i| {
            let lat = grid.latitude_deg(i);
            let a = lat.abs();
            let (mut u, mut v) = if a < hadley {
                (-1., -lat.signum() * 0.5)
            } else if a < (hadley + 30.).min(80.) {
                (1., lat.signum() * 0.5)
            } else {
                (-1., -lat.signum() * 0.4)
            };
            let (east, north) = grid.center_dir(i).tangent_frame();
            let mut sx = 0.;
            let mut sy = 0.;
            let mut thermal_x = 0.;
            let mut thermal_y = 0.;
            for &j in grid.neighbors(i) {
                let j = j as usize;
                let dir = grid.center_dir(j).as_vec() - grid.center_dir(i).as_vec();
                let len = dir.length().max(1e-12);
                let dir = dir / len;
                let slope = (fields.elevation[j] - fields.elevation[i]) as f64
                    / grid.distance_m(i, j).max(1.);
                sx += slope * dir.dot(east);
                sy += slope * dir.dot(north);
                let thermal = (fields.temperature[j] - fields.temperature[i]) as f64 / 20.;
                thermal_x += thermal * dir.dot(east);
                thermal_y += thermal * dir.dot(north);
            }
            u -= sy * 8.;
            v += sx * 8.;
            if monsoon {
                let seasonal = (planet.axial_tilt_deg / 23.44).min(2.);
                u += thermal_x * seasonal * 0.35;
                v += thermal_y * seasonal * 0.35;
            }
            let m = (u * u + v * v).sqrt().max(1e-12);
            (u / m, v / m)
        })
        .collect()
}
fn compute_precipitation<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    planet: &PlanetParams,
    params: &ClimateParams,
    cancel: &dyn Fn() -> bool,
) -> bool {
    let n = grid.cell_count();
    let wind = compute_wind(grid, fields, planet, params.monsoon);
    let capacity: Vec<f64> = fields
        .temperature
        .iter()
        .map(|&t| (0.06 * (t as f64 - 20.)).exp().clamp(0.01, 4.))
        .collect();
    // One transport step is the time taken to cross one average neighbour spacing.
    let spacing = (0..n.min(1024))
        .map(|i| {
            grid.neighbors(i)
                .first()
                .map_or(1., |&j| grid.distance_m(i, j as usize))
        })
        .sum::<f64>()
        / n.min(1024).max(1) as f64;
    let dt = (spacing / 10.).clamp(60., 86400.);
    let steps_per_year = 31_557_600. / dt;
    let mut transport: Vec<Vec<(usize, f64)>> = Vec::with_capacity(n);
    let mut fractions = Vec::with_capacity(n);
    for i in 0..n {
        let (east, north) = grid.center_dir(i).tangent_frame();
        let w = east * wind[i].0 + north * wind[i].1;
        let mut upstream = Vec::with_capacity(3);
        let mut total = 0.;
        let mut lift = 0.;
        for &j in grid.neighbors(i) {
            let j = j as usize;
            let d = grid.center_dir(j).as_vec() - grid.center_dir(i).as_vec();
            let align = -w.dot(d) / d.length().max(1e-12);
            if align > 0. {
                let weight = align * align;
                upstream.push((j, weight));
                total += weight;
                lift += (fields.elevation[i] - fields.elevation[j]).max(0.) as f64 * weight;
            }
        }
        if total > 0. {
            for p in &mut upstream {
                p.1 /= total;
            }
            lift /= total;
        }
        let lat = grid.latitude_deg(i).abs();
        let itcz = (-0.5 * (lat / 14.).powi(2)).exp();
        let storm = (-0.5 * ((lat - 55.) / 12.).powi(2)).exp();
        let convection = (dt / (8. * 86400.) * (0.35 + 1.3 * itcz + 0.6 * storm)).min(0.8);
        let oro = 1. - (-lift / 2200.).exp();
        fractions.push((convection + oro * 0.75).clamp(0., 0.9));
        transport.push(upstream);
    }
    let mut moisture: Vec<f64> = (0..n)
        .map(|i| {
            if fields.elevation[i] <= 0. {
                capacity[i]
            } else {
                0.
            }
        })
        .collect();
    let mut next = vec![0.; n];
    let mut rain = vec![0.; n];
    let iterations = params.advection_iterations.max(1);
    for _ in 0..iterations {
        if cancel() {
            return false;
        }
        for i in 0..n {
            let inflow = transport[i]
                .iter()
                .map(|&(j, w)| moisture[j] * w)
                .sum::<f64>();
            let carried = if fields.elevation[i] <= 0. {
                capacity[i]
            } else {
                moisture[i] * 0.25 + inflow * 0.75
            };
            let saturation = (carried - capacity[i]).max(0.);
            let condensed = (carried.min(capacity[i]) * fractions[i] + saturation).min(carried);
            rain[i] += condensed;
            next[i] = (carried - condensed).max(0.);
        }
        std::mem::swap(&mut moisture, &mut next);
    }
    for i in 0..n {
        fields.precipitation[i] = (rain[i] / iterations as f64
            * 30.
            * steps_per_year
            * params.precipitation_scale
            * planet.atmosphere_density.sqrt())
        .clamp(0., 15000.) as f32;
    }
    !cancel()
}
fn apply_ocean_currents<G: GridOps>(grid: &G, fields: &mut WorldFields) {
    // Symmetric equator-to-midlatitude transport; kept in the final temperature.
    fields
        .temperature
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, t)| {
            if fields.elevation[i] > 0. {
                return;
            }
            let x = (grid.latitude_deg(i).abs() / 90.) as f32;
            *t += (1. - ((x - 0.5) / 0.35).abs()).max(0.) * 4. - (1. - x) * 1.2;
        });
}
#[cfg(test)]
mod tests {
    use super::*;
    use tf_grid::{PatchGrid, SphericalPatch};
    #[test]
    fn no_rank_artifacts_over_uniform_ocean() {
        let g = PatchGrid::new(&SphericalPatch::new(0., 0., 100000., 16));
        let mut f = WorldFields::with_capacity(256);
        f.elevation.fill(-1000.);
        run_climate(
            &g,
            &mut f,
            &PlanetParams::default(),
            &ClimateParams::default(),
        );
        let min = f
            .precipitation
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min);
        let max = f.precipitation.iter().copied().fold(0., f32::max);
        assert!(min > 100.);
        assert!((max - min) / min < 0.05, "{min}..{max}");
    }
    #[test]
    fn ocean_current_correction_is_retained() {
        let g = PatchGrid::new(&SphericalPatch::new(45., 0., 1000., 4));
        let mut f = WorldFields::with_capacity(16);
        f.elevation.fill(-1000.);
        compute_temperature(
            &g,
            &mut f,
            &PlanetParams::default(),
            &ClimateParams::default(),
        );
        let baseline = f.temperature[5];
        run_climate(
            &g,
            &mut f,
            &PlanetParams::default(),
            &ClimateParams::default(),
        );
        assert!(f.temperature[5] > baseline + 3.);
    }
    #[test]
    fn annual_insolation_respects_tilt() {
        assert!(
            annual_insolation(89_f64.to_radians(), 80_f64.to_radians())
                > annual_insolation(89_f64.to_radians(), 0.)
        );
    }
}
