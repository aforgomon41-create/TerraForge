//! Portable projects preserve grid descriptors, seeds, the map tree and all channels.
use crate::{
    config::ServerConfig,
    jobs::now_millis,
    store::{Channel, MapNodeRecord, Store, WorldRecord},
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Read,
};

const FORMAT: &str = "terraforge-project-v2";
#[derive(Serialize, Deserialize)]
struct Field {
    encoding: String,
    count: usize,
    data: String,
}
#[derive(Serialize, Deserialize)]
struct Map<M> {
    node: MapNodeRecord,
    channels: BTreeMap<String, Field>,
    metadata: M,
}
#[derive(Serialize, Deserialize)]
struct Project<M> {
    format: String,
    generator_version: String,
    world: WorldRecord,
    maps: Vec<Map<M>>,
}
pub type ImportedMap = (MapNodeRecord, Vec<(Channel, Vec<u8>)>, serde_json::Value);
pub fn export(store: &Store, world_id: &str) -> anyhow::Result<Vec<u8>> {
    let mut world = store
        .get_world(world_id)?
        .ok_or_else(|| anyhow::anyhow!("世界已被删除"))?;
    world.owner_token.clear();
    world.share_token = None;
    let mut maps = Vec::new();
    for node in store.list_map_nodes(world_id)? {
        if node.status != "done" {
            continue;
        }
        anyhow::ensure!(
            node.grid.is_some(),
            "旧版地图缺少坐标描述，请重新生成后保存"
        );
        let mut channels = BTreeMap::new();
        for ch in Channel::ALL {
            if let Some(data) = store.compressed_field(&node.id, ch)? {
                channels.insert(
                    ch.name().into(),
                    Field {
                        encoding: "f32le+zstd+base64".into(),
                        count: node.cells as usize,
                        data: B64.encode(data),
                    },
                );
            }
        }
        maps.push(Map {
            metadata: store
                .get_metadata(&node.id)?
                .unwrap_or(serde_json::Value::Null),
            node,
            channels,
        });
    }
    anyhow::ensure!(!maps.is_empty(), "请先完成一张地图再保存工程");
    let ids: HashSet<_> = maps.iter().map(|m| m.node.id.as_str()).collect();
    anyhow::ensure!(
        maps.iter()
            .all(|m| m.node.parent.as_deref().map_or(true, |p| ids.contains(p))),
        "父图缺失，无法保存完整工程"
    );
    Ok(serde_json::to_vec(&Project {
        format: FORMAT.into(),
        generator_version: "tf-0.3".into(),
        world,
        maps,
    })?)
}
pub fn import(
    store: &Store,
    bytes: &[u8],
    owner: &str,
    config: &ServerConfig,
) -> anyhow::Result<WorldRecord> {
    let project: Project<Box<serde_json::value::RawValue>> = serde_json::from_slice(bytes)?;
    let metadata_bytes: usize = project.maps.iter().map(|m| m.metadata.get().len()).sum();
    anyhow::ensure!(
        metadata_bytes
            <= if config.desktop {
                32 * 1024 * 1024
            } else {
                4 * 1024 * 1024
            },
        "地质元数据超过当前模式限制"
    );
    anyhow::ensure!(
        project.format == FORMAT
            && matches!(project.generator_version.as_str(), "tf-0.2" | "tf-0.3"),
        "不支持的工程版本"
    );
    let limit = if config.desktop { 256 } else { 16 };
    anyhow::ensure!(
        !project.maps.is_empty() && project.maps.len() <= limit,
        "地图数量超出限制"
    );
    let mut params: tf_core::params::GenerationParams =
        serde_json::from_str(&project.world.params_json)?;
    params.validate().map_err(anyhow::Error::msg)?;
    let max_frequency = if config.desktop { 768 } else { 256 };
    let max_patch = if config.desktop {
        2048
    } else {
        config.max_patch_side()
    };
    let total: u64 = project.maps.iter().map(|m| m.node.cells as u64).sum();
    anyhow::ensure!(
        config.desktop || total <= 4_000_000,
        "工程总格数超过网页版限制，请在桌面版打开"
    );
    let mut ids = HashMap::new();
    let mut kinds = HashMap::new();
    for m in &project.maps {
        anyhow::ensure!(!ids.contains_key(&m.node.id), "工程有重复地图 ID");
        ids.insert(
            m.node.id.clone(),
            format!("map-{}", uuid::Uuid::new_v4().simple()),
        );
        kinds.insert(m.node.id.clone(), m.node.kind.clone());
    }
    let mut world = project.world;
    world.id = format!("world-{}", uuid::Uuid::new_v4().simple());
    world.owner_token = owner.into();
    world.share_token = None;
    world.created_at = now_millis() as i64;
    world.updated_at = world.created_at;
    params.seed = world.seed;
    world.params_json = serde_json::to_string(&params)?;
    anyhow::ensure!(
        !world.name.trim().is_empty() && world.name.chars().count() <= 80,
        "世界名称无效"
    );
    let parent_nodes: HashMap<_, _> = project
        .maps
        .iter()
        .map(|m| (m.node.id.clone(), m.node.clone()))
        .collect();
    let mut maps = Vec::new();
    for m in project.maps {
        let mut node = m.node;
        let descriptor = node
            .grid
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("地图缺少网格描述"))?;
        descriptor.validate().map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            (descriptor.radius_m() - params.planet.radius_m).abs() < 0.001,
            "地图半径与工程参数不一致"
        );
        anyhow::ensure!(
            node.cells as usize == descriptor.cell_count(),
            "网格与格数不匹配"
        );
        anyhow::ensure!(
            matches!(node.generator_version.as_str(), "tf-0.2" | "tf-0.3") && node.status == "done",
            "地图版本或状态无效"
        );
        anyhow::ensure!(node.name.chars().count() <= 80, "地图名称过长");
        match descriptor {
            tf_grid::GridDescriptor::Plane { size, rows, .. } => {
                anyhow::ensure!(*size <= max_patch && *rows <= max_patch, "平面网格精度超限");
                if node.parent.is_none() {
                    anyhow::ensure!(
                        node.kind
                            == match params.start_level {
                                tf_core::params::LevelKind::World => "world",
                                tf_core::params::LevelKind::Continent => "continent",
                                tf_core::params::LevelKind::Region => "region",
                                tf_core::params::LevelKind::Site => "site",
                            },
                        "独立地图类型与工程参数不一致"
                    );
                }
            }
            tf_grid::GridDescriptor::Sphere { frequency, .. } => anyhow::ensure!(
                node.kind == "world" && node.parent.is_none() && *frequency <= max_frequency,
                "世界网格精度或父级无效"
            ),
            tf_grid::GridDescriptor::Patch { size, width_m, .. } => {
                anyhow::ensure!(*size <= max_patch, "局部网格超过当前精度限制");
                let expected = match node.kind.as_str() {
                    "continent" => ("world", 1000.),
                    "region" => ("continent", 100.),
                    "site" => ("region", 1.),
                    _ => anyhow::bail!("地图层级无效"),
                };
                let parent = node
                    .parent
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("局部图缺少父图"))?;
                anyhow::ensure!(
                    kinds.get(parent).map(String::as_str) == Some(expected.0),
                    "地图父子关系无效"
                );
                anyhow::ensure!(width_m / *size as f64 >= 1., "局部图格宽无效");
            }
        }
        if let Some(parent_id) = &node.parent {
            let parent = parent_nodes
                .get(parent_id)
                .ok_or_else(|| anyhow::anyhow!("地图父图不存在"))?;
            let expected = match parent.kind.as_str() {
                "world" => "continent",
                "continent" => "region",
                "region" => "site",
                _ => anyhow::bail!("地图父子关系无效"),
            };
            anyhow::ensure!(
                node.kind == expected
                    && node.cell_size_m >= 1.
                    && node.cell_size_m < parent.cell_size_m,
                "子图层级或格距无效"
            );
            match (parent.grid.as_ref(), descriptor) {
                (
                    Some(tf_grid::GridDescriptor::Plane {
                        center_lat,
                        reference_lat,
                        ..
                    }),
                    tf_grid::GridDescriptor::Plane {
                        center_lat: child_lat,
                        reference_lat: child_ref,
                        ..
                    },
                ) => anyhow::ensure!(
                    (reference_lat.unwrap_or(*center_lat) - child_ref.unwrap_or(*child_lat)).abs()
                        < 1e-8,
                    "平面子图坐标基准不一致"
                ),
                (
                    Some(
                        tf_grid::GridDescriptor::Sphere { .. }
                        | tf_grid::GridDescriptor::Patch { .. },
                    ),
                    tf_grid::GridDescriptor::Patch { .. },
                ) => {}
                _ => anyhow::bail!("父子图的空间形状不一致"),
            }
        }
        if node.generator_version == "tf-0.3" {
            let spacing = match descriptor {
                tf_grid::GridDescriptor::Plane { width_m, size, .. }
                | tf_grid::GridDescriptor::Patch { width_m, size, .. } => *width_m / *size as f64,
                tf_grid::GridDescriptor::Sphere { radius_m, .. } => {
                    (4. * std::f64::consts::PI * radius_m.powi(2) / descriptor.cell_count() as f64)
                        .sqrt()
                }
            };
            anyhow::ensure!(
                node.cell_size_m.is_finite() && (node.cell_size_m - spacing).abs() < spacing * 1e-6,
                "地图采样间距与几何不一致"
            );
        }
        let mut channels = Vec::new();
        for (name, field) in m.channels {
            let ch = Channel::parse(&name).ok_or_else(|| anyhow::anyhow!("未知数据通道"))?;
            anyhow::ensure!(
                field.encoding == "f32le+zstd+base64" && field.count == node.cells as usize,
                "数据编码或长度无效"
            );
            let compressed = B64.decode(&field.data)?;
            let mut decoder = zstd::stream::read::Decoder::new(compressed.as_slice())?;
            decoder.window_log_max(26)?;
            let mut data = Vec::new();
            decoder
                .take(field.count as u64 * 4 + 1)
                .read_to_end(&mut data)?;
            anyhow::ensure!(
                data.len() == field.count * 4
                    && data
                        .chunks_exact(4)
                        .all(|b| f32::from_le_bytes(b.try_into().unwrap()).is_finite()),
                "通道数据损坏或含无效数值"
            );
            channels.push((ch, compressed));
        }
        for required in [
            Channel::Elevation,
            Channel::Temperature,
            Channel::Precipitation,
            Channel::Biome,
            Channel::DistanceToSea,
        ] {
            anyhow::ensure!(
                channels.iter().any(|(ch, _)| *ch == required),
                "工程缺少必要通道 {}",
                required.name()
            );
        }
        node.extent_json = serde_json::to_string(descriptor)?;
        node.id = ids[&node.id].clone();
        node.parent = node.parent.map(|p| ids[&p].clone());
        node.world_id = world.id.clone();
        let mut metadata: serde_json::Value = serde_json::from_str(m.metadata.get())?;
        if let Some(obj) = metadata.as_object_mut() {
            if let Some(parent) = &node.parent {
                obj.insert("parent".into(), serde_json::json!(parent));
            }
        }
        maps.push((node, channels, metadata));
    }
    store.import_project(&world, &maps)?;
    Ok(world)
}
