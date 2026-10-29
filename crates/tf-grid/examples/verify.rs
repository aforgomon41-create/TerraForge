//! 网格质量验证。
//!
//! 检查格数、五边形数、邻接对称性、面积均匀性、总面积守恒。
//! 这些是这个模块的验收标准。

use std::f64::consts::PI;
use tf_grid::SphereHexGrid;

fn check(freq: u32) {
    let radius = 6_371_000.0f64;
    let grid = SphereHexGrid::build(freq, radius, (glam::DVec3::Z, 0.0));
    let stats = grid.stats();
    let n = grid.cell_count();

    // 邻接对称性和邻居数
    let mut bad_adjacency = 0;
    let mut bad_neighbor_count = 0;
    for i in 0..n {
        let cnt = grid.neighbors(i).len();
        if cnt != 5 && cnt != 6 {
            bad_neighbor_count += 1;
        }
        for &nb in grid.neighbors(i) {
            if !grid.neighbors(nb as usize).contains(&(i as u32)) {
                bad_adjacency += 1;
            }
        }
    }

    // 最小邻居距离（查重复点）
    let mut min_dist = f64::INFINITY;
    for i in (0..n).step_by(97) {
        for &nb in grid.neighbors(i) {
            let d = grid.center(i).angle_to(&grid.center(nb as usize));
            min_dist = min_dist.min(d);
        }
    }
    let expected_edge = (4.0 * PI / n as f64).sqrt() * 1.0;

    println!("f={:<5} 格数={:<8} 五边形={:<3} 总面积误差={:+.6}%",
             freq, n, stats.pentagon_count, stats.total_area_error_pct);
    println!("       六边形面积 max/min={:.3}  格宽={:.2} km  CV={:.4}",
             stats.area_ratio, stats.cell_width_km,
             area_cv(&grid));
    println!("       邻居数异常={}  邻接不对称={}  最小邻居距离/理论={:.3}",
             bad_neighbor_count, bad_adjacency, min_dist / expected_edge);
    println!();
}

fn area_cv(grid: &SphereHexGrid) -> f64 {
    let areas: Vec<f64> = (0..grid.cell_count())
        .filter(|&i| !grid.is_pentagon(i))
        .map(|i| grid.area_m2(i))
        .collect();
    let mean: f64 = areas.iter().sum::<f64>() / areas.len() as f64;
    let var: f64 = areas.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / areas.len() as f64;
    var.sqrt() / mean
}

fn main() {
    println!("=== 球面六角格网格质量报告 ===\n");
    for freq in [16u32, 32, 64, 128, 256] {
        check(freq);
    }

    println!("=== 生产档位实测 ===\n");
    for (label, freq) in [("经济 Economy", 256u32), ("标准 Standard", 512), ("高精度 High", 768)] {
        let t = std::time::Instant::now();
        let grid = SphereHexGrid::build(freq, 6_371_000.0, (glam::DVec3::Z, 0.0));
        let build_ms = t.elapsed().as_millis();
        let s = grid.stats();
        println!("{} (f={})", label, freq);
        println!("  格数 {}  五边形 {}", s.cell_count, s.pentagon_count);
        println!("  格宽 {:.2} km  单格面积 {:.1} km²", s.cell_width_km, s.avg_cell_area_km2);
        println!("  构建耗时 {} ms  内存约 {:.1} MB",
                 build_ms, s.cell_count as f64 * 4.0 * 3.0 / 1e6);
        println!();
    }
}
