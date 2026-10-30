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
    pub wind_speed_m_s: f64,
    pub orographic_strength: f64,
    pub moisture_recycling: f64,
}
impl Default for ClimateParams {
    fn default() -> Self {
        Self {
            greenhouse: 1.,
            precipitation_scale: 1.,
            monsoon: true,
            wind_speed_m_s: 10.,
            orographic_strength: 1.,
            moisture_recycling: 0.35,
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
fn compute_precipitation<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    planet: &PlanetParams,
    params: &ClimateParams,
    cancel: &dyn Fn() -> bool,
) -> bool {
    crate::climate_transport::solve(grid, fields, planet, params, cancel)
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
    fn mountain_cross_section(cols: u32, atmosphere_spacing: f64) -> (f64, f64, f64) {
        use tf_grid::GridOps;
        let rows = cols / 4;
        let g = tf_grid::FlatGrid::new(cols, rows, 3_200_000., 45., 0., 6371000.);
        let mut f = WorldFields::with_capacity(g.cell_count());
        for i in 0..g.cell_count() {
            let x = g.xy(i).0;
            f.elevation[i] = if x < -1_050_000. {
                -100.
            } else {
                (200. + 2600. * (-0.5 * (x / 140000.).powi(2)).exp()) as f32
            };
        }
        crate::erosion::compute_distance_to_sea(&g, &mut f);
        let params = ClimateParams {
            monsoon: false,
            ..Default::default()
        };
        compute_temperature(&g, &mut f, &PlanetParams::default(), &params);
        apply_ocean_currents(&g, &mut f);
        crate::climate_transport::solve_at_resolution(
            &g,
            &mut f,
            &PlanetParams::default(),
            &params,
            &|| false,
            atmosphere_spacing,
            32,
        );
        let average = |low: f64, high: f64| {
            let selected: Vec<_> = (0..g.cell_count())
                .filter(|&i| {
                    let (x, y) = g.xy(i);
                    x > low && x < high && y.abs() < 120000.
                })
                .collect();
            selected
                .iter()
                .map(|&i| f.precipitation[i] as f64)
                .sum::<f64>()
                / selected.len() as f64
        };
        (
            average(-350000., -60000.),
            average(150000., 450000.),
            f.precipitation.iter().copied().fold(0_f32, f32::max) as f64,
        )
    }
    #[test]
    fn rain_shadow_converges_when_the_atmospheric_solver_is_refined() {
        let a = mountain_cross_section(192, 75_000.);
        let b = mountain_cross_section(192, 37_500.);
        let c = mountain_cross_section(192, 18_750.);
        let ab = (a.0 / a.1 / (b.0 / b.1) - 1.).abs();
        let bc = (b.0 / b.1 / (c.0 / c.1) - 1.).abs();
        assert!(
            bc < ab && bc < 0.15,
            "coarse {a:?}, medium {b:?}, fine {c:?}; drift {ab} -> {bc}"
        );
        assert!(
            (b.0 / c.0 - 1.).abs() < 0.15,
            "rainfall drift: {b:?}, {c:?}"
        );
    }
    #[test]
    fn windward_rain_and_lee_shadow_survive_a_change_in_sampling_density() {
        let a = mountain_cross_section(64, 75_000.);
        let b = mountain_cross_section(128, 75_000.);
        assert!(
            a.0 > a.1 * 1.3 && b.0 > b.1 * 1.3,
            "windward/lee: {a:?}, {b:?}"
        );
        assert!(
            (a.0 / b.0 - 1.).abs() < 0.25,
            "resolution changed rainfall: {a:?}, {b:?}"
        );
        assert!(
            a.2 < 15000. && b.2 < 15000.,
            "unphysical saturation clipping: {a:?}, {b:?}"
        );
    }
}
