//! 生成执行体。
//!
//! 在阻塞线程里跑完整物理流水线，同时：
//! - 按阶段上报进度
//! - 在求解过程中按观察频率把实际状态降采样推给前端
//! - 响应取消标志

use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tf_grid::GridOps;
use tokio::sync::broadcast;
use tracing::info;

use crate::jobs::{persist_fields, FrameCache, FrameMeta, FrameStats, JobEvent};
use crate::store::{MapNodeRecord, Store};

/// 帧的降采样分辨率。
/// 384x192 覆盖整个生成域；gzip 仅用于传输，不改变场值编码。
const FRAME_W: u32 = 384;
const FRAME_H: u32 = 192;

/// 跑一次完整生成。
pub fn run_generation(
    store: &Store,
    tx: &broadcast::Sender<JobEvent>,
    frames: &FrameCache,
    job_id: &str,
    map_id: &str,
    node: &MapNodeRecord,
    params: &tf_core::params::GenerationParams,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    info!("开始生成 {}: {} 格", map_id, node.cells);

    let grid = node
        .grid
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("地图缺少生成域"))?
        .build();

    let mut last_progress = -1.0f64;
    let mut last_stage = String::new();

    let store_ref = store;
    let job_id_owned = job_id.to_string();

    let mut live = LiveFrames::new(
        tx,
        frames,
        job_id,
        &grid,
        node.grid.as_ref().unwrap(),
        cancel,
    );
    let world = tf_sim::generate_observed(
        &grid,
        params,
        &mut |stage, progress| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let stage_name = stage.name_zh().to_string();

            let changed = stage_name != last_stage;
            if changed {
                let _ = tx.send(JobEvent::Stage {
                    job_id: job_id.to_string(),
                    from: last_stage.clone(),
                    to: stage_name.clone(),
                });
                last_stage = stage_name.clone();
            }

            if changed || progress - last_progress >= 0.01 {
                last_progress = progress;
                let _ = store_ref.update_progress(&job_id_owned, &stage_name, progress);
                let _ = tx.send(JobEvent::Progress {
                    job_id: job_id.to_string(),
                    stage: stage_name,
                    progress,
                    eta_secs: 0,
                });
            }
        },
        &mut live,
        &|| cancel.load(Ordering::Relaxed),
    );
    let Some(world) = world else {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        anyhow::bail!("模拟产生无效数值");
    };

    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }

    persist_fields(store, map_id, &world, &grid, &|| {
        cancel.load(Ordering::Relaxed)
    })?;

    info!("生成完成 {}: {} 个矿点", map_id, world.minerals.len());
    Ok(())
}

