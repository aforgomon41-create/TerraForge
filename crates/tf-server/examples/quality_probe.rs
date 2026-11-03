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
        use tf_core::params::{GenerationParams, LevelKind, WorldShape};
        let level = match mode.as_str() {
            "continent" => LevelKind::Continent,
            "region" => LevelKind::Region,
            "site" => LevelKind::Site,
            _ => LevelKind::World,
        };
        params = GenerationParams {
            seed: 42,
            quality: tf_core::params::SimQuality::Economy,
            ..GenerationParams::for_level(level)
        };
        params.world_shape = WorldShape::Plane;
    }
    params.seed = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(42);
    if let Some(count) = std::env::args().nth(4).and_then(|s| s.parse().ok()) {
        params.continent_count = count;
    }
    params.allow_land_at_edge = !std::env::args().any(|s| s == "--closed-coast");
    let descriptor = tf_server::api::root_descriptor(&params, 512).map_err(anyhow::Error::msg)?;
    descriptor.validate().map_err(anyhow::Error::msg)?;
    let grid = descriptor.build();
    let start = std::time::Instant::now();
    let mut last = (String::new(), u32::MAX);
    let world = tf_sim::generate(&grid, &params, &mut |stage, progress| {
        let now = (
            stage.name_zh().to_string(),
            (progress * 100.).round() as u32,
        );
        if now != last {
            eprintln!("{} {}%", now.0, now.1);
            last = now;
        }
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
    let mut seen = vec![false; grid.cell_count()];
    let mut components = Vec::new();
    for i in 0..grid.cell_count() {
        if seen[i] || world.fields.elevation[i] <= 0. {
            continue;
        }
        seen[i] = true;
        let mut queue = vec![i];
        let mut area = 0.;
        let mut cursor = 0;
        while cursor < queue.len() {
            let j = queue[cursor];
            cursor += 1;
            area += grid.area_m2(j);
            for &k in grid.neighbors(j) {
                let k = k as usize;
                if !seen[k] && world.fields.elevation[k] > 0. {
                    seen[k] = true;
                    queue.push(k);
                }
            }
        }
        components.push(area);
    }
    components.sort_by(|a, b| b.total_cmp(a));
    let land_area = components.iter().sum::<f64>();
    let connected = serde_json::json!({"largest_land_share":components.first().copied().unwrap_or(0.)/land_area.max(1.),"top_three_share":components.iter().take(3).sum::<f64>()/land_area.max(1.),"small_land_components_share":components.iter().filter(|&&area|area<1e11).sum::<f64>()/land_area.max(1.),"landmasses_over_100000_km2":components.iter().filter(|&&area|area>=1e11).count()});
    let report = serde_json::json!({"seed":params.seed,"connectivity":connected,"domain":mode,"extent_m":grid.extent_m(),"cells":grid.cell_count(),"seconds":elapsed,"land_fraction":land.len() as f64/grid.cell_count() as f64,"elevation_m":{"p10":percentile(&land,0.1),"p50":percentile(&land,0.5),"p90":percentile(&land,0.9),"p99":percentile(&land,0.99)},"rainfall_mm_year":{"p10":percentile(&rain,0.1),"p50":percentile(&rain,0.5),"p90":percentile(&rain,0.9),"p99":percentile(&rain,0.99)},"boundaries":{"convergent":boundaries.iter().filter(|b|b.kind==tf_sim::BoundaryKind::Convergent).count(),"divergent":boundaries.iter().filter(|b|b.kind==tf_sim::BoundaryKind::Divergent).count(),"transform":boundaries.iter().filter(|b|b.kind==tf_sim::BoundaryKind::Transform).count()}});
    let mut report = report;
    report["allow_land_at_edge"] = serde_json::json!(params.allow_land_at_edge);
    report["edge_land_cells"] = serde_json::json!(grid
        .boundary_mask()
        .iter_ones()
        .filter(|&i| world.fields.elevation[i] > 0.)
        .count());
    let mut biomes = vec![0usize; 14];
    for (i, &b) in world.ecology.biome.iter().enumerate() {
        if world.fields.elevation[i] > 0. {
            biomes[b as usize] += 1;
        }
    }
    report["biome_land_shares"] = serde_json::json!(biomes
        .iter()
        .map(|n| *n as f64 / land.len().max(1) as f64)
        .collect::<Vec<_>>());
    let mut history = serde_json::to_value(&world.tectonic.history)?;
    let events = history["volcanoes"].as_array().map_or(0, Vec::len);
    history.as_object_mut().unwrap().remove("volcanoes");
    history["volcanic_events"] = serde_json::json!(events);
    report["tectonics"] = history;
    if let Some(atlas) = &world.fields.climate {
        atlas.validate().map_err(anyhow::Error::msg)?;
        // Exercise the production serializer, not only the in-memory solver.
        let encoded = tf_server::climatology::encode(atlas)?;
        let decoded = tf_server::climatology::decode(&encoded)?;
        report["climate"] = serde_json::json!({"model":decoded.model,"cells":decoded.grid.cell_count(),"energy":decoded.energy,"water":decoded.water});
    }
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
