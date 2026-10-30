//! Offline quality evidence; outputs only to the explicitly selected artifact folder.
use tf_grid::GridOps;
fn main() -> anyhow::Result<()> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "artifacts/generation-v03".into());
    std::fs::create_dir_all(&output)?;
    let mut params = tf_core::params::GenerationParams {
        seed: 42,
        quality: tf_core::params::SimQuality::Economy,
        ..Default::default()
    };
    let mode = std::env::args().nth(2).unwrap_or_else(|| "sphere".into());
    if mode != "sphere" {
        use tf_core::params::{LevelKind, WorldShape};
        params.world_shape = WorldShape::Plane;
        params.start_level = match mode.as_str() {
            "continent" => LevelKind::Continent,
            "region" => LevelKind::Region,
            "site" => LevelKind::Site,
            _ => LevelKind::World,
        };
        let width = match params.start_level {
            LevelKind::Region => 200_000.,
            LevelKind::Site => 10_000.,
            _ => 8_000_000.,
        };
        params.map_width_m = width;
        params.map_height_m = width * 0.65;
    }
    let descriptor = if mode == "sphere" {
        tf_grid::GridDescriptor::sphere(256, params.planet.radius_m)
    } else {
        tf_grid::GridDescriptor::plane(
            25.,
            0.,
            params.map_width_m,
            512,
            384,
            params.planet.radius_m,
        )
    };
    descriptor.validate().map_err(anyhow::Error::msg)?;
    let grid = descriptor.build();
    let start = std::time::Instant::now();
    let world = tf_sim::generate(&grid, &params, &mut |stage, progress| {
        eprintln!("{} {:.0}%", stage.name_zh(), progress * 100.)
    });
    let elapsed = start.elapsed().as_secs_f64();
    let biome: Vec<f32> = world.ecology.biome.iter().map(|&b| b as f32).collect();
    let png =
        tf_server::render::render_map_png(&world.fields.elevation, &biome, &descriptor, 1536)?;
    std::fs::write(format!("{output}/terrain.png"), png)?;
    let boundaries = tf_sim::plates::find_boundaries(
        &grid,
        &world.fields.plate_id,
        &world.tectonic,
        &world.fields,
    );
    let mut land: Vec<_> = world
        .fields
        .elevation
        .iter()
        .copied()
        .filter(|h| *h > 0.)
        .collect();
    land.sort_by(f32::total_cmp);
    let mut rain: Vec<_> = world
        .fields
        .precipitation
        .iter()
        .enumerate()
        .filter(|(i, _)| world.fields.elevation[*i] > 0.)
        .map(|(_, r)| *r)
        .collect();
    rain.sort_by(f32::total_cmp);
    let percentile = |v: &Vec<f32>, q: f64| v[((v.len() - 1) as f64 * q) as usize];
    let report = serde_json::json!({"seed":42,"domain":mode,"cells":grid.cell_count(),"seconds":elapsed,"land_fraction":land.len() as f64/grid.cell_count() as f64,"elevation_m":{"p10":percentile(&land,0.1),"p50":percentile(&land,0.5),"p90":percentile(&land,0.9),"p99":percentile(&land,0.99)},"rainfall_mm_year":{"p10":percentile(&rain,0.1),"p50":percentile(&rain,0.5),"p90":percentile(&rain,0.9),"p99":percentile(&rain,0.99)},"boundaries":{"convergent":boundaries.iter().filter(|b|b.kind==tf_sim::BoundaryKind::Convergent).count(),"divergent":boundaries.iter().filter(|b|b.kind==tf_sim::BoundaryKind::Divergent).count(),"transform":boundaries.iter().filter(|b|b.kind==tf_sim::BoundaryKind::Transform).count()}});
    let text = serde_json::to_string_pretty(&report)?;
    println!("{text}");
    std::fs::write(format!("{output}/report.json"), text)?;
    // A diagnostic precipitation atlas uses its own legend and contains actual field values.
    let width = 1536_u32;
    let height = 768_u32;
    let mut pixels = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            let cell = match &grid {
                tf_grid::MapGrid::Sphere(g) => g.cell_at(tf_core::geo::UnitVec3::from_lat_lon_deg(
                    90. - (y as f64 + 0.5) / height as f64 * 180.,
                    (x as f64 + 0.5) / width as f64 * 360. - 180.,
                )),
                tf_grid::MapGrid::Plane(g) => g
                    .cell_at_xy(
                        ((x as f64 + 0.5) / width as f64 - 0.5) * g.width(),
                        (0.5 - (y as f64 + 0.5) / height as f64) * g.height(),
                    )
                    .unwrap_or(0),
                _ => unreachable!(),
            };
            let p = world.fields.precipitation[cell] as f64;
            let t = (p / 3000.).clamp(0., 1.).sqrt();
            let a = [225., 204., 152.];
            let b = [41., 112., 133.];
            for c in 0..3 {
                pixels.push((a[c] * (1. - t) + b[c] * t) as u8);
            }
        }
    }
    let file = std::fs::File::create(format!("{output}/precipitation.png"))?;
    let mut enc = png::Encoder::new(file, width, height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    writer.write_image_data(&pixels)?;
    writer.finish()?;
    Ok(())
}
