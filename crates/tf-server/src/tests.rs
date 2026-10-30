use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use store::{Channel, JobRecord, MapNodeRecord};
use tower::ServiceExt;
struct Fixture {
    app: Router,
    state: api::AppState,
    _dir: tempfile::TempDir,
}
impl Fixture {
    fn new(desktop: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("test.db")).unwrap();
        let config = ServerConfig {
            bind: "127.0.0.1".parse().unwrap(),
            data_dir: dir.path().into(),
            static_dir: None,
            base_path: String::new(),
            desktop,
            quality: tf_core::params::SimQuality::Economy,
            max_jobs: 1,
            max_queue: 4,
            session_ttl: std::time::Duration::from_secs(60),
            admin_token: Some("test-admin-token-with-at-least-32-characters".into()),
        };
        let state = api::AppState {
            jobs: JobManager::new(store.clone(), 1),
            store,
            config: Arc::new(config),
            exports: Arc::new(tokio::sync::Semaphore::new(1)),
            reads: Arc::new(tokio::sync::Semaphore::new(8)),
            streams: Arc::new(tokio::sync::Semaphore::new(32)),
            admission: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        };
        Self {
            app: api::build_router(state.clone()),
            state,
            _dir: dir,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(path).header(
            "host",
            if self.state.config.desktop {
                "127.0.0.1"
            } else {
                "test.local"
            },
        );
        if let Some(t) = token {
            request = request.header("authorization", format!("Bearer {t}"));
        }
        let request = request
            .header("content-type", "application/json")
            .body(body.map_or(Body::empty(), |b| Body::from(b.to_string())))
            .unwrap();
        let res = self.app.clone().oneshot(request).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 2_000_000).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    async fn session(&self) -> String {
        self.request("POST", "/api/v1/session", None, None).await.1["token"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn world(&self, token: &str) -> String {
        let (s, b) = self
            .request(
                "POST",
                "/api/v1/worlds",
                Some(token),
                Some(json!({"name":"Test world","seed":42,"params":{"quality":"high"}})),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        b["id"].as_str().unwrap().into()
    }
}
fn node(world: &str, id: &str) -> MapNodeRecord {
    MapNodeRecord {
        id: id.into(),
        world_id: world.into(),
        name: "Test map".into(),
        kind: "world".into(),
        parent: None,
        cells: 655362,
        cell_size_m: 28000.,
        extent_json: "{}".into(),
        status: "pending".into(),
        created_at: 0,
        completed_at: None,
        grid: Some(tf_grid::GridDescriptor::sphere(256, 6371000.)),
        seed: 42,
        generator_version: "tf-0.2".into(),
    }
}
fn job(world: &str, map: &str, id: &str) -> JobRecord {
    JobRecord {
        id: id.into(),
        world_id: world.into(),
        map_id: map.into(),
        state: "queued".into(),
        stage: String::new(),
        progress: 0.,
        error: None,
        created_at: 0,
        started_at: None,
        finished_at: None,
    }
}
#[tokio::test]
async fn capabilities_isolate_world_map_field_job_events_and_delete() {
    let f = Fixture::new(false);
    let a = f.session().await;
    let b = f.session().await;
    let w = f.world(&a).await;
    f.state
        .store
        .submit_job(&node(&w, "map-test"), &job(&w, "map-test", "job-test"))
        .unwrap();
    f.state
        .store
        .put_field("map-test", Channel::Elevation, &[1.])
        .unwrap();
    for (method, path) in [
        ("GET", format!("/api/v1/worlds/{w}")),
        ("DELETE", format!("/api/v1/worlds/{w}")),
        ("POST", format!("/api/v1/worlds/{w}/generate")),
        ("GET", "/api/v1/maps/map-test".into()),
        ("GET", "/api/v1/maps/map-test/field/elevation".into()),
        ("GET", "/api/v1/maps/map-test/surface".into()),
        (
            "GET",
            "/api/v1/maps/map-test/overlay?spacing_m=100000".into(),
        ),
        ("GET", "/api/v1/jobs/job-test".into()),
        ("POST", "/api/v1/jobs/job-test/cancel".into()),
        ("GET", "/api/v1/events?job_id=job-test".into()),
    ] {
        let (status, _) = f.request(method, &path, Some(&b), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}")
    }
    assert_eq!(
        f.request("GET", "/api/v1/worlds", Some(&b), None).await.1,
        json!([])
    );
    assert_eq!(
        f.request("GET", &format!("/api/v1/worlds/{w}"), Some(&a), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request("GET", "/api/v1/worlds", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let world = f.state.store.get_world(&w).unwrap().unwrap();
    let params: tf_core::params::GenerationParams =
        serde_json::from_str(&world.params_json).unwrap();
    assert_eq!(params.quality, tf_core::params::SimQuality::Economy);
    assert_eq!(
        f.request("DELETE", &format!("/api/v1/worlds/{w}"), Some(&a), None)
            .await
            .0,
        StatusCode::OK
    );
    assert!(f.state.store.get_map_node("map-test").unwrap().is_none());
    assert!(f.state.store.get_job("job-test").unwrap().is_none());
    assert!(f
        .state
        .store
        .get_field("map-test", Channel::Elevation)
        .unwrap()
        .is_none());
}
#[tokio::test]
async fn cross_origin_and_management_requests_are_blocked() {
    let f = Fixture::new(false);
    let token = f.session().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/session")
        .header("host", "test.local")
        .header("origin", "https://other.test")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        f.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request(
            "PATCH",
            "/api/v1/system/concurrency",
            Some(&token),
            Some(json!({"max_concurrent":8}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for n in [8, 1, 8, 1] {
        assert_eq!(
            f.request(
                "PATCH",
                "/api/v1/system/concurrency",
                Some("test-admin-token-with-at-least-32-characters"),
                Some(json!({"max_concurrent":n}))
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(f.state.jobs.max_concurrent().await, n);
    }
    assert_eq!(f.state.jobs.counts().await, (0, 0));
}
#[tokio::test]
async fn cancellation_and_restart_never_publish_partial_or_late_data() {
    let f = Fixture::new(false);
    let token = f.session().await;
    let world = f.world(&token).await;
    let mut n = node(&world, "m");
    n.cells = 16;
    n.grid = Some(tf_grid::GridDescriptor::patch(0., 0., 4000., 4, 6371000.));
    f.state
        .store
        .submit_job(&n, &job(&world, "m", "j"))
        .unwrap();
    f.state.store.transition_job("j", "running", None).unwrap();
    assert!(f.state.store.request_cancel("j").unwrap());
    f.state.store.update_progress("j", "late", 0.9).unwrap();
    assert_eq!(
        f.state.store.get_job("j").unwrap().unwrap().state,
        "cancelling"
    );
    assert!(f
        .state
        .store
        .publish_fields("m", &[(Channel::Elevation, &[1.; 16])], &json!({}))
        .is_err());
    f.state.jobs.recover().unwrap();
    assert_eq!(
        f.state.store.get_map_node("m").unwrap().unwrap().status,
        "failed"
    );
    f.state.store.delete_world(&world).unwrap();
    f.state.store.update_progress("j", "late", 0.9).unwrap();
    assert!(f.state.store.get_job("j").unwrap().is_none());
    assert!(f
        .state
        .store
        .publish_fields("m", &[(Channel::Elevation, &[1.; 16])], &json!({}))
        .is_err());
}
#[tokio::test]
async fn publishing_updates_map_and_job_in_the_same_transaction() {
    let f = Fixture::new(false);
    let t = f.session().await;
    let w = f.world(&t).await;
    let mut n = node(&w, "m");
    n.cells = 16;
    f.state.store.submit_job(&n, &job(&w, "m", "j")).unwrap();
    f.state.store.transition_job("j", "running", None).unwrap();
    f.state
        .store
        .publish_fields("m", &[(Channel::Elevation, &[1.; 16])], &json!({}))
        .unwrap();
    assert_eq!(
        f.state.store.get_map_node("m").unwrap().unwrap().status,
        "done"
    );
    assert_eq!(f.state.store.get_job("j").unwrap().unwrap().state, "done");
    assert!(!f.state.store.request_cancel("j").unwrap());
}
#[tokio::test]
async fn lease_cleanup_deletes_web_data_but_retains_desktop_projects() {
    for desktop in [false, true] {
        let f = Fixture::new(desktop);
        let t = f.session().await;
        let w = f.world(&t).await;
        let id = auth::digest(&t);
        let mut s = f.state.store.get_session(&id).unwrap().unwrap();
        s.last_seen = 0;
        f.state.store.put_session(&s).unwrap();
        api::expire_if_idle(&f.state, &id).await.unwrap();
        assert_eq!(f.state.store.get_world(&w).unwrap().is_some(), desktop);
        if desktop {
            api::expire_session(&f.state, &s).await.unwrap();
            assert!(f.state.store.get_world(&w).unwrap().is_some());
        }
    }
}
#[tokio::test]
async fn complete_project_round_trip_preserves_fields_and_rebinds_identity() {
    let f = Fixture::new(false);
    let t = f.session().await;
    let w = f.world(&t).await;
    let mut n = node(&w, "root");
    n.status = "done".into();
    f.state.store.put_map_node(&n).unwrap();
    for ch in [
        Channel::Elevation,
        Channel::Temperature,
        Channel::Precipitation,
        Channel::Biome,
        Channel::DistanceToSea,
    ] {
        f.state
            .store
            .put_field("root", ch, &vec![ch as u8 as f32 + 1.; 655362])
            .unwrap();
    }
    let mut child = node(&w, "child");
    child.kind = "continent".into();
    child.parent = Some("root".into());
    child.cells = 16;
    child.cell_size_m = 1000.;
    child.grid = Some(tf_grid::GridDescriptor::patch(
        40., 120., 4000., 4, 6371000.,
    ));
    child.status = "done".into();
    f.state.store.put_map_node(&child).unwrap();
    for ch in [
        Channel::Elevation,
        Channel::Temperature,
        Channel::Precipitation,
        Channel::Biome,
        Channel::DistanceToSea,
    ] {
        f.state
            .store
            .put_field("child", ch, &[ch as u8 as f32 + 2.; 16])
            .unwrap();
    }
    let bytes = project::export(&f.state.store, &w).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(&auth::digest(&t)));
    let restored = project::import(&f.state.store, &bytes, "new-owner", &f.state.config).unwrap();
    assert_ne!(restored.id, w);
    let maps = f.state.store.list_map_nodes(&restored.id).unwrap();
    assert_eq!(maps.len(), 2);
    let root = maps.iter().find(|m| m.kind == "world").unwrap();
    let sub = maps.iter().find(|m| m.kind == "continent").unwrap();
    assert_eq!(sub.parent.as_deref(), Some(root.id.as_str()));
    assert_eq!(sub.grid, child.grid);
    assert_eq!(
        f.state
            .store
            .get_field(&sub.id, Channel::Elevation)
            .unwrap()
            .unwrap(),
        vec![2.; 16]
    );
    let mut invalid: Value = serde_json::from_slice(&bytes).unwrap();
    invalid["maps"][0]["node"]["cells"] = json!(1);
    assert!(project::import(
        &f.state.store,
        &serde_json::to_vec(&invalid).unwrap(),
        "new-owner",
        &f.state.config
    )
    .is_err());
}

#[tokio::test]
async fn static_assets_and_api_work_under_the_deployment_prefix() {
    let f = Fixture::new(false);
    let mut config = (*f.state.config).clone();
    let dir = f._dir.path().join("web");
    std::fs::create_dir(&dir).unwrap();
    std::fs::create_dir(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), "<h1>Test</h1>").unwrap();
    std::fs::write(dir.join("assets/app.js"), "export{}").unwrap();
    config.static_dir = Some(dir);
    config.base_path = "/map".into();
    let mut state = f.state.clone();
    state.config = Arc::new(config);
    let app = application(&state, None).unwrap();
    for path in [
        "/",
        "/map/",
        "/map/index.html",
        "/map/assets/app.js",
        "/map/api/v1/health",
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
    let response = app
        .oneshot(Request::builder().uri("/map").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(response.headers()["location"], "/map/");
}

#[test]
fn every_starting_scope_has_a_real_independent_domain() {
    use tf_core::params::{GenerationParams, LevelKind, WorldShape};
    let narrow = GenerationParams {
        world_shape: WorldShape::Plane,
        map_height_m: 4.,
        ..Default::default()
    };
    assert!(api::root_descriptor(&narrow, 512).is_err());
    for kind in [
        LevelKind::World,
        LevelKind::Continent,
        LevelKind::Region,
        LevelKind::Site,
    ] {
        for shape in [WorldShape::Sphere, WorldShape::Plane] {
            let p = GenerationParams {
                start_level: kind,
                world_shape: shape,
                ..Default::default()
            };
            let descriptor = api::root_descriptor(&p, 32).unwrap();
            assert_eq!(
                matches!(descriptor, tf_grid::GridDescriptor::Sphere { .. }),
                kind == LevelKind::World && shape == WorldShape::Sphere
            );
            descriptor.validate().unwrap();
        }
    }
}
#[tokio::test]
async fn independent_region_plane_child_geometry_and_project_round_trip() {
    use tf_grid::GridOps;
    let f = Fixture::new(false);
    let token = f.session().await;
    let w = f.world(&token).await;
    let params = tf_core::params::GenerationParams {
        seed: 42,
        start_level: tf_core::params::LevelKind::Region,
        map_width_m: 64000.,
        map_height_m: 55000.,
        center_lat_deg: 25.,
        center_lon_deg: 179.9,
        quality: tf_core::params::SimQuality::Economy,
        ..Default::default()
    };
    let mut world = f.state.store.get_world(&w).unwrap().unwrap();
    world.params_json = serde_json::to_string(&params).unwrap();
    f.state.store.put_world(&world).unwrap();
    let descriptor = api::root_descriptor(&params, 32).unwrap();
    let mut n = node(&w, "region-root");
    n.kind = "region".into();
    n.grid = Some(descriptor.clone());
    n.cells = descriptor.cell_count() as u32;
    n.cell_size_m = params.map_width_m / 32.;
    n.generator_version = "tf-0.3".into();
    f.state
        .store
        .submit_job(&n, &job(&w, &n.id, "root-job"))
        .unwrap();
    f.state
        .store
        .transition_job("root-job", "running", None)
        .unwrap();
    let (tx, _) = tokio::sync::broadcast::channel(16);
    crate::generate::run_generation(
        &f.state.store,
        &tx,
        "root-job",
        &n.id,
        &n,
        &params,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        f.state.store.get_map_node(&n.id).unwrap().unwrap().status,
        "done"
    );
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/maps/{}/surface", n.id))
                .header("host", "test.local")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    let read = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    assert_eq!(read(0), 1);
    assert_eq!(read(4), n.cells);
    assert_eq!(
        bytes.len(),
        12 + n.cells as usize * 12 + read(8) as usize * 4
    );
    let (status,result)=f.request("POST",&format!("/api/v1/maps/{}/children",n.id),Some(&token),Some(json!({"kind":"site","center_lat":25.05,"center_lon":179.9,"width_m":16000.,"cell_size_m":500.}))).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let id = result["map_id"].as_str().unwrap();
    let jid = result["job_id"].as_str().unwrap();
    for _ in 0..500 {
        let j = f.state.store.get_job(jid).unwrap().unwrap();
        if j.state == "done" {
            break;
        }
        assert_ne!(j.state, "failed", "{:?}", j.error);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let child = f.state.store.get_map_node(id).unwrap().unwrap();
    assert_eq!(child.status, "done");
    let Some(tf_grid::GridDescriptor::Plane { reference_lat, .. }) = &child.grid else {
        panic!("child must retain planar metric")
    };
    assert_eq!(*reference_lat, Some(25.));
    let parent_grid = descriptor.build();
    let child_grid = child.grid.as_ref().unwrap().build();
    for i in 0..child.cells as usize {
        assert!(parent_grid.cell_at(child_grid.center_dir(i)).is_some());
    }
    let data = project::export(&f.state.store, &w).unwrap();
    let imported = project::import(&f.state.store, &data, "new-owner", &f.state.config).unwrap();
    let maps = f.state.store.list_map_nodes(&imported.id).unwrap();
    assert_eq!(maps.len(), 2);
    assert!(maps
        .iter()
        .any(|m| m.parent.is_none() && m.kind == "region"));
}
