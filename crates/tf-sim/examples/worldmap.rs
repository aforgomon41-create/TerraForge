//! 生成世界地图 PNG，用于人工验收地貌质量。
//!
//! 用法: cargo run --release --example worldmap -- <freq> <size> <out.png> [seed]

use std::f64::consts::PI;
use tf_core::params::GenerationParams;
use tf_grid::{GridOps, SphereHexGrid};
use tf_sim::ecology::Biome;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let freq: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(128);
    let size: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1600);
    let out = args.get(3).cloned().unwrap_or_else(|| "world.png".to_string());
    let seed: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(42);

    let mut params = GenerationParams::default();
    params.seed = seed;

    println!("=== TerraForge 世界生成 ===");
    println!("精度档位: {} (f={})", params.quality.name_zh(), freq);
    println!("种子: {}", seed);
    println!("参数: {}", params.summary_zh());

    let t0 = std::time::Instant::now();
    let grid = SphereHexGrid::build(freq, params.planet.radius_m, (glam::DVec3::Z, 0.0));
    println!("网格: {} 格, 构建 {:.2}s", grid.cell_count(), t0.elapsed().as_secs_f64());

    let t1 = std::time::Instant::now();
    let mut last_stage = String::new();
    let world = tf_sim::generate(&grid, &params, &mut |stage, progress| {
        let name = stage.name_zh().to_string();
        if name != last_stage {
            println!("  [{:>5.1}%] {}", progress * 100.0, name);
            last_stage = name;
        }
    });
    println!("生成完成: {:.2}s", t1.elapsed().as_secs_f64());

    for (stage, ms) in &world.stage_timings {
        println!("  {:<10} {:>6} ms", stage.name_zh(), ms);
    }

    // 统计
    let f = &world.fields;
    let n = grid.cell_count();
    let land = (0..n).filter(|&i| f.elevation[i] > 0.0).count();
    let max_e = f.elevation.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let min_e = f.elevation.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_p = f.precipitation.iter().cloned().fold(0.0f32, f32::max);
    let max_t = f.temperature.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let min_t = f.temperature.iter().cloned().fold(f32::INFINITY, f32::min);
    println!();
    println!("陆地占比: {:.1}%", land as f64 / n as f64 * 100.0);
    println!("高程范围: {:.0} m ~ {:.0} m", min_e, max_e);
    println!("温度范围: {:.1} C ~ {:.1} C", min_t, max_t);
    println!("最大降水: {:.0} mm/年", max_p);
    println!("矿点数量: {}", world.minerals.len());

    // 渲染
    let mut img = vec![0u8; (size * size * 3) as usize];
    for i in 0..(size * size) as usize {
        img[i * 3] = 8;
        img[i * 3 + 1] = 10;
        img[i * 3 + 2] = 14;
    }

    let cx = size as f64 / 2.0;
    let radius = size as f64 * 0.47;

    // 光照方向
    let light = glam::DVec3::new(-0.5, 0.6, 0.6).normalize();

    for i in 0..n {
        let center = grid.center(i).as_vec();
        if center.z < -0.05 { continue; }

        let corners = grid.corners_of(i);
        if corners.len() < 3 { continue; }

        let color = shade_cell(i, &grid, &world, light);

        // 投影到屏幕，并向外扩 0.6 像素。
        //
        // 六角格在球面上严丝合缝，但投影到平面后相邻格子的共享角点
        // 会因为浮点误差落在不同像素上，产生极细的缝。
        // 格子很小时这些缝密集排列，看起来就是梳状条纹。
        //
        // 人为放大一点点让相邻格子重叠，是标准的做法。
        let pts: Vec<(f64, f64)> = corners.iter().map(|p| {
            let sx = cx + p.x * radius;
            let sy = cx - p.y * radius;
            // 沿从中心向外的方向外扩
            let dx = sx - cx;
            let dy = sy - cx;
            let len = (dx * dx + dy * dy).sqrt().max(1e-9);
            (sx + dx / len * 0.6, sy + dy / len * 0.6)
        }).collect();

        // 海岸线描边：陆地边缘用深色
        let is_land = f.elevation[i] > 0.0;
        let near_sea = grid.neighbors(i).iter().any(|&nb| (f.elevation[nb as usize] > 0.0) != is_land);
        let color = if near_sea {
            if is_land { [230u8, 220, 190] } else { [40, 90, 140] }
        } else { color };

        fill_polygon(&mut img, size, &pts, color);
    }

    write_png(&out, &img, size, size);
    println!("已写出 {}", out);
}

