//! Bounded geographic catalogue + three cell membership channels. This is derived
//! data, never trusted on project import and never counted as additional maps.
use crate::{
    api::{ApiError, AppState},
    store::{Channel, MapNodeRecord, Store},
};
use axum::extract::{Path, State};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::io::Read;
use tf_grid::GridOps;
use tf_sim::geography::{Feature, Geography};

const MAX_RAW: usize = 128 * 1024 * 1024;
pub const KEY: &str = "geography_v1";
#[derive(serde::Serialize, serde::Deserialize)]
struct Catalogue {
    version: u32,
    cell_count: usize,
    resolution_m: f64,
    features: Vec<Feature>,
    omitted_sites: usize,
}
pub fn bytes(g: &Geography) -> anyhow::Result<Vec<u8>> {
    g.validate().map_err(anyhow::Error::msg)?;
    let json = serde_json::to_vec(&Catalogue {
        version: g.version,
        cell_count: g.cell_count,
        resolution_m: g.resolution_m,
        features: g.features.clone(),
        omitted_sites: g.omitted_sites,
    })?;
    let size = 4 + json.len() + 12 * g.cell_count;
    anyhow::ensure!(size <= MAX_RAW, "地理资料超过限制");
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    for labels in [&g.landmass, &g.region, &g.site] {
        for &id in labels {
            out.extend_from_slice(&id.to_le_bytes());
        }
    }
    Ok(out)
}
pub(crate) fn parse(raw: &[u8], n: usize) -> anyhow::Result<Geography> {
    anyhow::ensure!(raw.len() >= 4 && raw.len() <= MAX_RAW, "地理资料长度无效");
    let len = u32::from_le_bytes(raw[..4].try_into()?) as usize;
    anyhow::ensure!(
        len <= raw.len() - 4 && raw.len() == 4 + len + 12 * n,
        "地理归属尺寸不匹配"
    );
    let c: Catalogue = serde_json::from_slice(&raw[4..4 + len])?;
    anyhow::ensure!(c.cell_count == n, "地理格数不匹配");
    let labels: Vec<u32> = raw[4 + len..]
        .chunks_exact(4)
        .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
        .collect();
    let g = Geography {
        version: c.version,
        cell_count: n,
        resolution_m: c.resolution_m,
        features: c.features,
        omitted_sites: c.omitted_sites,
        landmass: labels[..n].to_vec(),
        region: labels[n..2 * n].to_vec(),
        site: labels[2 * n..].to_vec(),
    };
    g.validate().map_err(anyhow::Error::msg)?;
    Ok(g)
}
pub fn encode(g: &Geography) -> anyhow::Result<serde_json::Value> {
    let raw = bytes(g)?;
    Ok(
        serde_json::json!({"encoding":"tf-geography-v1+zstd+base64","bytes":raw.len(),
        "data":STANDARD.encode(zstd::stream::encode_all(raw.as_slice(),3)?)}),
    )
}
fn decode(value: &serde_json::Value, n: usize) -> anyhow::Result<Geography> {
    anyhow::ensure!(
        value["encoding"] == "tf-geography-v1+zstd+base64",
        "地理资料编码无效"
    );
    let size = value["bytes"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("缺少地理资料长度"))?;
    anyhow::ensure!(size <= MAX_RAW as u64, "地理资料超过限制");
    let data = value["data"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("缺少地理资料"))?;
    anyhow::ensure!(data.len() <= MAX_RAW * 2, "地理压缩资料过大");
    let packed = STANDARD.decode(data)?;
    let mut decoder = zstd::stream::read::Decoder::new(packed.as_slice())?;
    decoder.window_log_max(27)?;
    let mut raw = Vec::new();
    decoder.take(size + 1).read_to_end(&mut raw)?;
    anyhow::ensure!(raw.len() == size as usize, "地理资料长度不匹配");
    parse(&raw, n)
}

