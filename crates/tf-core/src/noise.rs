//! 噪声函数。
//!
//! 注意项目原则：地形**不靠噪声生成**，噪声只用在两个次要场合：
//! 1. 给本来就应该随机分布的属性加扰动（比如岩性边界的不规则性）
//! 2. 上采样时补充低于父层分辨率的细节纹理
//!
//! 所以这里只提供最小必要的实现，防止被误用成主要地形来源。

/// 3D Simplex 噪声。用置换表实现，种子决定表内容。
pub struct SimplexNoise {
    perm: [u8; 512],
}

impl SimplexNoise {
    pub fn new(seed: u64) -> Self {
        let mut perm: [u8; 256] = [0; 256];
        for (i, slot) in perm.iter_mut().enumerate() {
            *slot = i as u8;
        }
        // Fisher-Yates 洗牌，随机源来自种子的简单哈希链
        let mut state = seed | 1;
        for i in (1..256).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let j = (state % (i as u64 + 1)) as usize;
            perm.swap(i, j);
        }
        let mut full = [0u8; 512];
        for i in 0..512 {
            full[i] = perm[i & 255];
        }
        Self { perm: full }
    }

    #[inline]
    fn grad3(hash: u8, x: f64, y: f64, z: f64) -> f64 {
        // 12 个梯度方向，用低位选
        const GRAD: [[f64; 3]; 12] = [
            [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, -1.0, 0.0],
            [1.0, 0.0, 1.0], [-1.0, 0.0, 1.0], [1.0, 0.0, -1.0], [-1.0, 0.0, -1.0],
            [0.0, 1.0, 1.0], [0.0, -1.0, 1.0], [0.0, 1.0, -1.0], [0.0, -1.0, -1.0],
        ];
        let g = GRAD[(hash % 12) as usize];
        g[0] * x + g[1] * y + g[2] * z
    }

    /// 采样 [-1, 1] 范围的噪声值。
    pub fn sample(&self, x: f64, y: f64, z: f64) -> f64 {
        const F3: f64 = 1.0 / 3.0;
        const G3: f64 = 1.0 / 6.0;

        let s = (x + y + z) * F3;
        let i = (x + s).floor();
        let j = (y + s).floor();
        let k = (z + s).floor();

        let t = (i + j + k) * G3;
        let x0 = x - (i - t);
        let y0 = y - (j - t);
        let z0 = z - (k - t);

        let (i1, j1, k1, i2, j2, k2) = if x0 >= y0 {
            if y0 >= z0 {
                (1, 0, 0, 1, 1, 0)
            } else if x0 >= z0 {
                (1, 0, 0, 1, 0, 1)
            } else {
                (0, 0, 1, 1, 0, 1)
            }
        } else if y0 < z0 {
            (0, 0, 1, 0, 1, 1)
        } else if x0 < z0 {
            (0, 1, 0, 0, 1, 1)
        } else {
            (0, 1, 0, 1, 1, 0)
        };

        let x1 = x0 - i1 as f64 + G3;
        let y1 = y0 - j1 as f64 + G3;
        let z1 = z0 - k1 as f64 + G3;
        let x2 = x0 - i2 as f64 + 2.0 * G3;
        let y2 = y0 - j2 as f64 + 2.0 * G3;
        let z2 = z0 - k2 as f64 + 2.0 * G3;
        let x3 = x0 - 1.0 + 3.0 * G3;
        let y3 = y0 - 1.0 + 3.0 * G3;
        let z3 = z0 - 1.0 + 3.0 * G3;

        let ii = (i as i32 & 255) as usize;
        let jj = (j as i32 & 255) as usize;
        let kk = (k as i32 & 255) as usize;

        let mut n = 0.0;

        let mut t0 = 0.6 - x0 * x0 - y0 * y0 - z0 * z0;
        if t0 > 0.0 {
            t0 *= t0;
            n += t0 * t0 * Self::grad3(self.perm[ii + self.perm[jj + self.perm[kk] as usize] as usize], x0, y0, z0);
        }
        let mut t1 = 0.6 - x1 * x1 - y1 * y1 - z1 * z1;
        if t1 > 0.0 {
            t1 *= t1;
            n += t1 * t1
                * Self::grad3(
                    self.perm[ii + i1 + self.perm[jj + j1 + self.perm[kk + k1] as usize] as usize],
                    x1, y1, z1,
                );
        }
        let mut t2 = 0.6 - x2 * x2 - y2 * y2 - z2 * z2;
        if t2 > 0.0 {
            t2 *= t2;
            n += t2 * t2
                * Self::grad3(
                    self.perm[ii + i2 + self.perm[jj + j2 + self.perm[kk + k2] as usize] as usize],
                    x2, y2, z2,
                );
        }
        let mut t3 = 0.6 - x3 * x3 - y3 * y3 - z3 * z3;
        if t3 > 0.0 {
            t3 *= t3;
            n += t3 * t3
                * Self::grad3(
                    self.perm[ii + 1 + self.perm[jj + 1 + self.perm[kk + 1] as usize] as usize],
                    x3, y3, z3,
                );
        }

        32.0 * n
    }

    /// 分形布朗运动（多倍频叠加）。返回大致在 [-1, 1]。
    pub fn fbm(&self, x: f64, y: f64, z: f64, octaves: u32, lacunarity: f64, gain: f64) -> f64 {
        let mut sum = 0.0;
        let mut amp = 1.0;
        let mut freq = 1.0;
        let mut norm = 0.0;
        for _ in 0..octaves {
            sum += self.sample(x * freq, y * freq, z * freq) * amp;
            norm += amp;
            amp *= gain;
            freq *= lacunarity;
        }
        if norm > 0.0 { sum / norm } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_within_expected_range() {
        let noise = SimplexNoise::new(42);
        for i in 0..10000 {
            let x = i as f64 * 0.017;
            let v = noise.sample(x, x * 1.3, x * 0.7);
            assert!(v.is_finite(), "non-finite at {}", i);
            assert!(v >= -1.5 && v <= 1.5, "out of range: {}", v);
        }
    }

    #[test]
    fn deterministic_for_same_seed() {
        let a = SimplexNoise::new(777);
        let b = SimplexNoise::new(777);
        for i in 0..100 {
            let x = i as f64 * 0.1;
            assert_eq!(a.sample(x, x, x), b.sample(x, x, x));
        }
    }

    #[test]
    fn different_seeds_differ() {
        let a = SimplexNoise::new(1);
        let b = SimplexNoise::new(2);
        let mut diff = 0;
        let total = 200;
        for i in 0..total {
            // 在对角线上采样会落进 simplex 的退化区域，不同种子容易算出相近值，
            // 所以要用三个独立变化的坐标。
            let f = i as f64;
            let p = (f * 0.0731, f * 0.1179 + 3.1, f * 0.0523 + 7.7);
            if (a.sample(p.0, p.1, p.2) - b.sample(p.0, p.1, p.2)).abs() > 1e-9 {
                diff += 1;
            }
        }
        assert!(diff > total * 9 / 10, "only {} of {} differences", diff, total);
    }

    #[test]
    fn smooth_continuity() {
        // 相邻采样点之间不应该出现跳变
        let noise = SimplexNoise::new(9);
        let mut prev = noise.sample(0.0, 0.0, 0.0);
        for i in 1..1000 {
            let x = i as f64 * 0.001;
            let cur = noise.sample(x, 0.0, 0.0);
            assert!((cur - prev).abs() < 0.1, "jump at {}: {} -> {}", i, prev, cur);
            prev = cur;
        }
    }
}