fn shade_cell(
    i: usize,
    grid: &SphereHexGrid,
    world: &tf_sim::GeneratedWorld,
    light: glam::DVec3,
) -> [u8; 3] {
    let f = &world.fields;
    let e = f.elevation[i];

    if e <= 0.0 {
        // 海洋配色：按深度分层。
        //
        // 关键：用 powf 让浅海区（<500m）快速过渡到深色，
        // 否则大片海洋都是同一个中间蓝，看起来像浅滩。
        let depth = (-e / 5500.0).clamp(0.0, 1.0);
        let t = depth.powf(0.55);
        // 大陆架（浅）用青绿，深海用深靛蓝
        let shallow = [58.0f32, 118.0, 138.0];
        let mid = [22.0f32, 62.0, 104.0];
        let deep = [8.0f32, 20.0, 48.0];
        let (r, g, b) = if t < 0.35 {
            let u = t / 0.35;
            (
                shallow[0] + (mid[0] - shallow[0]) * u,
                shallow[1] + (mid[1] - shallow[1]) * u,
                shallow[2] + (mid[2] - shallow[2]) * u,
            )
        } else {
            let u = (t - 0.35) / 0.65;
            (
                mid[0] + (deep[0] - mid[0]) * u,
                mid[1] + (deep[1] - mid[1]) * u,
                mid[2] + (deep[2] - mid[2]) * u,
            )
        };
        return [r as u8, g as u8, b as u8];
    }

    // 陆地着色：基色来自生物群系，但要做三重连续调制，
    // 否则同一群系的整片区域会是同一个色块，边界生硬。
    let biome = world.ecology.biome[i];
    let base = biome_color(biome);

    // 1) 高程调制：越高越偏黄褐（土色），极高偏灰白（岩/雪）
    let h = (e / 5000.0).clamp(0.0, 1.5) as f32;
    let mut r = base[0] as f32;
    let mut g = base[1] as f32;
    let mut b = base[2] as f32;

    // 棕褐色调，模拟裸土与岩屑
    let earth = [142.0f32, 116.0, 82.0];
    let rock = [124.0f32, 118.0, 108.0];
    let snow = [242.0f32, 246.0, 250.0];

    // 2000 m 以上逐渐露出岩石
    let rock_t = ((e - 2000.0) / 2000.0).clamp(0.0, 1.0) * 0.55;
    r = r * (1.0 - rock_t) + rock[0] * rock_t;
    g = g * (1.0 - rock_t) + rock[1] * rock_t;
    b = b * (1.0 - rock_t) + rock[2] * rock_t;

    // 4800 m 以上覆雪
    let snow_t = ((e - 4800.0) / 1600.0).clamp(0.0, 1.0);
    r = r * (1.0 - snow_t) + snow[0] * snow_t;
    g = g * (1.0 - snow_t) + snow[1] * snow_t;
    b = b * (1.0 - snow_t) + snow[2] * snow_t;

    // 2) 降水调制：越干越偏土黄，越湿越偏深绿
    let precip = f.precipitation[i];
    let dry = (1.0 - (precip / 900.0).clamp(0.0, 1.0)) * 0.35;
    r = r * (1.0 - dry) + earth[0] * dry;
    g = g * (1.0 - dry) + earth[1] * dry;
    b = b * (1.0 - dry) + earth[2] * dry;

    // 3) 细节调制：用**高程的局部变化**而不是人为噪声。
    //
    // 之前用正弦叠加做纹理，在格宽 28-60 km 时会出现明显的混叠条纹——
    // 这是采样定理的必然结果，频率再调也只是把条纹换个方向。
    //
    // 改用高程本身的高频分量（相对邻居的偏差）做纹理，
    // 这样纹理完全贴合真实地形，永远不会混叠。
    let nb = grid.neighbors(i);
    let detail: f32 = if nb.is_empty() {
        0.0
    } else {
        let mean = nb.iter().map(|&j| f.elevation[j as usize]).sum::<f32>() / nb.len() as f32;
        // 相对邻居的高差，归一化到 [-1, 1] 量级
        ((e - mean) / 400.0).clamp(-1.0, 1.0)
    };
    // 细节幅度要随海拔**衰减**而不是增强。
    //
    // 之前是 `detail_amp = 8 + h*14`，海拔越高细节越强，
    // 在高纬度高原上就产生了密密麻麻的梳状条纹。
    // 真实情况是：高山被冰雪和裸岩覆盖，视觉上是均匀的；
    // 只有中低海拔的植被区才有丰富的色彩变化。
    let detail_amp = (10.0 - h * 6.0).clamp(2.0, 10.0);
    r += detail * detail_amp;
    g += detail * detail_amp;
    b += detail * detail_amp * 0.7;

    // 地形光照：最小二乘拟合切平面梯度
    let c = grid.center(i).as_vec();
    let (east, north) = grid.center_dir(i).tangent_frame();
    let mut sxx = 0.0f64;
    let mut sxy = 0.0f64;
    let mut syy = 0.0f64;
    let mut sxz = 0.0f64;
    let mut syz = 0.0f64;
    for &nb in grid.neighbors(i) {
        let dh = (f.elevation[nb as usize] - e) as f64;
        let d = grid.center_dir(nb as usize).as_vec() - c;
        let dx = d.dot(east);
        let dy = d.dot(north);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
        sxz += dx * dh;
        syz += dy * dh;
    }
    let det = sxx * syy - sxy * sxy;
    let (dx, dy) = if det.abs() > 1e-12 {
        ((sxz * syy - syz * sxy) / det, (syz * sxx - sxz * sxy) / det)
    } else {
        (0.0, 0.0)
    };

    let normal = glam::DVec3::new(-dx * 2500.0, -dy * 2500.0, 1.0).normalize();
    let diffuse = (normal.dot(light).max(0.0) as f32).powf(0.85);
    let shade = 0.40 + diffuse * 0.80;

    [
        (r * shade).clamp(0.0, 255.0) as u8,
        (g * shade).clamp(0.0, 255.0) as u8,
        (b * shade).clamp(0.0, 255.0) as u8,
    ]
}

