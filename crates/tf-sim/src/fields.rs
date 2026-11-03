//! 世界场数据。
//!
//! 所有物理场都挂在一个 WorldFields 上。场用扁平数组存储，索引即格子号，
//! 这样物理计算可以全并行，缓存也友好。

use serde::{Deserialize, Serialize};

/// 场数据的集合。
///
/// 分工原则：
/// - 高频访问、物理计算必须的场常驻这里
/// - 低频、可从高程重算的场按需生成，不进这个结构
#[derive(Debug, Clone, Default)]
pub struct WorldFields {
    /// 高程，米。相对海平面。负数是海底。
    pub elevation: Vec<f32>,
    /// 地壳厚度，米。控制均衡浮力和造山。
    pub crust_thickness: Vec<f32>,
    /// Initial equilibrated column; subsequent thickness change produces uplift.
    pub reference_crust_thickness: Vec<f32>,
    /// Accumulated horizontal shortening; transported with the crust.
    pub crustal_strain: Vec<f32>,
    pub orogen_age: Vec<f32>,
    pub shear_strain: Vec<f32>,
    /// Cold stable continental material; advected, never inferred from map colour.
    pub craton_strength: Vec<f32>,
    /// Accumulated extension of inherited weak material, transported with it.
    pub rift_damage: Vec<f32>,
    pub potential_evaporation: Vec<f32>,
    /// Dimensionless P/PET, not a prescribed desert fraction.
    pub aridity: Vec<f32>,
    /// Coarse monthly fields retain their own geometry instead of duplicating 36 fine fields.
    pub climate: Option<crate::seasonal::ClimateAtlas>,
    /// 地壳类型：0 = 洋壳，1 = 陆壳。过渡区用 0-1 之间的小数表示混合。
    pub crust_type: Vec<f32>,
    /// 地壳年龄，百万年。洋壳用热沉降模型转深度。
    pub crust_age: Vec<f32>,
    /// 所属板块编号。
    pub plate_id: Vec<u16>,
    /// 地表温度，摄氏度。
    pub temperature: Vec<f32>,
    /// 年降水量，毫米。
    pub precipitation: Vec<f32>,
    /// 距海距离，米。用于大陆度修正。
    pub distance_to_sea: Vec<f32>,
    /// 河流流量（上游汇流面积，平方米）。
    pub flow_accumulation: Vec<f32>,
    /// 年度水量平衡支持的实际湖深，米。0 表示无水，不是地形填洼容量。
    pub lake_depth: Vec<f32>,
}

impl WorldFields {
    /// 按格子数分配所有场。
    pub fn with_capacity(n: usize) -> Self {
        Self {
            elevation: vec![0.0; n],
            crust_thickness: vec![0.0; n],
            reference_crust_thickness: vec![0.0; n],
            crustal_strain: vec![0.0; n],
            orogen_age: vec![0.0; n],
            shear_strain: vec![0.0; n],
            craton_strength: vec![0.0; n],
            rift_damage: vec![0.0; n],
            potential_evaporation: vec![0.0; n],
            aridity: vec![0.0; n],
            climate: None,
            crust_type: vec![0.0; n],
            crust_age: vec![0.0; n],
            plate_id: vec![0; n],
            temperature: vec![0.0; n],
            precipitation: vec![0.0; n],
            distance_to_sea: vec![0.0; n],
            flow_accumulation: vec![0.0; n],
            lake_depth: vec![0.0; n],
        }
    }

    pub fn len(&self) -> usize {
        self.elevation.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elevation.is_empty()
    }
}

/// 板块。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Plate {
    /// 板块编号。
    pub id: u16,
    /// 欧拉极方向（单位向量），板块绕它旋转。
    pub euler_pole: [f64; 3],
    /// 旋转角速度，弧度每百万年。
    pub omega: f64,
    /// 是否以洋壳为主。
    pub oceanic: bool,
}

/// 板块边界的一段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoundaryKind {
    /// 汇聚：两侧相向运动。
    Convergent,
    /// 离散：两侧相背运动。
    Divergent,
    /// 转换：两者擦肩而过。
    Transform,
}

/// 边界的详细类型，由两侧地壳类型决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoundarySubtype {
    /// 洋-洋汇聚：岛弧加海沟。
    OceanOceanConvergent,
    /// 洋-陆汇聚：陆缘弧加海沟，安第斯型。
    OceanContinentConvergent,
    /// 陆-陆汇聚：造山带，喜马拉雅型。
    ContinentContinentConvergent,
    /// 洋-洋离散：洋中脊。
    OceanOceanDivergent,
    /// 陆-陆离散：裂谷，东非型。
    ContinentContinentDivergent,
    /// 转换断层。
    Transform,
}

/// 一个边界格子的信息。
#[derive(Debug, Clone, Copy)]
pub struct BoundaryCell {
    pub cell: u32,
    /// 相邻的另一个板块的格子。
    pub other: u32,
    pub kind: BoundaryKind,
    pub subtype: BoundarySubtype,
    /// 法向相对速度，米每百万年。正值是汇聚（当前格朝相邻格运动）。
    pub normal_velocity: f64,
    pub tangential_velocity: f64,
    pub normal: [f64; 3],
}
