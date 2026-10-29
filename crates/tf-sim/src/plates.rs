//! 阶段 1：地幔对流与板块构造。
//!
//! 真实的板块驱动力来自地幔对流。完整求解要解 Navier-Stokes，太重。
//! 这里用「运动学近似 + 统计正确」的模型：在球面上撒一批对流胞，
//! 用径向基函数叠加出速度场，再让板块顺着这个场漂移。
//!
//! 这个模型抓住了真实对流的关键定性特征——上升流聚成线状、下降流也聚成
//! 线状、两者交替分布。这正好对应洋中脊与海沟的格局。

use glam::DVec3;
use rayon::prelude::*;
use std::collections::HashMap;
use tf_core::geo::UnitVec3;
use tf_core::params::GenerationParams;
use tf_core::DeterministicRng;
use tf_grid::GridOps;

use crate::fields::{BoundaryCell, BoundaryKind, BoundarySubtype, Plate, WorldFields};

/// 一个地幔对流胞。
#[derive(Debug, Clone, Copy)]
pub struct ConvectionCell {
    pub center: UnitVec3,
    /// 正值是上升流，负值是下降流。
    pub polarity: f64,
    /// 影响半径（弧度）。
    pub sigma: f64,
}

/// 板块构造的中间结果。
#[derive(Debug, Clone)]
pub struct TectonicState {
    pub plates: Vec<Plate>,
    pub convection: Vec<ConvectionCell>,
}

/// 生成对流胞。
fn build_convection_cells(rng: &mut DeterministicRng, count: usize) -> Vec<ConvectionCell> {
    let base = tf_core::geo::fibonacci_sphere(count);
    let cell_angle = (4.0 * std::f64::consts::PI / count as f64).sqrt();
    base.into_iter()
        .map(|c| {
            let jitter_angle = rng.range_f64(0.0, 0.35);
            let bearing = rng.range_f64(0.0, std::f64::consts::TAU);
            let center = c.offset(bearing, jitter_angle * cell_angle);
            ConvectionCell {
                center,
                polarity: if rng.next_f64() < 0.5 { 1.0 } else { -1.0 } * rng.range_f64(0.5, 1.5),
                sigma: cell_angle * rng.range_f64(0.6, 1.2),
            }
        })
        .collect()
}

/// 计算某点的地幔对流速度场。
fn mantle_velocity(p: UnitVec3, cells: &[ConvectionCell]) -> (DVec3, f64) {
    let pv = p.as_vec();
    let (east, _north) = p.tangent_frame();
    let mut v = DVec3::ZERO;
    for cell in cells {
        let angle = p.angle_to(&cell.center);
        if angle > cell.sigma * 4.0 {
            continue;
        }
        let w = (-angle * angle / (2.0 * cell.sigma * cell.sigma)).exp() * cell.polarity;
        let dir = if angle > 1e-9 {
            let axis = cell.center.as_vec().cross(pv);
            if axis.length() < 1e-12 {
                east
            } else {
                pv.cross(axis.normalize()).normalize()
            }
        } else {
            east
        };
        v += dir * w;
    }
    let strength = v.length();
    if strength < 1e-12 {
        (east, 0.0)
    } else {
        (v / strength, strength)
    }
}

