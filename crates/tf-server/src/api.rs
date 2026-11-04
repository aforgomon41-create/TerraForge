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

use crate::jobs::{now_millis, JobEvent, JobManager};
use crate::store::{Channel, MapNodeRecord, Store, WorldRecord};

/// 应用状态。
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub jobs: JobManager,
    pub config: Arc<crate::config::ServerConfig>,
    pub exports: Arc<tokio::sync::Semaphore>,
    pub reads: Arc<tokio::sync::Semaphore>,
    pub streams: Arc<tokio::sync::Semaphore>,
    pub admission: Arc<tokio::sync::Mutex<Vec<u64>>>,
}

/// 组装路由。
pub fn build_router(state: AppState) -> Router {
    let body_limit = if state.config.desktop {
        512 * 1024 * 1024
    } else {
        64 * 1024 * 1024
    };
    let protected = Router::new()
        .route("/api/v1/worlds", post(create_world).get(list_worlds))
        .route("/api/v1/projects/import", post(import_project))
        .route("/api/v1/worlds/{id}", get(get_world).delete(delete_world))
        .route("/api/v1/worlds/{id}/maps", get(list_maps))
        .route("/api/v1/worlds/{id}/generate", post(generate_world))
        .route("/api/v1/maps/{id}", get(get_map))
        .route("/api/v1/maps/{id}/field/{channel}", get(get_field))
        .route(
            "/api/v1/maps/{id}/vectors/{kind}",
            get(crate::circulation::get_vectors),
        )
        .route("/api/v1/maps/{id}/preview", get(get_preview))
        .route("/api/v1/maps/{id}/grid", get(get_grid))
        .route(
            "/api/v1/maps/{id}/surface",
            get(crate::surface::get_surface),
        )
        .route(
            "/api/v1/maps/{id}/overlay",
            get(crate::surface::get_overlay),
        )
        .route("/api/v1/maps/{id}/metadata", get(get_metadata))
        .route(
            "/api/v1/maps/{id}/geography",
            get(crate::geography::get_geography),
        )
        .route("/api/v1/maps/{id}/climate", get(get_monthly_climate))
        .route(
            "/api/v1/ecosystem-catalogue",
            get(crate::ecosystems::catalogue),
        )
        .route("/api/v1/maps/{id}/ecosystems", get(crate::ecosystems::get))
        .route(
            "/api/v1/maps/{id}/cryosphere",
            get(crate::cryosphere::get_cryosphere),
        )
        .route(
            "/api/v1/maps/{id}/tectonics",
            get(crate::tectonics::get_tectonics),
        )
        .route("/api/v1/maps/{id}/sample", get(sample_map))
        .route("/api/v1/maps/{id}/children", post(generate_child))
        .route("/api/v1/maps/{id}/export.json", get(export_json))
        .route("/api/v1/maps/{id}/export.png", get(export_png))
        .route(
            "/api/v1/maps/{id}/hydrology",
            get(crate::hydrology::get_hydrology),
        )
        .route("/api/v1/jobs/{id}", get(get_job))
        .route("/api/v1/jobs/{id}/cancel", post(cancel_job))
        .route("/api/v1/events", get(sse_events))
        .route("/api/v1/system/status", get(system_status))
        .route("/api/v1/system/concurrency", patch(set_concurrency))
        .route(
            "/api/v1/session/heartbeat",
            post(|| async { Json(serde_json::json!({"ok":true})) }),
        )
        .route("/api/v1/session", delete(close_session))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::authorize,
        ));
    protected
        .route("/api/v1/health", get(health))
        .route("/api/v1/session", post(crate::auth::open_session))
        .layer(axum::extract::DefaultBodyLimit::max(body_limit))
        .with_state(state)
}

