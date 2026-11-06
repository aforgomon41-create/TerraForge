//! Depression hierarchies, annual surface-water balance and finite sediment storage.
//!
//! A topographic spill capacity is not a measured lake depth. Disconnected child
//! basins exchange overflow at their actual saddle before a common surface forms.
//! This is a steady annual model (Turc–Pike land ET); groundwater and seasonal
//! storage are not solved. Inspired by Barnes et al., ESurf 2020/2021.
use crate::WorldFields;
use serde::{Deserialize, Serialize};
use tf_grid::GridOps;

const OUT: usize = usize::MAX;
#[derive(Clone, Copy, PartialEq)]
struct Head(f64, usize);
impl Eq for Head {}
impl PartialOrd for Head {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Head {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&o.0).then_with(|| self.1.cmp(&o.1))
    }
}

/// Marine outlets for sediment transport: below-sea-level pits enclosed by land
/// are sediment traps, not unlimited exports. A closed spherical domain keeps
/// its largest sea; finite maps keep water connected to an open map boundary.
pub fn marine_cells<G: GridOps>(grid: &G, elevation: &[f32]) -> Vec<bool> {
    let n = grid.cell_count();
    let mut labels = vec![OUT; n];
    let mut seas = Vec::new();
    let boundary = grid.boundary_mask();
    for i in 0..n {
        if elevation[i] > 0. || labels[i] != OUT {
            continue;
        }
        let id = seas.len();
        let mut cells = vec![i];
        labels[i] = id;
        let (mut area, mut open, mut cursor) = (0., false, 0);
        while cursor < cells.len() {
            let cell = cells[cursor];
            cursor += 1;
            area += grid.area_m2(cell);
            open |= boundary[cell];
            for &j in grid.neighbors(cell) {
                let j = j as usize;
                if elevation[j] <= 0. && labels[j] == OUT {
                    labels[j] = id;
                    cells.push(j);
                }
            }
        }
        seas.push((area, open));
    }
    let largest = if boundary.any() {
        None
    } else {
        (0..seas.len()).max_by(|&a, &b| seas[a].0.total_cmp(&seas[b].0))
    };
    labels
        .into_iter()
        .map(|id| id != OUT && (Some(id) == largest || seas[id].1))
        .collect()
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct LakeBudget {
    pub precipitation_m3_yr: f64,
    #[serde(default)]
    pub external_inflow_m3_yr: f64,
    /// Water replacing extra evaporation from an externally maintained stage.
    #[serde(default)]
    pub prescribed_head_supply_m3_yr: f64,
    /// Rain/snow retained as solid water, net of melt from older snow/ice.
    #[serde(default)]
    pub solid_water_change_m3_yr: f64,
    pub land_evaporation_m3_yr: f64,
    pub additional_lake_evaporation_m3_yr: f64,
    pub terminal_runoff_m3_yr: f64,
    pub residual_m3_yr: f64,
    pub lake_area_m2: f64,
    pub lake_storage_m3: f64,
}

/// Turc–Pike annual reference-soil runoff, metres/year. No source without rain.
pub fn annual_runoff_m(rain_mm: f64, pet_mm: f64) -> f64 {
    let p = rain_mm.max(0.);
    let e = pet_mm.max(0.);
    if p == 0. {
        0.
    } else {
        (p - p * (e / p.hypot(e))).max(0.) / 1000.
    }
}

#[derive(Default)]
struct Basin {
    children: Option<[usize; 2]>,
    entry: [usize; 2],
    parent: usize,
    bottom: f64,
    spill: f64,
    cells: Vec<usize>,
    source: f64,
    extra: f64,
    // All solver amounts share one unit per invocation: annual additional
    // evaporation demand (m3/year), or solid sediment capacity (m3). They are
    // never compared to each other or persisted as physical lake depth.
    full_cost: f64,
    child_cost: f64,
    child_area: f64,
    area: f64,
    /// Some(surface) is a merged body, None retains distinct child surfaces.
    surface: Option<f64>,
    overflow: f64,
    forwarded: [f64; 2],
}

struct Hierarchy {
    nodes: Vec<Basin>,
    owner: Vec<usize>,
    geometry: Vec<usize>,
    height: Vec<f64>,
    area: Vec<f64>,
    width: Vec<f64>,
    weight: Vec<f64>,
    sediment: bool,
    outside: f64,
}

fn find(parent: &mut [usize], mut cell: usize) -> usize {
    let mut root = cell;
    while parent[root] != root {
        root = parent[root];
    }
    while parent[cell] != cell {
        let next = parent[cell];
        parent[cell] = root;
        cell = next;
    }
    root
}

impl Hierarchy {
    fn build<G: GridOps>(grid: &G, elevation: &[f32], trap_subsea_sediment: bool) -> Self {
        let n = grid.cell_count();
        let mut h = Self {
            nodes: Vec::new(),
            owner: vec![OUT; n],
            geometry: vec![OUT; n],
            height: elevation.iter().map(|&v| v as f64).collect(),
            area: (0..n).map(|i| grid.area_m2(i)).collect(),
            width: vec![0.; n],
            weight: vec![0.; n],
            sediment: false,
            outside: 0.,
        };
        if n == 0 {
            return h;
        }
        let mut parent: Vec<usize> = (0..n).collect();
        let mut active: Vec<bool> = if trap_subsea_sediment {
            marine_cells(grid, elevation)
        } else {
            elevation.iter().map(|&z| z <= 0.).collect()
        };
        let mut component = vec![OUT; n];
        let boundary = grid.boundary_mask();
        // Preserve the existing no-ocean/closed-domain terminal convention.
        let fallback = if !active.iter().any(|&v| v) && !boundary.any() {
            (0..n).min_by(|&a, &b| elevation[a].total_cmp(&elevation[b]))
        } else {
            None
        };
        let mut order: Vec<usize> = (0..n).filter(|&i| !active[i]).collect();
        order.sort_unstable_by(|&a, &b| elevation[a].total_cmp(&elevation[b]).then(a.cmp(&b)));
        for i in order {
            let z = h.height[i];
            let mut roots: Vec<(usize, usize)> = Vec::with_capacity(6);
            let mut steepest = None;
            let mut best = -1.;
            for &nb in grid.neighbors(i) {
                let j = nb as usize;
                if !active[j] {
                    continue;
                }
                let slope = (z - h.height[j]) / grid.distance_m(i, j).max(1.);
                if slope > best || (slope == best && steepest.is_some_and(|k| j < k)) {
                    best = slope;
                    steepest = Some(j);
                }
                let root = find(&mut parent, j);
                if !roots.iter().any(|&(r, _)| r == root) {
                    roots.push((root, j));
                }
            }
            let outside = boundary[i]
                || fallback == Some(i)
                || roots.iter().any(|&(r, _)| component[r] == OUT);
            h.owner[i] = steepest.map_or(OUT, |j| h.owner[j]);
            let node = if outside {
                for &(root, _) in &roots {
                    let node = component[root];
                    if node != OUT {
                        h.nodes[node].spill = z;
                    }
                }
                OUT
            } else if roots.is_empty() {
                let id = h.nodes.len();
                h.nodes.push(Basin {
                    parent: OUT,
                    bottom: z,
                    spill: f64::INFINITY,
                    ..Default::default()
                });
                h.owner[i] = id;
                id
            } else {
                let (first, j) = roots[0];
                let mut node = component[first];
                let first_entry = h.owner[j];
                for &(root, j) in &roots[1..] {
                    let right = component[root];
                    let id = h.nodes.len();
                    h.nodes[node].spill = z;
                    h.nodes[node].parent = id;
                    h.nodes[right].spill = z;
                    h.nodes[right].parent = id;
                    h.nodes.push(Basin {
                        children: Some([node, right]),
                        entry: [first_entry, h.owner[j]],
                        parent: OUT,
                        bottom: z,
                        spill: f64::INFINITY,
                        ..Default::default()
                    });
                    node = id;
                }
                node
            };
            if boundary[i] || fallback == Some(i) {
                h.owner[i] = OUT;
            }
            active[i] = true;
            for &(root, _) in &roots {
                parent[root] = i;
            }
            component[i] = node;
            h.geometry[i] = node;
        }
        for i in 0..n {
            let mut node = h.geometry[i];
            // Saddle/flat cells belong above their activation level, not to a
            // zero-capacity child. This also avoids artificial flat-area lakes.
            while node != OUT && h.height[i] >= h.nodes[node].spill {
                node = h.nodes[node].parent;
            }
            h.geometry[i] = node;
            if node != OUT {
                h.nodes[node].cells.push(i);
                let relief = grid
                    .neighbors(i)
                    .iter()
                    .map(|&j| (h.height[j as usize] - h.height[i]).abs())
                    .fold(0., f64::max);
                // Continuous sub-cell shoreline approximation; capped at the
                // true spill. It is not an extra depth added to the terrain.
                h.width[i] = (relief * 0.5)
                    .max(0.1)
                    .min(h.nodes[node].spill - h.height[i]);
            }
        }
        h
    }

    fn prepare(&mut self, source: &[f64], weight: &[f64], sediment: bool) {
        self.sediment = sediment;
        self.weight.copy_from_slice(weight);
        for (i, &value) in source.iter().enumerate() {
            if self.owner[i] == OUT {
                self.outside += value;
            } else {
                self.nodes[self.owner[i]].source += value;
            }
        }
        for i in 0..self.nodes.len() {
            let (mut child_cost, mut child_area, mut source) = (0., 0., self.nodes[i].source);
            if let Some(children) = self.nodes[i].children {
                for child in children {
                    child_cost += self.nodes[child].full_cost;
                    child_area += self.nodes[child].area;
                    source += self.nodes[child].source;
                }
            }
            self.nodes[i].source = source;
            self.nodes[i].child_cost = child_cost;
            self.nodes[i].child_area = child_area;
            self.nodes[i].area = child_area
                + self.nodes[i]
                    .cells
                    .iter()
                    .map(|&c| self.area[c])
                    .sum::<f64>();
            self.nodes[i].full_cost = self.cost(i, self.nodes[i].spill);
        }
    }

    fn cost(&self, i: usize, level: f64) -> f64 {
        let node = &self.nodes[i];
        let own = node
            .cells
            .iter()
            .map(|&c| {
                if self.sediment {
                    self.area[c] * (level - self.height[c]).max(0.)
                } else {
                    self.weight[c] * ((level - self.height[c]) / self.width[c]).clamp(0., 1.)
                }
            })
            .sum::<f64>();
        node.child_cost
            + own
            + if self.sediment {
                node.child_area * (level - node.bottom).max(0.)
            } else {
                0.
            }
    }

    /// Iterative work list: deeply nested basins do not consume the Rust stack.
    fn solve(&mut self) {
        for i in 0..self.nodes.len() {
            let mut work = vec![i];
            while let Some(id) = work.pop() {
                let supply = self.nodes[id].source + self.nodes[id].extra;
                let full = self.nodes[id].full_cost;
                if supply <= 0. {
                    self.nodes[id].surface = None;
                    self.nodes[id].overflow = 0.;
                    continue;
                }
                if supply >= full {
                    self.nodes[id].surface = Some(self.nodes[id].spill);
                    self.nodes[id].overflow = supply - full;
                    continue;
                }
                self.nodes[id].overflow = 0.;
                if supply >= self.nodes[id].child_cost {
                    let (mut lo, mut hi) = (self.nodes[id].bottom, self.nodes[id].spill);
                    for _ in 0..48 {
                        let mid = (lo + hi) * 0.5;
                        if self.cost(id, mid) > supply {
                            hi = mid;
                        } else {
                            lo = mid;
                        }
                    }
                    self.nodes[id].surface = Some((lo + hi) * 0.5);
                    continue;
                }
                self.nodes[id].surface = None;
                let Some(children) = self.nodes[id].children else {
                    continue;
                };
                for from in 0..2 {
                    let to = 1 - from;
                    let child = children[to];
                    let unused = (self.nodes[children[from]].overflow
                        - self.nodes[id].forwarded[from])
                        .max(0.);
                    let deficit = (self.nodes[child].full_cost
                        - self.nodes[child].source
                        - self.nodes[child].extra)
                        .max(0.);
                    let amount = unused.min(deficit);
                    if amount <= 1.0e-12 * supply.max(full).max(1.) {
                        continue;
                    }
                    self.nodes[id].forwarded[from] += amount;
                    work.push(id);
                    if self.nodes[child].surface.is_some() {
                        self.nodes[child].extra += amount;
                        work.push(child);
                    } else {
                        let mut path = Vec::new();
                        let mut leaf = self.nodes[id].entry[to];
                        loop {
                            assert!(leaf != OUT && path.len() <= self.nodes.len());
                            self.nodes[leaf].extra += amount;
                            path.push(leaf);
                            if leaf == child {
                                break;
                            }
                            leaf = self.nodes[leaf].parent;
                        }
                        work.extend(path.into_iter().rev());
                    }
                    break;
                }
            }
        }
    }

    fn surfaces(&self) -> Vec<f64> {
        // Build appends parents after children; reverse order propagates a
        // merged water/sediment surface down that same hierarchy.
        let mut inherited = vec![None; self.nodes.len()];
        let mut levels = self.height.clone();
        for i in (0..self.nodes.len()).rev() {
            let surface = inherited[i].or(self.nodes[i].surface);
            if let Some(level) = surface {
                for &cell in &self.nodes[i].cells {
                    levels[cell] = levels[cell].max(level);
                }
                if let Some(children) = self.nodes[i].children {
                    for child in children {
                        inherited[child] = Some(level);
                    }
                }
            }
        }
        levels
    }

    fn export(&self) -> f64 {
        self.outside
            + self
                .nodes
                .iter()
                .filter(|n| n.parent == OUT)
                .map(|n| n.overflow)
                .sum::<f64>()
    }
}

/// Annual equilibrium surface, without converting every spill capacity to water.
pub fn balance<G: GridOps>(grid: &G, fields: &mut WorldFields) -> (Vec<f64>, LakeBudget) {
    let liquid = fields.liquid_water();
    if !liquid.iter().any(|&p| p > 0.)
        && !fields.external_water_m3_yr.iter().any(|&q| q > 0.)
        && fields.boundary_stages.is_empty()
    {
        fields.lake_depth.fill(0.);
        let solid = (0..grid.cell_count())
            .filter(|&i| fields.elevation[i] > 0.)
            .map(|i| {
                fields.precipitation.get(i).copied().unwrap_or(0.).max(0.) as f64 / 1000.
                    * grid.area_m2(i)
            })
            .sum();
        return (
            fields.elevation.iter().map(|&h| h as f64).collect(),
            LakeBudget {
                precipitation_m3_yr: solid,
                solid_water_change_m3_yr: solid,
                ..Default::default()
            },
        );
    }
    let mut tree = Hierarchy::build(grid, &fields.elevation, false);
    let n = grid.cell_count();
    let mut source = vec![0.; n];
    let mut loss = vec![0.; n];
    let mut budget = LakeBudget::default();
    for i in 0..n {
        let external = fields.external_water_m3_yr.get(i).copied().unwrap_or(0.);
        budget.external_inflow_m3_yr += external;
        if fields.elevation[i] <= 0. {
            source[i] = external;
            continue;
        }
        let rain = (liquid[i] as f64).max(0.) / 1000.;
        let pet = (fields.runoff_pet(i) as f64).max(0.) / 1000.;
        source[i] = annual_runoff_m(rain * 1000., pet * 1000.) * tree.area[i];
        let actual_et = (rain * tree.area[i] - source[i]).max(0.);
        source[i] += external;
        budget.precipitation_m3_yr += fields.precipitation[i].max(0.) as f64 / 1000. * tree.area[i];
        budget.solid_water_change_m3_yr +=
            (fields.precipitation[i].max(0.) as f64 / 1000. - rain) * tree.area[i];
        budget.land_evaporation_m3_yr += actual_et;
        loss[i] = (pet * tree.area[i] - actual_et).max(0.);
    }
    tree.prepare(&source, &loss, false);
    tree.solve();
    let natural = tree.surfaces();
    let mut surfaces = natural.clone();
    // Minimum escape level across ALL exits. A high mountain-side outlet
    // cannot hold the entire crop underwater when a lower exit is available.
    let mut escape = if fields.boundary_stages.is_empty() {
        vec![]
    } else {
        vec![f64::INFINITY; n]
    };
    let mut pending = std::collections::BinaryHeap::new();
    if !fields.boundary_stages.is_empty() {
        let mut stages = vec![f64::NEG_INFINITY; n];
        for &(i, z) in &fields.boundary_stages {
            stages[i as usize] = stages[i as usize].max(z);
        }
        for i in 0..n {
            if grid.boundary_mask()[i] || tree.height[i] <= 0. {
                let z = tree.height[i].max(if tree.height[i] <= 0. { 0. } else { stages[i] });
                escape[i] = z;
                pending.push(std::cmp::Reverse(Head(z, i)));
            }
        }
        while let Some(std::cmp::Reverse(Head(z, i))) = pending.pop() {
            if z > escape[i] {
                continue;
            }
            for &j in grid.neighbors(i) {
                let j = j as usize;
                let next = z.max(tree.height[j]);
                if next < escape[j] {
                    escape[j] = next;
                    pending.push(std::cmp::Reverse(Head(next, j)));
                }
            }
        }
    }
    // Fixed downstream heads create connected backwater, never a raised bed.
    // The replacement water needed for its evaporation has its own ledger.
    let mut queue = std::collections::BinaryHeap::new();
    for &(i, z) in &fields.boundary_stages {
        let i = i as usize;
        let z = z.min(escape[i]);
        if z > surfaces[i] {
            surfaces[i] = z;
            queue.push(Head(z, i));
        }
    }
    while let Some(Head(z, i)) = queue.pop() {
        if z < surfaces[i] {
            continue;
        }
        for &j in grid.neighbors(i) {
            let j = j as usize;
            let head = z.min(escape[j]);
            if tree.height[j] < head && surfaces[j] < head {
                surfaces[j] = head;
                queue.push(Head(head, j));
            }
        }
    }
    if fields.external_area_m2.len() == n {
        fields.lake_runoff_loss_fraction = vec![0.; n];
    }
    for i in 0..n {
        if !fields.boundary_stages.is_empty() && fields.elevation[i] <= 0. {
            fields.lake_depth[i] = 0.;
            continue;
        }
        let depth = (surfaces[i] - tree.height[i]).max(0.);
        fields.lake_depth[i] = depth as f32;
        let natural_fraction =
            ((natural[i] - tree.height[i]).max(0.) / tree.width[i].max(1e-12)).clamp(0., 1.);
        if fields.lake_runoff_loss_fraction.len() == n {
            fields.lake_runoff_loss_fraction[i] = natural_fraction as f32;
        }
        if depth > 0. {
            let fraction = (depth / tree.width[i].max(1e-12)).clamp(0., 1.);
            budget.additional_lake_evaporation_m3_yr += loss[i] * fraction;
            budget.prescribed_head_supply_m3_yr += loss[i] * (fraction - natural_fraction).max(0.);
            budget.lake_area_m2 += tree.area[i] * fraction;
            budget.lake_storage_m3 += tree.area[i]
                * if depth < tree.width[i] {
                    depth * depth / (2. * tree.width[i])
                } else {
                    depth - tree.width[i] * 0.5
                };
        }
    }
    budget.terminal_runoff_m3_yr = tree.export();
    budget.residual_m3_yr = budget.precipitation_m3_yr
        + budget.external_inflow_m3_yr
        + budget.prescribed_head_supply_m3_yr
        - budget.solid_water_change_m3_yr
        - budget.land_evaporation_m3_yr
        - budget.additional_lake_evaporation_m3_yr
        - budget.terminal_runoff_m3_yr;
    (surfaces, budget)
}

/// Add transported solid volume to low basins, overflowing only after their
/// sediment capacity is exhausted. Actual elevation is changed; no visual cap.
pub fn deposit<G: GridOps>(
    grid: &G,
    fields: &mut WorldFields,
    source_m3: &[f64],
) -> (f64, f64, f64) {
    if !source_m3.iter().any(|&s| s > 0.) {
        return (0., 0., 0.);
    }
    let mut tree = Hierarchy::build(grid, &fields.elevation, true);
    tree.prepare(source_m3, &vec![0.; grid.cell_count()], true);
    tree.solve();
    let surfaces = tree.surfaces();
    let mut deposited = 0.;
    for (i, &height) in surfaces.iter().enumerate() {
        let mut new = height as f32;
        if new as f64 > height {
            new = f32::from_bits(if new > 0. {
                new.to_bits() - 1
            } else {
                new.to_bits() + 1
            });
        }
        deposited += (new - fields.elevation[i]).max(0.) as f64 * tree.area[i];
        fields.elevation[i] = new;
    }
    let exported = tree.export();
    let unresolved = (source_m3.iter().sum::<f64>() - deposited - exported).max(0.);
    (deposited, exported, unresolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitvec::prelude::*;
    struct Line {
        neighbors: Vec<Vec<u32>>,
        boundary: BitVec,
    }
    impl Line {
        fn new(n: usize) -> Self {
            let mut boundary = bitvec![0;n];
            if n > 0 {
                boundary.set(0, true);
                boundary.set(n - 1, true);
            }
            Self {
                neighbors: (0..n)
                    .map(|i| {
                        (i.saturating_sub(1)..=(i + 1).min(n - 1))
                            .filter(|&j| j != i)
                            .map(|j| j as u32)
                            .collect()
                    })
                    .collect(),
                boundary,
            }
        }
    }
    impl GridOps for Line {
        fn cell_count(&self) -> usize {
            self.neighbors.len()
        }
        fn neighbors(&self, i: usize) -> &[u32] {
            &self.neighbors[i]
        }
        fn distance_m(&self, a: usize, b: usize) -> f64 {
            (a as f64 - b as f64).abs() * 1000.
        }
        fn area_m2(&self, _: usize) -> f64 {
            1_000_000.
        }
        fn center_dir(&self, i: usize) -> tf_core::geo::UnitVec3 {
            tf_core::geo::UnitVec3::from_lat_lon_deg(0., i as f64 * 0.01)
        }
        fn latitude_deg(&self, _: usize) -> f64 {
            0.
        }
        fn boundary_mask(&self) -> &BitVec {
            &self.boundary
        }
    }
    fn bowl() -> (Line, WorldFields) {
        let grid = Line::new(6);
        let mut f = WorldFields::with_capacity(6);
        f.elevation = vec![-1., 5., 1., 1., 5., -1.];
        (grid, f)
    }
    fn closes(b: &LakeBudget) {
        assert!(
            b.residual_m3_yr.abs() <= b.precipitation_m3_yr.max(1.) * 1e-9,
            "{b:?}"
        );
    }
    #[test]
    fn a_spill_capacity_without_water_is_not_a_lake() {
        let (g, mut f) = bowl();
        f.potential_evaporation.fill(1000.);
        let (_, b) = balance(&g, &mut f);
        assert!(f.lake_depth.iter().all(|&d| d == 0.));
        closes(&b);
    }
    #[test]
    fn humid_basin_spills_but_arid_basin_does_not_fill_its_capacity() {
        let (g, mut f) = bowl();
        f.precipitation.fill(2000.);
        f.potential_evaporation.fill(1000.);
        let (_, wet) = balance(&g, &mut f);
        assert_eq!(f.lake_depth[2], 4.);
        assert!(wet.terminal_runoff_m3_yr > 0.);
        closes(&wet);
        f.precipitation.fill(50.);
        f.potential_evaporation.fill(2000.);
        let (_, dry) = balance(&g, &mut f);
        assert!(f.lake_depth[2] < 0.01 && dry.lake_area_m2 < wet.lake_area_m2 * 0.01);
        closes(&dry);
    }
    #[test]
    fn upstream_water_can_sustain_a_lake_in_a_locally_arid_basin() {
        let g = Line::new(7);
        let mut f = WorldFields::with_capacity(7);
        f.elevation = vec![-1., 9., 8., 1., 1., 5., -1.];
        f.precipitation.fill(50.);
        f.potential_evaporation.fill(2000.);
        f.precipitation[2] = 20000.;
        f.potential_evaporation[2] = 1000.;
        let (_, b) = balance(&g, &mut f);
        assert!(f.lake_depth[3] > 3.9 && f.lake_depth[4] > 3.9);
        closes(&b);
    }
    #[test]
    fn nested_children_exchange_spill_without_equalizing_disconnected_surfaces() {
        let g = Line::new(9);
        let mut f = WorldFields::with_capacity(9);
        f.elevation = vec![-1., 6., 2., 1., 4., 3., 4., 5., -1.];
        f.potential_evaporation.fill(1000.);
        f.potential_evaporation[5] = 3000.;
        f.precipitation[2] = 2000.;
        f.precipitation[3] = 2000.;
        let (s, b) = balance(&g, &mut f);
        assert!((s[3] - 4.).abs() < 1e-6, "{s:?}");
        assert!(
            s[5] > 3. && s[5] < 4.,
            "overflow should sustain a lower separate lake: {s:?}"
        );
        assert_eq!(f.lake_depth[4], 0.);
        closes(&b);
    }
    #[test]
    fn sediment_changes_real_bed_and_conserves_volume() {
        let (g, mut f) = bowl();
        let before = f.elevation.clone();
        let zero = vec![0.; 6];
        let (d, e, u) = deposit(&g, &mut f, &zero);
        assert_eq!(f.elevation, before);
        assert_eq!((d, e, u), (0., 0., 0.));
        let mut source = zero;
        source[2] = 1_000_000.;
        let (d, e, u) = deposit(&g, &mut f, &source);
        assert!(f.elevation[2] > 1. && f.elevation[3] > 1.);
        assert!(f.elevation.iter().zip(before).all(|(&now, old)| now >= old));
        assert!((d + e + u - source.iter().sum::<f64>()).abs() < 1e-6);
        assert_eq!(e, 0.);
    }
    #[test]
    fn enclosed_subsea_pits_trap_sediment_but_connected_seas_do_not() {
        let g = Line::new(7);
        let mut f = WorldFields::with_capacity(7);
        f.elevation = vec![-10., 9., 6., -2., -2., 5., -10.];
        assert_eq!(
            marine_cells(&g, &f.elevation),
            vec![true, false, false, false, false, false, true]
        );
        let mut source = vec![0.; 7];
        source[2] = 10e6;
        let (d, e, u) = deposit(&g, &mut f, &source);
        assert!(f.elevation[3] > 0. && f.elevation[4] > 0.);
        assert_eq!(f.elevation[0], -10.);
        assert_eq!(f.elevation[6], -10.);
        assert!((d + e + u - 10e6).abs() < 1e-6);
    }
    #[test]
    fn zero_depth_flats_and_deep_hierarchies_do_not_create_water_or_recurse() {
        let mut heights = vec![-1., 4098., 1.];
        for k in 2..2048 {
            heights.extend([k as f32, 0.5]);
        }
        heights.extend([4097., -1.]);
        let g = Line::new(heights.len());
        let mut f = WorldFields::with_capacity(heights.len());
        f.elevation = heights;
        let (_, b) = balance(&g, &mut f);
        assert!(f.lake_depth.iter().all(|&d| d == 0.));
        closes(&b);
        f.precipitation.fill(50.);
        f.potential_evaporation.fill(2000.);
        let (_, b) = balance(&g, &mut f);
        closes(&b);
    }
    #[test]
    fn varied_terrain_and_climates_close_water_and_sediment_budgets() {
        use tf_core::DeterministicRng;
        let grid = tf_grid::FlatGrid::new(31, 23, 31000., 25., 0., 6371000.);
        for seed in 0..32 {
            let mut rng = DeterministicRng::derive(seed, "lake-budget-regression");
            let mut f = WorldFields::with_capacity(grid.cell_count());
            for i in 0..grid.cell_count() {
                // Quantized terraces, enclosed pits, open boundaries and oceans.
                f.elevation[i] = rng.below(25) as f32 - if seed % 3 == 0 { 2. } else { 0. };
                f.precipitation[i] = rng.range_f64(0., 2500.) as f32;
                f.potential_evaporation[i] = if seed % 4 == 0 {
                    0.
                } else {
                    rng.range_f64(100., 3000.) as f32
                };
            }
            let old = f.elevation.clone();
            let (_, b) = balance(&grid, &mut f);
            closes(&b);
            assert_eq!(f.elevation, old, "water must not modify terrain");
            let state = crate::erosion::refresh_hydrology(&grid, &mut f);
            assert_eq!(state.topo_order.len(), grid.cell_count());
            let source: Vec<_> = (0..grid.cell_count())
                .map(|_| rng.range_f64(0., 2.5e6))
                .collect();
            let total: f64 = source.iter().sum();
            let (d, e, u) = deposit(&grid, &mut f, &source);
            assert!(
                (d + e + u - total).abs() < total * 1e-8,
                "seed {seed}: {d}/{e}/{u}/{total}"
            );
            assert!(f.elevation.iter().zip(old).all(|(&new, old)| new >= old));
        }
    }
    #[test]
    fn two_ocean_outlets_do_not_merge_across_the_highest_divide() {
        let g = Line::new(11);
        let mut f = WorldFields::with_capacity(11);
        f.elevation = vec![-1., 5., 1., 1., 5., 9., 5., 1., 1., 5., -1.];
        f.precipitation.fill(2000.);
        f.potential_evaporation.fill(1000.);
        let (surface, b) = balance(&g, &mut f);
        assert_eq!(surface, vec![-1., 5., 5., 5., 5., 9., 5., 5., 5., 5., -1.]);
        closes(&b);
        let source = vec![1e6; 11];
        let (d, e, u) = deposit(&g, &mut f, &source);
        assert!(f.elevation.iter().all(|h| h.is_finite()));
        assert!((d + e + u - 11e6).abs() < 1e-6);
    }
    #[test]
    fn all_closed_and_open_domain_basins_acquire_finite_spills() {
        use tf_grid::GridDescriptor;
        for descriptor in [
            GridDescriptor::sphere(4, 6371000.),
            GridDescriptor::plane(25., 0., 31000., 31, 23, 6371000.),
        ] {
            let grid = descriptor.build();
            for seed in 0..64 {
                let mut rng = tf_core::DeterministicRng::derive(seed, "finite-spill-invariant");
                let height: Vec<_> = (0..grid.cell_count())
                    .map(|_| rng.below(25) as f32 - if seed % 2 == 0 { 2. } else { 0. })
                    .collect();
                for sediment in [false, true] {
                    let hierarchy = Hierarchy::build(&grid, &height, sediment);
                    for (i, node) in hierarchy.nodes.iter().enumerate() {
                        assert!(
                            node.spill.is_finite(),
                            "{descriptor:?} seed {seed}, node {i}"
                        );
                        assert!(node.parent == OUT || node.parent > i);
                    }
                }
            }
        }
    }
}
