//! 服务端地图渲染。
//!
//! 把物理场画成 PNG，用于导出。
//! 这里不追求实时性，追求质量：用更精细的着色和抗锯齿。

/// Render from the recorded geometry; a globe exports as a 2:1 atlas.
pub fn render_map_png(
    elevation: &[f32],
    biome: &[f32],
    descriptor: &tf_grid::GridDescriptor,
    width: u32,
) -> anyhow::Result<Vec<u8>> {
    render_map_png_with_ice(elevation, biome, descriptor, width, None)
}

/// Optional saved ice appearance, without changing bedrock elevation colors.
pub fn render_map_png_with_ice(
    elevation: &[f32],
    biome: &[f32],
    descriptor: &tf_grid::GridDescriptor,
    width: u32,
    ice: Option<&[Vec<f32>]>,
) -> anyhow::Result<Vec<u8>> {
    use std::io::Write;
    use tf_grid::{GridOps, MapGrid};
    anyhow::ensure!(
        elevation.len() == descriptor.cell_count() && biome.len() == elevation.len(),
        "字段与网格尺寸不一致"
    );
    anyhow::ensure!((16..=8192).contains(&width), "导出尺寸无效");
    if let Some(ice) = ice {
        anyhow::ensure!(
            ice.len() == 8 && ice.iter().all(|c| c.len() == elevation.len()),
            "冰雪导出字段长度无效"
        );
    }
    let grid = descriptor.build();
    let height = match &grid {
        MapGrid::Sphere(_) => width / 2,
        MapGrid::Plane(g) => (width as f64 * g.height() / g.width())
            .round()
            .clamp(1., 8192.) as u32,
        MapGrid::Patch(_) => (width as f64 * 3_f64.sqrt() / 2.).round() as u32,
    };
    let colors: Vec<_> = (0..elevation.len())
        .map(|i| {
            let mut color = color_for(elevation[i], biome[i] as u8);
            if elevation[i] > 0. {
                let (east, north) = grid.center_dir(i).tangent_frame();
                let mut slope = 0.;
                for &j in grid.neighbors(i) {
                    let j = j as usize;
                    let d = grid.center_dir(j).as_vec() - grid.center_dir(i).as_vec();
                    let light = (-d.dot(east) + d.dot(north)) / d.length().max(1e-12);
                    slope += (elevation[j] - elevation[i]) as f64 / grid.distance_m(i, j).max(1.)
                        * light;
                }
                let shade = (1. - slope * 5.).clamp(0.65, 1.2);
                for c in &mut color {
                    *c = (*c as f64 * shade).min(255.) as u8;
                }
            }
            if let Some(ice) = ice {
                let snow = if elevation[i] > 0. {
                    ice[0][i].min(1.).max(ice[1][i]).max(ice[7][i])
                } else {
                    ice[2][i].max(1. - (-ice[3][i] / 30.).exp())
                };
                let white = [233., 245., 245.];
                for c in 0..3 {
                    color[c] = (color[c] as f32 * (1. - snow) + white[c] * snow) as u8;
                }
            }
            color
        })
        .collect();
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header()?;
        {
            let mut stream = writer.stream_writer()?;
            let mut row = vec![0u8; width as usize * 3];
            for y in 0..height {
                for x in 0..width {
                    let i = match &grid {
                        MapGrid::Sphere(g) => g.cell_at(tf_core::geo::UnitVec3::from_lat_lon_deg(
                            90. - (y as f64 + 0.5) / height as f64 * 180.,
                            (x as f64 + 0.5) / width as f64 * 360. - 180.,
                        )),
                        MapGrid::Plane(g) => g
                            .cell_at_xy(
                                ((x as f64 + 0.5) / width as f64 - 0.5) * g.width(),
                                (0.5 - (y as f64 + 0.5) / height as f64) * g.height(),
                            )
                            .unwrap_or(0),
                        MapGrid::Patch(g) => {
                            let side = (g.cell_count() as f64).sqrt() as usize;
                            let r = ((y as f64 + 0.5) / height as f64 * side as f64)
                                .floor()
                                .min((side - 1) as f64)
                                as usize;
                            let c = ((x as f64 + 0.5) / width as f64 * side as f64
                                - (r % 2) as f64 * 0.5
                                - 0.25)
                                .round()
                                .clamp(0., (side - 1) as f64)
                                as usize;
                            r * side + c
                        }
                    };
                    row[x as usize * 3..x as usize * 3 + 3].copy_from_slice(&colors[i]);
                }
                stream.write_all(&row)?;
            }
            stream.finish()?;
        }
        writer.finish()?;
    }
    Ok(output)
}