/// 建立板块并计算初始欧拉极。
pub fn build_plates<G: GridOps>(
    grid: &G,
    params: &GenerationParams,
    rng: &mut DeterministicRng,
) -> (TectonicState, Vec<u16>) {
    let n = grid.cell_count();
    let plate_count = params.plate_count as usize;

    let seeds: Vec<UnitVec3> = tf_core::geo::fibonacci_sphere(plate_count)
        .into_iter()
        .map(|c| {
            let bearing = rng.range_f64(0.0, std::f64::consts::TAU);
            c.offset(bearing, rng.range_f64(0.0, 0.25))
        })
        .collect();

    let convection = build_convection_cells(rng, plate_count * 4);

    // 板块划分：球面 Voronoi。
    //
    // 关键：纯 Voronoi 会在二十面体细分的格网上留下明显的三角纹理，
    // 因为种子点的分布和格网结构之间存在关联。
    // 加上一层噪声扰动边界距离，让板块边界变成自然的不规则曲线。
    let warp_noise = tf_core::noise::SimplexNoise::new(params.seed ^ 0xB0A7D);
    let plate_id: Vec<u16> = (0..n)
        .into_par_iter()
        .map(|i| {
            let c = grid.center_dir(i);
            let cv = c.as_vec();
            // 低频噪声扰动：让边界蜿蜒而不是直线
            let warp = warp_noise.fbm(cv.x * 3.5, cv.y * 3.5, cv.z * 3.5, 3, 2.0, 0.5) * 0.06;
            let mut best = 0usize;
            let mut best_score = f64::NEG_INFINITY;
            for (k, s) in seeds.iter().enumerate() {
                let mut score = cv.dot(s.as_vec());
                // 对每个种子用不同相位的噪声，扰动它的「势力范围」
                let phase = k as f64 * 1.7;
                score += warp * ((cv.x + phase).sin() + (cv.y - phase).cos()) * 0.5;
                if score > best_score {
                    best_score = score;
                    best = k;
                }
            }
            best as u16
        })
        .collect();

    // 决定哪些板块是陆壳。陆地占比由目标值反推。
    // 关键：陆壳板块的地壳厚度（35 km）显著大于洋壳（7 km），
    // 均摊到全星球时，陆壳板块覆盖的面积约等于陆地占比。
    let mut order: Vec<usize> = (0..plate_count).collect();
    for i in (1..plate_count).rev() {
        let j = rng.below(i as u32 + 1) as usize;
        order.swap(i, j);
    }
    // Continental crust also includes submerged shelves. Keep its coverage
    // above target land coverage, so sea level crosses continuous continental
    // topography rather than the gap between oceanic and continental crust.
    let mut counts = vec![0usize; plate_count];
    for &id in &plate_id {
        counts[id as usize] += 1;
    }
    let desired = (params.land_fraction * 1.45 + 0.04).min(0.95) * n as f64;
    let mut coverage = 0usize;
    let mut continental_count = 0;
    while continental_count < plate_count && (coverage as f64) < desired {
        coverage += counts[order[continental_count]];
        continental_count += 1;
    }

    let plates: Vec<Plate> = (0..plate_count)
        .map(|k| {
            let center = seeds[k];
            let (dir, strength) = mantle_velocity(center, &convection);
            let axis = center.as_vec().cross(dir);
            let axis = if axis.length() < 1e-12 {
                DVec3::Z
            } else {
                axis.normalize()
            };
            let oceanic = !order[..continental_count].contains(&k);
            Plate {
                id: k as u16,
                euler_pole: [axis.x, axis.y, axis.z],
                // 5 cm/year = 50 km/Myr; omega = v/R is about 0.0078 rad/Myr.
                omega: (if oceanic { 1.2 } else { 0.7 })
                    * params.tectonic_activity
                    * (0.4 + strength)
                    * 7.8e-3,
                oceanic,
            }
        })
        .collect();

    (TectonicState { plates, convection }, plate_id)
}

