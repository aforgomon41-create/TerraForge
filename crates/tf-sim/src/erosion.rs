//! Priority-Flood drainage, area-conserving accumulation and implicit incision.
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

/// Flood outward from oceans and open map boundaries. The flood parent drains
/// equal-height flats without adding arbitrary epsilon elevations to the ground.
fn drainage<G: GridOps>(grid: &G, fields: &mut WorldFields) -> ErosionState {
    let n = grid.cell_count();
    let mut seen = vec![false; n];
    let mut levels = fields.elevation.clone();
    let mut receivers = vec![u32::MAX; n];
    let mut heap = BinaryHeap::new();
    let mut pit = VecDeque::new();
    let mut order = Vec::with_capacity(n);
    let boundary = grid.boundary_mask();
    for i in 0..n {
        if fields.elevation[i] <= 0. {
            seen[i] = true;
            order.push(i as u32);
            if grid
                .neighbors(i)
                .iter()
                .any(|&j| fields.elevation[j as usize] > 0.)
            {
                heap.push(Entry {
                    height: 0.,
                    cell: i,
                });
            }
        } else if boundary[i] {
            seen[i] = true;
            heap.push(Entry {
                height: levels[i] as f64,
                cell: i,
            });
        }
    }
    if heap.is_empty() && order.len() < n {
        let i = (0..n)
            .min_by(|&a, &b| levels[a].total_cmp(&levels[b]))
            .unwrap();
        seen[i] = true;
        heap.push(Entry {
            height: levels[i] as f64,
            cell: i,
        });
    }
    let mut rank = vec![usize::MAX; n];
    while !heap.is_empty() || !pit.is_empty() {
        let cur = if let Some(i) = pit.pop_front() {
            i
        } else {
            heap.pop().unwrap().cell
        };
        if rank[cur] != usize::MAX {
            continue;
        }
        rank[cur] = order.len();
        if fields.elevation[cur] > 0. {
            order.push(cur as u32)
        }
        let level = if fields.elevation[cur] <= 0. {
            0.
        } else {
            levels[cur]
        };
        for &nb in grid.neighbors(cur) {
            let j = nb as usize;
            if seen[j] {
                continue;
            }
            seen[j] = true;
            receivers[j] = cur as u32;
            if levels[j] <= level {
                levels[j] = level;
                pit.push_back(j)
            } else {
                heap.push(Entry {
                    height: levels[j] as f64,
                    cell: j,
                })
            }
        }
    }
    // Oceans have no receiver; every positive cell is reached, including flats.
    for i in 0..n {
        fields.lake_depth[i] = if fields.elevation[i] > 0. {
            (levels[i] - fields.elevation[i]).max(0.)
        } else {
            0.
        };
        if receivers[i] == u32::MAX {
            continue;
        }
        let mut best = receivers[i] as usize;
        let mut best_slope = -1.0;
        for &j in grid.neighbors(i) {
            let j = j as usize;
            if rank[j] >= rank[i] && fields.elevation[j] > 0. {
                continue;
            }
            let target = if fields.elevation[j] <= 0. {
                0.
            } else {
                levels[j]
            };
            let slope = (levels[i] - target) as f64 / grid.distance_m(i, j).max(1e-6);
            if slope > best_slope && target <= levels[i] {
                best_slope = slope;
                best = j;
            }
        }
        receivers[i] = best as u32;
    }
    order.reverse();
    ErosionState {
        flow_dir: receivers,
        topo_order: order,
    }
}

fn accumulate<G: GridOps>(grid: &G, fields: &mut WorldFields, state: &ErosionState) {
    for i in 0..grid.cell_count() {
        fields.flow_accumulation[i] = grid.area_m2(i) as f32;
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
    if cancel() {
        return None;
    }
    let mut state = refresh_hydrology(grid, fields);
    for iteration in 0..params.iterations {
        if cancel() {
            return None;
        }
        if iteration > 0 && iteration % 8 == 0 {
            state = refresh_hydrology(grid, fields);
        }
        // The downstream solution must be available before its upstream cell.
        for &cell in state.topo_order.iter().rev() {
            let i = cell as usize;
            let receiver = state.flow_dir[i];
            if receiver == u32::MAX || fields.elevation[i] <= 0. || fields.lake_depth[i] > 0.01 {
                continue;
            }
            let j = receiver as usize;
            let dist = grid.distance_m(i, j).max(1.);
            let h = fields.elevation[i] as f64;
            let down = (fields.elevation[j] + fields.lake_depth[j]).max(0.) as f64;
            let k = params.k_river
                * params.timestep_years
                * (fields.flow_accumulation[i] as f64).powf(params.m)
                / dist;
            fields.elevation[i] = ((h + k * down) / (1. + k)).min(h).max(0.001) as f32;
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
    }
    if cancel() {
        None
    } else {
        Some(refresh_hydrology(grid, fields))
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
