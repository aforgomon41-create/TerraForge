//! Diagnose physical height and biome distributions through the production path.
//! cargo run -p tf-sim --example hypsometry -- [seed] [sphere|plane] [full|terrain]
use tf_core::params::GenerationParams;
use tf_grid::{GridDescriptor, GridOps};
use tf_sim::{ecology::Biome, WorldFields};

fn report(grid: &impl GridOps, f: &WorldFields) -> serde_json::Value {
    let mut land: Vec<f32> = f.elevation.iter().copied().filter(|&h| h > 0.).collect();
    land.sort_unstable_by(f32::total_cmp);
    let sum = |test: &dyn Fn(usize) -> bool| {
        (0..grid.cell_count())
            .filter(|&i| test(i))
            .map(|i| grid.area_m2(i))
            .sum::<f64>()
    };
    let area = sum(&|i| f.elevation[i] > 0.);
    let p = |q: f64| {
        if land.is_empty() {
            0.
        } else {
            land[((land.len() - 1) as f64 * q) as usize]
        }
    };
    let stable: Vec<_> = (0..grid.cell_count())
        .filter(|&i| {
            f.elevation[i] > 0.
                && f.crust_type[i] > 0.95
                && (f.crust_thickness[i] - f.reference_crust_thickness[i]).abs() < 1000.
                && f.crustal_strain[i] < 0.1
        })
        .collect();
    let mut roughness = serde_json::Map::new();
    for spacing in [250_000., 500_000., 1_000_000.] {
        let (mut count, mut square, mut absolute) = (0, 0., 0.);
        for i in 0..grid.cell_count() {
            if f.elevation[i] <= 0.
                || f.distance_to_sea[i] < 300_000.
                || grid.latitude_deg(i).abs() > 50.
            {
                continue;
            }
            for bearing in [0., std::f64::consts::FRAC_PI_2] {
                let j = if grid.field_is_planar() {
                    let q = grid.field_position(i);
                    grid.locate_local(q.x + spacing * bearing.sin(), q.y + spacing * bearing.cos())
                } else {
                    grid.locate(
                        grid.center_dir(i)
                            .offset(bearing, spacing / grid.radius_m()),
                    )
                };
                let Some(j) = j else {
                    continue;
                };
                if j == i || f.elevation[j] <= 0. || f.distance_to_sea[j] < 300_000. {
                    continue;
                }
                let delta = (f.elevation[i] - f.elevation[j]) as f64;
                count += 1;
                square += delta * delta;
                absolute += delta.abs();
            }
        }
        roughness.insert(format!("{:.0}_km",spacing/1000.),serde_json::json!({"pairs":count,"rms_height_difference_m":(square/count.max(1) as f64).sqrt(),"mean_absolute_height_difference_m":absolute/count.max(1) as f64}));
    }
    serde_json::json!({"interior_nonpolar_roughness":roughness,"land_fraction":area/sum(&|_|true),"p10":p(0.1),"p50":p(0.5),"p90":p(0.9),"max":p(1.),
        "below_1000_share":sum(&|i|f.elevation[i]>0. && f.elevation[i]<1000.)/area.max(1.),
        "above_3000_share":sum(&|i|f.elevation[i]>3000.)/area.max(1.),"stable_cells":stable.len(),
        "stable_mean_height":stable.iter().map(|&i|f.elevation[i] as f64).sum::<f64>()/stable.len().max(1) as f64,
        "stable_mean_baseline_excess":stable.iter().map(|&i|(f.crust_thickness[i]-f.reference_crust_thickness[i]) as f64).sum::<f64>()/stable.len().max(1) as f64})
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut params = GenerationParams {
        seed: args.get(1).and_then(|v| v.parse().ok()).unwrap_or(42),
        quality: tf_core::params::SimQuality::Economy,
        ..Default::default()
    };
    if let Some(duration) = std::env::var("TF_DIAG_MYR")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
    {
        params.tectonic_age_myr = duration;
    }
    let plane = args.get(2).is_some_and(|s| s == "plane");
    if plane {
        params.world_shape = tf_core::params::WorldShape::Plane;
    }
    let grid = if plane {
        GridDescriptor::plane(0., 0., 32_000_000., 256, 148, params.planet.radius_m)
    } else {
        GridDescriptor::sphere(64, params.planet.radius_m)
    }
    .build();
    if args.get(3).is_some_and(|v| v == "full") {
        let w = tf_sim::generate(&grid, &params, &mut |_, _| {});
        let mut data = report(&grid, &w.fields);
        let land = w
            .fields
            .elevation
            .iter()
            .filter(|&&h| h > 0.)
            .count()
            .max(1) as f64;
        for (label, kind) in [
            ("tundra", Biome::Tundra),
            ("ice", Biome::IceSheet),
            ("alpine", Biome::AlpineMeadow),
        ] {
            data[label] = serde_json::json!(
                w.ecology.biome.iter().filter(|&&b| b == kind as u8).count() as f64 / land
            );
        }
        println!(
            "{}",
            serde_json::json!({"seed":params.seed,"plane":plane,"full":data})
        );
    } else {
        let mut f = WorldFields::with_capacity(grid.cell_count());
        let state =
            tf_sim::evolution::generate(&grid, &params, &mut f, &mut |_| {}, &|| false).unwrap();
        let before = report(&grid, &f);
        tf_sim::evolution::landforms(&grid, &mut f, &state, &params);
        tf_sim::erosion::compute_distance_to_sea(&grid, &mut f);
        println!(
            "{}",
            serde_json::json!({"seed":params.seed,"plane":plane,"duration_myr":params.tectonic_age_myr,"ancient_steps":state.history.ancient_steps,"recent_steps":state.history.steps,"budget_error":state.history.relative_budget_error,"material":before,"terrain":report(&grid,&f)})
        );
    }
}
