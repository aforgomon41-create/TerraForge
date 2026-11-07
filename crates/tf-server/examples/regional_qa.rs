//! Fixed landscapes through the actual child generator and HTTP router, no port.
use axum::{
    body::{to_bytes, Body},
    http::Request,
    Router,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};
use tf_core::params::{GenerationParams, LandscapeKind, LevelKind, SimQuality};
use tf_grid::{GridDescriptor, GridOps};
use tf_server::{
    api::AppState,
    config::ServerConfig,
    jobs::{FrameCache, JobManager},
    store::{Channel, MapNodeRecord, Store, WorldRecord},
};
use tower::ServiceExt;
async fn call(
    app: &Router,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> anyhow::Result<Vec<u8>> {
    call_body(app, path, token, body.map(|b| b.to_string().into_bytes())).await
}
async fn call_body(
    app: &Router,
    path: &str,
    token: Option<&str>,
    body: Option<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    let mut r = Request::builder()
        .uri(path)
        .method(if body.is_some() { "POST" } else { "GET" })
        .header("host", "test.local")
        .header("content-type", "application/json");
    if let Some(t) = token {
        r = r.header("authorization", format!("Bearer {t}"));
    }
    let response = app
        .clone()
        .oneshot(r.body(body.map_or(Body::empty(), Body::from))?)
        .await?;
    let status = response.status();
    let b = to_bytes(response.into_body(), 128 * 1024 * 1024).await?;
    anyhow::ensure!(
        status.is_success(),
        "{path}: {status}: {}",
        String::from_utf8_lossy(&b[..b.len().min(500)])
    );
    Ok(b.to_vec())
}
#[tokio::main(worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).expect("output folder"));
    std::fs::create_dir_all(&out)?;
    let store = Store::open(out.join("qa.db"))?;
    let config = ServerConfig {
        bind: "127.0.0.1".parse()?,
        data_dir: out.clone(),
        static_dir: None,
        base_path: String::new(),
        desktop: false,
        quality: SimQuality::Economy,
        max_jobs: 1,
        max_queue: 4,
        session_ttl: Duration::from_secs(3600),
        admin_token: None,
    };
    let state = AppState {
        jobs: JobManager::new(store.clone(), 1),
        store: store.clone(),
        config: Arc::new(config),
        exports: Arc::new(tokio::sync::Semaphore::new(1)),
        reads: Arc::new(tokio::sync::Semaphore::new(8)),
        streams: Arc::new(tokio::sync::Semaphore::new(32)),
        admission: Arc::new(tokio::sync::Mutex::new(vec![])),
    };
    let app = tf_server::application(&state, None)?;
    let s: Value =
        serde_json::from_slice(&call(&app, "/api/v1/session", None, Some(json!({}))).await?)?;
    let token = s["token"].as_str().unwrap();
    let owner = s["owner"].as_str().unwrap();
    if let Some(source) = std::env::args().nth(2) {
        return full_continent(&app, &state, token, &out, &source).await;
    }
    let mut reports = vec![];
    for (index, (name, kind, rain, relief)) in [
        ("mountain", LandscapeKind::Mountains, 1100., 2600.),
        ("plain", LandscapeKind::Plains, 900., 120.),
        ("arid", LandscapeKind::Valley, 60., 700.),
        ("estuary", LandscapeKind::Coast, 1500., 700.),
    ]
    .into_iter()
    .enumerate()
    {
        let start = Instant::now();
        let dir = out.join(name);
        std::fs::create_dir_all(&dir)?;
        let mut p = GenerationParams::for_level(LevelKind::Region);
        p.seed = 4200 + index as u64;
        p.landscape = kind;
        p.terrain_relief_m = relief;
        p.regional_precipitation_mm = rain;
        p.regional_temperature_c = 16.;
        p.map_width_m = 360000.;
        p.map_height_m = 270000.;
        p.center_lat_deg = 25.;
        p.center_lon_deg = 179.7;
        let world_id = format!("world-{name}");
        store.put_world(&WorldRecord {
            id: world_id.clone(),
            name: name.into(),
            seed: p.seed,
            params_json: serde_json::to_string(&p)?,
            owner_token: owner.into(),
            share_token: None,
            created_at: 0,
            updated_at: 0,
        })?;
        let descriptor = GridDescriptor::plane(25., 179.7, 360000., 96, 72, p.planet.radius_m);
        let g = descriptor.build();
        let parent = MapNodeRecord {
            id: format!("parent-{name}"),
            world_id: world_id.clone(),
            name: format!("{name} background"),
            kind: "region".into(),
            parent: None,
            cells: g.cell_count() as u32,
            cell_size_m: 3750.,
            extent_json: serde_json::to_string(&descriptor)?,
            status: "pending".into(),
            created_at: 0,
            completed_at: None,
            grid: Some(descriptor),
            seed: p.seed,
            generator_version: "tf-0.6".into(),
            revision: None,
        };
        store.put_map_node(&parent)?;
        let result =
            tf_sim::generate_cancellable(&g, &p, &mut |_, _| {}, &mut |_, _| {}, &|| false)
                .ok_or_else(|| anyhow::anyhow!("parent generation failed"))?;
        tf_server::jobs::persist_fields(&store, &parent.id, &result, &g, &|| false)?;
        drop(result);
        let descriptor = GridDescriptor::plane(25., 179.7, 80000., 128, 128, p.planet.radius_m);
        let child = MapNodeRecord {
            id: format!("child-{name}"),
            world_id: world_id.clone(),
            name: format!("{name} detail"),
            kind: "site".into(),
            parent: Some(parent.id.clone()),
            cells: 16384,
            cell_size_m: 625.,
            extent_json: serde_json::to_string(&descriptor)?,
            status: "pending".into(),
            created_at: 1,
            completed_at: None,
            grid: Some(descriptor),
            seed: p.seed,
            generator_version: "tf-0.8".into(),
            revision: None,
        };
        store.put_map_node(&child)?;
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let frames: FrameCache = Default::default();
        tf_server::child_gen::run_child_generation(
            &store,
            &tx,
            &frames,
            "qa",
            &child,
            &p,
            &AtomicBool::new(false),
        )?;
        let h = store.get_field(&child.id, Channel::Elevation)?.unwrap();
        let q = store
            .get_field(&child.id, Channel::LiquidDischarge)?
            .unwrap();
        let lake = store.get_field(&child.id, Channel::LakeDepth)?.unwrap();
        let meta = store.get_metadata(&child.id)?.unwrap();
        let water = &meta["hydrology_model"]["water"];
        let supply = water["precipitation_m3_yr"].as_f64().unwrap()
            + water["external_inflow_m3_yr"].as_f64().unwrap()
            + water["prescribed_head_supply_m3_yr"].as_f64().unwrap();
        anyhow::ensure!(
            water["residual_m3_yr"].as_f64().unwrap().abs() < supply.max(1.) * 1e-8,
            "water ledger did not close"
        );
        let land = h.iter().filter(|&&h| h > 0.).count();
        let lakes = h
            .iter()
            .zip(&lake)
            .filter(|(h, l)| **h > 0. && **l > 5.)
            .count();
        anyhow::ensure!(
            lakes as f64 / (land.max(1) as f64) < 0.35,
            "{name}: artificial lake coverage {lakes}/{land}"
        );
        let mut again = child.clone();
        again.id = format!("repeat-{name}");
        store.put_map_node(&again)?;
        tf_server::child_gen::run_child_generation(
            &store,
            &tx,
            &frames,
            "qa",
            &again,
            &p,
            &AtomicBool::new(false),
        )?;
        anyhow::ensure!(
            store.get_field(&again.id, Channel::Elevation)?.unwrap() == h
                && store
                    .get_field(&again.id, Channel::LiquidDischarge)?
                    .unwrap()
                    == q,
            "same seed was not deterministic"
        );
        for (suffix, file) in [
            ("export.json", "world.terraforge.json"),
            ("metadata", "metadata.json"),
            ("surface", "surface.bin"),
            ("hydrology", "hydrology.json"),
            ("hydrology?month=1", "hydrology-1.json"),
            ("hydrology?month=7", "hydrology-7.json"),
            ("ecosystems", "ecosystems.bin"),
            ("cryosphere?month=0", "ice-0.bin"),
            ("cryosphere?month=1", "ice-1.bin"),
            ("cryosphere?month=7", "ice-7.bin"),
        ] {
            std::fs::write(
                dir.join(file),
                call(
                    &app,
                    &format!("/api/v1/maps/{}/{suffix}", child.id),
                    Some(token),
                    None,
                )
                .await?,
            )?;
        }
        let project: Value =
            serde_json::from_slice(&std::fs::read(dir.join("world.terraforge.json"))?)?;
        let restored: Value = serde_json::from_slice(
            &call(&app, "/api/v1/projects/import", Some(token), Some(project)).await?,
        )?;
        let restored_world = restored["id"].as_str().unwrap().to_owned();
        let restored = store
            .list_map_nodes(restored["id"].as_str().unwrap())?
            .into_iter()
            .find(|m| m.name == child.name)
            .unwrap();
        for ch in [
            Channel::Elevation,
            Channel::LiquidDischarge,
            Channel::LakeRunoffLossFraction,
        ] {
            anyhow::ensure!(
                store.get_field(&restored.id, ch)? == store.get_field(&child.id, ch)?,
                "import changed {}",
                ch.name()
            );
        }
        for (suffix, file) in [
            ("hydrology", "hydrology.json"),
            ("hydrology?month=1", "hydrology-1.json"),
            ("hydrology?month=7", "hydrology-7.json"),
            ("cryosphere?month=0", "ice-0.bin"),
        ] {
            anyhow::ensure!(
                call(
                    &app,
                    &format!("/api/v1/maps/{}/{suffix}", restored.id),
                    Some(token),
                    None
                )
                .await?
                    == std::fs::read(dir.join(file))?,
                "import changed {suffix}"
            );
        }
        let r = json!({"case":name,"seed":p.seed,"seconds":start.elapsed().as_secs_f64(),"map_id":child.id,"water":water,"refinement":meta["refinement_model"],"climate":meta["climate_model"],"return_links":meta[tf_server::refinement::KEY_V2]["links"].as_array().map_or(0,Vec::len),"land_cells":land,"deep_lake_cells":lakes,"min_m":h.iter().copied().fold(f32::INFINITY,f32::min),"max_m":h.iter().copied().fold(f32::NEG_INFINITY,f32::max),"max_river_m3_s":q.iter().copied().fold(0.,f32::max),"determinism":true,"project_and_monthly_roundtrip":true});
        println!("{}", serde_json::to_string(&r)?);
        reports.push(r);
        state.jobs.delete_world(&world_id).await?;
        state.jobs.delete_world(&restored_world).await?;
    }
    std::fs::write(
        out.join("report.json"),
        serde_json::to_vec_pretty(&reports)?,
    )?;
    Ok(())
}