/// 单个像素的颜色。
fn color_for(e: f32, b: u8) -> [u8; 3] {
    if e <= 0.0 {
        let depth = (-e / 6000.0).clamp(0.0, 1.0);
        let shallow = [46.0f32, 120.0, 150.0];
        let deep = [8.0f32, 22.0, 52.0];
        return [
            (shallow[0] * (1.0 - depth) + deep[0] * depth) as u8,
            (shallow[1] * (1.0 - depth) + deep[1] * depth) as u8,
            (shallow[2] * (1.0 - depth) + deep[2] * depth) as u8,
        ];
    }
    let base = biome_color(b);
    let t = ((e - 4500.0) / 2000.0).clamp(0.0, 1.0);
    [
        (base[0] as f32 * (1.0 - t) + 245.0 * t) as u8,
        (base[1] as f32 * (1.0 - t) + 248.0 * t) as u8,
        (base[2] as f32 * (1.0 - t) + 252.0 * t) as u8,
    ]
}

/// 生物群系配色。与前端保持一致。
fn biome_color(b: u8) -> [u8; 3] {
    match b {
        0 => [30, 70, 120],    // 海洋
        1 => [240, 245, 250],  // 冰盖
        2 => [150, 160, 140],  // 苔原
        3 => [45, 80, 55],     // 针叶林
        4 => [60, 110, 50],    // 温带阔叶林
        5 => [140, 160, 80],   // 温带草原
        6 => [150, 150, 90],   // 地中海
        7 => [210, 190, 140],  // 沙漠
        8 => [180, 170, 90],   // 稀树草原
        9 => [20, 90, 35],     // 热带雨林
        10 => [50, 120, 45],   // 热带季雨林
        11 => [120, 140, 100], // 高山草甸
        12 => [70, 110, 80],   // 湿地
        13 => [51, 140, 161],  // 内陆水体
        _ => [120, 120, 120],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compressed_png_has_correct_dimensions() {
        let d = tf_grid::GridDescriptor::patch(0., 0., 16000., 16, 6371000.);
        let bytes = render_map_png(&vec![100.; 256], &vec![4.; 256], &d, 128).unwrap();
        let reader = png::Decoder::new(bytes.as_slice()).read_info().unwrap();
        assert_eq!(reader.info().width, 128);
        assert_eq!(reader.info().height, 111);
        assert!(bytes.len() < 2000);
    }
    #[test]
    fn mismatched_data_is_rejected() {
        assert!(
            render_map_png(&[], &[], &tf_grid::GridDescriptor::sphere(4, 6371000.), 64).is_err()
        );
    }
    #[test]
    fn exported_ocean_ice_is_visible_without_turning_the_ocean_into_land() {
        let d = tf_grid::GridDescriptor::sphere(4, 6371000.);
        let n = d.cell_count();
        let mut ice = vec![vec![0.; n]; 8];
        ice[2].fill(1.);
        let bytes =
            render_map_png_with_ice(&vec![-1000.; n], &vec![0.; n], &d, 64, Some(&ice)).unwrap();
        let mut reader = png::Decoder::new(bytes.as_slice()).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert!(pixels[..info.buffer_size()]
            .chunks_exact(3)
            .all(|p| p[0] > 220 && p[1] > 230 && p[2] > 230));
    }
}
