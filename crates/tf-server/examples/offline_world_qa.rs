//! Exercise production routes and generation in-process, without opening a port.
//! Writes a real exported project and map diagnostics to the selected folder.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tf_server::{api::AppState, config::ServerConfig, jobs::JobManager, store::Store};
use tower::ServiceExt;

// JSON f64 text may change by a rounding bit after decoding a compressed cache.
// Keep integer identities and binary memberships exact; allow only that roundoff.
fn same_catalogue(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) if !x.is_f64() && !y.is_f64() => x == y,
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            (x - y).abs() <= 1e-12 * x.abs().max(y.abs()).max(1.)
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_catalogue(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_catalogue(v, w)))
        }
        _ => a == b,
    }
}

fn same_geography(a: &[u8], b: &[u8]) -> anyhow::Result<bool> {
    let la = u32::from_le_bytes(a[..4].try_into()?) as usize;
    let lb = u32::from_le_bytes(b[..4].try_into()?) as usize;
    Ok(a[4 + la..] == b[4 + lb..]
        && same_catalogue(
            &serde_json::from_slice(&a[4..4 + la])?,
            &serde_json::from_slice(&b[4..4 + lb])?,
        ))
}

async fn call(
    app: &Router,
    path: &str,
    token: Option<&str>,
    data: Option<Value>,
) -> anyhow::Result<Vec<u8>> {
    let mut request = Request::builder()
        .method(if data.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header("host", "test.local")
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(data.map_or(Body::empty(), |v| Body::from(v.to_string())))?)
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 128 * 1024 * 1024).await?;
    anyhow::ensure!(
        status == StatusCode::OK || status == StatusCode::ACCEPTED || status == StatusCode::CREATED,
        "{path}: {status}: {}",
        String::from_utf8_lossy(&bytes[..bytes.len().min(300)])
    );
    Ok(bytes.to_vec())
}

