//! Explicit finite-domain forcing and parent river geometry. No hidden random
//! state: contracts are serializable and monthly views use the saved inputs.
use crate::WorldFields;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tf_core::geo::UnitVec3;
use tf_grid::{GridOps, MapGrid};

pub const YEAR_SECONDS: f64 = 365.2425 * 86400.;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inlet {
    pub cell: u32,
    pub upstream_area_m2: f64,
    /// [annual, January, ..., December], monthly entries are annualized rates.
    pub water_m3_s: [f64; 13],
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outlet {
    pub cell: u32,
    pub surface_m: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryForcing {
    pub version: u32,
    pub cells: usize,
    pub monthly_available: bool,
    pub inlets: Vec<Inlet>,
    pub outlets: Vec<Outlet>,
    /// Parent legacy models save a sediment ledger, not per-edge load.
    pub sediment_available: bool,
}
impl BoundaryForcing {
    pub fn empty(n: usize) -> Self {
        Self {
            version: 1,
            cells: n,
            monthly_available: false,
            inlets: vec![],
            outlets: vec![],
            sediment_available: false,
        }
    }
    pub fn validate(&self, n: usize) -> Result<(), String> {
        if self.version != 1
            || self.cells != n
            || n == 0
            || self.inlets.len() > n * 2
            || self.outlets.len() > n
        {
            return Err("局部边界资料尺寸无效".into());
        }
        if self.inlets.iter().any(|p| {
            p.cell as usize >= n
                || !p.upstream_area_m2.is_finite()
                || !(0. ..=1e20).contains(&p.upstream_area_m2)
                || p.water_m3_s
                    .iter()
                    .any(|q| !q.is_finite() || !(0. ..=1e18).contains(q))
        }) || self
            .outlets
            .iter()
            .any(|p| p.cell as usize >= n || !p.surface_m.is_finite() || p.surface_m.abs() > 1e7)
        {
            return Err("局部边界资料数值无效".into());
        }
        Ok(())
    }
    pub fn apply(&self, fields: &mut WorldFields, month: u8) -> Result<(), String> {
        self.validate(fields.elevation.len())?;
        if month > 12 || (month > 0 && !self.monthly_available && !self.inlets.is_empty()) {
            return Err("缺少局部逐月边界来水".into());
        }
        fields.external_water_m3_yr = vec![0.; self.cells];
        fields.external_area_m2 = vec![0.; self.cells];
        fields.boundary_stages = self.outlets.iter().map(|p| (p.cell, p.surface_m)).collect();
        for p in &self.inlets {
            fields.external_water_m3_yr[p.cell as usize] +=
                p.water_m3_s[month as usize] * YEAR_SECONDS;
            fields.external_area_m2[p.cell as usize] += p.upstream_area_m2;
        }
        Ok(())
    }
}

/// Only inherited outward-facing terrain and actual river exits are open.
/// Physical geometry/areas stay unchanged; this wrapper is used for water only.
pub struct BoundaryGrid<'a> {
    pub grid: &'a MapGrid,
    open: bitvec::prelude::BitVec,
}
impl<'a> BoundaryGrid<'a> {
    pub fn new(grid: &'a MapGrid, b: &BoundaryForcing) -> Result<Self, String> {
        b.validate(grid.cell_count())?;
        let mut open = bitvec::bitvec![0;grid.cell_count()];
        for p in &b.outlets {
            if !grid.boundary_mask()[p.cell as usize] {
                return Err("出水口不在地图边界".into());
            }
            open.set(p.cell as usize, true);
        }
        Ok(Self { grid, open })
    }
}
impl GridOps for BoundaryGrid<'_> {
    fn field_is_planar(&self) -> bool {
        self.grid.field_is_planar()
    }
    fn field_position(&self, i: usize) -> glam::DVec3 {
        self.grid.field_position(i)
    }
    fn extent_m(&self) -> Option<(f64, f64)> {
        self.grid.extent_m()
    }
    fn local_center_m(&self, i: usize) -> Option<(f64, f64)> {
        self.grid.local_center_m(i)
    }
    fn locate_local(&self, x: f64, y: f64) -> Option<usize> {
        self.grid.locate_local(x, y)
    }
    fn locate(&self, d: UnitVec3) -> Option<usize> {
        self.grid.locate(d)
    }
    fn radius_m(&self) -> f64 {
        self.grid.radius_m()
    }
    fn cell_count(&self) -> usize {
        self.grid.cell_count()
    }
    fn neighbors(&self, i: usize) -> &[u32] {
        self.grid.neighbors(i)
    }
    fn distance_m(&self, a: usize, b: usize) -> f64 {
        self.grid.distance_m(a, b)
    }
    fn area_m2(&self, i: usize) -> f64 {
        self.grid.area_m2(i)
    }
    fn center_dir(&self, i: usize) -> UnitVec3 {
        self.grid.center_dir(i)
    }
    fn latitude_deg(&self, i: usize) -> f64 {
        self.grid.latitude_deg(i)
    }
    fn boundary_mask(&self) -> &bitvec::prelude::BitVec {
        &self.open
    }
}

