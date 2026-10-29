//! 确定性随机数。
//!
//! 整个项目对可复现性有硬要求：同一个 seed 必须永远产生同一个世界。
//! 所有随机数都从这里的 RNG 派生，并且派生子流时用带标签的哈希，
//! 保证「给某个阶段单独换个顺序」不会连锁影响其他阶段。

use rand_chacha::rand_core::{RngCore, SeedableRng};
use rand_chacha::ChaCha12Rng;

/// 包装 ChaCha12 的确定性随机源。
///
/// 选 ChaCha12 而不是 8 或 20：12 轮在这个项目里足够，且比 20 快约 40%。
pub struct DeterministicRng {
    inner: ChaCha12Rng,
}

impl DeterministicRng {
    /// 从主种子和一组标签派生一个独立子流。
    ///
    /// 标签的设计原则：用清晰的字符串或整数常量，比如
    /// `derive(seed, &["tectonics", plate_id])`。
    pub fn derive(seed: u64, tag: &str) -> Self {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in seed.to_le_bytes().into_iter().chain(tag.bytes()) {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        Self::from_seed(hash)
    }

    /// 直接用给定种子构造。
    pub fn from_seed(seed: u64) -> Self {
        Self {
            inner: ChaCha12Rng::seed_from_u64(seed),
        }
    }

    /// 均匀分布于 [0, 1) 的浮点数。
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        // 取 53 位有效位，得到 [0,1) 的均匀分布
        let bits = self.inner.next_u64() >> 11;
        bits as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// 均匀分布于 [lo, hi) 的浮点数。
    #[inline]
    pub fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.next_f64() * (hi - lo)
    }

    /// 均匀分布于 [0, n) 的整数。n 必须大于 0。
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        debug_assert!(n > 0);
        // 拒绝采样，消除取模偏差
        let zone = u64::MAX - (u64::MAX % n as u64);
        loop {
            let v = self.inner.next_u64();
            if v < zone {
                return (v % n as u64) as u32;
            }
        }
    }

    /// 均匀分布于 [lo, hi] 的整数。
    #[inline]
    pub fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        debug_assert!(hi >= lo);
        let span = (hi - lo) as u32 + 1;
        lo + self.below(span) as i32
    }

    /// 标准正态分布，用 Box-Muller 变换。
    #[inline]
    pub fn normal(&mut self) -> f64 {
        // 避免 ln(0)
        let u1 = self.next_f64().max(1e-300);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }

    /// 给定均值与标准差的正态分布。
    #[inline]
    pub fn normal_with(&mut self, mean: f64, std_dev: f64) -> f64 {
        mean + self.normal() * std_dev
    }

    /// 从权重表里抽一个索引。权重总和必须大于 0。
    pub fn weighted_choice(&mut self, weights: &[f64]) -> usize {
        let total: f64 = weights.iter().sum();
        debug_assert!(total > 0.0, "weights must sum to positive value");
        let mut acc = 0.0;
        let target = self.next_f64() * total;
        for (i, &w) in weights.iter().enumerate() {
            acc += w;
            if target < acc {
                return i;
            }
        }
        weights.len() - 1
    }

    /// 单位球面上一个均匀分布的点。
    pub fn unit_sphere(&mut self) -> crate::geo::UnitVec3 {
        // 用一个 3D 正态向量归一化，避免极点的面积偏差
        loop {
            let v = glam::DVec3::new(self.normal(), self.normal(), self.normal());
            if let Some(u) = crate::geo::UnitVec3::from_vec(v) {
                return u;
            }
        }
    }

    /// 直接暴露底层 RNG，给需要填充大块随机数的场合用。
    pub fn inner_mut(&mut self) -> &mut ChaCha12Rng {
        &mut self.inner
    }
}

/// 一个可序列化、可派生的种子路径。
///
/// 层级结构里每张子图都有自己的种子，由「世界种子 + 该图在树上的路径」派生。
/// 这样同一位置永远算出同样的结果，删除再重新生成也一致。
pub fn derive_child_seed(world_seed: u64, path: &[u128]) -> u64 {
    // Fixed FNV-1a over explicit little-endian bytes: independent of Rust's
    // unspecified DefaultHasher implementation and of randomly allocated IDs.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in world_seed
        .to_le_bytes()
        .into_iter()
        .chain(path.iter().flat_map(|id| id.to_le_bytes()))
    {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = DeterministicRng::from_seed(42);
        let mut b = DeterministicRng::from_seed(42);
        for _ in 0..1000 {
            assert_eq!(a.inner_mut().next_u64(), b.inner_mut().next_u64());
        }
    }

    #[test]
    fn different_tags_diverge() {
        let seed = 12345;
        let mut a = DeterministicRng::derive(seed, "tectonics");
        let mut b = DeterministicRng::derive(seed, "erosion");
        let seq_a: Vec<u64> = (0..10).map(|_| a.inner_mut().next_u64()).collect();
        let seq_b: Vec<u64> = (0..10).map(|_| b.inner_mut().next_u64()).collect();
        assert_ne!(seq_a, seq_b);
    }

    #[test]
    fn below_is_in_range() {
        let mut rng = DeterministicRng::from_seed(7);
        for _ in 0..10000 {
            let v = rng.below(37);
            assert!(v < 37);
        }
    }

    #[test]
    fn next_f64_in_unit_interval() {
        let mut rng = DeterministicRng::from_seed(99);
        for _ in 0..10000 {
            let v = rng.next_f64();
            assert!((0.0..1.0).contains(&v), "out of range: {}", v);
        }
    }

    #[test]
    fn normal_is_roughly_standard() {
        let mut rng = DeterministicRng::from_seed(2024);
        let n = 100_000;
        let mut sum = 0.0;
        let mut sum_sq = 0.0;
        for _ in 0..n {
            let v = rng.normal();
            sum += v;
            sum_sq += v * v;
        }
        let mean = sum / n as f64;
        let var = sum_sq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.02, "mean off: {}", mean);
        assert!((var - 1.0).abs() < 0.05, "variance off: {}", var);
    }

    #[test]
    fn child_seed_is_stable_and_path_dependent() {
        let s1 = derive_child_seed(100, &[1, 2, 3]);
        let s2 = derive_child_seed(100, &[1, 2, 3]);
        let s3 = derive_child_seed(100, &[1, 2, 4]);
        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }

    #[test]
    fn weighted_choice_respects_weights() {
        let mut rng = DeterministicRng::from_seed(5);
        // 第一个粉为 0.9 权重，应该绝大部分落到索引 0
        let mut counts = [0usize; 2];
        for _ in 0..10000 {
            counts[rng.weighted_choice(&[0.9, 0.1])] += 1;
        }
        assert!(counts[0] > 8000, "counts: {:?}", counts);
        assert!(counts[1] > 500, "counts: {:?}", counts);
    }
}
