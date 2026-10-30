//! Bounded, cancellation-aware in-process scheduling.
use crate::store::{Channel, JobRecord, MapNodeRecord, Store};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Mutex, Notify};
use tracing::{error, info};
/// 作业事件，广播给 SSE 订阅者。
#[derive(Debug, Clone)]
pub enum JobEvent {
    /// 状态变化。
    State { job_id: String, state: String },
    /// 进度更新。
    Progress {
        job_id: String,
        stage: String,
        progress: f64,
        eta_secs: u32,
    },
    /// 阶段切换。
    Stage {
        job_id: String,
        from: String,
        to: String,
    },
    /// 地图的一帧可视化数据。
    Frame {
        job_id: String,
        tick: u32,
        width: u32,
        height: u32,
        /// zstd 压缩后的多通道数据
        data: Vec<u8>,
        stats: FrameStats,
    },
    /// 完成。
    Done {
        job_id: String,
        map_id: String,
        duration_ms: u64,
    },
    /// 失败。
    Failed { job_id: String, error: String },
}

/// 帧的统计信息。
#[derive(Debug, Clone, Default)]
pub struct FrameStats {
    pub land_fraction: f32,
    pub max_elevation: f32,
    pub min_elevation: f32,
    pub max_temperature: f32,
    pub min_temperature: f32,
}

impl JobEvent {
    pub fn job_id(&self) -> &str {
        match self {
            Self::State { job_id, .. }
            | Self::Progress { job_id, .. }
            | Self::Stage { job_id, .. }
            | Self::Frame { job_id, .. }
            | Self::Done { job_id, .. }
            | Self::Failed { job_id, .. } => job_id,
        }
    }
}

#[derive(Clone)]
pub struct JobManager {
    inner: Arc<Inner>,
}
struct Ticket {
    world_id: String,
    cancel: Arc<AtomicBool>,
}
struct Scheduler {
    limit: usize,
    active: usize,
    tickets: std::collections::HashMap<String, Ticket>,
}
struct Inner {
    store: Store,
    state: Mutex<Scheduler>,
    changed: Notify,
    tx: broadcast::Sender<JobEvent>,
    max_queue: usize,
}

