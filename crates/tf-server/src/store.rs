//! 存储层。
//!
//! 用 redb 做嵌入式存储。表的设计原则：
//! - 世界与地图节点分开存，因为地图树可能很深
//! - 场数据按「世界 + 地图 + 通道」分块存，支持按需读取
//! - 快照单独存，用于回放生成过程

use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// 世界记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldRecord {
    pub id: String,
    pub name: String,
    pub seed: u64,
    /// 序列化的 GenerationParams。
    pub params_json: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// 创建者的客户端令牌哈希。
    pub owner_token: String,
    /// 分享令牌，非空表示可公开只读访问。
    pub share_token: Option<String>,
}

/// 地图节点记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapNodeRecord {
    pub id: String,
    pub world_id: String,
    /// 层级：world / continent / region / site
    pub kind: String,
    pub parent: Option<String>,
    pub cells: u32,
    pub cell_size_m: f64,
    /// 覆盖范围描述（JSON）。
    pub extent_json: String,
    /// 生成状态：pending / running / done / failed
    pub status: String,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

/// 一个生成作业。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub map_id: String,
    pub world_id: String,
    /// 状态：queued / running / paused / done / failed / cancelled
    pub state: String,
    /// 当前阶段名。
    pub stage: String,
    /// 0-1 进度。
    pub progress: f64,
    pub error: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

// ---- 表定义 ----

const WORLDS: TableDefinition<&str, &[u8]> = TableDefinition::new("worlds");
const MAP_NODES: TableDefinition<&str, &[u8]> = TableDefinition::new("map_nodes");
const JOBS: TableDefinition<&str, &[u8]> = TableDefinition::new("jobs");
/// 场数据：key 是 (map_id, channel)，value 是 zstd 压缩后的 f32 数组
const FIELDS: TableDefinition<(&str, u8), &[u8]> = TableDefinition::new("fields");
/// 元数据：参数、统计、时间轴快照索引
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

/// 通道编号。每个编号对应一个物理场。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Channel {
    Elevation = 0,
    CrustThickness = 1,
    CrustType = 2,
    CrustAge = 3,
    Temperature = 4,
    Precipitation = 5,
    FlowAccumulation = 6,
    LakeDepth = 7,
    DistanceToSea = 8,
    Biome = 9,
    Soil = 10,
    SoilDepth = 11,
}

impl Channel {
    pub fn name(&self) -> &'static str {
        match self {
            Channel::Elevation => "elevation",
            Channel::CrustThickness => "crust_thickness",
            Channel::CrustType => "crust_type",
            Channel::CrustAge => "crust_age",
            Channel::Temperature => "temperature",
            Channel::Precipitation => "precipitation",
            Channel::FlowAccumulation => "flow_accumulation",
            Channel::LakeDepth => "lake_depth",
            Channel::DistanceToSea => "distance_to_sea",
            Channel::Biome => "biome",
            Channel::Soil => "soil",
            Channel::SoilDepth => "soil_depth",
        }
    }
}

/// 存储句柄。
#[derive(Clone)]
pub struct Store {
    db: Arc<Database>,
}

