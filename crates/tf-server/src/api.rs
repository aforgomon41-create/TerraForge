//! HTTP API。
//!
//! 路由分组：
//! - /api/v1/worlds       世界 CRUD
//! - /api/v1/maps         地图节点
//! - /api/v1/jobs         作业
//! - /api/v1/events       SSE 事件流
//! - /api/v1/system       系统状态与管理

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json,
    },
    routing::{delete, get, patch, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::jobs::{now_millis, JobEvent, JobManager};
use crate::store::{Channel, MapNodeRecord, Store, WorldRecord};

/// 应用状态。
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub jobs: JobManager,
}

/// 组装路由。
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/worlds", post(create_world).get(list_worlds))
        .route("/api/v1/worlds/{id}", get(get_world).delete(delete_world))
        .route("/api/v1/worlds/{id}/maps", get(list_maps))
        .route("/api/v1/worlds/{id}/generate", post(generate_world))
        .route("/api/v1/maps/{id}", get(get_map))
        .route("/api/v1/maps/{id}/field/{channel}", get(get_field))
        .route("/api/v1/maps/{id}/preview", get(get_preview))
        .route("/api/v1/maps/{id}/children", post(generate_child))
        .route("/api/v1/maps/{id}/export.json", get(export_json))
        .route("/api/v1/maps/{id}/export.png", get(export_png))
        .route("/api/v1/jobs/{id}", get(get_job))
        .route("/api/v1/jobs/{id}/cancel", post(cancel_job))
        .route("/api/v1/events", get(sse_events))
        .route("/api/v1/system/status", get(system_status))
        .route("/api/v1/system/concurrency", patch(set_concurrency))
        .with_state(state)
}

// ---------------- 健康与状态 ----------------

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "terraforge",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Serialize)]
struct SystemStatus {
    max_concurrent: usize,
    active_jobs: usize,
    worlds: usize,
    uptime_secs: u64,
}

static START_TIME: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

async fn system_status(State(st): State<AppState>) -> Json<SystemStatus> {
    let start = START_TIME.get_or_init(std::time::Instant::now);
    let active = st.jobs.max_concurrent();
    Json(SystemStatus {
        max_concurrent: active,
        active_jobs: if active > 0 { active } else { 0 },
        worlds: 0,
        uptime_secs: start.elapsed().as_secs(),
    })
}

#[derive(Deserialize)]
struct ConcurrencyBody {
    max_concurrent: usize,
}

async fn set_concurrency(
    State(st): State<AppState>,
    Json(body): Json<ConcurrencyBody>,
) -> Json<serde_json::Value> {
    st.jobs.set_max_concurrent(body.max_concurrent).await;
    Json(serde_json::json!({ "ok": true, "max_concurrent": st.jobs.max_concurrent() }))
}

// ---------------- 世界 ----------------

#[derive(Deserialize)]
struct CreateWorldBody {
    name: String,
    seed: u64,
    #[serde(default)]
    params: Option<tf_core::params::GenerationParams>,
}

#[derive(Serialize)]
struct WorldResponse {
    id: String,
    name: String,
    seed: u64,
    created_at: i64,
    share_token: Option<String>,
}

fn owner_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-tf-client")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "anonymous".to_string())
}

fn short_id(prefix: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{}-{}", prefix, &id[..12])
}

async fn create_world(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateWorldBody>,
) -> Result<Json<WorldResponse>, ApiError> {
    let mut params = body.params.unwrap_or_default();
    params.seed = body.seed;

    // 部署形态决定精度档位，客户端不能越权。
    // 网页版锁死 Economy（世界层 f=256，约 65 万格）；
    // 本地部署和桌面版可以用 Standard 或 High。
    if let Ok(level) = std::env::var("TF_QUALITY") {
        params.quality = match level.to_lowercase().as_str() {
            "economy" | "low" => tf_core::params::SimQuality::Economy,
            "high" => tf_core::params::SimQuality::High,
            _ => tf_core::params::SimQuality::Standard,
        };
    }

    if let Err(e) = params.validate() {
        return Err(ApiError::bad_request(e));
    }

    let id = short_id("world");
    let owner = owner_from_headers(&headers);
    let now = now_millis() as i64;

    let rec = WorldRecord {
        id: id.clone(),
        name: body.name.clone(),
        seed: body.seed,
        params_json: serde_json::to_string(&params)?,
        created_at: now,
        updated_at: now,
        owner_token: owner,
        share_token: None,
    };
    st.store.put_world(&rec)?;

    Ok(Json(WorldResponse {
        id,
        name: body.name,
        seed: body.seed,
        created_at: now,
        share_token: None,
    }))
}

