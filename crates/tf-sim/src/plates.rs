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
#[derive(Debug, Clone, Default)]
pub struct TectonicState {
    pub plates: Vec<Plate>,
    pub convection: Vec<ConvectionCell>,
    pub planar_velocities: Option<Vec<DVec3>>,
    pub centers: Vec<DVec3>,
    pub partition_scale: f64,
    pub partition_seed: u64,
    pub partition_weights: Vec<f64>,
    pub history: crate::evolution::History,
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

    let major_count = ((plate_count as f64 * (1. - params.minor_plate_fraction)).round() as usize)
        .clamp(3, plate_count);
    let mut seeds: Vec<UnitVec3> = Vec::with_capacity(plate_count);
    for k in 0..plate_count {
        let center = if k < major_count {
            let candidates: Vec<_> = (0..24).map(|_| rng.unit_sphere()).collect();
            *candidates
                .iter()
                .max_by(|a, b| {
                    let clearance = |p: &UnitVec3| {
                        seeds
                            .iter()
                            .map(|q| p.angle_to(q))
                            .fold(f64::INFINITY, f64::min)
                    };
                    clearance(a).total_cmp(&clearance(b))
                })
                .unwrap()
        } else {
            let a = (k - major_count) % major_count;
            let b = (a + 1 + rng.below((major_count - 1) as u32) as usize) % major_count;
            UnitVec3::from_vec(seeds[a].as_vec() + seeds[b].as_vec())
                .unwrap_or(seeds[a])
                .offset(
                    rng.range_f64(0., std::f64::consts::TAU),
                    rng.range_f64(0.03, 0.20),
                )
        };
        seeds.push(center);
    }
    let partition_weights: Vec<f64> = (0..plate_count)
        .map(|i| {
            if i < major_count {
                rng.range_f64(0.8, 1.3)
            } else {
                rng.range_f64(0.18, 0.42)
            }
        })
        .collect();

    let convection = build_convection_cells(rng, plate_count * 4);
    let planar_seeds = if grid.field_is_planar() {
        grid.extent_m()
    } else {
        None
    }
    .map(|(w, h)| {
        let mut positions: Vec<DVec3> = Vec::new();
        for k in 0..plate_count {
            let point = if k < major_count {
                (0..24)
                    .map(|_| {
                        DVec3::new(
                            rng.range_f64(-0.46, 0.46) * w,
                            rng.range_f64(-0.46, 0.46) * h,
                            0.,
                        )
                    })
                    .max_by(|a, b| {
                        let clearance = |p: DVec3| {
                            positions
                                .iter()
                                .map(|q| (p - *q).length_squared())
                                .fold(f64::INFINITY, f64::min)
                        };
                        clearance(*a).total_cmp(&clearance(*b))
                    })
                    .unwrap()
            } else {
                let a = (k - major_count) % major_count;
                let b = (a + 1 + rng.below((major_count - 1) as u32) as usize) % major_count;
                (positions[a] + positions[b]) * 0.5
                    + DVec3::new(
                        rng.range_f64(-0.06, 0.06) * w,
                        rng.range_f64(-0.06, 0.06) * h,
                        0.,
                    )
            };
            positions.push(point);
        }
        positions
    });
    let planar_velocities = planar_seeds.as_ref().map(|_| {
        (0..plate_count)
            .map(|_| {
                let a = rng.range_f64(0., std::f64::consts::TAU);
                DVec3::new(a.cos(), a.sin(), 0.)
                    * rng.range_f64(25000., 75000.)
                    * params.tectonic_activity
            })
            .collect::<Vec<_>>()
    });

