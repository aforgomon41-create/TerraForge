//! 追踪每个阶段后的陆地高程分布。
use tf_core::params::GenerationParams;
use tf_grid::SphereHexGrid;

fn report(label: &str, elev: &[f32]) {
    let mut s: Vec<f32> = elev.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = s.len();
    let land: Vec<f32> = s.iter().cloned().filter(|&v| v > 0.0).collect();
    let ln = land.len();
    if ln == 0 { println!("{:<16} 无陆地", label); return; }
    println!("{:<16} 陆地 {:>6} 格 | p10={:>6.0} p50={:>6.0} p90={:>6.0} max={:>6.0}",
        label, ln,
        land[ln/10], land[ln/2], land[ln*9/10], land[ln-1]);
    let _ = n;
}

fn main() {
    let mut params = GenerationParams::default();
    params.seed = 42;
    let grid = SphereHexGrid::build(128, params.planet.radius_m, (glam::DVec3::Z, 0.0));
    let mut rng = tf_core::DeterministicRng::from_seed(42);
    let mut fields = tf_sim::WorldFields::with_capacity(grid.cell_count());

    let (tectonic, plate_id) = tf_sim::build_plates(&grid, &params, &mut rng);
    tf_sim::plates::init_crust(&grid, &plate_id, &tectonic, &mut fields, &mut rng, &params);
    for _ in 0..8 { tf_sim::plates::step_tectonics(&grid, &tectonic, &mut fields, &params, 12.0); }
    tf_sim::plates::apply_airy_isostasy(&grid, &mut fields, &params);
    report("1.Airy均衡", &fields.elevation);

    let bounds = tf_sim::plates::find_boundaries(&grid, &plate_id, &tectonic, &fields);
    let lf = tf_sim::landforms::LandformParams::default();
    tf_sim::landforms::apply_boundary_landforms(&grid, &mut fields, &bounds, &lf);
    report("2.边界地貌", &fields.elevation);

    tf_sim::landforms::apply_volcanic_arcs(&grid, &mut fields, &bounds, &lf);
    report("3.火山弧", &fields.elevation);

    let hs = tf_sim::landforms::generate_hotspots(8, &mut rng);
    tf_sim::landforms::apply_hotspots(&grid, &mut fields, &hs, &lf);
    report("4.热点", &fields.elevation);

    tf_sim::plates::apply_seafloor_age(&grid, &mut fields, &tectonic, &bounds, 1.0);
    report("5.热沉降", &fields.elevation);

    tf_sim::erosion::compute_distance_to_sea(&grid, &mut fields);
    let mut ep = tf_sim::erosion::ErosionParams::default();
    ep.iterations = 67;
    tf_sim::erosion::run_erosion(&grid, &mut fields, &ep);
    report("6.侵蚀", &fields.elevation);
}