pub async fn expire_session(
    st: &AppState,
    session: &crate::auth::SessionRecord,
) -> anyhow::Result<()> {
    if !st.config.desktop {
        for world in st.store.list_worlds(&session.owner)? {
            st.jobs.delete_world(&world.id).await?;
        }
    }
    st.store.delete_session(&session.id)?;
    Ok(())
}
pub async fn expire_if_idle(st: &AppState, id: &str) -> anyhow::Result<()> {
    let _admission = st.admission.lock().await;
    if let Some(session) = st.store.get_session(id)? {
        if !st.config.desktop
            && now_millis().saturating_sub(session.last_seen)
                >= st.config.session_ttl.as_millis() as u64
        {
            expire_session(st, &session).await?;
        }
    }
    Ok(())
}
fn ensure_live(st: &AppState, owner: &str) -> Result<(), ApiError> {
    if !st.config.desktop && st.store.get_session(owner)?.is_none() {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "会话已结束，请重新打开页面",
        ));
    }
    Ok(())
}
async fn close_session(
    State(st): State<AppState>,
    axum::Extension(session): axum::Extension<crate::auth::SessionRecord>,
) -> Result<impl IntoResponse, ApiError> {
    let _admission = st.admission.lock().await;
    expire_session(&st, &session).await?;
    Ok((
        [(
            "set-cookie",
            "tf_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
        )],
        Json(serde_json::json!({"ok":true})),
    ))
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

async fn system_status(State(st): State<AppState>, headers: HeaderMap) -> Json<SystemStatus> {
    let start = START_TIME.get_or_init(std::time::Instant::now);
    let (active, _) = st.jobs.counts().await;
    Json(SystemStatus {
        max_concurrent: st.jobs.max_concurrent().await,
        active_jobs: active,
        worlds: st
            .store
            .list_worlds(&owner_from_headers(&headers))
            .map_or(0, |w| w.len()),
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
    Json(serde_json::json!({ "ok": true, "max_concurrent": st.jobs.max_concurrent().await }))
}

// ---------------- 世界 ----------------

#[derive(Deserialize)]
struct CreateWorldBody {
    name: String,
    seed: Option<u64>,
    #[serde(default)]
    params: Option<serde_json::Value>,
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
        .get("x-tf-owner")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
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
    let supplied = body.params.unwrap_or_else(|| serde_json::json!({}));
    let level = serde_json::from_value(
        supplied
            .get("start_level")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("world")),
    )
    .map_err(|_| ApiError::bad_request("起始地图层级无效"))?;
    let mut merged = serde_json::to_value(tf_core::params::GenerationParams::for_level(level))?;
    let overrides = supplied
        .as_object()
        .ok_or_else(|| ApiError::bad_request("参数必须是对象"))?;
    for (key, value) in overrides {
        if key == "planet" {
            let values = value
                .as_object()
                .ok_or_else(|| ApiError::bad_request("星球参数必须是对象"))?;
            for (k, v) in values {
                merged["planet"][k] = v.clone();
            }
        } else {
            merged[key] = value.clone();
        }
    }
    let mut params: tf_core::params::GenerationParams =
        serde_json::from_value(merged).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let seed = body
        .seed
        .unwrap_or_else(|| (uuid::Uuid::new_v4().as_u128() & ((1_u128 << 53) - 1)) as u64);
    params.seed = seed;

    // 部署形态决定精度档位，客户端不能越权。
    // 网页版锁死 Economy（世界层 f=256，约 65 万格）；
    // 本地部署和桌面版可以用 Standard 或 High。
    params.quality = st.config.quality;
    if body.name.trim().is_empty() || body.name.chars().count() > 80 {
        return Err(ApiError::bad_request("世界名称需要 1～80 个字符"));
    }
    let owner = owner_from_headers(&headers);
    let _admission = st.admission.lock().await;
    ensure_live(&st, &owner)?;
    if !st.config.desktop && st.store.world_count()? >= 8 {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "体验世界数量已满，请稍后重试",
        ));
    }
    if !st.config.desktop && st.store.list_worlds(&owner)?.len() >= 4 {
        return Err(ApiError::bad_request("当前体验会话最多创建 4 个世界"));
    }

    if let Err(e) = params.validate() {
        return Err(ApiError::bad_request(e));
    }

    let id = short_id("world");
    let now = now_millis() as i64;

    let rec = WorldRecord {
        id: id.clone(),
        name: body.name.clone(),
        seed,
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
        seed,
        created_at: now,
        share_token: None,
    }))
}

