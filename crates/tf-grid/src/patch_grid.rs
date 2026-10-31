//! 球面局部区域的平面网格。
//!
//! 用于大陆/区域/地点三层：这些是球面上的一块区域，
//! 用切平面投影展开成规则六角格网格。

use bitvec::prelude::BitVec;
use glam::DVec3;
use serde::{Deserialize, Serialize};
use tf_core::geo::UnitVec3;

/// 球面局部区域。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SphericalPatch {
    /// 区域中心的单位方向。
    pub center: [f64; 3],
    /// 覆盖宽度（米），按大圆距离算。
    pub width_m: f64,
    /// 网格边长（格数）。
    pub size: u32,
    pub radius_m: f64,
}

impl SphericalPatch {
    pub fn new(center_lat: f64, center_lon: f64, width_m: f64, size: u32) -> Self {
        Self::with_radius(center_lat, center_lon, width_m, size, 6_371_000.0)
    }

    pub fn with_radius(
        center_lat: f64,
        center_lon: f64,
        width_m: f64,
        size: u32,
        radius_m: f64,
    ) -> Self {
        let c = UnitVec3::from_lat_lon_deg(center_lat, center_lon);
        Self {
            center: [c.as_vec().x, c.as_vec().y, c.as_vec().z],
            width_m,
            size,
            radius_m,
        }
    }

    pub fn center_vec(&self) -> UnitVec3 {
        UnitVec3::from_normalized(DVec3::new(self.center[0], self.center[1], self.center[2]))
    }

    pub fn cell_size_m(&self) -> f64 {
        self.width_m / self.size as f64
    }
}

/// 一块区域的平面六角格网格。
///
/// 用切平面（gnomonic）投影把球面的局部区域展开到平面，
/// 建立规则六角格。因为区域相对星球很小，投影畸变可以忽略。
#[derive(Debug, Clone)]
pub struct PatchGrid {
    size: u32,
    cell_size_m: f64,
    radius_m: f64,
    /// 区域中心方向。
    center: UnitVec3,
    /// 切平面基（东、北）。
    east: DVec3,
    north: DVec3,
    centers: Vec<UnitVec3>,
    lat_deg: Vec<f64>,
    areas: Vec<f64>,
    offsets: Vec<u32>,
    neighbors: Vec<u32>,
    boundary: BitVec,
}