    // 板块划分：球面 Voronoi。
    //
    // 关键：纯 Voronoi 会在二十面体细分的格网上留下明显的三角纹理，
    // 因为种子点的分布和格网结构之间存在关联。
    // 加上一层噪声扰动边界距离，让板块边界变成自然的不规则曲线。
    let warp_noise = tf_core::noise::SimplexNoise::new(params.seed ^ 0xB0A7D);
    let plate_id: Vec<u16> = (0..n)
        .into_par_iter()
        .map(|i| {
            if let Some(seeds) = &planar_seeds {
                let p = grid.field_position(i);
                let (w, h) = grid.extent_m().unwrap();
                let scale = w.max(h);
                let warp = DVec3::new(
                    warp_noise.sample(p.x / scale * 4., p.y / scale * 4., 17.),
                    warp_noise.sample(p.x / scale * 4., p.y / scale * 4., 37.),
                    0.,
                ) * scale
                    * 0.08;
                return seeds
                    .iter()
                    .enumerate()
                    .min_by(|(ai, a), (bi, b)| {
                        ((p + warp - **a).length_squared() / partition_weights[*ai].powi(2))
                            .total_cmp(
                                &((p + warp - **b).length_squared()
                                    / partition_weights[*bi].powi(2)),
                            )
                    })
                    .unwrap()
                    .0 as u16;
            }
            let c = grid.center_dir(i);
            let cv = c.as_vec();
            // 低频噪声扰动：让边界蜿蜒而不是直线
            let warp = warp_noise.fbm(cv.x * 3.5, cv.y * 3.5, cv.z * 3.5, 3, 2.0, 0.5) * 0.18;
            let mut best = 0usize;
            let mut best_score = f64::NEG_INFINITY;
            for (k, s) in seeds.iter().enumerate() {
                let mut score = (cv.dot(s.as_vec()) - 1.) / partition_weights[k].powi(2);
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
                    * (0.4 + strength).clamp(0.5, 1.6)
                    * 7.8e-3,
                oceanic,
            }
        })
        .collect();