impl Store {
    /// 打开或创建数据库。
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let db = Database::create(path)?;
        // 确保所有表都存在
        let write = db.begin_write()?;
        {
            write.open_table(WORLDS)?;
            write.open_table(MAP_NODES)?;
            write.open_table(JOBS)?;
            write.open_table(FIELDS)?;
            write.open_table(META)?;
        }
        write.commit()?;
        Ok(Self { db: Arc::new(db) })
    }

    // ---- 世界 ----

    pub fn put_world(&self, w: &WorldRecord) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(w)?;
        let write = self.db.begin_write()?;
        {
            let mut t = write.open_table(WORLDS)?;
            t.insert(w.id.as_str(), bytes.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn get_world(&self, id: &str) -> anyhow::Result<Option<WorldRecord>> {
        let read = self.db.begin_read()?;
        let t = read.open_table(WORLDS)?;
        match t.get(id)? {
            Some(v) => Ok(Some(serde_json::from_slice(v.value())?)),
            None => Ok(None),
        }
    }

    pub fn list_worlds(&self, owner: &str) -> anyhow::Result<Vec<WorldRecord>> {
        let read = self.db.begin_read()?;
        let t = read.open_table(WORLDS)?;
        let mut out = Vec::new();
        for entry in t.iter()? {
            let (_, v) = entry?;
            let w: WorldRecord = serde_json::from_slice(v.value())?;
            if w.owner_token == owner {
                out.push(w);
            }
        }
        out.sort_by_key(|w| std::cmp::Reverse(w.updated_at));
        Ok(out)
    }

    pub fn delete_world(&self, id: &str) -> anyhow::Result<()> {
        let write = self.db.begin_write()?;
        {
            let mut t = write.open_table(WORLDS)?;
            t.remove(id)?;
        }
        write.commit()?;
        Ok(())
    }

    // ---- 地图节点 ----

    pub fn put_map_node(&self, m: &MapNodeRecord) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(m)?;
        let write = self.db.begin_write()?;
        {
            let mut t = write.open_table(MAP_NODES)?;
            t.insert(m.id.as_str(), bytes.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn get_map_node(&self, id: &str) -> anyhow::Result<Option<MapNodeRecord>> {
        let read = self.db.begin_read()?;
        let t = read.open_table(MAP_NODES)?;
        match t.get(id)? {
            Some(v) => Ok(Some(serde_json::from_slice(v.value())?)),
            None => Ok(None),
        }
    }

    pub fn list_map_nodes(&self, world_id: &str) -> anyhow::Result<Vec<MapNodeRecord>> {
        let read = self.db.begin_read()?;
        let t = read.open_table(MAP_NODES)?;
        let mut out = Vec::new();
        for entry in t.iter()? {
            let (_, v) = entry?;
            let m: MapNodeRecord = serde_json::from_slice(v.value())?;
            if m.world_id == world_id {
                out.push(m);
            }
        }
        Ok(out)
    }

    // ---- 场数据 ----

    /// 存一个通道。用 zstd 压缩。
    pub fn put_field(&self, map_id: &str, ch: Channel, data: &[f32]) -> anyhow::Result<()> {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for v in data {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let compressed = zstd::encode_all(&bytes[..], 3)?;
        let write = self.db.begin_write()?;
        {
            let mut t = write.open_table(FIELDS)?;
            t.insert((map_id, ch as u8), compressed.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn get_field(&self, map_id: &str, ch: Channel) -> anyhow::Result<Option<Vec<f32>>> {
        let read = self.db.begin_read()?;
        let t = read.open_table(FIELDS)?;
        match t.get((map_id, ch as u8))? {
            Some(v) => {
                let bytes = zstd::decode_all(v.value())?;
                let mut out = Vec::with_capacity(bytes.len() / 4);
                for chunk in bytes.chunks_exact(4) {
                    out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                }
                Ok(Some(out))
            }
            None => Ok(None),
        }
    }

    // ---- 作业 ----

    pub fn put_job(&self, j: &JobRecord) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(j)?;
        let write = self.db.begin_write()?;
        {
            let mut t = write.open_table(JOBS)?;
            t.insert(j.id.as_str(), bytes.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn get_job(&self, id: &str) -> anyhow::Result<Option<JobRecord>> {
        let read = self.db.begin_read()?;
        let t = read.open_table(JOBS)?;
        match t.get(id)? {
            Some(v) => Ok(Some(serde_json::from_slice(v.value())?)),
            None => Ok(None),
        }
    }

    pub fn list_active_jobs(&self) -> anyhow::Result<Vec<JobRecord>> {
        let read = self.db.begin_read()?;
        let t = read.open_table(JOBS)?;
        let mut out = Vec::new();
        for entry in t.iter()? {
            let (_, v) = entry?;
            let j: JobRecord = serde_json::from_slice(v.value())?;
            if matches!(j.state.as_str(), "queued" | "running" | "paused") {
                out.push(j);
            }
        }
        Ok(out)
    }
}