//! 网格抽象。
//!
//! 物理引擎全部写成泛型函数，只依赖这个 trait。这样同一套算法
//! 在球面六角格和平面六角格上都能跑，不需要写两遍。

use bitvec::prelude::BitVec;
use tf_core::geo::UnitVec3;

/// 网格操作的公共接口。
pub trait GridOps: Send + Sync {
    /// 格子总数。
    fn cell_count(&self) -> usize;

    /// 第 i 个格子的邻居索引。
    fn neighbors(&self, i: usize) -> &[u32];

    /// 两个格子中心之间的距离，米。
    fn distance_m(&self, a: usize, b: usize) -> f64;

    /// 第 i 个格子的面积，平方米。
    fn area_m2(&self, i: usize) -> f64;

    /// 格子中心的球面方向（平面网格会投影到一个虚拟球面上）。
    fn center_dir(&self, i: usize) -> UnitVec3;

    /// 第 i 个格子中心的地理纬度，用于气候计算。
    fn latitude_deg(&self, i: usize) -> f64;

    /// 图边界掩码。这些格子在图的最外圈，处理时要特殊对待。
    fn boundary_mask(&self) -> &BitVec;

    /// 是不是五边形（只有球面网格会有）。
    fn is_pentagon(&self, i: usize) -> bool {
        self.neighbors(i).len() == 5
    }
}