impl JobManager {
    pub fn new(store: Store, limit: usize) -> Self {
        Self::with_limits(store, limit, 16)
    }
    pub fn with_limits(store: Store, limit: usize, max_queue: usize) -> Self {
        let (tx, _) = broadcast::channel(16);
        Self {
            inner: Arc::new(Inner {
                store,
                state: Mutex::new(Scheduler {
                    limit: limit.clamp(1, 8),
                    active: 0,
                    tickets: Default::default(),
                }),
                changed: Notify::new(),
                tx,
                max_queue,
            }),
        }
    }
    pub fn recover(&self) -> anyhow::Result<()> {
        for job in self.inner.store.list_active_jobs()? {
            self.inner.store.transition_job(
                &job.id,
                "failed",
                Some("服务已重启，请重新生成".into()),
            )?;
        }
        Ok(())
    }
    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.inner.tx.subscribe()
    }
    pub async fn max_concurrent(&self) -> usize {
        self.inner.state.lock().await.limit
    }
    pub async fn counts(&self) -> (usize, usize) {
        let st = self.inner.state.lock().await;
        (st.active, st.tickets.len().saturating_sub(st.active))
    }
    pub async fn set_max_concurrent(&self, n: usize) {
        self.inner.state.lock().await.limit = n.clamp(1, 8);
        self.inner.changed.notify_waiters();
    }
    pub async fn submit(
        &self,
        map_id: String,
        world_id: String,
        node: MapNodeRecord,
        params: tf_core::params::GenerationParams,
    ) -> anyhow::Result<String> {
        let mut st = self.inner.state.lock().await;
        anyhow::ensure!(
            st.tickets.len() < self.inner.max_queue,
            "作业队列已满，请稍后重试"
        );
        anyhow::ensure!(
            st.tickets
                .values()
                .filter(|t| t.world_id == world_id)
                .count()
                < 2,
            "当前世界已有两个生成任务，请等待或取消"
        );
        let job_id = format!("job-{}", uuid::Uuid::new_v4().simple());
        let record = JobRecord {
            id: job_id.clone(),
            map_id: map_id.clone(),
            world_id: world_id.clone(),
            state: "queued".into(),
            stage: String::new(),
            progress: 0.,
            error: None,
            created_at: now_millis() as i64,
            started_at: None,
            finished_at: None,
        };
        self.inner.store.submit_job(&node, &record)?;
        let cancel = Arc::new(AtomicBool::new(false));
        st.tickets.insert(
            job_id.clone(),
            Ticket {
                world_id,
                cancel: cancel.clone(),
            },
        );
        drop(st);
        let _ = self.inner.tx.send(JobEvent::State {
            job_id: job_id.clone(),
            state: "queued".into(),
        });
        let me = self.clone();
        let id = job_id.clone();
        tokio::spawn(async move {
            me.run_job(id, map_id, node, params, cancel).await;
        });
        Ok(job_id)
    }
    pub async fn cancel(&self, id: &str) -> bool {
        let st = self.inner.state.lock().await;
        let found = if let Some(t) = st.tickets.get(id) {
            if self.inner.store.request_cancel(id).unwrap_or(false) {
                t.cancel.store(true, Ordering::Relaxed);
                true
            } else {
                false
            }
        } else {
            false
        };
        drop(st);
        self.inner.changed.notify_waiters();
        found
    }
    pub async fn delete_world(&self, id: &str) -> anyhow::Result<()> {
        let st = self.inner.state.lock().await;
        for t in st.tickets.values().filter(|t| t.world_id == id) {
            t.cancel.store(true, Ordering::Relaxed)
        }
        self.inner.store.delete_world(id)?;
        drop(st);
        self.inner.changed.notify_waiters();
        Ok(())
    }
    pub async fn cancel_world(&self, id: &str) {
        let st = self.inner.state.lock().await;
        for t in st.tickets.values().filter(|t| t.world_id == id) {
            t.cancel.store(true, Ordering::Relaxed)
        }
        drop(st);
        self.inner.changed.notify_waiters();
    }
    async fn run_job(
        &self,
        id: String,
        map_id: String,
        node: MapNodeRecord,
        params: tf_core::params::GenerationParams,
        cancel: Arc<AtomicBool>,
    ) {
        let acquired = loop {
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let mut st = self.inner.state.lock().await;
            if cancel.load(Ordering::Relaxed) {
                break false;
            }
            if st.active < st.limit {
                st.active += 1;
                break true;
            }
            drop(st);
            notified.await;
        };
        if !acquired {
            self.finish(&id, "cancelled", None);
            self.inner.state.lock().await.tickets.remove(&id);
            return;
        }
        self.finish(&id, "running", None);
        let start = std::time::Instant::now();
        let store = self.inner.store.clone();
        let tx = self.inner.tx.clone();
        let jid = id.clone();
        let mid = map_id.clone();
        let flag = cancel.clone();
        let result = tokio::task::spawn_blocking(move || {
            if node.parent.is_none() {
                crate::generate::run_generation(&store, &tx, &jid, &mid, &node, &params, &flag)
            } else {
                crate::child_gen::run_child_generation(&store, &tx, &jid, &node, &params, &flag)
            }
        })
        .await;
        if cancel.load(Ordering::Relaxed) {
            self.finish(&id, "cancelled", None)
        } else {
            match result {
                Ok(Ok(())) => {
                    self.finish(&id, "done", None);
                    let _ = self.inner.tx.send(JobEvent::Done {
                        job_id: id.clone(),
                        map_id,
                        duration_ms: start.elapsed().as_millis() as u64,
                    });
                    info!("生成完成: {id}");
                }
                other => {
                    let message = match other {
                        Ok(Err(e)) => format!("{e:#}"),
                        Err(e) => format!("生成线程失败: {e}"),
                        _ => unreachable!(),
                    };
                    error!("{id}: {message}");
                    self.finish(&id, "failed", Some(message.clone()));
                    let _ = self.inner.tx.send(JobEvent::Failed {
                        job_id: id.clone(),
                        error: message,
                    });
                }
            }
        }
        let mut st = self.inner.state.lock().await;
        st.active = st.active.saturating_sub(1);
        st.tickets.remove(&id);
        drop(st);
        self.inner.changed.notify_waiters();
    }
    fn finish(&self, id: &str, state: &str, error: Option<String>) {
        if let Err(e) = self.inner.store.transition_job(id, state, error) {
            error!("保存作业状态失败 {id}: {e}");
        }
        let _ = self.inner.tx.send(JobEvent::State {
            job_id: id.into(),
            state: state.into(),
        });
    }
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
pub fn persist_fields(
    store: &Store,
    map_id: &str,
    world: &tf_sim::GeneratedWorld,
) -> anyhow::Result<()> {
    let f = &world.fields;
    let biome: Vec<f32> = world.ecology.biome.iter().map(|&b| b as f32).collect();
    let plate: Vec<f32> = f.plate_id.iter().map(|&v| v as f32).collect();
    let soil: Vec<f32> = world.ecology.soil.iter().map(|&b| b as f32).collect();
    let mut metadata = serde_json::json!({"minerals":world.minerals,"plates":world.tectonic.plates,"planar_velocities":world.tectonic.planar_velocities,"motion_domain":if world.tectonic.planar_velocities.is_some() {"plane"} else {"sphere"},"tectonic_history":world.tectonic.history,"generator_version":"tf-0.4"});
    if let Some(atlas) = &f.climate {
        metadata["climate_atlas"] = crate::climatology::encode(atlas)?;
        metadata["climate_model"] = serde_json::json!({"name":atlas.model,"months":12,"cells":atlas.grid.cell_count(),"energy":atlas.energy,"water":atlas.water,"pet":"Thornthwaite temperature/daylength approximation"});
    }
    store.publish_fields(
        map_id,
        &[
            (Channel::Elevation, &f.elevation),
            (Channel::CrustThickness, &f.crust_thickness),
            (Channel::CrustType, &f.crust_type),
            (Channel::CrustAge, &f.crust_age),
            (Channel::Temperature, &f.temperature),
            (Channel::Precipitation, &f.precipitation),
            (Channel::FlowAccumulation, &f.flow_accumulation),
            (Channel::LakeDepth, &f.lake_depth),
            (Channel::DistanceToSea, &f.distance_to_sea),
            (Channel::Biome, &biome),
            (Channel::Soil, &soil),
            (Channel::SoilDepth, &world.ecology.soil_depth_m),
            (Channel::PlateId, &plate),
            (Channel::CrustalStrain, &f.crustal_strain),
            (Channel::OrogenAge, &f.orogen_age),
            (Channel::PotentialEvaporation, &f.potential_evaporation),
            (Channel::Aridity, &f.aridity),
        ],
        &metadata,
    )
}