#[tokio::main(worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let out = PathBuf::from(args.get(1).expect("output folder"));
    let shape = args.get(2).map_or("sphere", String::as_str);
    let seed = args
        .get(3)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(42);
    std::fs::create_dir_all(&out)?;
    let data = out.join("data");
    std::fs::create_dir_all(&data)?;
    let store = Store::open(data.join("qa.db"))?;
    let config = ServerConfig {
        bind: "127.0.0.1".parse()?,
        data_dir: data,
        static_dir: None,
        base_path: String::new(),
        desktop: false,
        quality: tf_core::params::SimQuality::Economy,
        max_jobs: 1,
        max_queue: 4,
        session_ttl: Duration::from_secs(3600),
        admin_token: None,
    };
    let state = AppState {
        jobs: JobManager::new(store.clone(), 1),
        store,
        config: Arc::new(config),
        exports: Arc::new(tokio::sync::Semaphore::new(1)),
        reads: Arc::new(tokio::sync::Semaphore::new(8)),
        streams: Arc::new(tokio::sync::Semaphore::new(32)),
        admission: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    };
    let app = tf_server::application(&state, None)?;
    let session: Value =
        serde_json::from_slice(&call(&app, "/api/v1/session", None, Some(json!({}))).await?)?;
    let token = session["token"].as_str().unwrap();
    let start = Instant::now();
    let map = if let Some(source) = args.get(4) {
        // Targeted ecology replay after a rule change: retain all authoritative
        // terrain/climate/water channels from the real seed-generated project.
        let input: Value = serde_json::from_slice(&std::fs::read(source)?)?;
        let imported: Value = serde_json::from_slice(
            &call(&app, "/api/v1/projects/import", Some(token), Some(input)).await?,
        )?;
        let maps: Value = serde_json::from_slice(
            &call(
                &app,
                &format!("/api/v1/worlds/{}/maps", imported["id"].as_str().unwrap()),
                Some(token),
                None,
            )
            .await?,
        )?;
        let id = maps
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["parent"].is_null())
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let node = state.store.get_map_node(&id)?.unwrap();
        let grid = node.grid.as_ref().unwrap().build();
        let n = node.cells as usize;
        let field = |channel| {
            state
                .store
                .get_field(&id, channel)?
                .ok_or_else(|| anyhow::anyhow!("missing replay field"))
        };
        use tf_server::store::Channel;
        let mut f = tf_sim::WorldFields::with_capacity(n);
        f.elevation = field(Channel::Elevation)?;
        f.temperature = field(Channel::Temperature)?;
        f.precipitation = field(Channel::Precipitation)?;
        f.lake_depth = field(Channel::LakeDepth)?;
        f.flow_accumulation = field(Channel::FlowAccumulation)?;
        f.liquid_discharge = state
            .store
            .get_field(&id, Channel::LiquidDischarge)?
            .unwrap_or_default();
        f.distance_to_sea = field(Channel::DistanceToSea)?;
        f.climate = tf_server::climatology::for_map(&state.store, &node)?;
        if let Some(atlas) = tf_server::cryosphere::for_map(&state.store, &node)? {
            f.cryosphere = Some(tf_sim::cryosphere::Cryosphere {
                atlas,
                land_ice_m: field(Channel::LandIce)?,
                snow_cover: field(Channel::SnowCover)?,
                sea_ice_cover: field(Channel::SeaIce)?,
                shelf_ice_m: field(Channel::IceShelf)?,
                permafrost: field(Channel::Permafrost)?,
                active_layer_m: field(Channel::ActiveLayer)?,
                liquid_water_mm: field(Channel::LiquidWater)?,
                lake_ice_cover: field(Channel::LakeIce)?,
                pet_mm_year: vec![0.; n],
            });
        }
        let mut refreshed = Vec::<(Channel, Vec<f32>)>::new();
        let mut metadata = state.store.get_metadata(&id)?.unwrap();
        if args.get(5).is_some_and(|s| s == "cryosphere") {
            f.cryosphere = tf_sim::cryosphere::solve(&grid, &f, 9.81, &|| false);
            anyhow::ensure!(f.cryosphere.is_some(), "fine ice solve failed");
            let water = tf_sim::erosion::refresh_hydrology(&grid, &mut f);
            tf_sim::cryosphere::update_water_surfaces(&grid, &mut f);
            f.liquid_discharge = tf_sim::cryosphere::annual_discharge(&grid, &f, &water);
            let ecology = tf_sim::ecology::run_ecology(&grid, &f);
            refreshed.push((
                Channel::Biome,
                ecology.biome.iter().map(|&v| v as u8 as f32).collect(),
            ));
            refreshed.push((
                Channel::Soil,
                ecology.soil.iter().map(|&v| v as u8 as f32).collect(),
            ));
            for (channel, values) in [
                (Channel::LakeDepth, &f.lake_depth),
                (Channel::FlowAccumulation, &f.flow_accumulation),
                (Channel::LiquidDischarge, &f.liquid_discharge),
                (Channel::PotentialEvaporation, &f.potential_evaporation),
            ] {
                refreshed.push((channel, values.clone()));
            }
            refreshed.push((
                Channel::FlowReceiver,
                water
                    .flow_dir
                    .iter()
                    .map(|&v| if v == u32::MAX { -1. } else { v as f32 })
                    .collect(),
            ));
            metadata["hydrology_model"]["water"] = serde_json::to_value(&water.lake_budget)?;
            let ice = f.cryosphere.as_ref().unwrap();
            tf_server::cryosphere::attach(&mut metadata, ice)?;
            refreshed.extend(
                tf_server::cryosphere::CHANNELS.into_iter().zip(
                    tf_server::cryosphere::fields(ice)
                        .into_iter()
                        .map(|v| v.to_vec()),
                ),
            );
            tf_server::geography::attach(&mut metadata, &grid, &f, &water, &|| false);
            println!(
                "Recomputed map-grid ice, water and ecology from {source}; budget {:?}",
                ice.atlas.ice_budget
            );
        }
        let legacy = refreshed
            .iter()
            .find(|(c, _)| *c == Channel::Biome)
            .map(|(_, v)| v.clone())
            .unwrap_or(field(Channel::Biome)?)
            .iter()
            .map(|&v| v as u8)
            .collect::<Vec<_>>();
        let soil = refreshed
            .iter()
            .find(|(c, _)| *c == Channel::Soil)
            .map(|(_, v)| v.clone())
            .unwrap_or(field(Channel::Soil)?)
            .iter()
            .map(|&v| v as u8)
            .collect::<Vec<_>>();
        let e = tf_sim::ecosystems::classify(&grid, &f, &legacy, &soil);
        let channels = e.channels();
        tf_server::ecosystems::attach(&mut metadata, &e);
        refreshed.extend(
            tf_server::ecosystems::CHANNELS
                .into_iter()
                .zip(channels.iter().cloned()),
        );
        state.store.publish_fields(
            &id,
            &refreshed
                .iter()
                .map(|(c, v)| (*c, v.as_slice()))
                .collect::<Vec<_>>(),
            &metadata,
        )?;
        assert_eq!(field(Channel::Elevation)?, f.elevation);
        println!("Replayed saved project and refreshed ecosystem classification from {source}");
        id
    } else {
        let world: Value = serde_json::from_slice(
        &call(
            &app,
            "/api/v1/worlds",
            Some(token),
            Some(json!({
                "name":"Offline cryosphere regression", "seed": seed, "params":{"world_shape":shape}
            })),
        )
        .await?,
    )?;
        let job: Value = serde_json::from_slice(
            &call(
                &app,
                &format!("/api/v1/worlds/{}/generate", world["id"].as_str().unwrap()),
                Some(token),
                Some(json!({})),
            )
            .await?,
        )?;
        let mut poll = 0;
        loop {
            let status: Value = serde_json::from_slice(
                &call(
                    &app,
                    &format!("/api/v1/jobs/{}", job["job_id"].as_str().unwrap()),
                    Some(token),
                    None,
                )
                .await?,
            )?;
            anyhow::ensure!(
                status["state"] != "failed" && status["state"] != "cancelled",
                "generation: {status}"
            );
            if poll % 15 == 0 {
                println!(
                    "{}s {} {}",
                    start.elapsed().as_secs(),
                    status["stage"],
                    status["progress"]
                );
            }
            if status["state"] == "done" {
                break;
            }
            anyhow::ensure!(
                start.elapsed() < Duration::from_secs(1800),
                "generation timeout"
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
            poll += 1;
        }
        job["map_id"].as_str().unwrap().to_owned()
    };
    for (suffix, file) in [
        ("export.json", "world.terraforge.json"),
        ("metadata", "metadata.json"),
        ("hydrology", "hydrology.json"),
        ("surface", "surface.bin"),
        ("geography", "geography.bin"),
        ("ecosystems", "ecosystems.bin"),
        ("export.png?size=1024&ecosystem=group", "ecosystems.png"),
        ("cryosphere?month=0", "ice-0.bin"),
        ("cryosphere?month=1", "ice-1.bin"),
        ("cryosphere?month=7", "ice-7.bin"),
        ("hydrology?month=1", "hydrology-1.json"),
        ("hydrology?month=7", "hydrology-7.json"),
    ] {
        std::fs::write(
            out.join(file),
            call(
                &app,
                &format!("/api/v1/maps/{map}/{suffix}"),
                Some(token),
                None,
            )
            .await?,
        )?;
    }
    let saved: Value = serde_json::from_slice(&std::fs::read(out.join("world.terraforge.json"))?)?;
    let imported: Value = serde_json::from_slice(
        &call(&app, "/api/v1/projects/import", Some(token), Some(saved)).await?,
    )?;
    let maps: Value = serde_json::from_slice(
        &call(
            &app,
            &format!("/api/v1/worlds/{}/maps", imported["id"].as_str().unwrap()),
            Some(token),
            None,
        )
        .await?,
    )?;
    let restored = maps[0]["id"].as_str().unwrap();
    for (suffix, file) in [
        ("cryosphere?month=0", "ice-0.bin"),
        ("cryosphere?month=1", "ice-1.bin"),
        ("cryosphere?month=7", "ice-7.bin"),
        ("hydrology", "hydrology.json"),
        ("geography", "geography.bin"),
        ("ecosystems", "ecosystems.bin"),
    ] {
        let actual = call(
            &app,
            &format!("/api/v1/maps/{restored}/{suffix}"),
            Some(token),
            None,
        )
        .await?;
        let expected = std::fs::read(out.join(file))?;
        anyhow::ensure!(
            if suffix == "geography" {
                same_geography(&actual, &expected)?
            } else {
                actual == expected
            },
            "saved {suffix} changed after import"
        );
    }
    std::fs::write(
        out.join("report.json"),
        serde_json::to_vec_pretty(
            &json!({"shape":shape,"seed":seed,"seconds":start.elapsed().as_secs_f64(),"replayed_from":args.get(4),"recomputed_cryosphere":args.get(5).is_some_and(|s|s=="cryosphere"),"result":"passed","transport":"in-process production router; no TCP listener"}),
        )?,
    )?;
    println!("Full project written: {}", out.display());
    Ok(())
}