/// 初始化地壳场。
///
/// 这是整个模型的物理起点：陆壳厚、洋壳薄，厚度差通过 Airy 均衡
/// 直接决定了大是大海还是大陆。
pub fn init_crust<G: GridOps>(
    grid: &G,
    plate_id: &[u16],
    state: &TectonicState,
    fields: &mut WorldFields,
    rng: &mut DeterministicRng,
    params: &GenerationParams,
) {
    let n = grid.cell_count();
    let plate_map: HashMap<u16, &Plate> = state.plates.iter().map(|p| (p.id, p)).collect();
    let interior_noise = tf_core::noise::SimplexNoise::new(params.seed ^ 0x1A7E);

    // 每个板块的基准厚度。加一点板块间差异，让大陆不是千篇一律的台地。
    let plate_thickness: Vec<f32> = state
        .plates
        .iter()
        .map(|p| {
            if p.oceanic {
                (7_000.0 + rng.range_f64(-800.0, 800.0)) as f32
            } else {
                // 陆壳基准 32-42 km，越厚抬得越高
                (36_000.0 + rng.range_f64(-3500.0, 3500.0)) as f32
            }
        })
        .collect();

    fields
        .crust_thickness
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, t)| {
            let p = plate_id[i] as usize;
            // 板块内部结构。
            //
            // 真实大陆不是一块平板，它有大尺度的克拉通（稳定高原）、
            // 裂谷盆地、以及被侵蚀削平的准平原。
            // 用三个尺度的噪声叠加来生成这种结构：
            //   - 超低频：决定这是一块高原区还是低地区
            //   - 中频：山间盆地与台地
            //   - 高频：局部起伏
            let c = grid.center_dir(i).as_vec();
            let oceanic = plate_map[&plate_id[i]].oceanic;

            if oceanic {
                // 洋壳：微弱起伏即可
                let d = interior_noise.fbm(c.x * 4.0, c.y * 4.0, c.z * 4.0, 2, 2.0, 0.5);
                *t = plate_thickness[p] + (d as f32) * 400.0;
            } else {
                // 陆壳：三层结构
                let macro_n = interior_noise.fbm(c.x * 1.1, c.y * 1.1, c.z * 1.1, 2, 2.0, 0.55);
                let meso_n = interior_noise.fbm(c.x * 3.2, c.y * 3.2, c.z * 3.2, 3, 2.0, 0.5);
                let micro_n = interior_noise.fbm(c.x * 9.0, c.y * 9.0, c.z * 9.0, 2, 2.0, 0.45);

                // 超低频振幅最大：决定克拉通 vs 盆地，±9 km 地壳厚度
                let macro_amp = (macro_n as f32) * 9000.0;
                let meso = (meso_n as f32) * 3500.0;
                let micro = (micro_n as f32) * 900.0;

                *t = plate_thickness[p] + macro_amp + meso + micro;
            }
        });

    fields
        .crust_type
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, t)| {
            *t = if plate_map[&plate_id[i]].oceanic {
                0.0
            } else {
                1.0
            };
        });

    fields.plate_id.copy_from_slice(plate_id);
    let _ = (n, params);
}

/// 计算板块的旋转速度向量（球面叉积）。
#[inline]
fn plate_velocity(plate: &Plate, p: DVec3) -> DVec3 {
    let axis = DVec3::new(
        plate.euler_pole[0],
        plate.euler_pole[1],
        plate.euler_pole[2],
    );
    axis.cross(p) * plate.omega
}

/// 识别板块边界并分类。
pub fn find_boundaries<G: GridOps>(
    grid: &G,
    plate_id: &[u16],
    state: &TectonicState,
    fields: &WorldFields,
) -> Vec<BoundaryCell> {
    let n = grid.cell_count();

    (0..n)
        .into_par_iter()
        .filter_map(|i| {
            let my_plate = plate_id[i] as usize;
            for &nb in grid.neighbors(i) {
                let other_plate = plate_id[nb as usize] as usize;
                if other_plate == my_plate {
                    continue;
                }
                let p = grid.center_dir(i).as_vec();
                let q = grid.center_dir(nb as usize).as_vec();

                let v1 = plate_velocity(&state.plates[my_plate], p);
                let v2 = plate_velocity(&state.plates[other_plate], q);
                let v_rel = v1 - v2;

                let mut normal = q - p;
                normal -= p * p.dot(normal);
                let nl = normal.length();
                if nl < 1e-9 {
                    continue;
                }
                let normal = normal / nl;
                let v_normal_m = v_rel.dot(normal) * grid.radius_m();

                // 阈值：板块角速度是弧度/百万年量级，乘半径后典型值
                // 在几十到几百米/百万年。取 8 米/百万年做分类边界。
                let threshold = 8.0;
                let kind = if v_normal_m > threshold {
                    BoundaryKind::Convergent
                } else if v_normal_m < -threshold {
                    BoundaryKind::Divergent
                } else {
                    BoundaryKind::Transform
                };

                let me_oceanic = fields.crust_type[i] < 0.5;
                let other_oceanic = fields.crust_type[nb as usize] < 0.5;
                let subtype = match (kind, me_oceanic, other_oceanic) {
                    (BoundaryKind::Convergent, true, true) => BoundarySubtype::OceanOceanConvergent,
                    (BoundaryKind::Convergent, _, _) => {
                        if me_oceanic == other_oceanic {
                            BoundarySubtype::ContinentContinentConvergent
                        } else {
                            BoundarySubtype::OceanContinentConvergent
                        }
                    }
                    (BoundaryKind::Divergent, true, true) => BoundarySubtype::OceanOceanDivergent,
                    (BoundaryKind::Divergent, _, _) => BoundarySubtype::ContinentContinentDivergent,
                    _ => BoundarySubtype::Transform,
                };

                return Some(BoundaryCell {
                    cell: i as u32,
                    other: nb,
                    kind,
                    subtype,
                    normal_velocity: v_normal_m,
                });
            }
            None
        })
        .collect()
}

