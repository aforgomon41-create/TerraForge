//! Diagnostic cryosphere forced by the saved monthly climate, not latitude paint.
//! PDD snow/melt, conservative shallow-ice transport, hydrostatic shelves,
//! Stefan sea ice and an equilibrium TTOP/Stefan permafrost approximation.
//! Equations/references and approximation boundaries: docs/cryosphere.md.
#[cfg(test)]
use crate::seasonal::ClimateAtlas;
use crate::WorldFields;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tf_grid::{GridDescriptor, GridOps, MapGrid};

const DAYS: f64 = 365.2425;
const MONTH: f64 = DAYS / 12.;
const YEAR: f64 = DAYS * 86400.;
const RHO_I: f64 = 917.;
const RHO_O: f64 = 1028.;
mod ice_sheet;
pub const LEGACY_MODEL: &str = "seasonal-cryosphere-pdd-sia-stefan-ttop-v1";
pub const MODEL: &str = "map-grid-cryosphere-pdd-sia-stefan-ttop-v2";
/// Legacy v1 atlas limit; v2 stores ice in the map's existing channels.
pub const MAX_CELLS: usize = 12000;
pub const MAX_TRANSPORT_STEPS: usize = 2_000_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IceBudget {
    pub snowfall_m3: f64,
    pub surface_melt_m3: f64,
    pub basal_melt_m3: f64,
    pub calved_m3: f64,
    pub boundary_export_m3: f64,
    pub final_storage_m3: f64,
    pub rounding_m3: f64,
    pub residual_m3: f64,
    #[serde(default)]
    pub solver_cells: usize,
    #[serde(default)]
    pub peak_active_cells: usize,
    #[serde(default)]
    pub transport_steps: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryosphereAtlas {
    pub model: String,
    /// v1: ice solver grid. v2: monthly climate provenance only; actual ice
    /// uses the map node geometry and persisted LandIce/IceShelf channels.
    pub grid: GridDescriptor,
    pub spinup_years: u32,
    pub gravity_m_s2: f64,
    pub grounded_ice_m: Vec<f32>,
    pub shelf_ice_m: Vec<f32>,
    pub sea_ice_m: Vec<Vec<f32>>,
    pub ice_budget: IceBudget,
}

#[derive(Debug, Clone)]
pub struct Cryosphere {
    pub atlas: CryosphereAtlas,
    pub land_ice_m: Vec<f32>,
    /// Minimum monthly snow cover for month=0, selected cover otherwise.
    pub snow_cover: Vec<f32>,
    pub sea_ice_cover: Vec<f32>,
    pub shelf_ice_m: Vec<f32>,
    pub permafrost: Vec<f32>,
    pub active_layer_m: Vec<f32>,
    /// Rain plus snow/ice melt, before terrestrial evaporation; mm/year.
    pub liquid_water_mm: Vec<f32>,
    pub pet_mm_year: Vec<f32>,
    pub lake_ice_cover: Vec<f32>,
}

#[derive(Default)]
struct SnowCycle {
    snow: [f64; 12],
    liquid: [f64; 12],
    retained: f64,
    melted_ice: f64,
}

/// Three climatological cycles resolve winter carry-over in either hemisphere.
/// The last-year ledger P - liquid = solid storage change is exact.
fn snow_cycle(t: &[f32; 12], p: &[f32; 12], ice_m: f64) -> SnowCycle {
    let mut out = SnowCycle::default();
    let mut snow = 0.;
    for year in 0..3 {
        let start = snow;
        let mut ice_left = ice_m * RHO_I;
        for m in 0..12 {
            let temperature = t[m] as f64;
            let prec = (p[m] as f64).max(0.);
            let solid = prec * ((2. - temperature) / 2.).clamp(0., 1.);
            snow += solid;
            let pdd = temperature.max(0.) * MONTH;
            let melt_snow = snow.min(pdd * 3.);
            snow -= melt_snow;
            let melt_ice = ice_left.min((pdd - melt_snow / 3.).max(0.) * 8.);
            ice_left -= melt_ice;
            if year == 2 {
                out.snow[m] = snow;
                out.liquid[m] = prec - solid + melt_snow + melt_ice;
                out.melted_ice += melt_ice;
            }
        }
        if year == 2 {
            out.retained = snow - start;
        }
    }
    out
}

fn freezing_cycle(t: &[f32; 12], freezing: f64) -> [f32; 12] {
    if t.iter().all(|&v| v as f64 >= freezing) {
        return [0.; 12];
    }
    let mut h = 0.;
    let mut result = [0.; 12];
    // Thermal freezing, surface/basal melting and mechanical export. Export
    // prevents an unbounded immobile slab under a permanently cold climate.
    for cycle in 0..20 {
        for m in 0..12 {
            let cold = (freezing - t[m] as f64).max(0.);
            h = ((h + 0.1_f64).powi(2) + 2. * 2.2 / (RHO_I * 334000.) * cold * MONTH * 86400.)
                .sqrt()
                - 0.1;
            h = (h
                - (t[m] as f64 - freezing).max(0.) * 0.008 * MONTH
                - 2. * MONTH * 86400. / (RHO_I * 334000.))
                .max(0.);
            h *= (-MONTH / (4. * DAYS)).exp();
            if cycle == 19 {
                result[m] = h as f32;
            }
        }
    }
    result
}

fn cover(swe_mm: f64) -> f32 {
    (swe_mm / (swe_mm + 15.)).clamp(0., 1.) as f32
}
fn ice_cover(h: f64) -> f32 {
    (h / (h + 0.08)).clamp(0., 1.) as f32
}

fn weights(grid: &MapGrid, dir: tf_core::geo::UnitVec3) -> [(usize, f64); 3] {
    let position = match grid {
        MapGrid::Plane(g) => {
            let (x, y) = g.project(dir);
            glam::DVec3::new(x, y, 0.)
        }
        _ => dir.as_vec() * grid.radius_m(),
    };
    crate::evolution::stencil(grid, position).unwrap_or_else(|| {
        let i = grid.cell_at(dir).unwrap_or(0);
        [(i, 1.), (i, 0.), (i, 0.)]
    })
}

/// A finite-volume upwind SIA flux; floating ice has a separate extensional
/// spreading approximation. Every transfer subtracts and adds the same volume.
#[cfg(test)]
fn transport<G: GridOps>(
    grid: &G,
    bed: &[f64],
    valid: &[bool],
    h: &mut [f64],
    dt: f64,
    gravity: f64,
) {
    let n = h.len();
    let surface: Vec<_> = (0..n)
        .map(|i| {
            if bed[i] < 0. && RHO_I * h[i] < RHO_O * (-bed[i]) {
                (1. - RHO_I / RHO_O) * h[i]
            } else {
                bed[i] + h[i]
            }
        })
        .collect();
    let mut transfers = Vec::with_capacity(n * 3);
    let mut outgoing = vec![0.; n];
    for i in 0..n {
        if !valid[i] {
            continue;
        }
        for &nb in grid.neighbors(i) {
            let j = nb as usize;
            if j <= i || !valid[j] {
                continue;
            }
            let (a, b) = if surface[i] > surface[j] {
                (i, j)
            } else {
                (j, i)
            };
            if h[a] < 0.01 {
                continue;
            }
            let distance = grid.distance_m(a, b).max(1.);
            let slope = (surface[a] - surface[b]) / distance;
            // A regular Voronoi cell is split into degree triangles of height d/2:
            // A = degree * face_width * d / 4 (approximate on irregular cells).
            let width = 4. * grid.area_m2(a) / (grid.neighbors(a).len() as f64 * distance);
            let floating = bed[a] < 0. && RHO_I * h[a] < RHO_O * (-bed[a]);
            let flux = if floating {
                let stress = RHO_I * gravity * (1. - RHO_I / RHO_O) * h[a] / 4.;
                let opening = ((surface[a] - surface[b]) / ((1. - RHO_I / RHO_O) * h[a]).max(0.1))
                    .clamp(0., 1.);
                2.4e-24 * stress.powi(3) * distance * h[a] * width * dt * YEAR * opening
            } else {
                let mean = (h[a] + h[b]) * 0.5;
                let diffusivity =
                    2. * 2.4e-24 / 5. * (RHO_I * gravity).powi(3) * mean.powi(5) * slope.powi(2);
                diffusivity * slope * width * dt * YEAR
            };
            if flux > 0. {
                transfers.push((a, b, flux));
                outgoing[a] += flux;
            }
        }
    }
    let mut delta = vec![0.; n];
    for (a, b, flux) in transfers {
        let volume = flux * (0.2 * h[a] * grid.area_m2(a) / outgoing[a].max(1e-30)).min(1.);
        delta[a] -= volume;
        delta[b] += volume;
    }
    for i in 0..n {
        h[i] = (h[i] + delta[i] / grid.area_m2(i)).max(0.);
    }
}

pub fn solve<G: GridOps>(
    grid: &G,
    fields: &WorldFields,
    gravity: f64,
    cancel: &dyn Fn() -> bool,
) -> Option<Cryosphere> {
    ice_sheet::solve(grid, fields, gravity, cancel)
}

/// Fine monthly weather is retained: sub-grid mountains and cold coastal seas
/// are not forced to inherit the coarse cell's land/sea classification.
pub fn sample<G: GridOps>(
    grid: &G,
    fields: &WorldFields,
    atlas: CryosphereAtlas,
    month: u8,
    thickness: Option<(&[f32], &[f32])>,
) -> Option<Cryosphere> {
    if month > 12
        || (atlas.model == MODEL
            && (thickness.is_none() || atlas.ice_budget.solver_cells != grid.cell_count()))
        || thickness
            .is_some_and(|(a, b)| a.len() != grid.cell_count() || b.len() != grid.cell_count())
        || thickness.is_some_and(|(a, b)| a.iter().chain(b).any(|v| !v.is_finite() || *v < 0.))
    {
        return None;
    }
    let source = thickness.is_none().then(|| atlas.grid.build());
    let climate_grid = fields.climate.as_ref().map(|a| a.grid.build());
    let values: Vec<[f32; 9]> = (0..grid.cell_count())
        .into_par_iter()
        .map(|i| {
            let dir = grid.center_dir(i);
            let land = fields.elevation[i] > 0.;
            let stencil = source.as_ref().map(|g| weights(g, dir));
            let at = |v: &[f32]| {
                stencil
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|&(j, w)| v[j] as f64 * w)
                    .sum::<f64>()
            };
            let mut ice = if land || thickness.is_some() {
                thickness
                    .map_or_else(|| at(&atlas.grounded_ice_m), |(a, _)| a[i] as f64)
                    .max(0.)
            } else {
                0.
            };
            let shelf = if !land {
                thickness
                    .map_or_else(|| at(&atlas.shelf_ice_m), |(_, b)| b[i] as f64)
                    .max(0.)
            } else {
                0.
            };
            let (mut t, p, _) = if let (Some(a), Some(g)) = (&fields.climate, &climate_grid) {
                a.sample(g, dir, fields.temperature[i], fields.precipitation[i])
            } else {
                (
                    [fields.temperature[i]; 12],
                    [fields.precipitation[i] / 12.; 12],
                    [0.; 12],
                )
            };
            if land || (thickness.is_some() && (ice > 0. || shelf > 0.)) {
                let rise = if land {
                    ice
                } else if ice > 0. {
                    (fields.elevation[i] as f64 + ice).max(0.)
                } else {
                    (1. - RHO_I / RHO_O) * shelf
                };
                for v in &mut t {
                    *v -= (rise * 0.0065) as f32;
                }
            }
            let cycle = if land {
                snow_cycle(&t, &p, ice)
            } else {
                SnowCycle::default()
            };
            let pet = crate::scientific::monthly_pet(
                &t,
                dir.to_lat_lon_deg().0.to_radians(),
                fields
                    .climate
                    .as_ref()
                    .map_or(23.44, |a| a.axial_tilt_deg)
                    .to_radians(),
            );
            let snow = if month == 0 {
                cycle.snow.iter().copied().fold(f64::INFINITY, f64::min)
            } else {
                cycle.snow[month as usize - 1]
            };
            // Perennial snow itself remains visible before a coarse glacier grows.
            let snow_cover = if land { cover(snow) } else { 0. };
            if !land && thickness.is_none() {
                ice = 0.;
            }
            let sea_cover = if land {
                0.
            } else {
                // Resolve the actual fine coast. Average concentrations, never
                // convert annual mean thickness into a falsely perennial cover.
                let local = freezing_cycle(&t, -1.8);
                if month == 0 {
                    local.iter().map(|&v| ice_cover(v as f64)).sum::<f32>() / 12.
                } else {
                    ice_cover(local[month as usize - 1] as f64)
                }
            };
            let (mut freeze, mut thaw) = (0., 0.);
            for m in 0..12 {
                let insulating = 1. / (1. + cycle.snow[m] / 300.);
                freeze += (-(t[m] as f64)).max(0.) * MONTH * insulating;
                thaw += (t[m] as f64).max(0.) * MONTH;
            }
            let ground = (0.7 * 0.8 * thaw - freeze) / DAYS;
            let permafrost = if land && ice < 1. {
                (-ground / 2.).clamp(0., 1.) as f32
            } else {
                0.
            };
            let active = if permafrost > 0. {
                (2. * 1.2 * 0.8 * thaw * 86400. / (0.35 * 1000. * 334000.)).sqrt() as f32
            } else {
                0.
            };
            let lake_ice = if land && fields.lake_depth[i] > 0.1 {
                let monthly = freezing_cycle(&t, 0.);
                if month == 0 {
                    monthly.iter().map(|&h| ice_cover(h as f64)).sum::<f32>() / 12.
                } else {
                    ice_cover(monthly[month as usize - 1] as f64)
                }
            } else {
                0.
            };
            [
                ice as f32,
                snow_cover,
                sea_cover,
                shelf as f32,
                permafrost,
                active,
                if month == 0 {
                    cycle.liquid.iter().sum::<f64>() as f32
                } else {
                    cycle.liquid[month as usize - 1] as f32 * 12.
                },
                lake_ice,
                if month == 0 {
                    pet.iter().sum()
                } else {
                    pet[month as usize - 1] * 12.
                },
            ]
        })
        .collect();
    let channel = |c| values.iter().map(|v| v[c]).collect();
    Some(Cryosphere {
        land_ice_m: channel(0),
        snow_cover: channel(1),
        sea_ice_cover: channel(2),
        shelf_ice_m: channel(3),
        permafrost: channel(4),
        active_layer_m: channel(5),
        liquid_water_mm: channel(6),
        lake_ice_cover: channel(7),
        pet_mm_year: channel(8),
        atlas,
    })
}

