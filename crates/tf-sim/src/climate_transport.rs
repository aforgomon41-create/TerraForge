//! Finite-volume vapour/cloud transport on an atmospheric mesh independent of rendering.
//! Inspired by two-reservoir orographic models, not a weather forecasting solver.
use crate::{climate::ClimateParams, WorldFields};
use glam::DVec3;
use tf_core::params::PlanetParams;
use tf_grid::{FlatGrid, GridOps, MapGrid, SphereHexGrid};

const DAY: f64 = 86400.;
const YEAR: f64 = 365.25 * DAY;

#[derive(Clone, Default)]
struct Transport {
    edges: Vec<(usize, f64)>,
    remaining: f64,
    inflow: f64,
}
fn advect(input: &[f64], output: &mut [f64], transport: &[Transport], boundary: &[f64]) {
    for i in 0..input.len() {
        output[i] = input[i] * transport[i].remaining + boundary[i] * transport[i].inflow;
    }
    for i in 0..input.len() {
        for &(j, factor) in &transport[i].edges {
            output[j] += input[i] * factor;
        }
    }
}
fn smooth(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

fn wind<G: GridOps>(
    g: &G,
    i: usize,
    season: f64,
    planet: &PlanetParams,
    _land: f64,
    monsoon: bool,
) -> (DVec3, f64, f64) {
    let latitude = g.latitude_deg(i);
    let shift = if monsoon {
        season * planet.axial_tilt_deg * 0.45
    } else {
        0.
    };
    let relative = latitude - shift;
    let hadley = (30. * (planet.rotation_period_hours / 24.).powf(0.35)).clamp(12., 55.);
    let a = relative.abs();
    let u = -1. + 2. * smooth(hadley - 10., hadley + 8., a) - 2. * smooth(62., 78., a);
    let hemisphere = (relative / 12.).tanh();
    let v = -hemisphere * 0.4 * (-0.5 * ((a - hadley * 0.5) / (hadley * 0.65)).powi(2)).exp()
        + hemisphere * 0.4 * (-0.5 * ((a - (hadley + 18.)) / 13.).powi(2)).exp();
    let length = u.hypot(v).max(0.2);
    let (east, north) = g.center_dir(i).tangent_frame();
    let vector = if g.extent_m().is_some() {
        DVec3::new(u / length, v / length, 0.)
    } else {
        (east * u + north * v) / length
    };
    (vector, u / length, v / length)
}

/// Steady pressure-gradient / Coriolis / drag anomaly from the actual monthly
/// land-sea thermal contrast. The background wind belts remain a parameterization.
fn thermal_wind<G: GridOps>(
    g: &G,
    i: usize,
    base: (DVec3, f64, f64),
    temperature: &[f32],
    heights: &[f64],
    planet: &PlanetParams,
    speed: f64,
) -> (DVec3, f64, f64) {
    let (east, north) = if g.field_is_planar() {
        (DVec3::X, DVec3::Y)
    } else {
        g.center_dir(i).tangent_frame()
    };
    let mut gradient = DVec3::ZERO;
    for &j in g.neighbors(i) {
        let j = j as usize;
        let delta = g.field_position(j) - g.field_position(i);
        let distance = g.distance_m(i, j).max(1.);
        let dt = temperature[j] as f64 + heights[j].max(0.) * 0.0065
            - temperature[i] as f64
            - heights[i].max(0.) * 0.0065;
        gradient += delta.normalize_or_zero() * dt / distance * 2. / g.neighbors(i).len() as f64;
    }
    let ax = 287. * 0.10 * gradient.dot(east);
    let ay = 287. * 0.10 * gradient.dot(north);
    let coriolis = planet.coriolis(g.latitude_deg(i));
    let drag = 1. / (DAY * 1.2);
    let den = drag * drag + coriolis * coriolis;
    let mut anomaly =
        east * ((drag * ax + coriolis * ay) / den) + north * ((drag * ay - coriolis * ax) / den);
    if anomaly.length() > speed * 0.7 {
        anomaly *= speed * 0.7 / anomaly.length();
    }
    let v = (base.0 * speed + anomaly).normalize_or_zero();
    (v, v.dot(east), v.dot(north))
}

fn transfer_mesh<G: GridOps>(
    g: &G,
    winds: &[(DVec3, f64, f64)],
    speed: f64,
    dt: f64,
) -> Vec<Transport> {
    let mut result = Vec::with_capacity(g.cell_count());
    for i in 0..g.cell_count() {
        let mut edges = Vec::new();
        let mut outgoing = 0.;
        let mut incoming = 0.;
        let p = g.field_position(i);
        for &j in g.neighbors(i) {
            let j = j as usize;
            let distance = g.distance_m(i, j).max(1.);
            // Symmetric shared edge length; regular hexagon limit is s/sqrt(3).
            let edge_length = (g.area_m2(i) + g.area_m2(j)) / (3. * distance);
            let alignment = winds[i]
                .0
                .dot((g.field_position(j) - p).normalize_or_zero())
                .max(0.);
            let volume = speed * dt * alignment * edge_length;
            outgoing += volume / g.area_m2(i);
            edges.push((j, volume / g.area_m2(j)));
        }
        if let (Some((width, height)), Some((x, y))) = (g.extent_m(), g.local_center_m(i)) {
            let spacing = g
                .neighbors(i)
                .first()
                .map_or(1., |&j| g.distance_m(i, j as usize));
            for (dx, dy) in [
                (1., 0.),
                (-1., 0.),
                (0.5, 3_f64.sqrt() / 2.),
                (-0.5, 3_f64.sqrt() / 2.),
                (0.5, -3_f64.sqrt() / 2.),
                (-0.5, -3_f64.sqrt() / 2.),
            ] {
                let (qx, qy) = (x + dx * spacing, y + dy * spacing);
                if qx.abs() > width / 2. || qy.abs() > height / 2. {
                    let flux =
                        speed * dt * 2. / (3. * spacing) * winds[i].0.dot(DVec3::new(dx, dy, 0.));
                    outgoing += flux.max(0.);
                    incoming += (-flux).max(0.);
                }
            }
        }
        debug_assert!(outgoing <= 1. + 1e-8, "CFL violated: {outgoing}");
        result.push(Transport {
            edges,
            remaining: (1. - outgoing).max(0.),
            inflow: incoming,
        });
    }
    result
}

#[derive(Default)]
struct WaterFlux {
    evaporation: f64,
    runoff: f64,
    rain: f64,
}
fn water_step(
    v: &mut f64,
    c: &mut f64,
    soil: &mut f64,
    capacity: f64,
    ocean: f64,
    rise: f64,
    convection: f64,
    rain_fraction: f64,
    dt: f64,
    params: &ClimateParams,
) -> WaterFlux {
    let potential = 3.2 * (capacity / 30.).sqrt() * dt / DAY * params.precipitation_scale;
    let evap_ocean = (potential * ocean).min((capacity * 0.95 - *v).max(0.));
    let tension = (*soil / 120.).clamp(0., 1.);
    let evap_land = (potential * params.moisture_recycling * (1. - ocean) * tension)
        .min(*soil)
        .min((capacity * 0.8 - *v).max(0.));
    *soil -= evap_land;
    *v += evap_ocean + evap_land;
    let saturation = (*v - capacity).max(0.);
    let uplift = 1. - (-rise.max(0.) / 2200. * params.orographic_strength).exp();
    let condensed =
        (saturation + v.min(capacity) * (1. - (1. - convection) * (1. - uplift))).min(*v);
    *v -= condensed;
    *c += condensed;
    let falling = (1. - (rise.min(0.) / 1200.).exp()) * (1. - *v / capacity).max(0.);
    let reevap = (*c * falling).min((capacity - *v).max(0.));
    *c -= reevap;
    *v += reevap;
    let rain = *c * rain_fraction;
    *c -= rain;
    let infiltration = rain * (1. - ocean);
    let overflow = (*soil + infiltration - 200.).max(0.);
    *soil += infiltration - overflow;
    WaterFlux {
        evaporation: evap_ocean,
        runoff: rain * ocean + overflow,
        rain,
    }
}

/// Compact C2 weights with a bounded local fit avoid visible atmospheric triangles.
struct RainInterpolator<'a> {
    grid: &'a MapGrid,
    values: &'a [f64],
    stencils: Vec<Vec<usize>>,
    support: f64,
}
impl<'a> RainInterpolator<'a> {
    fn new(grid: &'a MapGrid, values: &'a [f64]) -> Self {
        let stencils = (0..grid.cell_count())
            .map(|i| {
                let mut cells = vec![i];
                let mut frontier = vec![i];
                for _ in 0..4 {
                    let mut next = Vec::new();
                    for &j in &frontier {
                        for &k in grid.neighbors(j) {
                            let k = k as usize;
                            if !cells.contains(&k) {
                                cells.push(k);
                                next.push(k);
                            }
                        }
                    }
                    frontier = next;
                }
                cells
            })
            .collect();
        let mean_area =
            (0..grid.cell_count()).map(|i| grid.area_m2(i)).sum::<f64>() / grid.cell_count() as f64;
        Self {
            grid,
            values,
            stencils,
            support: mean_area.sqrt() * 2.3,
        }
    }
    fn sample(&self, dir: tf_core::geo::UnitVec3) -> f64 {
        let g = self.grid;
        let query = match g {
            MapGrid::Plane(flat) => {
                let (x, y) = flat.project(dir);
                DVec3::new(x, y, 0.)
            }
            _ => dir.as_vec() * g.radius_m(),
        };
        let center = g.cell_at(dir).unwrap_or_else(|| match g {
            MapGrid::Plane(flat) => flat
                .cell_at_xy(
                    query.x.clamp(-flat.width() / 2., flat.width() / 2.),
                    query.y.clamp(-flat.height() / 2., flat.height() / 2.),
                )
                .unwrap_or(0),
            _ => 0,
        });
        let (east, north) = if g.field_is_planar() {
            (DVec3::X, DVec3::Y)
        } else {
            dir.tangent_frame()
        };
        let (mut sum, mut total, mut sx, mut sy, mut xx, mut xy, mut yy, mut vx, mut vy) =
            (0., 0., 0., 0., 0., 0., 0., 0., 0.);
        let mut low = f64::INFINITY;
        let mut high = f64::NEG_INFINITY;
        for &j in &self.stencils[center] {
            let delta = (g.field_position(j) - query) / self.support;
            let r = delta.length();
            if r >= 1. {
                continue;
            }
            let weight = (1. - r).powi(4) * (1. + 4. * r);
            let (x, y) = (delta.dot(east), delta.dot(north));
            let value = self.values[j];
            total += weight;
            sum += weight * value;
            sx += weight * x;
            sy += weight * y;
            xx += weight * x * x;
            xy += weight * x * y;
            yy += weight * y * y;
            vx += weight * value * x;
            vy += weight * value * y;
            low = low.min(value);
            high = high.max(value);
        }
        if total < 1e-20 {
            return self.values[center];
        }
        // Moving local linear fit removes bias on smooth gradients while preserving bounds.
        let mx = sx / total;
        let my = sy / total;
        let mean = sum / total;
        let cxx = xx / total - mx * mx;
        let cxy = xy / total - mx * my;
        let cyy = yy / total - my * my;
        let cxv = vx / total - mx * mean;
        let cyv = vy / total - my * mean;
        let determinant = cxx * cyy - cxy * cxy;
        if determinant > 1e-12 {
            let bx = (cxv * cyy - cyv * cxy) / determinant;
            let by = (cyv * cxx - cxv * cxy) / determinant;
            (mean - bx * mx - by * my).clamp(low, high)
        } else {
            mean
        }
    }
}

