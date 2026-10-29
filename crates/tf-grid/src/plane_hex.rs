//! 平面六角格网格，用轴向坐标（axial coordinates）。
//!
//! 下层三层（大陆/区域/地点）都是平面局部地图。
//!
//! # 坐标系统
//!
//! 用 Red Blob Games 的 axial 坐标：两个轴 q（东南）和 r（正南）。
//! 第三个轴 s = -q - r 隐含。
//!
//! 转像素：x = size * 1.5 * q，y = size * sqrt(3) * (r + q/2)。
//! 其中 size 是六边形的外接圆半径。

use bitvec::prelude::BitVec;
use serde::{Deserialize, Serialize};
use tf_core::geo::UnitVec3;

/// 轴向坐标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AxialCoord {
    pub q: i32,
    pub r: i32,
}

impl AxialCoord {
    pub const fn new(q: i32, r: i32) -> Self {
        Self { q, r }
    }

    /// 第三个轴，由 s = -q - r 决定。
    #[inline]
    pub fn s(&self) -> i32 {
        -self.q - self.r
    }

    /// 六角距离。
    #[inline]
    pub fn distance(&self, other: &Self) -> i32 {
        ((self.q - other.q).abs()
            + (self.r - other.r).abs()
            + (self.s() - other.s()).abs())
            / 2
    }

    /// 六个方向的邻居偏移。
    pub const DIRECTIONS: [(i32, i32); 6] = [
        (1, 0),
        (1, -1),
        (0, -1),
        (-1, 0),
        (-1, 1),
        (0, 1),
    ];

    /// 第 d 个方向的邻居。
    #[inline]
    pub fn neighbor(&self, d: usize) -> Self {
        let (dq, dr) = Self::DIRECTIONS[d % 6];
        Self::new(self.q + dq, self.r + dr)
    }

    /// 转到平面像素坐标。size 是六边形的外接圆半径。
    #[inline]
    pub fn to_pixel(&self, size: f64) -> (f64, f64) {
        let x = size * 1.5 * self.q as f64;
        let y = size * (3.0_f64).sqrt() * (self.r as f64 + self.q as f64 / 2.0);
        (x, y)
    }

    /// 从平面像素坐标反推最近的轴向坐标。
    pub fn from_pixel(x: f64, y: f64, size: f64) -> Self {
        let q = (2.0 / 3.0) * x / size;
        let r = (-1.0 / 3.0) * x / size + ((3.0_f64).sqrt() / 3.0) * y / size;
        axial_round(q, r)
    }
}

/// 浮点轴向坐标四舍五入到最近的整数格。
fn axial_round(qf: f64, rf: f64) -> AxialCoord {
    let sf = -qf - rf;
    let mut q = qf.round();
    let mut r = rf.round();
    let s = sf.round();

    let dq = (q - qf).abs();
    let dr = (r - rf).abs();
    let ds = (s - sf).abs();

    if dq > dr && dq > ds {
        q = -r - s;
    } else if dr > ds {
        r = -q - s;
    }
    AxialCoord::new(q as i32, r as i32)
}

/// 平面六角格网格。
///
/// 用包围盒存储：记录宽高，用一维数组索引。
/// 邻接表用 CSR 格式，按六角格的行奇偶偏移规则建立。
#[derive(Debug, Clone)]
pub struct HexGrid2D {
    width: u32,
    height: u32,
    centers: Vec<UnitVec3>,
    lat_deg: Vec<f64>,
    areas: Vec<f64>,
    offsets: Vec<u32>,
    neighbors: Vec<u32>,
    boundary: BitVec,
    cell_size_m: f64,
    center_lat_deg: f64,
    lat_span_deg: f64,
}

