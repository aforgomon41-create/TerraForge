//! Actual-water-surface drainage, conservative accumulation and implicit incision.
use crate::fields::WorldFields;
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, VecDeque},
};
use tf_grid::GridOps;

#[derive(Debug, Clone, Copy)]
pub struct ErosionParams {
    pub iterations: u32,
    pub k_river: f64,
    pub m: f64,
    pub n: f64,
    pub k_diffusion: f64,
    pub k_deposition: f64,
    pub timestep_years: f64,
}
impl Default for ErosionParams {
    fn default() -> Self {
        Self {
            iterations: 40,
            k_river: 3e-5,
            m: 0.5,
            n: 1.,
            k_diffusion: 20.,
            k_deposition: 0.3,
            timestep_years: 1000.,
        }
    }
}
/// Upstream-first order. Reversing it gives the downstream-first implicit solve.
pub struct ErosionState {
    pub flow_dir: Vec<u32>,
    pub topo_order: Vec<u32>,
    pub lake_budget: crate::lakes::LakeBudget,
    pub sediment: SedimentBudget,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct SedimentBudget {
    pub eroded_m3: f64,
    pub deposited_m3: f64,
    pub exported_m3: f64,
    pub suspended_m3: f64,
    pub residual_m3: f64,
}
impl std::ops::AddAssign for SedimentBudget {
    fn add_assign(&mut self, other: Self) {
        self.eroded_m3 += other.eroded_m3;
        self.deposited_m3 += other.deposited_m3;
        self.exported_m3 += other.exported_m3;
        self.suspended_m3 += other.suspended_m3;
        self.residual_m3 += other.residual_m3;
    }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    height: f64,
    cell: usize,
}
impl PartialEq for Entry {
    fn eq(&self, o: &Self) -> bool {
        self.height == o.height && self.cell == o.cell
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Entry {
    fn cmp(&self, o: &Self) -> Ordering {
        o.height
            .total_cmp(&self.height)
            .then_with(|| o.cell.cmp(&self.cell))
    }
}

/// Route the actual ground/water surface. A dry depression is a real terminal,
/// not a permanently full reservoir. Equal surfaces drain by a breadth-first
/// forest, so lake outlets and flat plains have no arbitrary epsilon slopes.
fn drainage<G: GridOps>(grid: &G, fields: &mut WorldFields) -> ErosionState {
    let n = grid.cell_count();
    let (surface, lake_budget) = crate::lakes::balance(grid, fields);
    let mut receiver = vec![u32::MAX; n];
    let mut assigned = vec![false; n];
    let mut queue = VecDeque::new();
    let boundary = grid.boundary_mask();
    for i in 0..n {
        if fields.elevation[i] <= 0. {
            assigned[i] = true;
            continue;
        }
        if boundary[i] {
            assigned[i] = true;
            queue.push_back(i);
            continue;
        }
        let mut slope = 0.;
        for &nb in grid.neighbors(i) {
            let j = nb as usize;
            let target = if fields.elevation[j] <= 0. {
                0.
            } else {
                surface[j]
            };
            let descent = (surface[i] - target) / grid.distance_m(i, j).max(1.);
            if descent > slope {
                slope = descent;
                receiver[i] = nb;
            }
        }
        if receiver[i] != u32::MAX {
            assigned[i] = true;
            queue.push_back(i);
        }
    }
    let expand = |queue: &mut VecDeque<usize>, assigned: &mut [bool], receiver: &mut [u32]| {
        while let Some(i) = queue.pop_front() {
            for &nb in grid.neighbors(i) {
                let j = nb as usize;
                if !assigned[j] && surface[j] == surface[i] {
                    assigned[j] = true;
                    receiver[j] = i as u32;
                    queue.push_back(j);
                }
            }
        }
    };
    expand(&mut queue, &mut assigned, &mut receiver);
    for i in 0..n {
        if !assigned[i] {
            assigned[i] = true;
            queue.push_back(i);
            expand(&mut queue, &mut assigned, &mut receiver);
        }
    }
    let mut incoming = vec![0u32; n];
    for &r in &receiver {
        if r != u32::MAX {
            incoming[r as usize] += 1;
        }
    }
    queue.extend((0..n).filter(|&i| incoming[i] == 0));
    let mut order = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i as u32);
        let r = receiver[i];
        if r != u32::MAX {
            incoming[r as usize] -= 1;
            if incoming[r as usize] == 0 {
                queue.push_back(r as usize);
            }
        }
    }
    assert_eq!(order.len(), n, "actual-surface drainage cycle");
    ErosionState {
        flow_dir: receiver,
        topo_order: order,
        lake_budget,
        sediment: SedimentBudget::default(),
    }
}

fn accumulate<G: GridOps>(grid: &G, fields: &mut WorldFields, state: &ErosionState) {
    for i in 0..grid.cell_count() {
        fields.flow_accumulation[i] =
            (grid.area_m2(i) + fields.external_area_m2.get(i).copied().unwrap_or(0.)) as f32;
    }
    for &cell in &state.topo_order {
        let i = cell as usize;
        let d = state.flow_dir[i];
        if d != u32::MAX {
            fields.flow_accumulation[d as usize] += fields.flow_accumulation[i];
        }
    }
}

pub fn refresh_hydrology<G: GridOps>(grid: &G, fields: &mut WorldFields) -> ErosionState {
    let state = drainage(grid, fields);
    accumulate(grid, fields, &state);
    state
}

/// Rebuild derived area/order from saved final receivers without re-simulating
/// snow, lakes or climate when an exported project is imported.
pub fn from_saved_receivers<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    receivers: Vec<u32>,
) -> Result<ErosionState, String> {
    let n = grid.cell_count();
    if receivers.len() != n {
        return Err("保存的流向尺寸无效".into());
    }
    let mut incoming = vec![0u32; n];
    for (i, &r) in receivers.iter().enumerate() {
        if r != u32::MAX {
            if r as usize >= n || !grid.neighbors(i).contains(&r) {
                return Err("保存的流向不满足地形邻接".into());
            }
            incoming[r as usize] += 1;
        }
    }
    let mut queue: VecDeque<_> = (0..n).filter(|&i| incoming[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i as u32);
        let r = receivers[i];
        if r != u32::MAX {
            incoming[r as usize] -= 1;
            if incoming[r as usize] == 0 {
                queue.push_back(r as usize);
            }
        }
    }
    if order.len() != n {
        return Err("保存的水系存在循环".into());
    }
    let state = ErosionState {
        flow_dir: receivers,
        topo_order: order,
        lake_budget: Default::default(),
        sediment: Default::default(),
    };
    accumulate(grid, fields, &state);
    Ok(state)
}