pub fn solve<G: GridOps>(
    fine: &G,
    fields: &mut WorldFields,
    planet: &PlanetParams,
    params: &ClimateParams,
    cancel: &dyn Fn() -> bool,
) -> bool {
    solve_at_resolution(fine, fields, planet, params, cancel, 75_000., 32)
}
pub(crate) fn solve_at_resolution<G: GridOps>(
    fine: &G,
    fields: &mut WorldFields,
    planet: &PlanetParams,
    params: &ClimateParams,
    cancel: &dyn Fn() -> bool,
    spacing_m: f64,
    sphere_frequency: u32,
) -> bool {
    let atmospheric = if let Some((w, h)) = fine.extent_m() {
        let central = (0..fine.cell_count())
            .min_by(|&a, &b| {
                let squared = |i| fine.local_center_m(i).map_or(0., |(x, y)| x * x + y * y);
                squared(a).total_cmp(&squared(b))
            })
            .unwrap_or(0);
        let (lat, lon) = fine.center_dir(central).to_lat_lon_deg();
        // Regional weather has an upstream context even when the map is only metres wide.
        if lat.abs() > 85. {
            MapGrid::Sphere(SphereHexGrid::build(12, planet.radius_m, (DVec3::Z, 0.)))
        } else {
            let padding = 1_000_000_f64.min(planet.radius_m * 0.5);
            let width = (w + spacing_m * 2.).max(padding);
            let max_height = (89.8 - lat.abs()).to_radians() * 2. * planet.radius_m;
            let height = (h + spacing_m * 2.)
                .max(padding * 3_f64.sqrt() / 2.)
                .min(max_height);
            let cols = (width / spacing_m)
                .round()
                .max((4. * width * 3_f64.sqrt() / 2. / max_height).ceil())
                .clamp(12., 256.) as u32;
            let rows = (height / width * cols as f64 * 2. / 3_f64.sqrt())
                .floor()
                .clamp(4., 128.) as u32;
            MapGrid::Plane(FlatGrid::new(cols, rows, width, lat, lon, planet.radius_m))
        }
    } else {
        MapGrid::Sphere(SphereHexGrid::build(
            ((planet.radius_m / 6_371_000. * sphere_frequency as f64).round() as u32)
                .clamp(8, 56)
                .min(((fine.cell_count() as f64 / 10.).sqrt().floor() as u32).max(4)),
            planet.radius_m,
            (DVec3::Z, 0.),
        ))
    };
    let n = atmospheric.cell_count();
    // Conservative restriction: every fine cell contributes its physical area.
    let mut heights = vec![0.; n];
    let mut temperatures = vec![0.; n];
    let mut oceans = vec![0.; n];
    let mut areas = vec![0.; n];
    for i in 0..fine.cell_count() {
        if let Some(j) = atmospheric.cell_at(fine.center_dir(i)) {
            let area = fine.area_m2(i);
            areas[j] += area;
            // Air flows over the sea surface, not over bathymetry. Clamp before
            // restriction so deep water cannot cancel nearby land elevations.
            heights[j] += fields.elevation[i].max(0.) as f64 * area;
            temperatures[j] += fields.temperature[i] as f64 * area;
            oceans[j] += if fields.elevation[i] <= 0. { area } else { 0. };
        }
    }
    let boundary_cells: Vec<_> = fine.boundary_mask().iter_ones().collect();
    for i in 0..n {
        if areas[i] > 0. {
            heights[i] /= areas[i];
            temperatures[i] /= areas[i];
            oceans[i] /= areas[i];
            continue;
        }
        let dir = atmospheric.center_dir(i);
        let nearest = fine.locate(dir).unwrap_or_else(|| {
            boundary_cells
                .iter()
                .copied()
                .min_by(|&a, &b| {
                    (fine.center_dir(a).as_vec() - dir.as_vec())
                        .length_squared()
                        .total_cmp(&(fine.center_dir(b).as_vec() - dir.as_vec()).length_squared())
                })
                .unwrap_or(0)
        });
        heights[i] = fields.elevation[nearest].max(0.) as f64;
        temperatures[i] = fields.temperature[nearest] as f64;
        oceans[i] = if fields.elevation[nearest] <= 0. {
            1.
        } else {
            0.
        };
    }
    let min_spacing = (0..n)
        .flat_map(|i| {
            atmospheric
                .neighbors(i)
                .iter()
                .map(move |&j| (i, j as usize))
        })
        .map(|(i, j)| atmospheric.distance_m(i, j))
        .fold(f64::INFINITY, f64::min);
    let dt = (min_spacing / params.wind_speed_m_s * 0.32).clamp(1., 21600.);
    let Some(mut atlas) = crate::seasonal::energy(
        &atmospheric,
        &heights,
        &oceans,
        planet,
        params.greenhouse,
        if params.prescribed_temperature {
            Some(&temperatures)
        } else {
            None
        },
        cancel,
    ) else {
        return false;
    };
    let sample = (YEAR / 12. / dt).ceil() as usize;
    let mut soil = vec![30_f64; n];
    let mut vapor = vec![0.; n];
    let mut cloud = vec![0.; n];
    let water_total = |v: &[f64], c: &[f64], s: &[f64]| {
        (0..n)
            .map(|i| (v[i] + c[i] + s[i]) * atmospheric.area_m2(i) / 1000.)
            .sum::<f64>()
    };
    let mut storage_start = 0.;
    for cycle in 0..2 {
        if cycle == 1 {
            storage_start = water_total(&vapor, &cloud, &soil);
        }
        for month in 0..12 {
            let season = crate::seasonal::solar_longitude(month).sin();
            if cancel() {
                return false;
            }
            let winds: Vec<_> = (0..n)
                .map(|i| {
                    let base = wind(
                        &atmospheric,
                        i,
                        season,
                        planet,
                        1. - oceans[i],
                        params.monsoon,
                    );
                    thermal_wind(
                        &atmospheric,
                        i,
                        base,
                        &atlas.temperature_c[month],
                        &heights,
                        planet,
                        params.wind_speed_m_s,
                    )
                })
                .collect();
            let transport = transfer_mesh(&atmospheric, &winds, params.wind_speed_m_s, dt);
            let capacity: Vec<_> = (0..n)
                .map(|i| {
                    (30. * (0.055 * (atlas.temperature_c[month][i] as f64 - 20.)).exp()
                        * planet.atmosphere_density.sqrt())
                    .clamp(0.05, 150.)
                })
                .collect();
            if cycle == 0 && month == 0 {
                for i in 0..n {
                    vapor[i] = capacity[i] * 0.7;
                }
            }
            let boundary: Vec<_> = capacity.iter().map(|&x| x * 0.7).collect();
            let dry = vec![0.; n];
            let mut next_v = vec![0.; n];
            let mut next_c = vec![0.; n];
            let mut rise = vec![0.; n];
            let mut convection = vec![0.; n];
            for i in 0..n {
                let p = atmospheric.field_position(i);
                let mut gradient = 0.;
                let mut weight = 0.;
                for &j in atmospheric.neighbors(i) {
                    let j = j as usize;
                    let delta = atmospheric.field_position(j) - p;
                    let upstream = (-winds[i].0.dot(delta.normalize_or_zero())).max(0.);
                    gradient +=
                        (heights[i] - heights[j]) / atmospheric.distance_m(i, j).max(1.) * upstream;
                    weight += upstream;
                }
                rise[i] = gradient / weight.max(1e-20) * params.wind_speed_m_s * dt;
                let latitude = atmospheric.latitude_deg(i)
                    - if params.monsoon {
                        season * planet.axial_tilt_deg * 0.45
                    } else {
                        0.
                    };
                let itcz = (-0.5 * (latitude / 12.).powi(2)).exp();
                let storms = (-0.5 * ((latitude.abs() - 52.) / 13.).powi(2)).exp();
                let tau = DAY * (14. - 10. * itcz - 7. * storms).max(2.5);
                convection[i] = 1. - (-dt / tau).exp();
            }
            let rain_fraction = 1. - (-dt / 10800.).exp();
            for step in 0..sample {
                if step % 16 == 0 && cancel() {
                    return false;
                }
                let before = if cycle == 1 {
                    water_total(&vapor, &cloud, &soil)
                } else {
                    0.
                };
                advect(&vapor, &mut next_v, &transport, &boundary);
                advect(&cloud, &mut next_c, &transport, &dry);
                if cycle == 1 {
                    atlas.water.boundary_inflow_m3 += water_total(&next_v, &next_c, &soil) - before;
                }
                for i in 0..n {
                    let flux = water_step(
                        &mut next_v[i],
                        &mut next_c[i],
                        &mut soil[i],
                        capacity[i],
                        oceans[i],
                        rise[i],
                        convection[i],
                        rain_fraction,
                        dt,
                        params,
                    );
                    if cycle == 1 {
                        let factor = (YEAR / 12.) / (sample as f64 * dt);
                        atlas.precipitation_mm[month][i] += (flux.rain * factor) as f32;
                        let a = atmospheric.area_m2(i) / 1000.;
                        atlas.water.evaporation_m3 += flux.evaporation * a;
                        atlas.water.runoff_to_ocean_m3 += flux.runoff * a;
                        atlas.water.precipitation_m3 += flux.rain * a;
                    }
                }
                std::mem::swap(&mut vapor, &mut next_v);
                std::mem::swap(&mut cloud, &mut next_c);
            }
        }
    }
    atlas.water.storage_change_m3 = water_total(&vapor, &cloud, &soil) - storage_start;
    atlas.water.residual_m3 = (atlas.water.evaporation_m3 + atlas.water.boundary_inflow_m3
        - atlas.water.runoff_to_ocean_m3
        - atlas.water.storage_change_m3)
        .abs();
    let annual: Vec<_> = (0..n)
        .map(|i| atlas.precipitation_mm.iter().map(|v| v[i] as f64).sum())
        .collect();
    let annual_temperature: Vec<_> = (0..n)
        .map(|i| atlas.temperature_c.iter().map(|v| v[i] as f64).sum::<f64>() / 12.)
        .collect();
    for i in 0..n {
        let t = std::array::from_fn(|m| atlas.temperature_c[m][i]);
        let pet = crate::scientific::monthly_pet(
            &t,
            atmospheric.latitude_deg(i).to_radians(),
            planet.axial_tilt_deg.to_radians(),
        );
        for m in 0..12 {
            atlas.pet_mm[m][i] = pet[m];
        }
    }
    let rainfall = RainInterpolator::new(&atmospheric, &annual);
    let temperature = RainInterpolator::new(&atmospheric, &annual_temperature);
    let lapse_heights: Vec<_> = heights.iter().map(|h| h.max(0.)).collect();
    let background_height = RainInterpolator::new(&atmospheric, &lapse_heights);
    for i in 0..fine.cell_count() {
        if i % 2048 == 0 && cancel() {
            return false;
        }
        fields.precipitation[i] = rainfall.sample(fine.center_dir(i)).max(0.) as f32;
        if !params.prescribed_temperature {
            // Restore fine-scale lapse information removed by atmospheric averaging.
            fields.temperature[i] = (temperature.sample(fine.center_dir(i))
                - (fields.elevation[i].max(0.) as f64
                    - background_height.sample(fine.center_dir(i)))
                    * 0.0065) as f32;
        }
        let (_, _, pet) = atlas.sample(
            &atmospheric,
            fine.center_dir(i),
            fields.temperature[i],
            fields.precipitation[i],
        );
        fields.potential_evaporation[i] = pet.iter().sum();
        fields.aridity[i] = fields.precipitation[i] / fields.potential_evaporation[i].max(1.);
    }
    fields.climate = Some(atlas);
    !cancel()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rainfall_and_air_temperature_do_not_follow_underwater_relief() {
        let g = FlatGrid::new(48, 20, 1_500_000., 35., 0., 6_371_000.);
        let mut shallow = WorldFields::with_capacity(g.cell_count());
        let mut deep = shallow.clone();
        for i in 0..g.cell_count() {
            let (x, y) = g.xy(i);
            let coast = -250_000. + 100_000. * (y / 90_000.).sin();
            if x < coast {
                shallow.elevation[i] = -10.;
                deep.elevation[i] = (-500. - 6_000. * (x / 130_000.).sin().powi(2)) as f32;
            } else {
                let land = (100. + 1_800. * (-0.5 * (x / 150_000.).powi(2)).exp()) as f32;
                shallow.elevation[i] = land;
                deep.elevation[i] = land;
            }
        }
        for fields in [&mut shallow, &mut deep] {
            assert!(solve(
                &g,
                fields,
                &PlanetParams::default(),
                &ClimateParams::default(),
                &|| false,
            ));
        }
        // There is no bathymetry-dependent ocean circulation in this model:
        // identical land and sea surfaces must produce identical air climate.
        for (name, a, b) in [
            ("rain", &shallow.precipitation, &deep.precipitation),
            ("temperature", &shallow.temperature, &deep.temperature),
            (
                "PET",
                &shallow.potential_evaporation,
                &deep.potential_evaporation,
            ),
        ] {
            let delta = a
                .iter()
                .zip(b)
                .map(|(a, b)| (a - b).abs())
                .fold(0_f32, f32::max);
            assert!(delta < 1e-5, "underwater relief changed {name} by {delta}");
        }
        let a = shallow.climate.as_ref().unwrap();
        let b = deep.climate.as_ref().unwrap();
        assert_eq!(a.temperature_c, b.temperature_c);
        assert_eq!(a.precipitation_mm, b.precipitation_mm);
        assert_eq!(a.pet_mm, b.pet_mm);
        assert_eq!(
            serde_json::to_value(&a.water).unwrap(),
            serde_json::to_value(&b.water).unwrap()
        );
    }
    #[test]
    fn uniform_diagonal_flow_has_no_boundary_or_corner_source() {
        let g = FlatGrid::new(24, 12, 240000., 30., 0., 6371000.);
        let v = DVec3::new(1., 1., 0.).normalize();
        let winds = vec![(v, v.x, v.y); g.cell_count()];
        let transport = transfer_mesh(&g, &winds, 10., 200.);
        let water = vec![17.; g.cell_count()];
        let mut out = water.clone();
        advect(&water, &mut out, &transport, &water);
        for (i, value) in out.iter().enumerate() {
            assert!((value - 17.).abs() < 1e-9, "cell {i}: {value}");
        }
    }
    #[test]
    fn complete_closed_water_cycle_accounts_for_evaporation_and_runoff() {
        let g = SphereHexGrid::build(4, 6371000., (DVec3::Z, 0.));
        let n = g.cell_count();
        let winds: Vec<_> = (0..n)
            .map(|i| wind(&g, i, 0., &PlanetParams::default(), 0.5, false))
            .collect();
        let t = transfer_mesh(&g, &winds, 10., 600.);
        let mut v = vec![20.; n];
        let mut c = vec![0.; n];
        let mut soil = vec![30.; n];
        let mut nv = v.clone();
        let mut nc = c.clone();
        let zero = vec![0.; n];
        let before = (0..n)
            .map(|i| (v[i] + c[i] + soil[i]) * g.area_m2(i))
            .sum::<f64>();
        let mut exchange = 0.;
        for _ in 0..300 {
            advect(&v, &mut nv, &t, &zero);
            advect(&c, &mut nc, &t, &zero);
            for i in 0..n {
                let flux = water_step(
                    &mut nv[i],
                    &mut nc[i],
                    &mut soil[i],
                    30.,
                    if i % 3 == 0 { 1. } else { 0. },
                    if i % 2 == 0 { 120. } else { -120. },
                    0.02,
                    0.1,
                    600.,
                    &ClimateParams::default(),
                );
                exchange += (flux.evaporation - flux.runoff) * g.area_m2(i);
            }
            std::mem::swap(&mut v, &mut nv);
            std::mem::swap(&mut c, &mut nc);
        }
        let after = (0..n)
            .map(|i| (v[i] + c[i] + soil[i]) * g.area_m2(i))
            .sum::<f64>();
        assert!(((after - before - exchange) / before).abs() < 1e-10);
    }
    #[test]
    fn reconstruction_preserves_gradients_without_stencil_seams() {
        let g = MapGrid::Plane(FlatGrid::new(24, 12, 240000., 30., 179.9, 6371000.));
        let values: Vec<_> = (0..g.cell_count())
            .map(|i| {
                let p = g.field_position(i);
                1000. + p.x * 0.001 + p.y * 0.002
            })
            .collect();
        let MapGrid::Plane(flat) = &g else {
            unreachable!()
        };
        let rainfall = RainInterpolator::new(&g, &values);
        for x in -100..100 {
            let x = x as f64 * 600.;
            let y = 1234.;
            let actual = rainfall.sample(flat.direction_at(x, y));
            assert!(
                (actual - (1000. + x * 0.001 + y * 0.002)).abs() < 0.1,
                "{x}: {actual}"
            );
        }
    }
    #[test]
    fn rainfall_derivative_does_not_jump_at_an_atmospheric_triangle_edge() {
        let g = MapGrid::Plane(FlatGrid::new(24, 12, 240000., 30., 0., 6371000.));
        let values: Vec<_> = (0..g.cell_count())
            .map(|i| 100. + (i % 13) as f64 * 70.)
            .collect();
        let interpolation = RainInterpolator::new(&g, &values);
        let MapGrid::Plane(flat) = &g else {
            unreachable!()
        };
        let i = 5 * 24 + 10;
        let j = i + 1;
        let (x, y) = flat.xy(i);
        let (u, v) = flat.xy(j);
        let mx = (x + u) / 2.;
        let my = (y + v) / 2.;
        let sample = |offset| interpolation.sample(flat.direction_at(mx, my + offset));
        let left = sample(0.) - sample(-1.);
        let right = sample(1.) - sample(0.);
        assert!((left - right).abs() < 1e-4, "{left} vs {right}");
    }
    #[test]
    fn branching_transport_conserves_water_in_unequal_area_cells() {
        let t = vec![
            Transport {
                remaining: 0.4,
                edges: vec![(1, 0.15), (2, 0.6)],
                inflow: 0.,
            },
            Transport {
                remaining: 1.,
                ..Default::default()
            },
            Transport {
                remaining: 1.,
                ..Default::default()
            },
        ];
        let mut out = vec![0.; 3];
        advect(&[10., 0., 0.], &mut out, &t, &[0.; 3]);
        let volume = out[0] * 2. + out[1] * 4. + out[2];
        assert!((volume - 20.).abs() < 1e-10);
        assert!(out.iter().all(|x| *x >= 0.));
    }
    #[test]
    fn closed_spherical_transport_keeps_total_column_water() {
        let g = SphereHexGrid::build(4, 6371000., (DVec3::Z, 0.));
        let winds: Vec<_> = (0..g.cell_count())
            .map(|i| wind(&g, i, 0., &PlanetParams::default(), 0., false))
            .collect();
        let t = transfer_mesh(&g, &winds, 10., 600.);
        let mut water: Vec<_> = (0..g.cell_count()).map(|i| 1. + (i % 7) as f64).collect();
        let mut out = water.clone();
        let before = water
            .iter()
            .enumerate()
            .map(|(i, v)| v * g.area_m2(i))
            .sum::<f64>();
        for _ in 0..100 {
            advect(&water, &mut out, &t, &vec![0.; water.len()]);
            std::mem::swap(&mut water, &mut out);
        }
        let after = water
            .iter()
            .enumerate()
            .map(|(i, v)| v * g.area_m2(i))
            .sum::<f64>();
        assert!((after / before - 1.).abs() < 1e-10);
    }
}