impl CryosphereAtlas {
    pub fn validate(&self) -> Result<(), String> {
        if ![MODEL, LEGACY_MODEL].contains(&self.model.as_str())
            || self.spinup_years > 20000
            || !self.gravity_m_s2.is_finite()
            || self.gravity_m_s2 <= 0.
        {
            return Err("冰雪模型或尺寸无效".into());
        }
        let max_frequency = if self.model == MODEL { 56 } else { 32 };
        match &self.grid {
            GridDescriptor::Sphere {
                frequency,
                radius_m,
                version,
            } if (4..=max_frequency).contains(frequency)
                && radius_m.is_finite()
                && (1e6..=5e7).contains(radius_m)
                && [tf_grid::GRID_VERSION, tf_grid::SPHERE_GRID_VERSION].contains(version) => {}
            GridDescriptor::Plane { .. } | GridDescriptor::Patch { .. } => self.grid.validate()?,
            _ => return Err("冰雪来源网格无效".into()),
        }
        let b = &self.ice_budget;
        if [
            b.snowfall_m3,
            b.surface_melt_m3,
            b.basal_melt_m3,
            b.calved_m3,
            b.boundary_export_m3,
            b.final_storage_m3,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.)
            || !b.residual_m3.is_finite()
            || !b.rounding_m3.is_finite()
        {
            return Err("冰量收支无效".into());
        }
        if self.model == MODEL {
            if self.ice_budget.solver_cells == 0
                || self.ice_budget.solver_cells > 16_777_216
                || self.ice_budget.peak_active_cells > self.ice_budget.solver_cells
                || self.ice_budget.transport_steps > MAX_TRANSPORT_STEPS
                || !self.grounded_ice_m.is_empty()
                || !self.shelf_ice_m.is_empty()
                || !self.sea_ice_m.is_empty()
            {
                return Err("新版冰雪必须使用地图网格上的保存字段".into());
            }
            return Ok(());
        }
        match &self.grid {
            GridDescriptor::Sphere {
                frequency,
                radius_m,
                version,
            } if (4..=32).contains(frequency)
                && radius_m.is_finite()
                && (1e6..=5e7).contains(radius_m)
                && [tf_grid::GRID_VERSION, tf_grid::SPHERE_GRID_VERSION].contains(version) => {}
            GridDescriptor::Plane { .. } | GridDescriptor::Patch { .. } => self.grid.validate()?,
            _ => return Err("冰雪网格无效".into()),
        }
        let n = self.grid.cell_count();
        if n > MAX_CELLS {
            return Err("冰雪网格过大".into());
        }
        if self.sea_ice_m.len() != 12
            || [&self.grounded_ice_m, &self.shelf_ice_m]
                .into_iter()
                .chain(self.sea_ice_m.iter())
                .any(|v| {
                    v.len() != n
                        || v.iter()
                            .any(|&x| !x.is_finite() || !(0. ..=100000.).contains(&x))
                })
        {
            return Err("冰雪字段无效".into());
        }
        Ok(())
    }
}

