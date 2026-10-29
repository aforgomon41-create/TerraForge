//! 生成六角球的 PNG 预览图，用于人工验证几何正确性。
//!
//! 用正交投影把球面上的六角格画成实心多边形，按纬度着色。
//! 能一眼看出：格宽是否均匀、五边形位置、有无破洞或重叠。

use std::f64::consts::PI;
use tf_grid::SphereHexGrid;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let freq: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(32);
    let size: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1200);
    let out = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| format!("hexgrid_f{}.png", freq));

    println!("构建 f={} 的六角球...", freq);
    let t = std::time::Instant::now();
    let grid = SphereHexGrid::build(freq, 6_371_000.0, (glam::DVec3::Z, 0.0));
    println!(
        "  耗时 {} ms, 格数 {}",
        t.elapsed().as_millis(),
        grid.cell_count()
    );

    let mut img = vec![0u8; (size * size * 3) as usize];
    // 深蓝背景
    for i in 0..(size * size) as usize {
        img[i * 3] = 10;
        img[i * 3 + 1] = 12;
        img[i * 3 + 2] = 16;
    }

    let cx = size as f64 / 2.0;
    let cy = size as f64 / 2.0;
    let radius = size as f64 * 0.46;

    // 把每个格子的角点投影到屏幕，画填充多边形
    println!("光栅化 {} 个格子...", grid.cell_count());
    for i in 0..grid.cell_count() {
        let corners = grid.corners_of(i);
        if corners.len() < 3 {
            continue;
        }

        // 只画朝向观察者的格子（z > 0，正交投影下即前半球的）
        let center = grid.center(i).as_vec();
        if center.z < 0.0 {
            continue;
        }

        // 按纬度着色：赤道偏暖，两极偏冷
        let lat = center.y.asin();
        let t01 = (lat / (PI / 2.0) + 1.0) * 0.5;
        let r = (40.0 + 200.0 * (1.0 - t01)) as u8;
        let g = (120.0 + 80.0 * (1.0 - (t01 - 0.5).abs() * 2.0)) as u8;
        let b = (60.0 + 180.0 * t01) as u8;

        // 五边形用亮黄色标记
        let (r, g, b) = if grid.is_pentagon(i) {
            (255u8, 220u8, 40u8)
        } else {
            (r, g, b)
        };

        // 投影角点
        let pts: Vec<(f64, f64)> = corners
            .iter()
            .map(|p| (cx + p.x * radius, cy - p.y * radius))
            .collect();

        // 扫描线填充
        let min_y = pts
            .iter()
            .map(|p| p.1)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0) as i32;
        let max_y = pts
            .iter()
            .map(|p| p.1)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .min(size as f64 - 1.0) as i32;
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
                    img[idx] = r;
                    img[idx + 1] = g;
                    img[idx + 2] = b;
                }
                k += 2;
            }
        }
    }

    // 写 PNG（手写最小 PNG 编码，避免引入依赖）
    write_png(&out, &img, size, size);
    println!("已写出 {}", out);
}

/// 把数据包成 zlib 流，内部用 deflate 的 stored（不压缩）块。
///
/// 这样写出来的 PNG 完全合法，只是文件偏大。预览图不追求体积。
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65535 * 5 + 16);
    // zlib 头：CMF=0x78 (deflate, 32K window), FLG=0x01
    out.push(0x78);
    out.push(0x01);

    // deflate stored blocks，每块最多 65535 字节
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
    if data.is_empty() {
        out.push(1);
        out.extend_from_slice(&[0, 0, 0xFF, 0xFF]);
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

fn write_png(path: &str, rgb: &[u8], w: u32, h: u32) {
    use std::io::Write;
    let mut raw = Vec::with_capacity((w * h * 3 + h) as usize);
    for y in 0..h {
        raw.push(0u8); // 滤波类型 None
        let start = (y * w * 3) as usize;
        raw.extend_from_slice(&rgb[start..start + (w * 3) as usize]);
    }
    // 用 zlib stored 块（不压缩）实现，避免引入压缩库依赖。
    // PNG 的 IDAT 是 zlib 流：2 字节头 + deflate 数据 + 4 字节 adler32。
    let compressed = zlib_stored(&raw);

    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
        .unwrap();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8bit, RGB
    write_chunk(&mut f, b"IHDR", &ihdr);
    write_chunk(&mut f, b"IDAT", &compressed);
    write_chunk(&mut f, b"IEND", &[]);
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

struct Crc32 {
    table: [u32; 256],
    value: u32,
}

impl Crc32 {
    fn new() -> Self {
        let mut table = [0u32; 256];
        for i in 0..256u32 {
            let mut c = i;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB88320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            table[i as usize] = c;
        }
        Self {
            table,
            value: 0xFFFFFFFF,
        }
    }
    fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.value = self.table[((self.value ^ b as u32) & 0xFF) as usize] ^ (self.value >> 8);
        }
    }
    fn finish(&self) -> u32 {
        self.value ^ 0xFFFFFFFF
    }
}
