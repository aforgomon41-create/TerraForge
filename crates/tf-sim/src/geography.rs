//! Geographic objects derived from the terrain graph, independent of saved map crops.
//! Land connectivity is not a political/cultural definition of continents. Regions
//! are drainage subdivisions; neighbouring tiny coastal basins may be joined.
use crate::{erosion::ErosionState, WorldFields};
use glam::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tf_grid::GridOps;

pub const VERSION: u32 = 1;
const MAX_FEATURES: usize = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Feature {
    pub id: u32,
    pub parent: Option<u32>,
    pub level: String,
    pub kind: String,
    pub name: String,
    pub anchor_cell: u32,
    pub lat: f64,
    pub lon: f64,
    pub area_m2: f64,
    pub cell_count: u32,
    pub radius_m: f64,
    pub partial: bool,
    pub elevation_m: f32,
    pub prominence_m: Option<f32>,
    pub drainage_area_m2: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Geography {
    pub version: u32,
    pub cell_count: usize,
    pub resolution_m: f64,
    pub features: Vec<Feature>,
    pub landmass: Vec<u32>,
    pub region: Vec<u32>,
    pub site: Vec<u32>,
    pub omitted_sites: usize,
}

impl Geography {
    pub fn validate(&self) -> Result<(), String> {
        let n = self.cell_count;
        if self.version != VERSION
            || n == 0
            || self.features.len() > MAX_FEATURES
            || !self.resolution_m.is_finite()
            || self.resolution_m <= 0.
            || [&self.landmass, &self.region, &self.site]
                .iter()
                .any(|v| v.len() != n)
        {
            return Err("地理对象版本或尺寸无效".into());
        }
        for (i, f) in self.features.iter().enumerate() {
            if f.id as usize != i + 1
                || f.anchor_cell as usize >= n
                || f.parent.is_some_and(|p| p == 0 || p >= f.id)
                || ![f.lat, f.lon, f.area_m2, f.radius_m]
                    .iter()
                    .all(|v| v.is_finite())
                || !f.elevation_m.is_finite()
                || f.area_m2 <= 0.
                || f.cell_count == 0
            {
                return Err("地理对象属性无效".into());
            }
        }
        for (labels, level) in [
            (&self.landmass, "continent"),
            (&self.region, "region"),
            (&self.site, "site"),
        ] {
            for &id in labels {
                if id != 0
                    && self
                        .features
                        .get(id as usize - 1)
                        .is_none_or(|f| f.level != level)
                {
                    return Err("地理对象归属无效".into());
                }
            }
        }
        Ok(())
    }
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}
fn position<G: GridOps>(grid: &G, i: usize) -> DVec3 {
    grid.local_center_m(i).map_or_else(
        || grid.center_dir(i).as_vec(),
        |(x, y)| DVec3::new(x, y, 0.),
    )
}
fn add(
    features: &mut Vec<Feature>,
    level: &str,
    kind: &str,
    parent: Option<u32>,
    cell: usize,
    prominence: Option<f32>,
    drainage: Option<f64>,
) -> Result<u32, String> {
    if features.len() >= MAX_FEATURES {
        return Err("地理对象过多，请降低精度或调整地形".into());
    }
    let id = features.len() as u32 + 1;
    let label = match kind {
        "continent" => "大陆",
        "island" => "岛屿",
        "land_fragment" => "陆地片区",
        "drainage_region" => "汇水区",
        "peak" => "山峰",
        "pass" => "山口",
        "lake_basin" => "湖盆",
        "river_mouth" => "河口",
        _ => "边界出水口",
    };
    features.push(Feature {
        id,
        parent,
        level: level.into(),
        kind: kind.into(),
        name: format!("{label} {id}"),
        anchor_cell: cell as u32,
        lat: 0.,
        lon: 0.,
        area_m2: 0.,
        cell_count: 0,
        radius_m: 0.,
        partial: false,
        elevation_m: 0.,
        prominence_m: prominence,
        drainage_area_m2: drainage,
    });
    Ok(id)
}

/// Detection only reads the final physical state. Cancellation is checked between
/// graph passes and during large traversals. Ocean is label zero at every level.
pub fn recognize<G: GridOps>(
    grid: &G,
    fields: &WorldFields,
    water: &ErosionState,
    cancel: &dyn Fn() -> bool,
) -> Result<Geography, String> {
    let n = grid.cell_count();
    let h = &fields.elevation;
    if n == 0
        || h.len() != n
        || fields.lake_depth.len() != n
        || water.flow_dir.len() != n
        || water.topo_order.len() != n
        || !h.iter().chain(&fields.lake_depth).all(|v| v.is_finite())
    {
        return Err("识别所需地形或水文尺寸无效".into());
    }
    let check = || {
        if cancel() {
            Err("地理识别已取消".to_string())
        } else {
            Ok(())
        }
    };
    check()?;
    let mut ranks = vec![usize::MAX; n];
    for (rank, &cell) in water.topo_order.iter().enumerate() {
        if cell as usize >= n || ranks[cell as usize] != usize::MAX {
            return Err("流向次序无效".into());
        }
        ranks[cell as usize] = rank;
    }
    for (i, &j) in water.flow_dir.iter().enumerate() {
        if j != u32::MAX
            && (j as usize >= n || ranks[j as usize] <= ranks[i] || !grid.neighbors(i).contains(&j))
        {
            return Err("流向不满足下游关系".into());
        }
    }
    drop(ranks);
    let mean_area = (0..n).map(|i| grid.area_m2(i)).sum::<f64>() / n as f64;
    let spacing = mean_area.sqrt();
    let mut out = Geography {
        version: VERSION,
        cell_count: n,
        resolution_m: spacing,
        features: Vec::new(),
        landmass: vec![0; n],
        region: vec![0; n],
        site: vec![0; n],
        omitted_sites: 0,
    };
    let boundary = grid.boundary_mask();
    let mut components: Vec<(Vec<usize>, f64, bool)> = Vec::new();
    let mut seen = vec![false; n];
    for i in 0..n {
        if i % 8192 == 0 {
            check()?;
        }
        if seen[i] || h[i] <= 0. {
            continue;
        }
        let mut cells = vec![i];
        seen[i] = true;
        let (mut area, mut partial, mut cursor) = (0., false, 0);
        while cursor < cells.len() {
            if cursor % 8192 == 0 {
                check()?;
            }
            let c = cells[cursor];
            cursor += 1;
            area += grid.area_m2(c);
            partial |= boundary[c];
            for &j in grid.neighbors(c) {
                let j = j as usize;
                if !seen[j] && h[j] > 0. {
                    seen[j] = true;
                    cells.push(j);
                }
            }
        }
        components.push((cells, area, partial));
    }
    components.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0[0].cmp(&b.0[0])));
    let total_land: f64 = components.iter().map(|v| v.1).sum();
    let continent_min = (1e12 * (grid.radius_m() / 6_371_000.).powi(2)).max(total_land * 0.04);
    let mut land_areas = vec![0.];
    let mut land_base = vec![0.];
    for (cells, area, partial) in &components {
        let kind = if *partial {
            "land_fragment"
        } else if *area >= continent_min {
            "continent"
        } else {
            "island"
        };
        let id = add(
            &mut out.features,
            "continent",
            kind,
            None,
            cells[0],
            None,
            None,
        )?;
        land_areas.push(*area);
        land_base.push(if *partial {
            cells.iter().map(|&i| h[i]).fold(f32::INFINITY, f32::min)
        } else {
            0.
        });
        for &i in cells {
            out.landmass[i] = id;
        }
    }
    drop(components);
    drop(seen);
    check()?;

    // Split the receiver forest at a scale relative to each landmass, then merge
    // undersized adjacent coastal catchments across the lowest connecting saddle.
    let targets: Vec<f64> = land_areas
        .iter()
        .map(|a| (a / 16.).max(mean_area * 128.))
        .collect();
    let mut residual: Vec<f64> = (0..n)
        .map(|i| if h[i] > 0. { grid.area_m2(i) } else { 0. })
        .collect();
    let mut cut = vec![false; n];
    for &c in &water.topo_order {
        let i = c as usize;
        if h[i] <= 0. {
            continue;
        }
        let d = water.flow_dir[i] as usize;
        cut[i] = d >= n || h[d] <= 0. || residual[i] >= targets[out.landmass[i] as usize];
        if !cut[i] {
            residual[d] += residual[i];
        }
    }
    let mut labels = vec![usize::MAX; n];
    let mut areas = Vec::new();
    for &c in water.topo_order.iter().rev() {
        let i = c as usize;
        if h[i] <= 0. {
            continue;
        }
        let label = if cut[i] {
            areas.push(0.);
            areas.len() - 1
        } else {
            labels[water.flow_dir[i] as usize]
        };
        labels[i] = label;
        areas[label] += grid.area_m2(i);
    }
    drop(residual);
    drop(cut);
    let mut parents: Vec<usize> = (0..areas.len()).collect();
    for _ in 0..4 {
        check()?;
        let mut best = vec![None::<(f32, usize, f64)>; areas.len()];
        for i in 0..n {
            if h[i] <= 0. {
                continue;
            }
            let a = root(&mut parents, labels[i]);
            if areas[a] >= targets[out.landmass[i] as usize] * 0.12 {
                continue;
            }
            for &j in grid.neighbors(i) {
                let j = j as usize;
                if h[j] <= 0. {
                    continue;
                }
                let b = root(&mut parents, labels[j]);
                if a == b {
                    continue;
                }
                let cost = h[i].max(h[j]);
                if best[a].is_none_or(|(v, k, _)| cost < v || (cost == v && b < k)) {
                    best[a] = Some((cost, b, targets[out.landmass[i] as usize] * 0.12));
                }
            }
        }
        let mut changed = false;
        for (i, edge) in best.iter().enumerate() {
            if let Some((_, j, limit)) = edge {
                let a = root(&mut parents, i);
                let b = root(&mut parents, *j);
                if areas[a] >= *limit {
                    continue;
                }
                if a != b {
                    let (a, b) = if areas[a] < areas[b] || (areas[a] == areas[b] && a > b) {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    parents[a] = b;
                    areas[b] += areas[a];
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut region_ids = BTreeMap::new();
    for i in 0..n {
        if h[i] <= 0. {
            continue;
        }
        let r = root(&mut parents, labels[i]);
        let id = if let Some(&id) = region_ids.get(&r) {
            id
        } else {
            let id = add(
                &mut out.features,
                "region",
                "drainage_region",
                Some(out.landmass[i]),
                i,
                None,
                None,
            )?;
            region_ids.insert(r, id);
            id
        };
        out.region[i] = id;
    }
    drop(labels);
    drop(parents);
    drop(areas);
    check()?;

    // Topographic prominence: descending superlevel sets merge at key saddles.
    // Suppress resolution-scale bumps; no peak is inferred from absolute height alone.
    let prominence_min = (150. * (spacing / 1000.).sqrt()).clamp(10., 450.) as f32;
    let mut order: Vec<usize> = (0..n).filter(|&i| h[i] > 0.).collect();
    order.sort_unstable_by(|&a, &b| h[b].total_cmp(&h[a]).then(a.cmp(&b)));
    let mut parents = vec![usize::MAX; n];
    let peaks: Vec<usize> = (0..n).collect();
    let mut candidates: Vec<(&str, usize, f32, Option<f64>)> = Vec::new();
    for (step, &i) in order.iter().enumerate() {
        if step % 8192 == 0 {
            check()?;
        }
        parents[i] = i;
        for &j in grid.neighbors(i) {
            let j = j as usize;
            if parents[j] == usize::MAX {
                continue;
            }
            let a = root(&mut parents, i);
            let b = root(&mut parents, j);
            if a == b {
                continue;
            }
            let (a, b) = if h[peaks[a]] > h[peaks[b]]
                || (h[peaks[a]] == h[peaks[b]] && peaks[a] < peaks[b])
            {
                (a, b)
            } else {
                (b, a)
            };
            let prominence = h[peaks[b]] - h[i];
            if prominence >= prominence_min {
                candidates.push(("peak", peaks[b], prominence, None));
                if h[i] > prominence_min && h[peaks[a]] - h[i] >= prominence_min {
                    candidates.push(("pass", i, prominence, None));
                }
            }
            parents[b] = a;
        }
    }
    for &i in &order {
        let prominence = h[peaks[i]] - land_base[out.landmass[i] as usize];
        if parents[i] == i && prominence >= prominence_min {
            candidates.push(("peak", peaks[i], prominence, None));
        }
    }
    drop(order);
    drop(parents);
    drop(peaks);
    // Catchment area is not discharge: require rainfall before calling it a river.
    let mut drainage: Vec<f64> = (0..n)
        .map(|i| if h[i] > 0. { grid.area_m2(i) } else { 0. })
        .collect();
    let mut rain_area: Vec<f64> = (0..n)
        .map(|i| drainage[i] * fields.precipitation.get(i).copied().unwrap_or(0.) as f64)
        .collect();
    for &c in &water.topo_order {
        let i = c as usize;
        let d = water.flow_dir[i] as usize;
        if h[i] > 0. && d < n && h[d] > 0. {
            drainage[d] += drainage[i];
            rain_area[d] += rain_area[i];
        }
    }
    for i in 0..n {
        if h[i] <= 0. {
            continue;
        }
        let d = water.flow_dir[i] as usize;
        if (d >= n || h[d] <= 0.)
            && drainage[i] >= (25. * mean_area).max(total_land / 4000.)
            && rain_area[i] / drainage[i] >= 80.
        {
            candidates.push((
                if boundary[i] || d >= n {
                    "map_outlet"
                } else {
                    "river_mouth"
                },
                i,
                drainage[i].sqrt() as f32,
                Some(drainage[i]),
            ));
        }
    }
    drop(drainage);
    drop(rain_area);
    candidates.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.1.cmp(&b.1)).then(a.0.cmp(b.0)));
    let mut chosen: BTreeMap<(u32, &str), Vec<usize>> = BTreeMap::new();
    for (kind, i, score, drainage) in candidates {
        if (kind == "peak" || kind == "pass") && boundary[i] {
            out.omitted_sites += 1;
            continue;
        }
        let group = chosen.entry((out.region[i], kind)).or_default();
        if group.len() >= 16
            || group.iter().any(|&j| grid.distance_m(i, j) < 3. * spacing)
            || out.site[i] != 0
        {
            out.omitted_sites += 1;
            continue;
        }
        group.push(i);
        let id = add(
            &mut out.features,
            "site",
            kind,
            Some(out.region[i]),
            i,
            if kind == "peak" || kind == "pass" {
                Some(score)
            } else {
                None
            },
            drainage,
        )?;
        out.site[i] = id;
    }
    check()?;
    // Filled depressions are candidate lake basins, not proven permanent lakes.
    let mut seen = vec![false; n];
    for i in 0..n {
        if seen[i] || h[i] <= 0. || fields.lake_depth[i] < 1. {
            continue;
        }
        let mut cells = vec![i];
        seen[i] = true;
        let mut cursor = 0;
        while cursor < cells.len() {
            let c = cells[cursor];
            cursor += 1;
            for &j in grid.neighbors(c) {
                let j = j as usize;
                if !seen[j] && h[j] > 0. && fields.lake_depth[j] >= 1. {
                    seen[j] = true;
                    cells.push(j);
                }
            }
        }
        let anchor = *cells
            .iter()
            .max_by(|&&a, &&b| {
                fields.lake_depth[a]
                    .total_cmp(&fields.lake_depth[b])
                    .then(b.cmp(&a))
            })
            .unwrap();
        let id = add(
            &mut out.features,
            "site",
            "lake_basin",
            Some(out.region[anchor]),
            anchor,
            None,
            None,
        )?;
        // Avoid displacing an already identified point; every basin retains its anchor.
        for c in cells {
            if out.site[c] == 0 || c == anchor {
                out.site[c] = id;
            }
        }
    }
    check()?;
    // Centres are chosen inside each object; spherical means never average longitudes.
    let count = out.features.len();
    let mut sums = vec![DVec3::ZERO; count];
    for labels in [&out.landmass, &out.region, &out.site] {
        for (i, &id) in labels.iter().enumerate() {
            if id == 0 {
                continue;
            }
            let k = id as usize - 1;
            let a = grid.area_m2(i);
            let f = &mut out.features[k];
            f.area_m2 += a;
            f.cell_count += 1;
            f.partial |= boundary[i];
            sums[k] += position(grid, i) * a;
        }
    }
    let mut nearest = vec![f64::INFINITY; count];
    for labels in [&out.landmass, &out.region] {
        for (i, &id) in labels.iter().enumerate() {
            if id == 0 {
                continue;
            }
            let k = id as usize - 1;
            let center = if grid.extent_m().is_some() {
                sums[k] / out.features[k].area_m2
            } else {
                sums[k].normalize_or_zero()
            };
            let d = position(grid, i).distance_squared(center);
            if d < nearest[k] {
                nearest[k] = d;
                out.features[k].anchor_cell = i as u32;
            }
        }
    }
    for labels in [&out.landmass, &out.region, &out.site] {
        for (i, &id) in labels.iter().enumerate() {
            if id != 0 {
                let f = &mut out.features[id as usize - 1];
                f.radius_m = f.radius_m.max(grid.distance_m(f.anchor_cell as usize, i));
            }
        }
    }
    for f in &mut out.features {
        let i = f.anchor_cell as usize;
        // Point overlap can remove its only label; the catalogue still has an exact point.
        if f.cell_count == 0 {
            f.cell_count = 1;
            f.area_m2 = grid.area_m2(i);
        }
        (f.lat, f.lon) = grid.center_dir(i).to_lat_lon_deg();
        f.elevation_m = h[i];
        f.radius_m = f.radius_m.max(spacing * 0.5);
    }
    out.validate()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitvec::prelude::Lsb0;
    use tf_grid::GridDescriptor;
    fn detect(grid: &impl GridOps, h: Vec<f32>) -> Geography {
        let original = h.clone();
        let mut f = WorldFields {
            elevation: h,
            lake_depth: vec![0.; grid.cell_count()],
            flow_accumulation: vec![0.; grid.cell_count()],
            precipitation: vec![600.; grid.cell_count()],
            ..Default::default()
        };
        let water = crate::erosion::refresh_hydrology(grid, &mut f);
        let out = recognize(grid, &f, &water, &|| false).unwrap();
        assert_eq!(f.elevation, original);
        out
    }
    #[test]
    fn ocean_and_flat_land_have_no_fabricated_peaks() {
        let g = GridDescriptor::plane(0., 0., 20_000., 20, 20, 6_371_000.).build();
        let sea = detect(&g, vec![-10.; g.cell_count()]);
        assert!(sea.features.is_empty());
        let land = detect(&g, vec![5.; g.cell_count()]);
        assert_eq!(
            land.features
                .iter()
                .filter(|f| f.level == "continent")
                .count(),
            1
        );
        assert!(land.features.iter().all(|f| f.kind != "peak"));
        assert_eq!(land.features[0].kind, "land_fragment");
        assert!(land.landmass.iter().all(|&v| v == 1));
        assert!(land.region.iter().all(|&v| v > 1));
    }
    #[test]
    fn disconnected_islands_and_drainage_are_deterministic() {
        let g = GridDescriptor::plane(0., 0., 100_000., 40, 40, 6_371_000.).build();
        let h: Vec<_> = (0..g.cell_count())
            .map(|i| {
                let (x, y) = g.local_center_m(i).unwrap();
                let a = (x + 22_000.).hypot(y);
                let b = (x - 22_000.).hypot(y);
                if a.min(b) < 14_000. {
                    1000. - a.min(b) as f32 / 18.
                } else {
                    -10.
                }
            })
            .collect();
        let out = detect(&g, h.clone());
        assert_eq!(out, detect(&g, h));
        assert_eq!(
            out.features
                .iter()
                .filter(|f| f.level == "continent")
                .count(),
            2
        );
        assert!(out.features.iter().filter(|f| f.kind == "peak").count() >= 2);
        for i in 0..g.cell_count() {
            if out.landmass[i] > 0 {
                let region = &out.features[out.region[i] as usize - 1];
                assert_eq!(region.parent, Some(out.landmass[i]));
            } else {
                assert_eq!(out.region[i], 0);
                assert_eq!(out.site[i], 0);
            }
        }
    }
    #[test]
    fn globe_seam_does_not_split_connected_land() {
        let g = GridDescriptor::sphere(8, 6_371_000.).build();
        let h = (0..g.cell_count())
            .map(|i| {
                let (lat, lon) = g.center_dir(i).to_lat_lon_deg();
                if lon.abs() > 150. && lat.abs() < 40. {
                    100.
                } else {
                    -10.
                }
            })
            .collect();
        let out = detect(&g, h);
        assert_eq!(
            out.features
                .iter()
                .filter(|f| f.level == "continent")
                .count(),
            1
        );
        assert!(out.features[0].lon.abs() > 150.);
        assert!(!out.features[0].partial);
    }
    #[test]
    fn cancellation_and_invalid_flow_are_rejected() {
        let g = GridDescriptor::plane(0., 0., 10_000., 10, 10, 6_371_000.).build();
        let mut f = WorldFields {
            elevation: vec![100.; g.cell_count()],
            lake_depth: vec![0.; g.cell_count()],
            flow_accumulation: vec![0.; g.cell_count()],
            ..Default::default()
        };
        let mut water = crate::erosion::refresh_hydrology(&g, &mut f);
        assert!(recognize(&g, &f, &water, &|| true).is_err());
        water.flow_dir[0] = 0;
        assert!(recognize(&g, &f, &water, &|| false).is_err());
    }

    #[test]
    fn prominence_uses_the_actual_connecting_saddle_and_not_sea_level() {
        struct Ridge {
            boundary: bitvec::vec::BitVec,
        }
        impl GridOps for Ridge {
            fn cell_count(&self) -> usize {
                5
            }
            fn neighbors(&self, i: usize) -> &[u32] {
                match i {
                    0 => &[1],
                    1 => &[0, 2],
                    2 => &[1, 3],
                    3 => &[2, 4],
                    _ => &[3],
                }
            }
            fn distance_m(&self, a: usize, b: usize) -> f64 {
                a.abs_diff(b) as f64 * 1000.
            }
            fn area_m2(&self, _: usize) -> f64 {
                10000.
            }
            fn center_dir(&self, i: usize) -> tf_core::geo::UnitVec3 {
                tf_core::geo::UnitVec3::from_lat_lon_deg(0., i as f64 * 0.01)
            }
            fn latitude_deg(&self, _: usize) -> f64 {
                0.
            }
            fn boundary_mask(&self) -> &bitvec::vec::BitVec {
                &self.boundary
            }
        }
        let grid = Ridge {
            boundary: bitvec::bitvec![1, 0, 0, 0, 1],
        };
        let out = detect(&grid, vec![100., 500., 250., 400., 100.]);
        let peak = out
            .features
            .iter()
            .find(|f| f.kind == "peak" && f.anchor_cell == 3)
            .unwrap();
        assert_eq!(peak.prominence_m, Some(150.));
        assert!(out
            .features
            .iter()
            .any(|f| f.kind == "pass" && f.anchor_cell == 2));
        assert!(!detect(&grid, vec![1000.; 5])
            .features
            .iter()
            .any(|f| f.kind == "peak"));
    }
}
