//! 降尺度继承：子图如何从父图获得边界条件。
//!
//! # 为什么需要这个
//!
//! 如果子图只是换一个种子重新跑物理模拟，那它和父图上对应的区域
//! 毫无关系——放大之后海岸线位置、山脉走向全都会变。
//! 用户看到的就是「换了一张图」，这是致命的。
//!
//! # 做法
//!
//! 子图的每个格子，从父图采样对应的物理量作为「目标值」，
//! 然后用软约束把物理模拟的结果拉向这个目标：
//!
//! ```text
//! h_new = h_physics + λ·(h_target − h_physics)
//! ```
//!
//! λ 是约束强度：
//! - 边界附近 λ=1，完全服从父图（保证无缝）
//! - 内部逐渐衰减到 0.3，允许子层长出新的细节
//!
//! 这是气候学里 GCM → RCM 降尺度的标准思路。

use tf_grid::{GridOps, PatchGrid, SphereHexGrid};

/// A missing parent cell is an error, never an invented ocean or default climate.
pub fn sample_from_grid<C: GridOps>(
    parent: &tf_grid::MapGrid,
    child: &C,
    elevation: &[f32],
    temperature: &[f32],
    precipitation: &[f32],
) -> Result<ParentSample, String> {
    if [elevation.len(), temperature.len(), precipitation.len()]
        .iter()
        .any(|&n| n != parent.cell_count())
    {
        return Err("父图字段与网格尺寸不一致".into());
    }
    let mut sample = ParentSample {
        elevation: Vec::new(),
        temperature: Vec::new(),
        precipitation: Vec::new(),
        is_land: Vec::new(),
    };
    sample.elevation = interpolate_from_grid(parent, child, elevation)?;
    sample.temperature = interpolate_from_grid(parent, child, temperature)?;
    sample.precipitation = interpolate_from_grid(parent, child, precipitation)?;
    sample.is_land = sample.elevation.iter().map(|&v| v > 0.).collect();
    Ok(sample)
}

pub fn interpolate_from_grid<C: GridOps>(
    parent: &tf_grid::MapGrid,
    child: &C,
    elevation: &[f32],
) -> Result<Vec<f32>, String> {
    if elevation.len() != parent.cell_count() {
        return Err("父图高程尺寸不一致".into());
    }
    (0..child.cell_count())
        .map(|i| {
            let dir = child.center_dir(i);
            let cell = parent.cell_at(dir).ok_or("子图超出父图范围")?;
            let mut sum = 0.0;
            let mut weights = 0.0;
            for j in std::iter::once(cell).chain(parent.neighbors(cell).iter().map(|&j| j as usize))
            {
                let d = if let tf_grid::MapGrid::Plane(g) = parent {
                    let (x, y) = g.project(dir);
                    let (a, b) = g.xy(j);
                    (x - a).hypot(y - b)
                } else {
                    dir.angle_to(&parent.center_dir(j)) * parent.radius_m()
                };
                if d < 1e-12 {
                    return Ok(elevation[j]);
                }
                let w = 1.0 / (d * d);
                sum += elevation[j] as f64 * w;
                weights += w;
            }
            Ok((sum / weights) as f32)
        })
        .collect()
}

/// 父图上的一块采样结果。
pub struct ParentSample {
    /// 每个子图格子的父图高程。
    pub elevation: Vec<f32>,
    /// 每个子图格子的父图温度。
    pub temperature: Vec<f32>,
    /// 每个子图格子的父图降水。
    pub precipitation: Vec<f32>,
    /// 父图是否在这些位置是陆地。
    pub is_land: Vec<bool>,
}

/// 从父图采样到子图。
///
/// 对子图的每个格子，找到它在父图上对应的格子，取那边的物理量。
pub fn sample_from_parent(
    parent_grid: &SphereHexGrid,
    patch: &PatchGrid,
    parent_elevation: &[f32],
    parent_temperature: &[f32],
    parent_precipitation: &[f32],
) -> ParentSample {
    assert_eq!(parent_elevation.len(), parent_grid.cell_count());
    assert_eq!(parent_temperature.len(), parent_grid.cell_count());
    assert_eq!(parent_precipitation.len(), parent_grid.cell_count());
    let n = patch.cell_count();
    let mut elevation = Vec::with_capacity(n);
    let mut temperature = Vec::with_capacity(n);
    let mut precipitation = Vec::with_capacity(n);
    let mut is_land = Vec::with_capacity(n);

    for i in 0..n {
        let dir = patch.center_dir(i);
        let parent_cell = parent_grid.cell_at(dir);

        elevation.push(parent_elevation[parent_cell]);
        temperature.push(parent_temperature[parent_cell]);
        precipitation.push(parent_precipitation[parent_cell]);
        is_land.push(parent_elevation[parent_cell] > 0.0);
    }

    ParentSample {
        elevation,
        temperature,
        precipitation,
        is_land,
    }
}