pub(crate) struct LiveFrames<'a> {
    tx: &'a broadcast::Sender<JobEvent>,
    frames: &'a FrameCache,
    job_id: &'a str,
    grid: &'a tf_grid::MapGrid,
    descriptor: &'a tf_grid::GridDescriptor,
    cancel: &'a AtomicBool,
    last_capture: Option<Instant>,
    sequence: u32,
    available_channels: u8,
    last_pixels: Option<Arc<[u8]>>,
    last_stats: FrameStats,
}
impl<'a> LiveFrames<'a> {
    pub(crate) fn new(
        tx: &'a broadcast::Sender<JobEvent>,
        frames: &'a FrameCache,
        job_id: &'a str,
        grid: &'a tf_grid::MapGrid,
        descriptor: &'a tf_grid::GridDescriptor,
        cancel: &'a AtomicBool,
    ) -> Self {
        Self {
            tx,
            frames,
            job_id,
            grid,
            descriptor,
            cancel,
            last_capture: None,
            sequence: 0,
            available_channels: 0,
            last_pixels: None,
            last_stats: FrameStats::default(),
        }
    }
}
impl tf_sim::preview::FrameObserver for LiveFrames<'_> {
    fn preview_cell_budget(&self) -> usize {
        6000
    }
    fn wants_frame(&mut self, info: tf_sim::preview::FrameInfo) -> bool {
        if self.cancel.load(Ordering::Relaxed) {
            return false;
        }
        // This legacy stage boundary contains raw reference columns; live
        // tectonic snapshots already show the corresponding actual relief.
        if info.stage == tf_sim::Stage::Tectonics && info.stage_complete {
            return false;
        }
        let interval = Duration::from_millis(if self.tx.receiver_count() == 0 {
            5000
        } else {
            500
        });
        info.stage_complete
            || self
                .last_capture
                .is_none_or(|last| last.elapsed() >= interval)
    }
    fn frame(
        &mut self,
        source: &dyn GridOps,
        fields: &tf_sim::WorldFields,
        info: tf_sim::preview::FrameInfo,
    ) {
        if self.cancel.load(Ordering::Relaxed) {
            return;
        }
        let (mut data, mut stats) =
            build_field_frame_in_view(source, self.grid, fields, info.channels);
        if let Some(previous) = &self.last_pixels {
            for (channel, flag) in [(0, 1), (1, 1), (2, 2), (3, 4), (4, 8)] {
                if info.channels & flag == 0 {
                    for i in 0..(FRAME_W * FRAME_H) as usize {
                        data[i * 5 + channel] = previous[i * 5 + channel];
                    }
                }
            }
            if info.channels & tf_sim::preview::HEIGHT == 0 {
                stats.land_fraction = self.last_stats.land_fraction;
                stats.min_elevation = self.last_stats.min_elevation;
                stats.max_elevation = self.last_stats.max_elevation;
            }
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        if encoder.write_all(&data).is_err() {
            return;
        }
        let Ok(compressed) = encoder.finish() else {
            return;
        };
        if self.cancel.load(Ordering::Relaxed) {
            return;
        }
        self.sequence += 1;
        self.available_channels |= info.channels;
        let data: Arc<[u8]> = data.into();
        let event = JobEvent::Frame {
            job_id: self.job_id.to_string(),
            tick: self.sequence,
            width: FRAME_W,
            height: FRAME_H,
            data: data.clone(),
            compressed: compressed.into(),
            stats: stats.clone(),
            grid: self.descriptor.clone(),
            meta: FrameMeta {
                stage: info.stage.name_zh().into(),
                detail: info.detail.into(),
                progress: info.progress,
                step: info.step,
                elapsed_myr: info.elapsed_myr,
                year: info.year,
                month: info.month,
                channels: info.channels,
                available_channels: self.available_channels,
            },
        };
        self.last_pixels = Some(data);
        self.last_stats = stats;
        self.last_capture = Some(Instant::now());
        self.frames
            .lock()
            .insert(self.job_id.to_string(), event.clone());
        let _ = self.tx.send(event);
    }
}

/// 把物理场降采样成前端能实时显示的多通道帧。
///
/// 输出格式：每像素 5 字节
///   [0..1] 高程（u16 小端，归一化到 0-65535）
///   [2]    温度（u8，线性映射 -40～40 摄氏度）
///   [3]    降水（u8，对数刻度）
///   [4]    河流流量（u8，对数刻度）
pub fn build_field_frame(
    grid: &tf_grid::MapGrid,
    fields: &tf_sim::WorldFields,
) -> (Vec<u8>, FrameStats) {
    build_field_frame_in_view(grid, grid, fields, tf_sim::preview::ALL)
}
fn build_field_frame_in_view(
    grid: &dyn GridOps,
    view: &tf_grid::MapGrid,
    fields: &tf_sim::WorldFields,
    channels: u8,
) -> (Vec<u8>, FrameStats) {
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
            let cell = if let Some((w, h)) = view.extent_m() {
                let x = ((px as f64 + 0.5) / FRAME_W as f64 - 0.5) * w;
                let y = (0.5 - (py as f64 + 0.5) / FRAME_H as f64) * h;
                if channels & tf_sim::preview::HEIGHT != 0 && grid.extent_m().is_some() {
                    local_sample(grid, x, y)
                } else {
                    let cell = local_sample(view, x, y);
                    grid.locate(view.center_dir(cell)).unwrap_or(0)
                }
            } else {
                grid.locate(dir).unwrap_or(0)
            };

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

    // Keep the lossless channel bytes. The sender compresses once per frame;
    // older clients can still negotiate the original raw representation.
    (pixels, stats)
}

