//! TerraForge 物理模拟引擎。
//!
//! # 阶段划分
//!
//! 0. 星球参数推导：从半径、自转、辐照推导重力、科里奥利、温度基准
//! 1. 板块构造：地幔对流驱动板块漂移，产生地壳厚度场
//! 2. 构造地貌：热沉降、俯冲、造山、裂谷
//! 3. 侵蚀水文：填洼、流向、汇流、河流下切、坡面扩散
//! 4. 气候：温度、风场、水汽平流降水、洋流
//! 5. 生态：生物群系、土壤、矿藏
//! 6. 覆盖层：植被、农田
//!
//! 所有阶段都作用在同一个 WorldFields 上，按顺序推进。

pub mod circulation;
pub mod climate;
pub mod downscale;
pub mod ecology;
pub mod erosion;
pub mod evolution;
pub mod fields;
pub mod geography;
pub mod geology;
pub mod lakes;
pub mod landforms;
pub mod lifecycle;
pub mod pipeline;
pub mod plates;
pub mod preview;
pub mod scientific;
pub mod scopes;
pub mod seasonal;

pub use fields::{BoundaryCell, BoundaryKind, BoundarySubtype, Plate, WorldFields};
pub use pipeline::{
    generate, generate_cancellable, generate_observed, generate_with_frames, FrameFn,
    GeneratedWorld, ProgressFn,
};
pub use plates::{build_plates, TectonicState};

use tf_core::params::GenerationParams;
use tf_core::DeterministicRng;
use tf_grid::GridOps;

/// 模拟阶段的编号，用于进度上报。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    PlanetSetup,
    Tectonics,
    Landforms,
    Erosion,
    Climate,
    Ecology,
    Cover,
    Finalize,
}

impl Stage {
    pub fn name_zh(&self) -> &'static str {
        match self {
            Stage::PlanetSetup => "星球参数",
            Stage::Tectonics => "板块构造",
            Stage::Landforms => "构造地貌",
            Stage::Erosion => "侵蚀水文",
            Stage::Climate => "气候系统",
            Stage::Ecology => "生态矿藏",
            Stage::Cover => "覆盖层",
            Stage::Finalize => "收尾",
        }
    }

    /// 该阶段在总进度里的权重，用于算 ETA。
    pub fn weight(&self) -> f64 {
        match self {
            Stage::PlanetSetup => 0.01,
            Stage::Tectonics => 0.12,
            Stage::Landforms => 0.15,
            Stage::Erosion => 0.38,
            Stage::Climate => 0.25,
            Stage::Ecology => 0.05,
            Stage::Cover => 0.03,
            Stage::Finalize => 0.01,
        }
    }
}

/// 一次模拟的上下文。
pub struct SimContext {
    pub params: GenerationParams,
    pub rng: DeterministicRng,
    pub tectonic: Option<TectonicState>,
    pub fields: WorldFields,
}

impl SimContext {
    pub fn new<G: GridOps>(grid: &G, params: GenerationParams) -> Self {
        let rng = DeterministicRng::from_seed(params.seed);
        Self {
            params,
            rng,
            tectonic: None,
            fields: WorldFields::with_capacity(grid.cell_count()),
        }
    }
}

pub mod terrain;

mod climate_transport;