impl PatchGrid {
    pub fn new(patch: &SphericalPatch) -> Self {
        assert!(patch.size > 0 && patch.width_m > 0. && patch.radius_m > 0.);
        let n = (patch.size * patch.size) as usize;
        let center = patch.center_vec();
        let (east, north) = center.tangent_frame();
        let cell_size = patch.cell_size_m();

        let mut centers = Vec::with_capacity(n);
        let mut lat_deg = Vec::with_capacity(n);
        let mut areas = Vec::with_capacity(n);

        // 六角格面积
        let hex_size = cell_size / (3.0_f64).sqrt();
        let area = (3.0 * (3.0_f64).sqrt() / 2.0) * hex_size * hex_size;

        let half = (patch.size as f64 - 1.0) / 2.0;

        for row in 0..patch.size {
            let row_shift = if row % 2 == 0 { 0.0 } else { 0.5 };
            for col in 0..patch.size {
                // 平面坐标（米）
                let px = ((col as f64 + row_shift) - half - 0.25) * cell_size;
                let py = (half - row as f64) * cell_size * (3.0_f64).sqrt() / 2.0;

                // 切平面投影回球面：
                // 从中心沿 east/north 走，用 gnomonic 投影的逆变换
                let angular_x = px / patch.radius_m;
                let angular_y = py / patch.radius_m;

                // 小角度近似：先走 east 再走 north
                let dir = (center.as_vec() + east * angular_x.tan() + north * angular_y.tan())
                    .normalize();

                let u = UnitVec3::from_normalized(dir);
                let (lat, _lon) = u.to_lat_lon_deg();

                centers.push(u);
                lat_deg.push(lat);
                areas.push(area);
            }
        }

        // 建邻接（与 plane_hex 相同的行奇偶规则）
        let mut offsets = Vec::with_capacity(n + 1);
        let mut neighbors = Vec::with_capacity(n * 6);
        offsets.push(0u32);

        for row in 0..patch.size as i32 {
            for col in 0..patch.size as i32 {
                let deltas: [(i32, i32); 6] = if row % 2 == 0 {
                    [(0, -1), (0, 1), (-1, -1), (-1, 0), (1, -1), (1, 0)]
                } else {
                    [(0, -1), (0, 1), (-1, 0), (-1, 1), (1, 0), (1, 1)]
                };
                for (dr, dc) in deltas {
                    let nr = row + dr;
                    let nc = col + dc;
                    if nr >= 0 && nr < patch.size as i32 && nc >= 0 && nc < patch.size as i32 {
                        neighbors.push((nr as u32) * patch.size + nc as u32);
                    }
                }
                offsets.push(neighbors.len() as u32);
            }
        }

        let mut boundary = BitVec::with_capacity(n);
        for i in 0..n {
            let cnt = (offsets[i + 1] - offsets[i]) as usize;
            boundary.push(cnt < 6);
        }

        Self {
            size: patch.size,
            cell_size_m: cell_size,
            radius_m: patch.radius_m,
            center,
            east,
            north,
            centers,
            lat_deg,
            areas,
            offsets,
            neighbors,
            boundary,
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn cell_size_m(&self) -> f64 {
        self.cell_size_m
    }

    /// 格子的 (行, 列)。
    pub fn row_col(&self, i: usize) -> (u32, u32) {
        (i as u32 / self.size, i as u32 % self.size)
    }

    /// 把球面方向投影到本区域的归一化平面坐标 [-1, 1]。
    pub fn project(&self, dir: UnitVec3) -> (f64, f64) {
        let v = dir.as_vec();
        // 转到以 center 为原点的切平面
        let dx = v.dot(self.east) / v.dot(self.center.as_vec());
        let dy = v.dot(self.north) / v.dot(self.center.as_vec());
        (dx.atan(), dy.atan())
    }

    pub fn local_center(&self, i: usize) -> (f64, f64) {
        let (row, col) = self.row_col(i);
        let half = (self.size as f64 - 1.) / 2.;
        (
            ((col as f64 + (row % 2) as f64 * 0.5) - half - 0.25) * self.cell_size_m,
            (half - row as f64) * self.cell_size_m * (3.0_f64).sqrt() / 2.,
        )
    }

    pub fn cell_at(&self, dir: UnitVec3) -> Option<usize> {
        if dir.as_vec().dot(self.center.as_vec()) <= 0. {
            return None;
        }
        let (ax, ay) = self.project(dir);
        let (x, y) = (ax * self.radius_m, ay * self.radius_m);
        self.cell_at_local(x, y)
    }

    pub fn cell_at_local(&self, x: f64, y: f64) -> Option<usize> {
        let half = (self.size as f64 - 1.) / 2.;
        let rf = half - y / (self.cell_size_m * (3.0_f64).sqrt() / 2.);
        if rf < -0.5 || rf > self.size as f64 - 0.5 {
            return None;
        }
        let mut best = None;
        let mut best_d = f64::INFINITY;
        for row in (rf.round() as i32 - 1)..=(rf.round() as i32 + 1) {
            if row < 0 || row >= self.size as i32 {
                continue;
            }
            let cf = x / self.cell_size_m + half + 0.25 - (row % 2) as f64 * 0.5;
            if cf < -0.5 || cf > self.size as f64 - 0.5 {
                continue;
            }
            for col in (cf.round() as i32 - 1)..=(cf.round() as i32 + 1) {
                if col < 0 || col >= self.size as i32 {
                    continue;
                }
                let i = row as usize * self.size as usize + col as usize;
                let (px, py) = self.local_center(i);
                let d = (px - x).powi(2) + (py - y).powi(2);
                if d < best_d {
                    best_d = d;
                    best = Some(i)
                }
            }
        }
        best
    }
}

impl crate::GridOps for PatchGrid {
    fn locate_local(&self, x: f64, y: f64) -> Option<usize> {
        self.cell_at_local(x, y)
    }
    fn locate(&self, dir: UnitVec3) -> Option<usize> {
        self.cell_at(dir)
    }
    fn field_position(&self, i: usize) -> DVec3 {
        // A child patch retains its geographic continuous-field coordinates.
        self.center_dir(i).as_vec() * self.radius_m
    }
    fn extent_m(&self) -> Option<(f64, f64)> {
        Some((
            self.size as f64 * self.cell_size_m,
            self.size as f64 * self.cell_size_m * 3_f64.sqrt() / 2.,
        ))
    }
    fn local_center_m(&self, i: usize) -> Option<(f64, f64)> {
        Some(self.local_center(i))
    }

    fn radius_m(&self) -> f64 {
        self.radius_m
    }
    fn cell_count(&self) -> usize {
        self.centers.len()
    }

    fn neighbors(&self, i: usize) -> &[u32] {
        let start = self.offsets[i] as usize;
        let end = self.offsets[i + 1] as usize;
        &self.neighbors[start..end]
    }

    fn distance_m(&self, a: usize, b: usize) -> f64 {
        self.centers[a].arc_distance_m(&self.centers[b], self.radius_m)
    }

    fn area_m2(&self, i: usize) -> f64 {
        self.areas[i]
    }

    fn center_dir(&self, i: usize) -> UnitVec3 {
        self.centers[i]
    }

    fn latitude_deg(&self, i: usize) -> f64 {
        self.lat_deg[i]
    }

    fn boundary_mask(&self) -> &BitVec {
        &self.boundary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GridOps;

    #[test]
    fn every_center_round_trips_across_dateline_poles_and_radii() {
        for (lat, lon, radius) in [
            (0., 179.9, 6_371_000.),
            (89.9, -179.9, 3_000_000.),
            (-90., 120., 12_000_000.),
        ] {
            let g = PatchGrid::new(&SphericalPatch::with_radius(lat, lon, 128_000., 32, radius));
            for i in 0..g.cell_count() {
                assert_eq!(g.cell_at(g.center_dir(i)), Some(i), "{lat}, {lon}, {i}");
            }
            assert!(g
                .cell_at(UnitVec3::from_lat_lon_deg(-lat, lon + 180.))
                .is_none());
        }
    }
    #[test]
    fn north_is_up_and_spacing_is_in_metres() {
        let g = PatchGrid::new(&SphericalPatch::with_radius(
            30., 100., 32_000., 32, 3_000_000.,
        ));
        assert!(g.latitude_deg(15) > g.latitude_deg(31 * 32 + 15));
        assert!((g.distance_m(16 * 32 + 15, 16 * 32 + 16) - 1000.).abs() < 1.);
    }

    #[test]
    fn patch_grid_dimensions() {
        let p = SphericalPatch::new(30.0, 100.0, 4_000_000.0, 64);
        let g = PatchGrid::new(&p);
        assert_eq!(g.cell_count(), 64 * 64);
        assert!((g.cell_size_m() - 62_500.0).abs() < 1.0);
    }

    #[test]
    fn interior_has_six_neighbors() {
        let p = SphericalPatch::new(0.0, 0.0, 1_000_000.0, 32);
        let g = PatchGrid::new(&p);
        let center = (16 * 32 + 16) as usize;
        assert_eq!(g.neighbors(center).len(), 6);
    }

    #[test]
    fn cells_near_center_of_patch() {
        let p = SphericalPatch::new(45.0, 120.0, 100_000.0, 16);
        let g = PatchGrid::new(&p);
        // 中心格应该接近给定的经纬度
        let center_idx = (8 * 16 + 8) as usize;
        let lat = g.latitude_deg(center_idx);
        assert!((lat - 45.0).abs() < 3.0, "lat {}", lat);
    }

    #[test]
    fn adjacency_symmetric() {
        let p = SphericalPatch::new(0.0, 0.0, 500_000.0, 16);
        let g = PatchGrid::new(&p);
        for i in 0..g.cell_count() {
            for &nb in g.neighbors(i) {
                assert!(g.neighbors(nb as usize).contains(&(i as u32)));
            }
        }
    }
}