/// 计算每个子图格子的约束强度 λ。
///
/// 规律：靠近补丁边界的地方约束强（完全服从父图），
/// 中心区域约束弱（允许自由生长）。
///
/// 这样保证子图和父图在接缝处严格一致，同时内部有新的细节。
pub fn constraint_weights<G: GridOps>(patch: &G) -> Vec<f32> {
    let (width, height) = patch.extent_m().unwrap_or((1., 1.));
    let band = (width.min(height) * 0.08).max(1e-6);
    (0..patch.cell_count())
        .map(|i| {
            if patch.boundary_mask()[i] {
                return 1.;
            }
            let (x, y) = patch.local_center_m(i).unwrap_or((0., 0.));
            let d = (width / 2. - x.abs()).min(height / 2. - y.abs()).max(0.);
            let t = (d / band).clamp(0., 1.);
            (1. - 0.7 * t * t * (3. - 2. * t)) as f32
        })
        .collect()
}

/// 把物理模拟的结果向父图拉拢。
///
/// 在子图生成完成后调用。对每个格子：
///   h_final = h_sim + λ·(h_parent − h_sim)
pub fn blend_with_parent<G: GridOps>(
    patch: &G,
    simulated: &mut [f32],
    parent: &ParentSample,
    weights: &[f32],
    // 强制的陆地/海洋一致性：如果父图是海，子图也必须是海
    enforce_land_mask: bool,
) {
    let n = patch.cell_count();
    for i in 0..n {
        let lambda = weights[i];
        let target = parent.elevation[i];
        let sim = simulated[i];

        let mut blended = sim + lambda * (target - sim);

        // 陆地/海洋必须一致，否则会出现「父图是海、子图是山」的矛盾。
        // 这里做硬约束：保留父图的分类，只用子图的细节调制高度。
        if enforce_land_mask {
            let parent_land = parent.is_land[i];
            let sim_land = blended > 0.0;
            if parent_land != sim_land {
                // 分类冲突，强行回到父图的分类
                if parent_land && blended <= 0.0 {
                    // 父图是陆，子图算成海：抬到略高于海平面
                    blended = 20.0;
                } else if !parent_land && blended > 0.0 {
                    // 父图是海，子图算成陆：压到海平面以下
                    blended = -20.0;
                }
            }
        }

        simulated[i] = blended;
    }
}

