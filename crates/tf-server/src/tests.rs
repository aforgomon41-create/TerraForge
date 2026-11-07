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
#[tokio::test]
async fn regional_climate_and_return_ports_round_trip_and_reject_corrupt_routing() {
    use tf_grid::{GridDescriptor, GridOps};
    let f = Fixture::new(false);
    let token = f.session().await;
    let w = f.world(&token).await;
    let pd = GridDescriptor::plane(25., 179.9, 32000., 8, 8, 6371000.);
    let pg = pd.build();
    let pn = pg.cell_count();
    let mut parent = node(&w, "regional-climate-parent");
    parent.cells = pn as u32;
    parent.grid = Some(pd.clone());
    parent.cell_size_m = 4000.;
    parent.generator_version = "tf-0.8".into();
    parent.status = "done".into();
    f.state.store.put_map_node(&parent).unwrap();
    let climate = tf_sim::seasonal::ClimateAtlas {
        grid: pd,
        model: "saved parent climate".into(),
        axial_tilt_deg: 23.44,
        temperature_c: vec![vec![15.; pn]; 12],
        precipitation_mm: vec![vec![50.; pn]; 12],
        pet_mm: vec![vec![40.; pn]; 12],
        energy: Default::default(),
        water: Default::default(),
        circulation: Some(tf_sim::circulation::CirculationAtlas {
            model: "saved parent wind".into(),
            wind_east_m_s: (0..12)
                .map(|m| vec![if m < 6 { 2. } else { -2. }; pn])
                .collect(),
            wind_north_m_s: vec![vec![0.; pn]; 12],
            current_east_m_s: vec![vec![0.; pn]; 12],
            current_north_m_s: vec![vec![0.; pn]; 12],
            ocean_fraction: vec![0.; pn],
            relative_solver_residual: vec![0.; 12],
            solver_iterations: vec![0; 12],
            heat_coupled: false,
            ..Default::default()
        }),
    };
    let required = |n: usize| {
        vec![
            (Channel::Elevation, vec![100.; n]),
            (Channel::Temperature, vec![15.; n]),
            (Channel::Precipitation, vec![600.; n]),
            (Channel::Biome, vec![1.; n]),
            (Channel::DistanceToSea, vec![10000.; n]),
            (Channel::FlowReceiver, vec![-1.; n]),
            (Channel::LiquidDischarge, vec![1.; n]),
            (Channel::LakeRunoffLossFraction, vec![0.; n]),
        ]
    };
    let values = required(pn);
    f.state
        .store
        .publish_fields(
            &parent.id,
            &values
                .iter()
                .map(|(c, v)| (*c, v.as_slice()))
                .collect::<Vec<_>>(),
            &json!({"climate_atlas":climatology::encode(&climate).unwrap()}),
        )
        .unwrap();
    let d = GridDescriptor::plane(25., 179.9, 16000., 8, 8, 6371000.);
    let g = d.build();
    let n = g.cell_count();
    let mut fields = tf_sim::WorldFields::with_capacity(n);
    fields.elevation.fill(100.);
    fields.temperature.fill(15.);
    fields.precipitation.fill(600.);
    let regional = tf_sim::regional_climate::refine(
        &climate,
        &pg,
        &vec![100.; pn],
        None,
        &g,
        &d,
        &fields,
        1.,
        &|| false,
    )
    .unwrap();
    let mut child = parent.clone();
    child.id = "regional-climate-child".into();
    child.kind = "continent".into();
    child.parent = Some(parent.id.clone());
    child.grid = Some(d);
    child.cells = n as u32;
    child.cell_size_m = 2000.;
    f.state.store.put_map_node(&child).unwrap();
    let mut saved = regional.atlas.clone();
    saved.circulation = None;
    let mut b = tf_sim::refinement::BoundaryForcing::empty(n);
    b.monthly_available = true;
    b.outlets.push(tf_sim::refinement::Outlet {
        cell: 7,
        surface_m: 100.,
    });
    b.links.push(tf_sim::refinement::ReturnLink {
        outlet: 7,
        inlet: 15,
        water_retention: [0.8; 13],
    });
    let meta = json!({"climate_atlas":climatology::encode(&saved).unwrap(),"climate_circulation_inherited":true,"regional_climate":regional.diagnostics,crate::refinement::KEY_V2:b});
    let values = required(n);
    f.state
        .store
        .publish_fields(
            &child.id,
            &values
                .iter()
                .map(|(c, v)| (*c, v.as_slice()))
                .collect::<Vec<_>>(),
            &meta,
        )
        .unwrap();
    let recovered = climatology::for_map(&f.state.store, &child)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.model, regional.atlas.model);
    assert_eq!(recovered.precipitation_mm, regional.atlas.precipitation_mm);
    assert_eq!(
        recovered.circulation.unwrap().wind_east_m_s,
        regional.atlas.circulation.unwrap().wind_east_m_s
    );
    let bytes = project::export(&f.state.store, &w).unwrap();
    let imported = project::import(&f.state.store, &bytes, "new-owner", &f.state.config).unwrap();
    let restored = f
        .state
        .store
        .list_map_nodes(&imported.id)
        .unwrap()
        .into_iter()
        .find(|n| n.parent.is_some())
        .unwrap();
    let recovered = climatology::for_map(&f.state.store, &restored)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.model, saved.model);
    assert!(recovered.circulation.unwrap().wind_east_m_s[7][0] < 0.);
    // An ordinary receiver plus the outside reach can make a cycle even when
    // each graph alone is valid. Reject before importing any part of a project.
    let mut receivers = vec![-1.; n];
    receivers[15] = 7.;
    f.state
        .store
        .put_field(&child.id, Channel::FlowReceiver, &receivers)
        .unwrap();
    let invalid = project::export(&f.state.store, &w).unwrap();
    let error =
        project::import(&f.state.store, &invalid, "new-owner", &f.state.config).unwrap_err();
    assert!(error.to_string().contains("循环"), "{error}");
    let mut corrupt: Value = serde_json::from_slice(&bytes).unwrap();
    let m = corrupt["maps"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|m| m["node"]["parent"].is_string())
        .unwrap();
    m["metadata"]["regional_climate"]["reference_elevation_m"] = json!([0.]);
    assert!(project::import(
        &f.state.store,
        &serde_json::to_vec(&corrupt).unwrap(),
        "new-owner",
        &f.state.config
    )
    .is_err());
}
#[tokio::test]
async fn spherical_patch_rivers_use_local_metres_instead_of_unit_sphere_vectors() {
    use tf_grid::GridOps;
    let f = Fixture::new(false);
    let token = f.session().await;
    let world = f.world(&token).await;
    let mut n = node(&world, "patch-rivers");
    let d = tf_grid::GridDescriptor::patch(25., 179.9, 80000., 16, 6371000.);
    let g = d.build();
    n.grid = Some(d);
    n.kind = "continent".into();
    n.cells = g.cell_count() as u32;
    n.cell_size_m = 5000.;
    f.state.store.put_map_node(&n).unwrap();
    let mut fields = tf_sim::WorldFields::with_capacity(g.cell_count());
    for i in 0..g.cell_count() {
        let (x, y) = g.local_center_m(i).unwrap();
        fields.elevation[i] = (1000. - x * 0.01 + y * 0.00001) as f32;
        fields.precipitation[i] = 1000.;
    }
    let water = tf_sim::erosion::refresh_hydrology(&g, &mut fields);
    fields.flow_accumulation.fill(1e12);
    let receivers: Vec<_> = water
        .flow_dir
        .iter()
        .map(|&r| if r == u32::MAX { -1. } else { r as f32 })
        .collect();
    f.state
        .store
        .publish_fields(
            &n.id,
            &[
                (Channel::Elevation, &fields.elevation),
                (Channel::FlowAccumulation, &fields.flow_accumulation),
                (Channel::FlowReceiver, &receivers),
            ],
            &json!({"hydrology_model":{"name":"depression-hierarchy-annual-balance-v1"}}),
        )
        .unwrap();
    let (status, body) = f
        .request(
            "GET",
            &format!("/api/v1/maps/{}/hydrology", n.id),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["segments"].as_u64().unwrap() > 0);
    for group in body["groups"].as_array().unwrap() {
        for point in group.as_array().unwrap().chunks_exact(3) {
            let (x, y) = (point[0].as_f64().unwrap(), point[1].as_f64().unwrap());
            let i = g.locate_local(x, y).unwrap();
            let (a, b) = g.local_center_m(i).unwrap();
            assert!((x - a).hypot(y - b) < 0.01, "{x}, {y}");
        }
    }
}

