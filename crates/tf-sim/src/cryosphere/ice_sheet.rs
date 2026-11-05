//! Map-grid ice mass balance and sparse, conservative ice transport.
//! The atlas carries only provenance/budgets; physical ice lives in map channels.
use super::*;

#[derive(Clone, Copy)]
struct Weather {
    temperature: [f32; 12],
    precipitation: [f32; 12],
}

/// Reusable indexed scratch space; only active/touched cells are visited.
struct Fluxes {
    outgoing: Vec<f64>,
    delta: Vec<f64>,
    touched: Vec<usize>,
    marked: Vec<bool>,
    transfers: Vec<(usize, usize, f64)>,
    last_dt: f64,
}
impl Fluxes {
    fn new(n: usize) -> Self {
        Self {
            outgoing: vec![0.; n],
            delta: vec![0.; n],
            marked: vec![false; n],
            touched: Vec::new(),
            transfers: Vec::new(),
            last_dt: 0.,
        }
    }
    fn transport<G: GridOps>(
        &mut self,
        grid: &G,
        bed: &[f64],
        h: &mut [f64],
        active: &[usize],
        dt: f64,
        gravity: f64,
    ) -> &[usize] {
        for &i in &self.touched {
            self.delta[i] = 0.;
            self.marked[i] = false;
        }
        self.touched.clear();
        self.transfers.clear();
        for &i in active {
            self.outgoing[i] = 0.;
        }
        let surface = |i: usize| {
            if bed[i] < 0. && RHO_I * h[i] < RHO_O * (-bed[i]) {
                (1. - RHO_I / RHO_O) * h[i]
            } else {
                bed[i] + h[i]
            }
        };
        for &i in active {
            if h[i] < 0.01 {
                continue;
            }
            for &nb in grid.neighbors(i) {
                let j = nb as usize;
                // Positive-positive edges once, positive-empty edges from the
                // positive side, regardless of cell numbering.
                if j < i && h[j] >= 0.01 {
                    continue;
                }
                let (a, b) = if surface(i) > surface(j) {
                    (i, j)
                } else {
                    (j, i)
                };
                if h[a] < 0.01 {
                    continue;
                }
                let distance = grid.distance_m(a, b).max(1.);
                let slope = (surface(a) - surface(b)) / distance;
                let width = 4. * grid.area_m2(a) / (grid.neighbors(a).len() as f64 * distance);
                let floating = bed[a] < 0. && RHO_I * h[a] < RHO_O * (-bed[a]);
                let flux = if floating {
                    let stress = RHO_I * gravity * (1. - RHO_I / RHO_O) * h[a] / 4.;
                    let opening = ((surface(a) - surface(b))
                        / ((1. - RHO_I / RHO_O) * h[a]).max(0.1))
                    .clamp(0., 1.);
                    2.4e-24 * stress.powi(3) * distance * h[a] * width * dt * YEAR * opening
                } else {
                    // Upwind H avoids assigning downstream mobility to a
                    // nearly empty donor at the ice margin.
                    let mean = h[a];
                    let diffusivity = 2. * 2.4e-24 / 5.
                        * (RHO_I * gravity).powi(3)
                        * mean.powi(5)
                        * slope.powi(2);
                    diffusivity * slope * width * dt * YEAR
                };
                if flux > 0. {
                    self.transfers.push((a, b, flux));
                    self.outgoing[a] += flux;
                }
            }
        }
        self.last_dt = active
            .iter()
            .filter(|&&i| self.outgoing[i] > 0.)
            .fold(dt, |d, &i| {
                d.min(dt * 0.2 * h[i] * grid.area_m2(i) / self.outgoing[i])
            });
        for &(a, b, flux) in &self.transfers {
            let volume = flux * (self.last_dt / dt);
            for i in [a, b] {
                if !self.marked[i] {
                    self.marked[i] = true;
                    self.touched.push(i);
                }
            }
            self.delta[a] -= volume;
            self.delta[b] += volume;
        }
        for &i in &self.touched {
            h[i] = (h[i] + self.delta[i] / grid.area_m2(i)).max(0.);
        }
        &self.touched
    }
}

