//! 生成执行体。
//!
//! 在阻塞线程里跑完整物理流水线，同时：
//! - 按阶段上报进度
//! - 每个阶段结束时把中间结果降采样成可视化帧推给前端
//! - 响应取消标志

use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::broadcast;
use tracing::info;

use crate::jobs::{persist_fields, FrameStats, JobEvent};
use crate::store::{MapNodeRecord, Store};

/// 帧的降采样分辨率。
/// 640x320 覆盖整个球面，单帧压缩后约 100-200 KB。
const FRAME_W: u32 = 384;
const FRAME_H: u32 = 192;

/// 跑一次完整生成。
pub fn run_generation(
    store: &Store,
    tx: &broadcast::Sender<JobEvent>,
    job_id: &str,
    map_id: &str,
    node: &MapNodeRecord,
    params: &tf_core::params::GenerationParams,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    info!("开始生成 {}: {} 格", map_id, node.cells);

    let freq = params.quality.world_subdiv_freq();
    let grid = tf_grid::SphereHexGrid::build(freq, params.planet.radius_m, (glam::DVec3::Z, 0.0));

    let mut last_progress = -1.0f64;
    let mut last_stage = String::new();

    let store_ref = store;
    let job_id_owned = job_id.to_string();

    let world = tf_sim::generate_with_frames(
        &grid,
        params,
        &mut |stage, progress| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let stage_name = stage.name_zh().to_string();

            if stage_name != last_stage {
                let _ = tx.send(JobEvent::Stage {
                    job_id: job_id.to_string(),
                    from: last_stage.clone(),
                    to: stage_name.clone(),
                });
                last_stage = stage_name.clone();
            }

            if progress - last_progress >= 0.02 {
                last_progress = progress;
                if let Ok(Some(mut rec)) = store_ref.get_job(&job_id_owned) {
                    rec.progress = progress;
                    rec.stage = stage_name.clone();
                    let _ = store_ref.put_job(&rec);
                }
                let _ = tx.send(JobEvent::Progress {
                    job_id: job_id.to_string(),
                    stage: stage_name,
                    progress,
                    eta_secs: 0,
                });
            }
        },
        &mut |stage, fields| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let tick = match stage {
                tf_sim::Stage::PlanetSetup => 0,
                tf_sim::Stage::Tectonics => 20,
                tf_sim::Stage::Landforms => 40,
                tf_sim::Stage::Erosion => 60,
                tf_sim::Stage::Climate => 80,
                tf_sim::Stage::Ecology => 95,
                tf_sim::Stage::Cover => 98,
                tf_sim::Stage::Finalize => 100,
            };
            let (data, stats) = build_field_frame(&grid, fields);
            let _ = tx.send(JobEvent::Frame {
                job_id: job_id.to_string(),
                tick,
                width: FRAME_W,
                height: FRAME_H,
                data,
                stats,
            });
        },
    );

    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }

    persist_fields(store, map_id, &world)?;

    if let Ok(Some(mut rec)) = store.get_map_node(map_id) {
        rec.status = "done".to_string();
        rec.completed_at = Some(crate::jobs::now_millis() as i64);
        let _ = store.put_map_node(&rec);
    }

    info!("生成完成 {}: {} 个矿点", map_id, world.minerals.len());
    Ok(())
}

/// 把物理场降采样成前端能实时显示的多通道帧。
///
/// 输出格式：每像素 5 字节
///   [0..1] 高程（u16 小端，归一化到 0-65535）
///   [2]    温度（u8，偏移 128，即 value - 128 是摄氏度）
///   [3]    降水（u8，对数刻度）
///   [4]    河流流量（u8，对数刻度）
pub fn build_field_frame(
    grid: &tf_grid::SphereHexGrid,
    fields: &tf_sim::WorldFields,
) -> (Vec<u8>, FrameStats) {
    use tf_grid::GridOps;

    let n = grid.cell_count();
    let mut max_e = f32::NEG_INFINITY;
    let mut min_e = f32::INFINITY;
    let mut max_t = f32::NEG_INFINITY;
    let mut min_t = f32::INFINITY;
    let mut land = 0usize;

    for i in 0..n {
        let e = fields.elevation[i];
        if e.is_finite() {
            max_e = max_e.max(e);
            min_e = min_e.min(e);
            if e > 0.0 {
                land += 1;
            }
        }
        let t = fields.temperature[i];
        if t.is_finite() && t.abs() > 0.01 {
            max_t = max_t.max(t);
            min_t = min_t.min(t);
        }
    }

    let stats = FrameStats {
        land_fraction: land as f32 / n.max(1) as f32,
        max_elevation: if max_e.is_finite() { max_e } else { 0.0 },
        min_elevation: if min_e.is_finite() { min_e } else { 0.0 },
        max_temperature: if max_t.is_finite() { max_t } else { 0.0 },
        min_temperature: if min_t.is_finite() { min_t } else { 0.0 },
    };

    let mut pixels = vec![0u8; (FRAME_W * FRAME_H * 5) as usize];
    let e_span = if max_e.is_finite() && min_e.is_finite() {
        (max_e - min_e).max(1.0)
    } else {
        1.0
    };
    let e_base = if min_e.is_finite() { min_e } else { 0.0 };

    for py in 0..FRAME_H {
        let lat = 90.0 - (py as f64 + 0.5) / FRAME_H as f64 * 180.0;
        for px in 0..FRAME_W {
            let lon = (px as f64 + 0.5) / FRAME_W as f64 * 360.0 - 180.0;
            let dir = tf_core::geo::UnitVec3::from_lat_lon_deg(lat, lon);
            let cell = grid.cell_at(dir);

            let idx = ((py * FRAME_W + px) * 5) as usize;
            let e = fields.elevation[cell];

            let e_norm = (((e - e_base) / e_span) * 65535.0).clamp(0.0, 65535.0) as u16;
            let [b0, b1] = e_norm.to_le_bytes();
            pixels[idx] = b0;
            pixels[idx + 1] = b1;

            // 温度归一化到 0-1：-40度映射到 0，40度映射到 1。
            // 这样前端着色器可以直接当「冷暖程度」用。
            let t_norm = ((fields.temperature[cell] + 40.0) / 80.0).clamp(0.0, 1.0);
            pixels[idx + 2] = (t_norm * 255.0) as u8;

            // 降水归一化到 0-1：用对数刻度，0mm -> 0，4000mm -> 1
            let p = fields.precipitation[cell].max(0.0);
            let p_norm = ((p + 1.0).ln() / 8.3).clamp(0.0, 1.0);
            pixels[idx + 3] = (p_norm * 255.0) as u8;

            let fa = fields.flow_accumulation[cell].max(1.0);
            pixels[idx + 4] = ((fa.ln() / 25.0) * 255.0).clamp(0.0, 255.0) as u8;
        }
    }

    // 不压缩直接返回。
    //
    // 浏览器对 zstd 的支持还不通用（只有 Chrome 123+ 的 DecompressionStream），
    // 而 384x192x5 = 368 KB 的原始数据在 base64 后约 490 KB，
    // 本地和局域网完全可接受。压缩带来的兼容问题不值得。
    (pixels, stats)
}
