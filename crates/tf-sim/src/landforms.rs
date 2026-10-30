//! 阶段 2：构造地貌。
//!
//! 阶段 1 给出的是地壳厚度场，这一步把它变成具体的地形起伏。
//! 六种边界类型各自产生不同形态的地形：
//!
//! - 洋中脊：热沉降让洋底从脊向两侧变深
//! - 海沟：俯冲板块切出的深槽
//! - 火山弧：海沟向陆侧 100-150 km 的弧形山脊
//! - 造山带：陆陆碰撞产生平行于边界的褶皱山脉
//! - 裂谷：陆陆分离处下沉，两侧有肩部隆起
//! - 转换断层：走滑，地形起伏小但会切出线性特征

use rayon::prelude::*;
use tf_grid::GridOps;

use crate::fields::{BoundaryCell, BoundarySubtype, WorldFields};

/// 构造地貌参数。
#[derive(Debug, Clone, Copy)]
pub struct LandformParams {
    /// 造山强度倍率。
    pub orogeny_strength: f64,
    /// 海沟深度倍率。
    pub trench_strength: f64,
    /// 火山弧高度倍率。
    pub arc_strength: f64,
    /// 热点强度。
    pub hotspot_strength: f64,
}

impl Default for LandformParams {
    fn default() -> Self {
        Self {
            orogeny_strength: 1.0,
            trench_strength: 1.0,
            arc_strength: 1.0,
            hotspot_strength: 1.0,
        }
    }
}

/// 沿边界施加构造地貌。
pub fn apply_boundary_landforms<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    boundaries: &[BoundaryCell],
    params: &LandformParams,
) {
    let n = grid.cell_count();
    let mut delta = vec![0.0f32; n];
    let mut weight = vec![0.0f32; n];

    for b in boundaries {
        let i = b.cell as usize;
        // 地球实测：海沟相对洋底约 -6 km，喜马拉雅约 +5 km。
        // 这里的幅度按物理量级设定，不做夸张。
        let strength = (b.normal_velocity.abs() / 100_000.0).clamp(0.0, 1.0);

        match b.subtype {
            BoundarySubtype::OceanOceanDivergent => {
                // 洋中脊：把边界处抬升成脊。热沉降已在别处处理，这里只加脊的隆起。
                delta[i] += (700.0 * strength) as f32;
                weight[i] += 1.0;
            }
            BoundarySubtype::OceanOceanConvergent => {
                // 海沟：深槽
                delta[i] -= (3200.0 * strength * params.trench_strength) as f32;
                weight[i] += 1.0;
            }
            BoundarySubtype::OceanContinentConvergent => {
                // 海沟 + 陆缘弧
                delta[i] -= (2800.0 * strength * params.trench_strength) as f32;
                weight[i] += 1.0;
            }
            BoundarySubtype::ContinentContinentConvergent => {
                // 造山带：大幅抬升
                delta[i] += (2600.0 * strength * params.orogeny_strength) as f32;
                weight[i] += 1.0;
            }
            BoundarySubtype::ContinentContinentDivergent => {
                // 裂谷：下沉
                delta[i] -= (1200.0 * strength) as f32;
                weight[i] += 1.0;
            }
            BoundarySubtype::Transform => {
                // 走滑：小幅起伏
                weight[i] += 1.0;
            }
        }
    }

    // 把边界效应沿距离衰减扩散出去，形成有宽度地貌带
    let sources: Vec<f32> = delta
        .iter()
        .zip(&weight)
        .map(|(&v, &w)| if w > 0. { v / w } else { 0. })
        .collect();
    let spread = spread_by_distance(grid, &sources, 90_000.0);

    fields
        .elevation
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, e)| {
            *e += spread[i];
        });
}

