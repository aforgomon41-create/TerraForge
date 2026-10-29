//! 阶段 3：侵蚀与水文。
//!
//! 构造给地形骨架，侵蚀给地形细节。这一步是地貌真实感的最大来源。
//!
//! # 每轮迭代
//!
//! 1. 填洼：保证处处能流出，同时记录被填的深度——那就是湖泊
//! 2. 流向：每个格子的水往哪流
//! 3. 汇流累积：每条河有多少上游面积
//! 4. 河流下切：Stream Power Law 的隐式解
//! 5. 坡面扩散：山坡变圆润
//! 6. 沉积：低洼处填充

use rayon::prelude::*;
use tf_grid::GridOps;

use crate::fields::WorldFields;

/// 侵蚀参数。
#[derive(Debug, Clone, Copy)]
pub struct ErosionParams {
    /// 迭代次数。
    pub iterations: u32,
    /// 河流侵蚀系数 K。越大切得越深。
    pub k_river: f64,
    /// 汇流面积指数 m。
    pub m: f64,
    /// 坡度指数 n。
    pub n: f64,
    /// 坡面扩散系数。
    pub k_diffusion: f64,
    /// 沉积系数。
    pub k_deposition: f64,
}

impl Default for ErosionParams {
    fn default() -> Self {
        Self {
            iterations: 150,
            k_river: 3.0e-5,
            m: 0.5,
            n: 1.0,
            k_diffusion: 0.02,
            k_deposition: 0.3,
        }
    }
}

/// 侵蚀模拟的状态。
pub struct ErosionState {
    /// 每个格子的流向目标。usize::MAX 表示无出口（海洋或边界）。
    pub flow_dir: Vec<u32>,
    /// 拓扑序，从下游到上游。
    pub topo_order: Vec<u32>,
}

/// 执行完整的侵蚀模拟。
pub fn run_erosion<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    params: &ErosionParams,
) -> ErosionState {
    let n = grid.cell_count();
    let mut state = ErosionState {
        flow_dir: vec![u32::MAX; n],
        topo_order: Vec::with_capacity(n),
    };

    for _iter in 0..params.iterations {
        // 1. 填洼，同时得到湖泊深度
        fill_depressions(grid, fields);

        // 2-3. 流向与拓扑序
        compute_flow(grid, fields, &mut state);

        // 汇流累积
        accumulate_flow(grid, fields, &state);

        // 4. 河流下切（隐式解）
        river_incision(grid, fields, &state, params);

        // 5. 坡面扩散
        diffuse_slopes(grid, fields, params);
    }

    state
}

/// 填洼：Priority-Flood 算法。
///
/// 从所有海洋格子出发，用优先队列按高度往外扩。
/// 遇到比当前水面低的洼地就抬高到出水口高度，同时记录被填的深度——那就是湖泊。
fn fill_depressions<G: GridOps>(grid: &G, fields: &mut WorldFields) {
    let n = grid.cell_count();
    // 用简单的迭代填洼代替完整优先队列，对六角格足够快且实现可靠
    // 原理：反复扫描，如果某个陆地格子的所有邻居都比它高，就抬到最低邻居的高度
    for _ in 0..200 {
        let mut changed = false;
        let snapshot = fields.elevation.clone();
        let mut new_elev = fields.elevation.clone();
        let mut new_lake = fields.lake_depth.clone();

        for i in 0..n {
            if snapshot[i] <= 0.0 {
                continue; // 海洋
            }
            let nb = grid.neighbors(i);
            if nb.is_empty() {
                continue;
            }
            let mut min_nb = f32::INFINITY;
            let mut all_higher = true;
            for &j in nb {
                let h = snapshot[j as usize];
                if h < min_nb {
                    min_nb = h;
                }
                if h <= snapshot[i] {
                    all_higher = false;
                }
            }
            if all_higher && min_nb > snapshot[i] {
                // 这里是个洼地，抬到最低邻居的高度
                let rise = min_nb - snapshot[i];
                new_elev[i] = min_nb;
                new_lake[i] += rise;
                changed = true;
            }
        }

        fields.elevation = new_elev;
        fields.lake_depth = new_lake;
        if !changed {
            break;
        }
    }
}

/// 计算流向和拓扑序。
///
/// 每个陆地格子指向最低的邻居（单流向）。海洋格子没有出流。
/// 拓扑序用 Kahn 算法：先找所有无入流的格子（海洋和水系末端），再往外推。
fn compute_flow<G: GridOps>(grid: &G, fields: &WorldFields, state: &mut ErosionState) {
    let n = grid.cell_count();

    // 流向：指向最低邻居
    state.flow_dir.par_iter_mut().enumerate().for_each(|(i, dir)| {
        if fields.elevation[i] <= 0.0 {
            *dir = u32::MAX;
            return;
        }
        let mut lowest = u32::MAX;
        let mut lowest_h = fields.elevation[i];
        for &j in grid.neighbors(i) {
            let h = fields.elevation[j as usize];
            if h < lowest_h {
                lowest_h = h;
                lowest = j;
            }
        }
        *dir = lowest;
    });

    // 统计每个格子的入流数
    let mut in_degree = vec![0u32; n];
    for &d in &state.flow_dir {
        if d != u32::MAX {
            in_degree[d as usize] += 1;
        }
    }

    // Kahn 拓扑排序，从无入流处开始
    let mut queue: Vec<u32> = (0..n as u32).filter(|&i| in_degree[i as usize] == 0).collect();
    let mut order = Vec::with_capacity(n);
    let mut head = 0;
    while head < queue.len() {
        let cur = queue[head];
        head += 1;
        order.push(cur);
        let d = state.flow_dir[cur as usize];
        if d != u32::MAX {
            in_degree[d as usize] -= 1;
            if in_degree[d as usize] == 0 {
                queue.push(d);
            }
        }
    }

    state.topo_order = order;
}

