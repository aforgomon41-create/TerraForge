//! 检查原始高程分布是否有断层。
use tf_core::params::GenerationParams;
use tf_grid::SphereHexGrid;

fn main() {
    let mut params = GenerationParams::default();
    params.seed = 42;
    let grid = SphereHexGrid::build(128, params.planet.radius_m, (glam::DVec3::Z, 0.0));

    // 手动跑到 normalize 之前
    let mut rng = tf_core::DeterministicRng::from_seed(42);
    let mut fields = tf_sim::WorldFields::with_capacity(grid.cell_count());
    let (tectonic, plate_id) = tf_sim::build_plates(&grid, &params, &mut rng);
    tf_sim::plates::init_crust(&grid, &plate_id, &tectonic, &mut fields, &mut rng, &params);
    for _ in 0..8 { tf_sim::plates::step_tectonics(&grid, &tectonic, &mut fields, &params, 12.0); }
    tf_sim::plates::apply_airy_isostasy(&grid, &mut fields, &params);

    let mut s: Vec<f32> = fields.elevation.clone();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = s.len();
    println!("Airy 均衡后分布（共 {} 格）:", n);
    for p in [0.0, 0.5, 0.70, 0.705, 0.71, 0.715, 0.72, 0.75, 0.8, 0.9, 1.0] {
        println!("  p{:<6.1} {:>9.0} m", p * 100.0, s[((n - 1) as f64 * p) as usize]);
    }

    // 统计海平面附近的格子数
    let near_zero = s.iter().filter(|&&v| v.abs() < 100.0).count();
    println!();
    println!("海平面 ±100m 内的格子: {} ({:.2}%)", near_zero, near_zero as f64 / n as f64 * 100.0);
}