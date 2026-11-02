//! Kinematic, damage-guided plate topology. Historical identities never reuse
//! an ID; dense computational slots may be reused only after carrier transfer.
//! Driving velocities are a procedural mantle-flow proxy, not a force solution.
use crate::{plates::TectonicState, WorldFields};
use glam::{DMat3, DVec3};
use serde::{Deserialize, Serialize};
use std::collections::{BinaryHeap, VecDeque};
use tf_core::{geo::UnitVec3, params::GenerationParams};
use tf_grid::{GridOps, MapGrid};

pub const MAX_PLATES: usize = 48;
#[derive(Debug, Clone)]
pub struct Marker {
    position: DVec3,
    slot: usize,
    crust: f32,
    strength: f32,
    rock_age: f64,
    damage: f32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub uid: u32,
    pub born_myr: f64,
    pub ended_myr: Option<f64>,
    pub parent_uid: Option<u32>,
    pub absorbed_by: Option<u32>,
    pub status: String,
    pub area_m2: f64,
    pub maximum_area_m2: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub time_myr: f64,
    pub kind: String,
    pub uid: u32,
    pub related_uid: Option<u32>,
    pub area_m2: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AreaSample {
    pub time_myr: f64,
    pub spatial_resolution_m: f64,
    pub areas: Vec<(u32, f64)>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Lifecycle {
    pub identities: Vec<Identity>,
    pub slot_uids: Vec<u32>,
    pub events: Vec<Event>,
    pub area_history: Vec<AreaSample>,
    #[serde(skip)]
    pub active: Vec<bool>,
    #[serde(skip)]
    pub locked_myr: Vec<f64>,
    #[serde(skip)]
    pub fracture_myr: Vec<f64>,
    #[serde(skip)]
    pub last_split_myr: Vec<f64>,
    #[serde(skip)]
    pub markers: Vec<Marker>,
    #[serde(skip)]
    pub marker_spacing_m: f64,
    #[serde(skip)]
    pub needs_carrier_transfer: bool,
    pub subducted_surface_samples: u64,
    pub exported_surface_samples: u64,
    pub ridge_surface_samples: u64,
    pub capacity_limited_events: u32,
}
impl Lifecycle {
    /// Validate persisted public records without reconstructing ephemeral markers.
    pub fn validate(&self, plate_count: usize, duration: f64) -> Result<(), String> {
        let valid_time = |t: f64| t.is_finite() && t >= 0. && t <= duration + 1e-5;
        if self.identities.is_empty() {
            return if self.slot_uids.is_empty()
                && self.events.is_empty()
                && self.area_history.is_empty()
            {
                Ok(())
            } else {
                Err("缺少板块历史身份".into())
            };
        }
        if self.identities.len() > 8192
            || self.events.len() > 16384
            || self.area_history.len() > 1024
            || self.slot_uids.len() != plate_count
            || plate_count > MAX_PLATES
        {
            return Err("板块历史记录数量无效".into());
        }
        let valid_uid = |u: u32| (u as usize) < self.identities.len();
        let mut slots = std::collections::HashSet::new();
        if self
            .slot_uids
            .iter()
            .any(|&u| !valid_uid(u) || !slots.insert(u))
        {
            return Err("板块身份与计算编号不匹配".into());
        }
        for (i, id) in self.identities.iter().enumerate() {
            if id.uid as usize != i
                || !valid_time(id.born_myr)
                || id
                    .ended_myr
                    .is_some_and(|t| !valid_time(t) || t < id.born_myr)
                || id.parent_uid.is_some_and(|u| u as usize >= i)
                || id.absorbed_by.is_some_and(|u| !valid_uid(u) || u == id.uid)
                || !id.area_m2.is_finite()
                || id.area_m2 < 0.
                || !id.maximum_area_m2.is_finite()
                || id.maximum_area_m2 < id.area_m2
                || !matches!(
                    id.status.as_str(),
                    "active"
                        | "consumed"
                        | "absorbed"
                        | "below_resolution"
                        | "outside_window"
                        | "unresolved_initial"
                )
            {
                return Err("板块身份记录无效".into());
            }
        }
        let mut previous = 0.;
        for e in &self.events {
            if !valid_time(e.time_myr)
                || e.time_myr < previous
                || !valid_uid(e.uid)
                || e.related_uid.is_some_and(|u| !valid_uid(u))
                || !e.area_m2.is_finite()
                || e.area_m2 < 0.
                || !matches!(
                    e.kind.as_str(),
                    "oceanic_subduction"
                        | "outside_window"
                        | "below_resolution"
                        | "boundary_locked_absorption"
                        | "persistent_fragmentation"
                        | "weak_zone_rifting"
                )
            {
                return Err("板块生灭事件无效".into());
            }
            previous = e.time_myr;
        }
        previous = -1.;
        for sample in &self.area_history {
            let mut seen = std::collections::HashSet::new();
            if !valid_time(sample.time_myr)
                || sample.time_myr <= previous
                || sample.areas.len() > MAX_PLATES
                || !sample.spatial_resolution_m.is_finite()
                || sample.spatial_resolution_m <= 0.
                || sample
                    .areas
                    .iter()
                    .any(|&(u, a)| !valid_uid(u) || !seen.insert(u) || !a.is_finite() || a <= 0.)
            {
                return Err("板块面积历史无效".into());
            }
            previous = sample.time_myr;
        }
        Ok(())
    }
    pub fn active(&self, slot: usize) -> bool {
        self.active
            .get(slot)
            .copied()
            .unwrap_or(self.slot_uids.is_empty())
    }
    pub fn uid(&self, slot: usize) -> u32 {
        self.slot_uids.get(slot).copied().unwrap_or(slot as u32)
    }
    pub(crate) fn record(&mut self, g: &MapGrid, f: &WorldFields, time: f64, force: bool) {
        let mut areas = vec![0.; self.slot_uids.len()];
        for (i, &p) in f.plate_id.iter().enumerate() {
            areas[p as usize] += g.area_m2(i);
        }
        let mut sample = Vec::new();
        for (s, &area) in areas.iter().enumerate() {
            let id = &mut self.identities[self.slot_uids[s] as usize];
            if id.ended_myr.is_none() {
                id.area_m2 = area;
                id.maximum_area_m2 = id.maximum_area_m2.max(area);
                if area > 0. {
                    id.status = "active".into();
                }
            }
            if area > 0. {
                sample.push((id.uid, area));
            }
        }
        if self.area_history.last().is_none_or(|last| {
            force || time - last.time_myr >= 9.999 || (last.time_myr - time).abs() < 1e-8
        }) {
            if self
                .area_history
                .last()
                .is_some_and(|last| (last.time_myr - time).abs() < 1e-8)
            {
                self.area_history.pop();
            }
            self.area_history.push(AreaSample {
                time_myr: time,
                spatial_resolution_m: g.area_m2(0).sqrt(),
                areas: sample,
            });
        }
    }
}
pub(crate) fn initialize(g: &MapGrid, f: &WorldFields, state: &mut TectonicState) {
    let count = state.plates.len();
    state.lifecycle = Lifecycle {
        identities: (0..count)
            .map(|i| Identity {
                uid: i as u32,
                born_myr: 0.,
                ended_myr: None,
                parent_uid: None,
                absorbed_by: None,
                status: "active".into(),
                area_m2: 0.,
                maximum_area_m2: 0.,
            })
            .collect(),
        slot_uids: (0..count as u32).collect(),
        active: vec![true; count],
        locked_myr: vec![0.; count],
        fracture_myr: vec![0.; count],
        last_split_myr: vec![0.; count],
        ..Default::default()
    };
    state.lifecycle.record(g, f, 0., true);
    for s in 0..count {
        if state.lifecycle.identities[s].area_m2 == 0. {
            state.lifecycle.active[s] = false;
            state.lifecycle.identities[s].status = "unresolved_initial".into();
            state.lifecycle.needs_carrier_transfer = true;
        }
    }
    reset_markers(g, f, state);
}

fn reset_markers(g: &MapGrid, f: &WorldFields, state: &mut TectonicState) {
    let mut represented = vec![false; state.plates.len()];
    for &slot in &f.plate_id {
        represented[slot as usize] = true;
    }
    // Refining the display/transport mesh must not delete a small surviving
    // plate whose points lost the coarse cell majority. Retain those points.
    let hidden: Vec<_> = state
        .lifecycle
        .markers
        .iter()
        .filter(|m| state.lifecycle.active(m.slot) && !represented[m.slot])
        .cloned()
        .collect();
    state.lifecycle.markers = (0..g.cell_count())
        .map(|i| Marker {
            position: g.field_position(i),
            slot: f.plate_id[i] as usize,
            crust: f.crust_type[i],
            strength: f.craton_strength[i],
            rock_age: f.crust_age[i] as f64,
            damage: f.rift_damage[i],
        })
        .collect();
    state.lifecycle.markers.extend(hidden);
    state.lifecycle.marker_spacing_m = g.distance_m(0, g.neighbors(0)[0] as usize);
}

/// Smooth time-dependent tangential driving field. Its differential component
/// loads inherited weak zones; plates still move as bounded rigid bodies.
fn drive(g: &MapGrid, p: DVec3, time: f64, params: &GenerationParams) -> DVec3 {
    let phase = (params.seed % 65521) as f64 * 0.017;
    let speed = 30_000. * params.tectonic_activity;
    if let Some((w, h)) = g.extent_m() {
        let x = p.x / w * std::f64::consts::TAU;
        let y = p.y / h * std::f64::consts::TAU;
        return DVec3::new(
            (x + time / 390. + phase).sin() * (y - time / 670.).cos(),
            -h / w * (x + time / 390. + phase).cos() * (y - time / 670.).sin(),
            0.,
        ) * speed
            + DVec3::new((x - time / 520. + phase).sin(), (y + time / 810.).sin(), 0.)
                * speed
                * 0.15
            - DVec3::new((p.x / (w * 0.5)).powi(5), (p.y / (h * 0.5)).powi(5), 0.) * speed * 0.6;
    }
    let q = p.normalize();
    let mut v = DVec3::ZERO;
    for k in 0..4 {
        let a = (phase + k as f64 * 2.399963).sin();
        let axis = DVec3::new(
            a,
            (phase + k as f64 * 1.71).cos(),
            (phase + k as f64 * 3.17).sin(),
        )
        .normalize();
        let phase = time / (350. + k as f64 * 137.) + phase + k as f64;
        let tangent = axis - q * axis.dot(q);
        // Predominantly toroidal stirring, with a smaller divergent component.
        // A gradient-dominated global driver funnels every continent into its
        // artificial sinks; it is not a solution for mantle downwelling.
        v += q.cross(axis) * (q.dot(axis) * 3. + phase).sin()
            + tangent * (q.dot(axis) * 3. + phase).cos() * 0.15;
    }
    v * speed * 0.6
}

pub(crate) fn fit_motion(
    g: &MapGrid,
    f: &WorldFields,
    state: &mut TectonicState,
    p: &GenerationParams,
    time: f64,
    dt: f64,
) {
    let count = state.plates.len();
    let mut area = vec![0.; count];
    let mut centers = vec![DVec3::ZERO; count];
    for (i, &s) in f.plate_id.iter().enumerate() {
        let s = s as usize;
        let a = g.area_m2(i);
        area[s] += a;
        centers[s] += g.field_position(i) * a;
    }
    for s in 0..count {
        if area[s] > 0. {
            centers[s] /= area[s];
            if !g.field_is_planar() {
                centers[s] = centers[s].normalize() * g.radius_m();
            }
            if let Some(v) = &mut state.planar_velocities {
                let spin = state.planar_spin_rad_myr.as_ref().map_or(0., |w| w[s]);
                // Re-centering the rotation pivot is a coordinate change,
                // not an impulse: retain the same velocity at every point.
                v[s] += DVec3::Z.cross(centers[s] - state.centers[s]) * spin;
            }
            state.centers[s] = centers[s];
        }
    }
    let mut means = vec![DVec3::ZERO; count];
    let mut matrix = vec![DMat3::ZERO; count];
    let mut moment = vec![DVec3::ZERO; count];
    for (i, &s) in f.plate_id.iter().enumerate() {
        let s = s as usize;
        let pos = g.field_position(i);
        let d = drive(g, pos, time, p);
        let a = g.area_m2(i) / area[s].max(1.);
        means[s] += d * a;
        if g.field_is_planar() {
            let arm = pos - centers[s];
            moment[s].z += arm.cross(d).z * a;
            matrix[s].x_axis.x += arm.length_squared() * a;
        } else {
            let q = pos / g.radius_m();
            matrix[s] += (DMat3::IDENTITY - DMat3::from_cols(q * q.x, q * q.y, q * q.z)) * a;
            moment[s] += q.cross(d / g.radius_m()) * a;
        }
    }
    let blend = 1. - (-dt / 60.).exp();
    for s in 0..count {
        if !state.lifecycle.active(s) || area[s] <= 0. {
            continue;
        }
        if let Some(v) = &mut state.planar_velocities {
            v[s] = v[s].lerp(means[s], blend);
            if let Some(spin) = &mut state.planar_spin_rad_myr {
                spin[s] += (moment[s].z / matrix[s].x_axis.x.max(1.) - spin[s]) * blend;
            }
        } else {
            let m = matrix[s] + DMat3::IDENTITY * 1e-5;
            let old = DVec3::from_array(state.plates[s].euler_pole) * state.plates[s].omega;
            let omega = old.lerp(m.inverse() * moment[s], blend);
            if omega.length() > 1e-12 {
                state.plates[s].euler_pole = omega.normalize().to_array();
                state.plates[s].omega = omega.length();
            }
        }
    }
    crate::plates::limit_speeds(state, g, p);
}

/// Lagrangian samples keep cumulative displacement between snapshots. Taking
/// the winning Eulerian label after every small step would freeze boundaries.
/// Only samples actually overtaken at a convergent oceanic contact subduct.
pub(crate) fn advance_labels(
    g: &MapGrid,
    f: &mut WorldFields,
    state: &mut TectonicState,
    dt: f64,
    time: f64,
    refresh_material: bool,
) -> bool {
    let spacing = g.distance_m(0, g.neighbors(0)[0] as usize);
    if (state.lifecycle.marker_spacing_m - spacing).abs() > spacing * 0.01
        || state.lifecycle.marker_spacing_m <= 0.
    {
        reset_markers(g, f, state);
    }
    let n = g.cell_count();
    let count = state.plates.len();
    let locate = |p: DVec3| {
        if g.field_is_planar() {
            g.locate_local(p.x, p.y)
        } else {
            UnitVec3::from_vec(p).and_then(|q| g.locate(q))
        }
    };
    let mut markers = std::mem::take(&mut state.lifecycle.markers);
    let mut buckets = vec![Vec::new(); n];
    // Buoyant continental pairs cannot subduct. Keep the eligible oceanic
    // subset in the same order so dense continental overlaps do not cause a
    // quadratic search through contacts that would always be discarded.
    let mut ocean_buckets = vec![Vec::new(); n];
    let mut dead = vec![false; markers.len()];
    let mut lost_subduction = vec![0usize; count];
    let mut lost_window = vec![0usize; count];
    for (k, m) in markers.iter_mut().enumerate() {
        if !state.lifecycle.active(m.slot) {
            dead[k] = true;
            continue;
        }
        m.position = crate::evolution::move_point(state, m.slot, m.position, dt);
        m.rock_age += dt;
        if m.crust > 0.65 && m.strength >= 0.2 {
            m.strength = (0.95 - (0.95 - m.strength as f64) * (-dt / 700.).exp()) as f32;
        }
        if let Some(i) = locate(m.position) {
            buckets[i].push(k);
            if m.crust <= 0.65 {
                ocean_buckets[i].push(k);
            }
        } else {
            dead[k] = true;
            lost_window[m.slot] += 1;
        }
    }
    for a in 0..markers.len() {
        if dead[a] {
            continue;
        }
        let Some(i) = locate(markers[a].position) else {
            continue;
        };
        for cell in std::iter::once(i).chain(g.neighbors(i).iter().map(|&j| j as usize)) {
            let candidates = if markers[a].crust > 0.65 {
                &ocean_buckets[cell]
            } else {
                &buckets[cell]
            };
            for &b in candidates {
                if b <= a || dead[a] || dead[b] || markers[a].slot == markers[b].slot {
                    continue;
                }
                let (ma, mb) = (&markers[a], &markers[b]);
                let delta = mb.position - ma.position;
                if delta.length_squared() > spacing.powi(2) * 0.20 || delta.length_squared() < 1e-12
                {
                    continue;
                }
                let mid = if g.field_is_planar() {
                    (ma.position + mb.position) * 0.5
                } else {
                    (ma.position + mb.position).normalize() * g.radius_m()
                };
                let relative = crate::evolution::velocity(state, ma.slot, mid)
                    - crate::evolution::velocity(state, mb.slot, mid);
                if relative.dot(delta.normalize()) < 500. {
                    continue;
                }
                let density =
                    |m: &Marker| (1. - m.crust as f64) * 2. + m.rock_age.min(250.) / 1000.;
                let under = if density(ma) > density(mb) { a } else { b };
                dead[under] = true;
                lost_subduction[markers[under].slot] += 1;
            }
        }
    }
    let mut kept = Vec::with_capacity(markers.len());
    for (k, m) in markers.into_iter().enumerate() {
        if !dead[k] {
            kept.push(m);
        }
    }
    markers = kept;
    let mut alive = vec![0usize; count];
    for m in &markers {
        alive[m.slot] += 1;
    }
    for s in 0..count {
        state.lifecycle.subducted_surface_samples += lost_subduction[s] as u64;
        state.lifecycle.exported_surface_samples += lost_window[s] as u64;
        if !state.lifecycle.active(s) || alive[s] > 0 {
            continue;
        }
        let uid = state.lifecycle.uid(s);
        let id = &mut state.lifecycle.identities[uid as usize];
        let consumed = lost_subduction[s] > 0 && lost_window[s] == 0;
        id.status = if consumed {
            "consumed"
        } else if g.field_is_planar() {
            "outside_window"
        } else {
            "below_resolution"
        }
        .into();
        id.area_m2 = 0.;
        if consumed {
            id.ended_myr = Some(time);
        }
        state.lifecycle.active[s] = false;
        state.lifecycle.needs_carrier_transfer = true;
        state.lifecycle.events.push(Event {
            time_myr: time,
            kind: if consumed {
                "oceanic_subduction"
            } else if lost_window[s] > 0 {
                "outside_window"
            } else {
                "below_resolution"
            }
            .into(),
            uid,
            related_uid: None,
            area_m2: 0.,
        });
    }
    if markers.is_empty() {
        state.lifecycle.markers = markers;
        return false;
    }
    let mut best = vec![f64::INFINITY; n];
    let mut owners = vec![usize::MAX; n];
    let mut heap = BinaryHeap::new();
    for (k, m) in markers.iter().enumerate() {
        if let Some(i) = locate(m.position) {
            let d = g.field_position(i).distance_squared(m.position);
            if d < best[i] {
                best[i] = d;
                owners[i] = k;
                heap.push(Visit {
                    distance: d,
                    cell: i,
                    marker: k,
                });
            }
        }
    }
    while let Some(v) = heap.pop() {
        if owners[v.cell] != v.marker || v.distance > best[v.cell] + 1e-6 {
            continue;
        }
        for &j in g.neighbors(v.cell) {
            let j = j as usize;
            let d = g
                .field_position(j)
                .distance_squared(markers[v.marker].position);
            if d + 1e-6 < best[j] {
                best[j] = d;
                owners[j] = v.marker;
                heap.push(Visit {
                    distance: d,
                    cell: j,
                    marker: v.marker,
                });
            }
        }
    }
    for i in 0..n {
        let m = &markers[owners[i]];
        f.plate_id[i] = m.slot as u16;
        f.rift_damage[i] = m.damage;
        if refresh_material {
            f.crust_type[i] = m.crust;
            f.craton_strength[i] = m.strength * (1. - 0.6 * m.damage);
            f.crust_age[i] = m.rock_age as f32;
        }
    }
    // New surface samples only fill a resolved spreading gap. They belong to
    // existing bordering plates; spreading is not itself a new plate identity.
    for i in 0..n {
        if best[i] < spacing.powi(2) * 0.81 || markers.len() >= n * 3 {
            continue;
        }
        let a = f.plate_id[i] as usize;
        let p = g.field_position(i);
        let spreading = g.neighbors(i).iter().any(|&j| {
            let j = j as usize;
            let b = f.plate_id[j] as usize;
            if b == a {
                return false;
            }
            let delta = g.field_position(j) - p;
            let relative =
                crate::evolution::velocity(state, a, p) - crate::evolution::velocity(state, b, p);
            relative.dot(delta.normalize()) < -1000.
        });
        if spreading {
            markers.push(Marker {
                position: p,
                slot: a,
                crust: 0.,
                strength: 0.,
                rock_age: 0.,
                damage: 0.,
            });
            state.lifecycle.ridge_surface_samples += 1;
        }
    }
    state.lifecycle.markers = markers;
    true
}
#[derive(Clone, Copy)]
struct Visit {
    distance: f64,
    cell: usize,
    marker: usize,
}
impl PartialEq for Visit {
    fn eq(&self, b: &Self) -> bool {
        self.distance == b.distance && self.cell == b.cell && self.marker == b.marker
    }
}
impl Eq for Visit {}
impl PartialOrd for Visit {
    fn partial_cmp(&self, b: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(b))
    }
}
impl Ord for Visit {
    fn cmp(&self, b: &Self) -> std::cmp::Ordering {
        b.distance
            .total_cmp(&self.distance)
            .then_with(|| b.cell.cmp(&self.cell))
            .then_with(|| b.marker.cmp(&self.marker))
    }
}

fn components(g: &MapGrid, ids: &[u16], slot: usize) -> Vec<Vec<usize>> {
    let mut seen = vec![false; ids.len()];
    let mut all = Vec::new();
    for i in 0..ids.len() {
        if seen[i] || ids[i] as usize != slot {
            continue;
        }
        let mut cells = vec![i];
        seen[i] = true;
        let mut head = 0;
        while head < cells.len() {
            let j = cells[head];
            head += 1;
            for &k in g.neighbors(j) {
                let k = k as usize;
                if !seen[k] && ids[k] as usize == slot {
                    seen[k] = true;
                    cells.push(k);
                }
            }
        }
        all.push(cells);
    }
    all.sort_by(|a, b| b.len().cmp(&a.len()));
    all
}

fn spawn(
    g: &MapGrid,
    f: &mut WorldFields,
    state: &mut TectonicState,
    parent: usize,
    cells: &[usize],
    time: f64,
    kind: &str,
) -> bool {
    if !state.lifecycle.markers.is_empty()
        && !state.lifecycle.markers.iter().any(|m| {
            if m.slot != parent {
                return false;
            }
            let i = if g.field_is_planar() {
                g.locate_local(m.position.x, m.position.y)
            } else {
                UnitVec3::from_vec(m.position).and_then(|q| g.locate(q))
            };
            i.is_some_and(|i| cells.contains(&i))
        })
    {
        return false;
    }
    let free = state
        .lifecycle
        .active
        .iter()
        .enumerate()
        .find(|(s, on)| {
            !**on && {
                let id = &state.lifecycle.identities[state.lifecycle.uid(*s) as usize];
                id.ended_myr.is_some() || id.status == "unresolved_initial"
            }
        })
        .map(|(s, _)| s);
    let slot = free.unwrap_or(state.plates.len());
    if slot >= MAX_PLATES {
        state.lifecycle.capacity_limited_events += 1;
        return false;
    }
    let uid = state.lifecycle.identities.len() as u32;
    let parent_uid = state.lifecycle.uid(parent);
    if slot == state.plates.len() {
        state.plates.push(state.plates[parent]);
        state.centers.push(state.centers[parent]);
        state
            .partition_weights
            .push(state.partition_weights[parent]);
        if let Some(v) = &mut state.planar_velocities {
            v.push(v[parent]);
        }
        if let Some(v) = &mut state.planar_spin_rad_myr {
            v.push(v[parent]);
        }
        state.lifecycle.slot_uids.push(uid);
        state.lifecycle.active.push(true);
        state.lifecycle.locked_myr.push(0.);
        state.lifecycle.fracture_myr.push(0.);
        state.lifecycle.last_split_myr.push(time);
    } else {
        state.plates[slot] = state.plates[parent];
        state.centers[slot] = state.centers[parent];
        state.partition_weights[slot] = state.partition_weights[parent];
        if let Some(v) = &mut state.planar_velocities {
            v[slot] = v[parent];
        }
        if let Some(v) = &mut state.planar_spin_rad_myr {
            v[slot] = v[parent];
        }
        state.lifecycle.slot_uids[slot] = uid;
        state.lifecycle.active[slot] = true;
        state.lifecycle.locked_myr[slot] = 0.;
        state.lifecycle.fracture_myr[slot] = 0.;
        state.lifecycle.last_split_myr[slot] = time;
    }
    state.plates[slot].id = slot as u16;
    state.lifecycle.last_split_myr[parent] = time;
    let area = cells.iter().map(|&i| g.area_m2(i)).sum();
    state.lifecycle.identities.push(Identity {
        uid,
        born_myr: time,
        ended_myr: None,
        parent_uid: Some(parent_uid),
        absorbed_by: None,
        status: "active".into(),
        area_m2: area,
        maximum_area_m2: area,
    });
    state.lifecycle.events.push(Event {
        time_myr: time,
        kind: kind.into(),
        uid,
        related_uid: Some(parent_uid),
        area_m2: area,
    });
    for &i in cells {
        f.plate_id[i] = slot as u16;
        f.rift_damage[i] *= 0.4;
    }
    for m in &mut state.lifecycle.markers {
        if m.slot != parent {
            continue;
        }
        let i = if g.field_is_planar() {
            g.locate_local(m.position.x, m.position.y)
        } else {
            UnitVec3::from_vec(m.position).and_then(|p| g.locate(p))
        };
        if i.is_some_and(|i| f.plate_id[i] as usize == slot) {
            m.slot = slot;
            m.damage *= 0.4;
        }
    }
    true
}

fn transfer_carriers(
    g: &MapGrid,
    f: &WorldFields,
    state: &mut TectonicState,
    volcanoes: &mut [crate::evolution::Volcano],
) {
    let locate = |p: DVec3| {
        if g.field_is_planar() {
            g.locate_local(p.x, p.y)
        } else {
            UnitVec3::from_vec(p).and_then(|q| g.locate(q))
        }
    };
    let fallback = |slot: usize, pos: DVec3| {
        let active = &state.lifecycle;
        if active.active(slot) {
            return slot;
        }
        let related = active.identities[active.uid(slot) as usize].absorbed_by;
        if let Some(s) = active
            .slot_uids
            .iter()
            .enumerate()
            .find(|(s, uid)| active.active(*s) && Some(**uid) == related)
            .map(|(s, _)| s)
        {
            return s;
        }
        state
            .centers
            .iter()
            .enumerate()
            .filter(|(s, _)| active.active(*s))
            .min_by(|(_, a), (_, b)| a.distance_squared(pos).total_cmp(&b.distance_squared(pos)))
            .map_or(slot, |(s, _)| s)
    };
    let assignments: Vec<_> = state
        .provinces
        .iter()
        .map(|p| {
            locate(p.shape.center).map_or_else(
                || fallback(p.plate, p.shape.center),
                |i| f.plate_id[i] as usize,
            )
        })
        .collect();
    for (p, slot) in state.provinces.iter_mut().zip(assignments) {
        p.plate = slot;
    }
    for v in volcanoes {
        let pos = DVec3::from_array(v.position_m);
        v.plate_id = locate(pos).map_or_else(
            || fallback(v.plate_id as usize, pos) as u16,
            |i| f.plate_id[i],
        );
    }
}

pub(crate) fn update(
    g: &MapGrid,
    f: &mut WorldFields,
    state: &mut TectonicState,
    p: &GenerationParams,
    time: f64,
    dt: f64,
    volcanoes: &mut [crate::evolution::Volcano],
) {
    if state.lifecycle.slot_uids.is_empty() {
        initialize(g, f, state);
    }
    let count = state.plates.len();
    let mut areas = vec![0.; count];
    let mut touches = vec![false; count];
    let drivers: Vec<_> = (0..g.cell_count())
        .map(|i| drive(g, g.field_position(i), time, p))
        .collect();
    for i in 0..g.cell_count() {
        let slot = f.plate_id[i] as usize;
        areas[slot] += g.area_m2(i);
        touches[slot] |= g.boundary_mask()[i];
        let mut extension = 0_f64;
        for &j in g.neighbors(i) {
            let j = j as usize;
            if f.plate_id[j] as usize != slot {
                continue;
            }
            let d = g.field_position(j) - g.field_position(i);
            extension =
                extension.max((drivers[j] - drivers[i]).dot(d) / d.length_squared().max(1.));
        }
        f.rift_damage[i] = (f.rift_damage[i] as f64 * (-dt / 500.).exp()
            + extension * dt * (1. - f.craton_strength[i] as f64).powi(2))
        .clamp(0., 1.) as f32;
    }
    let boundaries = crate::plates::find_boundaries(g, &f.plate_id, state, f);
    for m in &mut state.lifecycle.markers {
        let cell = if g.field_is_planar() {
            g.locate_local(m.position.x, m.position.y)
        } else {
            UnitVec3::from_vec(m.position).and_then(|d| g.locate(d))
        };
        if let Some(i) = cell {
            m.damage = f.rift_damage[i];
        }
    }
    let mut convergence = vec![0.; count];
    let mut length = vec![0.; count];
    let mut neighbors = vec![vec![0.; count]; count];
    let mut relative = vec![0.; count];
    for b in boundaries {
        let (i, j) = (b.cell as usize, b.other as usize);
        let (a, c) = (f.plate_id[i] as usize, f.plate_id[j] as usize);
        let l = 2. * (g.area_m2(i) * g.area_m2(j)).sqrt() / (3. * g.distance_m(i, j));
        for (s, t) in [(a, c), (c, a)] {
            length[s] += l;
            neighbors[s][t] += l;
            relative[s] += b.normal_velocity.hypot(b.tangential_velocity) * l;
            if b.normal_velocity > 1000. {
                convergence[s] += l;
            }
        }
    }
    let mut changes = state.lifecycle.needs_carrier_transfer;
    state.lifecycle.needs_carrier_transfer = false;
    for s in 0..count {
        if !state.lifecycle.active(s) {
            continue;
        }
        if areas[s] == 0. {
            let uid = state.lifecycle.uid(s);
            let id = &mut state.lifecycle.identities[uid as usize];
            // A vanished coarse sample is not proof of physical destruction.
            let surviving = state.lifecycle.markers.iter().any(|m| m.slot == s);
            id.status = if surviving {
                "below_resolution"
            } else if g.field_is_planar() {
                "outside_window"
            } else {
                "below_resolution"
            }
            .into();
            id.area_m2 = 0.;
            state.lifecycle.active[s] = surviving;
            if surviving {
                continue;
            }
            state.lifecycle.events.push(Event {
                time_myr: time,
                kind: id.status.clone(),
                uid,
                related_uid: None,
                area_m2: 0.,
            });
            changes = true;
            continue;
        }
        let small = areas[s] < (g.area_m2(0) * 12.).max(2e11);
        let slow = length[s] > 0. && relative[s] / length[s] < 1500.;
        let target = neighbors[s]
            .iter()
            .enumerate()
            .filter(|(t, _)| *t != s && state.lifecycle.active(*t))
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(t, l)| (t, *l));
        let locked =
            target.is_some_and(|(_, l)| l > length[s] * 0.8) && slow && small && !touches[s];
        state.lifecycle.locked_myr[s] = if locked {
            state.lifecycle.locked_myr[s] + dt
        } else {
            0.
        };
        if state.lifecycle.locked_myr[s] >= 20. {
            let target = target.unwrap().0;
            let uid = state.lifecycle.uid(s);
            let receiver = state.lifecycle.uid(target);
            let id = &mut state.lifecycle.identities[uid as usize];
            id.status = "absorbed".into();
            id.ended_myr = Some(time);
            id.absorbed_by = Some(receiver);
            id.area_m2 = 0.;
            state.lifecycle.active[s] = false;
            state.lifecycle.events.push(Event {
                time_myr: time,
                kind: "boundary_locked_absorption".into(),
                uid,
                related_uid: Some(receiver),
                area_m2: areas[s],
            });
            for id in &mut f.plate_id {
                if *id as usize == s {
                    *id = target as u16;
                }
            }
            for m in &mut state.lifecycle.markers {
                if m.slot == s {
                    m.slot = target;
                }
            }
            changes = true;
        }
    }
    // Rebind before any slot can be reused, including objects outside the window.
    if changes {
        transfer_carriers(g, f, state, volcanoes);
    }
    for s in 0..count {
        if !state.lifecycle.active(s)
            || time - state.lifecycle.identities[state.lifecycle.uid(s) as usize].born_myr < 30.
            || time - state.lifecycle.last_split_myr[s] < 30.
        {
            continue;
        }
        let all = components(g, &f.plate_id, s);
        if all.len() > 1 && all[1].len() >= 6 {
            state.lifecycle.fracture_myr[s] += dt;
            if state.lifecycle.fracture_myr[s] >= 20.
                && all[1].iter().map(|&i| g.area_m2(i)).sum::<f64>() > 2e11
            {
                changes |= spawn(g, f, state, s, &all[1], time, "persistent_fragmentation");
                state.lifecycle.fracture_myr[s] = 0.;
            }
        } else {
            state.lifecycle.fracture_myr[s] = 0.;
        }
        if areas[s] > g.area_m2(0) * 30. {
            if let Some(cells) = rift_cut(g, f, s, &drivers) {
                changes |= spawn(g, f, state, s, &cells, time, "weak_zone_rifting");
            }
        }
        if changes {
            transfer_carriers(g, f, state, volcanoes);
            break;
        }
    }
    state.lifecycle.record(g, f, time, false);
}

