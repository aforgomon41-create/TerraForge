//! 阶段 4：气候系统。
//!
//! 气候决定「这地方长什么样」，是生态和人文的依据。
//!
//! 三步走：
//! 1. 温度：纬度基准 + 高程修正 + 大陆度修正 + 洋流修正
//! 2. 风场：三圈环流 + 地形绕流 + 海陆热力差异
//! 3. 降水：水汽平流模型，雨影效应自然涌现

use rayon::prelude::*;
use tf_core::geo::UnitVec3;
use tf_core::params::{GenerationParams, PlanetParams};
use tf_grid::GridOps;

use crate::fields::WorldFields;

/// 气候参数。
#[derive(Debug, Clone, Copy)]
pub struct ClimateParams {
    /// 温室效应倍率。
    pub greenhouse: f64,
    /// 降水倍率。
    pub precipitation_scale: f64,
    /// 是否启用季风。
    pub monsoon: bool,
    /// 水汽平流迭代次数。
    pub advection_iterations: u32,
}

impl Default for ClimateParams {
    fn default() -> Self {
        Self {
            greenhouse: 1.0,
            precipitation_scale: 1.0,
            monsoon: true,
            advection_iterations: 80,
        }
    }
}

/// 运行完整的气候模拟。
pub fn run_climate<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    planet: &PlanetParams,
    params: &ClimateParams,
) {
    compute_temperature(grid, fields, planet, params);
    compute_wind(grid, fields, planet);
    compute_precipitation(grid, fields, params);
    // 洋流修正温度，再跑一轮降水
    apply_ocean_currents(grid, fields, planet);
    compute_temperature(grid, fields, planet, params);
    compute_precipitation(grid, fields, params);
}

/// 计算温度场。
///
/// 分四步：纬度基准、高程修正、大陆度修正、洋流修正。
fn compute_temperature<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    planet: &PlanetParams,
    params: &ClimateParams,
) {
    use tf_core::constants::ENV_LAPSE_RATE;
    let n = grid.cell_count();
    let tilt = planet.axial_tilt_deg.to_radians();
    let solar = planet.solar_constant / tf_core::constants::SOLAR_CONSTANT;

    fields.temperature.par_iter_mut().enumerate().for_each(|(i, t)| {
        let lat_deg = grid.latitude_deg(i);
        let lat = lat_deg.to_radians();

        // 1. 纬度基准。用太阳辐射的余弦加权：
        //    年积分的入射辐射与 cos(lat) 成正比，但实际地表温度还要考虑
        //    大气环流的热输运，把温差拉平一部分。
        //    地球实测：赤道 27 度，两极 -25 度。
        let insolation = lat.cos().abs().max(0.08);
        let base = -25.0 + 52.0 * insolation.powf(0.45);

        // 2. 轴倾角修正：倾角越大，极地越冷（极昼极夜时间更长）
        let tilt_norm = (tilt / 0.409).clamp(0.0, 2.0); // 1.0 = 地球 23.4 度
        let tilt_penalty = (tilt_norm - 1.0) * 8.0 * lat.abs().sin().powi(2);

        // 3. 温室效应：基准是地球的 33 度温室增温
        let greenhouse = (params.greenhouse - 1.0) * 20.0;

        // 4. 太阳常数修正：主序星辐射偏差 10% 对应约 15 度
        let solar_adj = (solar - 1.0) * 18.0;

        // 5. 高程修正：每升高 1 公里降 6.5 度
        let elev = (fields.elevation[i].max(0.0) as f64).min(10_000.0);
        let lapse = -elev * ENV_LAPSE_RATE;

        // 6. 大陆度修正：内陆季节温差大，年均温略降
        let dist_sea = fields.distance_to_sea[i] as f64;
        let continentality = -(dist_sea / 3_000_000.0).min(1.0) * 4.0;

        let value = base - tilt_penalty + greenhouse + solar_adj + lapse + continentality;
        *t = value.clamp(-88.0, 60.0) as f32;
    });

    let _ = n;
}

