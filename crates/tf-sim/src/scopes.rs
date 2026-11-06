//! Standalone regional/site maps use a local landscape and a prescribed climate background.
use crate::{climate::ClimateParams, TectonicState, WorldFields};
use tf_core::{
    noise::SimplexNoise,
    params::{GenerationParams, LandscapeKind},
    DeterministicRng,
};
use tf_grid::GridOps;
#[derive(Clone, Copy)]
struct Reach {
    a: (f64, f64),
    b: (f64, f64),
    za: f64,
    zb: f64,
}
impl Reach {
    fn surface(&self, u: f64, v: f64, relief: f64, shape: f64) -> f64 {
        let dx = self.b.0 - self.a.0;
        let dy = self.b.1 - self.a.1;
        let t = (((u - self.a.0) * dx + (v - self.a.1) * dy) / (dx * dx + dy * dy).max(1e-12))
            .clamp(0., 1.);
        let d = (u - self.a.0 - t * dx).hypot(v - self.a.1 - t * dy);
        self.za * (1. - t) + self.zb * t + relief * (d / 0.25).powf(shape)
    }
}
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
    // A connected, downhill trunk and tributaries define valleys and divides.
    // Noise adds small rock/soil texture, rather than deciding every landform.
    let along = (c.abs() * w + s.abs() * h) / size;
    let across = (s.abs() * w + c.abs() * h) / size;
    let trunk = |u: f64| 0.06 * (u * 5. + angle).sin() + 0.025 * (u * 11. + angle * 2.).sin();
    let relief = params.terrain_relief_m;
    let (valley_relief, gradient, shape) = match params.landscape {
        LandscapeKind::Mountains => (relief * 0.75, relief * 0.45, 0.85),
        LandscapeKind::Valley => (relief * 0.6, relief * 0.12, 1.15),
        LandscapeKind::Hills => (relief * 0.26, relief * 0.13, 1.1),
        LandscapeKind::Plains => (relief * 0.025, relief * 0.035, 1.4),
        LandscapeKind::Coast => (relief * 0.22, relief * 0.6, 1.05),
    };
    let bed = |u: f64| {
        if params.landscape == LandscapeKind::Coast {
            relief * (0.1 - u) * 0.6
        } else {
            params.base_elevation_m + gradient * (0.5 - u / along)
        }
    };
    let mut reaches = Vec::new();
    for k in 0..24 {
        let a = -along * 0.6 + along * 1.2 * k as f64 / 24.;
        let b = -along * 0.6 + along * 1.2 * (k + 1) as f64 / 24.;
        reaches.push(Reach {
            a: (a, trunk(a)),
            b: (b, trunk(b)),
            za: bed(a),
            zb: bed(b),
        });
    }
    for k in 0..14 {
        let side = if k % 2 == 0 { 1. } else { -1. };
        let join = -along * 0.42 + along * 0.84 * (k as f64 + rng.range_f64(0.1, 0.9)) / 14.;
        let head = (
            join - rng.range_f64(0.07, 0.27) * along,
            side * rng.range_f64(0.24, 0.62) * across,
        );
        let mid = (
            (head.0 + join) / 2. - rng.range_f64(-0.04, 0.04),
            head.1 * 0.52 + trunk(join) * 0.48,
        );
        let z = bed(join);
        let rise = valley_relief * rng.range_f64(0.12, 0.38);
        reaches.push(Reach {
            a: head,
            b: mid,
            za: z + rise,
            zb: z + rise * 0.4,
        });
        reaches.push(Reach {
            a: mid,
            b: (join, trunk(join)),
            za: z + rise * 0.4,
            zb: z,
        });
        let fork = (
            mid.0 - rng.range_f64(0.04, 0.16) * along,
            mid.1 + side * rng.range_f64(0.12, 0.25) * across,
        );
        reaches.push(Reach {
            a: fork,
            b: mid,
            za: z + rise * 0.9,
            zb: z + rise * 0.4,
        });
    }
    for i in 0..grid.cell_count() {
        let (x, y) = grid.local_center_m(i).unwrap();
        let u = (c * x + s * y) / size;
        let v = (-s * x + c * y) / size;
        let detail = noise.fbm(u * 6., v * 6., 31., 4, 2., 0.5);
        let broad = noise.fbm(u * 1.8, v * 1.8, 17., 3, 2., 0.5);
        let valley = reaches
            .iter()
            .map(|r| r.surface(u, v, valley_relief, shape))
            .fold(f64::INFINITY, f64::min);
        let texture = relief
            * if params.landscape == LandscapeKind::Plains {
                0.003
            } else {
                0.018
            }
            * (detail + 0.25 * broad);
        let height = if params.landscape == LandscapeKind::Coast {
            // An indented continental edge and estuary share the same drainage
            // base level. Tributaries continue inland instead of island chains.
            let shore = relief * (0.10 - u + 0.065 * broad + 0.018 * detail) * 1.2;
            shore.min(valley + texture)
        } else {
            valley + texture
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