pub fn run_erosion<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    params: &ErosionParams,
) -> ErosionState {
    run_erosion_cancellable(grid, fields, params, &|| false).expect("uncancelled erosion")
}

pub fn run_erosion_cancellable<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    params: &ErosionParams,
    cancel: &dyn Fn() -> bool,
) -> Option<ErosionState> {
    run_erosion_observed(grid, fields, params, cancel, &mut |_, _| {})
}

pub fn run_erosion_observed<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    params: &ErosionParams,
    cancel: &dyn Fn() -> bool,
    on_step: &mut dyn FnMut(u32, &WorldFields),
) -> Option<ErosionState> {
    if cancel() {
        return None;
    }
    let mut state = refresh_hydrology(grid, fields);
    let mut sediment = SedimentBudget::default();
    let mut pending = vec![0.; grid.cell_count()];
    // Sub-ULP bed-volume remainder stays unplaced. Never move it to an unrelated
    // source cell merely to close a scalar ledger.
    let mut unplaced_roundoff = 0.;
    for iteration in 0..params.iterations {
        if cancel() {
            return None;
        }
        if iteration > 0 && iteration % 8 == 0 {
            state = refresh_hydrology(grid, fields);
        }
        // Rainfall-weighted discharge is separate from the stored contributing area.
        let rain_driven =
            fields.precipitation.iter().any(|&p| p > 0.) || !fields.external_water_m3_yr.is_empty();
        let liquid = fields.liquid_water();
        let mut discharge: Vec<f64> = (0..grid.cell_count())
            .map(|i| {
                fields.external_water_m3_yr.get(i).copied().unwrap_or(0.)
                    + grid.area_m2(i)
                        * if rain_driven {
                            crate::lakes::annual_runoff_m(
                                liquid[i] as f64,
                                fields.runoff_pet(i) as f64,
                            )
                        } else {
                            1.
                        }
            })
            .collect();
        for &cell in &state.topo_order {
            let i = cell as usize;
            let target = state.flow_dir[i];
            if target != u32::MAX {
                discharge[target as usize] += discharge[i];
            }
        }
        // The downstream solution must be available before its upstream cell.
        for &cell in state.topo_order.iter().rev() {
            let i = cell as usize;
            let receiver = state.flow_dir[i];
            if receiver == u32::MAX || fields.elevation[i] <= 0. {
                continue;
            }
            let j = receiver as usize;
            let dist = grid.distance_m(i, j).max(1.);
            let h = fields.elevation[i] as f64;
            let down = (fields.elevation[j] + fields.lake_depth[j]).max(0.) as f64;
            let k = params.k_river * params.timestep_years * discharge[i].powf(params.m) / dist;
            fields.elevation[i] = ((h + k * down) / (1. + k)).min(h).max(0.001) as f32;
            let volume = (h - fields.elevation[i] as f64).max(0.) * grid.area_m2(i);
            sediment.eroded_m3 += volume;
            pending[i] += volume;
        }
        if params.k_diffusion > 0. {
            let old = fields.elevation.clone();
            for i in 0..grid.cell_count() {
                if old[i] <= 0. || grid.boundary_mask()[i] {
                    continue;
                }
                let mut delta = 0.0;
                let mut total = 0.0;
                for &j in grid.neighbors(i) {
                    let j = j as usize;
                    if old[j] <= 0. {
                        continue;
                    }
                    let d = grid.distance_m(i, j).max(1.);
                    let w = (params.k_diffusion * params.timestep_years / (d * d)).min(0.12);
                    delta += (old[j] - old[i]) as f64 * w;
                    total += w;
                }
                fields.elevation[i] = (old[i] as f64 + delta / total.max(1.)).max(0.001) as f32;
            }
        }
        if params.k_deposition > 0.
            && ((iteration + 1) % 8 == 0 || iteration + 1 == params.iterations)
        {
            // k_deposition is an annual settling rate. Keep unresolved/unsunk
            // load explicitly instead of inventing terrain or losing material.
            let fraction = 1. - (-params.k_deposition * params.timestep_years).exp();
            let source: Vec<f64> = pending
                .iter_mut()
                .map(|v| {
                    let settled = *v * fraction;
                    *v -= settled;
                    settled
                })
                .collect();
            let (deposited, exported, unresolved) = crate::lakes::deposit(grid, fields, &source);
            sediment.deposited_m3 += deposited;
            sediment.exported_m3 += exported;
            unplaced_roundoff += unresolved;
            state = refresh_hydrology(grid, fields);
        }
        on_step(iteration + 1, fields);
    }
    if cancel() {
        None
    } else {
        let mut final_state = refresh_hydrology(grid, fields);
        sediment.suspended_m3 = pending.iter().sum::<f64>() + unplaced_roundoff;
        sediment.residual_m3 = sediment.eroded_m3
            - sediment.deposited_m3
            - sediment.exported_m3
            - sediment.suspended_m3;
        final_state.sediment = sediment;
        Some(final_state)
    }
}

