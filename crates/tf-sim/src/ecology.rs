//! 阶段 5：生态、土壤、矿藏。
//!
//! 生物群系由温度和降水决定（Whittaker 图），
//! 土壤由 Jenny 方程的五要素决定，
//! 矿藏跟地质历史绑定——这是「成因驱动」最能体现的地方。

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tf_grid::GridOps;

use crate::fields::{BoundarySubtype, WorldFields};

/// 生物群系类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Biome {
    Ocean = 0,
    IceSheet = 1,
    Tundra = 2,
    BorealForest = 3,
    TemperateForest = 4,
    TemperateGrassland = 5,
    Mediterranean = 6,
    Desert = 7,
    Savanna = 8,
    TropicalRainforest = 9,
    TropicalSeasonalForest = 10,
    AlpineMeadow = 11,
    Wetland = 12,
    InlandWater = 13,
}

impl Biome {
    pub fn name_zh(&self) -> &'static str {
        match self {
            Biome::Ocean => "海洋",
            Biome::IceSheet => "冰盖",
            Biome::Tundra => "苔原",
            Biome::BorealForest => "针叶林",
            Biome::TemperateForest => "温带阔叶林",
            Biome::TemperateGrassland => "温带草原",
            Biome::Mediterranean => "地中海灌丛",
            Biome::Desert => "沙漠",
            Biome::Savanna => "稀树草原",
            Biome::TropicalRainforest => "热带雨林",
            Biome::TropicalSeasonalForest => "热带季雨林",
            Biome::AlpineMeadow => "高山草甸",
            Biome::Wetland => "湿地",
            Biome::InlandWater => "内陆水体",
        }
    }

    /// 该群系的典型颜色（用于渲染）。RGB。
    pub fn color(&self) -> [u8; 3] {
        match self {
            Biome::Ocean => [30, 70, 120],
            Biome::IceSheet => [240, 245, 250],
            Biome::Tundra => [150, 160, 140],
            Biome::BorealForest => [45, 80, 55],
            Biome::TemperateForest => [60, 110, 50],
            Biome::TemperateGrassland => [140, 160, 80],
            Biome::Mediterranean => [150, 150, 90],
            Biome::Desert => [210, 190, 140],
            Biome::Savanna => [180, 170, 90],
            Biome::TropicalRainforest => [20, 90, 35],
            Biome::TropicalSeasonalForest => [50, 120, 45],
            Biome::AlpineMeadow => [120, 140, 100],
            Biome::Wetland => [70, 110, 80],
            Biome::InlandWater => [51, 140, 161],
        }
    }
}

/// 土壤类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum SoilType {
    None = 0,
    Podzol = 1,
    Chernozem = 2,
    Ferralsol = 3,
    Calcisol = 4,
    Histosol = 5,
    Andosol = 6,
    Lithosol = 7,
    Alluvial = 8,
}

/// 计算结果。
pub struct EcologyResult {
    pub biome: Vec<u8>,
    pub soil: Vec<u8>,
    pub soil_depth_m: Vec<f32>,
}

/// 运行生态模拟。
pub fn run_ecology<G: GridOps>(grid: &G, fields: &WorldFields) -> EcologyResult {
    run_ecology_with_diversity(grid, fields, 1.)
}

