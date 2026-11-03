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
    let start = Instant::now();
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
    let map = job["map_id"].as_str().unwrap();
    for (suffix, file) in [
        ("export.json", "world.terraforge.json"),
        ("metadata", "metadata.json"),
        ("hydrology", "hydrology.json"),
        ("surface", "surface.bin"),
        ("geography", "geography.bin"),
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
            &json!({"shape":shape,"seed":seed,"seconds":start.elapsed().as_secs_f64(),"result":"passed","transport":"in-process production router; no TCP listener"}),
        )?,
    )?;
    println!("Full project written: {}", out.display());
    Ok(())
}