async fn list_worlds(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<Vec<WorldResponse>>, ApiError> {
    let owner = owner_from_headers(&headers);
    let worlds = st.store.list_worlds(&owner)?;
    Ok(Json(
        worlds
            .into_iter()
            .map(|w| WorldResponse {
                id: w.id,
                name: w.name,
                seed: w.seed,
                created_at: w.created_at,
                share_token: w.share_token,
            })
            .collect(),
    ))
}

async fn get_world(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    let w = st.store.get_world(&id)?.ok_or_else(|| ApiError::not_found("世界不存在"))?;
    Ok(Json(serde_json::json!({
        "id": w.id,
        "name": w.name,
        "seed": w.seed,
        "params": serde_json::from_str::<serde_json::Value>(&w.params_json).unwrap_or(serde_json::Value::Null),
        "created_at": w.created_at,
        "share_token": w.share_token,
    })))
}

async fn delete_world(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    st.store.delete_world(&id)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------------- 生成 ----------------

async fn generate_world(
    State(st): State<AppState>,
    Path(world_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let world = st.store.get_world(&world_id)?.ok_or_else(|| ApiError::not_found("世界不存在"))?;
    let params: tf_core::params::GenerationParams = serde_json::from_str(&world.params_json)?;

    let map_id = short_id("map");
    let freq = params.quality.world_subdiv_freq();
    let cell_size = params.planet.surface_area_m2() / (10.0 * freq as f64 * freq as f64 + 2.0);

    let node = MapNodeRecord {
        id: map_id.clone(),
        world_id: world_id.clone(),
        kind: "world".to_string(),
        parent: None,
        cells: 10 * freq * freq + 2,
        cell_size_m: cell_size.sqrt(),
        extent_json: "{\"type\":\"sphere\"}".to_string(),
        status: "pending".to_string(),
        created_at: now_millis() as i64,
        completed_at: None,
    };
    st.store.put_map_node(&node)?;

    let job_id = st.jobs.submit(map_id.clone(), world_id, node, params).await?;

    Ok(Json(serde_json::json!({
        "job_id": job_id,
        "map_id": map_id,
    })))
}

async fn list_maps(State(st): State<AppState>, Path(world_id): Path<String>) -> Result<Json<Vec<MapNodeRecord>>, ApiError> {
    Ok(Json(st.store.list_map_nodes(&world_id)?))
}

async fn get_map(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<MapNodeRecord>, ApiError> {
    let m = st.store.get_map_node(&id)?.ok_or_else(|| ApiError::not_found("地图不存在"))?;
    Ok(Json(m))
}

// ---------------- 场数据 ----------------

fn parse_channel(name: &str) -> Option<Channel> {
    Some(match name {
        "elevation" => Channel::Elevation,
        "crust_thickness" => Channel::CrustThickness,
        "crust_type" => Channel::CrustType,
        "crust_age" => Channel::CrustAge,
        "temperature" => Channel::Temperature,
        "precipitation" => Channel::Precipitation,
        "flow_accumulation" => Channel::FlowAccumulation,
        "lake_depth" => Channel::LakeDepth,
        "distance_to_sea" => Channel::DistanceToSea,
        "biome" => Channel::Biome,
        "soil" => Channel::Soil,
        "soil_depth" => Channel::SoilDepth,
        _ => return None,
    })
}

async fn get_field(
    State(st): State<AppState>,
    Path((map_id, ch)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let channel = parse_channel(&ch).ok_or_else(|| ApiError::bad_request(format!("未知通道: {}", ch)))?;
    let data = st
        .store
        .get_field(&map_id, channel)?
        .ok_or_else(|| ApiError::not_found("该地图还没有这个通道的数据"))?;

    // 转成二进制返回，比 JSON 小很多
    let mut bytes = Vec::with_capacity(data.len() * 4);
    for v in &data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    Ok(([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], bytes))
}

async fn get_preview(
    State(st): State<AppState>,
    Path(map_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    // 返回高程的降采样 PNG 预览（简化版：返回原始 f32 数据让前端自己渲染）
    let data = st
        .store
        .get_field(&map_id, Channel::Elevation)?
        .ok_or_else(|| ApiError::not_found("该地图还没有生成"))?;
    let mut bytes = Vec::with_capacity(data.len() * 4);
    for v in &data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    Ok(([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], bytes))
}

// ---------------- 作业 ----------------

async fn get_job(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    let j = st.store.get_job(&id)?.ok_or_else(|| ApiError::not_found("作业不存在"))?;
    Ok(Json(serde_json::json!({
        "id": j.id,
        "map_id": j.map_id,
        "state": j.state,
        "stage": j.stage,
        "progress": j.progress,
        "error": j.error,
        "created_at": j.created_at,
        "started_at": j.started_at,
        "finished_at": j.finished_at,
    })))
}

async fn cancel_job(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    let ok = st.jobs.cancel(&id).await;
    Ok(Json(serde_json::json!({ "ok": ok })))
}

// ---------------- SSE ----------------

async fn sse_events(
    State(st): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = st.jobs.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| {
        let event = match msg {
            Ok(ev) => encode_event(ev),
            Err(_) => return None,
        };
        Some(Ok(event))
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn encode_event(ev: JobEvent) -> Event {
    match ev {
        JobEvent::State { job_id, state } => Event::default()
            .event("state")
            .data(serde_json::json!({ "job_id": job_id, "state": state }).to_string()),
        JobEvent::Progress {
            job_id,
            stage,
            progress,
            eta_secs,
        } => Event::default().event("progress").data(
            serde_json::json!({
                "job_id": job_id,
                "stage": stage,
                "progress": progress,
                "eta_secs": eta_secs,
            })
            .to_string(),
        ),
        JobEvent::Stage { job_id, from, to } => Event::default().event("stage").data(
            serde_json::json!({ "job_id": job_id, "from": from, "to": to }).to_string(),
        ),
        JobEvent::Frame {
            job_id,
            tick,
            width,
            height,
            data,
            stats,
        } => {
            use base64_lite::encode;
            Event::default().event("frame").data(
                serde_json::json!({
                    "job_id": job_id,
                    "tick": tick,
                    "w": width,
                    "h": height,
                    "fmt": "zstd+u16le+i8+u8x3",
                    "stats": {
                        "land_fraction": stats.land_fraction,
                        "max_elevation": stats.max_elevation,
                        "min_elevation": stats.min_elevation,
                    },
                    "data": encode(&data),
                })
                .to_string(),
            )
        }
        JobEvent::Done { job_id, map_id, duration_ms } => Event::default().event("done").data(
            serde_json::json!({ "job_id": job_id, "map_id": map_id, "duration_ms": duration_ms })
                .to_string(),
        ),
        JobEvent::Failed { job_id, error } => Event::default()
            .event("failed")
            .data(serde_json::json!({ "job_id": job_id, "error": error }).to_string()),
    }
}

// base64 编码的最小实现，避免为了一个函数引入依赖
mod base64_lite {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(data: &[u8]) -> String {
        let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let triple = (b0 << 16) | (b1 << 8) | b2;
            out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
            out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
            out.push(if chunk.len() > 1 {
                CHARS[((triple >> 6) & 0x3F) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                CHARS[(triple & 0x3F) as usize] as char
            } else {
                '='
            });
        }
        out
    }
}


#[derive(Deserialize)]
struct ChildRequest {
    /// 子图类型：continent / region / site
    kind: String,
    /// 父图上选定的中心经度、纬度
    center_lon: f64,
    center_lat: f64,
    /// 覆盖范围的总宽度（米）
    width_m: f64,
    /// 可选的名称
    name: Option<String>,
}

/// 生成子图。
///
/// 这是「树形嵌套」的核心：子图以父图的某块区域为基础，
/// 用更高的分辨率重新跑物理模拟，但受父图约束。
async fn generate_child(
    State(st): State<AppState>,
    Path(parent_id): Path<String>,
    Json(body): Json<ChildRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let parent = st
        .store
        .get_map_node(&parent_id)?
        .ok_or_else(|| ApiError::not_found("父图不存在"))?;

    let world = st
        .store
        .get_world(&parent.world_id)?
        .ok_or_else(|| ApiError::not_found("世界不存在"))?;

    // 层级与格宽
    let (level_kind, cell_size_m) = match body.kind.as_str() {
        "continent" => ("continent", 1_000.0),
        "region" => ("region", 100.0),
        "site" => ("site", 1.0),
        other => return Err(ApiError::bad_request(format!("未知层级: {}", other))),
    };

    // 分辨率 = 覆盖宽度 / 格宽，限制在合理范围
    let target_cells = (body.width_m / cell_size_m).round() as u32;
    let cells = target_cells.clamp(256, 4096);

    let map_id = short_id("map");

    // 子种子：由世界种子 + 父图 id 派生，保证可复现
    let mut params: tf_core::params::GenerationParams =
        serde_json::from_str(&world.params_json)?;
    let path_seed = tf_core::rng::derive_child_seed(
        params.seed,
        &[hash_str(&parent_id), hash_str(&format!("{:.4},{:.4}", body.center_lon, body.center_lat))],
    );
    params.seed = path_seed;

    let node = MapNodeRecord {
        id: map_id.clone(),
        world_id: parent.world_id.clone(),
        kind: level_kind.to_string(),
        parent: Some(parent_id.clone()),
        cells,
        cell_size_m,
        extent_json: serde_json::json!({
            "type": "patch",
            "center_lon": body.center_lon,
            "center_lat": body.center_lat,
            "width_m": body.width_m,
            "parent": parent_id,
        })
        .to_string(),
        status: "pending".to_string(),
        created_at: now_millis() as i64,
        completed_at: None,
    };
    st.store.put_map_node(&node)?;

    let job_id = st
        .jobs
        .submit(map_id.clone(), parent.world_id.clone(), node, params)
        .await?;

    Ok(Json(serde_json::json!({
        "job_id": job_id,
        "map_id": map_id,
        "kind": level_kind,
        "cells": cells,
        "cell_size_m": cell_size_m,
    })))
}

/// 把字符串散列成 u128，用于种子派生路径。
fn hash_str(s: &str) -> u128 {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let out = h.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&out[..16]);
    u128::from_le_bytes(bytes)
}


// ---------------- 导出 ----------------

/// 导出完整世界数据为 JSON。
async fn export_json(
    State(st): State<AppState>,
    Path(map_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    use base64_lite::encode;

    let node = st
        .store
        .get_map_node(&map_id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;

    let world = st
        .store
        .get_world(&node.world_id)?
        .ok_or_else(|| ApiError::not_found("世界不存在"))?;

    let mut channels = serde_json::Map::new();
    for ch in [
        Channel::Elevation,
        Channel::CrustThickness,
        Channel::CrustType,
        Channel::CrustAge,
        Channel::Temperature,
        Channel::Precipitation,
        Channel::FlowAccumulation,
        Channel::LakeDepth,
        Channel::DistanceToSea,
        Channel::Biome,
        Channel::Soil,
        Channel::SoilDepth,
    ] {
        if let Some(data) = st.store.get_field(&map_id, ch)? {
            let mut bytes = Vec::with_capacity(data.len() * 4);
            for v in &data {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            channels.insert(
                ch.name().to_string(),
                serde_json::json!({
                    "encoding": "f32le+base64",
                    "count": data.len(),
                    "data": encode(&bytes),
                }),
            );
        }
    }

    let payload = serde_json::json!({
        "format": "terraforge-world-v1",
        "exported_at": now_millis(),
        "world": {
            "id": world.id,
            "name": world.name,
            "seed": world.seed,
            "params": serde_json::from_str::<serde_json::Value>(&world.params_json)
                .unwrap_or(serde_json::Value::Null),
        },
        "map": {
            "id": node.id,
            "kind": node.kind,
            "parent": node.parent,
            "cells": node.cells,
            "cell_size_m": node.cell_size_m,
        },
        "channels": channels,
    });

    let body = serde_json::to_vec_pretty(&payload)?;
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "application/json"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"world.json\"",
            ),
        ],
        body,
    ))
}

/// 导出高分辨率 PNG 地图。
async fn export_png(
    State(st): State<AppState>,
    Path(map_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    let size: u32 = params
        .get("size")
        .and_then(|s| s.parse().ok())
        .unwrap_or(2048)
        .clamp(256, 8192);

    let _node = st
        .store
        .get_map_node(&map_id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;

    let elevation = st
        .store
        .get_field(&map_id, Channel::Elevation)?
        .ok_or_else(|| ApiError::not_found("该地图还没有生成"))?;

    let biome = st
        .store
        .get_field(&map_id, Channel::Biome)?
        .unwrap_or_else(|| vec![0.0; elevation.len()]);

    let png = crate::render::render_map_png(&elevation, &biome, size)?;

    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "image/png"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"map.png\"",
            ),
        ],
        png,
    ))
}

// ---------------- 错误处理 ----------------

pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        Self {
            status,
            message: msg.into(),
        }
    }
    fn bad_request(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, msg)
    }
    fn not_found(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, msg)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(serde_json::json!({ "error": self.message }))).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, format!("{:#}", e.into()))
    }
}
