//! Linear wind-driven Stommel gyres, with coast walls and conservative heat fluxes.
//! This is a coarse, depth-uniform surface-ocean approximation, not MITgcm.
use glam::DVec3;
use serde::{Deserialize, Serialize};
use tf_core::params::PlanetParams;
use tf_grid::{GridOps, MapGrid};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CirculationAtlas {
    pub model: String,
    pub wind_east_m_s: Vec<Vec<f32>>,
    pub wind_north_m_s: Vec<Vec<f32>>,
    pub current_east_m_s: Vec<Vec<f32>>,
    pub current_north_m_s: Vec<Vec<f32>>,
    pub ocean_fraction: Vec<f32>,
    pub relative_solver_residual: Vec<f64>,
    pub solver_iterations: Vec<u32>,
    pub heat_coupled: bool,
    #[serde(default)]
    pub current_velocity_scale: Vec<f32>,
    /// Difference between the final water-transport wind and the lagged ocean forcing.
    #[serde(default)]
    pub forcing_wind_rms_difference_m_s: Vec<f64>,
}

fn frame(g: &MapGrid, i: usize) -> (DVec3, DVec3) {
    if g.field_is_planar() {
        (DVec3::X, DVec3::Y)
    } else {
        g.center_dir(i).tangent_frame()
    }
}

/// Neighbour least-squares gradient in local physical metres.
fn gradient(g: &MapGrid, values: &[f64], i: usize) -> DVec3 {
    let (e, n) = frame(g, i);
    let (mut xx, mut xy, mut yy, mut bx, mut by) = (0., 0., 0., 0., 0.);
    for &j in g.neighbors(i) {
        let j = j as usize;
        let d = g.field_position(j) - g.field_position(i);
        let (x, y) = (d.dot(e), d.dot(n));
        let w = 1. / d.length_squared().max(1.);
        xx += x * x * w;
        xy += x * y * w;
        yy += y * y * w;
        bx += (values[j] - values[i]) * x * w;
        by += (values[j] - values[i]) * y * w;
    }
    let det = xx * yy - xy * xy;
    if det < 1e-15 {
        return DVec3::ZERO;
    }
    e * ((yy * bx - xy * by) / det) + n * ((xx * by - xy * bx) / det)
}