    let centers = planar_seeds
        .unwrap_or_else(|| seeds.iter().map(|p| p.as_vec() * grid.radius_m()).collect());
    let partition_scale = grid
        .extent_m()
        .map_or(grid.radius_m(), |(w, h)| w.max(h).max(3_000_000.));
    (
        TectonicState {
            plates,
            convection,
            planar_velocities,
            centers,
            partition_scale,
            partition_seed: params.seed ^ 0xB0A7D,
            partition_weights,
            history: Default::default(),
        },
        plate_id,
    )
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
    let _ = (state, rng);
    crate::terrain::init_continents(grid, fields, params);
    fields.plate_id.copy_from_slice(plate_id);
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

fn partition_normal<G: GridOps>(
    grid: &G,
    ids: &[u16],
    i: usize,
    j: usize,
    a: usize,
    b: usize,
) -> DVec3 {
    let midpoint = (grid.field_position(i) + grid.field_position(j)) * 0.5;
    let scale = grid.distance_m(i, j).max(1.);
    let mut cells = vec![i, j];
    for root in [i, j] {
        for &nb in grid.neighbors(root) {
            cells.push(nb as usize);
            cells.extend(grid.neighbors(nb as usize).iter().map(|&x| x as usize));
        }
    }
    cells.sort_unstable();
    cells.dedup();
    let mut pa = DVec3::ZERO;
    let mut pb = DVec3::ZERO;
    let mut wa = 0.;
    let mut wb = 0.;
    for cell in cells {
        let p = grid.field_position(cell);
        let w = 1. / ((p - midpoint).length_squared() + scale * scale);
        if ids[cell] as usize == a {
            pa += p * w;
            wa += w;
        }
        if ids[cell] as usize == b {
            pb += p * w;
            wb += w;
        }
    }
    let mut normal = pb / wb.max(1e-30) - pa / wa.max(1e-30);
    if !grid.field_is_planar() {
        let radial = midpoint.normalize();
        normal -= radial * normal.dot(radial);
    }
    normal.normalize_or_zero()
}

/// 识别板块边界并分类。
pub fn find_boundaries<G: GridOps>(
    grid: &G,
    plate_id: &[u16],
    state: &TectonicState,
    fields: &WorldFields,
) -> Vec<BoundaryCell> {
    let mut result = Vec::new();
    for i in 0..grid.cell_count() {
        for &nb in grid.neighbors(i) {
            let j = nb as usize;
            if j <= i || plate_id[i] == plate_id[j] {
                continue;
            }
            let (i, j) = if plate_id[i] < plate_id[j] {
                (i, j)
            } else {
                (j, i)
            };
            let (p, q) = (grid.center_dir(i).as_vec(), grid.center_dir(j).as_vec());
            let normal = partition_normal(
                grid,
                plate_id,
                i,
                j,
                plate_id[i] as usize,
                plate_id[j] as usize,
            );
            let relative = if let Some(velocities) = &state.planar_velocities {
                velocities[plate_id[i] as usize] - velocities[plate_id[j] as usize]
            } else {
                // Both velocities must be evaluated at the same boundary point.
                let mid = (p + q).normalize();
                (plate_velocity(&state.plates[plate_id[i] as usize], mid)
                    - plate_velocity(&state.plates[plate_id[j] as usize], mid))
                    * grid.radius_m()
            };
            let vn = relative.dot(normal);
            let shear = (relative - normal * vn).length();
            let threshold = (shear * 0.45).max(1000.);
            let kind = if vn > threshold {
                BoundaryKind::Convergent
            } else if vn < -threshold {
                BoundaryKind::Divergent
            } else {
                BoundaryKind::Transform
            };
            let (a, b) = (fields.crust_type[i] < 0.5, fields.crust_type[j] < 0.5);
            let subtype = match (kind, a, b) {
                (BoundaryKind::Convergent, true, true) => BoundarySubtype::OceanOceanConvergent,
                (BoundaryKind::Convergent, false, false) => {
                    BoundarySubtype::ContinentContinentConvergent
                }
                (BoundaryKind::Convergent, _, _) => BoundarySubtype::OceanContinentConvergent,
                (BoundaryKind::Divergent, true, true) => BoundarySubtype::OceanOceanDivergent,
                (BoundaryKind::Divergent, _, _) => BoundarySubtype::ContinentContinentDivergent,
                _ => BoundarySubtype::Transform,
            };
            result.push(BoundaryCell {
                cell: i as u32,
                other: j as u32,
                kind,
                subtype,
                normal_velocity: vn,
                tangential_velocity: shear,
                normal: normal.to_array(),
            });
        }
    }
    result
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
            let continental = is_continental[i].clamp(0., 1.);
            let response = continental * continental * (3. - 2. * continental);
            let eff = delta * (0.15 + 0.85 * response);
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

/// Airy 均衡：相对初始平衡柱的厚度变化产生隆起；无参考柱时使用绝对近似。
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
            let reference = fields.reference_crust_thickness[i];
            if reference > 0. {
                *e += (thick - reference) * coeff;
            } else {
                *e = (thick - t_ref) * coeff;
            }
        });

    adjust_sea_level(grid, fields, params.land_fraction);
}

/// 调整海平面，使陆地占比接近目标值。
pub fn adjust_sea_level<G: GridOps>(grid: &G, fields: &mut WorldFields, target_land_fraction: f64) {
    let n = grid.cell_count();
    let mut sample: Vec<f32> = (0..n).map(|i| fields.elevation[i]).collect();
    sample.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    if target_land_fraction <= 0. || target_land_fraction >= 1. {
        let offset = if target_land_fraction >= 1. {
            sample[0] - 1.
        } else {
            sample[n - 1] + 1.
        };
        fields.elevation.par_iter_mut().for_each(|h| *h -= offset);
        return;
    }
    let target = target_land_fraction;
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
    let mut sources = vec![0f32; n];
    for b in boundaries {
        if b.kind == BoundaryKind::Divergent {
            for i in [b.cell as usize, b.other as usize] {
                if fields.crust_type[i] < 0.7 {
                    sources[i] = 1.;
                }
            }
        }
    }
    let has_source = sources.iter().any(|&x| x > 0.);
    let (distance, _) = crate::landforms::nearest_sources(
        grid,
        &sources,
        std::f64::consts::PI * grid.radius_m() * 2.,
    );
    let rate = 50_000. * spreading_rate.max(0.1);
    for i in 0..n {
        let ocean_age = if has_source {
            (distance[i] / rate).clamp(0., 200.)
        } else {
            60.
        };
        let continental = fields.crust_type[i] as f64;
        fields.crust_age[i] = (ocean_age * (1. - continental) + 600. * continental) as f32;
        let depth = if ocean_age < 70. {
            2500. + 350. * ocean_age.sqrt()
        } else {
            6400. - (6400. - (2500. + 350. * 70_f64.sqrt())) * (-(ocean_age - 70.) / 62.8).exp()
        };
        // Apply cooling relative to the young-ocean isostatic surface, continuously.
        fields.elevation[i] -= ((depth - 2500.) * (1. - continental).powi(2)) as f32;
    }
    let _ = state;
}

