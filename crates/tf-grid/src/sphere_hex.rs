//! 球面六角格网格（戈德堡多面体）。
//!
//! 这是整个项目的地基。世界层的所有数据都挂在这个结构上。
//!
//! # 拓扑
//!
//! 正二十面体有 20 个三角面、12 个顶点。每个面按重心坐标细分成 f 阶小三角，
//! 得到测地线球。测地线球的顶点数恰好是 10f^2+2，这也就是戈德堡多面体的面数。
//!
//! 相邻的戈德堡面（格子）由测地线球的一条边连接。所以建邻接就是遍历所有
//! 小三角形的边，把两端点连起来。这个关系完全由索引结构决定，与点的实际
//! 坐标无关——这一点很重要，它保证了拓扑的正确性不受浮点误差影响。
//!
//! # 正确性的关键：位置必须是拓扑键的纯函数
//!
//! 每个细分点有一个拓扑键（见 PointKey）。同一个物理点从相邻的几个三角面
//! 分别计算时，必须得到逐位相同的坐标，否则就会出现「同一个键对应两个不同
//! 位置」的错乱——这才是之前邻接异常的真正原因（拓扑没错，位置错）。
//!
//! 所以本模块的核心约束是：position_for_key 只依赖键本身，不依赖调用方来自
//! 哪个三角面。棱上的点用「顶点编号小的一端为起点」的规范方向插值，
//! 从而天然满足这个约束。

use bitvec::prelude::BitVec;
use glam::DVec3;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::f64::consts::PI;
use tf_core::geo::{rotate_axis_angle, UnitVec3};

// ============================================================
// 二十面体
// ============================================================

fn icosahedron_vertices() -> [DVec3; 12] {
    let phi = (1.0 + 5.0_f64.sqrt()) / 2.0;
    let raw = [
        (-1.0, phi, 0.0),
        (1.0, phi, 0.0),
        (-1.0, -phi, 0.0),
        (1.0, -phi, 0.0),
        (0.0, -1.0, phi),
        (0.0, 1.0, phi),
        (0.0, -1.0, -phi),
        (0.0, 1.0, -phi),
        (phi, 0.0, -1.0),
        (phi, 0.0, 1.0),
        (-phi, 0.0, -1.0),
        (-phi, 0.0, 1.0),
    ];
    let mut out = [DVec3::ZERO; 12];
    for (i, &(x, y, z)) in raw.iter().enumerate() {
        out[i] = DVec3::new(x, y, z).normalize();
    }
    out
}