fn biome_color(b: u8) -> [u8; 3] {
    let list = [
        Biome::Ocean, Biome::IceSheet, Biome::Tundra, Biome::BorealForest,
        Biome::TemperateForest, Biome::TemperateGrassland, Biome::Mediterranean,
        Biome::Desert, Biome::Savanna, Biome::TropicalRainforest,
        Biome::TropicalSeasonalForest, Biome::AlpineMeadow, Biome::Wetland,
    ];
    list.get(b as usize).map(|x| x.color()).unwrap_or([100, 100, 100])
}

fn fill_polygon(img: &mut [u8], size: u32, pts: &[(f64, f64)], color: [u8; 3]) {
    if pts.len() < 3 { return; }
    let min_y = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min).floor().max(0.0) as i32;
    let max_y = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max).ceil().min(size as f64 - 1.0) as i32;
    for y in min_y..=max_y {
        let yf = y as f64 + 0.5;
        let mut xs: Vec<f64> = Vec::new();
        for k in 0..pts.len() {
            let p1 = pts[k];
            let p2 = pts[(k + 1) % pts.len()];
            if (p1.1 <= yf && p2.1 > yf) || (p2.1 <= yf && p1.1 > yf) {
                let t = (yf - p1.1) / (p2.1 - p1.1);
                xs.push(p1.0 + t * (p2.0 - p1.0));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut k = 0;
        while k + 1 < xs.len() {
            let x0 = xs[k].ceil().max(0.0) as i32;
            let x1 = xs[k + 1].floor().min(size as f64 - 1.0) as i32;
            for x in x0..=x1 {
                let idx = (y as usize * size as usize + x as usize) * 3;
                img[idx] = color[0];
                img[idx + 1] = color[1];
                img[idx + 2] = color[2];
            }
            k += 2;
        }
    }
}

fn write_png(path: &str, rgb: &[u8], w: u32, h: u32) {
    use std::io::Write;
    let mut raw = Vec::with_capacity((w * h * 3 + h) as usize);
    for y in 0..h {
        raw.push(0u8);
        let start = (y * w * 3) as usize;
        raw.extend_from_slice(&rgb[start..start + (w * 3) as usize]);
    }
    let compressed = zlib_stored(&raw);
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]).unwrap();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    write_chunk(&mut f, b"IHDR", &ihdr);
    write_chunk(&mut f, b"IDAT", &compressed);
    write_chunk(&mut f, b"IEND", &[]);
}

fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65535 * 5 + 16);
    let nblocks = data.len() / 65535 + 1;
    out.push(0x78);
    out.push(0x01);
    let _ = nblocks;
    let mut pos = 0;
    if data.is_empty() {
        out.push(1);
        out.extend_from_slice(&[0, 0, 0xFF, 0xFF]);
    }
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
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out
}

fn write_chunk(f: &mut std::fs::File, tag: &[u8; 4], data: &[u8]) {
    use std::io::Write;
    f.write_all(&(data.len() as u32).to_be_bytes()).unwrap();
    f.write_all(tag).unwrap();
    f.write_all(data).unwrap();
    let mut crc = Crc32::new();
    crc.update(tag);
    crc.update(data);
    f.write_all(&crc.finish().to_be_bytes()).unwrap();
}

struct Crc32 { table: [u32; 256], value: u32 }

impl Crc32 {
    fn new() -> Self {
        let mut table = [0u32; 256];
        for i in 0..256u32 {
            let mut c = i;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            }
            table[i as usize] = c;
        }
        Self { table, value: 0xFFFFFFFF }
    }
    fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.value = self.table[((self.value ^ b as u32) & 0xFF) as usize] ^ (self.value >> 8);
        }
    }
    fn finish(&self) -> u32 { self.value ^ 0xFFFFFFFF }
}