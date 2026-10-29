//! 验证子图与父图的一致性。
use tf_grid::{GridOps, SphereHexGrid, SphericalPatch, PatchGrid};

fn main() {
    // 模拟：父图在赤道附近有一块陆地，子图应该继承这个分布
    let parent = SphereHexGrid::build(128, 6_371_000.0, (glam::DVec3::Z, 0.0));

    // 造一个假的父图高程：赤道附近是陆，其余是海
    let n = parent.cell_count();
    let parent_elev: Vec<f32> = (0..n)
        .map(|i| {
            let lat = parent.latitude_deg(i);
            if lat.abs() < 20.0 { 1500.0 } else { -3000.0 }
        })
        .collect();

    let spec = SphericalPatch::new(0.0, 0.0, 2_000_000.0, 64);
    let patch = PatchGrid::new(&spec);

    let sample = tf_sim::downscale::sample_from_parent(
        &parent, &patch,
        &parent_elev,
        &vec![25.0; n],
        &vec![800.0; n],
    );

    let land = sample.is_land.iter().filter(|&&x| x).count();
    println!("子图格数 {}, 继承为陆地的 {}", patch.cell_count(), land);
    println!("陆地占比 {:.1}% （父图赤道 ±20 度是陆）",
             land as f64 / patch.cell_count() as f64 * 100.0);

    // 检查约束权重
    let w = tf_sim::downscale::constraint_weights(&patch);
    println!();
    println!("约束权重: 角落 {:.2}, 中心 {:.2}", w[0], w[(32*64+32) as usize]);

    // 模拟子图细化
    let mut elev = tf_sim::downscale::upsample_parent(&parent, &patch, &parent_elev);
    let before_land = elev.iter().filter(|&&e| e > 0.0).count();
    tf_sim::downscale::blend_with_parent(&patch, &mut elev, &sample, &w, true);
    let after_land = elev.iter().filter(|&&e| e > 0.0).count();

    println!();
    println!("blend 前陆地 {} 格, blend 后 {} 格", before_land, after_land);
    println!("（应该非常接近，说明陆地掩码生效）");
}