pub fn project(grid: &MapGrid, dir: UnitVec3) -> Option<(f64, f64)> {
    match grid {
        MapGrid::Plane(g) => Some(g.project(dir)),
        MapGrid::Patch(g) => {
            let origin = g.origin().as_vec();
            if origin.dot(dir.as_vec()) <= 0. {
                return None;
            }
            let (x, y) = g.project(dir);
            Some((x * g.radius_m(), y * g.radius_m()))
        }
        _ => None,
    }
}
pub fn direction(grid: &MapGrid, x: f64, y: f64) -> Option<UnitVec3> {
    match grid {
        MapGrid::Plane(g) => Some(g.direction_at(x, y)),
        MapGrid::Patch(g) => Some(g.direction_at_local(x, y)),
        _ => None,
    }
}
pub fn order(receivers: &[u32]) -> Result<Vec<u32>, String> {
    let n = receivers.len();
    let mut incoming = vec![0u32; n];
    for (i, &j) in receivers.iter().enumerate() {
        if j != u32::MAX {
            if j as usize >= n || i == j as usize {
                return Err("父图流向无效".into());
            }
            incoming[j as usize] += 1;
        }
    }
    let mut queue: VecDeque<_> = (0..n).filter(|&i| incoming[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i as u32);
        let j = receivers[i];
        if j != u32::MAX {
            incoming[j as usize] -= 1;
            if incoming[j as usize] == 0 {
                queue.push_back(j as usize);
            }
        }
    }
    if order.len() != n {
        return Err("父图水系存在循环".into());
    }
    Ok(order)
}
/// Liang–Barsky clipping in the child's actual metric frame.
fn clip(a: (f64, f64), b: (f64, f64), width: f64, height: f64) -> Option<(f64, f64)> {
    let mut lo = 0_f64;
    let mut hi = 1_f64;
    for (p, q) in [
        (-(b.0 - a.0), a.0 + width / 2.),
        (b.0 - a.0, width / 2. - a.0),
        (-(b.1 - a.1), a.1 + height / 2.),
        (b.1 - a.1, height / 2. - a.1),
    ] {
        if p.abs() < 1e-12 {
            if q < 0. {
                return None;
            }
            continue;
        }
        let t = q / p;
        if p < 0. {
            lo = lo.max(t);
        } else {
            hi = hi.min(t);
        }
        if lo > hi {
            return None;
        }
    }
    Some((lo, hi))
}
fn boundary_cell(grid: &MapGrid, x: f64, y: f64) -> usize {
    let (w, h) = grid.extent_m().unwrap();
    let d = [
        (w / 2. - x).abs(),
        (w / 2. + x).abs(),
        (h / 2. - y).abs(),
        (h / 2. + y).abs(),
    ];
    let side = (0..4).min_by(|&a, &b| d[a].total_cmp(&d[b])).unwrap();
    let (x, y) = match side {
        0 => (w / 2., y),
        1 => (-w / 2., y),
        2 => (x, h / 2.),
        _ => (x, -h / 2.),
    };
    let i = grid
        .locate_local(x.clamp(-w / 2., w / 2.), y.clamp(-h / 2., h / 2.))
        .unwrap();
    if grid.boundary_mask()[i] {
        return i;
    }
    grid.boundary_mask()
        .iter_ones()
        .min_by(|&a, &b| {
            let p = grid.local_center_m(a).unwrap();
            let q = grid.local_center_m(b).unwrap();
            (p.0 - x)
                .hypot(p.1 - y)
                .total_cmp(&(q.0 - x).hypot(q.1 - y))
        })
        .unwrap()
}
#[derive(Debug)]
pub struct RiverGuide {
    pub cells: Vec<usize>,
    pub heights: Vec<f32>,
    pub discharge_m3_s: f64,
}

