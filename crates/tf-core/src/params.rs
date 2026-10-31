//! 生成参数与精度档位。
//!
//! 参数设计原则：
//! - 少而正交。用户调十来个数字就能造出完全不同的星球。
//! - 名字是人类可读的物理量，不是 `noise_scale_a`。
//! - 每个参数都带取值范围，前端可以直接生成滑杆，AI 也能理解。
//! - 精度档位单独成一层，由部署形态决定，用户不能随意调。

use serde::{Deserialize, Serialize};

/// 层级类型。严格的四种，到底为止。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelKind {
    World,
    Continent,
    Region,
    Site,
}

impl LevelKind {
    /// 该层默认格宽，单位米。
    pub fn default_cell_size_m(&self) -> f64 {
        match self {
            LevelKind::World => 10_000.0,
            LevelKind::Continent => 1_000.0,
            LevelKind::Region => 100.0,
            LevelKind::Site => 1.0,
        }
    }

    /// 该层默认的目标边长（格数）。
    pub fn default_target_cells(&self) -> u32 {
        match self {
            LevelKind::World => 0, // 世界是整球，由细分频率决定
            _ => 2048,
        }
    }

    pub fn next(&self) -> Option<LevelKind> {
        match self {
            LevelKind::World => Some(LevelKind::Continent),
            LevelKind::Continent => Some(LevelKind::Region),
            LevelKind::Region => Some(LevelKind::Site),
            LevelKind::Site => None,
        }
    }

    pub fn name_zh(&self) -> &'static str {
        match self {
            LevelKind::World => "世界",
            LevelKind::Continent => "大陆",
            LevelKind::Region => "区域",
            LevelKind::Site => "地点",
        }
    }
}

/// 模拟精度档位。
///
/// 这个不是给用户调的，是由部署形态决定：
/// - 网页版锁死 Economy，管理面板里改不了
/// - 桌面版本地部署默认 Standard，可以手动开 High
///
/// 档位同时影响两件事：世界层的细分频率，以及各阶段的迭代次数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimQuality {
    /// 最低档。给网页版用。世界层 f=256，约 65 万格，格宽 30 km。
    /// 生成时间目标：服务器 2 核 2-3 分钟。
    Economy,
    /// 标准档。桌面版默认。世界层 f=512，约 262 万格，格宽 15 km。
    Standard,
    /// 高精度。世界层 f=768，约 590 万格，格宽 10 km。
    High,
}

impl SimQuality {
    /// 世界层戈德堡球的分割频率。
    pub fn world_subdiv_freq(&self) -> u32 {
        match self {
            SimQuality::Economy => 256,
            SimQuality::Standard => 512,
            SimQuality::High => 768,
        }
    }

    /// 各阶段迭代次数倍率。低档位跑少一些迭代，视觉上差别不致命。
    pub fn iteration_scale(&self) -> f64 {
        match self {
            SimQuality::Economy => 0.45,
            SimQuality::Standard => 0.75,
            SimQuality::High => 1.0,
        }
    }

    /// 下层（大陆/区域/地点）的目标边长格数。
    pub fn sub_level_target_cells(&self) -> u32 {
        match self {
            SimQuality::Economy => 1024,
            SimQuality::Standard => 1536,
            SimQuality::High => 2048,
        }
    }

    pub fn name_zh(&self) -> &'static str {
        match self {
            SimQuality::Economy => "经济",
            SimQuality::Standard => "标准",
            SimQuality::High => "高精度",
        }
    }
}

/// 星球物理参数。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlanetParams {
    /// 星球半径，米。默认地球。
    pub radius_m: f64,
    /// 自转轴倾角，度。
    pub axial_tilt_deg: f64,
    /// 自转周期，小时。
    pub rotation_period_hours: f64,
    /// 太阳常数，W/m²。
    pub solar_constant: f64,
    /// 大气密度相对地球的倍数。影响温室效应和风应力。
    pub atmosphere_density: f64,
    /// 温室效应强度相对地球的倍数。
    pub greenhouse_factor: f64,
}