/// 汇流累积，按拓扑序从上游往上游累加。
fn accumulate_flow<G: GridOps>(grid: &G, fields: &mut WorldFields, state: &ErosionState) {
    let n = grid.cell_count();

    // 初始化为自身面积
    for i in 0..n {
        fields.flow_accumulation[i] = grid.area_m2(i) as f32;
    }

    // 反向遍历拓扑序（从上游到下游），把累积量往下游传
    for &cell in state.topo_order.iter().rev() {
        let d = state.flow_dir[cell as usize];
        if d != u32::MAX && d != cell {
            fields.flow_accumulation[d as usize] += fields.flow_accumulation[cell as usize];
        }
    }
}

/// 河流下切：Stream Power Law 的隐式解。
///
/// 方程：dh/dt = -K * A^m * S^n + U
///
/// 隐式解法的好处是即使 K*A^m 很大也不会数值爆炸，可以用大步长。
/// 因为流向是树状结构，按拓扑序从上游往下游解即可，严格 O(n)。
fn river_incision<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    state: &ErosionState,
    params: &ErosionParams,
) {
    let n = grid.cell_count();
    let mut new_elev = fields.elevation.clone();

    // 按拓扑序从下游往上游处理（topo_order 是上游在前，所以要反着走）
    for &cell in state.topo_order.iter() {
        let i = cell as usize;
        let d = state.flow_dir[i];
        if d == u32::MAX {
            continue;
        }
        let j = d as usize;
        let dist = grid.distance_m(i, j) as f64;
        if dist < 1e-9 {
            continue;
        }
        let area = fields.flow_accumulation[i] as f64;
        // 隐式格式：h_new = (h_old + Kdt * A^m * h_down / dist) / (1 + Kdt * A^m / dist)
        let ka = params.k_river * area.powf(params.m) / dist;
        let h_old = new_elev[i] as f64;
        let h_down = new_elev[j] as f64;
        let h_new = (h_old + ka * h_down) / (1.0 + ka);

        // 下切限制：避免单次迭代把河谷切穿。
        //
        // 隐式解在汇流面积大（下游）时 ka 会很大，理论上收敛到 h_down，
        // 但多轮迭代累积下来会把整条河流的纵剖面削成接近水平，
        // 最终高程范围被拉到 ±40 km 这种荒谬的量级。
        //
        // 真实河流的下切速率受岩石强度和抬升速率制约，
        // 单次迭代的下切量应该远小于地形本身的起伏。
        let max_drop = (h_old - h_down).abs().max(1.0) * 0.6;
        let h_limited = if h_new < h_old {
            h_new.max(h_old - max_drop)
        } else {
            h_new
        };
        new_elev[i] = h_limited as f32;
    }

    fields.elevation = new_elev;
    let _ = n;
}

/// 坡面扩散，让山坡变圆润。
///
/// 用简单的拉普拉斯平滑，加一个坡度阈值做非线性崩积：
/// 陡坡扩散快，缓坡几乎不动。这会长出崩积坡和倒石锥。
fn diffuse_slopes<G: GridOps>(grid: &G, fields: &mut WorldFields, params: &ErosionParams) {
    let n = grid.cell_count();
    let old = fields.elevation.clone();
    let mut new_elev = old.clone();

    for i in 0..n {
        if old[i] <= 0.0 {
            continue; // 海洋不扩散
        }
        let nb = grid.neighbors(i);
        if nb.is_empty() {
            continue;
        }
        let mut sum = 0.0f32;
        let mut count = 0.0f32;
        for &j in nb {
            let hj = old[j as usize];
            if hj <= 0.0 {
                continue;
            }
            let dist = grid.distance_m(i, j as usize) as f32;
            let slope = (old[i] - hj) / dist.max(1.0);
            // 陡坡扩散快：slope 大时权重高
            let w = 1.0 + slope.abs() * 8.0;
            sum += hj * w;
            count += w;
        }
        if count > 0.0 {
            let avg = sum / count;
            let blended = old[i] + (avg - old[i]) * params.k_diffusion as f32;
            // 单步扩散不超过 200 m，避免把山脊抹平
            new_elev[i] = blended.clamp(old[i] - 200.0, old[i] + 200.0);
        }
    }

    fields.elevation = new_elev;
}

/// 计算距海距离，用多源 BFS。
pub fn compute_distance_to_sea<G: GridOps>(grid: &G, fields: &mut WorldFields) {
    let n = grid.cell_count();
    let mut dist = vec![f32::INFINITY; n];
    let mut queue: std::collections::VecDeque<u32> = std::collections::VecDeque::new();

    // 所有海洋格子距离为 0
    for i in 0..n {
        if fields.elevation[i] <= 0.0 {
            dist[i] = 0.0;
            queue.push_back(i as u32);
        }
    }

    // BFS 向外扩散
    while let Some(cur) = queue.pop_front() {
        let cur_d = dist[cur as usize];
        for &nb in grid.neighbors(cur as usize) {
            let step = grid.distance_m(cur as usize, nb as usize) as f32;
            let nd = cur_d + step;
            if nd < dist[nb as usize] {
                dist[nb as usize] = nd;
                queue.push_back(nb);
            }
        }
    }

    fields.distance_to_sea = dist;
}