pub(super) fn solve<G: GridOps>(
    grid: &G,
    fields: &WorldFields,
    gravity: f64,
    cancel: &dyn Fn() -> bool,
) -> Option<Cryosphere> {
    let climate = fields.climate.as_ref()?;
    let climate_grid = climate.grid.build();
    let n = grid.cell_count();
    if fields.elevation.len() != n || !gravity.is_finite() || gravity <= 0. {
        return None;
    }
    let bed: Vec<f64> = fields.elevation.iter().map(|&h| h as f64).collect();
    let weather = |i: usize| {
        let (temperature, precipitation, _) = climate.sample(
            &climate_grid,
            grid.center_dir(i),
            fields.temperature[i],
            fields.precipitation[i],
        );
        Weather {
            temperature,
            precipitation,
        }
    };
    // Cache monthly forcing only for accumulation cells and cells reached by
    // flowing ice. Warm unglaciated terrain is not simulated for 1000 steps.
    let mut forcing = Vec::<Weather>::new();
    let mut slots = vec![usize::MAX; n];
    let mut active = Vec::new();
    let mut accumulating = vec![false; n];
    let mut present = vec![false; n];
    for i in 0..n {
        if i % 2048 == 0 && cancel() {
            return None;
        }
        if bed[i] <= 0. {
            continue;
        }
        let w = weather(i);
        if snow_cycle(&w.temperature, &w.precipitation, 0.).retained > 1e-8 {
            slots[i] = forcing.len();
            forcing.push(w);
            active.push(i);
            present[i] = true;
            accumulating[i] = true;
        }
    }
    let mut inverse = WorldFields {
        elevation: fields.elevation.iter().map(|&h| -h).collect(),
        distance_to_sea: vec![0.; n],
        ..Default::default()
    };
    crate::erosion::compute_distance_to_sea(grid, &mut inverse);
    let coast_distance = inverse.distance_to_sea;
    let mut h = vec![0.; n];
    let mut fluxes = Fluxes::new(n);
    let mut budget = IceBudget {
        solver_cells: n,
        ..Default::default()
    };
    let dt = 10.;
    let years = 10000;
    for step in 0..years / 10 {
        if cancel() {
            return None;
        }
        budget.peak_active_cells = budget.peak_active_cells.max(active.len());
        for &i in &active {
            if slots[i] == usize::MAX {
                slots[i] = forcing.len();
                forcing.push(weather(i));
            }
            let w = forcing[slots[i]];
            let mut t = w.temperature;
            let floating = bed[i] < 0. && RHO_I * h[i] < RHO_O * (-bed[i]);
            let rise = if floating {
                (1. - RHO_I / RHO_O) * h[i]
            } else {
                (bed[i] + h[i]).max(0.) - bed[i].max(0.)
            };
            for v in &mut t {
                *v -= (rise * 0.0065) as f32;
            }
            let cycle = snow_cycle(&t, &w.precipitation, h[i]);
            let coverage = if bed[i] <= 0. {
                (h[i] / 10.).clamp(0., 1.)
            } else {
                1.
            };
            let snow = cycle.retained.max(0.) / RHO_I * dt * coverage;
            let melt = (cycle.melted_ice / RHO_I * dt).min(h[i] + snow);
            h[i] += snow - melt;
            budget.snowfall_m3 += snow * grid.area_m2(i);
            budget.surface_melt_m3 += melt * grid.area_m2(i);
            if bed[i] < 0. && RHO_I * h[i] < RHO_O * (-bed[i]) {
                let warm = w.temperature.iter().map(|&v| v as f64).sum::<f64>() / 12.;
                let basal = (dt * (0.2 + warm.max(0.) * 1.5)).min(h[i]);
                h[i] -= basal;
                let residence = if warm > 0. {
                    20.
                } else {
                    4. + 496. * (-(coast_distance[i] as f64) / 300000.).exp()
                };
                let calved = h[i] * (1. - (-dt / residence).exp());
                h[i] -= calved;
                budget.basal_melt_m3 += basal * grid.area_m2(i);
                budget.calved_m3 += calved * grid.area_m2(i);
            }
            if grid.boundary_mask()[i] {
                let exported = h[i] * (1. - (-dt / 100.).exp());
                h[i] -= exported;
                budget.boundary_export_m3 += exported * grid.area_m2(i);
            }
        }
        let mut remaining = dt;
        while remaining > 1e-9 {
            if cancel() {
                return None;
            }
            fluxes.transport(grid, &bed, &mut h, &active, remaining, gravity);
            if !fluxes.last_dt.is_finite() || fluxes.last_dt <= 0. {
                return None;
            }
            remaining -= fluxes.last_dt;
            budget.transport_steps += 1;
            if budget.transport_steps > MAX_TRANSPORT_STEPS {
                return None;
            }
            for &i in &fluxes.touched {
                if !present[i] && h[i] > 0. {
                    active.push(i);
                    present[i] = true;
                }
            }
        }
        active.retain(|&i| {
            let keep = accumulating[i] || h[i] > 0.;
            present[i] = keep;
            keep
        });
        if step % 20 == 0 && active.iter().any(|&i| !h[i].is_finite()) {
            return None;
        }
    }
    let mut grounded = vec![0.; n];
    let mut shelf = vec![0.; n];
    for i in 0..n {
        budget.final_storage_m3 += h[i] * grid.area_m2(i);
        let value = h[i] as f32;
        budget.rounding_m3 += (value as f64 - h[i]) * grid.area_m2(i);
        if bed[i] < 0. && RHO_I * h[i] < RHO_O * (-bed[i]) {
            shelf[i] = value;
        } else {
            grounded[i] = value;
        }
    }
    budget.residual_m3 = budget.snowfall_m3
        - budget.surface_melt_m3
        - budget.basal_melt_m3
        - budget.calved_m3
        - budget.boundary_export_m3
        - budget.final_storage_m3;
    let atlas = CryosphereAtlas {
        model: MODEL.into(),
        grid: climate.grid.clone(),
        spinup_years: years,
        gravity_m_s2: gravity,
        grounded_ice_m: Vec::new(),
        shelf_ice_m: Vec::new(),
        sea_ice_m: Vec::new(),
        ice_budget: budget,
    };
    let result = sample(grid, fields, atlas, 0, Some((&grounded, &shelf)))?;
    // Independently integrate the actual published channels, not just the
    // solver work array. This includes marine-grounded ice as well as shelves.
    let stored = (0..n)
        .map(|i| (result.land_ice_m[i] as f64 + result.shelf_ice_m[i] as f64) * grid.area_m2(i))
        .sum::<f64>();
    let b = &result.atlas.ice_budget;
    if (stored - b.final_storage_m3 - b.rounding_m3).abs() > b.final_storage_m3.max(1.) * 1e-10
        || b.residual_m3.abs() > b.snowfall_m3.max(1.) * 1e-8
    {
        return None;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adaptive_sparse_flux_advances_full_time_and_conserves_volume() {
        let g = tf_grid::FlatGrid::new(12, 8, 120000., 65., 0., 6371000.);
        let n = g.cell_count();
        let mut h = vec![0.; n];
        h[n / 2] = 3000.;
        let bed = vec![200.; n];
        let active: Vec<_> = (0..n).collect();
        let before = (0..n).map(|i| h[i] * g.area_m2(i)).sum::<f64>();
        let mut flux = Fluxes::new(n);
        let mut remaining = 10.;
        let mut steps = 0;
        while remaining > 1e-9 {
            flux.transport(&g, &bed, &mut h, &active, remaining, 9.81);
            assert!(flux.last_dt > 0. && flux.last_dt <= remaining);
            remaining -= flux.last_dt;
            steps += 1;
            assert!(steps < 100000);
        }
        let after = (0..n).map(|i| h[i] * g.area_m2(i)).sum::<f64>();
        assert!((before - after).abs() < before * 1e-12);
        assert!(h.iter().all(|&v| v >= 0.));
        assert!(steps > 1);
        assert!(h.iter().enumerate().any(|(i, &v)| i != n / 2 && v > 0.));
    }
    #[test]
    fn sparse_level_shelves_have_no_cell_order_drift() {
        let g = tf_grid::FlatGrid::new(12, 8, 120000., 65., 0., 6371000.);
        let n = g.cell_count();
        let mut h = vec![100.; n];
        Fluxes::new(n).transport(
            &g,
            &vec![-500.; n],
            &mut h,
            &(0..n).collect::<Vec<_>>(),
            10.,
            9.81,
        );
        assert!(h.iter().all(|&v| v == 100.));
    }
}