const FACES: [[usize; 3]; 20] = [
    [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
    [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
    [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
    [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
];

// ============================================================
// 拓扑键
// ============================================================

/// 细分点的拓扑身份。
///
/// 同一个物理点从任何三角面算出来都得到相同的键，因此可以精确去重，
/// 完全不依赖浮点容差。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointKey {
    /// 二十面体顶点，参数是顶点编号 0..12。
    Vertex(u8),
    /// 棱上的点。(小端顶点号, 大端顶点号, 沿小端到大端方向的步数)
    Edge(u8, u8, u32),
    /// 面内点。(面编号, i, j)
    Interior(u8, u32, u32),
}

impl PointKey {
    fn packed(&self) -> u64 {
        match *self {
            PointKey::Vertex(v) => v as u64,
            PointKey::Edge(u, v, t) => {
                0x1000_0000_0000_0000 | ((u as u64) << 32) | ((v as u64) << 24) | (t as u64)
            }
            PointKey::Interior(f, i, j) => {
                0x2000_0000_0000_0000 | ((f as u64) << 48) | ((i as u64) << 24) | (j as u64)
            }
        }
    }
}

/// 把三角面内的重心坐标翻译成全局拓扑键。
fn face_point_key(face_idx: usize, vids: [usize; 3], freq: u32, i: u32, j: u32, k: u32) -> PointKey {
    debug_assert_eq!(i + j + k, freq);
    let [ia, ib, ic] = vids;

    // 重心坐标约定（全模块统一，务必与 position_for_key 保持一致）：
    //   w_a = k / freq,  w_b = i / freq,  w_c = j / freq
    // 其中 k = freq - i - j 是补数。
    //
    // 之前把 w = [i, j, k] 直接对应 (a, b, c) 是错的——
    // 那样点 (i=1, j=0) 会被放到顶点 c 附近而不是顶点 a 附近，
    // 表现出来就是「顶点格的邻居相距整条棱长」。

    if k == freq {
        return PointKey::Vertex(ia as u8);
    }
    if i == freq {
        return PointKey::Vertex(ib as u8);
    }
    if j == freq {
        return PointKey::Vertex(ic as u8);
    }
    if j == 0 {
        // w_c = 0，落在 a-b 棱上。沿 a -> b 走了 i 步。
        return edge_key(ia, ib, i, freq);
    }
    if k == 0 {
        // w_a = 0，落在 b-c 棱上。沿 b -> c 走了 j 步。
        return edge_key(ib, ic, j, freq);
    }
    if i == 0 {
        // w_b = 0，落在 a-c 棱上。沿 a -> c 走了 j 步。
        return edge_key(ia, ic, j, freq);
    }
    PointKey::Interior(face_idx as u8, i, j)
}

#[inline]
fn edge_key(va: usize, vb: usize, steps_from_a: u32, freq: u32) -> PointKey {
    if va < vb {
        PointKey::Edge(va as u8, vb as u8, steps_from_a)
    } else {
        PointKey::Edge(vb as u8, va as u8, freq - steps_from_a)
    }
}

/// 由拓扑键计算点位置。
///
/// 这个函数只依赖键本身，不依赖调用方来自哪个三角面。
/// 这是整个网格几何正确性的基石。
fn position_for_key(key: PointKey, freq: u32, verts: &[DVec3; 12]) -> DVec3 {
    let f = freq as f64;
    match key {
        PointKey::Vertex(v) => verts[v as usize],
        PointKey::Edge(u, v, t) => {
            // 方向永远是「编号小 -> 编号大」，因此两侧面算出的位置逐位相同。
            let (a, b) = (verts[u as usize], verts[v as usize]);
            slerp(a, b, t as f64 / f)
        }
        PointKey::Interior(face_idx, i, j) => {
            // 面内点只属于一个面，直接在该面上插值。
            let face = FACES[face_idx as usize];
            let (a, b, c) = (verts[face[0]], verts[face[1]], verts[face[2]]);
            let k = freq - i - j;

            // 与 face_point_key 相同的重心约定：w_a = k/f, w_b = i/f, w_c = j/f
            let wa = k as f64 / f;
            let wb = i as f64 / f;
            let wc = j as f64 / f;

            // 用 Snyder 等面积逆投影。它要求 beta 是平面三角形的重心坐标，
            // 与上面的 (wa, wb, wc) 一一对应。
            crate::snyder::snyder_inverse(a, b, c, [wa, wb, wc])
        }
    }
}

// ============================================================
// 网格
// ============================================================

#[derive(Debug, Clone)]
pub struct SphereHexGrid {
    freq: u32,
    rotation: (DVec3, f64),
    radius_m: f64,
    centers: Vec<UnitVec3>,
    offsets: Vec<u32>,
    neighbors: Vec<u32>,
    pentagons: Vec<u32>,
    areas: Vec<f64>,
    boundary: std::sync::OnceLock<BitVec>,
    /// 空间哈希索引，惰性构建，用于快速查找最近格
    spatial_index: std::sync::OnceLock<(std::collections::HashMap<(i32, i32, i32), Vec<u32>>, f64, i32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridStats {
    pub cell_count: usize,
    pub pentagon_count: usize,
    pub hexagon_count: usize,
    pub freq: u32,
    pub avg_cell_area_km2: f64,
    pub min_cell_area_km2: f64,
    pub max_cell_area_km2: f64,
    pub area_ratio: f64,
    pub avg_edge_length_km: f64,
    pub cell_width_km: f64,
    pub total_area_error_pct: f64,
}

impl SphereHexGrid {
    pub fn build(freq: u32, radius_m: f64, rotation: (DVec3, f64)) -> Self {
        assert!(freq >= 1, "freq must be at least 1");
        let verts = icosahedron_vertices();

        let expected = (10 * freq as u64 * freq as u64 + 2) as usize;
        let mut centers: Vec<UnitVec3> = Vec::with_capacity(expected);
        let mut key_to_id: HashMap<u64, u32> = HashMap::with_capacity(expected * 2);

        let per_face = ((freq + 1) * (freq + 2) / 2) as usize;
        let mut face_points: Vec<Vec<u32>> = vec![Vec::with_capacity(per_face); 20];

        for (face_idx, &vids) in FACES.iter().enumerate() {
            let mut ids = vec![0u32; per_face];
            for i in 0..=freq {
                for j in 0..=(freq - i) {
                    let k = freq - i - j;
                    let idx = tri_point_index(freq, i, j) as usize;
                    let key = face_point_key(face_idx, vids, freq, i, j, k);
                    let packed = key.packed();

                    let id = if let Some(&existing) = key_to_id.get(&packed) {
                        existing
                    } else {
                        let pos = position_for_key(key, freq, &verts);
                        let new_id = centers.len() as u32;
                        centers.push(UnitVec3::from_normalized(pos.normalize()));
                        key_to_id.insert(packed, new_id);
                        new_id
                    };
                    ids[idx] = id;
                }
            }
            face_points[face_idx] = ids;
        }

        let cell_count = centers.len();
        assert_eq!(cell_count, expected, "cell count mismatch for freq {}", freq);

        // 五边形就是二十面体顶点对应的格子。12 个，由拓扑精确确定。
        let mut pentagons: Vec<u32> = (0..12usize)
            .filter_map(|v| key_to_id.get(&PointKey::Vertex(v as u8).packed()).copied())
            .collect();
        pentagons.sort_unstable();
        assert_eq!(pentagons.len(), 12, "expected 12 icosahedron vertex cells");

        if rotation.1.abs() > 1e-12 {
            let axis = rotation.0.normalize();
            for c in centers.iter_mut() {
                *c = UnitVec3::from_normalized(rotate_axis_angle(c.as_vec(), axis, rotation.1));
            }
        }

        // 建邻接：遍历所有小三角形的三条边。
        // 拓扑正确性与坐标无关，所以这里天然可靠。
        let mut adj: Vec<Vec<u32>> = vec![Vec::with_capacity(6); cell_count];
        for ids in face_points.iter() {
            for i in 0..freq {
                for j in 0..(freq - i) {
                    let p0 = ids[tri_point_index(freq, i, j) as usize];
                    let p1 = ids[tri_point_index(freq, i + 1, j) as usize];
                    let p2 = ids[tri_point_index(freq, i, j + 1) as usize];
                    add_triangle_edges(&mut adj, p0, p1, p2);

                    // 倒三角 (i+1,j), (i+1,j+1), (i,j+1)
                    // 存在条件是 i+1+j+1 <= freq，即 i+j+2 <= freq
                    if i + j + 2 <= freq {
                        let q0 = ids[tri_point_index(freq, i + 1, j) as usize];
                        let q1 = ids[tri_point_index(freq, i + 1, j + 1) as usize];
                        let q2 = ids[tri_point_index(freq, i, j + 1) as usize];
                        add_triangle_edges(&mut adj, q0, q1, q2);
                    }
                }
            }
        }

        let mut offsets = Vec::with_capacity(cell_count + 1);
        let mut neighbors = Vec::with_capacity(cell_count * 6);
        offsets.push(0u32);
        for (i, set) in adj.iter_mut().enumerate() {
            set.sort_unstable();
            set.dedup();
            assert!(
                (5..=6).contains(&set.len()),
                "cell {} has {} neighbors, expected 5 or 6",
                i,
                set.len()
            );
            neighbors.extend_from_slice(set);
            offsets.push(neighbors.len() as u32);
        }

        for &p in &pentagons {
            let cnt = (offsets[p as usize + 1] - offsets[p as usize]) as usize;
            assert_eq!(cnt, 5, "pentagon cell {} has {} neighbors", p, cnt);
        }

        let areas = compute_cell_areas(&centers, &offsets, &neighbors, radius_m);

        Self {
            freq,
            rotation,
            radius_m,
            centers,
            offsets,
            neighbors,
            pentagons,
            areas,
            boundary: std::sync::OnceLock::new(),
            spatial_index: std::sync::OnceLock::new(),
        }
    }

    #[inline]
    pub fn cell_count(&self) -> usize {
        self.centers.len()
    }

    #[inline]
    pub fn freq(&self) -> u32 {
        self.freq
    }

    #[inline]
    pub fn radius_m(&self) -> f64 {
        self.radius_m
    }

    #[inline]
    pub fn rotation(&self) -> (DVec3, f64) {
        self.rotation
    }

    #[inline]
    pub fn center(&self, i: usize) -> UnitVec3 {
        self.centers[i]
    }

    #[inline]
    pub fn centers(&self) -> &[UnitVec3] {
        &self.centers
    }

    #[inline]
    pub fn area_m2(&self, i: usize) -> f64 {
        self.areas[i]
    }

    #[inline]
    pub fn neighbors(&self, i: usize) -> &[u32] {
        let start = self.offsets[i] as usize;
        let end = self.offsets[i + 1] as usize;
        &self.neighbors[start..end]
    }

    #[inline]
    pub fn neighbor_count(&self, i: usize) -> usize {
        (self.offsets[i + 1] - self.offsets[i]) as usize
    }

    #[inline]
    pub fn is_pentagon(&self, i: usize) -> bool {
        self.neighbor_count(i) == 5
    }

    #[inline]
    pub fn pentagons(&self) -> &[u32] {
        &self.pentagons
    }

    #[inline]
    pub fn distance_m(&self, a: usize, b: usize) -> f64 {
        self.centers[a].arc_distance_m(&self.centers[b], self.radius_m)
    }

    /// 找离给定方向最近的格子。
    ///
    /// 用立方体空间哈希加速。线性扫描在 590 万格时单次要几十毫秒，
    /// 而渲染一帧要查几十万次，必须加速。
    pub fn cell_at(&self, target: UnitVec3) -> usize {
        use std::collections::HashMap;

        // 惰性建索引：只建一次，后面复用
        let index = self.spatial_index.get_or_init(|| {
            let n = self.centers.len();
            // 桶尺寸：格间距的 2 倍，保证最近点一定在邻近 27 桶内
            let cell_angle = (4.0 * std::f64::consts::PI / n as f64).sqrt();
            let bucket = (cell_angle * 2.0).max(1e-4);
            let dim = (2.0 / bucket).ceil() as i32 + 1;
            let mut map: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::with_capacity(n / 2);
            for (i, c) in self.centers.iter().enumerate() {
                let v = c.as_vec();
                let key = (
                    ((v.x + 1.0) / bucket).floor() as i32,
                    ((v.y + 1.0) / bucket).floor() as i32,
                    ((v.z + 1.0) / bucket).floor() as i32,
                );
                map.entry(key).or_default().push(i as u32);
            }
            (map, bucket, dim)
        });

        let (map, bucket, dim) = index;
        let tv = target.as_vec();
        let base = (
            ((tv.x + 1.0) / bucket).floor() as i32,
            ((tv.y + 1.0) / bucket).floor() as i32,
            ((tv.z + 1.0) / bucket).floor() as i32,
        );

        let mut best = 0usize;
        let mut best_score = f64::NEG_INFINITY;
        let mut found = false;

        // 先在邻近 27 桶里找
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let key = (base.0 + dx, base.1 + dy, base.2 + dz);
                    if let Some(list) = map.get(&key) {
                        for &i in list {
                            let s = self.centers[i as usize].as_vec().dot(tv);
                            if s > best_score {
                                best_score = s;
                                best = i as usize;
                                found = true;
                            }
                        }
                    }
                }
            }
        }

        // 兜底：桶参数极端时可能找不到，退化为全扫描
        if !found {
            for (i, c) in self.centers.iter().enumerate() {
                let s = c.as_vec().dot(tv);
                if s > best_score {
                    best_score = s;
                    best = i;
                }
            }
        }
        let _ = dim;
        best
    }

    pub fn stats(&self) -> GridStats {
        let n = self.cell_count();
        let mut hex_areas = Vec::with_capacity(n);
        let mut pent_areas = Vec::new();
        for i in 0..n {
            if self.is_pentagon(i) {
                pent_areas.push(self.areas[i]);
            } else {
                hex_areas.push(self.areas[i]);
            }
        }

        let total: f64 = self.areas.iter().sum();
        let sphere_area = 4.0 * PI * self.radius_m * self.radius_m;
        let min_hex = hex_areas.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_hex = hex_areas.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        let mut edge_total = 0.0;
        let mut edge_count = 0usize;
        for i in 0..n {
            for &nb in self.neighbors(i) {
                if (nb as usize) > i {
                    edge_total += self.distance_m(i, nb as usize);
                    edge_count += 1;
                }
            }
        }
        let avg_edge_m = if edge_count > 0 {
            edge_total / edge_count as f64
        } else {
            0.0
        };

        GridStats {
            cell_count: n,
            pentagon_count: pent_areas.len(),
            hexagon_count: hex_areas.len(),
            freq: self.freq,
            avg_cell_area_km2: total / n as f64 / 1e6,
            min_cell_area_km2: min_hex / 1e6,
            max_cell_area_km2: max_hex / 1e6,
            area_ratio: if min_hex > 0.0 { max_hex / min_hex } else { 0.0 },
            avg_edge_length_km: avg_edge_m / 1000.0,
            cell_width_km: avg_edge_m / 1000.0,
            total_area_error_pct: (total / sphere_area - 1.0) * 100.0,
        }
    }
}

// ============================================================
// 辅助
// ============================================================

#[inline]
fn tri_point_index(freq: u32, i: u32, j: u32) -> u32 {
    debug_assert!(i + j <= freq, "invalid barycentric ({}, {}) for freq {}", i, j, freq);
    let before = i * (freq + 1) - i * (i.saturating_sub(1)) / 2;
    before + j
}

#[inline]
fn slerp(a: DVec3, b: DVec3, t: f64) -> DVec3 {
    let dot = a.dot(b).clamp(-1.0, 1.0);
    let theta = dot.acos();
    if theta.abs() < 1e-12 {
        return a;
    }
    let sin_theta = theta.sin();
    let w0 = ((1.0 - t) * theta).sin() / sin_theta;
    let w1 = (t * theta).sin() / sin_theta;
    a * w0 + b * w1
}

#[inline]
fn add_triangle_edges(adj: &mut [Vec<u32>], p0: u32, p1: u32, p2: u32) {
    add_edge(adj, p0, p1);
    add_edge(adj, p1, p2);
    add_edge(adj, p2, p0);
}

#[inline]
fn add_edge(adj: &mut [Vec<u32>], a: u32, b: u32) {
    if a == b {
        return;
    }
    if !adj[a as usize].contains(&b) {
        adj[a as usize].push(b);
    }
    if !adj[b as usize].contains(&a) {
        adj[b as usize].push(a);
    }
}

fn compute_cell_areas(
    centers: &[UnitVec3],
    offsets: &[u32],
    neighbors: &[u32],
    radius_m: f64,
) -> Vec<f64> {
    let n = centers.len();
    let r2 = radius_m * radius_m;

    let raw: Vec<f64> = (0..n)
        .into_par_iter()
        .map(|i| {
            let start = offsets[i] as usize;
            let end = offsets[i + 1] as usize;
            let nb = &neighbors[start..end];
            if nb.len() < 3 {
                return 0.0;
            }
            let c = centers[i].as_vec();
            let ordered = angular_sort_neighbors(c, centers, nb);
            let mut acc = 0.0;
            for k in 0..ordered.len() {
                let b = centers[ordered[k]].as_vec();
                let d = centers[ordered[(k + 1) % ordered.len()]].as_vec();
                acc += spherical_triangle_solid_angle(c, b, d);
            }
            acc / 3.0
        })
        .collect();

    let sum: f64 = raw.iter().sum();
    let scale = if sum > 1e-12 { 4.0 * PI / sum } else { 1.0 };
    raw.iter().map(|s| s * scale * r2).collect()
}

fn angular_sort_neighbors(center: DVec3, centers: &[UnitVec3], nb: &[u32]) -> Vec<usize> {
    let reference = if center.y.abs() < 0.9 { DVec3::Y } else { DVec3::X };
    let east = reference.cross(center).normalize();
    let north = center.cross(east);

    let mut with_angle: Vec<(f64, usize)> = nb
        .iter()
        .map(|&j| {
            let v = centers[j as usize].as_vec();
            (v.dot(north).atan2(v.dot(east)), j as usize)
        })
        .collect();

    with_angle.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    with_angle.into_iter().map(|(_, j)| j).collect()
}

#[inline]
fn spherical_triangle_solid_angle(a: DVec3, b: DVec3, c: DVec3) -> f64 {
    let numerator = a.dot(b.cross(c)).abs();
    let denominator = 1.0 + a.dot(b) + b.dot(c) + c.dot(a);
    if denominator.abs() < 1e-15 {
        return 0.0;
    }
    2.0 * numerator.atan2(denominator)
}

impl crate::GridOps for SphereHexGrid {
    fn cell_count(&self) -> usize {
        self.centers.len()
    }

    fn neighbors(&self, i: usize) -> &[u32] {
        let start = self.offsets[i] as usize;
        let end = self.offsets[i + 1] as usize;
        &self.neighbors[start..end]
    }

    fn distance_m(&self, a: usize, b: usize) -> f64 {
        SphereHexGrid::distance_m(self, a, b)
    }

    fn area_m2(&self, i: usize) -> f64 {
        self.areas[i]
    }

    fn center_dir(&self, i: usize) -> UnitVec3 {
        self.centers[i]
    }

    fn latitude_deg(&self, i: usize) -> f64 {
        self.centers[i].to_lat_lon_deg().0
    }

    fn boundary_mask(&self) -> &BitVec {
        self.boundary
            .get_or_init(|| BitVec::repeat(false, self.centers.len()))
    }
}

// ============================================================
// 可视化导出辅助
// ============================================================

/// 供预览用的六角球网格数据。
///
/// 前端渲染器直接消费这个结构。为了控制体积，顶点坐标为 f32 的 xyz，
/// 每个格子输出 6 个边界顶点（五边形只有 5 个有效）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewMesh {
    /// 格子数量。
    pub cell_count: u32,
    /// 每个格子的中心，xyz 交错存储，长度 = cell_count * 3。
    pub centers: Vec<f32>,
    /// 每个格子的 6 个角点，xyz 交错，长度 = cell_count * 6 * 3。
    /// 五边形格的第 6 个角点与第 1 个重复，前端可以按 neighbor_count 裁剪。
    pub corners: Vec<f32>,
    /// 每个格子的实际角点数（5 或 6）。
    pub corner_counts: Vec<u8>,
}

impl SphereHexGrid {
    /// 计算每个格子的多边形角点。
    ///
    /// 做法：格子的角点位于它相邻邻居之间的「中分方向」。
    /// 对中心 c 和两个相邻邻居 b、d，角点方向约为 normalize(b + d - c)
    /// 再按球面比例修正。这是个很好的近似，渲染时看不出差别。
    pub fn corners_of(&self, i: usize) -> Vec<DVec3> {
        let c = self.centers[i].as_vec();
        let nb = self.neighbors(i);
        let n = nb.len();
        if n < 3 {
            return Vec::new();
        }

        // Voronoi 格子的顶点，是「三个互相相邻的格子中心」的球面外心。
        //
        // 之前的实现用「两个邻居方向和中点」近似，在远离中心处会偏移，
        // 导致渲染出现同心圆纹理——那是角点算错，不是地形问题。
        //
        // 球面三角形外心：对单位球面上的三点 a、b、c，外心方向正比于
        // (b-a)×(c-a) 与球心的连线在球面上的投影，简化写法是
        // normalize(|c|²(a×b) + |a|²(b×c) + |b|²(c×a))，
        // 单位球上模长都是 1，退化为 normalize(a×b + b×c + c×a)。
        let ordered = angular_sort_neighbors(c, &self.centers, nb);
        let mut out = Vec::with_capacity(n);
        for k in 0..n {
            let a = self.centers[ordered[k]].as_vec();
            let b = self.centers[ordered[(k + 1) % n]].as_vec();
            let cross = a.cross(b) + b.cross(c) + c.cross(a);
            let len = cross.length();
            if len < 1e-12 {
                // 退化时回退到中点方向
                out.push(((a + b) * 0.5).normalize());
            } else {
                out.push(cross / len);
            }
        }
        out
    }


    /// 导出预览网格。
    pub fn to_preview_mesh(&self) -> PreviewMesh {
        let n = self.cell_count();
        let mut centers = Vec::with_capacity(n * 3);
        let mut corners = Vec::with_capacity(n * 6 * 3);
        let mut corner_counts = Vec::with_capacity(n);

        for i in 0..n {
            let c = self.centers[i].as_vec();
            centers.push(c.x as f32);
            centers.push(c.y as f32);
            centers.push(c.z as f32);

            let cs = self.corners_of(i);
            corner_counts.push(cs.len() as u8);
            for k in 0..6 {
                if k < cs.len() {
                    corners.push(cs[k].x as f32);
                    corners.push(cs[k].y as f32);
                    corners.push(cs[k].z as f32);
                } else {
                    // 五边形填充重复的最后一个角点
                    let last = cs.last().copied().unwrap_or(c);
                    corners.push(last.x as f32);
                    corners.push(last.y as f32);
                    corners.push(last.z as f32);
                }
            }
        }

        PreviewMesh {
            cell_count: n as u32,
            centers,
            corners,
            corner_counts,
        }
    }
}#[cfg(test)]
mod tests {
    use super::*;

    fn unit_rot() -> (DVec3, f64) {
        (DVec3::Z, 0.0)
    }

    #[test]
    fn cell_count_matches_formula() {
        for f in [1u32, 2, 3, 4, 8, 16] {
            let grid = SphereHexGrid::build(f, 6_371_000.0, unit_rot());
            assert_eq!(grid.cell_count(), (10 * f as usize * f as usize + 2) as usize);
        }
    }

    #[test]
    fn exactly_twelve_pentagons() {
        for f in [2u32, 4, 8, 16, 32] {
            let grid = SphereHexGrid::build(f, 6_371_000.0, unit_rot());
            assert_eq!(grid.pentagons().len(), 12, "freq {}", f);
        }
    }

    #[test]
    fn all_cells_have_5_or_6_neighbors() {
        for f in [2u32, 8, 16, 32] {
            let grid = SphereHexGrid::build(f, 6_371_000.0, unit_rot());
            for i in 0..grid.cell_count() {
                let n = grid.neighbor_count(i);
                assert!(n == 5 || n == 6, "freq {} cell {} has {}", f, i, n);
            }
        }
    }

    #[test]
    fn adjacency_is_symmetric() {
        let grid = SphereHexGrid::build(16, 6_371_000.0, unit_rot());
        for i in 0..grid.cell_count() {
            for &nb in grid.neighbors(i) {
                assert!(grid.neighbors(nb as usize).contains(&(i as u32)));
            }
        }
    }

    #[test]
    fn neighbor_distances_are_uniform() {
        // 最关键的一致性测试：所有邻居距离应该接近同一格宽。
        // 如果位置计算在接缝处不一致，这里会立刻暴露。
        for f in [8u32, 16, 32] {
            let grid = SphereHexGrid::build(f, 6_371_000.0, unit_rot());
            let ideal = (4.0 * PI / grid.cell_count() as f64).sqrt();
            for i in 0..grid.cell_count() {
                for &nb in grid.neighbors(i) {
                    let d = grid.center(i).angle_to(&grid.center(nb as usize));
                    let ratio = d / ideal;
                    assert!(
                        (0.6..1.6).contains(&ratio),
                        "freq {} cell {}->{} ratio {:.3} (d={:.5}, ideal={:.5})",
                        f, i, nb, ratio, d, ideal
                    );
                }
            }
        }
    }

    #[test]
    fn centers_are_unit_length() {
        let grid = SphereHexGrid::build(8, 6_371_000.0, unit_rot());
        for c in grid.centers() {
            assert!((c.as_vec().length() - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn total_area_equals_sphere_area() {
        let radius = 6_371_000.0f64;
        for f in [8u32, 16, 32] {
            let grid = SphereHexGrid::build(f, radius, unit_rot());
            let total: f64 = (0..grid.cell_count()).map(|i| grid.area_m2(i)).sum();
            let sphere = 4.0 * PI * radius * radius;
            assert!((total - sphere).abs() / sphere < 1e-6, "freq {}", f);
        }
    }

    #[test]
    fn cell_areas_are_uniform() {
        let grid = SphereHexGrid::build(32, 6_371_000.0, unit_rot());
        let stats = grid.stats();
        assert!(
            stats.area_ratio < 1.4,
            "hexagon area ratio {} (min {} max {} km2)",
            stats.area_ratio, stats.min_cell_area_km2, stats.max_cell_area_km2
        );
    }

    #[test]
    fn pentagons_are_smaller_than_hexagons() {
        let grid = SphereHexGrid::build(16, 6_371_000.0, unit_rot());
        let pent = grid.area_m2(grid.pentagons()[0] as usize);
        let hex = grid.area_m2((0..grid.cell_count()).find(|&i| !grid.is_pentagon(i)).unwrap());
        assert!(pent < hex, "pentagon {} vs hexagon {}", pent, hex);
    }

    #[test]
    fn cell_at_finds_exact_center() {
        let grid = SphereHexGrid::build(8, 6_371_000.0, unit_rot());
        for probe in [0usize, 10, 50, 100] {
            assert_eq!(grid.cell_at(grid.center(probe)), probe);
        }
    }

    #[test]
    fn rotation_preserves_topology() {
        let a = SphereHexGrid::build(8, 6_371_000.0, (DVec3::Z, 0.0));
        let b = SphereHexGrid::build(8, 6_371_000.0, (DVec3::X, 0.7));
        assert_eq!(a.cell_count(), b.cell_count());
        for i in 0..a.cell_count() {
            assert_eq!(a.neighbors(i), b.neighbors(i), "cell {}", i);
        }
    }

    #[test]
    fn tri_point_index_covers_range() {
        let freq = 8u32;
        let per = ((freq + 1) * (freq + 2) / 2) as usize;
        let mut seen = vec![false; per];
        for i in 0..=freq {
            for j in 0..=(freq - i) {
                let idx = tri_point_index(freq, i, j) as usize;
                assert!(idx < per);
                assert!(!seen[idx], "dup index for ({},{})", i, j);
                seen[idx] = true;
            }
        }
        assert!(seen.iter().all(|&x| x));
    }

    #[test]
    fn position_is_pure_function_of_key() {
        // 同一个键在两次调用中必须给出完全相同的位置。
        let freq = 8u32;
        let verts = icosahedron_vertices();
        for ki in 1..freq {
            let k = face_point_key(0, FACES[0], freq, ki, 0, freq - ki);
            let p1 = position_for_key(k, freq, &verts);
            let p2 = position_for_key(k, freq, &verts);
            assert!((p1 - p2).length() < 1e-15);
        }
        // 顶点 a 对应 (i=0, j=0, k=freq)
        let v1 = position_for_key(PointKey::Vertex(0), freq, &verts);
        let v2 = position_for_key(face_point_key(0, FACES[0], freq, 0, 0, freq), freq, &verts);
        assert!((v1 - v2).length() < 1e-15, "vertex a mismatch: {:?} vs {:?}", v1, v2);
        // 顶点 b 对应 (i=freq, j=0, k=0)
        let vb1 = position_for_key(PointKey::Vertex(11), freq, &verts);
        let vb2 = position_for_key(face_point_key(0, FACES[0], freq, freq, 0, 0), freq, &verts);
        assert!((vb1 - vb2).length() < 1e-15, "vertex b mismatch: {:?} vs {:?}", vb1, vb2);

    }
}


