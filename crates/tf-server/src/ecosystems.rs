//! Shared taxonomy is global; per-map observations always use map ownership auth.
use crate::{
    api::{ApiError, AppState},
    store::{Channel, Store},
};
use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};
use tf_sim::ecosystems::{Ecosystems, VERSION};
pub const CHANNELS: [Channel; 6] = [
    Channel::EcosystemBiome,
    Channel::EcosystemGroup,
    Channel::EcosystemHabitatBiome,
    Channel::EcosystemHabitatGroup,
    Channel::EcosystemVariant,
    Channel::EcosystemEvidence,
];

pub fn attach(metadata: &mut Value, e: &Ecosystems) {
    let mut groups = std::collections::BTreeMap::<u16, usize>::new();
    for &g in &e.group {
        if g > 0 {
            *groups.entry(g).or_default() += 1;
        }
    }
    metadata["ecosystem_model"] = json!({"version":VERSION,"classification":"IUCN GET v2.1; environment-based estimates",
        "resolved_ground_cells":e.group.iter().filter(|&&g|g>0).count(),"unresolved_ground_cells":e.group.iter().filter(|&&g|g==0).count(),
        "ground_group_counts":groups,"habitants_are_separate":true,"variants":"project environment descriptors, not formal IUCN classes"});
}

pub fn load(store: &Store, id: &str, n: usize) -> anyhow::Result<Option<Ecosystems>> {
    let data = CHANNELS
        .iter()
        .map(|&ch| store.get_field(id, ch))
        .collect::<anyhow::Result<Vec<_>>>()?;
    if data.iter().all(Option::is_none) {
        return Ok(None);
    }
    anyhow::ensure!(
        data.iter()
            .all(|v| v.as_ref().is_some_and(|v| v.len() == n)),
        "生态分级资料不完整，请重新生成"
    );
    let d = data.into_iter().map(Option::unwrap).collect::<Vec<_>>();
    Ok(Some(from_channels(&d, n)?))
}
pub fn from_channels(d: &[Vec<f32>], n: usize) -> anyhow::Result<Ecosystems> {
    anyhow::ensure!(
        d.len() == 6 && d.iter().all(|v| v.len() == n),
        "生态字段长度无效"
    );
    anyhow::ensure!(
        d.iter()
            .flatten()
            .all(|v| v.is_finite() && v.fract() == 0. && *v >= 0. && *v <= u16::MAX as f32),
        "生态分级资料损坏"
    );
    anyhow::ensure!(
        d[0].iter().chain(&d[2]).all(|&v| v <= 25.) && d[4].iter().all(|&v| v <= 63.),
        "生态类别编号无效"
    );
    let e = Ecosystems {
        biome: d[0].iter().map(|&v| v as u8).collect(),
        group: d[1].iter().map(|&v| v as u16).collect(),
        habitat_biome: d[2].iter().map(|&v| v as u8).collect(),
        habitat_group: d[3].iter().map(|&v| v as u16).collect(),
        variant: d[4].iter().map(|&v| v as u8).collect(),
        evidence: d[5].iter().map(|&v| v as u16).collect(),
    };
    anyhow::ensure!(e.validate(n), "生态类别与上级群系不一致");
    Ok(e)
}

pub async fn catalogue() -> Json<&'static tf_sim::ecosystems::Catalogue> {
    Json(tf_sim::ecosystems::catalogue())
}
pub async fn get(
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
        .map_err(|_| ApiError::busy("生态资料正在读取"))?;
    let payload=tokio::task::spawn_blocking(move||->anyhow::Result<(Vec<u8>, &'static str)>{
        let _permit=permit;let n=node.cells as usize;
        let Some(e)=load(&st.store,&id,n)? else{return Ok((serde_json::to_vec(&json!({"available":false,"reason":"旧工程未保存生态细分类，仍可查看原生态；重新生成可补齐"}))?,"application/json"));};
        let header=serde_json::to_vec(&json!({"available":true,"version":VERSION,"count":n,"channels":tf_sim::ecosystems::CHANNEL_NAMES,
            "notice":"环境条件识别；未模拟的过程保留大类，细类编号0表示待定。地方变体不是正式IUCN细类。"}))?;
        let mut out=Vec::with_capacity(4+header.len()+n*24);out.extend_from_slice(&(header.len() as u32).to_le_bytes());out.extend_from_slice(&header);
        for values in e.channels(){for v in values {out.extend_from_slice(&v.to_le_bytes());}}
        Ok((out,"application/octet-stream"))
    }).await??;
    let (payload, content_type) = payload;
    Ok((
        [
            ("content-type", content_type),
            ("cache-control", "private, no-store"),
        ],
        payload,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_fractional_ids_wrong_hierarchy_and_truncated_u16_ids() {
        let good = vec![
            vec![25.],
            vec![403.],
            vec![1.],
            vec![19.],
            vec![21.],
            vec![275.],
        ];
        assert!(from_channels(&good, 1).is_ok());
        for (channel, value) in [
            (1, 403.5),
            (1, 147.),
            (1, 408.),
            (4, 64.),
            (5, 65535.),
            (0, 19.),
        ] {
            let mut bad = good.clone();
            bad[channel][0] = value;
            assert!(
                from_channels(&bad, 1).is_err(),
                "channel {channel}: {value}"
            );
        }
    }
}
