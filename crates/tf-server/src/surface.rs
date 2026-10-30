//! Continuous terrain geometry and independent cartographic grid lines.
use crate::api::{ApiError, AppState};
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use tf_grid::{GridDescriptor, GridOps, MapGrid};

pub async fn get_surface(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let node = st
        .store
        .get_map_node(&id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    if node.status != "done" {
        return Err(ApiError::bad_request("地图尚未完成"));
    }
    let descriptor = node
        .grid
        .ok_or_else(|| ApiError::bad_request("旧地图缺少几何信息"))?;
    let permit = st
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("几何数据正在准备，请稍后重试"))?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let grid = descriptor.build();
        let triangles = tf_grid::mesh::triangles(&grid);
        let mut bytes = Vec::with_capacity(12 + grid.cell_count() * 12 + triangles.len() * 4);
        for value in [1, grid.cell_count() as u32, triangles.len() as u32] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for i in 0..grid.cell_count() {
            for value in grid.render_center(i) {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        for value in triangles {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    })
    .await?;
    Ok(([("content-type", "application/octet-stream")], bytes))
}

#[derive(Deserialize)]
pub struct GridQuery {
    pub spacing_m: f64,
}
pub async fn get_overlay(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<GridQuery>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    if !query.spacing_m.is_finite() || query.spacing_m < 0. || query.spacing_m > 1e8 {
        return Err(ApiError::bad_request("显示格距无效"));
    }
    let node = st
        .store
        .get_map_node(&id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    let descriptor = node
        .grid
        .ok_or_else(|| ApiError::bad_request("地图缺少几何信息"))?;
    let permit = st
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("请等待另一项几何数据操作"))?;
    let (bytes, spacing) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if query.spacing_m == 0. {
            return (Vec::new(), 0.);
        }
        let coarse = match descriptor {
            GridDescriptor::Sphere { radius_m, .. } => {
                let f = ((4. * std::f64::consts::PI / 10.).sqrt() * radius_m / query.spacing_m)
                    .round()
                    .clamp(4., 96.) as u32;
                MapGrid::Sphere(tf_grid::SphereHexGrid::build(
                    f,
                    radius_m,
                    (glam::DVec3::Z, 0.),
                ))
            }
            GridDescriptor::Patch {
                center_lat,
                center_lon,
                width_m,
                radius_m,
                ..
            } => {
                let side = (width_m / query.spacing_m).round().clamp(4., 256.) as u32;
                GridDescriptor::patch(center_lat, center_lon, width_m, side, radius_m).build()
            }
            GridDescriptor::Plane {
                center_lat,
                center_lon,
                width_m,
                size,
                rows,
                radius_m,
                reference_lat,
                ..
            } => {
                let min_cols = (4. * size as f64 / rows as f64).ceil().max(4.);
                let cols = (width_m / query.spacing_m)
                    .round()
                    .max(min_cols)
                    .min(256_f64.max(min_cols)) as u32;
                let height_rows = (rows as f64 / size as f64 * cols as f64)
                    .floor()
                    .clamp(4., 256.) as u32;
                GridDescriptor::Plane {
                    center_lat,
                    center_lon,
                    width_m,
                    size: cols,
                    rows: height_rows,
                    radius_m,
                    reference_lat,
                    version: tf_grid::descriptor::PLANE_GRID_VERSION,
                }
                .build()
            }
        };
        let spacing = match coarse.extent_m() {
            Some((width, _)) => match &coarse {
                MapGrid::Patch(g) => width / g.size() as f64,
                MapGrid::Plane(g) => width / g.cols as f64,
                _ => unreachable!(),
            },
            None => (4. * std::f64::consts::PI * coarse.radius_m().powi(2)
                / coarse.cell_count() as f64)
                .sqrt(),
        };
        let lines = tf_grid::mesh::dual_edges(&coarse);
        let mut bytes = Vec::with_capacity(lines.len() * 24);
        for edge in lines {
            for point in edge {
                for value in point.to_array() {
                    bytes.extend_from_slice(&(value as f32).to_le_bytes());
                }
            }
        }
        (bytes, spacing)
    })
    .await?;
    Ok((
        [
            ("content-type", "application/octet-stream".to_string()),
            ("x-tf-grid-spacing-m", spacing.to_string()),
        ],
        bytes,
    ))
}