/// 计算风场。
///
/// 基准是三圈环流。再加地形绕流和海陆热力差异。
/// 结果存进 flow_accumulation 的复用位置不太合适，这里用温度场之外的方式：
/// 我们暂时把风向存在 WorldFields 的两个额外通道里——但为了不膨胀结构，
/// 降水计算直接在这里内联算风。
fn compute_wind<G: GridOps>(grid: &G, fields: &WorldFields, planet: &PlanetParams) -> Vec<(f32, f32)> {
    let n = grid.cell_count();
    let _ = planet;

    (0..n)
        .into_par_iter()
        .map(|i| {
            let lat_deg = grid.latitude_deg(i);
            let lat = lat_deg.to_radians();

            // 三圈环流的纬向分量
            let abs_lat = lat.abs();
            let u = if abs_lat < 30.0_f64.to_radians() {
                // 信风：从副热带高压吹向赤道，北半球是东北风
                -1.0
            } else if abs_lat < 60.0_f64.to_radians() {
                // 西风带
                1.0
            } else {
                // 极地东风
                -1.0
            };

            // 经向分量：赤道上升、副热带下沉、中纬上升、极地下沉
            let v = if abs_lat < 30.0_f64.to_radians() {
                -lat.signum() * 0.5
            } else if abs_lat < 60.0_f64.to_radians() {
                lat.signum() * 0.5
            } else {
                -lat.signum() * 0.4
            };

            // 地形绕流：坡度越大，风越容易绕开
            let nb = grid.neighbors(i);
            let mut slope_x = 0.0f64;
            let mut slope_y = 0.0f64;
            let mut count = 0.0f64;
            for &j in nb {
                let dh = (fields.elevation[j as usize] - fields.elevation[i]) as f64;
                let dj = grid.center_dir(j as usize).as_vec() - grid.center_dir(i).as_vec();
                let (east, north) = grid.center_dir(i).tangent_frame();
                slope_x += dh / 1000.0 * dj.dot(east);
                slope_y += dh / 1000.0 * dj.dot(north);
                count += 1.0;
            }
            if count > 0.0 {
                slope_x /= count;
                slope_y /= count;
            }

            // 绕流分量：垂直于坡度方向
            let deflect = 0.25;
            let uf = u - slope_y * deflect;
            let vf = v + slope_x * deflect;

            // 归一化到合理量级
            let mag = (uf * uf + vf * vf).sqrt().max(1e-6);
            ((uf / mag) as f32, (vf / mag) as f32)
        })
        .collect()
}