#[cfg(test)]
mod continuity_tests {
    use super::*;
    #[test]
    fn initial_partition_has_major_and_minor_plate_area_scales() {
        let g = tf_grid::SphereHexGrid::build(32, 6371000., (DVec3::Z, 0.));
        let p = GenerationParams::default();
        let (_, ids) = build_plates(&g, &p, &mut DeterministicRng::derive(p.seed, "tectonics"));
        let mut areas = vec![0.; p.plate_count as usize];
        for (i, id) in ids.iter().enumerate() {
            areas[*id as usize] += g.area_m2(i);
        }
        let majors = ((p.plate_count as f64 * (1. - p.minor_plate_fraction)).round() as usize)
            .clamp(3, p.plate_count as usize);
        let mut large = areas[..majors].to_vec();
        let mut small = areas[majors..].to_vec();
        large.sort_by(f64::total_cmp);
        small.sort_by(f64::total_cmp);
        assert!(
            large[large.len() / 2] > small[small.len() / 2] * 4.,
            "areas {areas:?}"
        );
        assert!(small.iter().filter(|&&a| a > 0.).count() >= small.len() / 2);
    }
    #[test]
    fn shortening_response_is_continuous_through_mixed_crust() {
        let g = tf_grid::FlatGrid::new(32, 24, 3_200_000., 30., 0., 6371000.);
        let params = GenerationParams {
            seed: 42,
            plate_count: 8,
            ..Default::default()
        };
        let (state, ids) = build_plates(
            &g,
            &params,
            &mut DeterministicRng::derive(params.seed, "plates"),
        );
        let mut a = WorldFields::with_capacity(g.cell_count());
        a.plate_id = ids;
        a.crust_thickness.fill(21500.);
        a.crust_type.fill(0.499);
        let mut b = a.clone();
        b.crust_type.fill(0.501);
        step_tectonics(&g, &state, &mut a, &params, 1.);
        step_tectonics(&g, &state, &mut b, &params, 1.);
        assert!(a.crust_thickness.iter().any(|&x| (x - 21500.).abs() > 100.));
        assert!(a
            .crust_thickness
            .iter()
            .zip(&b.crust_thickness)
            .all(|(x, y)| (x - y).abs() < 100.));
    }
    #[test]
    fn thermal_cooling_has_no_crust_halfway_wall() {
        let g = tf_grid::FlatGrid::new(4, 4, 4000., 30., 0., 6371000.);
        let mut f = WorldFields::with_capacity(16);
        f.elevation.fill(100.);
        f.crust_type.fill(0.499);
        f.crust_type[1] = 0.5;
        f.crust_type[2] = 0.501;
        let state = TectonicState {
            plates: vec![],
            convection: vec![],
            planar_velocities: None,
            centers: vec![],
            partition_scale: 1.,
            partition_seed: 0,
            ..Default::default()
        };
        apply_seafloor_age(&g, &mut f, &state, &[], 1.);
        assert!((f.elevation[0] - f.elevation[1]).abs() < 5.);
        assert!((f.elevation[1] - f.elevation[2]).abs() < 5.);
    }
}
