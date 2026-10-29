//! 作业系统。
//!
//! 设计要点：
//! - 单进程内队列，因为部署形态要求单二进制
//! - 并发数可配，管理界面改，立即生效
//! - 进度通过 broadcast 广播给所有 SSE 订阅者
//! - 作业可取消，用原子标志位在每个 tick 检查

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Mutex, Semaphore};
use tracing::{error, info};

use crate::store::{Channel, JobRecord, MapNodeRecord, Store};

/// 作业事件，广播给 SSE 订阅者。
#[derive(Debug, Clone)]
pub enum JobEvent {
    /// 状态变化。
    State {
        job_id: String,
        state: String,
    },
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
    Failed {
        job_id: String,
        error: String,
    },
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

/// 作业管理器。
#[derive(Clone)]
pub struct JobManager {
    inner: Arc<Inner>,
}

struct Inner {
    store: Store,
    /// 并发控制。
    semaphore: Arc<Semaphore>,
    /// 事件广播。
    tx: broadcast::Sender<JobEvent>,
    /// 当前并发上限（可动态调整）。
    max_concurrent: AtomicU64,
    /// 作业计数。
    next_id: AtomicU64,
    /// 取消标志，按 job_id 分组。
    cancel_flags: Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>,
}

impl JobManager {
    pub fn new(store: Store, max_concurrent: usize) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Inner {
                store,
                semaphore: Arc::new(Semaphore::new(max_concurrent)),
                tx,
                max_concurrent: AtomicU64::new(max_concurrent as u64),
                next_id: AtomicU64::new(1),
                cancel_flags: Mutex::new(std::collections::HashMap::new()),
            }),
        }
    }

    /// 订阅事件流。
    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.inner.tx.subscribe()
    }

    /// 当前并发设置。
    pub fn max_concurrent(&self) -> usize {
        self.inner.max_concurrent.load(Ordering::Relaxed) as usize
    }

    /// 修改并发上限。已排队的作业会在有空位时被调度。
    pub async fn set_max_concurrent(&self, n: usize) {
        let n = n.clamp(1, 8);
        let old = self.inner.max_concurrent.swap(n as u64, Ordering::Relaxed) as usize;
        if n > old {
            self.inner.semaphore.add_permits(n - old);
        }
        // 减小时不主动收回许可，等现有作业自然结束
        info!("并发上限从 {} 改为 {}", old, n);
    }

    /// 提交一个生成作业。返回 job_id。
    pub async fn submit(
        &self,
        map_id: String,
        world_id: String,
        node: MapNodeRecord,
        params: tf_core::params::GenerationParams,
    ) -> anyhow::Result<String> {
        let seq = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let job_id = format!("job-{}-{}", now_millis(), seq);

        let record = JobRecord {
            id: job_id.clone(),
            map_id: map_id.clone(),
            world_id,
            state: "queued".to_string(),
            stage: String::new(),
            progress: 0.0,
            error: None,
            created_at: now_millis() as i64,
            started_at: None,
            finished_at: None,
        };
        self.inner.store.put_job(&record)?;

        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut flags = self.inner.cancel_flags.lock().await;
            flags.insert(job_id.clone(), cancel.clone());
        }

        let _ = self.inner.tx.send(JobEvent::State {
            job_id: job_id.clone(),
            state: "queued".to_string(),
        });

        // 后台执行
        let me = self.clone();
        let jid = job_id.clone();
        tokio::spawn(async move {
            me.run_job(jid, map_id, node, params, cancel).await;
        });

        Ok(job_id)
    }

    /// 取消作业。
    pub async fn cancel(&self, job_id: &str) -> bool {
        let flags = self.inner.cancel_flags.lock().await;
        if let Some(flag) = flags.get(job_id) {
            flag.store(true, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    async fn run_job(
        &self,
        job_id: String,
        map_id: String,
        node: MapNodeRecord,
        params: tf_core::params::GenerationParams,
        cancel: Arc<AtomicBool>,
    ) {
        // 等待并发许可
        let permit = match self.inner.semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => return,
        };

        if cancel.load(Ordering::Relaxed) {
            self.finish(&job_id, "cancelled", None).await;
            drop(permit);
            return;
        }

        // 标记为运行中
        self.finish(&job_id, "running", None).await;
        let started = std::time::Instant::now();
        let _ = self.inner.tx.send(JobEvent::State {
            job_id: job_id.clone(),
            state: "running".to_string(),
        });

        // 在阻塞线程池里跑物理模拟（rayon 本来就是 CPU 密集的）
        let store = self.inner.store.clone();
        let tx = self.inner.tx.clone();
        let jid = job_id.clone();
        let mid = map_id.clone();

        let result = tokio::task::spawn_blocking(move || {
            // 世界层跑整球模拟，其余层级跑降尺度子图生成
            if node.kind == "world" {
                crate::generate::run_generation(&store, &tx, &jid, &mid, &node, &params, &cancel)
            } else {
                crate::child_gen::run_child_generation(&store, &tx, &jid, &node, &params, &cancel)
            }
        })
        .await;

        match result {
            Ok(Ok(())) => {
                let dur = started.elapsed().as_millis() as u64;
                self.finish(&job_id, "done", None).await;
                let _ = self.inner.tx.send(JobEvent::Done {
                    job_id: job_id.clone(),
                    map_id: map_id.clone(),
                    duration_ms: dur,
                });
                info!("作业 {} 完成, 耗时 {} ms", job_id, dur);
            }
            Ok(Err(e)) => {
                let msg = format!("{:#}", e);
                error!("作业 {} 失败: {}", job_id, msg);
                self.finish(&job_id, "failed", Some(msg.clone())).await;
                let _ = self.inner.tx.send(JobEvent::Failed {
                    job_id: job_id.clone(),
                    error: msg,
                });
            }
            Err(e) => {
                let msg = format!("任务 panic: {}", e);
                error!("{}", msg);
                self.finish(&job_id, "failed", Some(msg.clone())).await;
                let _ = self.inner.tx.send(JobEvent::Failed {
                    job_id: job_id.clone(),
                    error: msg,
                });
            }
        }

        {
            let mut flags = self.inner.cancel_flags.lock().await;
            flags.remove(&job_id);
        }
        drop(permit);
    }

    async fn finish(&self, job_id: &str, state: &str, error: Option<String>) {
        if let Ok(Some(mut rec)) = self.inner.store.get_job(job_id) {
            rec.state = state.to_string();
            rec.error = error;
            if state != "queued" && rec.started_at.is_none() {
                rec.started_at = Some(now_millis() as i64);
            }
            if matches!(state, "done" | "failed" | "cancelled") {
                rec.finished_at = Some(now_millis() as i64);
                rec.progress = if state == "done" { 1.0 } else { rec.progress };
            }
            let _ = self.inner.store.put_job(&rec);
        }
    }
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 把世界场写进存储。
pub fn persist_fields(
    store: &Store,
    map_id: &str,
    world: &tf_sim::GeneratedWorld,
) -> anyhow::Result<()> {
    let f = &world.fields;
    store.put_field(map_id, Channel::Elevation, &f.elevation)?;
    store.put_field(map_id, Channel::CrustThickness, &f.crust_thickness)?;
    store.put_field(map_id, Channel::CrustType, &f.crust_type)?;
    store.put_field(map_id, Channel::CrustAge, &f.crust_age)?;
    store.put_field(map_id, Channel::Temperature, &f.temperature)?;
    store.put_field(map_id, Channel::Precipitation, &f.precipitation)?;
    store.put_field(map_id, Channel::FlowAccumulation, &f.flow_accumulation)?;
    store.put_field(map_id, Channel::LakeDepth, &f.lake_depth)?;
    store.put_field(map_id, Channel::DistanceToSea, &f.distance_to_sea)?;

    // 生态结果转 f32 存储
    let biome: Vec<f32> = world.ecology.biome.iter().map(|&b| b as f32).collect();
    let soil: Vec<f32> = world.ecology.soil.iter().map(|&s| s as f32).collect();
    store.put_field(map_id, Channel::Biome, &biome)?;
    store.put_field(map_id, Channel::Soil, &soil)?;
    store.put_field(map_id, Channel::SoilDepth, &world.ecology.soil_depth_m)?;

    Ok(())
}