impl Default for PlanetParams {
    fn default() -> Self {
        Self {
            radius_m: crate::constants::EARTH_RADIUS_M,
            axial_tilt_deg: 23.44,
            rotation_period_hours: 24.0,
            solar_constant: crate::constants::SOLAR_CONSTANT,
            atmosphere_density: 1.0,
            greenhouse_factor: 1.0,
        }
    }
}

impl PlanetParams {
    /// 表面积，平方米。
    pub fn surface_area_m2(&self) -> f64 {
        4.0 * std::f64::consts::PI * self.radius_m * self.radius_m
    }

    /// 表面重力加速度，m/s²。
    ///
    /// 用地球的平均密度做基准，按半径线性缩放。这是简化处理，
    /// 真实行星的质量-半径关系复杂得多，但我们只需要量级正确。
    pub fn surface_gravity(&self) -> f64 {
        let earth_radius = crate::constants::EARTH_RADIUS_M;
        let ratio = self.radius_m / earth_radius;
        crate::constants::GRAVITY * ratio
    }

    /// 科里奥利参数在纬度 phi 处的值，1/s。
    pub fn coriolis(&self, lat_deg: f64) -> f64 {
        let omega = 2.0 * std::f64::consts::PI / (self.rotation_period_hours * 3600.0);
        2.0 * omega * lat_deg.to_radians().sin()
    }

    /// 赤道自转线速度，m/s。影响大气环流的强度。
    pub fn equatorial_velocity(&self) -> f64 {
        2.0 * std::f64::consts::PI * self.radius_m / (self.rotation_period_hours * 3600.0)
    }
}

/// The generation domain is independent of its viewing projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorldShape {
    #[default]
    Sphere,
    Plane,
}

/// Local landform choice; regional/site roots do not run a miniature planet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LandscapeKind {
    #[default]
    Hills,
    Mountains,
    Plains,
    Valley,
    Coast,
}

impl LandscapeKind {
    pub fn name_zh(&self) -> &'static str {
        match self {
            Self::Hills => "丘陵",
            Self::Mountains => "山地",
            Self::Plains => "平原",
            Self::Valley => "河谷",
            Self::Coast => "海岸",
        }
    }
}

/// 完整的世界生成参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenerationParams {
    /// 随机种子。同种子同参数必然产出同一世界。
    pub seed: u64,
    /// 精度档位，由部署形态锁定。
    pub quality: SimQuality,
    /// 星球物理参数。
    pub planet: PlanetParams,

    /// Starting scope. Local scopes may be generated without an ancestor.
    pub start_level: LevelKind,
    pub world_shape: WorldShape,
    /// Flat world/continent roots may be cropped at their rectangular extent.
    /// False reserves an ocean rim; local regions/sites remain cropped windows.
    pub allow_land_at_edge: bool,
    pub map_width_m: f64,
    pub map_height_m: f64,
    pub center_lat_deg: f64,
    pub center_lon_deg: f64,
    /// Continuous province scale, independent of plate ownership.
    pub continent_scale: f64,
    pub continent_count: u32,
    pub coast_complexity: f64,
    /// Duration of the kinematic reconstruction, in million years.
    pub tectonic_age_myr: f64,
    /// Fraction of initial plate seeds reserved for smaller boundary plates.
    pub minor_plate_fraction: f64,
    pub landscape: LandscapeKind,
    pub base_elevation_m: f64,
    pub terrain_relief_m: f64,
    pub regional_temperature_c: f64,
    pub regional_precipitation_mm: f64,
    pub orogeny_strength: f64,
    pub rift_strength: f64,
    pub shear_strength: f64,
    pub interior_relief: f64,
    /// Atmospheric transport uses seconds, metres, and millimetres of column water.
    pub wind_speed_m_s: f64,
    pub orographic_strength: f64,
    pub moisture_recycling: f64,
    /// Display-only preference; changing it must not change generated fields.
    pub display_grid_m: f64,

    // ---- 构造 ----
    /// 板块数量。8-30，默认 14。
    pub plate_count: u32,
    /// 目标陆地占比。0.05-0.90，默认 0.29（接近地球）。
    pub land_fraction: f64,
    /// 构造活动强度倍率。0.2-2.0。影响造山速度和地震带密度。
    pub tectonic_activity: f64,
    /// 热点数量。0-20，默认 8。
    pub hotspot_count: u32,
    /// 洋中脊扩张速率倍率。影响洋壳年龄分布和热沉降。
    pub spreading_rate: f64,

    // ---- 侵蚀 ----
    /// 侵蚀强度倍率。0-2，默认 1.0。
    pub erosion_strength: f64,
    /// 是否启用冰川侵蚀。默认自动（由气候决定）。
    pub glacier_enabled: bool,

    // ---- 气候 ----
    /// 降水量倍率。0.3-2.0。
    pub precipitation_scale: f64,
    /// 是否启用季风效应。
    pub monsoon_enabled: bool,

    // ---- 生态 ----
    /// 生物群系丰度倍率。影响沙漠和森林的比例。
    pub biome_diversity: f64,

    // ---- 美术 ----
    /// 默认地图风格。
    pub style_preset: StylePreset,
}

