//! Local physical refinement using the actual parent geometry.
use crate::{
    jobs::{FrameCache, JobEvent},
    store::{Channel, MapNodeRecord, Store},
};
use std::sync::atomic::{AtomicBool, Ordering};
use tf_grid::GridOps;
use tokio::sync::broadcast;

pub fn run_child_generation(
    store: &Store,
    tx: &broadcast::Sender<JobEvent>,
    frames: &FrameCache,
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
    let patch = descriptor.build();
    let mut live = crate::generate::LiveFrames::new(tx, frames, job_id, &patch, descriptor, cancel);
    anyhow::ensure!(patch.extent_m().is_some(), "子图必须使用有限平面域");
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
    // Add only wavelengths the parent could not resolve, using a stable root coordinate frame.
    let mut root = parent.clone();
    while let Some(id) = &root.parent {
        root = store
            .get_map_node(id)?
            .ok_or_else(|| anyhow::anyhow!("缺少根地图"))?;
    }
    let root_grid = root
        .grid
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("缺少根几何"))?
        .build();
    let world_seed = store
        .get_world(&node.world_id)?
        .ok_or_else(|| anyhow::anyhow!("缺少工程"))?
        .seed;
    let noise = tf_core::noise::SimplexNoise::new(world_seed ^ 0x0ED0_6E17);
    let crust = field(Channel::CrustType)?;
    let child_crust = tf_sim::downscale::interpolate_from_grid(&parent_grid, &patch, &crust)
        .map_err(anyhow::Error::msg)?;
    for i in 0..patch.cell_count() {
        let dir = patch.center_dir(i);
        let p = if let tf_grid::MapGrid::Plane(g) = &root_grid {
            let (x, y) = g.project(dir);
            glam::DVec3::new(x, y, 0.)
        } else {
            dir.as_vec() * root_grid.radius_m()
        };
        let detail = tf_sim::terrain::interior_detail(&noise, p, node.cell_size_m)
            - tf_sim::terrain::interior_detail(&noise, p, parent.cell_size_m);
        fields.elevation[i] +=
            (detail * params.interior_relief * (0.12 + 0.88 * child_crust[i] as f64)) as f32;
    }
    let weights = tf_sim::downscale::constraint_weights(&patch);
    tf_sim::downscale::blend_with_parent(&patch, &mut fields.elevation, &sample, &weights, true);
    fields.temperature = sample.temperature.clone();
    fields.precipitation = sample.precipitation.clone();
    fields.climate = crate::climatology::for_map(store, &parent)?;
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
    let mut info = tf_sim::preview::FrameInfo::stage(tf_sim::Stage::Landforms, 0.15);
    info.detail = "父图继承与局部地貌";
    info.channels = tf_sim::preview::ALL;
    tf_sim::preview::emit(&mut live, &patch, &fields, info);
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let erosion = tf_sim::erosion::ErosionParams {
        iterations: 12,
        k_river: tf_sim::erosion::ErosionParams::default().k_river * params.erosion_strength,
        ..Default::default()
    };
    if tf_sim::erosion::run_erosion_observed(
        &patch,
        &mut fields,
        &erosion,
        &|| cancel.load(Ordering::Relaxed),
        &mut |step, fields| {
            let p = 0.15 + 0.4 * step as f64 / erosion.iterations as f64;
            progress("水文细化", p);
            let mut info = tf_sim::preview::FrameInfo::stage(tf_sim::Stage::Erosion, p);
            info.stage_complete = false;
            info.step = step;
            info.detail = "子区域河流与坡面细化";
            tf_sim::preview::emit(&mut live, &patch, fields, info);
        },
    )
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
    let water = tf_sim::erosion::refresh_hydrology(&patch, &mut fields);
    tf_sim::erosion::compute_distance_to_sea(&patch, &mut fields);
    if let Some(atlas) = &fields.climate {
        let climate_grid = atlas.grid.build();
        for i in 0..patch.cell_count() {
            let (_, _, pet) = atlas.sample(
                &climate_grid,
                patch.center_dir(i),
                fields.temperature[i],
                fields.precipitation[i],
            );
            fields.potential_evaporation[i] = pet.iter().sum();
            fields.aridity[i] = fields.precipitation[i] / fields.potential_evaporation[i].max(1.);
        }
    } else {
        // Annual-only legacy roots have no seasonal observations. Keep this
        // explicit constant-temperature PET fallback instead of saving false zeros.
        for i in 0..patch.cell_count() {
            let pet = tf_sim::scientific::monthly_pet(
                &[fields.temperature[i]; 12],
                patch.latitude_deg(i).to_radians(),
                params.planet.axial_tilt_deg.to_radians(),
            );
            fields.potential_evaporation[i] = pet.iter().sum();
            fields.aridity[i] = fields.precipitation[i] / fields.potential_evaporation[i].max(1.);
        }
    }
    let ecology =
        tf_sim::ecology::run_ecology_with_diversity(&patch, &fields, params.biome_diversity);
    progress("气候与生态", 0.9);
    let mut info = tf_sim::preview::FrameInfo::stage(tf_sim::Stage::Ecology, 0.9);
    info.detail = "父图气候与局部生态";
    tf_sim::preview::emit(&mut live, &patch, &fields, info);
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
        Channel::CrustalStrain,
        Channel::OrogenAge,
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
        (Channel::PotentialEvaporation, &fields.potential_evaporation),
        (Channel::Aridity, &fields.aridity),
    ];
    for (channel, data) in &inherited {
        channels.push((*channel, data.as_slice()));
    }
    let mut metadata = serde_json::json!({"generator_version":"tf-0.4","parent":parent.id,"minerals":minerals,"plates":parent_meta.get("plates"),"planar_velocities":parent_meta.get("planar_velocities"),"motion_domain":parent_meta.get("motion_domain"),"climate_model":parent_meta.get("climate_model")});
    metadata["planar_spin_rad_myr"] = parent_meta
        .get("planar_spin_rad_myr")
        .cloned()
        .unwrap_or_default();
    metadata["plate_centers_m"] = parent_meta
        .get("plate_centers_m")
        .cloned()
        .unwrap_or_default();
    crate::geography::attach(&mut metadata, &patch, &fields, &water, &|| {
        cancel.load(Ordering::Relaxed)
    });
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    store.publish_fields(&node.id, &channels, &metadata)?;
    progress("完成", 1.);
    Ok(())
}
