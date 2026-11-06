//! Bounded refinement plans and persisted boundary contracts.
use crate::{
    api::{ApiError, AppState},
    store::MapNodeRecord,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tf_grid::{GridDescriptor, GridOps, MapGrid};
pub const KEY: &str = "refinement_boundary_v1";
pub const KEY_V2: &str = "refinement_boundary_v2";
/// Even padding preserves the row parity and all output cell centres exactly.
/// A crop at its finite parent's edge gets a smaller halo, never invented data.
pub fn buffered(desc: &GridDescriptor, parent: &MapGrid) -> (GridDescriptor, u32) {
    let side = match desc {
        GridDescriptor::Plane { size, .. } | GridDescriptor::Patch { size, .. } => *size,
        _ => return (desc.clone(), 0),
    };
    let spacing = match desc {
        GridDescriptor::Plane { width_m, .. } | GridDescriptor::Patch { width_m, .. } => {
            *width_m / side as f64
        }
        _ => unreachable!(),
    };
    let maximum = ((side / 16).clamp(4, 24) / 2) * 2;
    for pad in (2..=maximum).rev().filter(|p| p % 2 == 0) {
        let mut d = desc.clone();
        match &mut d {
            GridDescriptor::Plane {
                width_m,
                size,
                rows,
                ..
            } => {
                *size += pad * 2;
                *rows += pad * 2;
                *width_m = *size as f64 * spacing;
            }
            GridDescriptor::Patch { width_m, size, .. } => {
                *size += pad * 2;
                *width_m = *size as f64 * spacing;
            }
            _ => {}
        }
        if d.validate().is_err() {
            continue;
        }
        if parent.extent_m().is_some() {
            let g = d.build();
            if g.boundary_mask()
                .iter_ones()
                .any(|i| parent.cell_at(g.center_dir(i)).is_none())
            {
                continue;
            }
        }
        return (d, pad);
    }
    (desc.clone(), 0)
}
pub fn crop<T: Copy>(v: &[T], side: usize, pad: usize) -> Vec<T> {
    let stride = side + 2 * pad;
    (0..side)
        .flat_map(|r| {
            v[(r + pad) * stride + pad..(r + pad) * stride + pad + side]
                .iter()
                .copied()
        })
        .collect()
}
pub fn decode(
    metadata: &Value,
    grid: &MapGrid,
) -> anyhow::Result<Option<tf_sim::refinement::BoundaryForcing>> {
    anyhow::ensure!(
        !(metadata.get(KEY).is_some() && metadata.get(KEY_V2).is_some()),
        "局部边界版本重复"
    );
    let Some(value) = metadata.get(KEY_V2).or_else(|| metadata.get(KEY)) else {
        return Ok(None);
    };
    let b: tf_sim::refinement::BoundaryForcing = serde_json::from_value(value.clone())?;
    b.validate(grid.cell_count()).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        (metadata.get(KEY_V2).is_some() && b.version == 2)
            || (metadata.get(KEY).is_some() && b.version == 1),
        "局部边界编码版本不符"
    );
    tf_sim::refinement::BoundaryGrid::new(grid, &b).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        b.inlets
            .iter()
            .all(|p| grid.boundary_mask()[p.cell as usize]),
        "入水口不在地图边界"
    );
    anyhow::ensure!(
        b.links
            .iter()
            .all(|p| grid.boundary_mask()[p.outlet as usize]
                && grid.boundary_mask()[p.inlet as usize]),
        "跨界河流端口不在地图边缘"
    );
    Ok(Some(b))
}
#[derive(Deserialize, Default)]
pub struct PlanQuery {
    pub feature_id: Option<u32>,
    pub center_lat: Option<f64>,
    pub center_lon: Option<f64>,
    pub width_m: Option<f64>,
    pub cell_size_m: Option<f64>,
}
#[derive(Serialize)]
pub struct Plan {
    pub kind: String,
    pub feature_id: Option<u32>,
    pub feature_name: Option<String>,
    pub center_lat: f64,
    pub center_lon: f64,
    pub width_m: f64,
    pub cell_size_m: f64,
    pub side: u32,
    pub cells: u32,
    pub can_generate: bool,
    pub complete_object: bool,
    pub message: String,
    pub maximum_width_m: f64,
}
pub fn plan(
    node: &MapNodeRecord,
    geography: Option<&tf_sim::geography::Geography>,
    q: &PlanQuery,
    limit: u32,
) -> Result<Plan, String> {
    let kind = match node.kind.as_str() {
        "world" => "continent",
        "continent" => "region",
        "region" => "site",
        _ => return Err("地点层不能继续细化".into()),
    };
    let preferred = match kind {
        "continent" => 1000_f64,
        "region" => 100.,
        _ => 1.,
    };
    let parent = node.grid.as_ref().ok_or("旧工程缺少地图几何")?;
    let (lat, lon) = match parent {
        GridDescriptor::Sphere { .. } => (0., 0.),
        GridDescriptor::Plane {
            center_lat,
            center_lon,
            ..
        }
        | GridDescriptor::Patch {
            center_lat,
            center_lon,
            ..
        } => (*center_lat, *center_lon),
    };
    let mut result = Plan {
        kind: kind.into(),
        feature_id: None,
        feature_name: None,
        center_lat: q.center_lat.unwrap_or(lat),
        center_lon: q.center_lon.unwrap_or(lon),
        width_m: q.width_m.unwrap_or(preferred * 256.),
        cell_size_m: preferred,
        side: 0,
        cells: 0,
        can_generate: true,
        complete_object: false,
        message: String::new(),
        maximum_width_m: (node.cell_size_m * limit as f64 * (1. - 1e-6)).min(
            if matches!(
                parent,
                GridDescriptor::Sphere { .. } | GridDescriptor::Patch { .. }
            ) {
                parent.radius_m() * 2.4
            } else {
                f64::MAX
            },
        ),
    };
    if !result.center_lat.is_finite()
        || !result.center_lon.is_finite()
        || result.center_lat.abs() > 90.
        || result.center_lon.abs() > 180.
    {
        return Err("经纬度无效".into());
    }
    let grid = parent.build();
    if let Some(id) = q.feature_id {
        let g = geography.ok_or("地理对象尚未识别")?;
        let f = g
            .features
            .iter()
            .find(|f| f.id == id)
            .ok_or("地理对象不存在")?;
        if f.level != kind {
            return Err("请选择与下一层对应的地理对象，或使用自定范围".into());
        }
        result.feature_id = Some(id);
        result.feature_name = Some(f.name.clone());
        result.center_lat = f.lat;
        result.center_lon = f.lon;
        let center = tf_core::geo::UnitVec3::from_lat_lon_deg(f.lat, f.lon);
        let (east, north) = center.tangent_frame();
        let labels = match kind {
            "continent" => &g.landmass,
            "region" => &g.region,
            _ => &g.site,
        };
        let mut width = 0_f64;
        let mut behind = false;
        let projected_plane = if let MapGrid::Plane(p) = &grid {
            Some(tf_grid::FlatGrid::with_reference_lat(
                4,
                4,
                4.,
                f.lat,
                f.lon,
                p.radius,
                p.reference_lat,
            ))
        } else {
            None
        };
        for (i, &label) in labels.iter().enumerate() {
            if label != id && i != f.anchor_cell as usize {
                continue;
            }
            let d = grid.center_dir(i);
            let (x, y) = if let Some(p) = &projected_plane {
                p.project(d)
            } else {
                let den = d.as_vec().dot(center.as_vec());
                if den <= 0. {
                    behind = true;
                    continue;
                }
                (
                    (d.as_vec().dot(east) / den).atan() * grid.radius_m(),
                    (d.as_vec().dot(north) / den).atan() * grid.radius_m(),
                )
            };
            width = width.max(x.abs() * 2.).max(y.abs() * 4. / 3_f64.sqrt());
        }
        result.width_m = (width + node.cell_size_m * 2.).max(preferred * 4.);
        result.complete_object = !f.partial;
        if f.partial {
            result.message = "该对象在父图边缘已被截断，仅能细化当前保存的部分。".into();
        }
        if behind {
            result.can_generate = false;
            result.message = "对象跨越局部投影背面，请分区细化；不会截断对象。".into();
        }
    }
    if !result.width_m.is_finite() || result.width_m < 4. {
        return Err("覆盖宽度无效".into());
    }
    let min_spacing = preferred.max(result.width_m / limit as f64);
    let requested = q.cell_size_m.unwrap_or(min_spacing);
    if !requested.is_finite() || requested < 1. {
        return Err("计算格距应至少为1米".into());
    }
    let side = (result.width_m / requested - 1e-9).ceil();
    result.side = side.clamp(4., limit as f64) as u32;
    result.cells = result.side * result.side;
    result.cell_size_m = result.width_m / result.side as f64;
    if side > limit as f64
        || result.cell_size_m >= node.cell_size_m
        || result.width_m > result.maximum_width_m
    {
        result.can_generate = false;
        result.message="当前预算无法按指定精度完整覆盖。请缩小局部范围或选择自动精度；不会悄悄裁切或降低指定精度。".into();
    }
    let desc = match parent {
        GridDescriptor::Plane {
            reference_lat,
            center_lat,
            ..
        } => GridDescriptor::Plane {
            center_lat: result.center_lat,
            center_lon: result.center_lon,
            width_m: result.width_m,
            size: result.side,
            rows: result.side,
            radius_m: parent.radius_m(),
            version: tf_grid::descriptor::PLANE_GRID_VERSION,
            reference_lat: Some(reference_lat.unwrap_or(*center_lat)),
        },
        _ => GridDescriptor::patch(
            result.center_lat,
            result.center_lon,
            result.width_m,
            result.side,
            parent.radius_m(),
        ),
    };
    if let Err(e) = desc.validate() {
        result.can_generate = false;
        result.message = e;
    } else if grid.extent_m().is_some() {
        let child = desc.build();
        if child
            .boundary_mask()
            .iter_ones()
            .any(|i| grid.cell_at(child.center_dir(i)).is_none())
        {
            result.can_generate = false;
            result.message = "完整范围超出父图，需返回上一级或选择较小范围；不会截断。".into();
        }
    }
    if result.message.is_empty() {
        result.message = if result.cell_size_m > preferred * 1.01 {
            "完整概览使用与覆盖范围匹配的采样，之后可选择局部继续细化。".into()
        } else {
            "按本次计算格距生成；显示网格可单独调整。".into()
        };
    }
    Ok(result)
}
pub async fn get_plan(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PlanQuery>,
) -> Result<Json<Plan>, ApiError> {
    let node = st
        .store
        .get_map_node(&id)?
        .ok_or_else(|| ApiError::not_found("父图不存在"))?;
    if node.status != "done" {
        return Err(ApiError::bad_request("父图尚未完成"));
    }
    let permit = st
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("范围资料正在读取"))?;
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Result<Plan, String>> {
        let _permit = permit;
        let g = if q.feature_id.is_some() {
            Some(crate::geography::for_map(&st.store, &node)?)
        } else {
            None
        };
        Ok(plan(&node, g.as_ref(), &q, st.config.max_patch_side()))
    })
    .await??;
    Ok(Json(result.map_err(ApiError::bad_request)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buffered_crop_preserves_centres_and_areas_for_plane_and_patch() {
        for desc in [
            GridDescriptor::plane(25., 179.9, 80000., 64, 64, 6371000.),
            GridDescriptor::patch(80., 179.9, 80000., 64, 6371000.),
        ] {
            let parent = match desc {
                GridDescriptor::Plane { .. } => {
                    GridDescriptor::plane(25., 179.9, 400000., 64, 64, 6371000.).build()
                }
                _ => GridDescriptor::patch(80., 179.9, 400000., 64, 6371000.).build(),
            };
            let (work, pad) = buffered(&desc, &parent);
            assert!(pad > 0 && pad % 2 == 0);
            let g = desc.build();
            let w = work.build();
            let ids = crop(&(0..w.cell_count()).collect::<Vec<_>>(), 64, pad as usize);
            for (i, &j) in ids.iter().enumerate() {
                assert!(g.center_dir(i).as_vec().distance(w.center_dir(j).as_vec()) < 1e-14);
                assert!((g.area_m2(i) - w.area_m2(j)).abs() < g.area_m2(i) * 1e-10);
            }
        }
    }
}