/// 地图视觉风格预设。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StylePreset {
    /// 写实卫星。PBR 光照、大气透视、水体反射。
    Realistic,
    /// 手绘水彩。墨线边缘、纸纹、色彩抖动。
    Watercolor,
    /// 奇幻羊皮纸。等高线、做旧纹理、复古色调。
    Parchment,
    /// 等高线测绘。分层设色、等高线、专业图例。
    Contour,
}

impl Default for GenerationParams {
    fn default() -> Self {
        Self {
            seed: 0,
            quality: SimQuality::Standard,
            planet: PlanetParams::default(),
            start_level: LevelKind::World,
            world_shape: WorldShape::Sphere,
            allow_land_at_edge: true,
            map_width_m: 8_000_000.,
            map_height_m: 5_000_000.,
            center_lat_deg: 25.,
            center_lon_deg: 0.,
            continent_scale: 1.,
            continent_count: 4,
            coast_complexity: 0.35,
            tectonic_age_myr: 80.,
            minor_plate_fraction: 0.55,
            landscape: LandscapeKind::Hills,
            base_elevation_m: 500.,
            terrain_relief_m: 600.,
            regional_temperature_c: 15.,
            regional_precipitation_mm: 900.,
            orogeny_strength: 1.,
            rift_strength: 1.,
            shear_strength: 1.,
            interior_relief: 1.,
            wind_speed_m_s: 10.,
            orographic_strength: 1.,
            moisture_recycling: 0.35,
            display_grid_m: 100_000.,
            plate_count: 14,
            land_fraction: 0.29,
            tectonic_activity: 1.0,
            hotspot_count: 8,
            spreading_rate: 1.0,
            erosion_strength: 1.0,
            glacier_enabled: true,
            precipitation_scale: 1.0,
            monsoon_enabled: true,
            biome_diversity: 1.0,
            style_preset: StylePreset::Realistic,
        }
    }
}