/// Weighted minimum cut: two broad opposite interiors stay connected to their
/// terminals. Only a cut supported by an already damaged, through-going weak
/// band may create an independently moving daughter plate.
fn rift_cut(g: &MapGrid, f: &WorldFields, slot: usize, drivers: &[DVec3]) -> Option<Vec<usize>> {
    let members: Vec<_> = (0..g.cell_count())
        .filter(|&i| f.plate_id[i] as usize == slot)
        .collect();
    let anchor = *members
        .iter()
        .max_by(|&&a, &&b| f.rift_damage[a].total_cmp(&f.rift_damage[b]))?;
    if f.rift_damage[anchor] < 0.25 {
        return None;
    }
    let mut normal = DVec3::ZERO;
    let mut loading = 0.;
    for &j in g.neighbors(anchor) {
        let j = j as usize;
        if f.plate_id[j] as usize != slot {
            continue;
        }
        let d = g.field_position(j) - g.field_position(anchor);
        let e = (drivers[j] - drivers[anchor]).dot(d) / d.length_squared().max(1.);
        if e > loading {
            loading = e;
            normal = d.normalize();
        }
    }
    if loading < 0.0002 {
        return None;
    }
    if !g.field_is_planar() {
        let up = g.field_position(anchor).normalize();
        normal = (normal - up * normal.dot(up)).normalize_or_zero();
    }
    let origin = g.field_position(anchor);
    let mut positions: Vec<_> = members
        .iter()
        .map(|&i| (i, (g.field_position(i) - origin).dot(normal)))
        .collect();
    positions.sort_by(|a, b| a.1.total_cmp(&b.1));
    let (low, high) = (
        positions[positions.len() / 4].1,
        positions[positions.len() * 3 / 4].1,
    );
    if low >= 0. || high <= 0. {
        return None;
    }
    let mut index = vec![usize::MAX; g.cell_count()];
    for (k, &i) in members.iter().enumerate() {
        index[i] = k;
    }
    let source = members.len();
    let sink = source + 1;
    let mut network = Network::new(sink + 1);
    let mut edges = Vec::new();
    let inf = 1e12;
    for (i, x) in positions {
        let a = index[i];
        if x < low {
            network.add(source, a, inf);
        }
        if x > high {
            network.add(a, sink, inf);
        }
        for &j in g.neighbors(i) {
            let j = j as usize;
            if j <= i || index[j] == usize::MAX {
                continue;
            }
            let stability = f.craton_strength[i].max(f.craton_strength[j]) as f64;
            let damage = f.rift_damage[i].min(f.rift_damage[j]) as f64;
            let cost = 0.02 + 4. * stability.powi(3) + 2. * (1. - damage).powi(2);
            network.add(a, index[j], cost);
            network.add(index[j], a, cost);
            edges.push((i, j));
        }
    }
    network.flow(source, sink);
    let side = network.reachable(source);
    let crossing: Vec<_> = edges
        .iter()
        .filter(|&&(i, j)| side[index[i]] != side[index[j]])
        .collect();
    if crossing.len() < 3 {
        return None;
    }
    let weak = crossing
        .iter()
        .filter(|&&(i, j)| f.rift_damage[*i].min(f.rift_damage[*j]) >= 0.18)
        .count();
    if weak * 4 < crossing.len() * 3 {
        return None;
    }
    let mut left: Vec<_> = members
        .iter()
        .copied()
        .filter(|&i| side[index[i]])
        .collect();
    let right: Vec<_> = members
        .iter()
        .copied()
        .filter(|&i| !side[index[i]])
        .collect();
    if left.len() < 6 || right.len() < 6 {
        return None;
    }
    if left.len() > right.len() {
        left = right;
    }
    // A detached island is handled by fragmentation, never by a fabricated rift.
    let selected: std::collections::HashSet<_> = left.iter().copied().collect();
    let mut seen = std::collections::HashSet::new();
    let mut todo = vec![left[0]];
    while let Some(i) = todo.pop() {
        if !seen.insert(i) {
            continue;
        }
        for &j in g.neighbors(i) {
            let j = j as usize;
            if selected.contains(&j) {
                todo.push(j);
            }
        }
    }
    if seen.len() != left.len() {
        return None;
    }
    Some(left)
}

