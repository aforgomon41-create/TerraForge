//! 子图生成：降尺度继承。
//!
//! 与世界层不同，子图不是整球模拟，而是球面上的一块区域。
//! 关键是**必须从父图继承边界条件**，否则放大后地形完全对不上。

use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::broadcast;
use tracing::info;

use tf_grid::GridOps;

use crate::jobs::{persist_fields, FrameStats, JobEvent};
use crate::store::{Channel, MapNodeRecord, Store};

const FRAME_W: u32 = 384;
const FRAME_H: u32 = 192;

/// 跑一次子图生成。
pub fn run_child_generation(
    store: &Store,
    tx: &broadcast::Sender<JobEvent>,
    job_id: &str,
    node: &MapNodeRecord,
    params: &tf_core::params::GenerationParams,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    info!("开始生成子图 {}: {} 格 @ {:.0}m", node.id, node.cells, node.cell_size_m);

    // 解析父图和覆盖范围
    let extent: serde_json::Value = serde_json::from_str(&node.extent_json)?;
    let center_lon = extent["center_lon"].as_f64().unwrap_or(0.0);
    let center_lat = extent["center_lat"].as_f64().unwrap_or(0.0);
    let width_m = extent["width_m"].as_f64().unwrap_or(400_000.0);
    let parent_id = extent["parent"].as_str().unwrap_or("");

    let size = (node.cells as f64).sqrt().round() as u32;

    // 建局部网格
    let patch_spec = tf_grid::SphericalPatch::new(center_lat, center_lon, width_m, size);
    let patch = tf_grid::PatchGrid::new(&patch_spec);

    // 发进度
    let _ = tx.send(JobEvent::Progress {
        job_id: job_id.to_string(),
        stage: "降尺度继承".to_string(),
        progress: 0.1,
        eta_secs: 3,
    });

    // ---- 从父图采样 ----
    let parent_elevation = store
        .get_field(parent_id, Channel::Elevation)?
        .ok_or_else(|| anyhow::anyhow!("父图没有高程数据"))?;
    let parent_temperature = store
        .get_field(parent_id, Channel::Temperature)?
        .unwrap_or_else(|| vec![15.0; parent_elevation.len()]);
    let parent_precipitation = store
        .get_field(parent_id, Channel::Precipitation)?
        .unwrap_or_else(|| vec![700.0; parent_elevation.len()]);

    let parent_radius = params.planet.radius_m;
    let parent_grid = tf_grid::SphereHexGrid::build(
        params.quality.world_subdiv_freq(),
        parent_radius,
        (glam::DVec3::Z, 0.0),
    );

    let sample = tf_sim::downscale::sample_from_parent(
        &parent_grid,
        &patch,
        &parent_elevation,
        &parent_temperature,
        &parent_precipitation,
    );

    let _ = tx.send(JobEvent::Progress {
        job_id: job_id.to_string(),
        stage: "细化地形".to_string(),
        progress: 0.4,
        eta_secs: 2,
    });

    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }

    // ---- 在局部网格上跑细化 ----
    // 先做上采样得到基础地形
    let mut elevation = tf_sim::downscale::upsample_parent(&parent_grid, &patch, &parent_elevation);

    // 加细节：用噪声补充父图分辨率以下的纹理
    let noise = tf_core::noise::SimplexNoise::new(params.seed ^ 0x9E37_79B9);
    let cell_angle = patch.cell_size_m() / parent_radius;
    let base_freq = 1.0 / (cell_angle * 6.0);

    for i in 0..patch.cell_count() {
        let c = patch.center_dir(i).as_vec();
        let e = elevation[i];

        // 陆地才有明显细节
        let amp = if e > 0.0 {
            let t = (e / 3000.0).clamp(0.0, 1.0);
            30.0 + t * 280.0
        } else {
            12.0
        };

        let s = noise.fbm(
            c.x * base_freq,
            c.y * base_freq,
            c.z * base_freq,
            3,
            2.0,
            0.5,
        ) as f32;
        elevation[i] = e + s * amp;
    }

    // ---- 软约束拉向父图 ----
    let weights = tf_sim::downscale::constraint_weights(&patch);
    tf_sim::downscale::blend_with_parent(&patch, &mut elevation, &sample, &weights, true);

    // ---- 重算气候（基于子图分辨率的高程）----
    let _ = tx.send(JobEvent::Progress {
        job_id: job_id.to_string(),
        stage: "气候细化".to_string(),
        progress: 0.7,
        eta_secs: 1,
    });

    let mut temperature = sample.temperature.clone();
    let mut precipitation = sample.precipitation.clone();

    // 高程修正温度：每升高 1 km 降 6.5 度
    for i in 0..patch.cell_count() {
        let parent_e = sample.elevation[i];
        let delta = elevation[i] - parent_e;
        temperature[i] -= delta * 0.0065;
    }

    // 迎风坡增雨：用局部坡度近似
    for i in 0..patch.cell_count() {
        let nb = patch.neighbors(i);
        if nb.is_empty() {
            continue;
        }
        let mut upslope = 0.0f32;
        for &j in nb {
            let dh = elevation[j as usize] - elevation[i];
            if dh > 0.0 {
                upslope += dh;
            }
        }
        upslope /= nb.len() as f32;
        // 抬升越多雨越多
        precipitation[i] += upslope * 0.08;
    }

    // ---- 生态分类 ----
    let mut biome = vec![0u8; patch.cell_count()];
    let mut soil = vec![0u8; patch.cell_count()];
    for i in 0..patch.cell_count() {
        if elevation[i] <= 0.0 {
            biome[i] = 0;
            continue;
        }
        biome[i] = match tf_sim::ecology::classify_biome(temperature[i], precipitation[i], elevation[i]) {
            tf_sim::ecology::Biome::Ocean => 0,
            tf_sim::ecology::Biome::IceSheet => 1,
            tf_sim::ecology::Biome::Tundra => 2,
            tf_sim::ecology::Biome::BorealForest => 3,
            tf_sim::ecology::Biome::TemperateForest => 4,
            tf_sim::ecology::Biome::TemperateGrassland => 5,
            tf_sim::ecology::Biome::Mediterranean => 6,
            tf_sim::ecology::Biome::Desert => 7,
            tf_sim::ecology::Biome::Savanna => 8,
            tf_sim::ecology::Biome::TropicalRainforest => 9,
            tf_sim::ecology::Biome::TropicalSeasonalForest => 10,
            tf_sim::ecology::Biome::AlpineMeadow => 11,
            tf_sim::ecology::Biome::Wetland => 12,
        };
        soil[i] = 1;
    }

    // ---- 存盘 ----
    store.put_field(&node.id, Channel::Elevation, &elevation)?;
    store.put_field(&node.id, Channel::Temperature, &temperature)?;
    store.put_field(&node.id, Channel::Precipitation, &precipitation)?;
    let biome_f: Vec<f32> = biome.iter().map(|&b| b as f32).collect();
    let soil_f: Vec<f32> = soil.iter().map(|&s| s as f32).collect();
    store.put_field(&node.id, Channel::Biome, &biome_f)?;
    store.put_field(&node.id, Channel::Soil, &soil_f)?;

    // 更新节点状态
    if let Ok(Some(mut rec)) = store.get_map_node(&node.id) {
        rec.status = "done".to_string();
        rec.completed_at = Some(crate::jobs::now_millis() as i64);
        let _ = store.put_map_node(&rec);
    }

    let _ = tx.send(JobEvent::Progress {
        job_id: job_id.to_string(),
        stage: "完成".to_string(),
        progress: 1.0,
        eta_secs: 0,
    });

    info!("子图完成 {}: {} 格", node.id, patch.cell_count());
    Ok(())
}