/// Diversity controls the width of mixed ecotones, without replacing climate.
pub fn run_ecology_with_diversity<G: GridOps>(
    grid: &G,
    fields: &WorldFields,
    diversity: f64,
) -> EcologyResult {
    let n = grid.cell_count();
    let mut biome = vec![0u8; n];
    let mut soil = vec![0u8; n];
    let mut soil_depth = vec![0.0f32; n];
    let climate_grid = fields.climate.as_ref().map(|c| c.grid.build());

    // 生物群系：温度 x 降水 -> Whittaker 图
    //
    // 关键：分类前对气候量加低频噪声扰动。
    //
    // 直接用阈值分类会产生沿等温线/等降水线的规则条带，
    // 因为温度本身是纬度和高程的平滑函数。真实世界的群系边界
    // 是斑驳的——同一片草原里会嵌着小片林地，沙漠边缘参差不齐。
    //
    // 噪声幅度约相当于 1.5 度温差、150 mm 降水差，
    // 足以打散规则的等值线，又不会改变大尺度格局。
    let jitter = tf_core::noise::SimplexNoise::new(0x51ED_2701);
    biome.par_iter_mut().enumerate().for_each(|(i, b)| {
        if fields.elevation[i] <= 0.0 {
            *b = Biome::Ocean as u8;
            return;
        }
        let p = grid.center_dir(i).as_vec();
        // 两个不同频率的扰动：大尺度做区域差异，小尺度做斑驳
        let big = jitter.fbm(p.x * 4.0, p.y * 4.0, p.z * 4.0, 2, 2.0, 0.5);
        let small = jitter.fbm(p.x * 16.0, p.y * 16.0, p.z * 16.0, 2, 2.0, 0.5);
        let mix = ((big * 0.65 + small * 0.35) * diversity.clamp(0.3, 2.)) as f32;

        let t = fields.temperature[i] + mix * 1.8;
        let prec = (fields.precipitation[i] * (1.0 + mix * 0.22)).max(0.0);

        *b = if let (Some(atlas), Some(climate_grid)) = (&fields.climate, &climate_grid) {
            let (temperature, rain, pet) = atlas.sample(climate_grid, grid.center_dir(i), t, prec);
            classify_seasonal(&temperature, &rain, &pet, fields.elevation[i]) as u8
        } else {
            // Legacy engineering files have no monthly state; retain their explicit
            // annual-only fallback rather than inventing an observed wet season.
            classify_biome(t, prec, fields.elevation[i]) as u8
        };
        if let Some(ice) = &fields.cryosphere {
            if ice.land_ice_m[i] >= 1. || ice.snow_cover[i] > 0.8 {
                *b = Biome::IceSheet as u8;
            } else if *b == Biome::IceSheet as u8 {
                // Cold bare ground is not an ice sheet without accumulated ice.
                *b = Biome::Tundra as u8;
            }
        }
        if fields.lake_depth[i] >= 0.1 {
            let shallow = fields.lake_depth[i] <= 2.5 && fields.temperature[i] > 0.;
            *b = if shallow {
                Biome::Wetland as u8
            } else {
                Biome::InlandWater as u8
            };
        }
    });

    // 土壤：Jenny 方程的简化实现
    for i in 0..n {
        let b = biome[i];
        if b == Biome::Ocean as u8 || b == Biome::InlandWater as u8 || b == Biome::IceSheet as u8 {
            continue;
        }
        let (t, d) = classify_soil(
            fields.temperature[i],
            fields.precipitation[i],
            fields.elevation[i],
            b,
        );
        soil[i] = t as u8;
        soil_depth[i] = d;
    }

    // 河流沿线算冲积土
    for i in 0..n {
        if fields.flow_accumulation[i] > 5e9
            && fields.elevation[i] > 0.0
            && biome[i] != Biome::InlandWater as u8
            && biome[i] != Biome::IceSheet as u8
        {
            soil[i] = SoilType::Alluvial as u8;
            soil_depth[i] = soil_depth[i].max(1.5);
        }
    }

    let _ = grid;
    EcologyResult {
        biome,
        soil,
        soil_depth_m: soil_depth,
    }
}