/// Multi-source Dijkstra. Patch boundaries can inherit distance from their parent.
pub fn compute_distance_to_sea<G: GridOps>(grid: &G, fields: &mut WorldFields) {
    let n = grid.cell_count();
    let mut distance = vec![f64::INFINITY; n];
    let mut heap = BinaryHeap::new();
    for i in 0..n {
        let seed = if fields.elevation[i] <= 0. {
            Some(0.)
        } else if grid.boundary_mask()[i]
            && fields.distance_to_sea[i] > 0.
            && fields.distance_to_sea[i].is_finite()
        {
            Some(fields.distance_to_sea[i])
        } else {
            None
        };
        if let Some(d) = seed {
            distance[i] = d as f64;
            heap.push(Entry {
                height: d as f64,
                cell: i,
            });
        }
    }
    while let Some(Entry { height, cell }) = heap.pop() {
        if height > distance[cell] {
            continue;
        }
        for &j in grid.neighbors(cell) {
            let j = j as usize;
            let d = height + grid.distance_m(cell, j);
            if d < distance[j] {
                distance[j] = d;
                heap.push(Entry { height: d, cell: j })
            }
        }
    }
    for d in &mut distance {
        if !d.is_finite() {
            *d = std::f64::consts::PI * grid.radius_m()
        }
    }
    fields.distance_to_sea = distance.into_iter().map(|v| v as f32).collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tf_grid::HexGrid2D;
    #[test]
    fn inherited_inland_distance_terminates_on_regular_hex_lattice() {
        let grid =
            tf_grid::PatchGrid::new(&tf_grid::SphericalPatch::new(25.827, 127.469, 256000., 256));
        let mut fields = WorldFields::with_capacity(256 * 256);
        fields.elevation.fill(300.);
        fields.distance_to_sea.fill(300000.);
        compute_distance_to_sea(&grid, &mut fields);
        assert!(fields
            .distance_to_sea
            .iter()
            .all(|v| v.is_finite() && *v >= 300000. && *v < 450000.));
    }
    #[test]
    fn chain_conserves_drainage_area() {
        let g = HexGrid2D::new(4, 1, 1000., 0., 0.);
        let mut f = WorldFields::with_capacity(4);
        f.elevation = vec![3., 2., 1., -1.];
        // A one-row patch has open boundaries. Use a sphere-like closed boundary test wrapper.
        struct Closed(HexGrid2D, bitvec::prelude::BitVec);
        impl GridOps for Closed {
            fn cell_count(&self) -> usize {
                4
            }
            fn neighbors(&self, i: usize) -> &[u32] {
                self.0.neighbors(i)
            }
            fn distance_m(&self, a: usize, b: usize) -> f64 {
                self.0.distance_m(a, b)
            }
            fn area_m2(&self, i: usize) -> f64 {
                self.0.area_m2(i)
            }
            fn center_dir(&self, i: usize) -> tf_core::geo::UnitVec3 {
                self.0.center_dir(i)
            }
            fn latitude_deg(&self, i: usize) -> f64 {
                self.0.latitude_deg(i)
            }
            fn boundary_mask(&self) -> &bitvec::prelude::BitVec {
                &self.1
            }
        }
        let g = Closed(g, bitvec::bitvec![0;4]);
        let s = refresh_hydrology(&g, &mut f);
        assert_eq!(s.flow_dir, vec![1, 2, 3, u32::MAX]);
        for (i, &a) in f.flow_accumulation.iter().enumerate() {
            assert!((a as f64 / g.area_m2(0) - (i + 1) as f64).abs() < 1e-5)
        }
    }
    #[test]
    fn broad_basin_has_lake_and_outlet() {
        let g = HexGrid2D::new(7, 7, 1000., 0., 0.);
        let mut f = WorldFields::with_capacity(49);
        f.elevation.fill(5.);
        for r in 2..5 {
            for c in 2..5 {
                f.elevation[r * 7 + c] = 1.;
            }
        }
        f.precipitation.fill(2000.);
        f.potential_evaporation.fill(1000.);
        let s = refresh_hydrology(&g, &mut f);
        assert!(f.lake_depth[3 * 7 + 3] > 3.9);
        for i in 0..49 {
            let mut j = i;
            for step in 0..=49 {
                let d = s.flow_dir[j];
                if d == u32::MAX {
                    break;
                }
                assert!(step < 49, "drainage cycle");
                j = d as usize;
            }
        }
    }
    #[test]
    fn erosion_never_raises_land() {
        let g = HexGrid2D::new(12, 12, 1000., 0., 0.);
        let mut f = WorldFields::with_capacity(144);
        for i in 0..144 {
            f.elevation[i] = (i as f32 % 12.) * 40. + 1.;
        }
        let before = f.elevation.clone();
        run_erosion(
            &g,
            &mut f,
            &ErosionParams {
                iterations: 2,
                k_diffusion: 0.,
                ..Default::default()
            },
        );
        assert!(f.elevation.iter().zip(before).all(|(a, b)| *a <= b));
    }
}
