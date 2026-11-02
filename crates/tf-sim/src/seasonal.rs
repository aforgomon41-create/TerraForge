//! Native seasonal reference-level energy balance model adapted from climlab (MIT).
//! Monthly thermal storage, A+B*T longwave loss and symmetric diffusive heat
//! exchange are solved implicitly at a sea-level reference column. Topographic
//! surface temperatures are lapse-rate diagnostics, not a separate closed surface budget.
use crate::scientific::{self, DAY};
use crate::WorldFields;
use glam::DVec3;
use serde::{Deserialize, Serialize};
use tf_core::params::PlanetParams;
use tf_grid::{FlatGrid, GridDescriptor, GridOps, MapGrid, SphereHexGrid};

/// One shared allocation limit for simulation, persistence and untrusted imports.
pub const MAX_ATLAS_CELLS: usize = 32768;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnergyBudget {
    #[serde(default)]
    pub temperature_basis: String,
    pub spinup_years: u32,
    pub annual_cycle_change_c: f64,
    #[serde(default)]
    pub annual_cycle_converged: bool,
    pub mean_absorbed_solar_w_m2: f64,
    pub mean_longwave_w_m2: f64,
    pub mean_storage_w_m2: f64,
    pub mean_boundary_heat_w_m2: f64,
    pub residual_w_m2: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClimateAtlas {
    pub grid: GridDescriptor,
    pub model: String,
    pub axial_tilt_deg: f64,
    /// Twelve equal-duration months; month zero is January.
    pub temperature_c: Vec<Vec<f32>>,
    pub precipitation_mm: Vec<Vec<f32>>,
    pub pet_mm: Vec<Vec<f32>>,
    pub energy: EnergyBudget,
    pub water: WaterBudget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circulation: Option<crate::circulation::CirculationAtlas>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WaterBudget {
    pub evaporation_m3: f64,
    pub runoff_to_ocean_m3: f64,
    pub boundary_inflow_m3: f64,
    pub precipitation_m3: f64,
    pub storage_change_m3: f64,
    pub residual_m3: f64,
    /// Local prescribed rainfall is a reporting constraint, outside the transport ledger.
    #[serde(default)]
    pub prescribed_output_scale: Option<f64>,
}

pub fn descriptor(grid: &MapGrid) -> GridDescriptor {
    match grid {
        MapGrid::Sphere(g) => GridDescriptor::sphere(g.freq(), g.radius_m()),
        MapGrid::Plane(g) => {
            let (lat, lon) = g.direction_at(0., 0.).to_lat_lon_deg();
            let cols = (g.width() / grid.distance_m(0, 1)).round() as u32;
            GridDescriptor::plane(
                lat,
                lon,
                g.width(),
                cols,
                (grid.cell_count() / cols as usize) as u32,
                g.radius_m(),
            )
        }
        _ => unreachable!("atmospheric atlas is spherical or metric planar"),
    }
}

pub fn solar_longitude(month: usize) -> f64 {
    std::f64::consts::TAU * ((month as f64 + 0.5) / 12. - 80. / scientific::DAYS_PER_YEAR)
}

fn energy_grid(atmosphere: &MapGrid) -> MapGrid {
    match atmosphere {
        MapGrid::Sphere(g) => MapGrid::Sphere(SphereHexGrid::build(
            g.freq().min(12),
            g.radius_m(),
            (DVec3::Z, 0.),
        )),
        MapGrid::Plane(g) => {
            let (lat, lon) = g.direction_at(0., 0.).to_lat_lon_deg();
            let cols = ((g.width() / 250000.).round() as u32).clamp(12, 36);
            let rows = (g.height() / g.width() * cols as f64 * 2. / 3_f64.sqrt()).ceil() as u32 + 2;
            MapGrid::Plane(FlatGrid::new(
                cols + 2,
                rows,
                g.width() * (cols + 2) as f64 / cols as f64,
                lat,
                lon,
                g.radius_m(),
            ))
        }
        _ => unreachable!(),
    }
}

fn apply_matrix(diagonal: &[f64], edges: &[(usize, usize, f64)], x: &[f64], out: &mut [f64]) {
    for i in 0..x.len() {
        out[i] = diagonal[i] * x[i];
    }
    for &(i, j, k) in edges {
        out[i] -= k * x[j];
        out[j] -= k * x[i];
    }
}

/// Diagonal-preconditioned conjugate gradient on the symmetric implicit heat system.
fn implicit_heat(
    diagonal: &[f64],
    edges: &[(usize, usize, f64)],
    rhs: &[f64],
    temperature: &mut [f64],
) -> bool {
    let n = temperature.len();
    let mut ap = vec![0.; n];
    apply_matrix(diagonal, edges, temperature, &mut ap);
    let mut r: Vec<_> = rhs.iter().zip(&ap).map(|(b, a)| b - a).collect();
    let mut z: Vec<_> = r.iter().zip(diagonal).map(|(r, d)| r / d).collect();
    let mut p = z.clone();
    let mut rz = r.iter().zip(&z).map(|(a, b)| a * b).sum::<f64>();
    let initial = rz.abs().max(1e-20);
    for _ in 0..200 {
        if rz.abs() < initial * 1e-16 || rz.abs() < 1e-20 {
            break;
        }
        apply_matrix(diagonal, edges, &p, &mut ap);
        let denominator = p.iter().zip(&ap).map(|(a, b)| a * b).sum::<f64>();
        if denominator <= 0. {
            break;
        }
        let alpha = rz / denominator;
        for i in 0..n {
            temperature[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
            z[i] = r[i] / diagonal[i];
        }
        let next = r.iter().zip(&z).map(|(a, b)| a * b).sum::<f64>();
        let beta = next / rz;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
        rz = next;
    }
    apply_matrix(diagonal, edges, temperature, &mut ap);
    let residual = rhs
        .iter()
        .zip(&ap)
        .map(|(b, a)| (b - a).powi(2))
        .sum::<f64>()
        .sqrt();
    let scale = rhs.iter().map(|b| b * b).sum::<f64>().sqrt().max(1.);
    residual.is_finite() && residual / scale < 1e-7
}

pub fn energy(
    atmosphere: &MapGrid,
    heights: &[f64],
    oceans: &[f64],
    planet: &PlanetParams,
    greenhouse: f64,
    prescribed: Option<&[f64]>,
    cancel: &dyn Fn() -> bool,
) -> Option<ClimateAtlas> {
    energy_observed(
        atmosphere,
        heights,
        oceans,
        planet,
        greenhouse,
        prescribed,
        cancel,
        &mut crate::preview::NoFrames,
    )
}

pub(crate) fn energy_observed(
    atmosphere: &MapGrid,
    heights: &[f64],
    oceans: &[f64],
    planet: &PlanetParams,
    greenhouse: f64,
    prescribed: Option<&[f64]>,
    cancel: &dyn Fn() -> bool,
    observer: &mut dyn crate::preview::FrameObserver,
) -> Option<ClimateAtlas> {
    energy_with_currents(
        atmosphere, heights, oceans, planet, greenhouse, prescribed, None, cancel, observer,
    )
}

pub(crate) fn energy_with_currents(
    atmosphere: &MapGrid,
    heights: &[f64],
    oceans: &[f64],
    planet: &PlanetParams,
    greenhouse: f64,
    prescribed: Option<&[f64]>,
    circulation: Option<&crate::circulation::CirculationAtlas>,
    cancel: &dyn Fn() -> bool,
    observer: &mut dyn crate::preview::FrameObserver,
) -> Option<ClimateAtlas> {
    let grid = energy_grid(atmosphere);
    let n = grid.cell_count();
    let mut area = vec![0.; n];
    let mut ocean = vec![0.; n];
    let mut elevation = vec![0.; n];
    for i in 0..atmosphere.cell_count() {
        let j = grid.cell_at(atmosphere.center_dir(i))?;
        let a = atmosphere.area_m2(i);
        area[j] += a;
        ocean[j] += oceans[i] * a;
        elevation[j] += heights[i] * a;
    }
    for i in 0..n {
        if area[i] > 0. {
            ocean[i] /= area[i];
            elevation[i] /= area[i];
        } else {
            let j = atmosphere.cell_at(grid.center_dir(i)).unwrap_or_else(|| {
                (0..atmosphere.cell_count())
                    .min_by(|&a, &b| {
                        grid.center_dir(i)
                            .angle_to(&atmosphere.center_dir(a))
                            .total_cmp(&grid.center_dir(i).angle_to(&atmosphere.center_dir(b)))
                    })
                    .unwrap()
            });
            ocean[i] = oceans[j];
            elevation[i] = heights[j];
        }
        area[i] = grid.area_m2(i);
    }
    let capacity: Vec<_> = ocean
        .iter()
        .map(|&o| scientific::ocean_heat_capacity(2. + o * 48.) + 1.0e7)
        .collect();
    let dt = DAY * scientific::DAYS_PER_YEAR / 12.;
    let ocean_transport: Option<Vec<_>> = circulation.map(|c| {
        (0..12)
            .map(|m| {
                let currents: Vec<_> = (0..n)
                    .map(|i| {
                        let p = if atmosphere.field_is_planar() {
                            grid.field_position(i)
                        } else {
                            grid.center_dir(i).as_vec() * atmosphere.radius_m()
                        };
                        let weights =
                            crate::evolution::stencil(atmosphere, p).unwrap_or_else(|| {
                                let j = atmosphere.cell_at(grid.center_dir(i)).unwrap_or(0);
                                [(j, 1.), (j, 0.), (j, 0.)]
                            });
                        (
                            weights
                                .iter()
                                .map(|&(j, w)| c.current_east_m_s[m][j] as f64 * w)
                                .sum(),
                            weights
                                .iter()
                                .map(|&(j, w)| c.current_north_m_s[m][j] as f64 * w)
                                .sum(),
                        )
                    })
                    .collect();
                crate::circulation::HeatTransport::new(&grid, &ocean, &currents, &capacity)
            })
            .collect()
    });
    let mut edges = Vec::new();
    let mut conductance = vec![0.; n];
    for i in 0..n {
        for &j in grid.neighbors(i) {
            let j = j as usize;
            if j <= i {
                continue;
            }
            let d = grid.distance_m(i, j);
            let face = 2. * (area[i] * area[j]).sqrt() / (3. * d);
            let k = 0.55 * planet.radius_m.powi(2) * face / d;
            edges.push((i, j, k));
            conductance[i] += k;
            conductance[j] += k;
        }
    }
    let b = 2.;
    let forcing = ((greenhouse - 1.) * 20. + planet.atmosphere_density.ln() * 5.) * b;
    let a = 210. - forcing;
    let boundary = grid.boundary_mask();
    let mut temperature: Vec<f64> = (0..n)
        .map(|i| 14. + 18. * grid.latitude_deg(i).to_radians().cos().powi(2) - 12.)
        .collect();
    let mut monthly = vec![vec![0_f32; n]; 12];
    let mut budget = EnergyBudget::default();
    let mut diagonal = vec![0.; n];
    let mut rhs = vec![0.; n];
    for year in 0..32 {
        let annual_start = temperature.clone();
        let mut absorbed_sum = 0.;
        let mut longwave_sum = 0.;
        let mut storage_sum = 0.;
        let mut external_sum = 0.;
        for month in 0..12 {
            if cancel() {
                return None;
            }
            let old = temperature.clone();
            let mut advected = old.clone();
            if let Some(transport) = &ocean_transport {
                transport[month].advect(&mut advected, dt);
            }
            let mut absorbed = vec![0.; n];
            let mut external = vec![0.; n];
            for i in 0..n {
                let local = old[i] - elevation[i].max(0.) * 0.0065;
                let ice = ((-local - 4.) / 12.).clamp(0., 1.);
                let albedo = (0.31 - ocean[i] * 0.04) * (1. - ice) + 0.60 * ice;
                let q = scientific::daily_insolation(
                    grid.latitude_deg(i).to_radians(),
                    solar_longitude(month),
                    planet.axial_tilt_deg.to_radians(),
                    planet.solar_constant,
                );
                absorbed[i] = q * (1. - albedo);
                // Open finite domains exchange heat with a broad external climate;
                // unlike a globe they are not a closed energy budget.
                let boundary_k = if boundary[i] { 4. } else { 0. };
                let background = 28. - 42. * grid.latitude_deg(i).to_radians().sin().powi(2);
                diagonal[i] = area[i] * (capacity[i] / dt + b + boundary_k) + conductance[i];
                rhs[i] = area[i]
                    * (capacity[i] / dt * advected[i] + absorbed[i] - a + boundary_k * background);
                external[i] = boundary_k * background;
            }
            if !implicit_heat(&diagonal, &edges, &rhs, &mut temperature) {
                return None;
            }
            for i in 0..n {
                if !temperature[i].is_finite() || temperature[i].abs() > 200. {
                    return None;
                }
                let boundary_k = if boundary[i] { 4. } else { 0. };
                monthly[month][i] = ((old[i] + temperature[i]) * 0.5) as f32;
                absorbed_sum += absorbed[i] * area[i] / 12.;
                longwave_sum += scientific::outgoing_longwave(temperature[i], a, b) * area[i] / 12.;
                storage_sum += capacity[i] * (temperature[i] - old[i]) / dt * area[i] / 12.;
                external_sum += (external[i] - boundary_k * temperature[i]) * area[i] / 12.;
            }
            let mut info = crate::preview::FrameInfo::stage(
                crate::Stage::Climate,
                0.35 * (year * 12 + month + 1) as f64 / (32. * 12.),
            );
            info.stage_complete = false;
            info.step = (year * 12 + month + 1) as u32;
            info.year = Some(year as u32 + 1);
            info.month = Some(month as u8 + 1);
            info.channels = crate::preview::TEMPERATURE;
            info.detail = "温度平衡";
            if observer.wants_frame(info) {
                let mut preview = WorldFields::with_capacity(n);
                for i in 0..n {
                    preview.elevation[i] = elevation[i] as f32;
                    preview.temperature[i] =
                        (temperature[i] - elevation[i].max(0.) * 0.0065) as f32;
                }
                observer.frame(&grid, &preview, info);
            }
        }
        let total_area = area.iter().sum::<f64>();
        budget = EnergyBudget {
            temperature_basis:
                "sea-level reference column; surface temperature uses a lapse-rate diagnostic"
                    .into(),
            spinup_years: year as u32 + 1,
            annual_cycle_converged: false,
            annual_cycle_change_c: temperature
                .iter()
                .zip(&annual_start)
                .map(|(a, b)| (a - b).abs())
                .fold(0., f64::max),
            mean_absorbed_solar_w_m2: absorbed_sum / total_area,
            mean_longwave_w_m2: longwave_sum / total_area,
            mean_storage_w_m2: storage_sum / total_area,
            mean_boundary_heat_w_m2: external_sum / total_area,
            residual_w_m2: (absorbed_sum - longwave_sum + external_sum - storage_sum).abs()
                / total_area,
        };
        budget.annual_cycle_converged = budget.annual_cycle_change_c < 0.03;
        if year >= 4 && budget.annual_cycle_change_c < 0.03 {
            break;
        }
    }
    let m = atmosphere.cell_count();
    let mut output = vec![vec![0_f32; m]; 12];
    for i in 0..m {
        let weights = crate::evolution::stencil(&grid, atmosphere.field_position(i))
            .unwrap_or_else(|| {
                let j = grid.cell_at(atmosphere.center_dir(i)).unwrap_or(0);
                [(j, 1.), (j, 0.), (j, 0.)]
            });
        for month in 0..12 {
            output[month][i] = weights
                .iter()
                .map(|&(j, w)| monthly[month][j] as f64 * w)
                .sum::<f64>() as f32
                - heights[i].max(0.) as f32 * 0.0065;
        }
        if let Some(means) = prescribed {
            let mean = output.iter().map(|v| v[i] as f64).sum::<f64>() / 12.;
            for month in 0..12 {
                output[month][i] += (means[i] - mean) as f32;
            }
        }
    }
    Some(ClimateAtlas {
        grid: descriptor(atmosphere),
        model: if prescribed.is_some() {
            "prescribed-local+climlab-ebm-seasonal-anomalies"
        } else {
            "climlab-reference-ebm+surface-lapse-v1"
        }
        .into(),
        axial_tilt_deg: planet.axial_tilt_deg,
        temperature_c: output,
        precipitation_mm: vec![vec![0.; m]; 12],
        pet_mm: vec![vec![0.; m]; 12],
        energy: budget,
        water: WaterBudget::default(),
        circulation: None,
    })
}

impl ClimateAtlas {
    pub fn sample(
        &self,
        grid: &MapGrid,
        direction: tf_core::geo::UnitVec3,
        annual_temperature: f32,
        annual_precipitation: f32,
    ) -> ([f32; 12], [f32; 12], [f32; 12]) {
        let position = match grid {
            MapGrid::Plane(g) => {
                let (x, y) = g.project(direction);
                DVec3::new(x, y, 0.)
            }
            _ => direction.as_vec() * grid.radius_m(),
        };
        let weights = crate::evolution::stencil(grid, position).unwrap_or_else(|| {
            let j = grid.cell_at(direction).unwrap_or(0);
            [(j, 1.), (j, 0.), (j, 0.)]
        });
        let mut temperature: [f32; 12] = std::array::from_fn(|m| {
            weights
                .iter()
                .map(|&(i, w)| self.temperature_c[m][i] as f64 * w)
                .sum::<f64>() as f32
        });
        let mut rain: [f32; 12] = std::array::from_fn(|m| {
            weights
                .iter()
                .map(|&(i, w)| self.precipitation_mm[m][i] as f64 * w)
                .sum::<f64>() as f32
        });
        let mean = temperature.iter().sum::<f32>() / 12.;
        for t in &mut temperature {
            *t += annual_temperature - mean;
        }
        let total = rain.iter().sum::<f32>();
        for r in &mut rain {
            *r = if total > 1e-8 {
                *r * annual_precipitation / total
            } else {
                annual_precipitation / 12.
            };
        }
        let pet = scientific::monthly_pet(
            &temperature,
            direction.to_lat_lon_deg().0.to_radians(),
            self.axial_tilt_deg.to_radians(),
        );
        (temperature, rain, pet)
    }
    pub fn validate(&self) -> Result<(), String> {
        // Atmospheric meshes deliberately differ from selectable map meshes.
        // Do not weaken GridDescriptor's public map-resolution validation.
        match &self.grid {
            GridDescriptor::Sphere {
                frequency,
                radius_m,
                version,
            } => {
                if *version != tf_grid::GRID_VERSION
                    || !(4..=56).contains(frequency)
                    || !radius_m.is_finite()
                    || !(1e6..=5e7).contains(radius_m)
                {
                    return Err("月气候球面网格无效".into());
                }
            }
            GridDescriptor::Plane { .. } => self.grid.validate()?,
            _ => return Err("月气候网格类型无效".into()),
        }
        let n = self.grid.cell_count();
        if n > MAX_ATLAS_CELLS {
            return Err("月气候网格超过限制".into());
        }
        if !self.axial_tilt_deg.is_finite() || !(0. ..=90.).contains(&self.axial_tilt_deg) {
            return Err("月气候轴倾角无效".into());
        }
        for values in [&self.temperature_c, &self.precipitation_mm, &self.pet_mm] {
            if values.len() != 12
                || values
                    .iter()
                    .any(|v| v.len() != n || v.iter().any(|x| !x.is_finite()))
            {
                return Err("月气候尺寸或数值无效".into());
            }
        }
        if self
            .precipitation_mm
            .iter()
            .chain(&self.pet_mm)
            .flatten()
            .any(|&x| x < 0.)
        {
            return Err("月降水和蒸散不能为负".into());
        }
        if let Some(c) = &self.circulation {
            if c.ocean_fraction.len() != n
                || c.ocean_fraction
                    .iter()
                    .any(|v| !v.is_finite() || !(0. ..=1.).contains(v))
                || c.relative_solver_residual.len() != 12
                || c.relative_solver_residual
                    .iter()
                    .any(|v| !v.is_finite() || *v < 0.)
                || c.solver_iterations.len() != 12
            {
                return Err("环流诊断资料无效".into());
            }
            for (values, limit) in [
                (&c.wind_east_m_s, 100.),
                (&c.wind_north_m_s, 100.),
                (&c.current_east_m_s, 10.),
                (&c.current_north_m_s, 10.),
            ] {
                if values.len() != 12
                    || values.iter().any(|v| {
                        v.len() != n || v.iter().any(|x| !x.is_finite() || x.abs() > limit)
                    })
                {
                    return Err("风场或洋流资料尺寸或数值无效".into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ocean_gyres_change_monthly_temperature_without_creating_global_heat() {
        let grid = MapGrid::Plane(FlatGrid::new(24, 18, 6e6, 30., 0., 6371000.));
        let n = grid.cell_count();
        let ocean = vec![1.; n];
        let height = vec![0.; n];
        let wind: Vec<_> = (0..n)
            .map(|i| {
                let u = (grid.local_center_m(i).unwrap().1 / 1e6).cos();
                (DVec3::X * u, u, 0.)
            })
            .collect();
        let c = crate::circulation::solve(
            &grid,
            &ocean,
            &vec![wind; 12],
            10.,
            &PlanetParams::default(),
            &|| false,
        )
        .unwrap();
        let base = energy(
            &grid,
            &height,
            &ocean,
            &PlanetParams::default(),
            1.,
            None,
            &|| false,
        )
        .unwrap();
        let coupled = energy_with_currents(
            &grid,
            &height,
            &ocean,
            &PlanetParams::default(),
            1.,
            None,
            Some(&c),
            &|| false,
            &mut crate::preview::NoFrames,
        )
        .unwrap();
        let difference = base
            .temperature_c
            .iter()
            .flatten()
            .zip(coupled.temperature_c.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0_f32, f32::max);
        assert!(
            difference > 0.002,
            "ocean transport had no thermal effect: {difference}"
        );
        assert!(coupled.energy.residual_w_m2 < 1e-4, "{:?}", coupled.energy);
    }
    #[test]
    fn homogeneous_ocean_agrees_with_complete_climlab_ebm() {
        // This oracle is actual climlab.EBM_seasonal output, not values produced
        // by this solver. A warm zero-tilt case isolates transport and radiation.
        let reference: serde_json::Value = serde_json::from_str(include_str!(
            "../../../third_party/climlab/ebm-reference.json"
        ))
        .unwrap();
        let latitudes: Vec<f64> = reference["reference_latitude_deg"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let temperatures: Vec<f64> = reference["reference_temperature_c"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let g = MapGrid::Sphere(SphereHexGrid::build(12, 6_371_000., (DVec3::Z, 0.)));
        let n = g.cell_count();
        let mut planet = PlanetParams::default();
        planet.axial_tilt_deg = 0.;
        let a = energy(&g, &vec![0.; n], &vec![1.; n], &planet, 2., None, &|| false).unwrap();
        assert!(a.energy.annual_cycle_converged);
        assert!(a.energy.residual_w_m2 < 0.01);
        let mut squared_error = 0.;
        let mut maximum_error = 0_f64;
        let mut total_area = 0.;
        for i in 0..n {
            let latitude = g.latitude_deg(i);
            let j = latitudes
                .partition_point(|x| *x < latitude)
                .clamp(1, latitudes.len() - 1);
            let t =
                ((latitude - latitudes[j - 1]) / (latitudes[j] - latitudes[j - 1])).clamp(0., 1.);
            let expected = temperatures[j - 1] * (1. - t) + temperatures[j] * t;
            let actual = a.temperature_c.iter().map(|m| m[i] as f64).sum::<f64>() / 12.;
            assert!(
                actual > 0.,
                "ice feedback must be excluded from the comparison"
            );
            let error = actual - expected;
            squared_error += g.area_m2(i) * error.powi(2);
            total_area += g.area_m2(i);
            maximum_error = maximum_error.max(error.abs());
        }
        let rms = (squared_error / total_area).sqrt();
        assert!(
            rms < 0.5 && maximum_error < 1.2,
            "RMS {rms}, max {maximum_error}"
        );
    }
    #[test]
    fn seasonal_thermal_storage_and_energy_budget_have_physical_limits() {
        let g = MapGrid::Sphere(SphereHexGrid::build(8, 6371000., (DVec3::Z, 0.)));
        let n = g.cell_count();
        let ocean: Vec<_> = (0..n)
            .map(|i| {
                if g.center_dir(i).as_vec().x > 0. {
                    1.
                } else {
                    0.
                }
            })
            .collect();
        let a = energy(
            &g,
            &vec![0.; n],
            &ocean,
            &PlanetParams::default(),
            1.,
            None,
            &|| false,
        )
        .unwrap();
        a.validate().unwrap();
        assert!(a.energy.residual_w_m2 < 0.01, "{:?}", a.energy);
        let amplitude = |wet: bool| {
            let cells: Vec<_> = (0..n)
                .filter(|&i| (40. ..50.).contains(&g.latitude_deg(i)) && (ocean[i] > 0.5) == wet)
                .collect();
            cells
                .iter()
                .map(|&i| {
                    let v: Vec<_> = a.temperature_c.iter().map(|v| v[i]).collect();
                    v.iter().copied().fold(f32::NEG_INFINITY, f32::max)
                        - v.iter().copied().fold(f32::INFINITY, f32::min)
                })
                .sum::<f32>()
                / cells.len() as f32
        };
        assert!(
            amplitude(false) > amplitude(true) * 1.2,
            "land {}, ocean {}",
            amplitude(false),
            amplitude(true)
        );
        let mut p = PlanetParams::default();
        p.axial_tilt_deg = 0.;
        let zero = energy(&g, &vec![0.; n], &ocean, &p, 1., None, &|| false).unwrap();
        assert!(zero.temperature_c.iter().all(|v| v
            .iter()
            .zip(&zero.temperature_c[0])
            .all(|(a, b)| (a - b).abs() < 0.1)));
    }
}
