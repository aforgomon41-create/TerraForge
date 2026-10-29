//! 服务端地图渲染。
//!
//! 把物理场画成 PNG，用于导出。
//! 这里不追求实时性，追求质量：用更精细的着色和抗锯齿。

/// 渲染一张地图 PNG。
///
/// 数据是等距圆柱投影的一维数组，渲染成正方形图。
/// 世界层和非世界层的处理不同：世界层用球面投影，其余用平面。
pub fn render_map_png(
    elevation: &[f32],
    biome: &[f32],
    size: u32,
) -> anyhow::Result<Vec<u8>> {
    let n = elevation.len();
    if n == 0 {
        anyhow::bail!("没有高程数据");
    }

    // 推断网格类型
    //
    // 关键：世界层的格子索引按二十面体细分顺序排列，不是按行列。
    // 直接把一维数组当二维图采样会得到斜条纹噪声。
    // 世界层必须按等距圆柱投影重采样。
    let is_world = {
        let f = (((n as f64) - 2.0) / 10.0).sqrt().round() as usize;
        10 * f * f + 2 == n
    };

    let mut rgb = vec![0u8; (size * size * 3) as usize];

    if is_world {
        let freq = (((n as f64) - 2.0) / 10.0).sqrt().round() as u32;
        let grid = tf_grid::SphereHexGrid::build(freq, 6_371_000.0, (glam::DVec3::Z, 0.0));

        for py in 0..size {
            let lat = 90.0 - (py as f64 + 0.5) / size as f64 * 180.0;
            for px in 0..size {
                let lon = (px as f64 + 0.5) / size as f64 * 360.0 - 180.0;
                let dir = tf_core::geo::UnitVec3::from_lat_lon_deg(lat, lon);
                let cell = grid.cell_at(dir);
                let e = elevation.get(cell).copied().unwrap_or(0.0);
                let b = biome.get(cell).copied().unwrap_or(0.0) as u8;
                let color = color_for(e, b);
                let o = ((py * size + px) * 3) as usize;
                rgb[o] = color[0];
                rgb[o + 1] = color[1];
                rgb[o + 2] = color[2];
            }
        }
    } else {
        let src_side = (n as f64).sqrt().round() as u32;
        for py in 0..size {
            for px in 0..size {
                let sx = (px as f64 / size as f64 * src_side as f64) as usize;
                let sy = (py as f64 / size as f64 * src_side as f64) as usize;
                let idx = sy.min(src_side as usize - 1) * src_side as usize
                    + sx.min(src_side as usize - 1);
                let e = elevation.get(idx).copied().unwrap_or(0.0);
                let b = biome.get(idx).copied().unwrap_or(0.0) as u8;
                let color = color_for(e, b);
                let o = ((py * size + px) * 3) as usize;
                rgb[o] = color[0];
                rgb[o + 1] = color[1];
                rgb[o + 2] = color[2];
            }
        }
    }
    // 编码 PNG
    encode_png(&rgb, size, size)
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
        _ => [120, 120, 120],
    }
}

/// 编码 PNG。用 zlib stored 块避免依赖压缩库。
fn encode_png(rgb: &[u8], w: u32, h: u32) -> anyhow::Result<Vec<u8>> {
    // 原始数据：每行前面加滤波类型字节
    let mut raw = Vec::with_capacity((w * h * 3 + h) as usize);
    for y in 0..h {
        raw.push(0u8);
        let start = (y * w * 3) as usize;
        raw.extend_from_slice(&rgb[start..start + (w * 3) as usize]);
    }

    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

    // IHDR
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    write_chunk(&mut out, b"IHDR", &ihdr);

    // IDAT
    let compressed = zlib_stored(&raw);
    write_chunk(&mut out, b"IDAT", &compressed);

    // IEND
    write_chunk(&mut out, b"IEND", &[]);

    Ok(out)
}

fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65535 * 5 + 16);
    out.push(0x78);
    out.push(0x01);

    if data.is_empty() {
        out.push(1);
        out.extend_from_slice(&[0, 0, 0xFF, 0xFF]);
    }

    let mut pos = 0;
    while pos < data.len() {
        let remaining = data.len() - pos;
        let block_len = remaining.min(65535);
        let is_last = pos + block_len >= data.len();
        out.push(if is_last { 1 } else { 0 });
        let len = block_len as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(&data[pos..pos + block_len]);
        pos += block_len;
    }

    // adler32
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out
}

fn write_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);

    let mut crc: u32 = 0xFFFFFFFF;
    for &b in tag.iter().chain(data.iter()) {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB88320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    out.extend_from_slice(&(crc ^ 0xFFFFFFFF).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_has_valid_signature() {
        let elev = vec![100.0f32; 256];
        let biome = vec![4.0f32; 256];
        let png = render_map_png(&elev, &biome, 64).unwrap();
        assert_eq!(&png[0..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    #[test]
    fn empty_data_is_rejected() {
        assert!(render_map_png(&[], &[], 64).is_err());
    }

    #[test]
    fn output_size_is_reasonable() {
        let elev = vec![500.0f32; 4096];
        let biome = vec![4.0f32; 4096];
        let png = render_map_png(&elev, &biome, 128).unwrap();
        // 128x128 的纯色图应该很小
        assert!(png.len() < 200_000, "PNG 太大: {}", png.len());
    }
}