/// 生成一份高质量的父图上采样（用于子图尚未生成时的临时显示）。
///
/// 做法：双线性采样 + 从父图的曲率推导细节。
pub fn upsample_parent(
    parent_grid: &SphereHexGrid,
    patch: &PatchGrid,
    parent_elevation: &[f32],
) -> Vec<f32> {
    let n = patch.cell_count();
    let mut out = Vec::with_capacity(n);

    for i in 0..n {
        let dir = patch.center_dir(i);
        let cell = parent_grid.cell_at(dir);
        let base = parent_elevation.get(cell).copied().unwrap_or(0.0);

        // 从父图邻居的差异估算局部坡度，作为细节纹理
        let nb = parent_grid.neighbors(cell);
        if nb.is_empty() {
            out.push(base);
            continue;
        }

        let mut grad = 0.0f32;
        for &j in nb {
            if let Some(&h) = parent_elevation.get(j as usize) {
                grad += h - base;
            }
        }
        grad /= nb.len() as f32;

        // 细节幅度正比于父图的局部起伏
        out.push(base + grad * 0.15);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tf_grid::SphericalPatch;

    fn make_grids() -> (SphereHexGrid, PatchGrid) {
        let parent = SphereHexGrid::build(64, 6_371_000.0, (glam::DVec3::Z, 0.0));
        let patch_spec = SphericalPatch::new(0.0, 0.0, 2_000_000.0, 32);
        let patch = PatchGrid::new(&patch_spec);
        (parent, patch)
    }

    #[test]
    fn sign_conflicts_clamp_to_the_coast_instead_of_reflecting_mountains() {
        let patch = PatchGrid::new(&tf_grid::SphericalPatch::new(0., 0., 4000., 4));
        let mut simulated = vec![3000.; 16];
        let mut parent = ParentSample {
            elevation: vec![-10.; 16],
            temperature: vec![0.; 16],
            precipitation: vec![0.; 16],
            is_land: vec![false; 16],
        };
        blend_with_parent(&patch, &mut simulated, &parent, &[0.; 16], true);
        assert_eq!(simulated, vec![-20.; 16]);
        simulated.fill(-3000.);
        parent.is_land.fill(true);
        blend_with_parent(&patch, &mut simulated, &parent, &[0.; 16], true);
        assert_eq!(simulated, vec![20.; 16]);
    }
    #[test]
    fn constraint_is_strong_at_edges_weak_inside() {
        let (_, patch) = make_grids();
        let w = constraint_weights(&patch);
        let size = patch.size() as usize;

        // 角落应该接近 1
        assert!(w[0] > 0.95, "corner weight {}", w[0]);
        // 中心应该接近 0.3
        let center = (size / 2) * size + size / 2;
        assert!(w[center] < 0.45, "center weight {}", w[center]);
    }

    #[test]
    fn weights_are_monotonic_from_edge_to_center() {
        let (_, patch) = make_grids();
        let w = constraint_weights(&patch);
        let size = patch.size() as usize;
        let mid_row = size / 2;
        // 沿中间一行从左边界走到中心，权重应该递减
        let mut prev = w[mid_row * size];
        for col in 1..size / 2 {
            let cur = w[mid_row * size + col];
            assert!(cur <= prev + 1e-6, "not monotonic at col {}", col);
            prev = cur;
        }
    }

    #[test]
    fn blend_pulls_toward_parent() {
        let (_, patch) = make_grids();
        let n = patch.cell_count();
        let weights = constraint_weights(&patch);

        // 模拟结果全是 5000，父图全是 0
        let mut sim = vec![5000.0f32; n];
        let parent = ParentSample {
            elevation: vec![0.0; n],
            temperature: vec![15.0; n],
            precipitation: vec![700.0; n],
            is_land: vec![false; n],
        };

        blend_with_parent(&patch, &mut sim, &parent, &weights, false);

        // 角落应该被拉到接近 0（父图是海，所以可能被压成略负的值）
        assert!(sim[0].abs() < 500.0, "corner {}", sim[0]);
        // 中心保留更多原来的值
        let size = patch.size() as usize;
        let center = (size / 2) * size + size / 2;
        // This test isolates soft blending; coastal sign conflicts are tested separately.
        assert!(sim[center].abs() > 3000.0, "center {}", sim[center]);
    }

    #[test]
    fn land_mask_is_enforced() {
        let (_, patch) = make_grids();
        let n = patch.cell_count();
        let weights = vec![0.0; n]; // 不加软约束

        let mut sim = vec![1000.0f32; n]; // 全是陆地
        let parent = ParentSample {
            elevation: vec![-2000.0; n],
            temperature: vec![15.0; n],
            precipitation: vec![700.0; n],
            is_land: vec![false; n], // 父图全是海
        };

        blend_with_parent(&patch, &mut sim, &parent, &weights, true);

        // 所有格子都该变成海
        for (i, &v) in sim.iter().enumerate() {
            assert!(v <= 0.0, "cell {} still land: {}", i, v);
        }
    }

    #[test]
    fn upsample_produces_reasonable_values() {
        let (parent, patch) = make_grids();
        let n = parent.cell_count();
        let parent_elev: Vec<f32> = (0..n)
            .map(|i| {
                let lat = parent.latitude_deg(i);
                (lat * 100.0) as f32
            })
            .collect();

        let up = upsample_parent(&parent, &patch, &parent_elev);
        assert_eq!(up.len(), patch.cell_count());
        // 赤道附近应该接近 0
        for v in up.iter().take(100) {
            assert!(v.abs() < 3000.0, "unreasonable {}", v);
        }
    }
}