/// Reconcile water after the solid/liquid split changed final lakes. Do not
/// retain the pre-cryosphere lake mask in lake ice or exposed-ground diagnostics.
pub fn update_water_surfaces<G: GridOps>(grid: &G, fields: &mut WorldFields) {
    let Some(mut ice) = fields.cryosphere.take() else {
        return;
    };
    let source = fields.climate.as_ref().map(|a| a.grid.build());
    for i in 0..grid.cell_count() {
        ice.lake_ice_cover[i] = 0.;
        if fields.elevation[i] <= 0. || fields.lake_depth[i] <= 0.1 {
            continue;
        }
        ice.permafrost[i] = 0.;
        ice.active_layer_m[i] = 0.;
        let mut t = if let (Some(a), Some(g)) = (&fields.climate, &source) {
            a.sample(
                g,
                grid.center_dir(i),
                fields.temperature[i],
                fields.precipitation[i],
            )
            .0
        } else {
            [fields.temperature[i]; 12]
        };
        for v in &mut t {
            *v -= ice.land_ice_m[i] * 0.0065;
        }
        let h = freezing_cycle(&t, 0.);
        ice.lake_ice_cover[i] = h.iter().map(|&h| ice_cover(h as f64)).sum::<f32>() / 12.;
    }
    fields.cryosphere = Some(ice);
}