/// Rebuild old/imported maps from the authoritative elevation and receiver graph.
fn for_map(store: &Store, node: &MapNodeRecord) -> anyhow::Result<Geography> {
    if let Some(value) = store
        .get_metadata(&node.id)?
        .and_then(|mut m| m.as_object_mut()?.remove(KEY))
    {
        if let Ok(g) = decode(&value, node.cells as usize) {
            return Ok(g);
        }
    }
    let grid = node
        .grid
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("旧地图缺少空间信息，请重新生成"))?
        .build();
    let elevation = store
        .get_field(&node.id, Channel::Elevation)?
        .ok_or_else(|| anyhow::anyhow!("缺少地形"))?;
    let mut fields = tf_sim::WorldFields::with_capacity(grid.cell_count());
    fields.elevation = elevation;
    if let Some(rain) = store.get_field(&node.id, Channel::Precipitation)? {
        fields.precipitation = rain;
    }
    if let Some(pet) = store.get_field(&node.id, Channel::PotentialEvaporation)? {
        fields.potential_evaporation = pet;
    }
    let water = tf_sim::erosion::refresh_hydrology(&grid, &mut fields);
    let g = tf_sim::geography::recognize(&grid, &fields, &water, &|| false)
        .map_err(anyhow::Error::msg)?;
    store.cache_geography(&node.id, &encode(&g)?)?;
    Ok(g)
}

pub async fn get_geography(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let node = st
        .store
        .get_map_node(&id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    if node.status != "done" {
        return Err(ApiError::bad_request("地图尚未完成"));
    }
    let permit = st
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("正在准备地图资料，请稍后重试"))?;
    let payload = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let _permit = permit;
        bytes(&for_map(&st.store, &node)?)
    })
    .await??;
    Ok((
        [
            ("content-type", "application/octet-stream"),
            ("cache-control", "private, no-store"),
        ],
        payload,
    ))
}

/// An optional derived catalogue must not prevent publication of a completed world.
pub fn attach<G: tf_grid::GridOps>(
    metadata: &mut serde_json::Value,
    grid: &G,
    fields: &tf_sim::WorldFields,
    water: &tf_sim::erosion::ErosionState,
    cancel: &dyn Fn() -> bool,
) {
    match tf_sim::geography::recognize(grid, fields, water, cancel)
        .map_err(anyhow::Error::msg)
        .and_then(|g| encode(&g))
    {
        Ok(value) => metadata[KEY] = value,
        Err(e) => {
            tracing::warn!("地理识别未完成: {e}");
            metadata["geography_error"] = serde_json::json!(e.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_codec_and_memberships_round_trip() {
        let grid = tf_grid::GridDescriptor::plane(0., 0., 20_000., 20, 20, 6_371_000.).build();
        let mut fields = tf_sim::WorldFields {
            elevation: vec![50.; 400],
            lake_depth: vec![0.; 400],
            flow_accumulation: vec![0.; 400],
            ..Default::default()
        };
        let water = tf_sim::erosion::refresh_hydrology(&grid, &mut fields);
        let g = tf_sim::geography::recognize(&grid, &fields, &water, &|| false).unwrap();
        let encoded = encode(&g).unwrap();
        let restored = decode(&encoded, 400).unwrap();
        assert_eq!(g.landmass, restored.landmass);
        assert_eq!(g.region, restored.region);
        assert_eq!(g.site, restored.site);
        assert_eq!(g.features.len(), restored.features.len());
        for (a, b) in g.features.iter().zip(&restored.features) {
            assert_eq!(
                (a.id, a.parent, a.anchor_cell, a.cell_count),
                (b.id, b.parent, b.anchor_cell, b.cell_count)
            );
            assert!((a.area_m2 - b.area_m2).abs() < a.area_m2 * 1e-12);
            assert!((a.lat - b.lat).abs() < 1e-12 && (a.lon - b.lon).abs() < 1e-12);
        }
        assert!(decode(&encoded, 401).is_err());
        let mut corrupt = encoded.clone();
        corrupt["bytes"] = serde_json::json!(MAX_RAW + 1);
        assert!(decode(&corrupt, 400).is_err());
        let mut raw = bytes(&g).unwrap();
        raw.pop();
        assert!(parse(&raw, 400).is_err());
    }
}
