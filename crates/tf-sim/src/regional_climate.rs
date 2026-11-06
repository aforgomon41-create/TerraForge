//! Prescribed-background regional downscaling. Two steady advective cloud
//! delays redistribute unresolved orographic rain, constrained to parent moments.
//! This is not the full Smith-Barstad wave model or an independent atmosphere.
use crate::{seasonal::ClimateAtlas, WorldFields};
use glam::DVec3;
use serde::{Deserialize, Serialize};
use tf_grid::{GridDescriptor, GridOps, MapGrid};
const TAU: f64 = 1000.;
const MOIST_HEIGHT: f64 = 1000.;
const MAX_LOCAL_CELLS: usize = 16384;
const PARENT_MOMENT_TOLERANCE: f64 = 0.02;
type Weights = [(usize, f64); 3];
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostics {
    pub model: String,
    pub cells: usize,
    #[serde(default)]
    pub fine_cells: usize,
    #[serde(default)]
    pub precipitation_budget_domain: String,
    pub parent_model: String,
    pub wind_available: bool,
    pub conversion_seconds: f64,
    pub fallout_seconds: f64,
    pub moisture_height_m: f64,
    pub reference_elevation_m: Vec<f32>,
    pub baseline_precipitation_m3: [f64; 12],
    pub precipitation_m3: [f64; 12],
    pub maximum_parent_moment_error: [f64; 12],
    pub solver_relative_residual: [f64; 12],
    pub maximum_rain_factor: f64,
    pub constraint_grid: GridDescriptor,
    pub orographic_signal_retained: [f64; 12],
    pub baseline_fine_precipitation_m3: [f64; 12],
    pub fine_precipitation_m3: [f64; 12],
}
pub struct RegionalClimate {
    pub atlas: ClimateAtlas,
    pub annual_precipitation: Vec<f32>,
    pub diagnostics: Diagnostics,
}
fn frame(g: &MapGrid, i: usize) -> (DVec3, DVec3) {
    if g.field_is_planar() {
        (DVec3::X, DVec3::Y)
    } else {
        g.center_dir(i).tangent_frame()
    }
}
fn stencil(g: &MapGrid, dir: tf_core::geo::UnitVec3) -> Result<Weights, String> {
    let p = if let MapGrid::Plane(p) = g {
        let (x, y) = p.project(dir);
        DVec3::new(x, y, 0.)
    } else {
        dir.as_vec() * g.radius_m()
    };
    crate::evolution::stencil(g, p).ok_or_else(|| "局部气候超出背景气候范围".into())
}
fn gradient(g: &MapGrid, z: &[f64], i: usize) -> DVec3 {
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
        bx += (z[j] - z[i]) * x * w;
        by += (z[j] - z[i]) * y * w;
    }
    let det = xx * yy - xy * xy;
    if det < 1e-15 {
        DVec3::ZERO
    } else {
        e * ((yy * bx - xy * by) / det) + n * ((xx * by - xy * bx) / det)
    }
}
fn atmospheric_descriptor(d: &GridDescriptor) -> Result<GridDescriptor, String> {
    let mut a = d.clone();
    let (cols, rows) = match d {
        GridDescriptor::Plane { size, rows, .. } => (*size, *rows),
        GridDescriptor::Patch { size, .. } => (*size, *size),
        _ => return Err("局部气候需要有限地图".into()),
    };
    let factor = ((cols as f64 * rows as f64 / MAX_LOCAL_CELLS as f64)
        .sqrt()
        .ceil() as u32)
        .max(1)
        .min((cols.min(rows) / 4).max(1));
    match &mut a {
        GridDescriptor::Plane { size, rows: r, .. } => {
            *size = (cols / factor).max(4);
            *r = (rows / factor).max(4);
        }
        GridDescriptor::Patch { size, .. } => *size = (cols / factor).max(4),
        _ => {}
    }
    a.validate()?;
    if a.cell_count() > crate::seasonal::MAX_ATLAS_CELLS {
        return Err("局部气候网格超过预算".into());
    }
    Ok(a)
}
/// Circulation is a derived remap, so regional saves need not duplicate all
/// four monthly vector fields. Use the same operation before and after reload.
pub fn remap_circulation(
    parent: &ClimateAtlas,
    descriptor: &GridDescriptor,
    height: &[f32],
) -> Result<Option<crate::circulation::CirculationAtlas>, String> {
    parent.validate()?;
    let Some(p) = &parent.circulation else {
        return Ok(None);
    };
    let source = parent.grid.build();
    let g = descriptor.build();
    let n = g.cell_count();
    if height.len() != n || height.iter().any(|h| !h.is_finite()) {
        return Err("局部风场地形尺寸无效".into());
    }
    let mut out = crate::circulation::CirculationAtlas {
        model: format!("regional remap of {}", p.model),
        wind_east_m_s: vec![vec![0.; n]; 12],
        wind_north_m_s: vec![vec![0.; n]; 12],
        current_east_m_s: vec![vec![0.; n]; 12],
        current_north_m_s: vec![vec![0.; n]; 12],
        ocean_fraction: height
            .iter()
            .map(|&h| if h <= 0. { 1. } else { 0. })
            .collect(),
        relative_solver_residual: vec![0.; 12],
        solver_iterations: vec![0; 12],
        heat_coupled: false,
        current_velocity_scale: vec![1.; 12],
        forcing_wind_rms_difference_m_s: vec![0.; 12],
    };
    for i in 0..n {
        let weights = stencil(&source, g.center_dir(i))?;
        let (e, north) = frame(&g, i);
        for m in 0..12 {
            let mut wind = DVec3::ZERO;
            let mut current = DVec3::ZERO;
            for &(j, a) in &weights {
                let (se, sn) = frame(&source, j);
                wind +=
                    (se * p.wind_east_m_s[m][j] as f64 + sn * p.wind_north_m_s[m][j] as f64) * a;
                current += (se * p.current_east_m_s[m][j] as f64
                    + sn * p.current_north_m_s[m][j] as f64)
                    * a;
            }
            out.wind_east_m_s[m][i] = wind.dot(e) as f32;
            out.wind_north_m_s[m][i] = wind.dot(north) as f32;
            if out.ocean_fraction[i] > 0. {
                out.current_east_m_s[m][i] = current.dot(e) as f32;
                out.current_north_m_s[m][i] = current.dot(north) as f32;
            }
        }
    }
    Ok(Some(out))
}
struct Advection {
    incoming: Vec<(usize, f64)>,
    diagonal: f64,
}
fn transport(g: &MapGrid, wind: &[DVec3]) -> Vec<Advection> {
    (0..g.cell_count())
        .map(|i| {
            let p = g.field_position(i);
            let mut incoming = vec![];
            let (mut ins, mut outs) = (0., 0.);
            for &j in g.neighbors(i) {
                let j = j as usize;
                let d = g.field_position(j) - p;
                let face = (g.area_m2(i) + g.area_m2(j)) / (3. * g.distance_m(i, j).max(1.));
                let speed = (wind[i] + wind[j]).dot(d.normalize_or_zero()) * 0.5;
                let rate = speed * face / g.area_m2(i);
                if rate < 0. {
                    incoming.push((j, -rate * TAU));
                    ins -= rate * TAU;
                } else {
                    outs += rate * TAU;
                }
            }
            let boundary = if g.boundary_mask()[i] {
                (outs - ins).max(0.)
            } else {
                0.
            };
            Advection {
                incoming,
                diagonal: 1. + ins + boundary,
            }
        })
        .collect()
}
fn delayed(
    source: &[f64],
    mesh: &[Advection],
    order: &[usize],
    cancel: &dyn Fn() -> bool,
) -> Result<(Vec<f64>, f64), String> {
    let mut value = vec![0.; source.len()];
    let scale = source.iter().map(|x| x.abs()).fold(1e-12, f64::max);
    let mut residual = f64::INFINITY;
    for sweep in 0..256 {
        if cancel() {
            return Err("局部气候已取消".into());
        }
        for &i in order {
            value[i] = (source[i]
                + mesh[i]
                    .incoming
                    .iter()
                    .map(|&(j, a)| a * value[j])
                    .sum::<f64>())
                / mesh[i].diagonal;
        }
        if sweep % 4 == 0 {
            residual = (0..value.len())
                .map(|i| {
                    (value[i] * mesh[i].diagonal
                        - source[i]
                        - mesh[i]
                            .incoming
                            .iter()
                            .map(|&(j, a)| a * value[j])
                            .sum::<f64>())
                    .abs()
                        / scale
                })
                .fold(0., f64::max);
            if residual < 1e-7 {
                break;
            }
        }
    }
    if residual > 1e-4 {
        return Err("局部云雨平流未收敛".into());
    }
    Ok((value, residual))
}
/// Smooth maximum-entropy moment adjustment; unlike cellwise renormalisation,
/// triangle weights do not paint the parent's cell boundaries into rainfall.
fn constrain(
    weights: &[Weights],
    area: &[f64],
    base: &[f64],
    rain: &mut [f64],
    anchors: usize,
    cancel: &dyn Fn() -> bool,
) -> Result<(f64, f64), String> {
    let mut target = vec![0.; anchors];
    for i in 0..base.len() {
        for &(j, w) in &weights[i] {
            target[j] += area[i] * w * base[i];
        }
    }
    let total = base.iter().zip(area).map(|(p, a)| p * a).sum::<f64>();
    if total == 0. {
        rain.fill(0.);
        return Ok((0., 1.));
    }
    for _ in 0..128 {
        if cancel() {
            return Err("局部降水约束已取消".into());
        }
        let mut current = vec![0.; anchors];
        for i in 0..rain.len() {
            for &(j, w) in &weights[i] {
                current[j] += area[i] * w * rain[i];
            }
        }
        let error = (0..anchors)
            .filter(|&j| target[j] > total * 1e-10)
            .map(|j| (current[j] - target[j]).abs() / target[j])
            .fold(0., f64::max);
        if error < PARENT_MOMENT_TOLERANCE * 0.25 {
            break;
        }
        let update: Vec<_> = (0..anchors)
            .map(|j| {
                if target[j] > 0. && current[j] > 0. {
                    (target[j] / current[j]).ln() * 0.75
                } else {
                    0.
                }
            })
            .collect();
        for i in 0..rain.len() {
            rain[i] *= weights[i]
                .iter()
                .map(|&(j, w)| w * update[j])
                .sum::<f64>()
                .exp();
        }
    }
    let actual = rain.iter().zip(area).map(|(p, a)| p * a).sum::<f64>();
    let scale = total / actual;
    for p in rain.iter_mut() {
        *p *= scale;
    }
    let mut current = vec![0.; anchors];
    for i in 0..rain.len() {
        for &(j, w) in &weights[i] {
            current[j] += area[i] * w * rain[i];
        }
    }
    let mut error = (0..anchors)
        .filter(|&j| target[j] > total * 1e-10)
        .map(|j| (current[j] - target[j]).abs() / target[j])
        .fold(0., f64::max);
    // If the constraints leave too little room for a new local signal, reduce
    // that signal toward the feasible background rather than inventing water.
    let retained = if error > PARENT_MOMENT_TOLERANCE {
        PARENT_MOMENT_TOLERANCE / error
    } else {
        1.
    };
    if retained < 1. {
        for i in 0..rain.len() {
            rain[i] = base[i] + retained * (rain[i] - base[i]);
        }
        error *= retained;
    }
    Ok((error, retained))
}
pub fn refine(
    parent: &ClimateAtlas,
    parent_grid: &MapGrid,
    parent_height: &[f32],
    source_reference: Option<&[f32]>,
    fine_grid: &MapGrid,
    descriptor: &GridDescriptor,
    fine: &WorldFields,
    strength: f64,
    cancel: &dyn Fn() -> bool,
) -> Result<RegionalClimate, String> {
    parent.validate()?;
    if parent_height.len() != parent_grid.cell_count()
        || fine.elevation.len() != fine_grid.cell_count()
        || fine.precipitation.len() != fine_grid.cell_count()
        || fine.temperature.len() != fine_grid.cell_count()
        || fine.precipitation.iter().any(|v| !v.is_finite() || *v < 0.)
        || !strength.is_finite()
    {
        return Err("局部气候地形尺寸无效".into());
    }
    let source = parent.grid.build();
    let mut constraint_descriptor = parent.grid.clone();
    match &mut constraint_descriptor {
        GridDescriptor::Sphere { frequency, .. } => *frequency = (*frequency / 2).max(4),
        GridDescriptor::Plane { size, rows, .. } => {
            *size = (*size / 2).max(4);
            *rows = (*rows / 2).max(4);
        }
        GridDescriptor::Patch { size, .. } => *size = (*size / 2).max(4),
    }
    let constraint_grid = constraint_descriptor.build();
    let d = atmospheric_descriptor(descriptor)?;
    let g = d.build();
    let n = g.cell_count();
    let areas: Vec<_> = (0..n).map(|i| g.area_m2(i)).collect();
    let weights: Vec<_> = (0..n)
        .map(|i| stencil(&source, g.center_dir(i)))
        .collect::<Result<_, _>>()?;
    let constraint_weights: Vec<_> = (0..n)
        .map(|i| stencil(&constraint_grid, g.center_dir(i)))
        .collect::<Result<_, _>>()?;
    let h = crate::downscale::interpolate_from_grid(fine_grid, &g, &fine.elevation)?;
    let temp = crate::downscale::interpolate_from_grid(fine_grid, &g, &fine.temperature)?;
    let annual = crate::downscale::interpolate_from_grid(fine_grid, &g, &fine.precipitation)?;
    let reference: Vec<Option<f32>> = if let Some(z) = source_reference {
        if z.len() != source.cell_count() || z.iter().any(|v| !v.is_finite()) {
            return Err("背景气候地形尺寸无效".into());
        }
        z.iter().map(|&v| Some(v)).collect()
    } else {
        (0..source.cell_count())
            .map(|i| crate::downscale::sample_at(parent_grid, parent_height, source.center_dir(i)))
            .collect()
    };
    let unresolved: Vec<_> = (0..n)
        .map(|i| {
            let (mut z, mut sum) = (0., 0.);
            for &(j, w) in &weights[i] {
                if let Some(v) = reference[j] {
                    z += v as f64 * w;
                    sum += w;
                }
            }
            if sum > 0.5 {
                h[i] as f64 - z / sum
            } else {
                0.
            }
        })
        .collect();
    let gradients: Vec<_> = (0..n).map(|i| gradient(&g, &unresolved, i)).collect();
    let weather: Vec<_> = (0..n)
        .map(|i| parent.sample(&source, g.center_dir(i), temp[i], annual[i]))
        .collect();
    let mut atlas = ClimateAtlas {
        grid: d,
        model: "parent-constrained-regional-cloud-delay-v1".into(),
        axial_tilt_deg: parent.axial_tilt_deg,
        temperature_c: vec![vec![0.; n]; 12],
        precipitation_mm: vec![vec![0.; n]; 12],
        pet_mm: vec![vec![0.; n]; 12],
        energy: Default::default(),
        water: Default::default(),
        circulation: None,
    };
    atlas.energy.temperature_basis =
        "prescribed parent seasonal thermal background; no independent regional energy balance"
            .into();
    atlas.water.prescribed_output_scale = Some(1.);
    let mut diagnostics = Diagnostics {
        model: atlas.model.clone(),
        cells: n,
        fine_cells: fine_grid.cell_count(),
        precipitation_budget_domain:
            "fine solver domain including any halo; output crop is an open subdomain".into(),
        parent_model: parent.model.clone(),
        wind_available: parent.circulation.is_some(),
        conversion_seconds: TAU,
        fallout_seconds: TAU,
        moisture_height_m: MOIST_HEIGHT,
        reference_elevation_m: h,
        baseline_precipitation_m3: [0.; 12],
        precipitation_m3: [0.; 12],
        maximum_parent_moment_error: [0.; 12],
        solver_relative_residual: [0.; 12],
        maximum_rain_factor: 1.,
        constraint_grid: constraint_descriptor,
        orographic_signal_retained: [1.; 12],
        baseline_fine_precipitation_m3: [0.; 12],
        fine_precipitation_m3: [0.; 12],
    };
    let circulation = remap_circulation(parent, &atlas.grid, &diagnostics.reference_elevation_m)?;
    let mut baseline_annual = vec![0.; n];
    for m in 0..12 {
        if cancel() {
            return Err("局部气候已取消".into());
        }
        let mut wind = vec![DVec3::ZERO; n];
        if let Some(c) = &circulation {
            for i in 0..n {
                let (e, north) = frame(&g, i);
                wind[i] = e * c.wind_east_m_s[m][i] as f64 + north * c.wind_north_m_s[m][i] as f64;
            }
        }
        let base: Vec<_> = weather.iter().map(|w| w.1[m] as f64).collect();
        let forcing: Vec<_> = (0..n)
            .map(|i| {
                base[i]
                    * (strength.clamp(0., 3.) * wind[i].dot(gradients[i]) * TAU / MOIST_HEIGHT)
                        .clamp(-0.75, 3.)
            })
            .collect();
        let mean = wind.iter().copied().sum::<DVec3>() / n as f64;
        let mut order: Vec<_> = (0..n).collect();
        order.sort_by(|&a, &b| {
            g.field_position(a)
                .dot(mean)
                .total_cmp(&g.field_position(b).dot(mean))
                .then(a.cmp(&b))
        });
        let mesh = transport(&g, &wind);
        let (cloud, r1) = delayed(&forcing, &mesh, &order, cancel)?;
        let (anomaly, r2) = delayed(&cloud, &mesh, &order, cancel)?;
        let mut rain: Vec<_> = (0..n)
            .map(|i| (base[i] + anomaly[i]).clamp(base[i] * 0.25, base[i] * 4.))
            .collect();
        let (error, retained) = constrain(
            &constraint_weights,
            &areas,
            &base,
            &mut rain,
            constraint_grid.cell_count(),
            cancel,
        )?;
        diagnostics.maximum_parent_moment_error[m] = error;
        diagnostics.orographic_signal_retained[m] = retained;
        diagnostics.solver_relative_residual[m] = r1.max(r2);
        for i in 0..n {
            atlas.temperature_c[m][i] = weather[i].0[m];
            atlas.precipitation_mm[m][i] = rain[i] as f32;
            atlas.pet_mm[m][i] = weather[i].2[m];
            baseline_annual[i] += base[i];
            diagnostics.maximum_rain_factor = diagnostics
                .maximum_rain_factor
                .max(if base[i] > 0. { rain[i] / base[i] } else { 1. });
            diagnostics.baseline_precipitation_m3[m] += base[i] * areas[i] / 1000.;
            diagnostics.precipitation_m3[m] += rain[i] * areas[i] / 1000.;
        }
    }
    atlas.circulation = circulation;
    let fine_weights: Vec<_> = (0..fine_grid.cell_count())
        .map(|i| stencil(&g, fine_grid.center_dir(i)))
        .collect::<Result<_, _>>()?;
    let mut coefficient = vec![0.; fine_grid.cell_count()];
    let mut actual = [0.; 12];
    for i in 0..fine_grid.cell_count() {
        if i % 2048 == 0 && cancel() {
            return Err("局部气候已取消".into());
        }
        let old = fine_weights[i]
            .iter()
            .map(|&(j, w)| w * baseline_annual[j])
            .sum::<f64>();
        coefficient[i] = if old > 1e-8 {
            fine.precipitation[i] as f64 / old
        } else {
            0.
        };
        let background = parent
            .sample(
                &source,
                fine_grid.center_dir(i),
                fine.temperature[i],
                fine.precipitation[i],
            )
            .1;
        for m in 0..12 {
            diagnostics.baseline_fine_precipitation_m3[m] +=
                background[m] as f64 * fine_grid.area_m2(i) / 1000.;
            actual[m] += coefficient[i]
                * fine_weights[i]
                    .iter()
                    .map(|&(j, w)| w * atlas.precipitation_mm[m][j] as f64)
                    .sum::<f64>()
                * fine_grid.area_m2(i)
                / 1000.;
        }
    }
    // Keep the actual published fine map's monthly water, not merely an
    // atmospheric quadrature total. Scaling is global and introduces no seams.
    for m in 0..12 {
        let target = diagnostics.baseline_fine_precipitation_m3[m];
        let scale = if actual[m] > 0. {
            target / actual[m]
        } else if target <= 1e-8 {
            1.
        } else {
            return Err("局部气候未解析到背景降水".into());
        };
        for p in &mut atlas.precipitation_mm[m] {
            *p = (*p as f64 * scale) as f32;
        }
        let mut target = vec![0.; constraint_grid.cell_count()];
        let mut current = vec![0.; constraint_grid.cell_count()];
        for i in 0..n {
            let base = weather[i].1[m] as f64;
            let rain = atlas.precipitation_mm[m][i] as f64;
            for &(j, w) in &constraint_weights[i] {
                target[j] += areas[i] * w * base;
                current[j] += areas[i] * w * rain;
            }
            if base > 0. {
                diagnostics.maximum_rain_factor = diagnostics.maximum_rain_factor.max(rain / base);
            }
        }
        let total = target.iter().sum::<f64>();
        diagnostics.maximum_parent_moment_error[m] = (0..target.len())
            .filter(|&j| target[j] > total * 1e-10)
            .map(|j| (current[j] - target[j]).abs() / target[j])
            .fold(0., f64::max);
        diagnostics.precipitation_m3[m] = (0..n)
            .map(|i| atlas.precipitation_mm[m][i] as f64 * areas[i] / 1000.)
            .sum();
    }
    let mut annual_precipitation = vec![0.; fine_grid.cell_count()];
    for i in 0..fine_grid.cell_count() {
        let mut annual = 0.;
        for m in 0..12 {
            let p = coefficient[i]
                * fine_weights[i]
                    .iter()
                    .map(|&(j, w)| w * atlas.precipitation_mm[m][j] as f64)
                    .sum::<f64>();
            annual += p;
            diagnostics.fine_precipitation_m3[m] += p * fine_grid.area_m2(i) / 1000.;
        }
        annual_precipitation[i] = annual as f32;
    }
    atlas.water.precipitation_m3 = diagnostics.fine_precipitation_m3.iter().sum();
    atlas.validate()?;
    Ok(RegionalClimate {
        atlas,
        annual_precipitation,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (
        ClimateAtlas,
        MapGrid,
        Vec<f32>,
        MapGrid,
        GridDescriptor,
        WorldFields,
    ) {
        let pd = GridDescriptor::plane(30., 179.9, 96000., 8, 8, 6371000.);
        let p = pd.build();
        let n = p.cell_count();
        let c = crate::circulation::CirculationAtlas {
            model: "fixture east wind".into(),
            wind_east_m_s: vec![vec![10.; n]; 12],
            wind_north_m_s: vec![vec![0.; n]; 12],
            current_east_m_s: vec![vec![0.; n]; 12],
            current_north_m_s: vec![vec![0.; n]; 12],
            ocean_fraction: vec![0.; n],
            relative_solver_residual: vec![0.; 12],
            solver_iterations: vec![0; 12],
            heat_coupled: false,
            ..Default::default()
        };
        let a = ClimateAtlas {
            grid: pd,
            model: "fixture background".into(),
            axial_tilt_deg: 23.44,
            temperature_c: vec![vec![12.; n]; 12],
            precipitation_mm: vec![vec![1000. / 12.; n]; 12],
            pet_mm: vec![vec![50.; n]; 12],
            energy: Default::default(),
            water: Default::default(),
            circulation: Some(c),
        };
        let d = GridDescriptor::plane(30., 179.9, 72000., 96, 96, 6371000.);
        let g = d.build();
        let mut f = WorldFields::with_capacity(g.cell_count());
        f.elevation.fill(0.);
        f.temperature.fill(12.);
        f.precipitation.fill(1000.);
        (a, p, vec![0.; n], g, d, f)
    }
    #[test]
    fn flat_parent_climate_is_preserved_and_zero_rain_month_stays_dry() {
        let (a, p, h, g, d, mut f) = fixture();
        let out = refine(&a, &p, &h, None, &g, &d, &f, 1., &|| false).unwrap();
        assert!(out
            .annual_precipitation
            .iter()
            .all(|&v| (v - 1000.).abs() < 0.001));
        assert!(out
            .diagnostics
            .maximum_parent_moment_error
            .iter()
            .all(|&e| e < 1e-6));
        let mut a = a;
        for v in &mut a.precipitation_mm[0] {
            *v = 0.;
        }
        for i in 0..g.cell_count() {
            let (x, _) = g.local_center_m(i).unwrap();
            f.elevation[i] = (1500. * (-0.5 * (x / 8000.).powi(2)).exp()) as f32;
        }
        let out = refine(&a, &p, &h, None, &g, &d, &f, 1., &|| false).unwrap();
        assert!(out.atlas.precipitation_mm[0].iter().all(|&p| p == 0.));
    }
    #[test]
    fn subgrid_ridge_changes_rain_without_changing_monthly_parent_water() {
        let (a, p, h, g, d, mut f) = fixture();
        for i in 0..g.cell_count() {
            let (x, _) = g.local_center_m(i).unwrap();
            f.elevation[i] = (1500. * (-0.5 * (x / 8000.).powi(2)).exp()) as f32;
        }
        let out = refine(&a, &p, &h, None, &g, &d, &f, 1., &|| false).unwrap();
        let range = out
            .annual_precipitation
            .iter()
            .copied()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                (lo.min(v), hi.max(v))
            });
        assert!(range.1 - range.0 > 20., "{range:?}");
        for m in 0..12 {
            let b = out.diagnostics.baseline_fine_precipitation_m3[m];
            assert!((out.diagnostics.fine_precipitation_m3[m] - b).abs() < b * 1e-6);
            assert!(out.diagnostics.maximum_parent_moment_error[m] < 0.01);
        }
        let climate_grid = out.atlas.grid.build();
        let mut observed = [0.; 12];
        for i in 0..g.cell_count() {
            let rain = out
                .atlas
                .sample(
                    &climate_grid,
                    g.center_dir(i),
                    f.temperature[i],
                    out.annual_precipitation[i],
                )
                .1;
            for m in 0..12 {
                observed[m] += rain[m] as f64 * g.area_m2(i) / 1000.;
            }
        }
        for m in 0..12 {
            let target = out.diagnostics.baseline_fine_precipitation_m3[m];
            assert!(
                (observed[m] - target).abs() < target * 2e-6,
                "published month {m}: {} != {target}",
                observed[m]
            );
        }
        assert!(out
            .annual_precipitation
            .iter()
            .all(|&v| v.is_finite() && v >= 0.));
        let mut reverse = a.clone();
        reverse
            .circulation
            .as_mut()
            .unwrap()
            .wind_east_m_s
            .iter_mut()
            .flatten()
            .for_each(|v| *v = -*v);
        let other = refine(&reverse, &p, &h, None, &g, &d, &f, 1., &|| false).unwrap();
        let difference = out
            .annual_precipitation
            .iter()
            .zip(&other.annual_precipitation)
            .map(|(a, b)| (a - b).abs() as f64)
            .sum::<f64>()
            / g.cell_count() as f64;
        assert!(difference > 5., "wind has no effect: {difference}");
    }
    #[test]
    fn cancellation_and_invalid_reference_do_not_publish_partial_climate() {
        let (a, p, h, g, d, mut f) = fixture();
        assert!(refine(&a, &p, &h, None, &g, &d, &f, 1., &|| true).is_err());
        assert!(refine(&a, &p, &h, Some(&[0.]), &g, &d, &f, 1., &|| false).is_err());
        f.precipitation[0] = -1.;
        assert!(refine(&a, &p, &h, None, &g, &d, &f, 1., &|| false).is_err());
    }
}
