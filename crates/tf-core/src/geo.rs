//! 球面几何与地理坐标。
//!
//! 内部一律使用单位向量做球面运算，避免经纬度在极点附近的奇异性和
//! 跨 180 度经线的环绕问题。只在输入输出和 UI 展示时才转成经纬度。

use glam::DVec3;
use serde::{Deserialize, Serialize};

pub const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// 单位球面上的一个点。构造时保证已归一化。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitVec3(DVec3);

impl UnitVec3 {
    /// 从已归一化的向量构造。debug 下会检查。
    #[inline]
    pub fn from_normalized(v: DVec3) -> Self {
        debug_assert!(
            (v.length() - 1.0).abs() < 1e-9,
            "vector not normalized: {:?}",
            v
        );
        Self(v)
    }

    /// 任意非零向量归一化后构造。
    #[inline]
    pub fn from_vec(v: DVec3) -> Option<Self> {
        let len = v.length();
        if len < 1e-12 {
            None
        } else {
            Some(Self(v / len))
        }
    }

    #[inline]
    pub fn as_vec(&self) -> DVec3 {
        self.0
    }

    /// 球面大圆距离（弧长），单位米。
    #[inline]
    pub fn arc_distance_m(&self, other: &Self, radius_m: f64) -> f64 {
        // 用 atan2 形式，比 acos(dot) 在近距和远距都更稳定
        let cross = self.0.cross(other.0).length();
        let dot = self.0.dot(other.0);
        cross.atan2(dot) * radius_m
    }

    /// 球面夹角，弧度。
    #[inline]
    pub fn angle_to(&self, other: &Self) -> f64 {
        self.0.cross(other.0).length().atan2(self.0.dot(other.0))
    }

    /// 在切平面上的一个正交基，用于把球面方向映射到局部平面。
    #[inline]
    pub fn tangent_frame(&self) -> (DVec3, DVec3) {
        // +longitude is east; keep north aligned with +latitude, including near poles.
        let horizontal = DVec3::new(-self.0.z, 0.0, self.0.x);
        let east = if horizontal.length() > 1e-12 {
            horizontal.normalize()
        } else {
            DVec3::Z
        };
        let north = east.cross(self.0).normalize();
        (east, north)
    }

    /// 从本点出发，沿给定方位角和距离走一段球面。
    ///
    /// bearing 是从正北顺时针的角度（弧度），distance 是弧长（弧度）。
    #[inline]
    pub fn offset(&self, bearing: f64, angular_distance: f64) -> Self {
        let (east, north) = self.tangent_frame();
        let dir = north * bearing.cos() + east * bearing.sin();
        let v = self.0 * angular_distance.cos() + dir * angular_distance.sin();
        Self(v.normalize())
    }

    /// 转经纬度（度数）。纬度 [-90, 90]，经度 [-180, 180)。
    #[inline]
    pub fn to_lat_lon_deg(&self) -> (f64, f64) {
        let lat = self.0.y.clamp(-1.0, 1.0).asin().to_degrees();
        let lon = self.0.z.atan2(self.0.x).to_degrees();
        (lat, lon)
    }

    /// 从经纬度（度数）构造。
    #[inline]
    pub fn from_lat_lon_deg(lat_deg: f64, lon_deg: f64) -> Self {
        let lat = lat_deg.to_radians();
        let lon = lon_deg.to_radians();
        let (sl, cl) = lat.sin_cos();
        let (so, co) = lon.sin_cos();
        Self(DVec3::new(cl * co, sl, cl * so))
    }
}

/// 面向用户的地理坐标，度数表示。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub lat_deg: f64,
    pub lon_deg: f64,
}

impl GeoPoint {
    pub fn new(lat_deg: f64, lon_deg: f64) -> Self {
        Self { lat_deg, lon_deg }
    }

    pub fn to_unit(&self) -> UnitVec3 {
        UnitVec3::from_lat_lon_deg(self.lat_deg, self.lon_deg)
    }

    pub fn from_unit(v: UnitVec3) -> Self {
        let (lat_deg, lon_deg) = v.to_lat_lon_deg();
        Self { lat_deg, lon_deg }
    }
}

/// 斐波那契球面分布，用于生成均匀分布的种子点。
///
/// 返回 n 个近似均匀分布在单位球面上的点。
pub fn fibonacci_sphere(n: usize) -> Vec<UnitVec3> {
    if n == 0 {
        return Vec::new();
    }
    let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - (i as f64 + 0.5) * 2.0 / n as f64;
            let radius = (1.0 - y * y).max(0.0).sqrt();
            let theta = golden_angle * i as f64;
            UnitVec3::from_normalized(DVec3::new(theta.cos() * radius, y, theta.sin() * radius))
        })
        .collect()
}

/// 旋转一个方向向量。用轴角形式。
pub fn rotate_axis_angle(v: DVec3, axis: DVec3, angle_rad: f64) -> DVec3 {
    let (s, c) = angle_rad.sin_cos();
    let k = axis.normalize();
    v * c + k.cross(v) * s + k * (k.dot(v)) * (1.0 - c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compass_bearings_increase_correct_coordinate() {
        let p = UnitVec3::from_lat_lon_deg(30., 100.);
        let east = p
            .offset(std::f64::consts::FRAC_PI_2, 0.001)
            .to_lat_lon_deg();
        let north = p.offset(0., 0.001).to_lat_lon_deg();
        assert!(east.1 > 100.);
        assert!(north.0 > 30.);
    }
    #[test]
    fn lat_lon_roundtrip() {
        for &(lat, lon) in &[(0.0, 0.0), (45.0, 90.0), (-30.0, -120.0), (89.0, 179.0)] {
            let p = GeoPoint::new(lat, lon);
            let back = GeoPoint::from_unit(p.to_unit());
            assert!(
                (back.lat_deg - lat).abs() < 1e-9,
                "lat {} -> {}",
                lat,
                back.lat_deg
            );
            assert!(
                (back.lon_deg - lon).abs() < 1e-9,
                "lon {} -> {}",
                lon,
                back.lon_deg
            );
        }
    }

    #[test]
    fn arc_distance_quarter_circle() {
        let a = UnitVec3::from_lat_lon_deg(0.0, 0.0);
        let b = UnitVec3::from_lat_lon_deg(0.0, 90.0);
        let d = a.arc_distance_m(&b, EARTH_RADIUS_M);
        let expected = std::f64::consts::FRAC_PI_2 * EARTH_RADIUS_M;
        assert!((d - expected).abs() < 1.0, "{} vs {}", d, expected);
    }

    #[test]
    fn fibonacci_points_are_unit_length_and_distinct() {
        let pts = fibonacci_sphere(500);
        assert_eq!(pts.len(), 500);
        for p in &pts {
            assert!((p.as_vec().length() - 1.0).abs() < 1e-12);
        }
        // 相邻点不应该重合
        for i in 0..pts.len() - 1 {
            assert!(pts[i].angle_to(&pts[i + 1]) > 1e-6);
        }
    }

    #[test]
    fn offset_then_measure_returns_same_distance() {
        let start = UnitVec3::from_lat_lon_deg(20.0, 100.0);
        let angular = 0.05;
        let end = start.offset(1.234, angular);
        assert!((start.angle_to(&end) - angular).abs() < 1e-12);
    }
}
