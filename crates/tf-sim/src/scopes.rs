//! Standalone regional/site maps use a local landscape and a prescribed climate background.
use crate::{climate::ClimateParams, TectonicState, WorldFields};
use tf_core::{
    noise::SimplexNoise,
    params::{GenerationParams, LandscapeKind},
    DeterministicRng,
};
use tf_grid::GridOps;
pub fn local_terrain<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    params: &GenerationParams,
) -> TectonicState {
    let noise = SimplexNoise::new(params.seed ^ 0x10CA_1E17);
    let mut rng = DeterministicRng::derive(params.seed, "local-landscape");
    let angle = rng.range_f64(0., std::f64::consts::TAU);
    let (s, c) = angle.sin_cos();
    let (w, h) = grid
        .extent_m()
        .expect("local roots require a finite domain");
    let size = w.min(h);
    for i in 0..grid.cell_count() {
        let (x, y) = grid.local_center_m(i).unwrap();
        let u = (c * x + s * y) / size;
        let v = (-s * x + c * y) / size;
        let detail = noise.fbm(u * 6., v * 6., 31., 4, 2., 0.5);
        let broad = noise.fbm(u * 1.8, v * 1.8, 17., 3, 2., 0.5);
        let channel = v - 0.10 * (u * 7. + angle).sin() - 0.05 * broad;
        let height = match params.landscape {
            LandscapeKind::Hills => {
                params.base_elevation_m + params.terrain_relief_m * (0.35 * broad + 0.10 * detail)
            }
            LandscapeKind::Plains => {
                params.base_elevation_m + params.terrain_relief_m * (0.10 * broad + 0.03 * detail)
            }
            LandscapeKind::Mountains => {
                params.base_elevation_m
                    + params.terrain_relief_m
                        * ((-channel.powi(2) / 0.045).exp() * (0.75 + 0.2 * detail) + 0.15 * broad)
            }
            LandscapeKind::Valley => {
                params.base_elevation_m
                    + params.terrain_relief_m * ((channel.abs() * 2.).powf(0.8) + 0.05 * detail)
            }
            LandscapeKind::Coast => {
                params.terrain_relief_m * (u + 0.15 * broad + 0.025 * detail) * 2.
            }
        };
        fields.elevation[i] = height as f32;
        fields.crust_type[i] = if height > 0. { 1. } else { 0. };
        fields.crust_thickness[i] = if height > 0. { 35000. } else { 7000. };
        fields.crust_age[i] = if height > 0. { 600. } else { 60. };
    }
    TectonicState {
        plates: vec![],
        convection: vec![],
        planar_velocities: None,
        centers: vec![],
        partition_scale: size,
        partition_seed: params.seed,
        partition_weights: vec![],
        history: Default::default(),
        planar_spin_rad_myr: None,
        provinces: Vec::new(),
        lifecycle: Default::default(),
    }
}
pub fn local_climate<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    params: &GenerationParams,
    climate: &ClimateParams,
    cancel: &dyn Fn() -> bool,
) -> bool {
    local_climate_observed(
        grid,
        fields,
        params,
        climate,
        cancel,
        &mut crate::preview::NoFrames,
    )
}
pub fn local_climate_observed<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    params: &GenerationParams,
    climate: &ClimateParams,
    cancel: &dyn Fn() -> bool,
    observer: &mut dyn crate::preview::FrameObserver,
) -> bool {
    for i in 0..grid.cell_count() {
        fields.temperature[i] = (params.regional_temperature_c
            - (fields.elevation[i] as f64 - params.base_elevation_m) * 0.0065)
            .clamp(-100., 90.) as f32;
    }
    let mut local = climate.clone();
    local.prescribed_temperature = true;
    if !crate::climate_transport::solve_observed(
        grid,
        fields,
        &params.planet,
        &local,
        cancel,
        observer,
    ) {
        return false;
    }
    let area = (0..grid.cell_count()).map(|i| grid.area_m2(i)).sum::<f64>();
    let mean = (0..grid.cell_count())
        .map(|i| fields.precipitation[i] as f64 * grid.area_m2(i))
        .sum::<f64>()
        / area;
    // Local maps prescribe their external climatic background; this is not a closed planet budget.
    for p in &mut fields.precipitation {
        *p = if mean > 1e-8 {
            (*p as f64 * params.regional_precipitation_mm / mean) as f32
        } else {
            params.regional_precipitation_mm as f32
        };
    }
    let scale = if mean > 1e-8 {
        params.regional_precipitation_mm / mean
    } else {
        0.
    };
    if let Some(atlas) = &mut fields.climate {
        for month in &mut atlas.precipitation_mm {
            for p in month {
                *p = (*p as f64 * scale) as f32;
            }
        }
        atlas.water.prescribed_output_scale = Some(scale);
    }
    for i in 0..fields.len() {
        fields.aridity[i] = fields.precipitation[i] / fields.potential_evaporation[i].max(1.);
    }
    !cancel()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_background_climate_uses_its_own_temperature_and_rainfall_settings() {
        let mut p = GenerationParams::for_level(tf_core::params::LevelKind::Region);
        p.regional_temperature_c = 12.;
        p.regional_precipitation_mm = 800.;
        let g = tf_grid::FlatGrid::new(24, 18, p.map_width_m, 25., 0., p.planet.radius_m);
        let mut f = WorldFields::with_capacity(g.cell_count());
        local_terrain(&g, &mut f, &p);
        assert!(local_climate(
            &g,
            &mut f,
            &p,
            &ClimateParams::default(),
            &|| false
        ));
        let mean = f.precipitation.iter().map(|&x| x as f64).sum::<f64>() / f.len() as f64;
        assert!((mean - 800.).abs() < 0.01);
        let old = f.temperature.clone();
        p.regional_temperature_c += 5.;
        p.regional_precipitation_mm = 0.;
        assert!(local_climate(
            &g,
            &mut f,
            &p,
            &ClimateParams::default(),
            &|| false
        ));
        assert!(old
            .iter()
            .zip(&f.temperature)
            .all(|(a, b)| (b - a - 5.).abs() < 0.0001));
        assert!(f.precipitation.iter().all(|&x| x == 0.));
    }
    #[test]
    fn a_site_has_local_relief_without_new_oceans_or_plates() {
        let p = GenerationParams::for_level(tf_core::params::LevelKind::Site);
        let g = tf_grid::FlatGrid::new(48, 36, p.map_width_m, 25., 0., p.planet.radius_m);
        let mut f = WorldFields::with_capacity(g.cell_count());
        let t = local_terrain(&g, &mut f, &p);
        let min = f.elevation.iter().copied().fold(f32::INFINITY, f32::min);
        let max = f
            .elevation
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(t.plates.is_empty());
        assert!(
            min > 0. && max - min > 5. && max - min < p.terrain_relief_m as f32,
            "{min}..{max}; relief {}",
            p.terrain_relief_m
        );
    }
}