#[tokio::test]
async fn refinement_plan_keeps_full_width_uses_auto_precision_and_is_private() {
    let f = Fixture::new(false);
    let token = f.session().await;
    let world = f.world(&token).await;
    let mut n = node(&world, "planning-parent");
    n.grid = Some(tf_grid::GridDescriptor::plane(
        25., 0., 1_200_000., 100, 100, 6371000.,
    ));
    n.cells = 10000;
    n.cell_size_m = 12000.;
    f.state.store.put_map_node(&n).unwrap();
    f.state
        .store
        .publish_fields(
            &n.id,
            &[(Channel::Elevation, &vec![100.; 10000])],
            &json!({}),
        )
        .unwrap();
    let path = format!(
        "/api/v1/maps/{}/refinement-plan?width_m=800000&center_lat=25&center_lon=0",
        n.id
    );
    let (status, p) = f.request("GET", &path, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{p}");
    assert_eq!(p["can_generate"], true);
    assert_eq!(p["width_m"], 800000.);
    assert_eq!(p["side"], 512);
    assert_eq!(p["cell_size_m"], 1562.5);
    let (status, p) = f
        .request(
            "GET",
            &(path.clone() + "&cell_size_m=100"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["can_generate"], false);
    assert_eq!(
        f.request(
            "GET",
            &format!("/api/v1/maps/{}/refinement-plan?width_m=-1", n.id),
            Some(&token),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let other = f.session().await;
    assert_eq!(
        f.request("GET", &path, Some(&other), None).await.0,
        StatusCode::NOT_FOUND
    );
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
        let exports = Arc::new(tokio::sync::Semaphore::new(1));
        let state = api::AppState {
            jobs: JobManager::with_memory_budget(
                store.clone(),
                1,
                4,
                (!desktop).then(|| exports.clone()),
            ),
            store,
            config: Arc::new(config),
            exports,
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
        revision: None,
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
async fn directed_evolution_repairs_a_strait_and_preserves_the_harbour_and_original() {
    use tf_grid::{GridDescriptor, GridOps};
    let f = Fixture::new(false);
    let token = f.session().await;
    let world = f.world(&token).await;
    let descriptor = GridDescriptor::plane(25., 179.9, 64_000., 64, 48, 6_371_000.);
    let grid = descriptor.build();
    let count = grid.cell_count();
    let mut original = node(&world, "strait-original");
    original.grid = Some(descriptor.clone());
    original.cells = count as u32;
    original.cell_size_m = 1000.;
    original.generator_version = "tf-0.8".into();
    f.state.store.put_map_node(&original).unwrap();
    let terrain: Vec<_> = (0..count)
        .map(|i| {
            let (x, y) = grid.local_center_m(i).unwrap();
            if x.abs() <= 4800. {
                40. + (y.abs() / 5000.) as f32
            } else {
                -100.
            }
        })
        .collect();
    let atmosphere = GridDescriptor::plane(25., 179.9, 64_000., 16, 12, 6_371_000.);
    let climate = tf_sim::seasonal::ClimateAtlas {
        grid: atmosphere.clone(),
        model: "test baseline".into(),
        axial_tilt_deg: 23.44,
        temperature_c: vec![vec![17.; atmosphere.cell_count()]; 12],
        precipitation_mm: vec![vec![55.; atmosphere.cell_count()]; 12],
        pet_mm: vec![vec![40.; atmosphere.cell_count()]; 12],
        energy: Default::default(),
        water: Default::default(),
        circulation: None,
    };
    f.state
        .store
        .publish_fields(
            &original.id,
            &[
                (Channel::Elevation, &terrain),
                (Channel::Temperature, &vec![17.; count]),
                (Channel::Precipitation, &vec![660.; count]),
                (Channel::PotentialEvaporation, &vec![480.; count]),
                (Channel::LakeDepth, &vec![0.; count]),
                (Channel::Biome, &vec![1.; count]),
                (Channel::DistanceToSea, &vec![5000.; count]),
                (Channel::CrustType, &vec![0.75; count]),
            ],
            &json!({"climate_atlas": climatology::encode(&climate).unwrap()}),
        )
        .unwrap();
    let start = grid.locate_local(-19000., 0.).unwrap();
    let end = grid.locate_local(19000., 0.).unwrap();
    let port = grid.locate_local(-3600., 0.).unwrap();
    assert!(terrain[port] > 0.);
    let uri = format!("/api/v1/maps/{}/evolution", original.id);
    let payload = |maximum_subsidence_m| {
        json!({
            "protected_areas": [{"cells": [port], "radius_m": 2600.}],
            "strait": {
                "corridor": {"cells": [start, end], "radius_m": 15500.},
                "minimum_width_m": 3800., "minimum_depth_m": 12.,
                "repair": {"maximum_subsidence_m": maximum_subsidence_m, "duration_years": 40000}
            }
        })
    };
    let wait = |id: String| {
        let store = f.state.store.clone();
        async move {
            for _ in 0..3000 {
                let j = store.get_job(&id).unwrap().unwrap();
                if matches!(j.state.as_str(), "done" | "failed" | "cancelled") {
                    return j;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            panic!("试算作业未结束");
        }
    };
    let other = f.session().await;
    assert_eq!(
        f.request("POST", &uri, Some(&other), Some(payload(140.)))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let memory_gate = f.state.exports.acquire().await.unwrap();
    let (status, queued) = f
        .request("POST", &uri, Some(&token), Some(payload(140.)))
        .await;
    assert_eq!(status, StatusCode::OK);
    let queued_id = queued["job_id"].as_str().unwrap();
    assert_eq!(
        f.request(
            "POST",
            &format!("/api/v1/jobs/{queued_id}/cancel"),
            Some(&token),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    let cancelled = wait(queued_id.into()).await;
    assert_eq!(cancelled.state, "cancelled");
    assert!(f
        .state
        .store
        .get_field(&cancelled.map_id, Channel::Elevation)
        .unwrap()
        .is_none());
    assert_eq!(
        f.request(
            "DELETE",
            &format!("/api/v1/maps/{}/evolution", cancelled.map_id),
            Some(&token),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    drop(memory_gate);
    let (status, pending) = f
        .request("POST", &uri, Some(&token), Some(payload(10.)))
        .await;
    assert_eq!(status, StatusCode::OK, "{pending}");
    let finished = wait(pending["job_id"].as_str().unwrap().into()).await;
    assert_eq!(finished.state, "done", "{:?}", finished.error);
    let draft_uri = format!("/api/v1/maps/{}/evolution", finished.map_id);
    let (_, preview) = f.request("GET", &draft_uri, Some(&token), None).await;
    assert_eq!(preview["evolution"]["report"]["can_apply"], false);
    assert_eq!(
        f.request("POST", &format!("{draft_uri}/apply"), Some(&token), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let exported = project::export(&f.state.store, &world).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&exported).unwrap()["maps"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.request("DELETE", &draft_uri, Some(&token), None).await.0,
        StatusCode::NO_CONTENT
    );
    assert!(f
        .state
        .store
        .get_map_node(&finished.map_id)
        .unwrap()
        .is_none());
    let (status, pending) = f
        .request("POST", &uri, Some(&token), Some(payload(140.)))
        .await;
    assert_eq!(status, StatusCode::OK, "{pending}");
    let finished = wait(pending["job_id"].as_str().unwrap().into()).await;
    assert_eq!(finished.state, "done", "{:?}", finished.error);
    let draft_uri = format!("/api/v1/maps/{}/evolution", finished.map_id);
    let (_, preview) = f.request("GET", &uri, Some(&token), None).await;
    let report = &preview["evolution"]["report"];
    assert_eq!(report["before"]["connected"], false, "{report}");
    assert_eq!(report["after"]["passed"], true, "{report}");
    assert!(report["after"]["width_estimate_m"].as_f64().unwrap() >= 3800.);
    assert!(report["after"]["shallowest_depth_m"].as_f64().unwrap() >= 12.);
    assert_eq!(report["flooded_protected_cells"], 0);
    assert_eq!(report["lost_protected_water_cells"], 0);
    assert_eq!(report["can_apply"], true, "{report}");
    let candidate = f
        .state
        .store
        .get_field(&finished.map_id, Channel::Elevation)
        .unwrap()
        .unwrap();
    for i in 0..count {
        if grid.distance_m(port, i) <= 2600. || grid.boundary_mask()[i] {
            assert_eq!(candidate[i], terrain[i]);
        }
    }
    assert_eq!(
        f.state
            .store
            .get_field(&original.id, Channel::Elevation)
            .unwrap()
            .unwrap(),
        terrain
    );
    assert!(f
        .state
        .store
        .get_field(&finished.map_id, Channel::FlowReceiver)
        .unwrap()
        .is_some());
    assert!(f
        .state
        .store
        .get_field(&finished.map_id, Channel::EcosystemGroup)
        .unwrap()
        .is_some());
    let (_, listed) = f
        .request(
            "GET",
            &format!("/api/v1/worlds/{world}/maps"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    let (status, saved) = f
        .request("POST", &format!("{draft_uri}/apply"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["revision"]["draft"], false);
    assert!(saved["revision"]["applied_at"].is_number());
    assert_eq!(
        f.request("POST", &format!("{draft_uri}/apply"), Some(&token), None)
            .await
            .1,
        saved
    );
    assert_eq!(
        f.request("DELETE", &draft_uri, Some(&token), None).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.request("GET", &uri, Some(&token), None).await.1,
        Value::Null
    );
    let bytes = project::export(&f.state.store, &world).unwrap();
    let restored =
        project::import(&f.state.store, &bytes, "restored-owner", &f.state.config).unwrap();
    let maps = f.state.store.list_map_nodes(&restored.id).unwrap();
    let edited = maps.iter().find(|m| m.revision.is_some()).unwrap();
    let root = maps.iter().find(|m| m.revision.is_none()).unwrap();
    assert_eq!(edited.revision.as_ref().unwrap().source_id, root.id);
    assert_eq!(
        f.state
            .store
            .get_field(&edited.id, Channel::Elevation)
            .unwrap()
            .unwrap(),
        candidate
    );
    assert_eq!(
        f.state
            .store
            .get_field(&root.id, Channel::Elevation)
            .unwrap()
            .unwrap(),
        terrain
    );
}
#[tokio::test]
async fn terminal_event_reconnects_finish_and_release_the_stream_capacity() {
    let f = Fixture::new(false);
    let token = f.session().await;
    let world = f.world(&token).await;
    for state in ["done", "failed", "cancelled"] {
        let map_id = format!("map-{state}");
        let job_id = format!("job-{state}");
        let mut map = node(&world, &map_id);
        map.cells = 16;
        f.state
            .store
            .submit_job(&map, &job(&world, &map_id, &job_id))
            .unwrap();
        f.state
            .store
            .transition_job(&job_id, "running", None)
            .unwrap();
        if state == "done" {
            f.state
                .store
                .publish_fields(&map_id, &[(Channel::Elevation, &[1.; 16])], &json!({}))
                .unwrap();
        } else {
            f.state
                .store
                .put_field(&map_id, Channel::EcosystemEvidence, &[275.; 16])
                .unwrap();
            f.state.store.transition_job(&job_id, state, None).unwrap();
            assert!(f
                .state
                .store
                .get_field(&map_id, Channel::EcosystemEvidence)
                .unwrap()
                .is_none());
        }
        // More than the global capacity: every completed body must drop its permit.
        for _ in 0..33 {
            let request = Request::builder()
                .uri(format!("/api/v1/events?job_id={job_id}"))
                .header("host", "test.local")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let response = f.app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                to_bytes(response.into_body(), 10000),
            )
            .await
            .expect("terminal SSE must close")
            .unwrap();
            assert!(String::from_utf8_lossy(&body).contains(&format!(
                "event: {}",
                if state == "failed" { "failed" } else { state }
            )));
            assert_eq!(f.state.streams.available_permits(), 32);
        }
    }
}

#[tokio::test]
async fn rendered_rivers_follow_saved_receivers_and_keep_session_isolation() {
    use tf_grid::GridOps;
    let f = Fixture::new(false);
    let token = f.session().await;
    let other = f.session().await;
    let world = f.world(&token).await;
    let descriptor = tf_grid::GridDescriptor::plane(25., 0., 32000., 32, 24, 6371000.);
    let grid = descriptor.build();
    let n = grid.cell_count();
    let mut fields = tf_sim::WorldFields::with_capacity(n);
    for i in 0..n {
        let p = grid.field_position(i);
        fields.elevation[i] = (500. + p.x.abs() * 0.02 + p.y * 0.02) as f32;
    }
    let water = tf_sim::erosion::refresh_hydrology(&grid, &mut fields);
    let receivers: Vec<f32> = water
        .flow_dir
        .iter()
        .map(|&r| if r == u32::MAX { -1. } else { r as f32 })
        .collect();
    let mut map = node(&world, "river-map");
    map.cells = n as u32;
    map.cell_size_m = 1000.;
    map.grid = Some(descriptor);
    f.state
        .store
        .submit_job(&map, &job(&world, "river-map", "river-job"))
        .unwrap();
    f.state
        .store
        .transition_job("river-job", "running", None)
        .unwrap();
    f.state
        .store
        .publish_fields(
            "river-map",
            &[
                (Channel::Elevation, &fields.elevation),
                (Channel::FlowAccumulation, &fields.flow_accumulation),
                (Channel::LakeDepth, &fields.lake_depth),
                (Channel::FlowReceiver, &receivers),
            ],
            &json!({}),
        )
        .unwrap();
    let (s, body) = f
        .request(
            "GET",
            "/api/v1/maps/river-map/hydrology",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "saved_flow_receivers");
    assert!(body["segments"].as_u64().unwrap() > 0, "{body}");
    let points = body["groups"][0].as_array().unwrap();
    assert_eq!(points.len() % 6, 0);
    for segment in points.chunks_exact(6) {
        let a = grid
            .locate_local(segment[0].as_f64().unwrap(), segment[1].as_f64().unwrap())
            .unwrap();
        let b = grid
            .locate_local(segment[3].as_f64().unwrap(), segment[4].as_f64().unwrap())
            .unwrap();
        assert_eq!(water.flow_dir[a] as usize, b);
    }
    assert_eq!(
        f.request(
            "GET",
            "/api/v1/maps/river-map/hydrology",
            Some(&other),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn circulation_months_use_saved_vectors_and_remain_private_and_bounded() {
    let f = Fixture::new(false);
    let token = f.session().await;
    let world = f.world(&token).await;
    let grid = tf_grid::GridDescriptor::sphere(8, 6371000.);
    let count = grid.cell_count();
    let mut map = node(&world, "circulation-map");
    map.grid = Some(grid.clone());
    map.cells = count as u32;
    f.state
        .store
        .submit_job(&map, &job(&world, &map.id, "circulation-job"))
        .unwrap();
    f.state
        .store
        .transition_job("circulation-job", "running", None)
        .unwrap();
    let east: Vec<Vec<f32>> = (0..12)
        .map(|m| vec![if m < 6 { 2. } else { -2. }; count])
        .collect();
    let atlas = tf_sim::seasonal::ClimateAtlas {
        grid,
        model: "fixture".into(),
        axial_tilt_deg: 23.44,
        temperature_c: vec![vec![15.; count]; 12],
        precipitation_mm: vec![vec![60.; count]; 12],
        pet_mm: vec![vec![40.; count]; 12],
        energy: Default::default(),
        water: Default::default(),
        circulation: Some(tf_sim::circulation::CirculationAtlas {
            current_velocity_scale: vec![1.; 12],
            forcing_wind_rms_difference_m_s: vec![0.; 12],
            model: "fixture".into(),
            wind_east_m_s: east.clone(),
            wind_north_m_s: vec![vec![0.; count]; 12],
            current_east_m_s: east,
            current_north_m_s: vec![vec![0.; count]; 12],
            ocean_fraction: vec![1.; count],
            relative_solver_residual: vec![0.; 12],
            solver_iterations: vec![1; 12],
            heat_coupled: true,
        }),
    };
    let encoded = climatology::encode(&atlas).unwrap();
    let restored = climatology::decode(&encoded).unwrap();
    assert_eq!(
        restored.circulation.as_ref().unwrap().wind_east_m_s[6][0],
        -2.
    );
    f.state
        .store
        .publish_fields(
            &map.id,
            &[(Channel::Elevation, &vec![-1000.; count])],
            &json!({"climate_atlas":encoded}),
        )
        .unwrap();
    let path = format!("/api/v1/maps/{}/vectors/wind", map.id);
    let (status, jan) = f
        .request("GET", &format!("{path}?month=1"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{jan}");
    assert_eq!(jan["units"], "m/s");
    assert_eq!(jan["maximum_speed_m_s"], 2.);
    let samples = jan["samples"].as_array().unwrap();
    assert!(!samples.is_empty() && samples.len() <= 700);
    assert!(samples.iter().all(|s| s["east_m_s"] == 2.));
    let (_, jul) = f
        .request("GET", &format!("{path}?month=7"), Some(&token), None)
        .await;
    assert!(jul["samples"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["east_m_s"] == -2.));
    let (_, year) = f.request("GET", &path, Some(&token), None).await;
    assert_eq!(year["maximum_speed_m_s"], 0.);
    assert!(year["samples"].as_array().unwrap().is_empty());
    let (status, _) = f
        .request("GET", &format!("{path}?month=13"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let stranger = f.session().await;
    let (status, _) = f.request("GET", &path, Some(&stranger), None).await;
    assert!(matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
    ));
    let mut legacy = map.clone();
    legacy.id = "legacy-circulation-map".into();
    f.state
        .store
        .submit_job(&legacy, &job(&world, &legacy.id, "legacy-circulation-job"))
        .unwrap();
    f.state
        .store
        .transition_job("legacy-circulation-job", "running", None)
        .unwrap();
    f.state
        .store
        .publish_fields(
            &legacy.id,
            &[(Channel::Elevation, &vec![-1000.; count])],
            &json!({}),
        )
        .unwrap();
    let path = format!("/api/v1/maps/{}/vectors/wind", legacy.id);
    let (status, old) = f.request("GET", &path, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(old["available"], false);
}

#[tokio::test]
async fn ice_sheet_rivers_are_hidden_but_thawing_permafrost_keeps_surface_rivers() {
    use tf_grid::GridOps;
    let f = Fixture::new(false);
    let token = f.session().await;
    let other = f.session().await;
    let world = f.world(&token).await;
    let descriptor = tf_grid::GridDescriptor::plane(65., 0., 32000., 32, 24, 6371000.);
    let grid = descriptor.build();
    let n = grid.cell_count();
    let mut fields = tf_sim::WorldFields::with_capacity(n);
    for i in 0..n {
        let p = grid.field_position(i);
        fields.elevation[i] = (500. + p.x.abs() * 0.02 + p.y * 0.02) as f32;
    }
    let water = tf_sim::erosion::refresh_hydrology(&grid, &mut fields);
    let receivers: Vec<f32> = water
        .flow_dir
        .iter()
        .map(|&r| if r == u32::MAX { -1. } else { r as f32 })
        .collect();
    let mut temperatures = vec![vec![-15.; n]; 12];
    temperatures[6].fill(10.);
    let annual = vec![(-15. * 11. + 10.) / 12.; n];
    let rain = vec![600.; n];
    let zeros = vec![0.; n];
    let ones = vec![1.; n];
    let thick = vec![100.; n];
    let atlas = tf_sim::seasonal::ClimateAtlas {
        grid: descriptor.clone(),
        model: "fixture".into(),
        axial_tilt_deg: 23.44,
        temperature_c: temperatures,
        precipitation_mm: vec![vec![50.; n]; 12],
        pet_mm: vec![vec![0.; n]; 12],
        energy: Default::default(),
        water: Default::default(),
        circulation: None,
    };
    let mut map = node(&world, "polar-river-map");
    map.grid = Some(descriptor);
    map.cells = n as u32;
    map.cell_size_m = 1000.;
    f.state
        .store
        .submit_job(&map, &job(&world, &map.id, "polar-river-job"))
        .unwrap();
    f.state
        .store
        .transition_job("polar-river-job", "running", None)
        .unwrap();
    f.state.store.publish_fields(&map.id,&[(Channel::Elevation,&fields.elevation),(Channel::FlowAccumulation,&fields.flow_accumulation),
        (Channel::FlowReceiver,&receivers),(Channel::LakeDepth,&fields.lake_depth),(Channel::Temperature,&annual),(Channel::Precipitation,&rain),
        (Channel::LandIce,&thick),(Channel::SnowCover,&ones),(Channel::Permafrost,&ones),(Channel::LiquidDischarge,&zeros)],
        &json!({"climate_atlas":crate::climatology::encode(&atlas).unwrap(),"hydrology_model":{"name":"depression-hierarchy-annual-balance-v1"}})).unwrap();
    let path = "/api/v1/maps/polar-river-map/hydrology";
    let (s, b) = f.request("GET", path, Some(&token), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["segments"], 0);
    f.state
        .store
        .put_field(&map.id, Channel::LandIce, &zeros)
        .unwrap();
    f.state
        .store
        .put_field(&map.id, Channel::SnowCover, &zeros)
        .unwrap();
    f.state
        .store
        .put_field(&map.id, Channel::LiquidDischarge, &vec![10.; n])
        .unwrap();
    let (_, b) = f.request("GET", path, Some(&token), None).await;
    assert!(b["segments"].as_u64().unwrap() > 0);
    assert!(b["seasonal_segments"].as_u64().unwrap() > 0);
    let (_, winter) = f
        .request("GET", &(path.to_owned() + "?month=1"), Some(&token), None)
        .await;
    assert_eq!(winter["segments"], 0);
    let (_, summer) = f
        .request("GET", &(path.to_owned() + "?month=7"), Some(&token), None)
        .await;
    assert!(summer["segments"].as_u64().unwrap() > 0);
    assert_eq!(
        f.request("GET", path, Some(&other), None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("GET", &(path.to_owned() + "?month=13"), Some(&token), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    f.state
        .store
        .put_field(&map.id, Channel::LiquidDischarge, &zeros)
        .unwrap();
    assert_eq!(
        f.request("GET", path, Some(&token), None).await.1["segments"],
        0
    );
}

#[tokio::test]
async fn saved_cryosphere_binary_is_bounded_private_and_keeps_canonical_fields() {
    let f = Fixture::new(false);
    let token = f.session().await;
    let other = f.session().await;
    let world = f.world(&token).await;
    let descriptor = tf_grid::GridDescriptor::plane(65., 0., 600000., 12, 8, 6371000.);
    let n = descriptor.cell_count();
    let atlas = tf_sim::cryosphere::CryosphereAtlas {
        model: tf_sim::cryosphere::LEGACY_MODEL.into(),
        grid: descriptor.clone(),
        spinup_years: 10000,
        gravity_m_s2: 9.81,
        grounded_ice_m: vec![0.; n],
        shelf_ice_m: vec![0.; n],
        sea_ice_m: vec![vec![1.; n]; 12],
        ice_budget: Default::default(),
    };
    let mut map = node(&world, "ice-map");
    map.grid = Some(descriptor);
    map.cells = n as u32;
    f.state
        .store
        .submit_job(&map, &job(&world, &map.id, "ice-job"))
        .unwrap();
    f.state
        .store
        .transition_job("ice-job", "running", None)
        .unwrap();
    let mut data = vec![vec![0.; n]; 8];
    data[2].fill(0.75);
    let channels: Vec<_> = crate::cryosphere::CHANNELS
        .into_iter()
        .zip(data.iter().map(Vec::as_slice))
        .collect();
    f.state
        .store
        .publish_fields(
            &map.id,
            &channels,
            &json!({"cryosphere_atlas":crate::cryosphere::encode(&atlas).unwrap()}),
        )
        .unwrap();
    let request = Request::builder()
        .uri("/api/v1/maps/ice-map/cryosphere")
        .header("host", "test.local")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let raw = to_bytes(response.into_body(), 100000).await.unwrap();
    let size = u32::from_le_bytes(raw[..4].try_into().unwrap()) as usize;
    let header: Value = serde_json::from_slice(&raw[4..4 + size]).unwrap();
    assert_eq!(header["count"], n);
    assert_eq!(raw.len(), 4 + size + n * 32);
    assert_eq!(
        f.request("GET", "/api/v1/maps/ice-map/cryosphere", Some(&other), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request(
            "GET",
            "/api/v1/maps/ice-map/cryosphere?month=13",
            Some(&token),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (_, metadata) = f
        .request("GET", "/api/v1/maps/ice-map/metadata", Some(&token), None)
        .await;
    assert!(metadata.get("cryosphere_atlas").is_none());
    let mut corrupt = crate::cryosphere::encode(&atlas).unwrap();
    corrupt["bytes"] = json!(32 * 1024 * 1024);
    assert!(crate::cryosphere::decode(&corrupt).is_err());
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
async fn ecosystem_hierarchy_is_private_preserves_u16_ids_and_deletes_channel_32() {
    let f = Fixture::new(false);
    let token = f.session().await;
    let other = f.session().await;
    let (status, world) = f
        .request(
            "POST",
            "/api/v1/worlds",
            Some(&token),
            Some(json!({"name":"Eco test","seed":42,"params":{"world_shape":"plane"}})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let w = world["id"].as_str().unwrap().to_owned();
    let descriptor = tf_grid::GridDescriptor::plane(0., 0., 4000., 4, 4, 6371000.);
    let count = descriptor.cell_count();
    let mut map = node(&w, "eco-root");
    map.cells = count as u32;
    map.cell_size_m = 1000.;
    map.grid = Some(descriptor);
    map.status = "done".into();
    map.generator_version = "tf-0.6".into();
    f.state.store.put_map_node(&map).unwrap();
    for (ch, value) in [
        (Channel::Elevation, 200.),
        (Channel::Temperature, 10.),
        (Channel::Precipitation, 900.),
        (Channel::Biome, 4.),
        (Channel::DistanceToSea, 1e5),
    ] {
        f.state
            .store
            .put_field(&map.id, ch, &vec![value; count])
            .unwrap();
    }
    let e = tf_sim::ecosystems::Ecosystems {
        biome: vec![19; count],
        group: vec![306; count],
        habitat_biome: vec![1; count],
        habitat_group: vec![19; count],
        variant: vec![21; count],
        evidence: vec![275; count],
    };
    for (ch, values) in crate::ecosystems::CHANNELS.into_iter().zip(e.channels()) {
        f.state.store.put_field(&map.id, ch, &values).unwrap();
    }
    let mut metadata = json!({});
    crate::ecosystems::attach(&mut metadata, &e);
    f.state
        .store
        .publish_fields(&map.id, &[], &metadata)
        .unwrap();
    let (status, body) = f
        .request("GET", "/api/v1/ecosystem-catalogue", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["groups"].as_array().unwrap().len(), 110);
    assert_eq!(
        f.request(
            "GET",
            "/api/v1/maps/eco-root/ecosystems",
            Some(&other),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/maps/eco-root/ecosystems")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let header = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    assert_eq!(bytes.len(), 4 + header + count * 24);
    assert_eq!(
        f32::from_le_bytes(
            bytes[4 + header + count * 4..8 + header + count * 4]
                .try_into()
                .unwrap()
        ),
        306.
    );
    let saved = project::export(&f.state.store, &w).unwrap();
    let mut pngs = Vec::new();
    for suffix in [
        "export.png?size=256",
        "export.png?size=256&ecosystem=legacy",
        "export.png?size=256&ecosystem=group",
    ] {
        let response = f
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/maps/eco-root/{suffix}"))
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        pngs.push(to_bytes(response.into_body(), 1_000_000).await.unwrap());
    }
    assert_eq!(pngs[0], pngs[1]);
    assert_ne!(pngs[0], pngs[2]);
    let imported = project::import(&f.state.store, &saved, "new-owner", &f.state.config).unwrap();
    let restored = f
        .state
        .store
        .list_map_nodes(&imported.id)
        .unwrap()
        .remove(0);
    assert_eq!(
        crate::ecosystems::load(&f.state.store, &restored.id, count)
            .unwrap()
            .unwrap()
            .group,
        e.group
    );
    f.state.store.delete_world(&w).unwrap();
    assert!(f
        .state
        .store
        .get_field(&map.id, Channel::EcosystemEvidence)
        .unwrap()
        .is_none());
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
        &Default::default(),
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
    let payload = |map_id: &str, bearer: &str| {
        Request::builder()
            .uri(format!("/api/v1/maps/{map_id}/geography"))
            .header("host", "test.local")
            .header("authorization", format!("Bearer {bearer}"))
            .body(Body::empty())
            .unwrap()
    };
    let response = f.app.clone().oneshot(payload(&n.id, &token)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let data = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    let geography = crate::geography::parse(&data, n.cells as usize).unwrap();
    assert!(geography.features.iter().any(|g| g.level == "region"));
    assert!(f
        .state
        .store
        .get_metadata(&n.id)
        .unwrap()
        .unwrap()
        .get(crate::geography::KEY)
        .is_some());
    let other = f.session().await;
    let response = f.app.clone().oneshot(payload(&n.id, &other)).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let (_, metadata) = f
        .request(
            "GET",
            &format!("/api/v1/maps/{}/metadata", n.id),
            Some(&token),
            None,
        )
        .await;
    assert!(metadata.get(crate::geography::KEY).is_none());
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
    let imported_root = maps.iter().find(|m| m.parent.is_none()).unwrap();
    assert!(f
        .state
        .store
        .get_metadata(&imported_root.id)
        .unwrap()
        .unwrap()
        .get(crate::geography::KEY)
        .is_none());
    // Imported (and legacy) maps rebuild lazily using the saved physical fields.
    let mut imported_world = f.state.store.get_world(&imported.id).unwrap().unwrap();
    imported_world.owner_token = f.state.store.get_world(&w).unwrap().unwrap().owner_token;
    f.state.store.put_world(&imported_world).unwrap();
    let response = f
        .app
        .clone()
        .oneshot(payload(&imported_root.id, &token))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let data = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    let rebuilt = crate::geography::parse(&data, imported_root.cells as usize).unwrap();
    assert_eq!(geography.landmass, rebuilt.landmass);
    assert_eq!(geography.region, rebuilt.region);
    assert_eq!(geography.site, rebuilt.site);

    // The normal child-generation path produces the boundary and climate state
    // used by a same-level edited version, including after project import.
    let point = child_grid.locate_local(0., 0.).unwrap();
    let (status, revision) = f.request("POST", &format!("/api/v1/maps/{id}/evolution"), Some(&token), Some(json!({
        "operations": [{"process":"uplift", "area":{"cells":[point], "radius_m":2500.}, "duration_years":25000, "intensity":2.}]
    }))).await;
    assert_eq!(status, StatusCode::OK, "{revision}");
    let revised_id = revision["map_id"].as_str().unwrap();
    let revision_job = revision["job_id"].as_str().unwrap();
    for _ in 0..1000 {
        let job = f.state.store.get_job(revision_job).unwrap().unwrap();
        if job.state == "done" {
            break;
        }
        assert_ne!(job.state, "failed", "{:?}", job.error);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let (status, revision) = f
        .request(
            "POST",
            &format!("/api/v1/maps/{revised_id}/evolution/apply"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{revision}");
    let old = f.state.store.get_metadata(id).unwrap().unwrap();
    let new = f.state.store.get_metadata(revised_id).unwrap().unwrap();
    assert_eq!(old[refinement::KEY_V2], new[refinement::KEY_V2]);
    assert!(new["regional_climate"].is_object());
    let old_height = f
        .state
        .store
        .get_field(id, Channel::Elevation)
        .unwrap()
        .unwrap();
    let new_height = f
        .state
        .store
        .get_field(revised_id, Channel::Elevation)
        .unwrap()
        .unwrap();
    assert!((new_height[point] - old_height[point] - 50.).abs() < 1e-3);
    let data = project::export(&f.state.store, &w).unwrap();
    let restored = project::import(&f.state.store, &data, "new-owner", &f.state.config).unwrap();
    let maps = f.state.store.list_map_nodes(&restored.id).unwrap();
    let revision = maps.iter().find(|m| m.revision.is_some()).unwrap();
    let source = maps
        .iter()
        .find(|m| m.id == revision.revision.as_ref().unwrap().source_id)
        .unwrap();
    assert_eq!(revision.parent, source.parent);
    assert_eq!(
        f.state
            .store
            .get_field(&revision.id, Channel::Elevation)
            .unwrap()
            .unwrap(),
        new_height
    );
}

#[tokio::test]
async fn automatic_seeds_and_scope_defaults_preserve_explicit_overrides() {
    let f = Fixture::new(false);
    let t = f.session().await;
    let mut random_seeds = Vec::new();
    for _ in 0..2 {
        let (s, b) = f
            .request(
                "POST",
                "/api/v1/worlds",
                Some(&t),
                Some(json!({"name":"Random region","params":{"start_level":"region"}})),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let seed = b["seed"].as_u64().unwrap();
        assert!(seed < 1_u64 << 53);
        random_seeds.push(seed);
        let world = f
            .state
            .store
            .get_world(b["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        let p: tf_core::params::GenerationParams =
            serde_json::from_str(&world.params_json).unwrap();
        assert_eq!(p.map_width_m, 200000.);
        assert_eq!(p.land_fraction, 1.);
        assert_eq!(p.seed, seed);
    }
    assert_ne!(random_seeds[0], random_seeds[1]);
    let (s, b) = f
        .request(
            "POST",
            "/api/v1/worlds",
            Some(&t),
            Some(json!({
                "name": "Fixed site",
                "seed": 0,
                "params": {
                    "start_level": "site",
                    "map_width_m": 8000.,
                    "terrain_relief_m": 40.,
                    "planet": {"radius_m": 6371000.}
                }
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["seed"], 0);
    let world = f
        .state
        .store
        .get_world(b["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    let p: tf_core::params::GenerationParams = serde_json::from_str(&world.params_json).unwrap();
    assert_eq!(p.map_width_m, 8000.);
    assert_eq!(p.terrain_relief_m, 40.);
    assert_eq!(p.map_height_m, 7500.);
    assert_eq!(p.base_elevation_m, 150.);
}

#[tokio::test]
async fn plane_coast_preference_is_saved_and_missing_values_keep_legacy_cropping() {
    let f = Fixture::new(false);
    let token = f.session().await;
    for setting in [None, Some(false), Some(true)] {
        let mut params = json!({"world_shape":"plane"});
        if let Some(value) = setting {
            params["allow_land_at_edge"] = json!(value);
        }
        let (status, body) = f
            .request(
                "POST",
                "/api/v1/worlds",
                Some(&token),
                Some(json!({"name":"Flat coast", "seed":42, "params":params})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let world = f
            .state
            .store
            .get_world(body["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        let p: tf_core::params::GenerationParams =
            serde_json::from_str(&world.params_json).unwrap();
        assert_eq!(p.world_shape, tf_core::params::WorldShape::Plane);
        assert_eq!(p.allow_land_at_edge, setting.unwrap_or(true));
    }
}

#[tokio::test]
async fn busy_import_does_not_buffer_body_and_waiting_generation_can_be_cancelled() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = Fixture::new(false);
    let token = f.session().await;
    let world = f.world(&token).await;
    let memory = f.state.exports.clone().acquire_owned().await.unwrap();
    let polled = Arc::new(AtomicBool::new(false));
    let body_polled = polled.clone();
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/projects/import")
        .header("host", "test.local")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from_stream(futures::stream::once(async move {
            body_polled.store(true, Ordering::SeqCst);
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{}"))
        })))
        .unwrap();
    assert_eq!(
        f.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert!(
        !polled.load(Ordering::SeqCst),
        "busy import read its request body"
    );
    let (status, submitted) = f
        .request(
            "POST",
            &format!("/api/v1/worlds/{world}/generate"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["job_id"].as_str().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while f.state.jobs.counts().await.0 == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.state.store.get_job(id).unwrap().unwrap().state, "queued");
    assert!(f.state.jobs.latest_frame(id).is_none());
    assert!(f.state.jobs.cancel(id).await);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while f.state.jobs.counts().await != (0, 0) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.state.store.get_job(id).unwrap().unwrap().state,
        "cancelled"
    );
    // The cancelled waiter neither stole nor leaked the data operation's permit.
    assert_eq!(f.state.exports.available_permits(), 0);
    drop(memory);
    assert_eq!(f.state.exports.available_permits(), 1);
    assert_eq!(
        f.request(
            "POST",
            "/api/v1/projects/import",
            Some(&token),
            Some(json!({}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn slow_import_body_keeps_session_admission_available_and_releases_budget_on_abort() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = Fixture::new(false);
    let token = f.session().await;
    let polled = Arc::new(AtomicBool::new(false));
    let body_polled = polled.clone();
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/projects/import")
        .header("host", "test.local")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from_stream(futures::stream::once(async move {
            body_polled.store(true, Ordering::SeqCst);
            futures::future::pending::<Result<axum::body::Bytes, std::io::Error>>().await
        })))
        .unwrap();
    let app = f.app.clone();
    let task = tokio::spawn(async move { app.oneshot(req).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !polled.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.state.exports.available_permits(), 0);
    tokio::time::timeout(std::time::Duration::from_secs(1), f.session())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(f.state.exports.available_permits(), 1);
}