impl GenerationParams {
    /// Scope defaults are applied before explicit request overrides.
    pub fn for_level(level: LevelKind) -> Self {
        let mut p = Self {
            start_level: level,
            ..Self::default()
        };
        match level {
            LevelKind::World => {}
            LevelKind::Continent => {
                p.world_shape = WorldShape::Plane;
                p.continent_count = 1;
                p.land_fraction = 0.65;
                p.map_width_m = 6_000_000.;
                p.map_height_m = 4_000_000.;
                p.display_grid_m = 50_000.;
            }
            LevelKind::Region => {
                p.world_shape = WorldShape::Plane;
                p.land_fraction = 1.;
                p.map_width_m = 200_000.;
                p.map_height_m = 150_000.;
                p.display_grid_m = 5_000.;
                p.hotspot_count = 0;
            }
            LevelKind::Site => {
                p.world_shape = WorldShape::Plane;
                p.land_fraction = 1.;
                p.map_width_m = 10_000.;
                p.map_height_m = 7_500.;
                p.display_grid_m = 250.;
                p.base_elevation_m = 150.;
                p.terrain_relief_m = 80.;
                p.hotspot_count = 0;
            }
        }
        p
    }
    pub fn is_local(&self) -> bool {
        matches!(self.start_level, LevelKind::Region | LevelKind::Site)
    }
    /// 校验参数是否在合法范围内，返回第一个越界项的描述。
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=12).contains(&self.continent_count) {
            return Err("主要大陆数量必须为1～12".into());
        }
        if !(8..=30).contains(&self.plate_count) {
            return Err(format!("板块数量 {} 超出范围 8-30", self.plate_count));
        }
        if !(0.0..=1.0).contains(&self.land_fraction) {
            return Err(format!("陆地占比 {} 超出范围 0-1", self.land_fraction));
        }
        if !(0.2..=2.0).contains(&self.tectonic_activity) {
            return Err(format!(
                "构造活动强度 {} 超出范围 0.2-2.0",
                self.tectonic_activity
            ));
        }
        if self.hotspot_count > 20 {
            return Err(format!("热点数量 {} 超出范围 0-20", self.hotspot_count));
        }
        if !(0.0..=2.0).contains(&self.erosion_strength) {
            return Err(format!("侵蚀强度 {} 超出范围 0-2", self.erosion_strength));
        }
        if !(0.3..=2.0).contains(&self.precipitation_scale) {
            return Err(format!(
                "降水倍率 {} 超出范围 0.3-2.0",
                self.precipitation_scale
            ));
        }
        if !(1.0e6..=5.0e7).contains(&self.planet.radius_m) {
            return Err(format!(
                "星球半径 {} m 超出范围 1000-50000 km",
                self.planet.radius_m
            ));
        }
        if !(0.0..=90.0).contains(&self.planet.axial_tilt_deg) {
            return Err(format!(
                "自转轴倾角 {} 超出范围 0-90 度",
                self.planet.axial_tilt_deg
            ));
        }
        if !(1.0..=1000.0).contains(&self.planet.rotation_period_hours) {
            return Err("自转周期必须为正".to_string());
        }
        for (name, value, min, max) in [
            ("太阳辐射", self.planet.solar_constant, 100., 4000.),
            ("大气密度", self.planet.atmosphere_density, 0.05, 10.),
            ("温室效应", self.planet.greenhouse_factor, 0.1, 3.),
            ("扩张速率", self.spreading_rate, 0.1, 3.),
            ("生态丰度", self.biome_diversity, 0.3, 2.),
            ("大陆尺度", self.continent_scale, 0.3, 3.),
            ("海岸复杂度", self.coast_complexity, 0., 1.),
            ("构造演化时间", self.tectonic_age_myr, 0., 250.),
            ("小板块比例", self.minor_plate_fraction, 0.2, 0.8),
            ("局部基准海拔", self.base_elevation_m, -1000., 8000.),
            ("局部起伏", self.terrain_relief_m, 0., 6000.),
            ("局部基准温度", self.regional_temperature_c, -60., 50.),
            ("局部年均降雨", self.regional_precipitation_mm, 0., 12000.),
            ("造山强度", self.orogeny_strength, 0., 3.),
            ("裂谷强度", self.rift_strength, 0., 3.),
            ("走滑强度", self.shear_strength, 0., 3.),
            ("内陆起伏", self.interior_relief, 0., 3.),
            ("盛行风速", self.wind_speed_m_s, 1., 40.),
            ("地形雨强度", self.orographic_strength, 0., 3.),
            ("水汽再循环", self.moisture_recycling, 0., 0.8),
            ("显示格距", self.display_grid_m, 0., 10_000_000.),
            ("地图宽度", self.map_width_m, 4., 200_000_000.),
            ("地图高度", self.map_height_m, 4., 100_000_000.),
            ("参考纬度", self.center_lat_deg, -85., 85.),
            ("参考经度", self.center_lon_deg, -180., 180.),
        ] {
            if !value.is_finite() || !(min..=max).contains(&value) {
                return Err(format!("{name} 超出范围 {min}～{max}"));
            }
        }
        if !self.is_local() && self.world_shape == WorldShape::Plane && self.tectonic_age_myr > 0. {
            if self.map_width_m.min(self.map_height_m) < 500_000. {
                return Err("板块演化需要至少 500 km 的范围；较小地图请选择区域或地点".into());
            }
            let cols = (self.map_width_m / 80000.).round().clamp(16., 160.);
            let spacing = self.map_width_m / cols;
            let steps = self.tectonic_age_myr
                * 3.
                * 75000.
                * self.tectonic_activity
                * self.spreading_rate.max(1.)
                / spacing
                / 0.32;
            if steps > 3500. {
                return Err("范围与演化时长需要过多构造步骤；请扩大范围或缩短演化时间".into());
            }
        }
        Ok(())
    }

    /// 参数的中文摘要，给 AI 和 UI 用。
    pub fn summary_zh(&self) -> String {
        if self.is_local() {
            return format!("独立{}，范围 {:.1}×{:.1} km，{}，基准海拔 {:.0} m，起伏强度 {:.0} m，基准年均温 {:.1}℃，年降水 {:.0} mm",self.start_level.name_zh(),self.map_width_m/1000.,self.map_height_m/1000.,self.landscape.name_zh(),self.base_elevation_m,self.terrain_relief_m,self.regional_temperature_c,self.regional_precipitation_mm);
        }
        if self.start_level == LevelKind::Continent {
            return format!(
                "独立大陆，范围 {:.0}×{:.0} km，陆地目标 {:.0}%，海岸复杂度 {:.2}，构造强度 {:.1}",
                self.map_width_m / 1000.,
                self.map_height_m / 1000.,
                self.land_fraction * 100.,
                self.coast_complexity,
                self.tectonic_activity
            );
        }
        format!(
            "星球半径 {:.0} km，板块 {} 块，陆地占比 {:.0}%，构造强度 {:.1}，热点 {} 个，精度档位{}",
            self.planet.radius_m / 1000.0,
            self.plate_count,
            self.land_fraction * 100.0,
            self.tectonic_activity,
            self.hotspot_count,
            self.quality.name_zh(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_params_are_valid() {
        assert!(GenerationParams::default().validate().is_ok());
    }

    #[test]
    fn out_of_range_params_are_rejected() {
        let mut p = GenerationParams::default();
        p.plate_count = 100;
        assert!(p.validate().is_err());

        let mut p = GenerationParams::default();
        p.land_fraction = 1.01;
        assert!(p.validate().is_err());

        let mut p = GenerationParams::default();
        p.planet.radius_m = 1.0;
        assert!(p.validate().is_err());
    }

    #[test]
    fn tectonic_runtime_budget_rejects_tiny_or_excessively_long_plane_runs() {
        let mut p = GenerationParams::for_level(LevelKind::Continent);
        p.map_width_m = 4.;
        p.map_height_m = 4.;
        assert!(p.validate().is_err());
        p.map_width_m = 500000.;
        p.map_height_m = 500000.;
        p.tectonic_age_myr = 250.;
        p.tectonic_activity = 2.;
        assert!(p.validate().is_err());
        assert!(GenerationParams::for_level(LevelKind::Site)
            .validate()
            .is_ok());
    }

    #[test]
    fn quality_affects_resolution() {
        assert!(SimQuality::Economy.world_subdiv_freq() < SimQuality::High.world_subdiv_freq());
        assert!(SimQuality::Economy.iteration_scale() < SimQuality::High.iteration_scale());
    }

    #[test]
    fn earth_like_gravity() {
        let p = PlanetParams::default();
        let g = p.surface_gravity();
        assert!((g - 9.80665).abs() < 0.01, "gravity: {}", g);
    }

    #[test]
    fn lower_land_fraction_is_valid() {
        let mut p = GenerationParams::default();
        p.land_fraction = 0.05;
        assert!(p.validate().is_ok());
    }

    #[test]
    fn level_chain_is_terminating() {
        let mut lv = LevelKind::World;
        let mut count = 1;
        while let Some(next) = lv.next() {
            lv = next;
            count += 1;
        }
        assert_eq!(count, 4);
        assert_eq!(lv, LevelKind::Site);
    }
}