/// 板块运动的一个迭代步：地壳增厚/减薄。
pub fn step_tectonics<G: GridOps>(
    grid: &G,
    state: &TectonicState,
    fields: &mut WorldFields,
    _params: &GenerationParams,
    dt_myr: f64,
) {
    let n = grid.cell_count();
    let plate_ids = fields.plate_id.clone();
    let boundaries = find_boundaries(grid, &plate_ids, state, fields);

    // 构造形变速率。
    //
    // 关键：取每条边界的影响力，但要避免同一格被多条边界反复加厚。
    // 真实造山带的地壳增厚是有限度的——喜马拉雅下面约 70 km，
    // 而不是无限叠加。所以这里取「最大值」而不是求和。
    let mut change = vec![0.0f32; n];
    for b in &boundaries {
        // 相对速度换算成「每百万年的增厚量」，单位米。
        // 系数做了标定：典型 50 km/Myr 的缩短分布在约 200 km 宽的造山带。
        let rate = (b.normal_velocity.abs() * 35_000.0 / 200_000.0) as f32;
        match b.kind {
            BoundaryKind::Convergent => {
                // 用 max 而不是 +=，避免边界密集处无限叠加
                change[b.cell as usize] = change[b.cell as usize].max(rate);
                change[b.other as usize] = change[b.other as usize].max(rate * 0.55);
            }
            BoundaryKind::Divergent => {
                change[b.cell as usize] = change[b.cell as usize].min(-rate * 0.35);
                change[b.other as usize] = change[b.other as usize].min(-rate * 0.35);
            }
            BoundaryKind::Transform => {}
        }
    }

    // 造山带要有宽度。真实山脉的隆起范围是数百公里，
    // 所以形变必须向两侧扩散若干圈，而不是只在边界一格上堆。
    let spread = crate::landforms::spread_by_distance(grid, &change, 100_000.0);

    let old = fields.crust_thickness.clone();
    let is_continental = fields.crust_type.clone();
    fields
        .crust_thickness
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, t)| {
            let delta = spread[i] * dt_myr as f32;
            // 只有陆壳能大幅增厚。洋壳被挤压时会俯冲，不会隆起成山。
            let eff = if is_continental[i] > 0.5 {
                delta
            } else {
                delta * 0.15
            };
            *t = (old[i] + eff).clamp(4_000.0, 78_000.0);
        });

    // 地壳类型由厚度决定：薄于 18 km 是洋壳，厚于 25 km 是陆壳
    let thick = fields.crust_thickness.clone();
    fields
        .crust_type
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, t)| {
            *t = ((thick[i] - 18_000.0) / 7_000.0).clamp(0.0, 1.0);
        });
}

/// Airy 均衡：把地壳厚度换成高程。
///
/// h = (t_c - t_ref) * (1 - rho_crust / rho_mantle)
///
/// 取 rho_crust = 2800、rho_mantle = 3300，系数约 0.1515。
/// 地壳从 7 km 到 40 km，高程差约 5000 m，正好是大洋盆地与大陆的量级。
pub fn apply_airy_isostasy<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    params: &GenerationParams,
) {
    use tf_core::constants::{CRUST_DENSITY, MANTLE_DENSITY};
    let _ = grid;
    let coeff = (1.0 - CRUST_DENSITY / MANTLE_DENSITY) as f32;

    // 参考厚度：取海平面处的等效地壳厚度。
    // 洋壳 7 km 应该在海平面以下约 4000 m，陆壳 36 km 应该在海平面以上。
    // 解方程 0 = (t_ref - t_ref)，海平面在 t_ref 处。取 t_ref 使陆壳占比合理。
    let t_ref = 20_000.0f32;

    fields
        .elevation
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, e)| {
            let thick = fields.crust_thickness[i];
            *e = (thick - t_ref) * coeff;
        });

    adjust_sea_level(grid, fields, params.land_fraction);
}

