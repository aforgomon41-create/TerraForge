//! Read an exported baseline project without regenerating or mutating it.
use base64::{engine::general_purpose::STANDARD, Engine};
use tf_grid::GridOps;
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("exported .tfworld path");
    let project: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let root = project["maps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["node"]["parent"].is_null())
        .unwrap();
    let desc: tf_grid::GridDescriptor = serde_json::from_value(root["node"]["grid"].clone())?;
    let grid = desc.build();
    let bytes = zstd::stream::decode_all(
        STANDARD
            .decode(root["channels"]["elevation"]["data"].as_str().unwrap())?
            .as_slice(),
    )?;
    anyhow::ensure!(bytes.len() == grid.cell_count() * 4, "channel length");
    let elevation: Vec<_> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    if let Some(output) = std::env::args().nth(2) {
        let biome_bytes = zstd::stream::decode_all(
            STANDARD
                .decode(root["channels"]["biome"]["data"].as_str().unwrap())?
                .as_slice(),
        )?;
        anyhow::ensure!(biome_bytes.len() == grid.cell_count() * 4, "biome length");
        let biome: Vec<_> = biome_bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let png = tf_server::render::render_map_png(&elevation, &biome, &desc, 1536)?;
        std::fs::write(output, png)?;
    }
    let mut seen = vec![false; grid.cell_count()];
    let mut components = Vec::new();
    for i in 0..grid.cell_count() {
        if seen[i] || elevation[i] <= 0. {
            continue;
        }
        seen[i] = true;
        let mut queue = vec![i];
        let mut cursor = 0;
        let mut area = 0.;
        while cursor < queue.len() {
            let j = queue[cursor];
            cursor += 1;
            area += grid.area_m2(j);
            for &k in grid.neighbors(j) {
                let k = k as usize;
                if !seen[k] && elevation[k] > 0. {
                    seen[k] = true;
                    queue.push(k);
                }
            }
        }
        components.push(area);
    }
    components.sort_by(|a, b| b.total_cmp(a));
    let area = components.iter().sum::<f64>();
    println!(
        "{}",
        serde_json::json!({"seed":project["world"]["seed"],"cells":grid.cell_count(),"largest_land_share":components.first().copied().unwrap_or(0.)/area.max(1.),"top_three_share":components.iter().take(3).sum::<f64>()/area.max(1.),"small_land_components_share":components.iter().filter(|&&x|x<1e11).sum::<f64>()/area.max(1.),"landmasses_over_100000_km2":components.iter().filter(|&&x|x>=1e11).count()})
    );
    Ok(())
}