fn edge_end(parent: &MapGrid, child: &MapGrid, i: usize, j: usize) -> Option<(f64, f64)> {
    if let (MapGrid::Plane(p), MapGrid::Plane(c)) = (parent, child) {
        let a = c.project(parent.center_dir(i));
        let u = p.field_position(i);
        let v = p.field_position(j);
        let scale_x = c.radius * c.reference_lat.to_radians().cos().max(0.05)
            / (p.radius * p.reference_lat.to_radians().cos().max(0.05));
        return Some((
            a.0 + (v.x - u.x) * scale_x,
            a.1 + (v.y - u.y) * c.radius / p.radius,
        ));
    }
    project(child, parent.center_dir(j))
}

/// Integrate only external source fractions on the parent's saved DAG. Child
/// area is removed at source, not subtracted arbitrarily from an inlet total.
pub fn inherit(
    parent: &MapGrid,
    child: &MapGrid,
    f: &WorldFields,
    receivers: &[u32],
    parent_forcing: Option<&BoundaryForcing>,
    ice: Option<&[f32]>,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<(BoundaryForcing, Vec<RiverGuide>), String> {
    let n = parent.cell_count();
    if [
        f.elevation.len(),
        f.temperature.len(),
        f.precipitation.len(),
        receivers.len(),
    ]
    .iter()
    .any(|&s| s != n)
    {
        return Err("父图水文尺寸无效".into());
    }
    let order = order(receivers)?;
    let (width, height) = child.extent_m().ok_or("细化图必须为有限域")?;
    if child.boundary_mask().not_any() {
        return Err("细化图没有边界".into());
    }
    let mut covered = vec![0.; n];
    for i in 0..child.cell_count() {
        if i % 2048 == 0 && cancel() {
            return Err("细化已取消".into());
        }
        let j = parent
            .cell_at(child.center_dir(i))
            .ok_or("细化范围超出父图")?;
        covered[j] += child.area_m2(i);
    }
    let outside: Vec<_> = (0..n)
        .map(|i| {
            if let Some((x, y)) = project(child, parent.center_dir(i)) {
                let radius = parent
                    .neighbors(i)
                    .iter()
                    .map(|&j| parent.distance_m(i, j as usize))
                    .fold(0., f64::max)
                    * 1.05;
                if width / 2. - x.abs() > radius && height / 2. - y.abs() > radius {
                    return 0.;
                }
            }
            (1. - covered[i] / parent.area_m2(i)).clamp(0., 1.)
        })
        .collect();
    let mut q =
        crate::cryosphere::runoff_climatology(parent, f, ice, cancel).ok_or("细化已取消")?;
    let mut area = vec![0_f64; n];
    // Follow the outside share of saved lake losses without charging the
    // child's local rainfall for that evaporation a second time.
    let mut full_annual = vec![0_f64; n];
    let mut supplied = std::collections::BTreeMap::<usize, ([f64; 13], f64)>::new();
    if let Some(b) = parent_forcing {
        b.validate(n)?;
        for p in &b.inlets {
            let v = supplied.entry(p.cell as usize).or_insert(([0.; 13], 0.));
            for m in 0..13 {
                v.0[m] += p.water_m3_s[m];
            }
            v.1 += p.upstream_area_m2;
        }
    }
    let mut inputs = std::collections::BTreeMap::<usize, ([f64; 13], f64)>::new();
    for i in 0..n {
        let (supplied_water, supplied_area) = supplied.get(&i).copied().unwrap_or(([0.; 13], 0.));
        full_annual[i] = q[i][0] + supplied_water[0];
        if i % 2048 == 0 && cancel() {
            return Err("细化已取消".into());
        }
        for m in 0..13 {
            q[i][m] = q[i][m] * outside[i] + supplied_water[m];
        }
        area[i] = parent.area_m2(i) * outside[i] + supplied_area;
        if outside[i] > 0. && child.cell_at(parent.center_dir(i)).is_some() {
            if let Some(a) = project(child, parent.center_dir(i)) {
                let cell = boundary_cell(child, a.0, a.1);
                let entry = inputs.entry(cell).or_insert(([0.; 13], 0.));
                for m in 0..13 {
                    entry.0[m] += q[i][m];
                }
                entry.1 += area[i];
                q[i] = [0.; 13];
                area[i] = 0.;
            }
        }
    }
    // Consume an external source at its FIRST entrance. Otherwise a stream
    // leaving and re-entering the crop (or a partially covered source cell)
    // would inject the same water twice. Child rainfall is routed only locally.
    for (step, &i) in order.iter().enumerate() {
        if step % 2048 == 0 && cancel() {
            return Err("细化已取消".into());
        }
        let i = i as usize;
        let j = receivers[i];
        if j == u32::MAX {
            continue;
        }
        let j = j as usize;
        if f.lake_runoff_loss_fraction.get(i).is_some_and(|&v| v > 0.)
            && f.liquid_discharge.len() == n
            && full_annual[i] > 0.
        {
            let retained = (f.liquid_discharge[i] as f64 / full_annual[i]).clamp(0., 1.);
            for m in 0..13 {
                q[i][m] *= retained;
            }
            full_annual[i] *= retained;
        }
        if let (Some(a), Some(b)) = (
            project(child, parent.center_dir(i)),
            edge_end(parent, child, i, j),
        ) {
            if child.cell_at(parent.center_dir(i)).is_none() {
                if let Some((lo, hi)) = clip(a, b, width, height) {
                    if hi - lo > 1e-10 {
                        let cell =
                            boundary_cell(child, a.0 + (b.0 - a.0) * lo, a.1 + (b.1 - a.1) * lo);
                        let entry = inputs.entry(cell).or_insert(([0.; 13], 0.));
                        for m in 0..13 {
                            entry.0[m] += q[i][m];
                        }
                        entry.1 += area[i];
                        q[i] = [0.; 13];
                        area[i] = 0.;
                    }
                }
            }
        }
        for m in 0..13 {
            q[j][m] += q[i][m];
        }
        area[j] += area[i];
        full_annual[j] += full_annual[i];
    }
    drop(q);
    drop(area);
    let mut result = BoundaryForcing::empty(child.cell_count());
    result.monthly_available =
        f.climate.is_some() && parent_forcing.is_none_or(|b| b.monthly_available);
    let mut exits = std::collections::BTreeMap::<usize, f64>::new();
    let mut guides = Vec::new();
    let mut guide_points = 0usize;
    for i in 0..n {
        if i % 2048 == 0 && cancel() {
            return Err("细化已取消".into());
        }
        let Some(a) = project(child, parent.center_dir(i)) else {
            continue;
        };
        let j = receivers[i];
        if j == u32::MAX {
            continue;
        }
        let j = j as usize;
        let Some(b) = edge_end(parent, child, i, j) else {
            continue;
        };
        let Some((lo, hi)) = clip(a, b, width, height) else {
            continue;
        };
        if hi - lo < 1e-10 {
            continue;
        }
        let point = |t: f64| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        if hi < 1. - 1e-10 || child.cell_at(parent.center_dir(j)).is_none() {
            let p = point(hi);
            let cell = boundary_cell(child, p.0, p.1);
            let surface = |i: usize| {
                f.elevation[i] as f64 + f.lake_depth.get(i).copied().unwrap_or(0.) as f64
            };
            let z = surface(i) * (1. - hi) + surface(j) * hi;
            exits
                .entry(cell)
                .and_modify(|v| *v = v.min(z.max(0.)))
                .or_insert(z.max(0.));
        }
        let discharge = f.liquid_discharge.get(i).copied().unwrap_or(0.) as f64;
        if (f.flow_accumulation.get(i).copied().unwrap_or(0.) as f64) < parent.area_m2(i) * 12.
            || discharge <= 0.01
            || guide_points >= child.cell_count() * 8
        {
            continue;
        }
        let spacing = (child.area_m2(0) * 2. / 3_f64.sqrt()).sqrt();
        let count = (((b.0 - a.0).hypot(b.1 - a.1) * (hi - lo) / (spacing * 0.3)).ceil() as usize)
            .clamp(1, 8192);
        let mut guide = RiverGuide {
            cells: vec![],
            heights: vec![],
            discharge_m3_s: discharge,
        };
        for k in 0..=count {
            let t = lo + (hi - lo) * k as f64 / count as f64;
            let p = point(t);
            if let Some(cell) = child.locate_local(p.0, p.1) {
                if guide.cells.last() != Some(&cell) {
                    guide.cells.push(cell);
                    guide.heights.push(
                        (f.elevation[i] as f64 * (1. - t) + f.elevation[j] as f64 * t) as f32,
                    );
                }
            }
        }
        if guide.cells.len() > 1 {
            guide_points += guide.cells.len();
            guides.push(guide);
        }
    }
    result.inlets = inputs
        .into_iter()
        .map(|(cell, (water, area))| Inlet {
            cell: cell as u32,
            upstream_area_m2: area,
            water_m3_s: water,
        })
        .collect();
    // Fine drainage may leave between coarse rivers if the parent slope points out.
    for i in child.boundary_mask().iter_ones() {
        if cancel() {
            return Err("细化已取消".into());
        }
        let (x, y) = child.local_center_m(i).unwrap();
        let spacing = (child.area_m2(i) * 2. / 3_f64.sqrt()).sqrt();
        let (dx, dy) = if width / 2. - x.abs() < height / 2. - y.abs() {
            (x.signum() * spacing, 0.)
        } else {
            (0., y.signum() * spacing)
        };
        if let (Some(d0), Some(d1)) = (direction(child, x, y), direction(child, x + dx, y + dy)) {
            let sample = |d| crate::downscale::sample_at(parent, &f.elevation, d);
            if let (Some(z0), Some(z1)) = (sample(d0), sample(d1)) {
                if z1 < z0 - 1e-4 {
                    exits.entry(i).or_insert(z1.max(0.) as f64);
                }
            }
        }
    }
    // A partial coarse cell may contribute outside water straight to an exit.
    // Keeping both ports is conservative; deleting that exit makes a false lake.
    result.outlets = exits
        .into_iter()
        .map(|(cell, z)| Outlet {
            cell: cell as u32,
            surface_m: z,
        })
        .collect();
    result.validate(child.cell_count())?;
    Ok((result, guides))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn slope(cols: u32, rows: u32) -> (MapGrid, WorldFields) {
        let g = MapGrid::Plane(tf_grid::FlatGrid::new(
            cols,
            rows,
            cols as f64 * 1000.,
            25.,
            0.,
            6371000.,
        ));
        let mut f = WorldFields::with_capacity(g.cell_count());
        for i in 0..g.cell_count() {
            let (x, y) = g.local_center_m(i).unwrap();
            f.elevation[i] = (1000. - x * 0.01 + y * 0.00001) as f32;
            f.temperature[i] = 15.;
            f.precipitation[i] = 1000.;
        }
        (g, f)
    }
    #[test]
    fn finite_parent_sampling_preserves_an_affine_surface_across_cell_boundaries() {
        let (parent, f) = slope(32, 24);
        let child = MapGrid::Plane(tf_grid::FlatGrid::new(80, 60, 16000., 25., 0., 6371000.));
        let values =
            crate::downscale::interpolate_from_grid(&parent, &child, &f.elevation).unwrap();
        for (i, &z) in values.iter().enumerate() {
            let (x, y) = child.local_center_m(i).unwrap();
            assert!(
                (z as f64 - (1000. - x * 0.01 + y * 0.00001)).abs() < 0.002,
                "{i}: {z}"
            );
        }
    }
    #[test]
    fn inherited_external_runoff_excludes_the_childs_own_rainfall() {
        let (parent, mut f) = slope(32, 24);
        let water = crate::erosion::refresh_hydrology(&parent, &mut f);
        f.liquid_discharge = crate::cryosphere::annual_discharge(&parent, &f, &water);
        let (child, mut local) = slope(16, 12);
        let (b, _) = inherit(&parent, &child, &f, &water.flow_dir, None, None, &|| false).unwrap();
        assert!(!b.inlets.is_empty() && !b.outlets.is_empty());
        b.apply(&mut local, 0).unwrap();
        // Parent's outermost column is already an open terminal; only the
        // seven upstream interior columns actually enter the clipped rivers.
        let expected_external_area = 7. * 12. * parent.area_m2(0);
        let actual = local.external_area_m2.iter().sum::<f64>();
        assert!(
            (actual - expected_external_area).abs() < expected_external_area * 1e-5,
            "{actual} vs {expected_external_area}"
        );
        let grid = BoundaryGrid::new(&child, &b).unwrap();
        let result = crate::erosion::refresh_hydrology(&grid, &mut local);
        let q = crate::cryosphere::annual_discharge(&grid, &local, &result);
        let out = result
            .flow_dir
            .iter()
            .enumerate()
            .filter(|(_, r)| **r == u32::MAX)
            .map(|(i, _)| q[i] as f64 * YEAR_SECONDS)
            .sum::<f64>();
        let local_rain = (0..child.cell_count())
            .map(|i| child.area_m2(i))
            .sum::<f64>();
        assert!(
            (out - local_rain - actual).abs() < out * 1e-5,
            "out {out}, local {local_rain}, external {actual}"
        );
        assert!(result.lake_budget.residual_m3_yr.abs() < out * 1e-9);
    }
    #[test]
    fn a_dry_region_can_receive_a_real_river_without_inventing_rain() {
        let (grid, mut f) = slope(16, 12);
        f.precipitation.fill(0.);
        let i = grid
            .boundary_mask()
            .iter_ones()
            .find(|&i| grid.local_center_m(i).unwrap().0 < -7000.)
            .unwrap();
        let mut b = BoundaryForcing::empty(grid.cell_count());
        b.monthly_available = true;
        b.inlets.push(Inlet {
            cell: i as u32,
            upstream_area_m2: 1e9,
            water_m3_s: [2.; 13],
        });
        b.apply(&mut f, 0).unwrap();
        let water = crate::erosion::refresh_hydrology(&grid, &mut f);
        assert_eq!(water.lake_budget.precipitation_m3_yr, 0.);
        assert!((water.lake_budget.terminal_runoff_m3_yr - 2. * YEAR_SECONDS).abs() < 1e-5);
        assert!(water.lake_budget.residual_m3_yr.abs() < 1e-5);
        let mut monthly = b.clone();
        monthly.inlets[0].water_m3_s[1] = 4.;
        let restored: BoundaryForcing =
            serde_json::from_str(&serde_json::to_string(&monthly).unwrap()).unwrap();
        restored.apply(&mut f, 1).unwrap();
        assert_eq!(f.external_water_m3_yr[i], 4. * YEAR_SECONDS);
        assert!(order(&[1, 0]).is_err());
    }
    #[test]
    fn a_river_reentering_an_offset_crop_is_not_injected_twice() {
        let (parent, mut f) = slope(32, 24);
        f.precipitation.fill(0.);
        let mut receivers = vec![u32::MAX; parent.cell_count()];
        for c in 1..30 {
            receivers[8 * 32 + c] = (8 * 32 + c + 1) as u32;
        }
        receivers[8 * 32 + 30] = 9 * 32 + 30;
        receivers[9 * 32 + 30] = 10 * 32 + 30;
        for c in 2..=30 {
            receivers[10 * 32 + c] = (10 * 32 + c - 1) as u32;
        }
        let mut forcing = BoundaryForcing::empty(parent.cell_count());
        forcing.monthly_available = true;
        forcing.inlets.push(Inlet {
            cell: 8 * 32 + 1,
            upstream_area_m2: 1e9,
            water_m3_s: [2.; 13],
        });
        let child = MapGrid::Plane(tf_grid::FlatGrid::new(
            47, 39, 15123., 25.0003, 0.001, 6371000.,
        ));
        let (b, _) = inherit(
            &parent,
            &child,
            &f,
            &receivers,
            Some(&forcing),
            None,
            &|| false,
        )
        .unwrap();
        for m in 0..13 {
            let input = b.inlets.iter().map(|p| p.water_m3_s[m]).sum::<f64>();
            assert!((input - 2.).abs() < 1e-12, "month {m}: {input}");
        }
    }
    #[test]
    fn offset_sources_remain_conservative_and_annual_matches_months() {
        let (parent, mut f) = slope(32, 24);
        let water = crate::erosion::refresh_hydrology(&parent, &mut f);
        let child = MapGrid::Plane(tf_grid::FlatGrid::new(
            47, 39, 15123., 25.0003, 0.001, 6371000.,
        ));
        let (b, _) = inherit(&parent, &child, &f, &water.flow_dir, None, None, &|| false).unwrap();
        for p in &b.inlets {
            assert!((p.water_m3_s[1..].iter().sum::<f64>() / 12. - p.water_m3_s[0]).abs() < 1e-10);
        }
        let total = b
            .inlets
            .iter()
            .map(|p| p.water_m3_s[0] * YEAR_SECONDS)
            .sum::<f64>();
        let parent_input = (0..parent.cell_count())
            .map(|i| parent.area_m2(i))
            .sum::<f64>();
        assert!(total > 0. && total < parent_input);
        assert!(inherit(&parent, &child, &f, &water.flow_dir, None, None, &|| true).is_err());
    }
    #[test]
    fn planar_edges_are_transformed_into_the_childs_metric_frame() {
        let p = MapGrid::Plane(tf_grid::FlatGrid::with_reference_lat(
            32, 24, 32000., 25., 179.99, 6371000., 25.,
        ));
        let c = MapGrid::Plane(tf_grid::FlatGrid::with_reference_lat(
            32, 24, 16000., 25., -179.99, 6371000., 55.,
        ));
        let expected = project(&c, p.center_dir(32 * 12 + 15)).unwrap();
        let actual = edge_end(&p, &c, 32 * 12 + 14, 32 * 12 + 15).unwrap();
        assert!((actual.0 - expected.0).abs() < 1e-5 && (actual.1 - expected.1).abs() < 1e-5);
    }
    #[test]
    fn prescribed_backwater_does_not_raise_land_or_invent_rain() {
        let (grid, mut f) = slope(16, 12);
        f.precipitation.fill(0.);
        f.potential_evaporation.fill(1000.);
        let ground = f.elevation.clone();
        let mut b = BoundaryForcing::empty(grid.cell_count());
        let i = grid
            .boundary_mask()
            .iter_ones()
            .find(|&i| grid.local_center_m(i).unwrap().0 > 7000.)
            .unwrap();
        b.outlets.push(Outlet {
            cell: i as u32,
            surface_m: 1100.,
        });
        b.apply(&mut f, 0).unwrap();
        let g = BoundaryGrid::new(&grid, &b).unwrap();
        let water = crate::erosion::refresh_hydrology(&g, &mut f);
        assert_eq!(ground, f.elevation);
        assert_eq!(water.lake_budget.precipitation_m3_yr, 0.);
        assert!(f.lake_depth.iter().any(|&h| h > 10.));
        assert!(water.lake_budget.prescribed_head_supply_m3_yr > 0.);
        assert!(water.lake_budget.residual_m3_yr.abs() < 1e-6);
        assert!(crate::cryosphere::annual_discharge(&g, &f, &water)
            .iter()
            .all(|&q| q == 0.));
    }
    #[test]
    fn successive_parent_lake_losses_are_applied_once_to_the_outside_share() {
        let (parent, mut f) = slope(32, 24);
        f.precipitation.fill(0.);
        f.liquid_discharge = vec![100.; parent.cell_count()];
        f.lake_runoff_loss_fraction = vec![0.; parent.cell_count()];
        let mut receivers = vec![u32::MAX; parent.cell_count()];
        for c in 0..31 {
            receivers[12 * 32 + c] = (12 * 32 + c + 1) as u32;
        }
        f.lake_runoff_loss_fraction[12 * 32 + 4] = 1.;
        f.liquid_discharge[12 * 32 + 4] = 80.;
        f.lake_runoff_loss_fraction[12 * 32 + 6] = 1.;
        f.liquid_discharge[12 * 32 + 6] = 64.;
        let mut forcing = BoundaryForcing::empty(parent.cell_count());
        forcing.monthly_available = true;
        forcing.inlets.push(Inlet {
            cell: 12 * 32,
            upstream_area_m2: 1e9,
            water_m3_s: [100.; 13],
        });
        let (child, _) = slope(16, 12);
        let (b, _) = inherit(
            &parent,
            &child,
            &f,
            &receivers,
            Some(&forcing),
            None,
            &|| false,
        )
        .unwrap();
        for m in 0..13 {
            let q = b.inlets.iter().map(|p| p.water_m3_s[m]).sum::<f64>();
            assert!((q - 64.).abs() < 1e-10, "{m}: {q}");
        }
    }
    #[test]
    fn a_high_boundary_head_cannot_drown_a_crop_with_a_lower_exit() {
        let (grid, mut f) = slope(16, 12);
        f.precipitation.fill(0.);
        f.potential_evaporation.fill(1000.);
        let mut b = BoundaryForcing::empty(grid.cell_count());
        let west = grid
            .boundary_mask()
            .iter_ones()
            .find(|&i| grid.local_center_m(i).unwrap().0 < -7000.)
            .unwrap();
        let east = grid
            .boundary_mask()
            .iter_ones()
            .find(|&i| grid.local_center_m(i).unwrap().0 > 7000.)
            .unwrap();
        b.outlets = vec![
            Outlet {
                cell: west as u32,
                surface_m: 1300.,
            },
            Outlet {
                cell: east as u32,
                surface_m: 900.,
            },
        ];
        b.apply(&mut f, 0).unwrap();
        let g = BoundaryGrid::new(&grid, &b).unwrap();
        let water = crate::erosion::refresh_hydrology(&g, &mut f);
        assert!(f.lake_depth.iter().all(|&h| h < 0.001));
        assert!(water.lake_budget.prescribed_head_supply_m3_yr < 1e-6);
    }
}
