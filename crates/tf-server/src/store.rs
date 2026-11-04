//! 存储层。
//!
//! 用 redb 做嵌入式存储。表的设计原则：
//! - 世界与地图节点分开存，因为地图树可能很深
//! - 场数据按「世界 + 地图 + 通道」分块存，支持按需读取
//! - 快照单独存，用于回放生成过程

use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
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
    #[serde(default)]
    pub name: String,
    pub parent: Option<String>,
    pub cells: u32,
    pub cell_size_m: f64,
    /// 覆盖范围描述（JSON）。
    pub extent_json: String,
    /// 生成状态：pending / running / done / failed
    pub status: String,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub grid: Option<tf_grid::GridDescriptor>,
    #[serde(default)]
    pub seed: u64,
    #[serde(default)]
    pub generator_version: String,
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
const SESSIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("sessions");

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
    PlateId = 12,
    CrustalStrain = 13,
    OrogenAge = 14,
    PotentialEvaporation = 15,
    Aridity = 16,
    FlowReceiver = 17,
    LandIce = 18,
    SnowCover = 19,
    SeaIce = 20,
    IceShelf = 21,
    Permafrost = 22,
    ActiveLayer = 23,
    LiquidWater = 24,
    LakeIce = 25,
    LiquidDischarge = 26,
    EcosystemBiome = 27,
    EcosystemGroup = 28,
    EcosystemHabitatBiome = 29,
    EcosystemHabitatGroup = 30,
    EcosystemVariant = 31,
    EcosystemEvidence = 32,
}