impl HexGrid2D {
    /// 构造一个矩形范围的平面六角格网格。
    ///
    /// cell_size_m 是相邻格中心之间的距离。
    /// center_lat_deg 和 lat_span_deg 用于给每格分配纬度，从而算气候。
    pub fn new(
        width: u32,
        height: u32,
        cell_size_m: f64,
        center_lat_deg: f64,
        lat_span_deg: f64,
    ) -> Self {
        assert!(width > 0 && height > 0, "grid dimensions must be positive");
        assert!(cell_size_m > 0.0, "cell size must be positive");

        let n = (width as usize) * (height as usize);
        let mut centers = Vec::with_capacity(n);
        let mut lat_deg = Vec::with_capacity(n);
        let mut areas = Vec::with_capacity(n);

        // 六角格面积 = (3*sqrt(3)/2) * size^2，size 是外接圆半径。
        // 相邻中心距离 = sqrt(3) * size，所以 size = cell_size / sqrt(3)。
        let size = cell_size_m / (3.0_f64).sqrt();
        let cell_area = (3.0 * (3.0_f64).sqrt() / 2.0) * size * size;

        let total_h = height as f64;

        for row in 0..height {
            // 奇数行错开半格，这是六角格的经典布局
            let row_shift = if row % 2 == 0 { 0.0 } else { 0.5 };
            for col in 0..width {
                let lat = center_lat_deg - ((row as f64 + 0.5) / total_h - 0.5) * lat_span_deg;
                let lat_rad = lat.to_radians();

                // 局部切平面上的经度偏移，按纬度压缩以保持近似等距
                let lon_offset_m = ((col as f64 + row_shift) - width as f64 / 2.0) * cell_size_m;
                let lon_rad =
                    lon_offset_m / (6_371_000.0 * lat_rad.cos().abs().max(0.05));

                let (sl, cl) = lat_rad.sin_cos();
                let (so, co) = lon_rad.sin_cos();
                let dir = UnitVec3::from_normalized(glam::DVec3::new(cl * co, sl, cl * so));

                centers.push(dir);
                lat_deg.push(lat);
                areas.push(cell_area);
            }
        }

        // 建邻接：按行奇偶决定六个邻居的列偏移
        let mut offsets = Vec::with_capacity(n + 1);
        let mut neighbors = Vec::with_capacity(n * 6);
        offsets.push(0u32);

        for row in 0..height as i32 {
            for col in 0..width as i32 {
                // 偶数行的邻居偏移
                let deltas: [(i32, i32); 6] = if row % 2 == 0 {
                    [(0, -1), (0, 1), (-1, -1), (-1, 0), (1, -1), (1, 0)]
                } else {
                    [(0, -1), (0, 1), (-1, 0), (-1, 1), (1, 0), (1, 1)]
                };

                for (dr, dc) in deltas {
                    let nr = row + dr;
                    let nc = col + dc;
                    if nr >= 0 && nr < height as i32 && nc >= 0 && nc < width as i32 {
                        neighbors.push((nr as u32) * width + nc as u32);
                    }
                }
                offsets.push(neighbors.len() as u32);
            }
        }

        // 边界掩码：邻居数少于 6
        let mut boundary = BitVec::with_capacity(n);
        for i in 0..n {
            let cnt = (offsets[i + 1] - offsets[i]) as usize;
            boundary.push(cnt < 6);
        }

        Self {
            width,
            height,
            centers,
            lat_deg,
            areas,
            offsets,
            neighbors,
            boundary,
            cell_size_m,
            center_lat_deg,
            lat_span_deg,
        }
    }

    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[inline]
    pub fn cell_size_m(&self) -> f64 {
        self.cell_size_m
    }

    #[inline]
    pub fn center_lat_deg(&self) -> f64 {
        self.center_lat_deg
    }

    #[inline]
    pub fn lat_span_deg(&self) -> f64 {
        self.lat_span_deg
    }

    /// 格子索引转 (行, 列)。
    #[inline]
    pub fn row_col(&self, i: usize) -> (u32, u32) {
        ((i as u32) / self.width, (i as u32) % self.width)
    }

    /// (行, 列) 转格子索引。
    #[inline]
    pub fn index(&self, row: u32, col: u32) -> Option<usize> {
        if row < self.height && col < self.width {
            Some((row * self.width + col) as usize)
        } else {
            None
        }
    }
}

impl crate::GridOps for HexGrid2D {
    fn cell_count(&self) -> usize {
        self.centers.len()
    }

    fn neighbors(&self, i: usize) -> &[u32] {
        let start = self.offsets[i] as usize;
        let end = self.offsets[i + 1] as usize;
        &self.neighbors[start..end]
    }

    fn distance_m(&self, _a: usize, _b: usize) -> f64 {
        // 平面网格上所有相邻中心距离都等于格宽
        self.cell_size_m
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
    fn cell_count_is_width_times_height() {
        let g = HexGrid2D::new(64, 48, 1000.0, 30.0, 10.0);
        assert_eq!(g.cell_count(), 64 * 48);
    }

    #[test]
    fn interior_cells_have_six_neighbors() {
        let g = HexGrid2D::new(32, 32, 1000.0, 30.0, 10.0);
        let center = g.index(16, 16).unwrap();
        assert_eq!(g.neighbors(center).len(), 6);
    }

    #[test]
    fn corner_cells_have_fewer_neighbors() {
        let g = HexGrid2D::new(32, 32, 1000.0, 30.0, 10.0);
        let corner = g.index(0, 0).unwrap();
        assert!(g.neighbors(corner).len() < 6);
    }

    #[test]
    fn adjacency_is_symmetric() {
        let g = HexGrid2D::new(16, 16, 500.0, 0.0, 5.0);
        for i in 0..g.cell_count() {
            for &nb in g.neighbors(i) {
                let back = g.neighbors(nb as usize);
                assert!(back.contains(&(i as u32)), "{} -> {} not symmetric", i, nb);
            }
        }
    }

    #[test]
    fn axial_distance_works() {
        let a = AxialCoord::new(0, 0);
        let b = AxialCoord::new(2, -1);
        assert_eq!(a.distance(&b), 2);
        assert_eq!(a.distance(&a), 0);
    }

    #[test]
    fn pixel_roundtrip() {
        let size = 10.0;
        for q in -5..=5 {
            for r in -5..=5 {
                let c = AxialCoord::new(q, r);
                let (x, y) = c.to_pixel(size);
                let back = AxialCoord::from_pixel(x, y, size);
                assert_eq!(back, c);
            }
        }
    }

    #[test]
    fn latitude_varies_across_rows() {
        let g = HexGrid2D::new(32, 32, 1000.0, 45.0, 20.0);
        let top = g.index(0, 0).unwrap();
        let bottom = g.index(31, 0).unwrap();
        assert!(g.latitude_deg(top) > g.latitude_deg(bottom));
    }

    #[test]
    fn boundary_mask_marks_edges() {
        let g = HexGrid2D::new(16, 16, 1000.0, 0.0, 5.0);
        let corner = g.index(0, 0).unwrap();
        let center = g.index(8, 8).unwrap();
        assert!(g.boundary_mask()[corner]);
        assert!(!g.boundary_mask()[center]);
    }
}