async fn full_continent(
    app: &Router,
    state: &AppState,
    token: &str,
    out: &std::path::Path,
    source: &str,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let input: Value = serde_json::from_slice(&std::fs::read(source)?)?;
    let imported: Value = serde_json::from_slice(
        &call(app, "/api/v1/projects/import", Some(token), Some(input)).await?,
    )?;
    let maps = state
        .store
        .list_map_nodes(imported["id"].as_str().unwrap())?;
    let parent = maps.iter().find(|m| m.parent.is_none()).unwrap();
    let bytes = call(
        app,
        &format!("/api/v1/maps/{}/geography", parent.id),
        Some(token),
        None,
    )
    .await?;
    let g = tf_server::geography::parse(&bytes, parent.cells as usize)?;
    let mut features: Vec<_> = g
        .features
        .iter()
        .filter(|f| f.level == "continent")
        .collect();
    features.sort_by(|a, b| b.area_m2.total_cmp(&a.area_m2));
    let mut chosen = None;
    for f in features {
        let p: Value = serde_json::from_slice(
            &call(
                app,
                &format!(
                    "/api/v1/maps/{}/refinement-plan?feature_id={}",
                    parent.id, f.id
                ),
                Some(token),
                None,
            )
            .await?,
        )?;
        if p["can_generate"] == true && p["complete_object"] == true {
            chosen = Some(p);
            break;
        }
    }
    let p = chosen.ok_or_else(|| anyhow::anyhow!("no complete continent fits current budget"))?;
    std::fs::write(out.join("plan.json"), serde_json::to_vec_pretty(&p)?)?;
    println!("Plan: {}", p);
    let generated:Value=serde_json::from_slice(&call(app,&format!("/api/v1/maps/{}/children",parent.id),Some(token),Some(json!({"kind":"continent","name":"Full continent QA","width_m":p["width_m"],"center_lat":p["center_lat"],"center_lon":p["center_lon"],"cell_size_m":p["cell_size_m"]}))).await?)?;
    let map = generated["map_id"].as_str().unwrap();
    let job = generated["job_id"].as_str().unwrap();
    let mut last_stage = String::new();
    let mut last_report = Instant::now();
    loop {
        let j = state.store.get_job(job)?.unwrap();
        if j.stage != last_stage || last_report.elapsed() > Duration::from_secs(15) {
            println!(
                "{}s: {} {:.0}%",
                start.elapsed().as_secs(),
                j.stage,
                j.progress * 100.
            );
            last_stage = j.stage.clone();
            last_report = Instant::now();
        }
        if j.state == "done" {
            break;
        }
        anyhow::ensure!(j.state != "failed", "{:?}", j.error);
        if start.elapsed() > Duration::from_secs(600) {
            state.jobs.cancel(job).await;
            anyhow::bail!("generation timed out");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let node = state.store.get_map_node(map)?.unwrap();
    let grid = node.grid.as_ref().unwrap().build();
    let parent_grid = parent.grid.as_ref().unwrap().build();
    let feature = p["feature_id"].as_u64().unwrap() as u32;
    let covered = g
        .landmass
        .iter()
        .enumerate()
        .filter(|(_, id)| **id == feature)
        .all(|(i, _)| grid.cell_at(parent_grid.center_dir(i)).is_some());
    anyhow::ensure!(covered, "full continent was truncated");
    drop(parent_grid);
    for (suffix, file) in [
        ("export.json", "world.terraforge.json"),
        ("metadata", "metadata.json"),
        ("surface", "surface.bin"),
        ("hydrology", "hydrology.json"),
        ("hydrology?month=1", "hydrology-1.json"),
        ("hydrology?month=7", "hydrology-7.json"),
        ("ecosystems", "ecosystems.bin"),
        ("cryosphere?month=0", "ice-0.bin"),
        ("cryosphere?month=1", "ice-1.bin"),
        ("cryosphere?month=7", "ice-7.bin"),
    ] {
        std::fs::write(
            out.join(file),
            call(
                app,
                &format!("/api/v1/maps/{map}/{suffix}"),
                Some(token),
                None,
            )
            .await?,
        )?;
    }
    let meta = state.store.get_metadata(map)?.unwrap();
    let water = &meta["hydrology_model"]["water"];
    let supply = water["precipitation_m3_yr"].as_f64().unwrap()
        + water["external_inflow_m3_yr"].as_f64().unwrap()
        + water["prescribed_head_supply_m3_yr"].as_f64().unwrap();
    anyhow::ensure!(
        water["residual_m3_yr"].as_f64().unwrap().abs() < supply.max(1.) * 1e-8,
        "water budget"
    );
    let generation_seconds = start.elapsed().as_secs_f64();
    let project: Value =
        serde_json::from_slice(&std::fs::read(out.join("world.terraforge.json"))?)?;
    let restored: Value = serde_json::from_slice(
        &call(app, "/api/v1/projects/import", Some(token), Some(project)).await?,
    )?;
    let restored = state
        .store
        .list_map_nodes(restored["id"].as_str().unwrap())?
        .into_iter()
        .find(|m| m.name == node.name)
        .unwrap();
    for ch in [
        Channel::Elevation,
        Channel::FlowReceiver,
        Channel::LiquidDischarge,
        Channel::LakeRunoffLossFraction,
    ] {
        anyhow::ensure!(
            state.store.get_field(map, ch)? == state.store.get_field(&restored.id, ch)?,
            "full continent import changed {}",
            ch.name()
        );
    }
    for (suffix, file) in [
        ("hydrology", "hydrology.json"),
        ("hydrology?month=1", "hydrology-1.json"),
        ("hydrology?month=7", "hydrology-7.json"),
        ("cryosphere?month=0", "ice-0.bin"),
    ] {
        anyhow::ensure!(
            call(
                app,
                &format!("/api/v1/maps/{}/{suffix}", restored.id),
                Some(token),
                None
            )
            .await?
                == std::fs::read(out.join(file))?,
            "full continent import changed {suffix}"
        );
    }
    let bytes = call(
        app,
        &format!("/api/v1/maps/{map}/geography"),
        Some(token),
        None,
    )
    .await?;
    let regions = tf_server::geography::parse(&bytes, node.cells as usize)?;
    let mut candidates: Vec<_> = regions
        .features
        .iter()
        .filter(|f| f.level == "region" && f.lat.abs() < 50. && f.elevation_m < 1000.)
        .collect();
    candidates.sort_by(|a, b| b.area_m2.total_cmp(&a.area_m2));
    let mut region_plan = None;
    for f in candidates.into_iter().take(20) {
        let plan: Value = serde_json::from_slice(
            &call(
                app,
                &format!("/api/v1/maps/{map}/refinement-plan?feature_id={}", f.id),
                Some(token),
                None,
            )
            .await?,
        )?;
        if plan["can_generate"] == true && plan["complete_object"] == true {
            region_plan = Some(plan);
            break;
        }
    }
    let region_plan =
        region_plan.ok_or_else(|| anyhow::anyhow!("no interior drainage region was refinable"))?;
    let generated:Value=serde_json::from_slice(&call(app,&format!("/api/v1/maps/{map}/children"),Some(token),Some(json!({"kind":"region","name":"Second-level drainage QA","width_m":region_plan["width_m"],"center_lat":region_plan["center_lat"],"center_lon":region_plan["center_lon"],"cell_size_m":region_plan["cell_size_m"]}))).await?)?;
    let region_id = generated["map_id"].as_str().unwrap();
    let region_job = generated["job_id"].as_str().unwrap();
    let region_start = Instant::now();
    loop {
        let j = state.store.get_job(region_job)?.unwrap();
        if j.state == "done" {
            break;
        }
        anyhow::ensure!(j.state != "failed", "nested region failed: {:?}", j.error);
        if region_start.elapsed() > Duration::from_secs(180) {
            state.jobs.cancel(region_job).await;
            anyhow::bail!("nested region timed out");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let region_dir = out.join("region");
    std::fs::create_dir_all(&region_dir)?;
    for (suffix, file) in [
        ("export.json", "world.terraforge.json"),
        ("metadata", "metadata.json"),
        ("surface", "surface.bin"),
        ("hydrology", "hydrology.json"),
        ("hydrology?month=1", "hydrology-1.json"),
        ("hydrology?month=7", "hydrology-7.json"),
        ("ecosystems", "ecosystems.bin"),
        ("cryosphere?month=0", "ice-0.bin"),
        ("cryosphere?month=1", "ice-1.bin"),
        ("cryosphere?month=7", "ice-7.bin"),
    ] {
        std::fs::write(
            region_dir.join(file),
            call(
                app,
                &format!("/api/v1/maps/{region_id}/{suffix}"),
                Some(token),
                None,
            )
            .await?,
        )?;
    }
    let packed = call(
        app,
        &format!("/api/v1/maps/{region_id}/export.json?packed=true"),
        Some(token),
        None,
    )
    .await?;
    anyhow::ensure!(
        packed.len() <= 64 * 1024 * 1024,
        "saved project cannot be reopened under web upload cap"
    );
    let packed_size = packed.len();
    std::fs::write(region_dir.join("world.tfworld"), &packed)?;
    let imported: Value = serde_json::from_slice(
        &call_body(app, "/api/v1/projects/import", Some(token), Some(packed)).await?,
    )?;
    let restored = state
        .store
        .list_map_nodes(imported["id"].as_str().unwrap())?
        .into_iter()
        .find(|n| n.name == "Second-level drainage QA")
        .unwrap();
    for (suffix, file) in [
        ("hydrology", "hydrology.json"),
        ("hydrology?month=1", "hydrology-1.json"),
        ("hydrology?month=7", "hydrology-7.json"),
        ("cryosphere?month=0", "ice-0.bin"),
    ] {
        anyhow::ensure!(
            call(
                app,
                &format!("/api/v1/maps/{}/{suffix}", restored.id),
                Some(token),
                None
            )
            .await?
                == std::fs::read(region_dir.join(file))?,
            "nested packed import changed {suffix}"
        );
    }
    let report = json!({"result":"passed","source":source,"map_id":map,"feature":p,"all_source_cells_covered":covered,"cells":node.cells,"refinement":meta["refinement_model"],"water":water,"generation_and_export_seconds":generation_seconds,"full_project_monthly_roundtrip":true,"nested_region":{"map_id":region_id,"plan":region_plan,"seconds":region_start.elapsed().as_secs_f64(),"packed_project_bytes":packed_size,"packed_project_monthly_roundtrip":true},"total_seconds":start.elapsed().as_secs_f64()});
    std::fs::write(out.join("report.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("{report}");
    Ok(())
}