/// Whittaker 图：温度 x 降水 -> 生物群系。
pub fn classify_biome(temp_c: f32, precip_mm: f32, elevation: f32) -> Biome {
    let t = temp_c as f64;
    let p = precip_mm as f64;

    // 极冷 = 冰盖或苔原
    if t < -12.0 {
        return Biome::IceSheet;
    }
    if t < -2.0 {
        return if p > 250.0 {
            Biome::Tundra
        } else {
            Biome::IceSheet
        };
    }

    // 高海拔 = 高山草甸
    if elevation > 4000.0 && t < 5.0 {
        return Biome::AlpineMeadow;
    }

    // 干旱判定：降水少就是沙漠
    if p < 200.0 {
        return Biome::Desert;
    }
    if p < 400.0 {
        return if t > 18.0 {
            Biome::Savanna
        } else {
            Biome::TemperateGrassland
        };
    }

    // 按温度和降水的组合分类
    if t < 5.0 {
        if p > 400.0 {
            Biome::BorealForest
        } else {
            Biome::TemperateGrassland
        }
    } else if t < 20.0 {
        if p > 900.0 {
            Biome::TemperateForest
        } else if p > 500.0 {
            Biome::TemperateForest
        } else if p > 320.0 {
            Biome::TemperateGrassland
        } else {
            Biome::Mediterranean
        }
    } else {
        // 热带
        if p > 2000.0 {
            Biome::TropicalRainforest
        } else if p > 1200.0 {
            Biome::TropicalSeasonalForest
        } else if p > 600.0 {
            Biome::Savanna
        } else {
            Biome::Desert
        }
    }
}

/// Biome heuristic based on seasonal water availability, not a target desert area.
/// P/PET follows the aridity-index convention; climate classes and vegetation
/// are not claimed to be an exact Köppen–Geiger implementation.
pub fn classify_seasonal(
    temperature: &[f32; 12],
    precipitation: &[f32; 12],
    pet: &[f32; 12],
    elevation: f32,
) -> Biome {
    let warm = temperature
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let cold = temperature.iter().copied().fold(f32::INFINITY, f32::min);
    let mean = temperature.iter().sum::<f32>() / 12.;
    if warm < 0. {
        return Biome::IceSheet;
    }
    if warm < 10. {
        return if elevation > 4000. {
            Biome::AlpineMeadow
        } else {
            Biome::Tundra
        };
    }
    let annual_rain = precipitation.iter().sum::<f32>();
    let annual_pet = pet.iter().sum::<f32>().max(1.);
    let aridity = annual_rain / annual_pet;
    if aridity < 0.20 {
        return Biome::Desert;
    }
    if aridity < 0.50 {
        return if cold >= 18. {
            Biome::Savanna
        } else {
            Biome::TemperateGrassland
        };
    }
    let growing = temperature.iter().filter(|&&t| t >= 5.).count();
    if cold >= 18. {
        let dry = precipitation.iter().copied().fold(f32::INFINITY, f32::min);
        if dry >= 60. && annual_rain > 1500. {
            return Biome::TropicalRainforest;
        }
        let droughts = precipitation
            .iter()
            .zip(pet)
            .filter(|(p, e)| **p < **e * 0.5)
            .count();
        return if droughts >= 4 {
            Biome::Savanna
        } else {
            Biome::TropicalSeasonalForest
        };
    }
    if cold < -3. && growing < 7 {
        return Biome::BorealForest;
    }
    let mut months: Vec<_> = (0..12).collect();
    months.sort_by(|&a, &b| temperature[b].total_cmp(&temperature[a]));
    let summer = months
        .iter()
        .take(6)
        .map(|&m| precipitation[m])
        .sum::<f32>();
    let winter = months
        .iter()
        .skip(6)
        .map(|&m| precipitation[m])
        .sum::<f32>();
    let summer_pet = months.iter().take(6).map(|&m| pet[m]).sum::<f32>();
    if cold > 0. && summer < summer_pet * 0.5 && winter > summer * 1.8 {
        return Biome::Mediterranean;
    }
    if aridity < 0.65 || mean < -2. {
        Biome::TemperateGrassland
    } else {
        Biome::TemperateForest
    }
}

