//! Controlled all-ocean benchmark, separate from terrain generation/calibration.
use glam::DVec3;
use tf_core::params::PlanetParams;
use tf_grid::{GridOps, MapGrid, SphereHexGrid};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "artifacts/climate-native-reference.json".into());
    let grid = MapGrid::Sphere(SphereHexGrid::build(12, 6371000., (DVec3::Z, 0.)));
    let n = grid.cell_count();
    // A warm, zero-obliquity ocean excludes ice feedback and seasonal differences,
    // isolating insolation, longwave radiation and spherical heat transport.
    let mut planet = PlanetParams::default();
    planet.axial_tilt_deg = 0.;
    let atlas = tf_sim::seasonal::energy(
        &grid,
        &vec![0.; n],
        &vec![1.; n],
        &planet,
        2.,
        None,
        &|| false,
    )
    .ok_or_else(|| std::io::Error::other("Native reference energy model failed"))?;
    let rows: Vec<_> = (0..n)
        .map(|i| {
            serde_json::json!({
                "latitude_deg": grid.latitude_deg(i),
                "area_m2": grid.area_m2(i),
                "temperature_c": atlas.temperature_c.iter().map(|m| m[i] as f64).sum::<f64>() / 12.,
            })
        })
        .collect();
    let p2: Vec<_> = (0..n)
        .map(|i| 0.5 * (3. * grid.center_dir(i).as_vec().y.powi(2) - 1.))
        .collect();
    let mut dissipation = 0.;
    for i in 0..n {
        for &j in grid.neighbors(i) {
            let j = j as usize;
            if j <= i {
                continue;
            }
            let d = grid.distance_m(i, j);
            let face = 2. * (grid.area_m2(i) * grid.area_m2(j)).sqrt() / (3. * d);
            dissipation += 0.55 * grid.radius_m().powi(2) * face / d * (p2[i] - p2[j]).powi(2);
        }
    }
    let eigenvalue = dissipation / (0..n).map(|i| grid.area_m2(i) * p2[i] * p2[i]).sum::<f64>();
    let result = serde_json::json!({
        "case": "zero-tilt warm homogeneous ocean", "model": atlas.model,
        "p2_diffusion_eigenvalue_w_m2_k": eigenvalue, "p2_expected_eigenvalue": 3.3,
        "parameters": {
            "A": 170., "B": 2., "D": 0.55, "solar_constant": planet.solar_constant,
            "albedo": 0.27, "water_depth_m": 50., "axial_tilt_deg": 0.,
        },
        "energy": atlas.energy, "cells": rows,
    });
    if let Some(parent) = std::path::Path::new(&output).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output, serde_json::to_vec_pretty(&result)?)?;
    println!("Native climate reference written: {output}");
    Ok(())
}
