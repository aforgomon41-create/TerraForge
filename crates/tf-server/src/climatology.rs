//! Bounded, compressed monthly climate storage. Public metadata contains only
//! diagnostics; the full atlas is decoded only by explicit climate queries/refinement.
use base64::{engine::general_purpose::STANDARD, Engine};
use std::io::Read;
use tf_sim::seasonal::ClimateAtlas;

const MAX_RAW: usize = 16 * 1024 * 1024;

pub fn encode(atlas: &ClimateAtlas) -> anyhow::Result<serde_json::Value> {
    atlas.validate().map_err(anyhow::Error::msg)?;
    let bytes = serde_json::to_vec(atlas)?;
    anyhow::ensure!(bytes.len() <= MAX_RAW, "月气候资料超过限制");
    let compressed = zstd::stream::encode_all(bytes.as_slice(), 3)?;
    Ok(
        serde_json::json!({"encoding":"json+zstd+base64","bytes":bytes.len(),"data":STANDARD.encode(compressed)}),
    )
}

pub fn decode(value: &serde_json::Value) -> anyhow::Result<ClimateAtlas> {
    anyhow::ensure!(value["encoding"] == "json+zstd+base64", "月气候编码无效");
    let expected = value["bytes"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("月气候长度无效"))? as usize;
    anyhow::ensure!(expected <= MAX_RAW, "月气候资料超过限制");
    let data = value["data"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("缺少月气候数据"))?;
    anyhow::ensure!(data.len() <= MAX_RAW * 2, "月气候压缩资料过大");
    let compressed = STANDARD.decode(data)?;
    let mut decoder = zstd::stream::read::Decoder::new(compressed.as_slice())?;
    decoder.window_log_max(24)?;
    let mut raw = Vec::new();
    decoder.take(MAX_RAW as u64 + 1).read_to_end(&mut raw)?;
    anyhow::ensure!(raw.len() == expected, "月气候长度不匹配");
    let atlas: ClimateAtlas = serde_json::from_slice(&raw)?;
    atlas.validate().map_err(anyhow::Error::msg)?;
    Ok(atlas)
}

pub fn for_map(
    store: &crate::store::Store,
    node: &crate::store::MapNodeRecord,
) -> anyhow::Result<Option<ClimateAtlas>> {
    let mut root = node.clone();
    let mut depth = 0;
    while let Some(id) = &root.parent {
        depth += 1;
        anyhow::ensure!(depth <= 4, "气候继承层级无效");
        root = store
            .get_map_node(id)?
            .ok_or_else(|| anyhow::anyhow!("缺少气候父图"))?;
        anyhow::ensure!(root.world_id == node.world_id, "气候父图不属于此工程");
    }
    let metadata = store.get_metadata(&root.id)?.unwrap_or_default();
    metadata.get("climate_atlas").map(decode).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn atlas() -> ClimateAtlas {
        let grid = tf_grid::GridDescriptor::sphere(32, 6371000.);
        let n = grid.cell_count();
        ClimateAtlas {
            grid,
            model: "test".into(),
            axial_tilt_deg: 23.44,
            temperature_c: vec![vec![15.; n]; 12],
            precipitation_mm: vec![vec![60.; n]; 12],
            pet_mm: vec![vec![40.; n]; 12],
            energy: Default::default(),
            water: Default::default(),
        }
    }
    #[test]
    fn coarse_spherical_monthly_state_round_trips_through_production_codec() {
        let source = atlas();
        let encoded = encode(&source).unwrap();
        let restored = decode(&encoded).unwrap();
        assert_eq!(restored.grid, source.grid);
        assert_eq!(restored.temperature_c, source.temperature_c);
        assert_eq!(restored.precipitation_mm, source.precipitation_mm);
        let mut corrupt = encoded;
        corrupt["bytes"] = serde_json::json!(1);
        assert!(decode(&corrupt).is_err());
    }
    #[test]
    fn both_codec_directions_reject_unsupported_or_unbounded_atlases() {
        let mut source = atlas();
        source.grid = tf_grid::GridDescriptor::sphere(64, 6371000.);
        assert!(encode(&source).is_err());
        source.grid = tf_grid::GridDescriptor::sphere(u32::MAX, 6371000.);
        assert!(encode(&source).is_err());
        assert!(decode(
            &serde_json::json!({"encoding":"json+zstd+base64","bytes":MAX_RAW+1,"data":""})
        )
        .is_err());
    }
}
