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
    let parent_meta = store.get_metadata(&parent.id)?.unwrap_or_default();
    let mut parent_fields = tf_sim::WorldFields {
        elevation: field(Channel::Elevation)?,
        temperature: field(Channel::Temperature)?,
        precipitation: field(Channel::Precipitation)?,
        potential_evaporation: field(Channel::PotentialEvaporation)?,
        flow_accumulation: field(Channel::FlowAccumulation)?,
        liquid_discharge: store
            .get_field(&parent.id, Channel::LiquidDischarge)?
            .unwrap_or_default(),
        lake_depth: field(Channel::LakeDepth)?,
        lake_runoff_loss_fraction: store
            .get_field(&parent.id, Channel::LakeRunoffLossFraction)?
            .unwrap_or_default(),
        climate: crate::climatology::for_map(store, &parent)?,
        ..Default::default()
    };
    let parent_forcing = crate::refinement::decode(&parent_meta, &parent_grid)?;
    if let Some(b) = &parent_forcing {
        b.apply(&mut parent_fields, 0).map_err(anyhow::Error::msg)?;
    }
    if let Some(atlas) = crate::cryosphere::for_map(store, &parent)? {
        let ice = store.get_field(&parent.id, Channel::LandIce)?;
        let shelf = store.get_field(&parent.id, Channel::IceShelf)?;
        let canonical = ice.as_deref().zip(shelf.as_deref());
        parent_fields.cryosphere =
            tf_sim::cryosphere::sample(&parent_grid, &parent_fields, atlas, 0, canonical);
        anyhow::ensure!(parent_fields.cryosphere.is_some(), "父图冰雪资料不完整");
    }
    let parent_receivers = field(Channel::FlowReceiver)?
        .into_iter()
        .map(|r| {
            if r == -1. {
                Ok(u32::MAX)
            } else if r.is_finite()
                && r >= 0.
                && r.fract() == 0.
                && r < parent_grid.cell_count() as f32
            {
                Ok(r as u32)
            } else {
                Err(anyhow::anyhow!("父图流向无效"))
            }
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    tf_sim::erosion::from_saved_receivers(
        &parent_grid,
        &mut parent_fields,
        parent_receivers.clone(),
    )
    .map_err(anyhow::Error::msg)?;
    let (work_descriptor, padding) = crate::refinement::buffered(descriptor, &parent_grid);
    let work = work_descriptor.build();
    let (work_forcing, guides) = tf_sim::refinement::inherit(
        &parent_grid,
        &work,
        &parent_fields,
        &parent_receivers,
        parent_forcing.as_ref(),
        parent_fields
            .cryosphere
            .as_ref()
            .map(|c| c.land_ice_m.as_slice()),
        &|| cancel.load(Ordering::Relaxed),
    )
    .map_err(anyhow::Error::msg)?;
    let pd = field(Channel::DistanceToSea)?;
    let sample = tf_sim::downscale::sample_from_grid(
        &parent_grid,
        &work,
        &parent_fields.elevation,
        &parent_fields.temperature,
        &parent_fields.precipitation,
    )
    .map_err(anyhow::Error::msg)?;
    let relief: Vec<f32> = (0..parent_grid.cell_count())
        .map(|i| {
            let h = parent_fields.elevation[i];
            let mut lo = h;
            let mut hi = h;
            for &j in parent_grid.neighbors(i) {
                lo = lo.min(parent_fields.elevation[j as usize]);
                hi = hi.max(parent_fields.elevation[j as usize]);
            }
            hi - lo
        })
        .collect();
    let relief = tf_sim::downscale::interpolate_from_grid(&parent_grid, &work, &relief)
        .map_err(anyhow::Error::msg)?;
    let background_ice = if padding > 0 {
        parent_fields
            .cryosphere
            .as_ref()
            .map(|c| tf_sim::downscale::interpolate_from_grid(&parent_grid, &work, &c.land_ice_m))
            .transpose()
            .map_err(anyhow::Error::msg)?
    } else {
        None
    };
    let initial_ice = parent_fields
        .cryosphere
        .as_ref()
        .map(|c| -> Result<_, String> {
            Ok((
                tf_sim::downscale::interpolate_from_grid(&parent_grid, &patch, &c.land_ice_m)?,
                tf_sim::downscale::interpolate_from_grid(&parent_grid, &patch, &c.shelf_ice_m)?,
            ))
        })
        .transpose()
        .map_err(anyhow::Error::msg)?;
    drop(parent_fields);
    let mut fields = tf_sim::WorldFields::with_capacity(work.cell_count());
    fields.elevation = sample.elevation.clone();
    fields.distance_to_sea = tf_sim::downscale::interpolate_from_grid(&parent_grid, &work, &pd)
        .map_err(anyhow::Error::msg)?;
    // Add only wavelengths the parent could not resolve, using a stable root coordinate frame.
    let mut root = parent.clone();
    while let Some(id) = &root.parent {
        root = store
            .get_map_node(id)?
            .ok_or_else(|| anyhow::anyhow!("缺少根地图"))?;
    }
    let root_descriptor = root
        .grid
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("缺少根几何"))?;
    let root_plane = if matches!(root_descriptor, tf_grid::GridDescriptor::Plane { .. }) {
        Some(root_descriptor.build())
    } else {
        None
    };
    let world_seed = store
        .get_world(&node.world_id)?
        .ok_or_else(|| anyhow::anyhow!("缺少工程"))?
        .seed;
    let noise = tf_core::noise::SimplexNoise::new(world_seed ^ 0x0ED0_6E17);
    let crust = field(Channel::CrustType)?;
    let child_crust = tf_sim::downscale::interpolate_from_grid(&parent_grid, &work, &crust)
        .map_err(anyhow::Error::msg)?;
    fields.crust_type = child_crust.clone();
    for i in 0..work.cell_count() {
        let dir = work.center_dir(i);
        let p = if let Some(tf_grid::MapGrid::Plane(g)) = &root_plane {
            let (x, y) = g.project(dir);
            glam::DVec3::new(x, y, 0.)
        } else {
            dir.as_vec() * root_descriptor.radius_m()
        };
        let detail = tf_sim::terrain::interior_detail(&noise, p, node.cell_size_m)
            - tf_sim::terrain::interior_detail(&noise, p, parent.cell_size_m);
        // Fine relief follows the observed local parent relief. The same global
        // rock noise amplitude would turn a low-gradient plain into lake pits.
        let budget = relief[i] as f64 * 0.35 * (node.cell_size_m / parent.cell_size_m).sqrt();
        fields.elevation[i] +=
            (detail * params.interior_relief * (0.12 + 0.88 * child_crust[i] as f64))
                .clamp(-budget, budget) as f32;
    }
    tf_sim::downscale::guide_rivers(&work, &mut fields.elevation, &guides, node.cell_size_m);
    tf_sim::downscale::constrain_edges(
        &work,
        &mut fields.elevation,
        &sample.elevation,
        node.cell_size_m * 4.,
    );
    work_forcing
        .apply(&mut fields, 0)
        .map_err(anyhow::Error::msg)?;
    let water_grid =
        tf_sim::refinement::BoundaryGrid::new(&work, &work_forcing).map_err(anyhow::Error::msg)?;
    fields.temperature = sample.temperature.clone();
    fields.precipitation = sample.precipitation.clone();
    fields.climate = crate::climatology::for_map(store, &parent)?;
    inherited_pet(&work, &mut fields, params);
    // The halo has the parent's prescribed ice background. Computing its snow
    // and melt budget does not require a second evolving ice-sheet solver.
    if padding > 0 && fields.climate.is_some() {
        let Some((liquid, pet)) = tf_sim::cryosphere::liquid_background(
            &work,
            &fields,
            background_ice.as_deref(),
            &|| cancel.load(Ordering::Relaxed),
        ) else {
            return Ok(());
        };
        fields.liquid_background_mm = liquid;
        fields.pet_background_mm = pet;
    }
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
    tf_sim::preview::emit(&mut live, &work, &fields, info);
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let erosion = tf_sim::erosion::ErosionParams {
        iterations: 12,
        k_river: tf_sim::erosion::ErosionParams::default().k_river * params.erosion_strength,
        ..Default::default()
    };
    let Some(erosion_result) = tf_sim::erosion::run_erosion_observed(
        &water_grid,
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
            tf_sim::preview::emit(&mut live, &work, fields, info);
        },
    ) else {
        return Ok(());
    };
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    progress("水文细化", 0.55);
    for i in 0..work.cell_count() {
        fields.temperature[i] -= (fields.elevation[i] - sample.elevation[i]) * 0.0065;
    }
    inherited_pet(&work, &mut fields, params);
    let forcing = if padding > 0 {
        // The halo's own rainfall becomes external only when it crosses into
        // the final crop. Grandparent inflow stays explicit through this step.
        fields.liquid_discharge =
            tf_sim::cryosphere::annual_discharge(&water_grid, &fields, &erosion_result);
        let (b, _) = tf_sim::refinement::inherit(
            &work,
            &patch,
            &fields,
            &erosion_result.flow_dir,
            Some(&work_forcing),
            background_ice.as_deref(),
            &|| cancel.load(Ordering::Relaxed),
        )
        .map_err(anyhow::Error::msg)?;
        let side = (patch.cell_count() as f64).sqrt() as usize;
        let crop = |v: &[f32]| crate::refinement::crop(v, side, padding as usize);
        fields.elevation = crop(&fields.elevation);
        fields.temperature = crop(&fields.temperature);
        fields.precipitation = crop(&fields.precipitation);
        fields.distance_to_sea = crop(&fields.distance_to_sea);
        fields.crust_type = crop(&fields.crust_type);
        fields.potential_evaporation = crop(&fields.potential_evaporation);
        fields.aridity = crop(&fields.aridity);
        fields.flow_accumulation = vec![0.; patch.cell_count()];
        fields.lake_depth = vec![0.; patch.cell_count()];
        b
    } else {
        work_forcing.clone()
    };
    fields.liquid_discharge.clear();
    fields.cryosphere = None;
    fields.liquid_background_mm.clear();
    fields.pet_background_mm.clear();
    forcing.apply(&mut fields, 0).map_err(anyhow::Error::msg)?;
    let water_grid =
        tf_sim::refinement::BoundaryGrid::new(&patch, &forcing).map_err(anyhow::Error::msg)?;
    if fields.climate.is_some() {
        progress("冰雪细化", 0.65);
        fields.cryosphere = if let Some((land, shelf)) = &initial_ice {
            tf_sim::cryosphere::solve_refinement(
                &patch,
                &fields,
                params.planet.surface_gravity(),
                (land, shelf),
                &|| cancel.load(Ordering::Relaxed),
            )
        } else {
            tf_sim::cryosphere::solve(&patch, &fields, params.planet.surface_gravity(), &|| {
                cancel.load(Ordering::Relaxed)
            })
        };
        if fields.cryosphere.is_none() {
            anyhow::ensure!(
                cancel.load(Ordering::Relaxed),
                "局部冰雪求解未完成，请调整参数后重试"
            );
            return Ok(());
        }
    }
    let water = tf_sim::erosion::refresh_hydrology(&water_grid, &mut fields);
    tf_sim::cryosphere::update_water_surfaces(&patch, &mut fields);
    fields.liquid_discharge = tf_sim::cryosphere::annual_discharge(&water_grid, &fields, &water);
    tf_sim::erosion::compute_distance_to_sea(&patch, &mut fields);
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
    let receivers: Vec<f32> = water
        .flow_dir
        .iter()
        .map(|&r| if r == u32::MAX { -1. } else { r as f32 })
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
        (Channel::FlowReceiver, &receivers),
        (
            Channel::LakeRunoffLossFraction,
            &fields.lake_runoff_loss_fraction,
        ),
        (Channel::LiquidDischarge, &fields.liquid_discharge),
    ];
    for (channel, data) in &inherited {
        channels.push((*channel, data.as_slice()));
    }
    let mut metadata = serde_json::json!({"generator_version":"tf-0.7","parent":parent.id,"minerals":minerals,"plates":parent_meta.get("plates"),"planar_velocities":parent_meta.get("planar_velocities"),"motion_domain":parent_meta.get("motion_domain"),"climate_model":parent_meta.get("climate_model")});
    metadata[crate::refinement::KEY] = serde_json::to_value(&forcing)?;
    metadata["erosion_budget_domain"] =
        "buffered solver domain; sediment ledger is not an output-only mass balance".into();
    metadata["refinement_model"] = serde_json::json!({"name":"continuous-parent-river-boundary-v1","halo_cells":padding,"solver_cells":work.cell_count(),"output_cells":patch.cell_count(),"river_guides":guides.len(),"sediment_inflow":"unavailable in parent; local eroded sediment only","local_climate":"parent seasonal background with elevation lapse","ice_initial_condition":if initial_ice.is_some(){"continuous parent ice + 500-year fine-grid relaxation"}else{"no parent ice; fresh spinup"}});
    if let Some(ice) = &fields.cryosphere {
        crate::cryosphere::attach(&mut metadata, ice)?;
        channels.extend(
            crate::cryosphere::CHANNELS
                .into_iter()
                .zip(crate::cryosphere::fields(ice)),
        );
    }
    anyhow::ensure!(
        ecology.ecosystems.validate(fields.elevation.len()),
        "生态分类结果无效"
    );
    let ecosystems = ecology.ecosystems.channels();
    crate::ecosystems::attach(&mut metadata, &ecology.ecosystems);
    channels.extend(
        crate::ecosystems::CHANNELS
            .into_iter()
            .zip(ecosystems.iter().map(Vec::as_slice)),
    );
    metadata["hydrology_model"] = serde_json::json!({"name":"depression-hierarchy-annual-balance-v1","lake_depth":"actual_supported_water","water":water.lake_budget,"sediment":erosion_result.sediment});
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

fn inherited_pet<G: GridOps>(
    grid: &G,
    fields: &mut tf_sim::WorldFields,
    params: &tf_core::params::GenerationParams,
) {
    if let Some(atlas) = &fields.climate {
        let climate_grid = atlas.grid.build();
        for i in 0..grid.cell_count() {
            let (_, _, pet) = atlas.sample(
                &climate_grid,
                grid.center_dir(i),
                fields.temperature[i],
                fields.precipitation[i],
            );
            fields.potential_evaporation[i] = pet.iter().sum();
            fields.aridity[i] = fields.precipitation[i] / fields.potential_evaporation[i].max(1.);
        }
    } else {
        // Annual-only legacy roots have no seasonal observations. Keep this
        // explicit constant-temperature PET fallback instead of saving false zeros.
        for i in 0..grid.cell_count() {
            let pet = tf_sim::scientific::monthly_pet(
                &[fields.temperature[i]; 12],
                grid.latitude_deg(i).to_radians(),
                params.planet.axial_tilt_deg.to_radians(),
            );
            fields.potential_evaporation[i] = pet.iter().sum();
            fields.aridity[i] = fields.precipitation[i] / fields.potential_evaporation[i].max(1.);
        }
    }
}