/// 把边界处的形变按距离扩散若干圈，产生有宽度的地貌带。
///
/// 每一圈强度按高斯衰减。造山带能向外扩出一两百公里，海沟则很窄。
pub fn spread_by_distance<G: GridOps>(grid: &G, sources: &[f32], sigma_m: f64) -> Vec<f32> {
    let (distance, origin) = nearest_sources(grid, sources, sigma_m * 3.);
    (0..grid.cell_count())
        .map(|i| {
            if origin[i] == usize::MAX {
                0.
            } else {
                sources[origin[i]] * (-0.5 * (distance[i] / sigma_m).powi(2)).exp() as f32
            }
        })
        .collect()
}
pub(crate) fn nearest_sources<G: GridOps>(
    grid: &G,
    sources: &[f32],
    cutoff: f64,
) -> (Vec<f64>, Vec<usize>) {
    use std::{cmp::Ordering, collections::BinaryHeap};
    #[derive(PartialEq)]
    struct Step(f64, usize, usize);
    impl Eq for Step {}
    impl PartialOrd for Step {
        fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Step {
        fn cmp(&self, o: &Self) -> Ordering {
            o.0.total_cmp(&self.0)
                .then_with(|| o.1.cmp(&self.1))
                .then_with(|| o.2.cmp(&self.2))
        }
    }
    let n = grid.cell_count();
    let mut distance = vec![f64::INFINITY; n];
    let mut origin = vec![usize::MAX; n];
    let mut heap = BinaryHeap::new();
    for i in 0..n {
        if sources[i].abs() > 1e-6 {
            distance[i] = 0.;
            origin[i] = i;
            heap.push(Step(0., i, i));
        }
    }
    while let Some(Step(d, i, source)) = heap.pop() {
        if d > distance[i] + 1e-8 || source != origin[i] {
            continue;
        }
        for &j in grid.neighbors(i) {
            let j = j as usize;
            let nd = d + grid.distance_m(i, j);
            if nd > cutoff {
                continue;
            }
            if nd < distance[j] - 1e-8 || ((nd - distance[j]).abs() < 1e-8 && source < origin[j]) {
                distance[j] = nd;
                origin[j] = source;
                heap.push(Step(nd, j, source));
            }
        }
    }
    (distance, origin)
}

/// 火山弧：在海沟向陆侧 100-150 km 处生成弧形山脊。
pub fn apply_volcanic_arcs<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    boundaries: &[BoundaryCell],
    params: &LandformParams,
) {
    let n = grid.cell_count();
    let mut sources = vec![0f32; n];
    for b in boundaries {
        let i = b.cell as usize;
        let j = b.other as usize;
        let (source, amplitude) = match b.subtype {
            BoundarySubtype::OceanContinentConvergent => {
                let land = if fields.crust_type[i] >= fields.crust_type[j] {
                    i
                } else {
                    j
                };
                (land, 1400.)
            }
            BoundarySubtype::OceanOceanConvergent => {
                let overriding = if fields.crust_age[i] <= fields.crust_age[j] {
                    i
                } else {
                    j
                };
                (overriding, 6000.)
            }
            _ => continue,
        };
        let strength = (b.normal_velocity.abs() / 100_000.).clamp(0., 1.);
        sources[source] = sources[source].max((amplitude * strength * params.arc_strength) as f32);
    }
    let (distance, origin) = nearest_sources(grid, &sources, 260_000.);
    fields
        .elevation
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, e)| {
            let source = origin[i];
            if source == usize::MAX || fields.plate_id[i] != fields.plate_id[source] {
                return;
            }
            let arc = (-0.5 * ((distance[i] - 120_000.) / 40_000.).powi(2)).exp();
            *e += (arc * sources[source] as f64) as f32;
        });
}

/// 热点火山岛链。
///
/// 当前近似只生成静止火山盾，尚未模拟板块平移形成的岛链。
pub fn apply_hotspots<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    hotspots: &[tf_core::geo::UnitVec3],
    params: &LandformParams,
) {
    if hotspots.is_empty() {
        return;
    }

    fields
        .elevation
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, e)| {
            let p = grid.center_dir(i);
            let mut boost = 0.0f32;
            for &hs in hotspots {
                let angle = p.angle_to(&hs);
                // Approximate 150 km shield footprint, independent of cell count.
                let r = angle * grid.radius_m() / 150_000.;
                if r < 1.0 {
                    let h = (1.0 - r * r) * 2400.0 * params.hotspot_strength;
                    boost += h as f32;
                }
            }
            *e += boost;
        });
}

/// 生成热点位置。
pub fn generate_hotspots(
    count: u32,
    rng: &mut tf_core::DeterministicRng,
) -> Vec<tf_core::geo::UnitVec3> {
    (0..count)
        .map(|_| {
            // 热点大多在洋壳上，用球面均匀分布即可
            rng.unit_sphere()
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ocean_convergence_can_form_an_island_arc_on_the_overriding_plate() {
        let grid = tf_grid::HexGrid2D::new(32, 32, 10000., 0., 0.);
        let mut fields = WorldFields::with_capacity(1024);
        fields.elevation.fill(-4500.);
        let i = 16 * 32 + 2;
        let j = i - 1;
        fields.plate_id[j] = 1;
        fields.crust_age[j] = 100.;
        let boundary = BoundaryCell {
            cell: i as u32,
            other: j as u32,
            kind: crate::fields::BoundaryKind::Convergent,
            subtype: BoundarySubtype::OceanOceanConvergent,
            normal_velocity: 100000.,
            tangential_velocity: 0.,
            normal: [1., 0., 0.],
        };
        apply_volcanic_arcs(&grid, &mut fields, &[boundary], &LandformParams::default());
        assert!(fields.elevation.iter().any(|&e| e > 0.));
        assert_eq!(fields.elevation[j], -4500.);
    }
}
