//! Local physical refinement using the actual parent geometry.
use crate::{
    jobs::JobEvent,
    store::{Channel, MapNodeRecord, Store},
};
use std::sync::atomic::{AtomicBool, Ordering};
use tf_grid::GridOps;
use tokio::sync::broadcast;

pub fn run_child_generation(
    store: &Store,
    tx: &broadcast::Sender<JobEvent>,
    job_id: &str,
    node: &MapNodeRecord,
    params: &tf_core::params::GenerationParams,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let descriptor = node
        .grid
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("地图缺少网格描述"))?;
    let tf_grid::MapGrid::Patch(patch) = descriptor.build() else {
        anyhow::bail!("子图必须使用局部网格")
    };
    let parent = store
        .get_map_node(
            node.parent
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("缺少父图"))?,
        )?
        .ok_or_else(|| anyhow::anyhow!("父图已被删除"))?;
    anyhow::ensure!(parent.status == "done", "父图尚未完成");
    let parent_grid = parent
        .grid
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("旧父图需要重新生成"))?
        .build();
    let field = |ch| {
        store
            .get_field(&parent.id, ch)?
            .ok_or_else(|| anyhow::anyhow!("父图缺少 {} 数据", ch.name()))
    };
    let pe = field(Channel::Elevation)?;
    let pt = field(Channel::Temperature)?;
    let pp = field(Channel::Precipitation)?;
    let pd = field(Channel::DistanceToSea)?;
    let sample = tf_sim::downscale::sample_from_grid(&parent_grid, &patch, &pe, &pt, &pp)
        .map_err(anyhow::Error::msg)?;
    let mut fields = tf_sim::WorldFields::with_capacity(patch.cell_count());
    fields.elevation = sample.elevation.clone();
    fields.distance_to_sea = tf_sim::downscale::interpolate_from_grid(&parent_grid, &patch, &pd)
        .map_err(anyhow::Error::msg)?;
    let weights = tf_sim::downscale::constraint_weights(&patch);
    tf_sim::downscale::blend_with_parent(&patch, &mut fields.elevation, &sample, &weights, true);
    fields.temperature = sample.temperature.clone();
    fields.precipitation = sample.precipitation.clone();
    let progress = |stage: &str, p: f64| {
        let _ = store.update_progress(job_id, stage, p);
        let _ = tx.send(JobEvent::Progress {
            job_id: job_id.into(),
            stage: stage.into(),
            progress: p,
            eta_secs: 0,
        });
    };
    progress("父图继承", 0.15);
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let erosion = tf_sim::erosion::ErosionParams {
        iterations: 12,
        k_river: tf_sim::erosion::ErosionParams::default().k_river * params.erosion_strength,
        ..Default::default()
    };
    if tf_sim::erosion::run_erosion_cancellable(&patch, &mut fields, &erosion, &|| {
        cancel.load(Ordering::Relaxed)
    })
    .is_none()
    {
        return Ok(());
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    progress("水文细化", 0.55);
    tf_sim::downscale::blend_with_parent(&patch, &mut fields.elevation, &sample, &weights, true);
    for i in 0..patch.cell_count() {
        fields.temperature[i] -= (fields.elevation[i] - sample.elevation[i]) * 0.0065;
    }
    tf_sim::erosion::refresh_hydrology(&patch, &mut fields);
    tf_sim::erosion::compute_distance_to_sea(&patch, &mut fields);
    let ecology =
        tf_sim::ecology::run_ecology_with_diversity(&patch, &fields, params.biome_diversity);
    progress("气候与生态", 0.9);
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let biome: Vec<f32> = ecology.biome.iter().map(|&b| b as f32).collect();
    let soil: Vec<f32> = ecology.soil.iter().map(|&b| b as f32).collect();
    let mut inherited = Vec::new();
    for channel in [
        Channel::CrustThickness,
        Channel::CrustType,
        Channel::CrustAge,
        Channel::PlateId,
    ] {
        if let Some(values) = store.get_field(&parent.id, channel)? {
            let data = if channel == Channel::PlateId {
                (0..patch.cell_count())
                    .map(|i| {
                        parent_grid
                            .cell_at(patch.center_dir(i))
                            .map(|cell| values[cell])
                            .ok_or_else(|| anyhow::anyhow!("子图超出父图范围"))
                    })
                    .collect::<anyhow::Result<Vec<f32>>>()?
            } else {
                tf_sim::downscale::interpolate_from_grid(&parent_grid, &patch, &values)
                    .map_err(anyhow::Error::msg)?
            };
            inherited.push((channel, data));
        }
    }
    let parent_meta = store.get_metadata(&parent.id)?.unwrap_or_default();
    let minerals: Vec<serde_json::Value> = parent_meta
        .get("minerals")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let cell = item.get("cell")?.as_u64()? as usize;
            if cell >= parent_grid.cell_count() {
                return None;
            }
            let child = patch.cell_at(parent_grid.center_dir(cell))?;
            let mut item = item.clone();
            item["cell"] = serde_json::json!(child);
            Some(item)
        })
        .collect();
    let mut channels: Vec<(Channel, &[f32])> = vec![
        (Channel::Elevation, &fields.elevation),
        (Channel::Temperature, &fields.temperature),
        (Channel::Precipitation, &fields.precipitation),
        (Channel::FlowAccumulation, &fields.flow_accumulation),
        (Channel::LakeDepth, &fields.lake_depth),
        (Channel::DistanceToSea, &fields.distance_to_sea),
        (Channel::Biome, &biome),
        (Channel::Soil, &soil),
        (Channel::SoilDepth, &ecology.soil_depth_m),
    ];
    for (channel, data) in &inherited {
        channels.push((*channel, data.as_slice()));
    }
    store.publish_fields(&node.id,&channels,&serde_json::json!({"generator_version":"tf-0.2","parent":parent.id,"minerals":minerals,"plates":parent_meta.get("plates")}))?;
    progress("完成", 1.);
    Ok(())
}
