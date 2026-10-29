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

use crate::fields::{BoundaryCell, BoundaryKind, BoundarySubtype, WorldFields};

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
        let strength = (b.normal_velocity.abs() / 120.0).clamp(0.0, 1.0);

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
    let spread = spread_from_boundaries(grid, &delta, &weight, 6);

    fields.elevation.par_iter_mut().enumerate().for_each(|(i, e)| {
        *e += spread[i];
    });
}

/// 把边界处的形变按距离扩散若干圈，产生有宽度的地貌带。
///
/// 每一圈强度按高斯衰减。造山带能向外扩出一两百公里，海沟则很窄。
fn spread_from_boundaries<G: GridOps>(
    grid: &G,
    delta: &[f32],
    weight: &[f32],
    rings: u32,
) -> Vec<f32> {
    let n = grid.cell_count();

    // 初始：边界格子自身的形变值。
    let mut current = vec![0.0f32; n];
    let mut visited = vec![false; n];
    for i in 0..n {
        if weight[i] > 0.0 {
            current[i] = delta[i] / weight[i];
            visited[i] = true;
        }
    }

    // 每一圈的贡献单独累加，每圈都重新从初始值算，
    // 而不是把上一圈的结果再传下去。
    //
    // 之前的实现用「上一圈的值 × 衰减」再传下一圈，
    // 同时在 result 上累加，导致同一个格子的形变被计入多次，
    // 6 圈之后数值放大到 4 万米量级——这就是高程范围爆炸的原因。
    let mut result = current.clone();

    for ring in 1..=rings {
        let decay = (-(ring as f32).powi(2) / 6.0).exp();
        let mut ring_delta: std::collections::HashMap<u32, (f32, f32)> =
            std::collections::HashMap::new();

        // 只从「上一圈刚刚触及的格子」向外一圈，
        // 但值取的是**原始边界值**乘衰减，不是逐圈连乘。
        for i in 0..n {
            if !visited[i] && ring > 1 {
                continue;
            }
            let base = current[i];
            if base.abs() < 1e-3 {
                continue;
            }
            for &nb in grid.neighbors(i) {
                if visited[nb as usize] {
                    continue;
                }
                let entry = ring_delta.entry(nb).or_insert((0.0, 0.0));
                entry.0 += base * decay;
                entry.1 += decay;
            }
        }

        if ring_delta.is_empty() {
            break;
        }

        for (cell, (sum, w)) in ring_delta {
            if w > 0.0 {
                let v = sum / w * 0.6;
                result[cell as usize] += v;
                visited[cell as usize] = true;
            }
        }
    }

    // 最终夹到合理范围。构造地貌的总幅度不应该超过 ±6 km。
    for v in result.iter_mut() {
        *v = v.clamp(-6_000.0, 6_000.0);
    }

    result
}


/// 火山弧：在海沟向陆侧 100-150 km 处生成弧形山脊。
pub fn apply_volcanic_arcs<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    boundaries: &[BoundaryCell],
    params: &LandformParams,
) {
    let n = grid.cell_count();
    // 找出所有汇聚型边界作为海沟位置
    let trenches: Vec<u32> = boundaries
        .iter()
        .filter(|b| {
            matches!(
                b.subtype,
                BoundarySubtype::OceanOceanConvergent | BoundarySubtype::OceanContinentConvergent
            )
        })
        .map(|b| b.cell)
        .collect();

    if trenches.is_empty() {
        return;
    }

    // 从海沟往外扩 2-3 环，形成弧
    let mut arc_boost = vec![0.0f32; n];
    let target_rings = 3u32;
    let mut frontier: Vec<(u32, u32)> = trenches.iter().map(|&c| (c, 0u32)).collect();
    let ring_h = [0.0f32, 0.5, 1.0, 0.7];

    while !frontier.is_empty() {
        let mut next: Vec<(u32, u32)> = Vec::new();
        for &(cell, ring) in &frontier {
            if ring >= target_rings {
                continue;
            }
            let h = ring_h[(ring + 1) as usize] * 1400.0 * params.arc_strength as f32;
            for &nb in grid.neighbors(cell as usize) {
                if arc_boost[nb as usize] == 0.0 {
                    arc_boost[nb as usize] = h;
                    next.push((nb, ring + 1));
                }
            }
        }
        frontier = next;
    }

    fields.elevation.par_iter_mut().enumerate().for_each(|(i, e)| {
        *e += arc_boost[i];
    });
}

/// 热点火山岛链。
///
/// 板块在移动而热点不动，所以自然长出岛链（夏威夷模式）。
/// 板块移动快就岛链稀疏，慢就密集——这是纯粹的涌现行为。
pub fn apply_hotspots<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    hotspots: &[tf_core::geo::UnitVec3],
    params: &LandformParams,
) {
    let n = grid.cell_count();
    if hotspots.is_empty() {
        return;
    }

    fields.elevation.par_iter_mut().enumerate().for_each(|(i, e)| {
        let p = grid.center_dir(i);
        let mut boost = 0.0f32;
        for &hs in hotspots {
            let angle = p.angle_to(&hs);
            // 火山半径约 2 个格宽
            let cell_angle = (4.0 * std::f64::consts::PI / n as f64).sqrt();
            let r = angle / (cell_angle * 2.5);
            if r < 1.0 {
                let h = (1.0 - r * r) * 2400.0 * params.hotspot_strength;
                boost += h as f32;
            }
        }
        *e += boost;
    });
}

/// 生成热点位置。
pub fn generate_hotspots(count: u32, rng: &mut tf_core::DeterministicRng) -> Vec<tf_core::geo::UnitVec3> {
    (0..count)
        .map(|_| {
            // 热点大多在洋壳上，用球面均匀分布即可
            rng.unit_sphere()
        })
        .collect()
}