pub fn solve(
    grid: &MapGrid,
    oceans: &[f64],
    winds: &[Vec<(DVec3, f64, f64)>],
    wind_speed: f64,
    planet: &PlanetParams,
    cancel: &dyn Fn() -> bool,
) -> Option<CirculationAtlas> {
    let count = grid.cell_count();
    let wet: Vec<_> = oceans.iter().map(|&o| o >= 0.6).collect();
    // Streamfunction is constant along every solid coast, including islands.
    let mut wall: Vec<_> = (0..count)
        .map(|i| {
            !wet[i]
                || grid.boundary_mask()[i]
                || grid.neighbors(i).iter().any(|&j| !wet[j as usize])
        })
        .collect();
    let unbounded_ocean = wall.iter().all(|w| !w);
    if unbounded_ocean {
        wall[0] = true;
    } // Streamfunction gauge, not an island.
    let drag = 1. / (30. * 86400.);
    let omega = std::f64::consts::TAU / (planet.rotation_period_hours * 3600.);
    let mut result = CirculationAtlas {
        model: "stommel-linear-wind-gyres+closed-coast-heat-flux-v1".into(),
        ocean_fraction: oceans.iter().map(|&o| o as f32).collect(),
        ..Default::default()
    };
    for wind in winds {
        if cancel() {
            return None;
        }
        result
            .wind_east_m_s
            .push(wind.iter().map(|v| (v.1 * wind_speed) as f32).collect());
        result
            .wind_north_m_s
            .push(wind.iter().map(|v| (v.2 * wind_speed) as f32).collect());
        let stress = |c: usize| -> Vec<f64> {
            wind.iter()
                .map(|v| {
                    let component = if c == 0 { v.1 } else { v.2 };
                    1.225
                        * planet.atmosphere_density
                        * 0.0013
                        * wind_speed.powi(2)
                        * component
                        * v.1.hypot(v.2)
                })
                .collect()
        };
        let (tx, ty) = (stress(0), stress(1));
        let mut rhs = vec![0.; count];
        let mut diag = vec![1.; count];
        let mut rows = vec![Vec::<(usize, f64)>::new(); count];
        for i in 0..count {
            if wall[i] {
                continue;
            }
            let (east, north) = frame(grid, i);
            let latitude = grid.latitude_deg(i).to_radians();
            let metric = if grid.field_is_planar() {
                0.
            } else {
                tx[i] * latitude.sin() / (grid.radius_m() * latitude.cos().max(0.001))
            };
            let curl =
                gradient(grid, &ty, i).dot(east) - gradient(grid, &tx, i).dot(north) + metric;
            rhs[i] = -curl / (1025. * 50.);
            diag[i] = 0.;
            let beta = 2. * omega * grid.latitude_deg(i).to_radians().cos() / grid.radius_m();
            // -r Laplacian(psi) - beta d(psi)/dx = -curl(tau)/(rho H).
            // Upwind beta transport resolves the western boundary on this mesh.
            for &j in grid.neighbors(i) {
                let j = j as usize;
                let d = grid.distance_m(i, j);
                let dx = (grid.field_position(j) - grid.field_position(i)).dot(east);
                let diffusion = drag * 4. / grid.neighbors(i).len() as f64 / d.powi(2);
                let advection =
                    (beta * dx / d.powi(2)).max(0.) * 4. / grid.neighbors(i).len() as f64;
                let a = diffusion + advection;
                diag[i] += a;
                rows[i].push((j, a));
            }
        }
        if unbounded_ocean {
            let area = (0..count)
                .filter(|&i| !wall[i])
                .map(|i| grid.area_m2(i))
                .sum::<f64>();
            let mean = (0..count)
                .filter(|&i| !wall[i])
                .map(|i| rhs[i] * grid.area_m2(i))
                .sum::<f64>()
                / area.max(1.);
            for i in 0..count {
                if !wall[i] {
                    rhs[i] -= mean;
                }
            }
        }
        let mut psi = vec![0.; count];
        let mut next = psi.clone();
        let mut iterations = 0;
        for iteration in 0..1600 {
            iterations = iteration + 1;
            if iteration % 32 == 0 && cancel() {
                return None;
            }
            let mut change = 0_f64;
            let mut size = 0_f64;
            for i in 0..count {
                if wall[i] {
                    continue;
                }
                next[i] = 0.25 * psi[i]
                    + 0.75 * (rhs[i] + rows[i].iter().map(|&(j, a)| a * psi[j]).sum::<f64>())
                        / diag[i];
                change = change.max((next[i] - psi[i]).abs());
                size = size.max(next[i].abs());
            }
            std::mem::swap(&mut psi, &mut next);
            if iteration > 64 && change < 1e-7 * size.max(1.) {
                break;
            }
        }
        let norm = rhs.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-30);
        let residual = (0..count)
            .filter(|&i| !wall[i])
            .map(|i| {
                (diag[i] * psi[i] - rows[i].iter().map(|&(j, a)| a * psi[j]).sum::<f64>() - rhs[i])
                    .powi(2)
            })
            .sum::<f64>()
            .sqrt()
            / norm;
        result.relative_solver_residual.push(residual);
        result.solver_iterations.push(iterations);
        let mut u = vec![0_f32; count];
        let mut v = u.clone();
        for i in 0..count {
            if !wet[i] {
                continue;
            }
            let (e, n) = frame(grid, i);
            let d = gradient(grid, &psi, i);
            u[i] = -d.dot(n) as f32;
            v[i] = d.dot(e) as f32;
        }
        // One global scale retains streamline structure; no per-cell clipping.
        let peak = u
            .iter()
            .zip(&v)
            .map(|(u, v)| u.hypot(*v))
            .fold(0_f32, f32::max);
        let scale = (2. / peak.max(2.)) as f32;
        result.current_velocity_scale.push(scale);
        result.forcing_wind_rms_difference_m_s.push(0.);
        for x in u.iter_mut().chain(&mut v) {
            *x *= scale;
        }
        result.current_east_m_s.push(u);
        result.current_north_m_s.push(v);
    }
    Some(result)
}