impl Channel {
    pub const ALL: [Self; 33] = [
        Self::Elevation,
        Self::CrustThickness,
        Self::CrustType,
        Self::CrustAge,
        Self::Temperature,
        Self::Precipitation,
        Self::FlowAccumulation,
        Self::LakeDepth,
        Self::DistanceToSea,
        Self::Biome,
        Self::Soil,
        Self::SoilDepth,
        Self::PlateId,
        Self::CrustalStrain,
        Self::OrogenAge,
        Self::PotentialEvaporation,
        Self::Aridity,
        Self::FlowReceiver,
        Self::LandIce,
        Self::SnowCover,
        Self::SeaIce,
        Self::IceShelf,
        Self::Permafrost,
        Self::ActiveLayer,
        Self::LiquidWater,
        Self::LakeIce,
        Self::LiquidDischarge,
        Self::EcosystemBiome,
        Self::EcosystemGroup,
        Self::EcosystemHabitatBiome,
        Self::EcosystemHabitatGroup,
        Self::EcosystemVariant,
        Self::EcosystemEvidence,
    ];
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ch| ch.name() == name)
    }
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
            Channel::PlateId => "plate_id",
            Channel::CrustalStrain => "crustal_strain",
            Channel::OrogenAge => "orogen_age",
            Channel::PotentialEvaporation => "potential_evaporation",
            Channel::Aridity => "aridity",
            Channel::FlowReceiver => "flow_receiver",
            Channel::LandIce => "land_ice_m",
            Channel::SnowCover => "snow_cover",
            Channel::SeaIce => "sea_ice_cover",
            Channel::IceShelf => "ice_shelf_m",
            Channel::Permafrost => "permafrost",
            Channel::ActiveLayer => "active_layer_m",
            Channel::LiquidWater => "liquid_water_mm",
            Channel::LakeIce => "lake_ice_cover",
            Channel::LiquidDischarge => "liquid_discharge_m3_s",
            Channel::EcosystemBiome => "ecosystem_biome",
            Channel::EcosystemGroup => "ecosystem_group",
            Channel::EcosystemHabitatBiome => "ecosystem_habitat_biome",
            Channel::EcosystemHabitatGroup => "ecosystem_habitat_group",
            Channel::EcosystemVariant => "ecosystem_variant",
            Channel::EcosystemEvidence => "ecosystem_evidence",
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
        // redb defaults to a 1 GiB cache, larger than the hosted service budget.
        let db = Database::builder()
            .set_cache_size(64 * 1024 * 1024)
            .create(path)?;
        // 确保所有表都存在
        let write = db.begin_write()?;
        {
            write.open_table(WORLDS)?;
            write.open_table(MAP_NODES)?;
            write.open_table(JOBS)?;
            write.open_table(FIELDS)?;
            write.open_table(META)?;
            write.open_table(SESSIONS)?;
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

    pub fn world_count(&self) -> anyhow::Result<usize> {
        let read = self.db.begin_read()?;
        let t = read.open_table(WORLDS)?;
        Ok(t.len()? as usize)
    }

    pub fn delete_world(&self, id: &str) -> anyhow::Result<()> {
        let write = self.db.begin_write()?;
        let map_ids: Vec<String> = {
            let t = write.open_table(MAP_NODES)?;
            let mut ids = Vec::new();
            for entry in t.iter()? {
                let (key, value) = entry?;
                let node: MapNodeRecord = serde_json::from_slice(value.value())?;
                if node.world_id == id {
                    ids.push(key.value().to_owned());
                }
            }
            ids
        };
        {
            let mut t = write.open_table(FIELDS)?;
            for map_id in &map_ids {
                for ch in 0u8..=u8::MAX {
                    t.remove((map_id.as_str(), ch))?;
                }
            }
        }
        {
            let mut t = write.open_table(META)?;
            for map_id in &map_ids {
                t.remove(map_id.as_str())?;
            }
        }
        {
            let mut t = write.open_table(MAP_NODES)?;
            for map_id in &map_ids {
                t.remove(map_id.as_str())?;
            }
        }
        let job_ids: Vec<String> = {
            let t = write.open_table(JOBS)?;
            let mut ids = Vec::new();
            for entry in t.iter()? {
                let (key, value) = entry?;
                let job: JobRecord = serde_json::from_slice(value.value())?;
                if job.world_id == id {
                    ids.push(key.value().to_owned());
                }
            }
            ids
        };
        {
            let mut t = write.open_table(JOBS)?;
            for jid in job_ids {
                t.remove(jid.as_str())?;
            }
        }
        {
            let mut t = write.open_table(WORLDS)?;
            t.remove(id)?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn put_session(&self, s: &crate::auth::SessionRecord) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(s)?;
        let write = self.db.begin_write()?;
        {
            write
                .open_table(SESSIONS)?
                .insert(s.id.as_str(), bytes.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn touch_session(&self, id: &str, now: u64) -> anyhow::Result<bool> {
        let write = self.db.begin_write()?;
        let mut session: crate::auth::SessionRecord = {
            let t = write.open_table(SESSIONS)?;
            let Some(value) = t.get(id)? else {
                return Ok(false);
            };
            serde_json::from_slice(value.value())?
        };
        session.last_seen = session.last_seen.max(now);
        {
            let bytes = serde_json::to_vec(&session)?;
            write.open_table(SESSIONS)?.insert(id, bytes.as_slice())?;
        }
        write.commit()?;
        Ok(true)
    }

    pub fn get_session(&self, id: &str) -> anyhow::Result<Option<crate::auth::SessionRecord>> {
        let read = self.db.begin_read()?;
        let t = read.open_table(SESSIONS)?;
        t.get(id)?
            .map(|v| serde_json::from_slice(v.value()).map_err(Into::into))
            .transpose()
    }

    pub fn sessions(&self) -> anyhow::Result<Vec<crate::auth::SessionRecord>> {
        let read = self.db.begin_read()?;
        let t = read.open_table(SESSIONS)?;
        t.iter()?
            .map(|r| {
                let (_, v) = r?;
                Ok(serde_json::from_slice(v.value())?)
            })
            .collect()
    }

    pub fn delete_session(&self, id: &str) -> anyhow::Result<()> {
        let write = self.db.begin_write()?;
        {
            write.open_table(SESSIONS)?.remove(id)?;
        }
        write.commit()?;
        Ok(())
    }

    /// The node and queued job appear together, or not at all.
    pub fn submit_job(&self, node: &MapNodeRecord, job: &JobRecord) -> anyhow::Result<()> {
        let write = self.db.begin_write()?;
        anyhow::ensure!(
            write
                .open_table(WORLDS)?
                .get(node.world_id.as_str())?
                .is_some(),
            "世界已被删除"
        );
        if let Some(parent) = &node.parent {
            anyhow::ensure!(
                write.open_table(MAP_NODES)?.get(parent.as_str())?.is_some(),
                "父图已被删除"
            );
        }
        let nb = serde_json::to_vec(node)?;
        let jb = serde_json::to_vec(job)?;
        {
            write
                .open_table(MAP_NODES)?
                .insert(node.id.as_str(), nb.as_slice())?;
        }
        {
            write
                .open_table(JOBS)?
                .insert(job.id.as_str(), jb.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    /// Publish all channels atomically. A concurrent delete prevents publication.
    pub fn publish_fields(
        &self,
        map_id: &str,
        fields: &[(Channel, &[f32])],
        metadata: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let encoded: Vec<_> = fields
            .iter()
            .map(|(ch, data)| {
                let bytes: Vec<_> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
                Ok((*ch, zstd::encode_all(bytes.as_slice(), 3)?))
            })
            .collect::<anyhow::Result<_>>()?;
        let write = self.db.begin_write()?;
        let mut node: MapNodeRecord = {
            let t = write.open_table(MAP_NODES)?;
            let v = t
                .get(map_id)?
                .ok_or_else(|| anyhow::anyhow!("地图已被删除"))?;
            serde_json::from_slice(v.value())?
        };
        anyhow::ensure!(
            write
                .open_table(WORLDS)?
                .get(node.world_id.as_str())?
                .is_some(),
            "世界已被删除"
        );
        anyhow::ensure!(
            fields
                .iter()
                .all(|(_, data)| data.len() == node.cells as usize
                    && data.iter().all(|v| v.is_finite())),
            "字段尺寸或数值无效"
        );
        let active_job: Option<JobRecord> = {
            let t = write.open_table(JOBS)?;
            let mut found = None;
            for item in t.iter()? {
                let (_, value) = item?;
                let job: JobRecord = serde_json::from_slice(value.value())?;
                if job.map_id == map_id {
                    anyhow::ensure!(job.state == "running", "作业已取消或结束");
                    found = Some(job);
                    break;
                }
            }
            found
        };
        {
            let mut t = write.open_table(FIELDS)?;
            for (ch, bytes) in encoded {
                t.insert((map_id, ch as u8), bytes.as_slice())?;
            }
        }
        {
            let bytes = serde_json::to_vec(metadata)?;
            write.open_table(META)?.insert(map_id, bytes.as_slice())?;
        }
        let now = crate::jobs::now_millis() as i64;
        node.status = "done".into();
        node.completed_at = Some(now);
        {
            let bytes = serde_json::to_vec(&node)?;
            write
                .open_table(MAP_NODES)?
                .insert(map_id, bytes.as_slice())?;
        }
        if let Some(mut job) = active_job {
            job.state = "done".into();
            job.progress = 1.;
            job.finished_at = Some(now);
            let bytes = serde_json::to_vec(&job)?;
            write
                .open_table(JOBS)?
                .insert(job.id.as_str(), bytes.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn get_metadata(&self, map_id: &str) -> anyhow::Result<Option<serde_json::Value>> {
        let read = self.db.begin_read()?;
        let t = read.open_table(META)?;
        t.get(map_id)?
            .map(|v| serde_json::from_slice(v.value()).map_err(Into::into))
            .transpose()
    }

    /// Cache only derived geography; never resurrect a concurrently deleted map.
    pub fn cache_geography(&self, map_id: &str, value: &serde_json::Value) -> anyhow::Result<()> {
        let write = self.db.begin_write()?;
        let node: MapNodeRecord = {
            let table = write.open_table(MAP_NODES)?;
            let record = table
                .get(map_id)?
                .ok_or_else(|| anyhow::anyhow!("地图已删除"))?;
            serde_json::from_slice(record.value())?
        };
        anyhow::ensure!(
            node.status == "done"
                && write
                    .open_table(WORLDS)?
                    .get(node.world_id.as_str())?
                    .is_some(),
            "地图已删除或尚未完成"
        );
        {
            let mut table = write.open_table(META)?;
            let mut metadata: serde_json::Value = table
                .get(map_id)?
                .map(|v| serde_json::from_slice(v.value()))
                .transpose()?
                .unwrap_or_else(|| serde_json::json!({}));
            if !metadata.is_object() {
                metadata = serde_json::json!({});
            }
            metadata[crate::geography::KEY] = value.clone();
            metadata.as_object_mut().unwrap().remove("geography_error");
            let bytes = serde_json::to_vec(&metadata)?;
            table.insert(map_id, bytes.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn transition_job(
        &self,
        id: &str,
        state: &str,
        error: Option<String>,
    ) -> anyhow::Result<()> {
        let write = self.db.begin_write()?;
        let mut job: JobRecord = {
            let t = write.open_table(JOBS)?;
            let Some(v) = t.get(id)? else { return Ok(()) };
            serde_json::from_slice(v.value())?
        };
        if matches!(job.state.as_str(), "done" | "failed" | "cancelled") {
            return Ok(());
        }
        if job.state == "cancelling" && state == "running" {
            return Ok(());
        }
        job.state = state.into();
        job.error = error;
        if state == "running" && job.started_at.is_none() {
            job.started_at = Some(crate::jobs::now_millis() as i64);
        }
        if matches!(state, "done" | "failed" | "cancelled") {
            job.finished_at = Some(crate::jobs::now_millis() as i64);
        }
        if state == "done" {
            job.progress = 1.0;
        }
        let node = {
            let t = write.open_table(MAP_NODES)?;
            let v = t.get(job.map_id.as_str())?;
            v.map(|v| serde_json::from_slice::<MapNodeRecord>(v.value()))
                .transpose()?
        };
        if let Some(mut node) = node {
            node.status = state.into();
            node.completed_at = job.finished_at;
            let bytes = serde_json::to_vec(&node)?;
            {
                write
                    .open_table(MAP_NODES)?
                    .insert(node.id.as_str(), bytes.as_slice())?;
            }
        }
        if matches!(state, "failed" | "cancelled") {
            {
                write.open_table(META)?.remove(job.map_id.as_str())?;
            }
            let mut fields = write.open_table(FIELDS)?;
            for ch in 0u8..=u8::MAX {
                fields.remove((job.map_id.as_str(), ch))?;
            }
        }
        {
            let bytes = serde_json::to_vec(&job)?;
            write.open_table(JOBS)?.insert(id, bytes.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn update_progress(&self, id: &str, stage: &str, progress: f64) -> anyhow::Result<()> {
        let write = self.db.begin_write()?;
        let mut job: JobRecord = {
            let t = write.open_table(JOBS)?;
            let Some(value) = t.get(id)? else {
                return Ok(());
            };
            serde_json::from_slice(value.value())?
        };
        if job.state != "running" {
            return Ok(());
        }
        job.stage = stage.into();
        job.progress = progress.clamp(0., 0.99);
        {
            let bytes = serde_json::to_vec(&job)?;
            write.open_table(JOBS)?.insert(id, bytes.as_slice())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn request_cancel(&self, id: &str) -> anyhow::Result<bool> {
        let write = self.db.begin_write()?;
        let mut job: JobRecord = {
            let t = write.open_table(JOBS)?;
            let Some(value) = t.get(id)? else {
                return Ok(false);
            };
            serde_json::from_slice(value.value())?
        };
        if !matches!(job.state.as_str(), "queued" | "running") {
            return Ok(false);
        }
        job.state = "cancelling".into();
        {
            let bytes = serde_json::to_vec(&job)?;
            write.open_table(JOBS)?.insert(id, bytes.as_slice())?;
        }
        write.commit()?;
        Ok(true)
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

    pub fn compressed_field(&self, map_id: &str, ch: Channel) -> anyhow::Result<Option<Vec<u8>>> {
        let read = self.db.begin_read()?;
        let t = read.open_table(FIELDS)?;
        Ok(t.get((map_id, ch as u8))?.map(|v| v.value().to_vec()))
    }
    pub fn import_project(
        &self,
        world: &WorldRecord,
        maps: &[crate::project::ImportedMap],
    ) -> anyhow::Result<()> {
        let write = self.db.begin_write()?;
        {
            let bytes = serde_json::to_vec(world)?;
            write
                .open_table(WORLDS)?
                .insert(world.id.as_str(), bytes.as_slice())?;
        }
        for (node, channels, metadata) in maps {
            {
                let bytes = serde_json::to_vec(node)?;
                write
                    .open_table(MAP_NODES)?
                    .insert(node.id.as_str(), bytes.as_slice())?;
            }
            {
                let mut table = write.open_table(FIELDS)?;
                for (ch, bytes) in channels {
                    table.insert((node.id.as_str(), *ch as u8), bytes.as_slice())?;
                }
            }
            {
                let bytes = serde_json::to_vec(metadata)?;
                write
                    .open_table(META)?
                    .insert(node.id.as_str(), bytes.as_slice())?;
            }
        }
        write.commit()?;
        Ok(())
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
            if matches!(
                j.state.as_str(),
                "queued" | "running" | "paused" | "cancelling"
            ) {
                out.push(j);
            }
        }
        Ok(out)
    }
}
