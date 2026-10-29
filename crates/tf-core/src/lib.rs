//! TerraForge 核心基础库。
//!
//! 这里只放与具体算法无关的底层设施：球面几何、确定性随机、参数定义。
//! 任何会被 tf-grid / tf-sim 同时用到的东西都应该放在这里。

pub mod geo;
pub mod noise;
pub mod params;
pub mod rng;

pub use geo::{GeoPoint, UnitVec3, EARTH_RADIUS_M};
pub use params::{GenerationParams, LevelKind, PlanetParams};
pub use rng::DeterministicRng;

/// 物理常量，单位统一用 SI（米、秒、开尔文）。
pub mod constants {
    /// 地球平均半径，米。
    pub const EARTH_RADIUS_M: f64 = 6_371_000.0;
    /// 地球表面积，平方米。
    pub const EARTH_SURFACE_M2: f64 = 5.100_656_5e14;
    /// 海水密度，kg/m³。
    pub const WATER_DENSITY: f64 = 1025.0;
    /// 地壳密度，kg/m³。
    pub const CRUST_DENSITY: f64 = 2800.0;
    /// 地幔密度，kg/m³。
    pub const MANTLE_DENSITY: f64 = 3300.0;
    /// 标准重力加速度，m/s²。
    pub const GRAVITY: f64 = 9.806_65;
    /// 太阳常数，W/m²。
    pub const SOLAR_CONSTANT: f64 = 1361.0;
    /// 斯特藩-玻尔兹曼常数。
    pub const STEFAN_BOLTZMANN: f64 = 5.670_374_419e-8;
    /// 干绝热递减率，K/m。用于焚风计算。
    pub const DRY_ADIABATIC_LAPSE: f64 = 9.8e-3;
    /// 湿绝热递减率，K/m。
    pub const MOIST_ADIABATIC_LAPSE: f64 = 5.0e-3;
    /// 标准大气温度递减率，K/m。用于高程温度修正。
    pub const ENV_LAPSE_RATE: f64 = 6.5e-3;
}