/// 计算降水。
///
/// 水汽平流模型：
/// 1. 水汽从海洋蒸发进入近地层
/// 2. 风把水汽吹走
/// 3. 遇地形抬升，冷却，凝结成雨
/// 4. 饱和不了的继续往内陆吹
/// 5. 水汽有衰减
///
/// 这个模型的关键优势：**雨影效应是自然涌现的**。
/// 湿润气流从海上吹来，迎风坡倾盆大雨，翻过去就是荒漠。
fn compute_precipitation<G: GridOps>(grid: &G, fields: &mut WorldFields, params: &ClimateParams) {
    let n = grid.cell_count();
    let wind = compute_wind(grid, fields, &PlanetParams::default());

    // 水汽容量由温度决定（Clausius-Clapeyron 的线性近似）。
    let capacity: Vec<f32> = (0..n)
        .map(|i| (((fields.temperature[i] as f64 + 25.0) / 45.0).clamp(0.08, 1.6)) as f32)
        .collect();

    // 初始：只有海洋有水汽。陆地从 0 开始，靠风一点点送进去。
    // 这是雨影效应的来源——如果陆地一开始就充满水汽，就没有雨影了。
    let mut moisture = vec![0.0f32; n];
    for i in 0..n {
        if fields.elevation[i] <= 0.0 {
            moisture[i] = capacity[i];
        }
    }

    let mut precip = vec![0.0f32; n];
    let iterations = params.advection_iterations.max(1);

    for _ in 0..iterations {
        let snapshot = moisture.clone();
        let mut next = moisture.clone();

        for i in 0..n {
            // 海洋持续蒸发，保持饱和
            if fields.elevation[i] <= 0.0 {
                next[i] = capacity[i];
                continue;
            }

            let nb = grid.neighbors(i);
            if nb.is_empty() {
                continue;
            }

            let (u, v) = wind[i];
            let (east, north) = grid.center_dir(i).tangent_frame();
            let wind_vec = east * u as f64 + north * v as f64;
            let wind_mag = wind_vec.length().max(1e-9);

            // 平流：只取「上游」邻居的水汽。
            // 关键改动：之前对所有邻居加权平均，等于把水汽稀释掉，
            // 传不了几格就衰减到零。现在只从真正的上游取，水汽能长距离输送。
            let mut inflow = 0.0f32;
            let mut wsum = 0.0f32;
            for &j in nb {
                let dj = grid.center_dir(j as usize).as_vec() - grid.center_dir(i).as_vec();
                let dl = dj.length();
                if dl < 1e-12 {
                    continue;
                }
                // 上游方向：与风向相反
                let upstream = -dj / dl;
                let align = wind_vec.dot(upstream) / wind_mag;
                if align <= 0.0 {
                    continue;
                }
                // 用 align 的平方做权重，强化顺风方向
                let w = (align * align) as f32;
                inflow += snapshot[j as usize] * w;
                wsum += w;
            }
            if wsum > 0.0 {
                inflow /= wsum;
            }

            // 平流：自身残留 + 上游输入。
            // 0.55 的混合系数意味着水汽在一格内衰减 45%，
            // 对应真实大气中水汽的典型停留时间。
            let carried = snapshot[i] * 0.25 + inflow * 0.75;

            // 地形抬升致雨
            let mut upslope = 0.0f64;
            for &j in nb {
                let dh = (fields.elevation[j as usize] - fields.elevation[i]) as f64;
                let dj = grid.center_dir(j as usize).as_vec() - grid.center_dir(i).as_vec();
                let dl = dj.length();
                if dl < 1e-12 || dh <= 0.0 {
                    continue;
                }
                let along = wind_vec.dot(dj / dl) / wind_mag;
                if along > 0.0 {
                    upslope += dh * along;
                }
            }
            // 每上升 1 公里消耗掉约 45% 的水汽
            let lift = (upslope / 1000.0).min(6.0);
            let orographic = 1.0 - (-0.6 * lift).exp();

            // 对流降雨：暖湿地区自身也会下雨，赤道最强
            let lat = grid.latitude_deg(i).to_radians().abs();
            let itcz = (1.0 - (lat / (20.0_f64.to_radians())).min(1.0)) as f32;
            let convective = (0.02 + 0.10 * itcz) * (capacity[i] as f32);

            let rain_frac = (orographic as f32 * 0.85 + convective).clamp(0.0, 0.92);
            let rain = carried * rain_frac;

            next[i] = (carried - rain).max(0.0);
            precip[i] += rain;
        }

        moisture = next;
    }

    // 转成毫米每年。
    //
    // 用「分位数映射 + 原始残差」的混合，而不是纯分位数。
    //
    // 纯分位数会把降水量化成台阶：同一 rank 区间内的格子被赋成同一个值。
    // 渲染时这些台阶就是大块纯色。加回原始偏差可以保留细节。
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_by(|&a, &b| {
        precip[a as usize]
            .partial_cmp(&precip[b as usize])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let pmin = precip[order[0] as usize];
    let pmax = precip[order[n - 1] as usize];
    let pspan = (pmax - pmin).max(1e-9);

    for (rank, &cell) in order.iter().enumerate() {
        let q = rank as f64 / (n - 1).max(1) as f64;

        // 目标分布：Logistic 型的经验曲线
        let target_mm = if q < 0.15 {
            q / 0.15 * 200.0
        } else if q < 0.5 {
            200.0 + (q - 0.15) / 0.35 * 500.0
        } else if q < 0.85 {
            700.0 + (q - 0.5) / 0.35 * 1300.0
        } else if q < 0.98 {
            2000.0 + (q - 0.85) / 0.13 * 2000.0
        } else {
            4000.0 + (q - 0.98) / 0.02 * 8000.0
        };

        // 原始残差：同排名位置上的相对偏离，幅度是局部降水的典型变化
        let raw_norm = ((precip[cell as usize] - pmin) / pspan) as f64;
        let deviation = (raw_norm - q).clamp(-0.3, 0.3);
        let detail = deviation * 2.0 * 180.0;

        fields.precipitation[cell as usize] =
            ((target_mm + detail) * params.precipitation_scale).max(0.0) as f32;
    }
}

/// 洋流对温度的修正。
///
/// 简化版：暖流把热量往高纬搬。用经向环流的定性模式。
fn apply_ocean_currents<G: GridOps>(grid: &G, fields: &mut WorldFields, _planet: &PlanetParams) {
    let old_temp = fields.temperature.clone();

    // 洋流的热输运必须**对称**：把热量从低纬搬向高纬，
    // 高纬增温、低纬略微降温，总量守恒。
    //
    // 之前的写法只对海洋加 `-lat.signum() * ...`，
    // 北半球中纬被减温、南半球被加温，造成南北半球温度差 16 度。
    // 这不是物理，是符号错误。
    fields.temperature.par_iter_mut().enumerate().for_each(|(i, t)| {
        if fields.elevation[i] > 0.0 {
            return;
        }
        let lat = grid.latitude_deg(i).to_radians().abs();
        let lat_norm = (lat / (std::f64::consts::PI / 2.0)) as f32;

        // 高纬海洋得到暖流增温，低纬略微降温（热量被带走）
        // 峰值在 45 度附近，那里是西风带把热量往极地送最有效的区域
        let peak = 1.0 - ((lat_norm - 0.5) / 0.35).abs();
        let transport = peak.max(0.0) * 4.0 - (1.0 - lat_norm) * 1.2;

        *t = old_temp[i] + transport;
    });
}
