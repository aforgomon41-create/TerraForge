//! Saved, bounded seasonal cryosphere. Month=0 is the canonical annual view.
use crate::{
    api::{ApiError, AppState},
    store::{Channel, Store},
};
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Read;
use tf_sim::cryosphere::{Cryosphere, CryosphereAtlas};
const MAX_RAW: usize = 16 * 1024 * 1024;
pub const CHANNELS: [Channel; 8] = [
    Channel::LandIce,
    Channel::SnowCover,
    Channel::SeaIce,
    Channel::IceShelf,
    Channel::Permafrost,
    Channel::ActiveLayer,
    Channel::LiquidWater,
    Channel::LakeIce,
];

pub fn encode(atlas: &CryosphereAtlas) -> anyhow::Result<Value> {
    atlas.validate().map_err(anyhow::Error::msg)?;
    let raw = serde_json::to_vec(atlas)?;
    anyhow::ensure!(raw.len() <= MAX_RAW, "冰雪资料过大");
    Ok(
        json!({"encoding":"cryosphere-v1+zstd+base64","bytes":raw.len(),"data":STANDARD.encode(zstd::encode_all(raw.as_slice(),3)?)}),
    )
}
pub fn decode(value: &Value) -> anyhow::Result<CryosphereAtlas> {
    anyhow::ensure!(
        value["encoding"] == "cryosphere-v1+zstd+base64",
        "冰雪编码无效"
    );
    let size = value["bytes"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("缺少冰雪长度"))?;
    anyhow::ensure!(size <= MAX_RAW as u64, "冰雪资料过大");
    let encoded = value["data"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("缺少冰雪资料"))?;
    anyhow::ensure!(encoded.len() <= MAX_RAW * 2, "冰雪编码过大");
    let packed = STANDARD.decode(encoded)?;
    let mut decoder = zstd::stream::read::Decoder::new(packed.as_slice())?;
    decoder.window_log_max(24)?;
    let mut raw = Vec::new();
    decoder.take(size + 1).read_to_end(&mut raw)?;
    anyhow::ensure!(raw.len() == size as usize, "冰雪资料长度不符");
    let atlas: CryosphereAtlas = serde_json::from_slice(&raw)?;
    atlas.validate().map_err(anyhow::Error::msg)?;
    Ok(atlas)
}
pub fn fields(ice: &Cryosphere) -> [&[f32]; 8] {
    [
        &ice.land_ice_m,
        &ice.snow_cover,
        &ice.sea_ice_cover,
        &ice.shelf_ice_m,
        &ice.permafrost,
        &ice.active_layer_m,
        &ice.liquid_water_mm,
        &ice.lake_ice_cover,
    ]
}
pub fn attach(metadata: &mut Value, ice: &Cryosphere) -> anyhow::Result<()> {
    metadata["cryosphere_atlas"] = encode(&ice.atlas)?;
    metadata["cryosphere_model"] = json!({"name":ice.atlas.model,"ice_spinup_years":ice.atlas.spinup_years,
        "ice_budget":ice.atlas.ice_budget,"sea_ice":"seasonal Stefan growth; -1.8 C reference seawater freezing",
        "permafrost":"equilibrium TTOP proxy; not observed ground-ice content"});
    Ok(())
}
pub fn for_map(
    store: &Store,
    node: &crate::store::MapNodeRecord,
) -> anyhow::Result<Option<CryosphereAtlas>> {
    // Every newly refined child saves its own atlas; old children may inherit.
    let mut current = node.clone();
    for _ in 0..4 {
        if let Some(value) = store
            .get_metadata(&current.id)?
            .and_then(|v| v.get("cryosphere_atlas").cloned())
        {
            return Ok(Some(decode(&value)?));
        }
        let Some(parent) = &current.parent else {
            return Ok(None);
        };
        current = store
            .get_map_node(parent)?
            .ok_or_else(|| anyhow::anyhow!("缺少冰雪父图"))?;
        anyhow::ensure!(current.world_id == node.world_id, "冰雪父图不属于此工程");
    }
    anyhow::bail!("冰雪继承层级无效")
}
#[derive(Deserialize)]
pub struct IceQuery {
    #[serde(default)]
    pub month: u8,
}
pub async fn get_cryosphere(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<IceQuery>,
) -> Result<Response, ApiError> {
    if query.month > 12 {
        return Err(ApiError::bad_request("冰雪月份应为0～12"));
    }
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
        .map_err(|_| ApiError::busy("冰雪资料正在读取"))?;
    let result=tokio::task::spawn_blocking(move||->anyhow::Result<Response>{
        let _permit=permit;
        let Some(atlas)=for_map(&st.store,&node)? else{return Ok(Json(json!({"available":false,"reason":"旧工程未保存冰雪状态，请重新生成后查看海冰、冰架与冻土"})).into_response());};
        let count=node.cells as usize;
        let data:Vec<Vec<f32>>=if query.month==0 {
            CHANNELS.iter().map(|&c|st.store.get_field(&id,c)?.ok_or_else(||anyhow::anyhow!("缺少冰雪字段，请重新生成"))).collect::<anyhow::Result<_>>()?
        } else {
            let grid=node.grid.as_ref().ok_or_else(||anyhow::anyhow!("缺少地图网格"))?.build();
            let mut f=tf_sim::WorldFields::with_capacity(count);
            f.elevation=st.store.get_field(&id,Channel::Elevation)?.ok_or_else(||anyhow::anyhow!("缺少高程"))?;
            f.temperature=st.store.get_field(&id,Channel::Temperature)?.ok_or_else(||anyhow::anyhow!("缺少温度"))?;
            f.precipitation=st.store.get_field(&id,Channel::Precipitation)?.ok_or_else(||anyhow::anyhow!("缺少降水"))?;
            f.lake_depth=st.store.get_field(&id,Channel::LakeDepth)?.unwrap_or_else(||vec![0.;count]);
            f.climate=crate::climatology::for_map(&st.store,&node)?;
            if f.climate.is_none(){return Ok(Json(json!({"available":false,"reason":"此工程缺少月气候，无法查看季节冰雪，请重新生成"})).into_response());}
            let mut ice=tf_sim::cryosphere::sample(&grid,&f,atlas,query.month);
            for i in 0..count {if f.lake_depth[i]>0.1 {ice.permafrost[i]=0.;ice.active_layer_m[i]=0.;}}
            fields(&ice).iter().map(|v|v.to_vec()).collect()
        };
        anyhow::ensure!(data.iter().all(|v|v.len()==count),"冰雪字段长度无效");
        let header=serde_json::to_vec(&json!({"available":true,"model":tf_sim::cryosphere::MODEL,"month":query.month,
            "count":count,"liquid_water_units":if query.month==0 {"mm/year"}else{"selected-month annualized mm/year"},"channels":CHANNELS.iter().map(|c|c.name()).collect::<Vec<_>>()}))?;
        let mut out=Vec::with_capacity(4+header.len()+count*32);out.extend_from_slice(&(header.len() as u32).to_le_bytes());out.extend_from_slice(&header);
        for channel in data {for v in channel {out.extend_from_slice(&v.to_le_bytes());}}
        Ok(([("content-type","application/octet-stream"),("cache-control","private, no-store")],out).into_response())
    }).await??;
    Ok(result)
}