#[cfg(test)]
mod seasonal_tests {
    use super::*;
    #[test]
    fn dry_land_shallow_wetlands_and_deep_lakes_remain_distinct() {
        use tf_grid::GridOps;
        let grid = tf_grid::FlatGrid::new(6, 4, 6000., 25., 0., 6371000.);
        let mut f = WorldFields::with_capacity(grid.cell_count());
        f.elevation.fill(100.);
        f.temperature.fill(15.);
        f.precipitation.fill(800.);
        f.lake_depth[1] = 1.;
        f.lake_depth[2] = 25.;
        f.lake_depth[3] = 1.;
        f.temperature[3] = -5.;
        f.flow_accumulation[2] = 1e12;
        let ecology = run_ecology(&grid, &f);
        assert_ne!(ecology.biome[0], Biome::Wetland as u8);
        assert_ne!(ecology.biome[0], Biome::InlandWater as u8);
        assert_eq!(ecology.biome[1], Biome::Wetland as u8);
        for i in [2, 3] {
            assert_eq!(ecology.biome[i], Biome::InlandWater as u8);
            assert_eq!(ecology.soil[i], SoilType::None as u8);
            assert_eq!(ecology.soil_depth_m[i], 0.);
        }
    }
    #[test]
    fn the_same_rainfall_can_support_cold_grassland_or_hot_desert() {
        let rain = [20.; 12];
        assert_ne!(
            classify_seasonal(&[7.; 12], &rain, &[30.; 12], 100.),
            Biome::Desert
        );
        assert_eq!(
            classify_seasonal(&[29.; 12], &rain, &[160.; 12], 100.),
            Biome::Desert
        );
    }
    #[test]
    fn dry_summers_are_distinct_from_equal_annual_rainfall_with_wet_summers() {
        let t = [8., 9., 12., 16., 20., 24., 27., 26., 22., 17., 12., 9.];
        let p = [120., 100., 70., 40., 15., 5., 5., 5., 15., 60., 90., 120.];
        let e = [
            20., 25., 40., 65., 90., 120., 145., 130., 90., 60., 35., 20.,
        ];
        assert_eq!(classify_seasonal(&t, &p, &e, 100.), Biome::Mediterranean);
        let mut flipped = p;
        flipped.rotate_left(6);
        assert_ne!(
            classify_seasonal(&t, &flipped, &e, 100.),
            Biome::Mediterranean
        );
    }
}

/// 土壤分类。返回 (类型, 厚度米)。
fn classify_soil(temp: f32, precip: f32, elevation: f32, biome: u8) -> (SoilType, f32) {
    let t = temp as f64;
    let p = precip as f64;

    // 陡峭或高海拔 = 岩成土，土层薄
    if elevation > 4500.0 {
        return (SoilType::Lithosol, 0.2);
    }
    // 冰盖下没有土壤
    if biome == Biome::IceSheet as u8 {
        return (SoilType::None, 0.0);
    }
    // 湿热 = 强淋溶，酸性铁铝土
    if t > 22.0 && p > 1500.0 {
        return (SoilType::Ferralsol, 8.0);
    }
    // 温带草原 = 黑土，最肥沃
    if (3.0..18.0).contains(&t) && (300.0..700.0).contains(&p) {
        return (SoilType::Chernozem, 1.5);
    }
    // 干旱 = 钙积土
    if p < 350.0 {
        return (SoilType::Calcisol, 0.8);
    }
    // 寒冷针叶林 = 灰化土
    if t < 5.0 {
        return (SoilType::Podzol, 0.9);
    }
    // 湿地 = 有机土
    if biome == Biome::Wetland as u8 || (p > 2000.0 && t < 15.0) {
        return (SoilType::Histosol, 2.5);
    }
    // 默认：温带森林下的棕壤
    (SoilType::Podzol, 1.2)
}

/// 一个矿点。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MineralDeposit {
    pub cell: u32,
    pub kind: MineralKind,
    /// 成因说明，给用户和 AI 看。
    pub genesis: String,
    /// 富集程度 0-1。
    pub abundance: f32,
}

/// 矿产类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MineralKind {
    PorphyryCopper,
    MassiveSulfide,
    BandedIron,
    OrogenicGold,
    Coal,
    Petroleum,
    RareEarth,
    Salt,
}

