//! Preview and apply the same deterministic evolution plan for every caller.
use crate::{
    api::{ApiError, AppState},
    jobs::{now_millis, JobEvent},
    store::{Channel, MapNodeRecord, MapRevision, Store},
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use tf_core::params::GenerationParams;
use tf_grid::{GridDescriptor, GridOps, MapGrid};
use tf_sim::{
    directed::{Evolution, Plan},
    WorldFields,
};
use tokio::sync::broadcast;

const MAX_CELLS: u32 = 512 * 512;
const KEY: &str = "directed_evolution";

pub async fn preview(
    State(st): State<AppState>,
    Path(source_id): Path<String>,
    Json(plan): Json<Plan>,
) -> Result<Json<Value>, ApiError> {
    let _admission = st.admission.lock().await;
    let source = st
        .store
        .get_map_node(&source_id)?
        .ok_or_else(|| ApiError::not_found("原图不存在"))?;
    if source.status != "done" || source.is_draft() {
        return Err(ApiError::bad_request("请使用已保存的完成地图"));
    }
    let (width, height) = match source.grid.as_ref() {
        Some(GridDescriptor::Plane {
            width_m,
            size,
            rows,
            ..
        }) => (*width_m, *width_m * *rows as f64 / *size as f64),
        Some(GridDescriptor::Patch { width_m, .. }) => (*width_m, *width_m),
        _ => {
            return Err(ApiError::bad_request(
                "请选择有限范围的地图，可先从球形世界细化一张子图",
            ))
        }
    };
    if source.cells > MAX_CELLS {
        return Err(ApiError::bad_request(
            "定向演化目前最多支持 262,144 个采样点，请选择较小范围的子图",
        ));
    }
    plan.validate(source.cells as usize, source.cell_size_m, (width, height))
        .map_err(ApiError::bad_request)?;
    let maps = st.store.list_map_nodes(&source.world_id)?;
    if maps.iter().any(|m| {
        m.is_draft()
            && m.revision
                .as_ref()
                .is_some_and(|r| r.source_id == source_id)
    }) {
        return Err(ApiError::bad_request("原图已有试算，请先套用或放弃它"));
    }
    let map_limit = if st.config.desktop { 256 } else { 16 };
    if maps.len() >= map_limit
        || (!st.config.desktop
            && maps.iter().map(|m| m.cells as u64).sum::<u64>() + source.cells as u64 > 4_000_000)
    {
        return Err(ApiError::bad_request("地图数量或总采样数已达上限"));
    }
    let world = st
        .store
        .get_world(&source.world_id)?
        .ok_or_else(|| ApiError::not_found("工程不存在"))?;
    let params = serde_json::from_str(&world.params_json)?;
    let mut node = source;
    node.id = format!("map-{}", uuid::Uuid::new_v4().simple());
    node.name = format!(
        "{} · 定向演化",
        node.name.chars().take(73).collect::<String>()
    );
    node.status = "pending".into();
    node.created_at = now_millis() as i64;
    node.completed_at = None;
    node.revision = Some(MapRevision {
        source_id,
        draft: true,
        applied_at: None,
    });
    let map_id = node.id.clone();
    let job_id = st
        .jobs
        .submit_with_evolution(
            map_id.clone(),
            node.world_id.clone(),
            node,
            params,
            Some(plan),
        )
        .await?;
    Ok(Json(json!({"map_id": map_id, "job_id": job_id})))
}

#[derive(Serialize)]
pub struct Preview {
    map: MapNodeRecord,
    job: Option<crate::store::JobRecord>,
    evolution: Option<Value>,
}

pub async fn get_preview(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Option<Preview>>, ApiError> {
    let node = st
        .store
        .get_map_node(&id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    let draft = if node.is_draft() {
        Some(node)
    } else {
        st.store
            .list_map_nodes(&node.world_id)?
            .into_iter()
            .find(|m| {
                m.revision
                    .as_ref()
                    .is_some_and(|r| r.draft && r.source_id == id)
            })
    };
    let Some(map) = draft else {
        return Ok(Json(None));
    };
    let job = st.store.job_for_map(&map.id)?;
    let evolution = if map.status == "done" {
        let permit = st.reads.clone().acquire_owned().await?;
        let store = st.store.clone();
        let map_id = map.id.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            store
                .get_metadata(&map_id)
                .map(|metadata| metadata.and_then(|mut m| m.as_object_mut()?.remove(KEY)))
        })
        .await??
    } else {
        None
    };
    Ok(Json(Some(Preview {
        map,
        job,
        evolution,
    })))
}

pub async fn apply(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MapNodeRecord>, ApiError> {
    st.store
        .apply_evolution(&id)?
        .map(Json)
        .ok_or_else(|| ApiError::bad_request("试算尚未完成，或未通过目标与保护区检查"))
}

pub async fn discard(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !st.store.discard_evolution(&id)? {
        return Err(ApiError::bad_request(
            "请先取消正在运行的试算，已套用版本不能在此放弃",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub fn run(
    store: &Store,
    tx: &broadcast::Sender<JobEvent>,
    job_id: &str,
    node: &MapNodeRecord,
    params: &GenerationParams,
    plan: &Plan,
    flag: &AtomicBool,
) -> anyhow::Result<()> {
    let cancel = || flag.load(Ordering::Relaxed);
    let progress = |stage: &str, p: f64| {
        let _ = store.update_progress(job_id, stage, p);
        let _ = tx.send(JobEvent::Progress {
            job_id: job_id.into(),
            stage: stage.into(),
            progress: p,
            eta_secs: 0,
        });
    };
    let revision = node
        .revision
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("试算缺少版本信息"))?;
    let source = store
        .get_map_node(&revision.source_id)?
        .filter(|m| m.world_id == node.world_id && m.status == "done" && !m.is_draft())
        .ok_or_else(|| anyhow::anyhow!("原图不存在或尚未完成"))?;
    let descriptor = node
        .grid
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("地图缺少网格"))?;
    let grid = descriptor.build();
    let mut metadata = store
        .get_metadata(&source.id)?
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let forcing = crate::refinement::decode(&metadata, &grid)?;
    anyhow::ensure!(
        source.parent.is_none() || forcing.is_some(),
        "旧子图缺少边界来水资料，请重新细化"
    );
    let load = |ch: Channel| -> anyhow::Result<Vec<f32>> {
        let values = store
            .get_field(&source.id, ch)?
            .ok_or_else(|| anyhow::anyhow!("原图缺少 {}", ch.name()))?;
        anyhow::ensure!(values.len() == grid.cell_count(), "原图字段尺寸错误");
        Ok(values)
    };
    let old_elevation = load(Channel::Elevation)?;
    let old_lakes = load(Channel::LakeDepth)?;
    let mut fields = WorldFields {
        elevation: old_elevation.clone(),
        lake_depth: old_lakes.clone(),
        temperature: load(Channel::Temperature)?,
        precipitation: load(Channel::Precipitation)?,
        potential_evaporation: load(Channel::PotentialEvaporation)?,
        distance_to_sea: load(Channel::DistanceToSea)?,
        crust_type: load(Channel::CrustType)?,
        climate: crate::climatology::for_map(store, &source)?,
        aridity: vec![0.; grid.cell_count()],
        flow_accumulation: vec![0.; grid.cell_count()],
        lake_runoff_loss_fraction: vec![0.; grid.cell_count()],
        ..Default::default()
    };
    let initial_ice = store
        .get_field(&source.id, Channel::LandIce)?
        .zip(store.get_field(&source.id, Channel::IceShelf)?);
    let mut evolution =
        Evolution::new(&grid, node.cell_size_m, plan).map_err(anyhow::Error::msg)?;
    if let Some(b) = &forcing {
        b.apply(&mut fields, 0).map_err(anyhow::Error::msg)?;
    }
    let boundary_grid = forcing
        .as_ref()
        .map(|b| tf_sim::refinement::BoundaryGrid::new(&grid, b))
        .transpose()
        .map_err(anyhow::Error::msg)?;
    if let Some((liquid, pet)) = tf_sim::cryosphere::liquid_background(
        &grid,
        &fields,
        initial_ice.as_ref().map(|(ground, _)| ground.as_slice()),
        &cancel,
    ) {
        fields.liquid_background_mm = liquid;
        fields.pet_background_mm = pet;
    } else if cancel() {
        return Ok(());
    }
    progress("局部演化", 0.16);
    if let Some(water_grid) = &boundary_grid {
        evolution.apply(&grid, water_grid, &mut fields, plan, &cancel)
    } else {
        evolution.apply(&grid, &grid, &mut fields, plan, &cancel)
    }
    .map_err(anyhow::Error::msg)?;
    if fields.elevation == old_elevation {
        fields.lake_depth.clone_from(&old_lakes);
        let report = evolution.report(&grid, &old_elevation, &old_lakes, &fields);
        metadata[KEY] = json!({"version": 1, "plan": plan, "report": report});
        let mut inherited = Vec::new();
        for ch in Channel::ALL {
            if cancel() {
                return Ok(());
            }
            if let Some(values) = store.get_field(&source.id, ch)? {
                inherited.push((ch, values));
            }
        }
        let channels = inherited
            .iter()
            .map(|(ch, values)| (*ch, values.as_slice()))
            .collect::<Vec<_>>();
        return store.publish_fields(&node.id, &channels, &metadata);
    }
    for (i, &old) in old_elevation.iter().enumerate() {
        fields.temperature[i] -= (fields.elevation[i].max(0.) - old.max(0.)) * 0.0065;
    }
    progress("更新降雨与冰雪", 0.5);
    if let Some(background) = fields.climate.take() {
        let local = tf_sim::regional_climate::refine(
            &background,
            &grid,
            &old_elevation,
            None,
            &grid,
            descriptor,
            &fields,
            params.orographic_strength,
            &cancel,
        )
        .map_err(anyhow::Error::msg)?;
        fields.precipitation = local.annual_precipitation;
        if source.parent.is_some() {
            let mut saved = local.atlas.clone();
            saved.circulation = None;
            metadata["climate_atlas"] = crate::climatology::encode(&saved)?;
            metadata["climate_circulation_inherited"] = true.into();
            metadata["regional_climate"] = serde_json::to_value(local.diagnostics)?;
        } else {
            metadata["climate_atlas"] = crate::climatology::encode(&local.atlas)?;
        }
        metadata["climate_model"] = json!({"name": local.atlas.model, "months": 12, "energy": "source seasonal background and elevation lapse", "water": "terrain-forced rainfall constrained to source moisture"});
        fields.climate = Some(local.atlas);
    }
    crate::child_gen::inherited_pet(&grid, &mut fields, params);
    fields.liquid_background_mm.clear();
    fields.pet_background_mm.clear();
    if fields.climate.is_some() {
        let gravity = params.planet.surface_gravity();
        fields.cryosphere = if let Some((ground, shelf)) = &initial_ice {
            tf_sim::cryosphere::solve_refinement(&grid, &fields, gravity, (ground, shelf), &cancel)
        } else {
            tf_sim::cryosphere::solve(&grid, &fields, gravity, &cancel)
        };
        if cancel() {
            return Ok(());
        }
        anyhow::ensure!(fields.cryosphere.is_some(), "冰雪求解未完成");
    }
    progress("检查水系与保护区", 0.7);
    let water = if let Some(water_grid) = &boundary_grid {
        finalize_water(water_grid, &grid, &mut fields, &cancel)
    } else {
        finalize_water(&grid, &grid, &mut fields, &cancel)
    }
    .map_err(anyhow::Error::msg)?;
    tf_sim::erosion::compute_distance_to_sea(&grid, &mut fields);
    let report = evolution.report(&grid, &old_elevation, &old_lakes, &fields);
    let ecology =
        tf_sim::ecology::run_ecology_with_diversity(&grid, &fields, params.biome_diversity);
    let biome: Vec<f32> = ecology.biome.iter().map(|&b| b as f32).collect();
    let soil: Vec<f32> = ecology.soil.iter().map(|&b| b as f32).collect();
    let receivers: Vec<_> = water
        .flow_dir
        .iter()
        .map(|&r| if r == u32::MAX { -1. } else { r as f32 })
        .collect();
    let mut inherited = Vec::new();
    for ch in [
        Channel::CrustThickness,
        Channel::CrustAge,
        Channel::PlateId,
        Channel::CrustalStrain,
        Channel::OrogenAge,
    ] {
        if let Some(values) = store.get_field(&source.id, ch)? {
            inherited.push((ch, values));
        }
    }
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
        (Channel::LiquidDischarge, &fields.liquid_discharge),
        (
            Channel::LakeRunoffLossFraction,
            &fields.lake_runoff_loss_fraction,
        ),
        (Channel::CrustType, &fields.crust_type),
    ];
    channels.extend(
        inherited
            .iter()
            .map(|(ch, values)| (*ch, values.as_slice())),
    );
    let ice = &fields.cryosphere;
    if let Some(ice) = ice {
        crate::cryosphere::attach(&mut metadata, ice)?;
        channels.extend(
            crate::cryosphere::CHANNELS
                .into_iter()
                .zip(crate::cryosphere::fields(ice)),
        );
    } else {
        metadata.as_object_mut().unwrap().remove("cryosphere_atlas");
        metadata.as_object_mut().unwrap().remove("cryosphere_model");
    }
    anyhow::ensure!(
        ecology.ecosystems.validate(grid.cell_count()),
        "生态分类结果无效"
    );
    let ecosystems = ecology.ecosystems.channels();
    crate::ecosystems::attach(&mut metadata, &ecology.ecosystems);
    channels.extend(
        crate::ecosystems::CHANNELS
            .into_iter()
            .zip(ecosystems.iter().map(Vec::as_slice)),
    );
    metadata[KEY] = json!({"version": 1, "plan": plan, "report": report});
    metadata["hydrology_model"] = json!({"name": "depression-hierarchy-annual-balance-v1", "lake_depth": "actual_supported_water", "water": water.lake_budget, "boundary_iterations": water.boundary_iterations, "sediment": evolution.sediment});
    metadata["erosion_budget_domain"] =
        "directed erosion within this map; unresolved incised load remains suspended".into();
    metadata
        .as_object_mut()
        .unwrap()
        .remove(crate::geography::KEY);
    metadata.as_object_mut().unwrap().remove("geography_error");
    crate::geography::attach(&mut metadata, &grid, &fields, &water, &cancel);
    if cancel() {
        return Ok(());
    }
    progress("保存候选图", 0.95);
    store.publish_fields(&node.id, &channels, &metadata)
}

fn finalize_water<W: GridOps>(
    water_grid: &W,
    grid: &MapGrid,
    fields: &mut WorldFields,
    cancel: &dyn Fn() -> bool,
) -> Result<tf_sim::erosion::ErosionState, String> {
    let water = tf_sim::erosion::refresh_linked_hydrology(water_grid, fields, cancel)?;
    tf_sim::cryosphere::update_water_surfaces(grid, fields);
    fields.liquid_discharge = tf_sim::cryosphere::annual_discharge(water_grid, fields, &water);
    Ok(water)
}