fn local_sample(grid: &dyn GridOps, x: f64, y: f64) -> usize {
    grid.locate_local(x, y)
        .or_else(|| {
            let (w, h) = grid.extent_m()?;
            let d = grid.distance_m(0, 1);
            grid.locate_local(
                x.clamp(-w / 2. + d, w / 2. - d),
                y.clamp(-h / 2. + d, h / 2. - d),
            )
        })
        .expect("a valid map has interior samples")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tf_sim::preview::{FrameInfo, FrameObserver, ALL, PRECIPITATION, TEMPERATURE};

    #[test]
    fn climate_frames_preserve_terrain_and_compressed_replay_is_lossless() {
        let descriptor = tf_grid::GridDescriptor::plane(25., 179.9, 1_600_000., 16, 10, 6371000.);
        let view = descriptor.build();
        let mut initial = tf_sim::WorldFields::with_capacity(view.cell_count());
        for i in 0..view.cell_count() {
            initial.elevation[i] = (view.field_position(i).x * 0.003) as f32;
        }
        initial.temperature.fill(10.);
        initial.precipitation.fill(1000.);
        initial.flow_accumulation.fill(1e10);
        let (tx, mut rx) = broadcast::channel(16);
        let cache = FrameCache::default();
        let cancel = AtomicBool::new(false);
        let mut observer = LiveFrames {
            tx: &tx,
            frames: &cache,
            job_id: "test",
            grid: &view,
            descriptor: &descriptor,
            cancel: &cancel,
            last_capture: None,
            sequence: 0,
            available_channels: 0,
            last_pixels: None,
            last_stats: FrameStats::default(),
        };
        let mut info = FrameInfo::stage(tf_sim::Stage::Landforms, 0.27);
        info.channels = ALL;
        observer.frame(&view, &initial, info);
        let JobEvent::Frame {
            data: first,
            stats: first_stats,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!()
        };
        let atmosphere =
            tf_grid::GridDescriptor::plane(25., 179.9, 1_600_000., 8, 5, 6371000.).build();
        let mut weather = tf_sim::WorldFields::with_capacity(atmosphere.cell_count());
        for i in 0..atmosphere.cell_count() {
            weather.temperature[i] = (20. + atmosphere.field_position(i).x / 100_000.) as f32;
        }
        weather.precipitation.fill(50.);
        info = FrameInfo::stage(tf_sim::Stage::Climate, 0.8);
        info.channels = TEMPERATURE | PRECIPITATION;
        observer.frame(&atmosphere, &weather, info);
        let JobEvent::Frame {
            data,
            compressed,
            stats,
            tick,
            meta,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!()
        };
        assert_eq!(tick, 2);
        assert_eq!(meta.available_channels, ALL);
        for i in 0..(FRAME_W * FRAME_H) as usize {
            for channel in [0, 1, 4] {
                assert_eq!(data[i * 5 + channel], first[i * 5 + channel]);
            }
        }
        assert_eq!(stats.min_elevation, first_stats.min_elevation);
        assert_eq!(stats.max_elevation, first_stats.max_elevation);
        assert!(data[2] < data[(FRAME_W as usize - 1) * 5 + 2]);
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(compressed.as_ref())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, data.as_ref());
        assert!(compressed.len() < data.len() / 3);
        let JobEvent::Frame {
            data: cached,
            tick: cached_tick,
            ..
        } = cache.lock().get("test").unwrap().clone()
        else {
            panic!()
        };
        assert_eq!(cached_tick, tick);
        assert!(Arc::ptr_eq(&cached, &data));
    }
}