impl MineralKind {
    pub fn name_zh(&self) -> &'static str {
        match self {
            MineralKind::PorphyryCopper => "斑岩铜矿",
            MineralKind::MassiveSulfide => "块状硫化物",
            MineralKind::BandedIron => "带状铁建造",
            MineralKind::OrogenicGold => "造山型金矿",
            MineralKind::Coal => "煤矿",
            MineralKind::Petroleum => "石油",
            MineralKind::RareEarth => "稀土",
            MineralKind::Salt => "盐矿",
        }
    }
}

/// 生成矿藏。
///
/// 矿不是随机撒点，是跟地质历史绑定。每一处都有成因。
pub fn find_minerals<G: GridOps>(
    grid: &G,
    fields: &WorldFields,
    boundaries: &[crate::fields::BoundaryCell],
    ecology: &EcologyResult,
    rng: &mut tf_core::DeterministicRng,
) -> Vec<MineralDeposit> {
    let mut deposits = Vec::new();

    // 斑岩铜矿：火山弧附近 + 汇聚边界
    for b in boundaries {
        if b.kind != crate::fields::BoundaryKind::Convergent {
            continue;
        }
        if rng.next_f64() > 0.05 {
            continue;
        }
        // 在边界向陆侧 2-3 环找位置
        let mut candidates = vec![b.cell];
        for &nb in grid.neighbors(b.cell as usize) {
            candidates.push(nb);
        }
        if let Some(&cell) = candidates.get(rng.below(candidates.len() as u32) as usize) {
            if fields.elevation[cell as usize] > 0.0 {
                deposits.push(MineralDeposit {
                    cell,
                    kind: MineralKind::PorphyryCopper,
                    genesis: format!(
                        "俯冲带岩浆侵入形成。位于{}边界向陆侧，岩浆上升过程中铜富集。",
                        match b.subtype {
                            BoundarySubtype::OceanOceanConvergent => "洋洋汇聚",
                            BoundarySubtype::OceanContinentConvergent => "洋陆汇聚",
                            _ => "汇聚",
                        }
                    ),
                    abundance: rng.range_f64(0.3, 1.0) as f32,
                });
            }
        }
    }

    // 煤矿：大陆内部 + 沉积岩 + 温带/亚热带湿润
    for i in 0..grid.cell_count() {
        if fields.elevation[i] <= 0.0 {
            continue;
        }
        let b = ecology.biome[i];
        let is_wet_temperate = b == Biome::TemperateForest as u8
            || b == Biome::TropicalSeasonalForest as u8
            || b == Biome::BorealForest as u8;
        if is_wet_temperate && fields.distance_to_sea[i] > 200_000.0 && rng.next_f64() < 0.0015 {
            deposits.push(MineralDeposit {
                cell: i as u32,
                kind: MineralKind::Coal,
                genesis: "古沉积盆地中的植物遗骸经地质作用形成。该地历史上是湿润的沉积环境。"
                    .to_string(),
                abundance: rng.range_f64(0.4, 1.0) as f32,
            });
        }
    }

    // 盐矿：干旱区封闭盆地
    for i in 0..grid.cell_count() {
        if fields.elevation[i] <= 0.0 {
            continue;
        }
        if ecology.biome[i] != Biome::Desert as u8 {
            continue;
        }
        if fields.lake_depth[i] > 0.0 && rng.next_f64() < 0.02 {
            deposits.push(MineralDeposit {
                cell: i as u32,
                kind: MineralKind::Salt,
                genesis: "干旱封闭盆地中的湖水蒸发，盐类沉积成矿。".to_string(),
                abundance: rng.range_f64(0.5, 1.0) as f32,
            });
        }
    }

    // 造山型金矿：陆陆汇聚带
    for b in boundaries {
        if b.subtype != BoundarySubtype::ContinentContinentConvergent {
            continue;
        }
        if rng.next_f64() < 0.03 {
            deposits.push(MineralDeposit {
                cell: b.cell,
                kind: MineralKind::OrogenicGold,
                genesis: "陆陆碰撞带的变质热液活动形成。碰撞造山时流体沿断裂上升，金富集成矿。"
                    .to_string(),
                abundance: rng.range_f64(0.2, 0.8) as f32,
            });
        }
    }

    deposits
}