/// 调整海平面，使陆地占比接近目标值。
pub fn adjust_sea_level<G: GridOps>(grid: &G, fields: &mut WorldFields, target_land_fraction: f64) {
    let n = grid.cell_count();
    let mut sample: Vec<f32> = (0..n).map(|i| fields.elevation[i]).collect();
    sample.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let target = target_land_fraction.clamp(0.02, 0.95);
    let idx = (((1.0 - target) * n as f64).round() as usize).min(n - 1);
    let sea_level = sample[idx];

    fields.elevation.par_iter_mut().for_each(|e| {
        *e -= sea_level;
    });
    let _ = grid;
}

/// 计算洋壳年龄并施加热沉降。
///
/// 洋壳年龄转深度用半空间冷却模型（Parsons & Sclater 1977）：
///   age < 70 Myr: d = 2500 + 350 * sqrt(age)
///   age >= 70 Myr: d = 6400 - 3200 * exp(-age / 62.8)
pub fn apply_seafloor_age<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    state: &TectonicState,
    boundaries: &[BoundaryCell],
    spreading_rate: f64,
) {
    let n = grid.cell_count();

    // 洋壳年龄的正确算法：从洋中脊（离散边界）出发做 BFS，
    // 距离除以扩张速率就是年龄。
    //
    // 之前的实现用「到板块欧拉极的角距」估年龄，会在板块内部
    // 产生一圈圈同心圆——那不是地貌，是数学伪影。
    let mut age = vec![f32::INFINITY; n];
    let mut queue: std::collections::VecDeque<u32> = std::collections::VecDeque::new();

    // 所有离散边界（洋中脊）作为年龄为 0 的源
    for b in boundaries {
        if b.kind == BoundaryKind::Divergent && fields.crust_type[b.cell as usize] < 0.5 {
            age[b.cell as usize] = 0.0;
            queue.push_back(b.cell);
        }
    }

    // 如果没有离散边界，退化为「从所有洋壳边缘开始」
    if queue.is_empty() {
        for i in 0..n {
            if fields.crust_type[i] < 0.5 {
                // 与陆壳相邻的洋壳算年轻
                let near_land = grid
                    .neighbors(i)
                    .iter()
                    .any(|&nb| fields.crust_type[nb as usize] > 0.5);
                if near_land {
                    age[i] = 0.0;
                    queue.push_back(i as u32);
                }
            }
        }
    }

    // BFS 传播年龄。扩张速率：典型洋中脊 5 cm/年 = 50 km/百万年 = 50000 m/Myr
    let rate_m_per_myr = 50_000.0 * spreading_rate.max(0.1);
    while let Some(cur) = queue.pop_front() {
        let cur_age = age[cur as usize];
        for &nb in grid.neighbors(cur as usize) {
            // 只在大洋板块内部传播
            if fields.crust_type[nb as usize] >= 0.5 {
                continue;
            }
            let dist = grid.distance_m(cur as usize, nb as usize);
            let nb_age = cur_age + (dist / rate_m_per_myr) as f32;
            if nb_age < age[nb as usize] {
                age[nb as usize] = nb_age;
                queue.push_back(nb);
            }
        }
    }

    // 无法到达的洋壳（被陆壳完全隔开）给个中老龄
    for i in 0..n {
        if !age[i].is_finite() {
            age[i] = if fields.crust_type[i] < 0.5 {
                60.0
            } else {
                0.0
            };
        }
    }

    fields.crust_age = age;

    // 施加热沉降：只作用于洋壳
    fields
        .elevation
        .par_iter_mut()
        .enumerate()
        .for_each(|(i, e)| {
            if fields.crust_type[i] >= 1.0 {
                return;
            }
            let a = fields.crust_age[i] as f64;
            let depth = if a < 70.0 {
                2500.0 + 350.0 * a.sqrt()
            } else {
                6400.0 - 3200.0 * (-a / 62.8).exp()
            };
            let weight = (1. - fields.crust_type[i]).clamp(0., 1.);
            *e = *e * (1. - weight) - (depth as f32) * weight;
        });

    let _ = state;
}
