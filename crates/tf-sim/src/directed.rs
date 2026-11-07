//! Local surface forcing on an existing finite map. Selections use source-grid
//! cell IDs, so changing the view or projection cannot move an operation.
use crate::{erosion, WorldFields};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, collections::BinaryHeap};
use tf_grid::GridOps;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Footprint {
    pub cells: Vec<u32>,
    pub radius_m: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Process {
    Uplift,
    Subsidence,
    Erosion,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub process: Process,
    pub area: Footprint,
    pub duration_years: u32,
    /// Metres per thousand years for vertical movement, relative strength for erosion.
    pub intensity: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Strait {
    /// The first and last points mark existing sea on each side of the gap.
    pub corridor: Footprint,
    pub minimum_width_m: f64,
    pub minimum_depth_m: f64,
    pub repair: Option<StraitRepair>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StraitRepair {
    pub maximum_subsidence_m: f64,
    pub duration_years: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    #[serde(default)]
    pub operations: Vec<Operation>,
    #[serde(default)]
    pub protected_areas: Vec<Footprint>,
    pub strait: Option<Strait>,
}

impl Plan {
    pub fn validate(&self, cells: usize, spacing: f64, extent: (f64, f64)) -> Result<(), String> {
        let footprint = |area: &Footprint| {
            !area.cells.is_empty()
                && area.cells.len() <= 12
                && area.cells.iter().all(|&i| (i as usize) < cells)
                && (spacing..=extent.0.hypot(extent.1)).contains(&area.radius_m)
        };
        let duration = |years| (100..=1_000_000).contains(&years);
        if self.operations.len() > 8
            || self.protected_areas.len() > 8
            || (self.operations.is_empty() && self.strait.is_none())
            || self.protected_areas.iter().any(|a| !footprint(a))
        {
            return Err("请提供演化操作或海峡条件，操作与保护区各最多 8 个".into());
        }
        let mut erosion_steps = 0;
        for op in &self.operations {
            if !footprint(&op.area)
                || !duration(op.duration_years)
                || !(0.001..=5.).contains(&op.intensity)
                || (!matches!(op.process, Process::Erosion)
                    && op.intensity * op.duration_years as f64 / 1000. > 3000.)
            {
                return Err("演化范围、强度或时长无效，单次垂直运动最多 3000 米".into());
            }
            if matches!(op.process, Process::Erosion) {
                erosion_steps += erosion_iterations(op.duration_years);
            }
        }
        if erosion_steps > 24 {
            return Err("单次试算最多 24 个侵蚀步，请缩短时间或分批试算".into());
        }
        if let Some(goal) = &self.strait {
            if !footprint(&goal.corridor)
                || goal.corridor.cells.len() < 2
                || goal.corridor.cells.first() == goal.corridor.cells.last()
                || !(spacing * 3. ..=goal.corridor.radius_m * 2.).contains(&goal.minimum_width_m)
                || !(0.5..=1000.).contains(&goal.minimum_depth_m)
                || goal.repair.as_ref().is_some_and(|r| {
                    !duration(r.duration_years)
                        || !(1. ..=3000.).contains(&r.maximum_subsidence_m)
                        || r.maximum_subsidence_m > r.duration_years as f64 * 0.005
                })
            {
                return Err(
                    "海峡条件无效，最小宽度需至少为 3 倍采样间距，且有两个不同海域入口".into(),
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Passage {
    pub both_ends_at_sea: bool,
    pub connected: bool,
    pub width_estimate_m: Option<f64>,
    pub shallowest_depth_m: Option<f64>,
    pub passed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub can_apply: bool,
    pub changed_cells: usize,
    pub flooded_protected_cells: usize,
    pub lost_protected_water_cells: usize,
    pub land_area_before_m2: f64,
    pub land_area_after_m2: f64,
    pub changed_lake_cells: usize,
    pub before: Option<Passage>,
    pub after: Option<Passage>,
    pub repair_trials: u32,
    pub repair_path: Vec<u32>,
    pub sediment: erosion::SedimentBudget,
}

fn distance<G: GridOps>(grid: &G, area: &Footprint) -> Vec<f64> {
    let xy = |i| {
        let (x, y) = grid.local_center_m(i).expect("finite map coordinates");
        glam::DVec2::new(x, y)
    };
    let points: Vec<_> = area.cells.iter().map(|&i| xy(i as usize)).collect();
    (0..grid.cell_count())
        .map(|i| {
            let p = xy(i);
            let mut distance = p.distance(points[0]);
            for pair in points.windows(2) {
                let along = pair[1] - pair[0];
                let t =
                    ((p - pair[0]).dot(along) / along.length_squared().max(1e-12)).clamp(0., 1.);
                distance = distance.min(p.distance(pair[0] + along * t));
            }
            distance
        })
        .collect()
}

fn smooth(t: f64) -> f64 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

#[derive(Clone, Copy, PartialEq)]
struct Entry(f64, usize);
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .0
            .total_cmp(&self.0)
            .then_with(|| other.1.cmp(&self.1))
    }
}

/// Distances along sampling edges with half-edge distances at a dry/wet interface.
fn distances<G: GridOps>(grid: &G, seeds: impl Iterator<Item = Entry>, limit: f64) -> Vec<f64> {
    let mut distances = vec![f64::INFINITY; grid.cell_count()];
    let mut heap = BinaryHeap::new();
    for Entry(d, i) in seeds {
        if d < distances[i] {
            distances[i] = d;
            heap.push(Entry(d, i));
        }
    }
    while let Some(Entry(d, i)) = heap.pop() {
        if d > distances[i] || d >= limit {
            continue;
        }
        for &j in grid.neighbors(i) {
            let j = j as usize;
            let next = d + grid.distance_m(i, j);
            if next < distances[j] && next <= limit {
                distances[j] = next;
                heap.push(Entry(next, j));
            }
        }
    }
    distances
}

fn sea<G: GridOps>(grid: &G, elevation: &[f32]) -> Vec<bool> {
    let mut connected = vec![false; elevation.len()];
    let mut queue = Vec::new();
    for i in grid
        .boundary_mask()
        .iter_ones()
        .filter(|&i| elevation[i] < 0.)
    {
        connected[i] = true;
        queue.push(i);
    }
    let mut head = 0;
    while let Some(&i) = queue.get(head) {
        head += 1;
        for &j in grid.neighbors(i) {
            let j = j as usize;
            if !connected[j] && elevation[j] < 0. {
                connected[j] = true;
                queue.push(j);
            }
        }
    }
    connected
}

struct Corridor<'a> {
    goal: &'a Strait,
    distance_from_guide: Vec<f64>,
    start: usize,
    end: usize,
}

impl Corridor<'_> {
    fn inside(&self, i: usize) -> bool {
        self.distance_from_guide[i] <= self.goal.corridor.radius_m
    }

    fn passage<G: GridOps>(&self, grid: &G, elevation: &[f32]) -> Passage {
        let wet = sea(grid, elevation);
        let traversable = |i: usize| wet[i] && self.inside(i);
        let both_ends_at_sea = traversable(self.start) && traversable(self.end);
        let connected = both_ends_at_sea && {
            let mut seen = vec![false; elevation.len()];
            let mut queue = vec![self.start];
            seen[self.start] = true;
            while let Some(i) = queue.pop() {
                if i == self.end {
                    break;
                }
                for &j in grid.neighbors(i) {
                    let j = j as usize;
                    if !seen[j] && traversable(j) {
                        seen[j] = true;
                        queue.push(j);
                    }
                }
            }
            seen[self.end]
        };
        let mut report = Passage {
            both_ends_at_sea,
            connected,
            width_estimate_m: None,
            shallowest_depth_m: None,
            passed: false,
        };
        let depth = self.goal.minimum_depth_m;
        let deep = |i: usize| wet[i] && -(elevation[i] as f64) >= depth;
        if !connected || !deep(self.start) || !deep(self.end) {
            return report;
        }
        let clearance = distances(
            grid,
            (0..elevation.len()).filter_map(|i| {
                if !deep(i) || grid.boundary_mask()[i] {
                    return Some(Entry(0., i));
                }
                grid.neighbors(i)
                    .iter()
                    .filter_map(|&j| {
                        (!deep(j as usize)).then(|| grid.distance_m(i, j as usize) * 0.5)
                    })
                    .min_by(f64::total_cmp)
                    .map(|d| Entry(d, i))
            }),
            f64::INFINITY,
        );
        // Graph distances follow a discrete grid. Discount diagonal detours
        // and round the displayed result down at a fraction of the sample spacing.
        let mut widths = vec![-1.; elevation.len()];
        let mut depths = vec![0.; elevation.len()];
        widths[self.start] = clearance[self.start];
        depths[self.start] = -(elevation[self.start] as f64);
        let mut heap = BinaryHeap::from([Entry(-widths[self.start], self.start)]);
        while let Some(Entry(priority, i)) = heap.pop() {
            if i == self.end {
                let width = (widths[i] * 1.6).min(self.goal.corridor.radius_m * 2.);
                report.width_estimate_m = Some(width);
                report.shallowest_depth_m = Some(depths[i]);
                report.passed = width >= self.goal.minimum_width_m;
                break;
            }
            if -priority < widths[i] {
                continue;
            }
            for &j in grid.neighbors(i) {
                let j = j as usize;
                if !deep(j) || !self.inside(j) {
                    continue;
                }
                let width = widths[i].min(clearance[j]);
                if width > widths[j] {
                    widths[j] = width;
                    depths[j] = depths[i].min(-(elevation[j] as f64));
                    heap.push(Entry(-width, j));
                }
            }
        }
        report
    }
}

fn vertical(elevation: &mut [f32], weights: &[f64], displacement_m: f64) {
    for (z, &w) in elevation.iter_mut().zip(weights) {
        if w > 0. {
            *z = (*z as f64 + displacement_m * w) as f32;
        }
    }
}

fn erosion_iterations(years: u32) -> u32 {
    years.div_ceil(2500).clamp(1, 12)
}

pub struct Evolution<'a> {
    protected: Vec<bool>,
    weight: Vec<f64>,
    corridor: Option<Corridor<'a>>,
    spacing: f64,
    pub repair_trials: u32,
    pub repair_path: Vec<u32>,
    pub sediment: erosion::SedimentBudget,
}

impl<'a> Evolution<'a> {
    pub fn new<G: GridOps>(grid: &G, spacing: f64, plan: &'a Plan) -> Result<Self, String> {
        let extent = grid.extent_m().ok_or("请先生成有限范围的地图")?;
        let n = grid.cell_count();
        plan.validate(n, spacing, extent)?;
        let edge = distances(
            grid,
            grid.boundary_mask().iter_ones().map(|i| Entry(0., i)),
            spacing * 4.,
        );
        let mut weight: Vec<_> = edge
            .into_iter()
            .map(|d| smooth((d - spacing) / (3. * spacing)))
            .collect();
        let mut protected = vec![false; n];
        for area in &plan.protected_areas {
            for (i, d) in distance(grid, area).into_iter().enumerate() {
                protected[i] |= d <= area.radius_m;
                weight[i] *= smooth((d - area.radius_m) / (2. * spacing));
            }
        }
        let corridor = plan.strait.as_ref().map(|goal| Corridor {
            goal,
            distance_from_guide: distance(grid, &goal.corridor),
            start: goal.corridor.cells[0] as usize,
            end: *goal.corridor.cells.last().unwrap() as usize,
        });
        Ok(Self {
            protected,
            weight,
            corridor,
            spacing,
            repair_trials: 0,
            repair_path: Vec::new(),
            sediment: Default::default(),
        })
    }

    pub fn apply<G: GridOps, W: GridOps>(
        &mut self,
        grid: &G,
        water_grid: &W,
        fields: &mut WorldFields,
        plan: &Plan,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        for op in &plan.operations {
            if cancel() {
                return Err("试算已取消".into());
            }
            let weights: Vec<_> = distance(grid, &op.area)
                .into_iter()
                .zip(&self.weight)
                .map(|(d, w)| w * smooth(1. - d / op.area.radius_m))
                .collect();
            let amount = op.intensity * op.duration_years as f64 / 1000.;
            match op.process {
                Process::Uplift => vertical(&mut fields.elevation, &weights, amount),
                Process::Subsidence => vertical(&mut fields.elevation, &weights, -amount),
                Process::Erosion => {
                    let defaults = erosion::ErosionParams::default();
                    let iterations = erosion_iterations(op.duration_years);
                    let params = erosion::ErosionParams {
                        iterations,
                        timestep_years: op.duration_years as f64 / iterations as f64,
                        k_river: defaults.k_river * op.intensity,
                        k_diffusion: defaults.k_diffusion * op.intensity,
                        // Incised load stays in the unresolved sediment ledger. A
                        // deposition solver could otherwise reshape a protected coast.
                        k_deposition: 0.,
                        ..defaults
                    };
                    let state = erosion::run_erosion_weighted(
                        water_grid,
                        fields,
                        &params,
                        Some(&weights),
                        cancel,
                        &mut |_, _| {},
                    )?
                    .ok_or("试算已取消")?;
                    self.sediment += state.sediment;
                }
            }
        }
        self.repair(grid, &mut fields.elevation, cancel)
    }

    fn repair<G: GridOps>(
        &mut self,
        grid: &G,
        elevation: &mut [f32],
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let Some(corridor) = &self.corridor else {
            return Ok(());
        };
        let Some(repair) = &corridor.goal.repair else {
            return Ok(());
        };
        if corridor.passage(grid, elevation).passed {
            return Ok(());
        }
        let ocean = sea(grid, elevation);
        if !ocean[corridor.start] || !ocean[corridor.end] {
            return Ok(());
        }
        let depth = corridor.goal.minimum_depth_m;
        let half_width = corridor.goal.minimum_width_m / 1.6 + self.spacing;
        let available = |i: usize| {
            repair.maximum_subsidence_m
                * self.weight[i]
                * smooth(
                    (corridor.goal.corridor.radius_m - corridor.distance_from_guide[i])
                        / (2. * self.spacing),
                )
        };
        let unusable = |i| elevation[i] as f64 + depth + 0.01 > available(i);
        let clearance = distances(
            grid,
            (0..elevation.len())
                .filter(|&i| unusable(i))
                .map(|i| Entry(0., i)),
            half_width * 3.,
        );
        for margin in [1., 1.25, 1.6] {
            if cancel() {
                return Err("试算已取消".into());
            }
            self.repair_trials += 1;
            let radius = half_width * margin;
            let mut cost = vec![f64::INFINITY; elevation.len()];
            let mut previous = vec![usize::MAX; elevation.len()];
            let mut heap = BinaryHeap::from([Entry(0., corridor.start)]);
            cost[corridor.start] = 0.;
            while let Some(Entry(value, i)) = heap.pop() {
                if i == corridor.end {
                    break;
                }
                if value > cost[i] {
                    continue;
                }
                for &j in grid.neighbors(i) {
                    let j = j as usize;
                    if !corridor.inside(j) || unusable(j) || clearance[j] < radius {
                        continue;
                    }
                    let work = (elevation[j] as f64 + depth).max(0.);
                    let next = value + grid.distance_m(i, j) * (1. + work / 10.);
                    if next < cost[j] {
                        cost[j] = next;
                        previous[j] = i;
                        heap.push(Entry(next, j));
                    }
                }
            }
            if previous[corridor.end] == usize::MAX {
                continue;
            }
            let mut path = vec![corridor.end];
            while *path.last().unwrap() != corridor.start {
                path.push(previous[*path.last().unwrap()]);
            }
            path.reverse();
            let spread = distances(
                grid,
                path.iter().map(|&i| Entry(0., i)),
                radius + 3. * self.spacing,
            );
            let weights: Vec<_> = spread
                .into_iter()
                .enumerate()
                .map(|(i, d)| {
                    let needed = (elevation[i] as f64 + depth + 0.05).max(0.);
                    let amount = needed.min(available(i));
                    let taper = smooth(1. - (d - radius) / (3. * self.spacing));
                    amount * taper / repair.maximum_subsidence_m
                })
                .collect();
            let mut trial = elevation.to_vec();
            vertical(&mut trial, &weights, -repair.maximum_subsidence_m);
            if corridor.passage(grid, &trial).passed {
                elevation.copy_from_slice(&trial);
                self.repair_path = path.into_iter().map(|i| i as u32).collect();
                break;
            }
        }
        Ok(())
    }

    pub fn report<G: GridOps>(
        &self,
        grid: &G,
        old_elevation: &[f32],
        old_lakes: &[f32],
        fields: &WorldFields,
    ) -> Report {
        let before = self
            .corridor
            .as_ref()
            .map(|c| c.passage(grid, old_elevation));
        let after = self
            .corridor
            .as_ref()
            .map(|c| c.passage(grid, &fields.elevation));
        let sea_connection = self
            .protected
            .iter()
            .any(|&p| p)
            .then(|| (sea(grid, old_elevation), sea(grid, &fields.elevation)));
        let mut report = Report {
            can_apply: false,
            changed_cells: 0,
            flooded_protected_cells: 0,
            lost_protected_water_cells: 0,
            land_area_before_m2: 0.,
            land_area_after_m2: 0.,
            changed_lake_cells: 0,
            before,
            after,
            repair_trials: self.repair_trials,
            repair_path: self.repair_path.clone(),
            sediment: self.sediment,
        };
        for i in 0..old_elevation.len() {
            let was_land = old_elevation[i] > 0.;
            let is_land = fields.elevation[i] > 0.;
            let was_wet = !was_land || old_lakes[i] > 0.05;
            let is_wet = !is_land || fields.lake_depth[i] > 0.05;
            report.changed_cells += usize::from(old_elevation[i] != fields.elevation[i]);
            report.changed_lake_cells +=
                usize::from((old_lakes[i] - fields.lake_depth[i]).abs() > 0.05);
            if was_land {
                report.land_area_before_m2 += grid.area_m2(i);
            }
            if is_land {
                report.land_area_after_m2 += grid.area_m2(i);
            }
            if self.protected[i] {
                report.flooded_protected_cells += usize::from(!was_wet && is_wet);
                let lost_sea_access = sea_connection
                    .as_ref()
                    .is_some_and(|(before, after)| before[i] && !after[i]);
                report.lost_protected_water_cells +=
                    usize::from((was_wet && !is_wet) || lost_sea_access);
            }
        }
        report.can_apply = report.changed_cells > 0
            && report.flooded_protected_cells == 0
            && report.lost_protected_water_cells == 0
            && report.after.as_ref().is_none_or(|p| p.passed);
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tf_grid::GridDescriptor;

    #[test]
    fn manual_forcing_is_repeatable_and_respects_locked_terrain() {
        let grid = GridDescriptor::patch(18., 179.9, 48_000., 48, 6_371_000.).build();
        let n = grid.cell_count();
        let at = |x, y| grid.locate_local(x, y).unwrap() as u32;
        let original: Vec<_> = (0..n)
            .map(|i| {
                let (x, y) = grid.local_center_m(i).unwrap();
                300. - x as f32 * 0.003 + (y as f32 * 0.0003).sin() * 8.
            })
            .collect();
        let mountain = at(-6500., 0.);
        let plain = at(6500., 0.);
        let locked = at(0., 0.);
        let area = |cell, radius_m| Footprint {
            cells: vec![cell],
            radius_m,
        };
        let plan = Plan {
            operations: vec![
                Operation {
                    process: Process::Uplift,
                    area: area(mountain, 7000.),
                    duration_years: 60_000,
                    intensity: 2.,
                },
                Operation {
                    process: Process::Subsidence,
                    area: area(plain, 6200.),
                    duration_years: 40_000,
                    intensity: 2.,
                },
                Operation {
                    process: Process::Erosion,
                    area: area(at(-5500., 1500.), 8200.),
                    duration_years: 2500,
                    intensity: 0.25,
                },
            ],
            protected_areas: vec![area(locked, 2600.)],
            ..Default::default()
        };
        let run = || {
            let mut fields = WorldFields::with_capacity(n);
            fields.elevation.clone_from(&original);
            fields.precipitation.fill(1500.);
            fields.potential_evaporation.fill(300.);
            let mut evolution = Evolution::new(&grid, 1000., &plan).unwrap();
            evolution
                .apply(&grid, &grid, &mut fields, &plan, &|| false)
                .unwrap();
            assert!(evolution.sediment.eroded_m3 > 0.);
            assert!(evolution.sediment.residual_m3.abs() < evolution.sediment.eroded_m3 * 1e-12);
            fields.elevation
        };
        let result = run();
        assert_eq!(result, run());
        assert!(result[mountain as usize] > original[mountain as usize]);
        assert!(result[plain as usize] < original[plain as usize]);
        let protected_distance = distance(&grid, &plan.protected_areas[0]);
        for i in 0..n {
            if grid.boundary_mask()[i] || protected_distance[i] <= 2600. {
                assert_eq!(result[i], original[i]);
            }
        }
    }
}