#[derive(Clone)]
struct Edge {
    to: usize,
    reverse: usize,
    capacity: f64,
}
struct Network {
    edges: Vec<Vec<Edge>>,
    level: Vec<i32>,
    next: Vec<usize>,
}
impl Network {
    fn new(n: usize) -> Self {
        Self {
            edges: vec![Vec::new(); n],
            level: vec![-1; n],
            next: vec![0; n],
        }
    }
    fn add(&mut self, a: usize, b: usize, c: f64) {
        let (ar, br) = (self.edges[a].len(), self.edges[b].len());
        self.edges[a].push(Edge {
            to: b,
            reverse: br,
            capacity: c,
        });
        self.edges[b].push(Edge {
            to: a,
            reverse: ar,
            capacity: 0.,
        });
    }
    fn push(&mut self, v: usize, t: usize, amount: f64, depth: usize) -> f64 {
        if v == t {
            return amount;
        }
        if depth > 1024 {
            return 0.;
        }
        while self.next[v] < self.edges[v].len() {
            let k = self.next[v];
            let e = self.edges[v][k].clone();
            if e.capacity > 1e-9 && self.level[e.to] == self.level[v] + 1 {
                let flow = self.push(e.to, t, amount.min(e.capacity), depth + 1);
                if flow > 1e-9 {
                    self.edges[v][k].capacity -= flow;
                    self.edges[e.to][e.reverse].capacity += flow;
                    return flow;
                }
            }
            self.next[v] += 1;
        }
        0.
    }
    fn flow(&mut self, s: usize, t: usize) {
        for _ in 0..self.edges.len() {
            self.level.fill(-1);
            self.level[s] = 0;
            let mut q = VecDeque::from([s]);
            while let Some(i) = q.pop_front() {
                for e in &self.edges[i] {
                    if e.capacity > 1e-9 && self.level[e.to] < 0 {
                        self.level[e.to] = self.level[i] + 1;
                        q.push_back(e.to);
                    }
                }
            }
            if self.level[t] < 0 {
                return;
            }
            self.next.fill(0);
            let mut total = 0.;
            loop {
                let q = self.push(s, t, 1e30, 0);
                if q <= 1e-9 {
                    break;
                }
                total += q;
            }
            if total <= 1e-9 {
                return;
            }
        }
    }
    fn reachable(&self, s: usize) -> Vec<bool> {
        let mut seen = vec![false; self.edges.len()];
        let mut q = VecDeque::from([s]);
        seen[s] = true;
        while let Some(i) = q.pop_front() {
            for e in &self.edges[i] {
                if e.capacity > 1e-9 && !seen[e.to] {
                    seen[e.to] = true;
                    q.push_back(e.to);
                }
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (MapGrid, WorldFields, TectonicState) {
        let g = MapGrid::Plane(tf_grid::FlatGrid::new(
            24, 20, 2_400_000., 25., 0., 6371000.,
        ));
        let mut f = WorldFields::with_capacity(g.cell_count());
        f.crust_type.fill(1.);
        f.crust_thickness.fill(35000.);
        f.reference_crust_thickness.fill(35000.);
        f.crust_age.fill(800.);
        f.elevation.fill(400.);
        f.craton_strength.fill(0.1);
        let state = TectonicState {
            plates: (0..3)
                .map(|id| crate::Plate {
                    id,
                    euler_pole: DVec3::Z.to_array(),
                    omega: 0.,
                    oceanic: false,
                })
                .collect(),
            centers: vec![DVec3::ZERO; 3],
            planar_velocities: Some(vec![DVec3::ZERO; 3]),
            planar_spin_rad_myr: Some(vec![0.; 3]),
            partition_weights: vec![1.; 3],
            ..Default::default()
        };
        (g, f, state)
    }
    #[test]
    fn an_empty_finite_window_stops_instead_of_publishing_inactive_labels() {
        let (g, mut f, mut state) = fixture();
        initialize(&g, &f, &mut state);
        state
            .planar_velocities
            .as_mut()
            .unwrap()
            .fill(DVec3::X * 1e8);
        assert!(!advance_labels(&g, &mut f, &mut state, 1., 1., false));
        assert!(state.lifecycle.active.iter().all(|on| !*on));
        assert!(state
            .lifecycle
            .identities
            .iter()
            .all(|id| id.ended_myr.is_none()));
    }
    #[test]
    fn global_driving_is_stirring_rather_than_dominated_by_artificial_sinks() {
        let g = tf_grid::GridDescriptor::sphere(8, 6371000.).build();
        let p = GenerationParams::default();
        let (mut divergent, mut rotational) = (0., 0.);
        let epsilon = 0.0001;
        for i in 0..g.cell_count() {
            let q = g.center_dir(i);
            let (e, n) = q.tangent_frame();
            let delta = |axis: DVec3| {
                let a = (q.as_vec() + axis * epsilon).normalize() * g.radius_m();
                let b = (q.as_vec() - axis * epsilon).normalize() * g.radius_m();
                (drive(&g, a, 1300., &p) - drive(&g, b, 1300., &p)) / (2. * epsilon * g.radius_m())
            };
            let (de, dn) = (delta(e), delta(n));
            divergent += (de.dot(e) + dn.dot(n)).powi(2) * g.area_m2(i);
            rotational += (de.dot(n) - dn.dot(e)).powi(2) * g.area_m2(i);
        }
        assert!(
            divergent < rotational * 0.25,
            "divergent {divergent}, rotational {rotational}"
        );
    }
    #[test]
    fn changing_a_planar_rotation_pivot_preserves_physical_velocity() {
        let (g, f, mut state) = fixture();
        state.centers[0] = DVec3::new(200000., 300000., 0.);
        state.planar_spin_rad_myr.as_mut().unwrap()[0] = 0.02;
        state.planar_velocities.as_mut().unwrap()[0] = DVec3::X * 10000.;
        initialize(&g, &f, &mut state);
        let point = g.field_position(30);
        let before = crate::evolution::velocity(&state, 0, point);
        fit_motion(&g, &f, &mut state, &GenerationParams::default(), 1., 0.);
        assert!(before.distance(crate::evolution::velocity(&state, 0, point)) < 1e-8);
    }
    #[test]
    fn mesh_refinement_retains_hidden_survivors_and_persisted_identity_is_validated() {
        let (g, mut f, mut state) = fixture();
        let i = g.cell_count() / 2;
        f.plate_id[i] = 1;
        initialize(&g, &f, &mut state);
        let old = state
            .lifecycle
            .markers
            .iter()
            .filter(|m| m.slot == 1)
            .count();
        assert_eq!(old, 1);
        f.plate_id[i] = 0;
        reset_markers(&g, &f, &mut state);
        assert_eq!(
            state
                .lifecycle
                .markers
                .iter()
                .filter(|m| m.slot == 1)
                .count(),
            old
        );
        advance_labels(&g, &mut f, &mut state, 0., 1., false);
        assert!(state.lifecycle.active(1));
        assert!(state.lifecycle.identities[1].ended_myr.is_none());
        assert!(state
            .lifecycle
            .events
            .iter()
            .all(|e| e.kind != "outside_window"));
        state.lifecycle.record(&g, &f, 1.5, true);
        assert_eq!(state.lifecycle.area_history.last().unwrap().time_myr, 1.5);
        assert!(state.lifecycle.validate(3, 1.5).is_ok());
        let mut corrupt = state.lifecycle.clone();
        corrupt.slot_uids[1] = 0;
        assert!(corrupt.validate(3, 1.5).is_err());
        corrupt = state.lifecycle.clone();
        corrupt.identities[1].parent_uid = Some(1);
        assert!(corrupt.validate(3, 1.5).is_err());
    }
    #[test]
    fn a_throughgoing_weak_band_splits_without_resetting_old_material() {
        let (g, mut f, mut state) = fixture();
        for i in 0..g.cell_count() {
            if g.field_position(i).x.abs() < 160000. {
                f.rift_damage[i] = 0.8;
            }
        }
        initialize(&g, &f, &mut state);
        let driver: Vec<_> = (0..g.cell_count())
            .map(|i| DVec3::X * g.field_position(i).x * 0.01)
            .collect();
        let cut =
            rift_cut(&g, &f, 0, &driver).expect("continuous weak band should divide the plate");
        let age = f.crust_age.clone();
        let rock = f.crust_thickness.clone();
        let height = f.elevation.clone();
        assert!(spawn(
            &g,
            &mut f,
            &mut state,
            0,
            &cut,
            100.,
            "weak_zone_rifting"
        ));
        assert_eq!(f.crust_age, age);
        assert_eq!(f.crust_thickness, rock);
        assert_eq!(f.elevation, height);
        let child = state.lifecycle.identities.last().unwrap();
        assert_eq!(child.parent_uid, Some(0));
        assert_eq!(child.born_myr, 100.);
        let slot = state
            .lifecycle
            .slot_uids
            .iter()
            .position(|&uid| uid == child.uid)
            .unwrap();
        assert!(components(&g, &f.plate_id, slot).len() == 1);
        let strong = WorldFields {
            rift_damage: vec![0.; g.cell_count()],
            ..f.clone()
        };
        assert!(rift_cut(&g, &strong, 0, &driver).is_none());
    }
    #[test]
    fn absorption_ends_identity_and_reused_slot_cannot_capture_old_carriers() {
        let (g, mut f, mut state) = fixture();
        for i in 0..g.cell_count() {
            let pos = g.field_position(i);
            if pos.x.abs() < 160000. && pos.y.abs() < 160000. {
                f.plate_id[i] = 2;
            }
        }
        initialize(&g, &f, &mut state);
        let before = f.crust_age.clone();
        let mut volcanoes = vec![crate::evolution::Volcano {
            position_m: [0., 0., 0.],
            plate_id: 2,
            age_myr: 300.,
            radius_m: 40000.,
            summit_growth_m: 3000.,
            origin: "hotspot".into(),
            along_arc: [1., 0., 0.],
            arc_platform_fraction: 0.,
            shape_seed: 0,
            quiet_myr: 0.,
            preserved_growth_m: None,
        }];
        update(
            &g,
            &mut f,
            &mut state,
            &GenerationParams::default(),
            20.,
            20.,
            &mut volcanoes,
        );
        assert_eq!(state.lifecycle.identities[2].status, "absorbed");
        assert_eq!(state.lifecycle.identities[2].ended_myr, Some(20.));
        assert_eq!(volcanoes[0].plate_id, 0);
        assert_eq!(volcanoes[0].age_myr, 300.);
        assert_eq!(f.crust_age, before);
        let selected: Vec<_> = (0..g.cell_count())
            .filter(|&i| g.field_position(i).x > 0.)
            .collect();
        assert!(spawn(
            &g,
            &mut f,
            &mut state,
            0,
            &selected,
            100.,
            "weak_zone_rifting"
        ));
        // An unresolved initial slot may be reused without claiming that a
        // physically observed plate died. Historical UID 2 remains absorbed.
        assert_eq!(state.lifecycle.uid(1), 3);
        assert_eq!(volcanoes[0].plate_id, 0);
        assert_eq!(state.lifecycle.identities[2].absorbed_by, Some(0));
        assert_eq!(f.crust_age, before);
        let areas: f64 = (0..g.cell_count()).map(|i| g.area_m2(i)).sum();
        state.lifecycle.record(&g, &f, 100., true);
        assert!(
            (state
                .lifecycle
                .area_history
                .last()
                .unwrap()
                .areas
                .iter()
                .map(|(_, a)| a)
                .sum::<f64>()
                - areas)
                .abs()
                / areas
                < 1e-12
        );
    }
    #[test]
    fn disappearing_from_a_finite_window_is_not_physical_death() {
        let (g, mut f, mut state) = fixture();
        for i in 0..g.cell_count() {
            if g.field_position(i).x > 900000. {
                f.plate_id[i] = 1;
            }
        }
        state.planar_velocities.as_mut().unwrap()[1] = DVec3::X * 500000.;
        initialize(&g, &f, &mut state);
        advance_labels(&g, &mut f, &mut state, 1., 1., false);
        update(
            &g,
            &mut f,
            &mut state,
            &GenerationParams::default(),
            1.,
            1.,
            &mut [],
        );
        assert_eq!(state.lifecycle.identities[1].status, "outside_window");
        assert_eq!(state.lifecycle.identities[1].ended_myr, None);
        assert!(state.lifecycle.events.iter().all(|e| e.kind != "consumed"));
        let old = state.lifecycle.uid(1);
        advance_labels(&g, &mut f, &mut state, 1., 2., false);
        assert!(f
            .plate_id
            .iter()
            .all(|&s| state.lifecycle.uid(s as usize) != old));
    }

    #[test]
    fn subduction_consumes_a_moving_oceanic_remnant_and_never_revives_its_identity() {
        let (g, mut f, mut state) = fixture();
        for i in 0..g.cell_count() {
            let p = g.field_position(i);
            if p.x.abs() < 160000. && p.y.abs() < 160000. {
                f.plate_id[i] = 2;
                f.crust_type[i] = 0.;
            }
        }
        state.planar_velocities.as_mut().unwrap()[2] = DVec3::X * 40000.;
        initialize(&g, &f, &mut state);
        let before = f.crust_age.clone();
        let mut saw_loss = false;
        for step in 1..=100 {
            advance_labels(&g, &mut f, &mut state, 0.5, step as f64 * 0.5, false);
            if !state.lifecycle.active(2) {
                saw_loss = true;
            }
        }
        assert!(saw_loss, "converging oceanic samples should be consumed");
        assert_eq!(state.lifecycle.identities[2].status, "consumed");
        assert!(state.lifecycle.identities[2].ended_myr.is_some());
        assert!(state.lifecycle.subducted_surface_samples > 0);
        assert!(f.plate_id.iter().all(|&p| p != 2));
        assert_eq!(f.crust_age, before);
    }

    #[test]
    fn small_steps_accumulate_real_motion_instead_of_freezing_the_winning_label() {
        let (g, mut f, mut state) = fixture();
        for i in 0..g.cell_count() {
            if g.field_position(i).x > 0. {
                f.plate_id[i] = 1;
            }
        }
        state
            .planar_velocities
            .as_mut()
            .unwrap()
            .fill(DVec3::X * 10000.);
        initialize(&g, &f, &mut state);
        let old = f.plate_id.clone();
        let mut refined = state.clone();
        let mut finer = f.clone();
        for step in 1..=20 {
            advance_labels(&g, &mut f, &mut state, 1., step as f64, false);
        }
        for step in 1..=40 {
            advance_labels(&g, &mut finer, &mut refined, 0.5, step as f64 * 0.5, false);
        }
        assert_ne!(f.plate_id, old);
        assert_eq!(f.plate_id, finer.plate_id);
        assert_eq!(
            state.lifecycle.markers.len(),
            refined.lifecycle.markers.len()
        );
    }
}