/// Annual liquid support in m3/s. Contributing area remains a separate channel.
pub fn annual_discharge<G: GridOps>(
    grid: &G,
    fields: &WorldFields,
    water: &crate::erosion::ErosionState,
) -> Vec<f32> {
    let input = fields
        .cryosphere
        .as_ref()
        .map(|c| c.liquid_water_mm.as_slice())
        .unwrap_or(&fields.precipitation);
    let mut flux: Vec<f64> = (0..grid.cell_count())
        .map(|i| {
            if fields.elevation[i] > 0. {
                crate::lakes::annual_runoff_m(
                    input[i] as f64,
                    fields.cryosphere.as_ref().map_or(
                        fields.potential_evaporation.get(i).copied().unwrap_or(0.),
                        |c| c.pet_mm_year[i],
                    ) as f64,
                ) * grid.area_m2(i)
                    / YEAR
            } else {
                0.
            }
        })
        .collect();
    for &cell in &water.topo_order {
        let i = cell as usize;
        let j = water.flow_dir[i];
        if j != u32::MAX {
            flux[j as usize] += flux[i];
        }
    }
    flux.into_iter().map(|v| v as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frozen_snow_is_not_rain_and_summer_melt_is_retained() {
        let p = [50.; 12];
        let cold = snow_cycle(&[-15.; 12], &p, 0.);
        assert_eq!(cold.liquid.iter().sum::<f64>(), 0.);
        assert!((cold.retained - 600.).abs() < 1e-9);
        let mut t = [-15.; 12];
        for m in 4..9 {
            t[m] = 8.;
        }
        let warm = snow_cycle(&t, &p, 0.);
        assert!(warm.liquid[4] > 50.);
        assert!(warm.snow.iter().any(|&v| v == 0.));
        assert!((600. - warm.liquid.iter().sum::<f64>() - warm.retained).abs() < 1e-9);
    }
    #[test]
    fn saline_sea_ice_freezes_seasonally_and_warm_ocean_stays_open() {
        assert!(freezing_cycle(&[10.; 12], -1.8).iter().all(|&h| h == 0.));
        assert!(freezing_cycle(&[-15.; 12], -1.8)
            .iter()
            .all(|&h| h > 0.5 && h < 10.));
        let mut t = [-15.; 12];
        for m in 4..9 {
            t[m] = 8.;
        }
        let h = freezing_cycle(&t, -1.8);
        assert!(h.iter().any(|&h| h > 0.5));
        assert!(h.iter().any(|&h| h == 0.));
    }
    #[test]
    fn upwind_ice_flux_conserves_volume_and_never_creates_negative_ice() {
        let g = tf_grid::FlatGrid::new(12, 8, 120000., 65., 0., 6371000.);
        let n = g.cell_count();
        let mut h = vec![0.; n];
        h[n / 2] = 3000.;
        let mut bed = vec![-200.; n];
        bed[n / 2] = 300.;
        let valid = vec![true; n];
        let before: f64 = (0..n).map(|i| h[i] * g.area_m2(i)).sum();
        for _ in 0..50 {
            transport(&g, &bed, &valid, &mut h, 10., 9.81);
        }
        let after: f64 = (0..n).map(|i| h[i] * g.area_m2(i)).sum();
        assert!((before - after).abs() < before * 1e-12);
        assert!(h.iter().all(|&h| h >= 0.));
        assert!(h.iter().enumerate().any(|(i, &h)| i != n / 2 && h > 0.));
    }
    fn fixture() -> (MapGrid, WorldFields) {
        let descriptor = GridDescriptor::plane(65., 0., 600000., 12, 8, 6371000.);
        let grid = descriptor.build();
        let n = grid.cell_count();
        let mut f = WorldFields::with_capacity(n);
        f.elevation.fill(-800.);
        f.temperature.fill(-8.);
        f.precipitation.fill(600.);
        for row in 2..6 {
            for col in 3..9 {
                let i = row * 12 + col;
                f.elevation[i] = 200.;
                f.temperature[i] = -15.;
            }
        }
        f.climate = Some(ClimateAtlas {
            grid: descriptor,
            model: "fixture".into(),
            axial_tilt_deg: 23.44,
            temperature_c: vec![f.temperature.clone(); 12],
            precipitation_mm: vec![vec![50.; n]; 12],
            pet_mm: vec![vec![0.; n]; 12],
            energy: Default::default(),
            water: Default::default(),
            circulation: None,
        });
        (grid, f)
    }
    #[test]
    fn cold_snow_builds_ice_shelves_with_a_closed_solid_water_budget() {
        let (g, f) = fixture();
        let ice = solve(&g, &f, 9.81, &|| false).unwrap();
        let b = &ice.atlas.ice_budget;
        assert!(b.residual_m3.abs() < b.snowfall_m3 * 1e-9, "{b:?}");
        assert!(ice.land_ice_m.iter().any(|&h| h > 100.));
        assert!(
            ice.shelf_ice_m.iter().any(|&h| h > 0.1),
            "no floating land-ice outflow"
        );
        assert!(ice.sea_ice_cover.iter().any(|&c| c > 0.8));
        assert!(ice
            .liquid_water_mm
            .iter()
            .enumerate()
            .all(|(i, &p)| f.elevation[i] <= 0. || p == 0.));
        ice.atlas.validate().unwrap();
        let saved = (0..g.cell_count())
            .map(|i| (ice.land_ice_m[i] + ice.shelf_ice_m[i]) as f64 * g.area_m2(i))
            .sum::<f64>();
        assert!((saved - b.final_storage_m3 - b.rounding_m3).abs() < b.final_storage_m3 * 1e-6);
        assert_eq!(b.solver_cells, g.cell_count());
        assert!(ice.atlas.grounded_ice_m.is_empty());
        assert!(sample(&g, &f, ice.atlas.clone(), 1, None).is_none());
    }
    #[test]
    fn warm_climate_and_dry_cold_ground_do_not_invent_ice_sheets() {
        let (g, mut f) = fixture();
        f.temperature.fill(15.);
        f.climate
            .as_mut()
            .unwrap()
            .temperature_c
            .fill(vec![15.; g.cell_count()]);
        let ice = solve(&g, &f, 9.81, &|| false).unwrap();
        assert!(ice.land_ice_m.iter().all(|&h| h == 0.));
        assert!(ice.sea_ice_cover.iter().all(|&c| c == 0.));
        assert!(ice.permafrost.iter().all(|&c| c == 0.));
        f.temperature.fill(-15.);
        f.precipitation.fill(0.);
        let a = f.climate.as_mut().unwrap();
        a.temperature_c.fill(vec![-15.; g.cell_count()]);
        a.precipitation_mm.fill(vec![0.; g.cell_count()]);
        let ice = solve(&g, &f, 9.81, &|| false).unwrap();
        assert!(ice.land_ice_m.iter().all(|&h| h == 0.));
        assert!(ice.snow_cover.iter().all(|&h| h == 0.));
        assert!(ice.permafrost.iter().any(|&c| c > 0.9));
    }

    #[test]
    fn fine_snowline_is_not_a_coarse_cell_footprint_and_months_keep_ice_identity() {
        let descriptor = GridDescriptor::plane(35., 0., 1600000., 64, 32, 6371000.);
        let g = descriptor.build();
        let n = g.cell_count();
        let climate_descriptor = GridDescriptor::plane(35., 0., 1600000., 8, 4, 6371000.);
        let mut f = WorldFields::with_capacity(n);
        f.elevation.fill(200.);
        f.temperature.fill(20.);
        f.precipitation.fill(600.);
        let peak = n / 2 + 32;
        f.elevation[peak] = 4000.;
        f.temperature[peak] = -5.;
        let m = climate_descriptor.cell_count();
        f.climate = Some(ClimateAtlas {
            grid: climate_descriptor,
            model: "fixture".into(),
            axial_tilt_deg: 23.44,
            temperature_c: vec![vec![20.; m]; 12],
            precipitation_mm: vec![vec![50.; m]; 12],
            pet_mm: vec![vec![0.; m]; 12],
            energy: Default::default(),
            water: Default::default(),
            circulation: None,
        });
        let ice = solve(&g, &f, 9.81, &|| false).unwrap();
        assert!(ice.land_ice_m[peak] > 1.);
        for i in 0..n {
            if g.distance_m(peak, i) > 200000. {
                assert!(ice.land_ice_m[i] < 1e-3, "remote warm cell {i}");
            }
        }
        for month in [1, 7] {
            let view = sample(
                &g,
                &f,
                ice.atlas.clone(),
                month,
                Some((&ice.land_ice_m, &ice.shelf_ice_m)),
            )
            .unwrap();
            assert_eq!(view.land_ice_m, ice.land_ice_m);
            assert_eq!(view.shelf_ice_m, ice.shelf_ice_m);
        }
        let b = &ice.atlas.ice_budget;
        let stored = (0..n)
            .map(|i| (ice.land_ice_m[i] + ice.shelf_ice_m[i]) as f64 * g.area_m2(i))
            .sum::<f64>();
        assert!((stored - b.final_storage_m3 - b.rounding_m3).abs() < stored * 1e-6);
    }

    #[test]
    fn map_grid_ice_is_not_subject_to_the_legacy_twelve_thousand_cell_limit() {
        let g = GridDescriptor::sphere(40, 6371000.).build();
        let n = g.cell_count();
        let d = GridDescriptor::sphere(4, 6371000.);
        let m = d.cell_count();
        let mut f = WorldFields::with_capacity(n);
        f.elevation.fill(100.);
        f.temperature.fill(20.);
        f.precipitation.fill(500.);
        f.climate = Some(ClimateAtlas {
            grid: d,
            model: "fixture".into(),
            axial_tilt_deg: 23.44,
            temperature_c: vec![vec![20.; m]; 12],
            precipitation_mm: vec![vec![42.; m]; 12],
            pet_mm: vec![vec![0.; m]; 12],
            energy: Default::default(),
            water: Default::default(),
            circulation: None,
        });
        let ice = solve(&g, &f, 9.81, &|| false).unwrap();
        assert!(n > MAX_CELLS);
        assert!(ice.land_ice_m.iter().all(|&h| h == 0.));
        ice.atlas.validate().unwrap();
    }
    #[test]
    fn level_floating_shelves_have_no_index_driven_ice_transport() {
        let g = tf_grid::FlatGrid::new(12, 8, 120000., 65., 0., 6371000.);
        let n = g.cell_count();
        let mut h = vec![100.; n];
        transport(&g, &vec![-500.; n], &vec![true; n], &mut h, 10., 9.81);
        assert!(h.iter().all(|&v| v == 100.));
    }
    #[test]
    fn subzero_precipitation_is_stored_and_thawing_frozen_ground_releases_summer_water() {
        let frozen = snow_cycle(&[-0.1; 12], &[100.; 12], 0.);
        assert!(frozen.liquid.iter().all(|&v| v == 0.));
        let (g, mut f) = fixture();
        f.elevation.fill(100.);
        let mut t = [-15.; 12];
        for m in 4..9 {
            t[m] = 8.;
        }
        f.temperature.fill(t.iter().sum::<f32>() / 12.);
        f.climate.as_mut().unwrap().temperature_c =
            t.iter().map(|&v| vec![v; g.cell_count()]).collect();
        let annual = solve(&g, &f, 9.81, &|| false).unwrap();
        assert!(annual.permafrost.iter().all(|&v| v > 0.5));
        assert!(annual.active_layer_m.iter().all(|&v| v > 0.1));
        let winter = sample(
            &g,
            &f,
            annual.atlas.clone(),
            1,
            Some((&annual.land_ice_m, &annual.shelf_ice_m)),
        )
        .unwrap();
        let summer = sample(
            &g,
            &f,
            annual.atlas.clone(),
            7,
            Some((&annual.land_ice_m, &annual.shelf_ice_m)),
        )
        .unwrap();
        assert!(winter.liquid_water_mm.iter().all(|&v| v == 0.));
        assert!(summer.liquid_water_mm.iter().all(|&v| v > 0.));
        assert!(annual.land_ice_m.iter().all(|&v| v == 0.));
    }
}
