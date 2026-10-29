//! 诊断地形视觉问题的数据根源。
use tf_core::params::GenerationParams;
use tf_grid::{GridOps, SphereHexGrid};

fn pct(v: &mut Vec<f32>, p: f64) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p) as usize]
}

fn main() {
    let mut params = GenerationParams::default();
    params.seed = 42;
    let grid = SphereHexGrid::build(128, params.planet.radius_m, (glam::DVec3::Z, 0.0));
    let world = tf_sim::generate(&grid, &params, &mut |_, _| {});
    let f = &world.fields;
    let n = grid.cell_count();

    // 1. 陆地高程分布
    let mut land: Vec<f32> = (0..n).filter(|&i| f.elevation[i] > 0.0).map(|i| f.elevation[i]).collect();
    println!("陆地 {} 格", land.len());
    for p in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 0.99, 1.0] {
        println!("  高程 p{:<4.0} {:>7.0} m", p * 100.0, pct(&mut land.clone(), p));
    }

    // 2. 陆地局部起伏（视觉细节的关键）
    let mut relief: Vec<f32> = Vec::new();
    for i in 0..n {
        if f.elevation[i] <= 0.0 { continue; }
        let nb = grid.neighbors(i);
        if nb.is_empty() { continue; }
        let mean = nb.iter().map(|&j| f.elevation[j as usize]).sum::<f32>() / nb.len() as f32;
        relief.push((f.elevation[i] - mean).abs());
    }
    println!();
    println!("陆地局部起伏（相对邻居）:");
    for p in [0.5, 0.75, 0.9, 0.99] {
        println!("  p{:<4.0} {:>6.1} m", p * 100.0, pct(&mut relief.clone(), p));
    }

    // 3. 生物群系破碎度
    let mut edges = 0;
    let mut total = 0;
    for i in 0..n {
        if f.elevation[i] <= 0.0 { continue; }
        for &nb in grid.neighbors(i) {
            if (nb as usize) > i && f.elevation[nb as usize] > 0.0 {
                total += 1;
                if world.ecology.biome[i] != world.ecology.biome[nb as usize] { edges += 1; }
            }
        }
    }
    println!();
    println!("生物群系边界占比: {:.1}%", edges as f64 / total.max(1) as f64 * 100.0);

    // 4. 各群系占比
    let mut counts = [0usize; 13];
    for i in 0..n { counts[world.ecology.biome[i] as usize] += 1; }
    let names = ["海洋","冰盖","苔原","针叶林","温带林","温带草原","地中海","沙漠","稀树草原","热带雨林","热带季雨林","高山草甸","湿地"];
    println!();
    for (k, &c) in counts.iter().enumerate() {
        if c > 0 && k != 0 {
            println!("  {:<10} {:>6.1}%", names[k], c as f64 / n as f64 * 100.0);
        }
    }
}