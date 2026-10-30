//! Small native adaptations of climlab and Landlab kernels (MIT).
//! Pinned sources, adaptation boundaries and complete notices are in third_party/.
use std::f64::consts::PI;

pub const DAY: f64 = 86_400.;
pub const DAYS_PER_YEAR: f64 = 365.25;

/// climlab.solar.insolation daily mean, circular orbit, solar longitude in radians.
/// Declination and polar day/night use the same astronomical geometry as climlab.
pub fn daily_insolation(
    latitude: f64,
    solar_longitude: f64,
    tilt: f64,
    solar_constant: f64,
) -> f64 {
    let declination = (tilt.sin() * solar_longitude.sin()).asin();
    let h = (-latitude.tan() * declination.tan()).clamp(-1., 1.).acos();
    solar_constant / PI
        * (h * latitude.sin() * declination.sin() + latitude.cos() * declination.cos() * h.sin())
            .max(0.)
}

/// climlab.utils.heat_capacity.ocean: J m^-2 K^-1, with water depth in metres.
pub fn ocean_heat_capacity(depth_m: f64) -> f64 {
    1000. * 4181.3 * depth_m
}

/// climlab.radiation.AplusBT, temperature in degrees Celsius, W m^-2.
pub fn outgoing_longwave(temperature_c: f64, a: f64, b: f64) -> f64 {
    a + b * temperature_c
}

/// Port of landlab.components.flexure.funcs.get_flexure_parameter (1-D section).
pub fn flexure_length(elastic_thickness_m: f64, youngs_pa: f64, mantle_weight: f64) -> f64 {
    let rigidity = youngs_pa * elastic_thickness_m.powi(3) / (12. * (1. - 0.25_f64.powi(2)));
    (4. * rigidity / mantle_weight).powf(0.25)
}

/// Landlab's one-dimensional line-load Green function; positive is downward.
/// The signed outer lobe represents a forebulge, rather than a second Gaussian ridge.
pub fn line_load_deflection(
    load_n_per_m: f64,
    distance_m: f64,
    alpha_m: f64,
    mantle_weight: f64,
) -> f64 {
    let r = distance_m.abs() / alpha_m;
    load_n_per_m / (2. * alpha_m * mantle_weight) * (-r).exp() * (r.cos() + r.sin())
}

/// Temperature/day-length PET (Thornthwaite); an empirical vegetation diagnostic,
/// not a replacement for the energy or atmospheric water budgets.
pub fn monthly_pet(temperatures: &[f32; 12], latitude: f64, tilt: f64) -> [f32; 12] {
    let heat_index = temperatures
        .iter()
        .map(|&t| (t.max(0.) as f64 / 5.).powf(1.514))
        .sum::<f64>();
    if heat_index < 1e-8 {
        return [0.; 12];
    }
    let exponent = 6.75e-7 * heat_index.powi(3) - 7.71e-5 * heat_index.powi(2)
        + 0.01792 * heat_index
        + 0.49239;
    std::array::from_fn(|month| {
        let t = temperatures[month] as f64;
        if t <= 0. {
            return 0.;
        }
        let longitude = crate::seasonal::solar_longitude(month);
        let declination = (tilt.sin() * longitude.sin()).asin();
        let daylight = 24. / PI * (-latitude.tan() * declination.tan()).clamp(-1., 1.).acos();
        let base = if t > 26.5 {
            let bounded = t.min(38.);
            (-415.85 + 32.24 * bounded - 0.43 * bounded * bounded).max(0.)
        } else {
            16. * (10. * t / heat_index).powf(exponent)
        };
        (base * daylight / 12. * (DAYS_PER_YEAR / 12. / 30.)) as f32
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_landlab_examples_and_climlab_heat_capacity_agree() {
        assert!((flexure_length(65000., 7e10, 33000.) - 119965.926).abs() < 0.001);
        assert_eq!(ocean_heat_capacity(50.), 209065000.);
        assert_eq!(outgoing_longwave(10., 210., 2.), 230.);
    }
    #[test]
    fn native_ports_agree_with_executed_upstream_scientific_code() {
        let reference: serde_json::Value = serde_json::from_str(include_str!(
            "../../../third_party/scientific-reference.json"
        ))
        .unwrap();
        for row in reference["insolation"].as_array().unwrap() {
            let actual = daily_insolation(
                row["latitude_deg"].as_f64().unwrap().to_radians(),
                row["solar_longitude_deg"].as_f64().unwrap().to_radians(),
                23.44_f64.to_radians(),
                1361.,
            );
            assert!((actual - row["insolation_w_m2"].as_f64().unwrap()).abs() < 1e-9);
        }
        let alpha = flexure_length(65000., 7e10, 33000.);
        for (index, distance) in [0., 10000., 100000.].iter().enumerate() {
            assert!(
                (line_load_deflection(1e9, *distance, alpha, 33000.)
                    - reference["landlab_deflection"][index].as_f64().unwrap())
                .abs()
                    < 1e-12
            );
        }
    }
    #[test]
    fn circular_orbit_has_the_correct_global_energy_input() {
        for tilt in [0., 23.44_f64.to_radians(), 60_f64.to_radians()] {
            for longitude in [0., PI * 0.5, PI, PI * 1.5] {
                let n = 4000;
                let mean = (0..n)
                    .map(|i| {
                        let sin_lat = -1. + 2. * (i as f64 + 0.5) / n as f64;
                        daily_insolation(sin_lat.asin(), longitude, tilt, 1361.)
                    })
                    .sum::<f64>()
                    / n as f64;
                assert!((mean - 1361. / 4.).abs() < 0.02, "{mean}");
            }
        }
    }
}