async fn list_worlds(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<WorldResponse>>, ApiError> {
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

async fn get_world(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let w = st
        .store
        .get_world(&id)?
        .ok_or_else(|| ApiError::not_found("世界不存在"))?;
    Ok(Json(serde_json::json!({
        "id": w.id,
        "name": w.name,
        "seed": w.seed,
        "params": serde_json::from_str::<serde_json::Value>(&w.params_json).unwrap_or(serde_json::Value::Null),
        "created_at": w.created_at,
        "share_token": w.share_token,
    })))
}

async fn delete_world(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    st.jobs.delete_world(&id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------------- 生成 ----------------

pub fn root_descriptor(
    params: &tf_core::params::GenerationParams,
    limit: u32,
) -> Result<tf_grid::GridDescriptor, String> {
    use tf_core::params::{LevelKind, WorldShape};
    let descriptor = if params.start_level == LevelKind::World
        && params.world_shape == WorldShape::Sphere
    {
        tf_grid::GridDescriptor::sphere(params.quality.world_subdiv_freq(), params.planet.radius_m)
    } else {
        let aspect = params.map_height_m / params.map_width_m;
        let columns = (limit as f64 / (aspect * 2. / 3_f64.sqrt()).max(1.)).floor();
        let row_count = aspect * columns * 2. / 3_f64.sqrt();
        if columns < 4. || row_count < 3.5 {
            return Err("当前精度无法保持这样狭长的地图比例，请调整宽度或高度".into());
        }
        let cols = columns as u32;
        let rows = row_count.round().clamp(4., limit as f64) as u32;
        tf_grid::GridDescriptor::plane(
            params.center_lat_deg,
            params.center_lon_deg,
            params.map_width_m,
            cols,
            rows,
            params.planet.radius_m,
        )
    };
    descriptor.validate()?;
    Ok(descriptor)
}

async fn generate_world(
    State(st): State<AppState>,
    Path(world_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _admission = st.admission.lock().await;
    let existing = st.store.list_map_nodes(&world_id)?;
    if !st.config.desktop && (existing.len() >= 16) {
        return Err(ApiError::bad_request("当前世界地图数量或总格数已达上限"));
    }
    let world = st
        .store
        .get_world(&world_id)?
        .ok_or_else(|| ApiError::not_found("世界不存在"))?;
    let params: tf_core::params::GenerationParams = serde_json::from_str(&world.params_json)?;

    let map_id = short_id("map");
    let descriptor =
        root_descriptor(&params, st.config.max_patch_side()).map_err(ApiError::bad_request)?;
    let cells = descriptor.cell_count() as u32;
    if !st.config.desktop
        && existing.iter().map(|m| m.cells as u64).sum::<u64>() + cells as u64 > 4_000_000
    {
        return Err(ApiError::bad_request("当前工程的总采样数已达上限"));
    }
    let kind = match params.start_level {
        tf_core::params::LevelKind::World => "world",
        tf_core::params::LevelKind::Continent => "continent",
        tf_core::params::LevelKind::Region => "region",
        tf_core::params::LevelKind::Site => "site",
    };
    let cell_size = match &descriptor {
        tf_grid::GridDescriptor::Sphere { .. } => {
            (params.planet.surface_area_m2() / cells as f64).sqrt()
        }
        tf_grid::GridDescriptor::Plane { width_m, size, .. } => width_m / *size as f64,
        _ => unreachable!(),
    };
    let node = MapNodeRecord {
        id: map_id.clone(),
        world_id: world_id.clone(),
        kind: kind.into(),
        name: format!("{}总图", params.start_level.name_zh()),
        parent: None,
        cells,
        cell_size_m: cell_size,
        extent_json: serde_json::to_string(&descriptor)?,
        grid: Some(descriptor),
        status: "pending".into(),
        created_at: now_millis() as i64,
        completed_at: None,
        seed: params.seed,
        generator_version: "tf-0.6".into(),
    };

    let job_id = st
        .jobs
        .submit(map_id.clone(), world_id, node, params)
        .await?;

    Ok(Json(serde_json::json!({
        "job_id": job_id,
        "map_id": map_id,
    })))
}

async fn list_maps(
    State(st): State<AppState>,
    Path(world_id): Path<String>,
) -> Result<Json<Vec<MapNodeRecord>>, ApiError> {
    Ok(Json(st.store.list_map_nodes(&world_id)?))
}

async fn get_map(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MapNodeRecord>, ApiError> {
    let m = st
        .store
        .get_map_node(&id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    Ok(Json(m))
}

// ---------------- 场数据 ----------------

fn parse_channel(name: &str) -> Option<Channel> {
    Channel::parse(name)
}

async fn get_field(
    State(st): State<AppState>,
    Path((map_id, ch)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let channel =
        parse_channel(&ch).ok_or_else(|| ApiError::bad_request(format!("未知通道: {}", ch)))?;
    field_bytes(st, map_id, channel).await
}
async fn field_bytes(
    st: AppState,
    map_id: String,
    channel: Channel,
) -> Result<impl IntoResponse, ApiError> {
    let permit =
        st.reads.clone().try_acquire_owned().map_err(|_| {
            ApiError::new(StatusCode::TOO_MANY_REQUESTS, "地图读取繁忙，请稍后重试")
        })?;
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let _permit = permit;
        let data = st
            .store
            .get_field(&map_id, channel)?
            .ok_or_else(|| anyhow::anyhow!("地图缺少此通道"))?;
        Ok(data.iter().flat_map(|v| v.to_le_bytes()).collect())
    })
    .await??;
    Ok(([("content-type", "application/octet-stream")], bytes))
}
async fn get_preview(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    field_bytes(st, id, Channel::Elevation).await
}

async fn get_grid(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let node = st
        .store
        .get_map_node(&id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    let descriptor = node
        .grid
        .ok_or_else(|| ApiError::bad_request("旧版地图缺少网格描述，请重新生成"))?;
    let permit = st.exports.clone().try_acquire_owned().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "正在准备地图数据，请稍后重试",
        )
    })?;
    let bytes = tokio::task::spawn_blocking(move || {
        use tf_grid::GridOps;
        let grid = descriptor.build();
        let mut out = Vec::with_capacity(grid.cell_count() * 12);
        for i in 0..grid.cell_count() {
            for x in grid.render_center(i) {
                out.extend_from_slice(&x.to_le_bytes())
            }
        }
        drop(permit);
        out
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(([("content-type", "application/octet-stream")], bytes))
}

#[derive(Deserialize)]
struct SampleQuery {
    lon: f64,
    lat: f64,
}
async fn sample_map(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SampleQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !q.lon.is_finite()
        || !q.lat.is_finite()
        || !(-180.0..=180.0).contains(&q.lon)
        || !(-90.0..=90.0).contains(&q.lat)
    {
        return Err(ApiError::bad_request("坐标无效"));
    }
    let node = st
        .store
        .get_map_node(&id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    if node.status != "done" {
        return Err(ApiError::bad_request("地图尚未完成"));
    }
    let descriptor = node
        .grid
        .ok_or_else(|| ApiError::bad_request("旧版地图需要重新生成"))?;
    let permit = st.exports.clone().try_acquire_owned().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "正在处理地图数据，请稍后重试",
        )
    })?;
    let store = st.store.clone();
    let out = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        let _permit = permit;
        let grid = descriptor.build();
        let dir = tf_core::geo::UnitVec3::from_lat_lon_deg(q.lat, q.lon);
        let cell = grid
            .cell_at(dir)
            .ok_or_else(|| anyhow::anyhow!("坐标超出地图范围"))?;
        let mut values = serde_json::Map::new();
        for ch in [
            Channel::Elevation,
            Channel::Temperature,
            Channel::Precipitation,
            Channel::Biome,
            Channel::FlowAccumulation,
            Channel::LakeDepth,
            Channel::PlateId,
            Channel::LandIce,
            Channel::SnowCover,
            Channel::SeaIce,
            Channel::IceShelf,
            Channel::Permafrost,
            Channel::ActiveLayer,
            Channel::LiquidWater,
            Channel::LakeIce,
            Channel::LiquidDischarge,
            Channel::EcosystemBiome,
            Channel::EcosystemGroup,
            Channel::EcosystemHabitatBiome,
            Channel::EcosystemHabitatGroup,
            Channel::EcosystemVariant,
            Channel::EcosystemEvidence,
        ] {
            if let Some(data) = store.get_field(&id, ch)? {
                if let Some(value) = data.get(cell) {
                    values.insert(ch.name().into(), serde_json::json!(value));
                }
            }
        }
        Ok(serde_json::json!({"cell":cell,"lon":q.lon,"lat":q.lat,"values":values}))
    })
    .await
    .map_err(|e| ApiError::bad_request(e.to_string()))??;
    Ok(Json(out))
}

// ---------------- 作业 ----------------

async fn get_job(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let j = st
        .store
        .get_job(&id)?
        .ok_or_else(|| ApiError::not_found("作业不存在"))?;
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

async fn cancel_job(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ok = st.jobs.cancel(&id).await;
    Ok(Json(serde_json::json!({ "ok": ok })))
}

// ---------------- SSE ----------------

async fn sse_events(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let id = query
        .get("job_id")
        .cloned()
        .ok_or_else(|| ApiError::bad_request("需要 job_id"))?;
    let job = st
        .store
        .get_job(&id)?
        .ok_or_else(|| ApiError::not_found("作业不存在"))?;
    let world = st
        .store
        .get_world(&job.world_id)?
        .ok_or_else(|| ApiError::not_found("世界不存在"))?;
    if world.owner_token != owner_from_headers(&headers) {
        return Err(ApiError::not_found("作业不存在"));
    }
    let permit = st
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::new(StatusCode::TOO_MANY_REQUESTS, "实时连接数量已达上限"))?;
    let rx = st.jobs.subscribe();
    let gzip = query.get("frame_encoding").is_some_and(|v| v == "gzip");
    // Subscribe before the second state read: terminal events cannot fall into
    // the gap between the persisted-state check and joining the live channel.
    let current = st.store.get_job(&id)?.unwrap_or(job);
    let terminal = terminal_event(&current);
    let finished = terminal.is_some();
    let mut pending = std::collections::VecDeque::new();
    if let Some(frame) = st.jobs.latest_frame(&id) {
        pending.push_back(encode_event(frame, gzip));
    }
    if let Some(event) = terminal {
        pending.push_back(event);
    }
    let jobs = st.jobs.clone();
    let store = st.store.clone();
    let stream = futures::stream::unfold(
        (rx, permit, pending, finished),
        move |(mut rx, permit, mut pending, mut finished)| {
            let id = id.clone();
            let jobs = jobs.clone();
            let store = store.clone();
            async move {
                let event = if let Some(event) = pending.pop_front() {
                    event
                } else {
                    if finished {
                        return None;
                    }
                    loop {
                        match rx.recv().await {
                            Ok(ev) if ev.job_id() == id => {
                                finished = matches!(
                                    &ev,
                                    JobEvent::Done { .. } | JobEvent::Failed { .. }
                                ) || matches!(&ev, JobEvent::State { state, .. } if matches!(state.as_str(), "done" | "failed" | "cancelled"));
                                if finished {
                                    break store
                                        .get_job(&id)
                                        .ok()
                                        .flatten()
                                        .as_ref()
                                        .and_then(terminal_event)
                                        .unwrap_or_else(|| encode_event(ev, gzip));
                                }
                                break encode_event(ev, gzip);
                            }
                            Ok(_) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                if let Some(event) = store
                                    .get_job(&id)
                                    .ok()
                                    .flatten()
                                    .as_ref()
                                    .and_then(terminal_event)
                                {
                                    finished = true;
                                    break event;
                                }
                                break jobs
                                    .latest_frame(&id)
                                    .map(|ev| encode_event(ev, gzip))
                                    .unwrap_or_else(|| {
                                        Event::default().event("resync").data("{}")
                                    });
                            }
                        }
                    }
                };
                Some((Ok(event), (rx, permit, pending, finished)))
            }
        },
    );

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

fn terminal_event(job: &crate::store::JobRecord) -> Option<Event> {
    match job.state.as_str() {
        "done" => Some(encode_event(
            JobEvent::Done {
                job_id: job.id.clone(),
                map_id: job.map_id.clone(),
                duration_ms: job
                    .finished_at
                    .unwrap_or(0)
                    .saturating_sub(job.started_at.unwrap_or(0))
                    .max(0) as u64,
            },
            false,
        )),
        "failed" => Some(encode_event(
            JobEvent::Failed {
                job_id: job.id.clone(),
                error: job.error.clone().unwrap_or_else(|| "生成失败".into()),
            },
            false,
        )),
        "cancelled" => Some(
            Event::default()
                .event("cancelled")
                .data(serde_json::json!({"job_id":job.id,"state":"cancelled"}).to_string()),
        ),
        _ => None,
    }
}

fn encode_event(ev: JobEvent, gzip: bool) -> Event {
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
        JobEvent::Stage { job_id, from, to } => Event::default()
            .event("stage")
            .data(serde_json::json!({ "job_id": job_id, "from": from, "to": to }).to_string()),
        JobEvent::Frame {
            job_id,
            tick,
            width,
            height,
            data,
            stats,
            compressed,
            grid,
            meta,
        } => {
            use base64_lite::encode;
            Event::default().event("frame").data(
                serde_json::json!({
                    "job_id": job_id,
                    "tick": tick,
                    "w": width,
                    "h": height,
                    "fmt": if gzip { "gzip-u16le+u8x3" } else { "raw-u16le+u8x3" },
                    "grid": grid,
                    "meta": meta,
                    "stats": {
                        "land_fraction": stats.land_fraction,
                        "max_elevation": stats.max_elevation,
                        "min_elevation": stats.min_elevation,
                        "max_temperature": stats.max_temperature,
                        "min_temperature": stats.min_temperature,
                    },
                    "data": encode(if gzip { &compressed } else { &data }),
                })
                .to_string(),
            )
        }
        JobEvent::Done {
            job_id,
            map_id,
            duration_ms,
        } => Event::default().event("done").data(
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
    /// Optional solver spacing. The server returns the actual rounded spacing.
    cell_size_m: Option<f64>,
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
    let _admission = st.admission.lock().await;
    let parent = st
        .store
        .get_map_node(&parent_id)?
        .ok_or_else(|| ApiError::not_found("父图不存在"))?;

    let world = st
        .store
        .get_world(&parent.world_id)?
        .ok_or_else(|| ApiError::not_found("世界不存在"))?;

    if body.name.as_ref().is_some_and(|n| n.chars().count() > 80) {
        return Err(ApiError::bad_request("地图名称过长"));
    }
    let expected = match parent.kind.as_str() {
        "world" => "continent",
        "continent" => "region",
        "region" => "site",
        _ => return Err(ApiError::bad_request("地点层不能继续细化")),
    };
    if body.kind != expected || parent.status != "done" {
        return Err(ApiError::bad_request("只能从已完成的父图生成下一层"));
    }
    let default_spacing = match body.kind.as_str() {
        "continent" => 1000.,
        "region" => 100.,
        "site" => 1.,
        _ => unreachable!(),
    };
    let cell_size_m = body.cell_size_m.unwrap_or(default_spacing);
    if !cell_size_m.is_finite() || cell_size_m < 1. || cell_size_m >= parent.cell_size_m {
        return Err(ApiError::bad_request(
            "子图格距必须至少 1 米，并小于父图格距",
        ));
    }
    if !body.width_m.is_finite()
        || body.width_m <= 0.
        || !body.center_lat.is_finite()
        || !body.center_lon.is_finite()
        || !(-90.0..=90.0).contains(&body.center_lat)
        || !(-180.0..=180.0).contains(&body.center_lon)
    {
        return Err(ApiError::bad_request("经纬度或覆盖宽度无效"));
    }
    let side_f = (body.width_m / cell_size_m).ceil();
    let limit = st.config.max_patch_side();
    if side_f < 4. || side_f > limit as f64 {
        return Err(ApiError::bad_request(format!(
            "当前精度最多 {limit}×{limit} 格，请将覆盖宽度设为 {}～{} 米",
            cell_size_m * 4.,
            cell_size_m * limit as f64
        )));
    }
    let side = side_f as u32;
    let width_m = side as f64 * cell_size_m;
    let params: tf_core::params::GenerationParams = serde_json::from_str(&world.params_json)?;
    let parent_desc = parent
        .grid
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("旧版父图缺少空间描述，请重新生成"))?;
    let grid = if let tf_grid::GridDescriptor::Plane {
        reference_lat,
        center_lat,
        ..
    } = parent_desc
    {
        tf_grid::GridDescriptor::Plane {
            center_lat: body.center_lat,
            center_lon: body.center_lon,
            width_m,
            size: side,
            rows: side,
            radius_m: params.planet.radius_m,
            version: tf_grid::descriptor::PLANE_GRID_VERSION,
            reference_lat: Some(reference_lat.unwrap_or(*center_lat)),
        }
    } else {
        tf_grid::GridDescriptor::patch(
            body.center_lat,
            body.center_lon,
            width_m,
            side,
            params.planet.radius_m,
        )
    };
    grid.validate().map_err(ApiError::bad_request)?;
    let parent_grid = parent_desc.build();
    if !matches!(parent_grid, tf_grid::MapGrid::Sphere(_)) {
        use tf_grid::GridOps;
        let child_grid = grid.build();
        for i in child_grid.boundary_mask().iter_ones() {
            if parent_grid.cell_at(child_grid.center_dir(i)).is_none() {
                return Err(ApiError::bad_request(
                    "子图超出父图覆盖范围，请缩小选区或移动中心",
                ));
            }
        }
    }
    let mut params = params;
    let descriptor = serde_json::to_string(&grid)?;
    params.seed = tf_core::rng::derive_child_seed(parent.seed, &[hash_str(&descriptor)]);
    let cells = side * side;
    let maps = st.store.list_map_nodes(&parent.world_id)?;
    if !st.config.desktop
        && (maps.len() >= 16
            || maps.iter().map(|m| m.cells as u64).sum::<u64>() + cells as u64 > 4_000_000)
    {
        return Err(ApiError::bad_request(
            "当前世界地图数量或总格数已达网页版上限",
        ));
    }
    let map_id = short_id("map");
    let node = MapNodeRecord {
        id: map_id.clone(),
        world_id: parent.world_id.clone(),
        kind: body.kind.clone(),
        name: body.name.unwrap_or_else(|| expected.to_string()),
        parent: Some(parent_id),
        cells,
        cell_size_m,
        extent_json: descriptor,
        status: "pending".into(),
        created_at: now_millis() as i64,
        completed_at: None,
        grid: Some(grid),
        seed: params.seed,
        generator_version: "tf-0.6".into(),
    };
    let job_id = st
        .jobs
        .submit(map_id.clone(), parent.world_id.clone(), node, params)
        .await?;

    Ok(Json(serde_json::json!({
        "job_id": job_id,
        "map_id": map_id,
        "kind": body.kind,
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

async fn get_metadata(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut metadata = st.store.get_metadata(&id)?.unwrap_or_default();
    if let Some(object) = metadata.as_object_mut() {
        object.remove("climate_atlas");
        object.remove("cryosphere_atlas");
        object.remove(crate::geography::KEY);
    }
    if let Some(cell) = query.get("cell") {
        let cell = cell
            .parse::<u64>()
            .map_err(|_| ApiError::bad_request("格子编号无效"))?;
        if let Some(minerals) = metadata.get_mut("minerals").and_then(|v| v.as_array_mut()) {
            minerals.retain(|m| m.get("cell").and_then(|v| v.as_u64()) == Some(cell));
        }
    }
    Ok(Json(metadata))
}

async fn get_monthly_climate(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use tf_grid::GridOps;
    let node = st
        .store
        .get_map_node(&id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    let cell = query
        .get("cell")
        .and_then(|x| x.parse::<usize>().ok())
        .ok_or_else(|| ApiError::bad_request("请指定格子编号"))?;
    if cell >= node.cells as usize {
        return Err(ApiError::bad_request("格子编号超出范围"));
    }
    let permit = st
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("气候资料读取繁忙"))?;
    let result=tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        let _permit=permit;
        let Some(atlas)=crate::climatology::for_map(&st.store,&node)? else { return Ok(serde_json::json!({"available":false,"reason":"旧工程没有月气候资料"})); };
        let descriptor=node.grid.as_ref().ok_or_else(||anyhow::anyhow!("缺少网格"))?;
        let grid=descriptor.build(); let climate_grid=atlas.grid.build();
        let t=st.store.get_field(&id,Channel::Temperature)?.ok_or_else(||anyhow::anyhow!("缺少年均温"))?;
        let p=st.store.get_field(&id,Channel::Precipitation)?.ok_or_else(||anyhow::anyhow!("缺少年降水"))?;
        let (temperature,rain,pet)=atlas.sample(&climate_grid,grid.center_dir(cell),t[cell],p[cell]);
        Ok(serde_json::json!({"available":true,"cell":cell,"model":atlas.model,"temperature_c":temperature,"precipitation_mm":rain,"potential_evaporation_mm":pet,"months":"January through December, equal-duration model months","pet_method":"Thornthwaite approximation","energy":atlas.energy,"water":atlas.water}))
    }).await??;
    Ok(Json(result))
}
async fn export_json(
    State(st): State<AppState>,
    Path(map_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let node = st
        .store
        .get_map_node(&map_id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    let permit = st.exports.clone().try_acquire_owned().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "正在处理另一项数据操作，请稍后再试",
        )
    })?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        crate::project::export(&st.store, &node.world_id)
    })
    .await??;
    Ok((
        [
            ("content-type", "application/json"),
            (
                "content-disposition",
                "attachment; filename=terraforge.tfworld",
            ),
        ],
        bytes,
    ))
}
async fn import_project(
    State(st): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Result<Json<serde_json::Value>, ApiError> {
    let owner = owner_from_headers(&headers);
    let _admission = st.admission.lock().await;
    ensure_live(&st, &owner)?;
    if !st.config.desktop
        && (st.store.world_count()? >= 8 || st.store.list_worlds(&owner)?.len() >= 4)
    {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "当前世界数量已达上限",
        ));
    }
    let permit = st.exports.clone().try_acquire_owned().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "生成或其他数据操作正在进行，请稍后重试",
        )
    })?;
    // Admit before buffering a potentially 64 MiB request, not after it.
    let limit = if st.config.desktop { 512 } else { 64 } * 1024 * 1024;
    let body = axum::body::to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "工程文件超过大小限制或读取失败",
            )
        })?;
    let world = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        crate::project::import(&st.store, &body, &owner, &st.config)
    })
    .await?
    .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(
        serde_json::json!({"id":world.id,"name":world.name,"seed":world.seed,"created_at":world.created_at}),
    ))
}
async fn export_png(
    State(st): State<AppState>,
    Path(map_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    let max = if st.config.desktop { 8192 } else { 2048 };
    let size = params
        .get("size")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(2048);
    let ecosystem_mode = match params.get("ecosystem").map(String::as_str) {
        Some("group") => Some(1),
        Some("biome") => Some(0),
        Some("variant") => Some(2),
        None | Some("legacy") => None,
        _ => return Err(ApiError::bad_request("生态导出模式无效")),
    };
    if !(256..=max).contains(&size) {
        return Err(ApiError::bad_request(format!(
            "当前模式导出宽度须为 256～{max} 像素"
        )));
    }
    let node = st
        .store
        .get_map_node(&map_id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    if node.status != "done" {
        return Err(ApiError::bad_request("请等待地图生成完成"));
    }
    let descriptor = node
        .grid
        .ok_or_else(|| ApiError::bad_request("旧版地图缺少网格描述，请重新生成"))?;
    let permit =
        st.exports.clone().try_acquire_owned().map_err(|_| {
            ApiError::new(StatusCode::TOO_MANY_REQUESTS, "请等待另一项数据操作完成")
        })?;
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let _permit = permit;
        let elevation = st
            .store
            .get_field(&map_id, Channel::Elevation)?
            .ok_or_else(|| anyhow::anyhow!("缺少高程"))?;
        let biome = st
            .store
            .get_field(&map_id, Channel::Biome)?
            .ok_or_else(|| anyhow::anyhow!("缺少生态"))?;
        let ice = crate::cryosphere::CHANNELS
            .iter()
            .map(|&ch| st.store.get_field(&map_id, ch))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let ice: Option<Vec<_>> = ice.into_iter().collect();
        let ecosystems = if ecosystem_mode.is_some() {
            crate::ecosystems::load(&st.store, &map_id, node.cells as usize)?
        } else {
            None
        };
        crate::render::render_map_png_with_ecosystems(
            &elevation,
            &biome,
            &descriptor,
            size,
            ice.as_deref(),
            ecosystems.as_ref(),
            ecosystem_mode.unwrap_or(1),
        )
    })
    .await??;
    Ok((
        [
            ("content-type", "image/png"),
            ("content-disposition", "attachment; filename=terraforge.png"),
        ],
        bytes,
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
    pub(crate) fn busy(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, msg)
    }
    pub(crate) fn bad_request(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, msg)
    }
    pub(crate) fn not_found(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, msg)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, format!("{:#}", e.into()))
    }
}