/// Closed finite-volume face transport, projected onto zero volume divergence.
/// The projection removes collocated-grid divergence before moving heat.
pub(crate) struct HeatTransport {
    edges: Vec<(usize, usize, f64)>,
    area_capacity: Vec<f64>,
    max_rate: f64,
}
impl HeatTransport {
    pub(crate) fn new(
        g: &MapGrid,
        ocean: &[f64],
        currents: &[(f64, f64)],
        capacity: &[f64],
    ) -> Self {
        let n = g.cell_count();
        let mut edges = Vec::new();
        let mut divergence = vec![0.; n];
        for i in 0..n {
            if ocean[i] < 0.6 {
                continue;
            }
            let (e, north) = frame(g, i);
            let vi = e * currents[i].0 + north * currents[i].1;
            for &j in g.neighbors(i) {
                let j = j as usize;
                if j <= i || ocean[j] < 0.6 {
                    continue;
                }
                let (e, north) = frame(g, j);
                let vj = e * currents[j].0 + north * currents[j].1;
                let d = g.distance_m(i, j);
                let face =
                    2. * (g.area_m2(i) * g.area_m2(j)).sqrt() / (3. * d) * ocean[i].min(ocean[j]);
                let q = (vi + vj).dot((g.field_position(j) - g.field_position(i)).normalize())
                    * 0.5
                    * face
                    * 50.;
                edges.push((i, j, face / d, q));
                divergence[i] += q;
                divergence[j] -= q;
            }
        }
        let apply = |x: &[f64]| {
            let mut y = vec![0.; n];
            for &(i, j, w, _) in &edges {
                let q = w * (x[i] - x[j]);
                y[i] += q;
                y[j] -= q;
            }
            y
        };
        let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
        let mut pressure = vec![0.; n];
        let mut residual = divergence;
        let mut direction = residual.clone();
        let mut norm = dot(&residual, &residual);
        let initial = norm;
        for _ in 0..(n * 2).min(4000) {
            if norm <= initial.max(1.) * 1e-20 {
                break;
            }
            let ad = apply(&direction);
            let denom = dot(&direction, &ad);
            if denom <= 1e-30 {
                break;
            }
            let alpha = norm / denom;
            for i in 0..n {
                pressure[i] += alpha * direction[i];
                residual[i] -= alpha * ad[i];
            }
            let next = dot(&residual, &residual);
            let beta = next / norm;
            for i in 0..n {
                direction[i] = residual[i] + beta * direction[i];
            }
            norm = next;
        }
        let mut outward = vec![0.; n];
        let edges: Vec<_> = edges
            .into_iter()
            .map(|(i, j, w, q)| {
                let heat = (q - w * (pressure[i] - pressure[j])) * 1025. * 4181.3;
                if heat > 0. {
                    outward[i] += heat;
                } else {
                    outward[j] -= heat;
                }
                (i, j, heat)
            })
            .collect();
        let area_capacity: Vec<_> = (0..n).map(|i| g.area_m2(i) * capacity[i]).collect();
        let max_rate = (0..n)
            .map(|i| outward[i] / area_capacity[i])
            .fold(0_f64, f64::max);
        Self {
            edges,
            area_capacity,
            max_rate,
        }
    }
    pub(crate) fn advect(&self, temperature: &mut [f64], duration_s: f64) {
        let steps = (duration_s * self.max_rate / 0.45).ceil().max(1.) as usize;
        let dt = duration_s / steps as f64;
        let mut delta = vec![0.; temperature.len()];
        for _ in 0..steps {
            delta.fill(0.);
            for &(i, j, q) in &self.edges {
                let heat = q
                    * dt
                    * if q >= 0. {
                        temperature[i]
                    } else {
                        temperature[j]
                    };
                delta[i] -= heat;
                delta[j] += heat;
            }
            for i in 0..temperature.len() {
                temperature[i] += delta[i] / self.area_capacity[i];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tf_grid::FlatGrid;
    #[test]
    fn closed_ocean_heat_transport_preserves_constant_temperature_and_total_heat() {
        let g = MapGrid::Plane(FlatGrid::new(18, 14, 3e6, 30., 0., 6371000.));
        let n = g.cell_count();
        let ocean: Vec<_> = (0..n)
            .map(|i| {
                if g.local_center_m(i).unwrap().0.abs() < 1e6 {
                    1.
                } else {
                    0.
                }
            })
            .collect();
        let currents: Vec<_> = (0..n).map(|i| (0.15, 0.10 * (i as f64).sin())).collect();
        let capacity = vec![2.1e8; n];
        let h = HeatTransport::new(&g, &ocean, &currents, &capacity);
        let mut uniform = vec![14.; n];
        h.advect(&mut uniform, 365.25 * 86400.);
        assert!(uniform.iter().all(|t| (*t - 14.).abs() < 1e-6));
        let mut t: Vec<_> = (0..n).map(|i| 5. + 20. * i as f64 / n as f64).collect();
        let original = t.clone();
        let heat = |t: &[f64]| {
            t.iter()
                .zip(&h.area_capacity)
                .map(|(t, c)| t * c)
                .sum::<f64>()
        };
        let before = heat(&t);
        h.advect(&mut t, 365.25 * 86400.);
        assert!((heat(&t) - before).abs() / before.abs() < 1e-12);
        assert!(t.iter().all(|t| *t >= 5. - 1e-6 && *t <= 25. + 1e-6));
        for i in 0..n {
            if ocean[i] == 0. {
                assert_eq!(t[i], original[i]);
            }
        }
    }
    #[test]
    fn wind_driven_gyre_reverses_when_forcing_reverses_and_never_flows_on_land() {
        let g = MapGrid::Plane(FlatGrid::new(22, 18, 6e6, 25., 0., 6371000.));
        let n = g.cell_count();
        let ocean: Vec<_> = (0..n)
            .map(|i| if g.boundary_mask()[i] { 0. } else { 1. })
            .collect();
        let wind: Vec<_> = (0..n)
            .map(|i| {
                let u = (g.local_center_m(i).unwrap().1 / 1e6).cos();
                (DVec3::X * u, u, 0.)
            })
            .collect();
        let reverse = wind.iter().map(|(v, u, n)| (-*v, -*u, -*n)).collect();
        let a = solve(&g, &ocean, &[wind], 10., &PlanetParams::default(), &|| {
            false
        })
        .unwrap();
        let b = solve(
            &g,
            &ocean,
            &[reverse],
            10.,
            &PlanetParams::default(),
            &|| false,
        )
        .unwrap();
        assert!(a.current_east_m_s[0]
            .iter()
            .chain(&a.current_north_m_s[0])
            .any(|v| v.abs() > 1e-4));
        for i in 0..n {
            assert!((a.current_east_m_s[0][i] + b.current_east_m_s[0][i]).abs() < 1e-6);
            if ocean[i] == 0. {
                assert_eq!(a.current_east_m_s[0][i], 0.);
                assert_eq!(a.current_north_m_s[0][i], 0.);
            }
        }
    }
}
