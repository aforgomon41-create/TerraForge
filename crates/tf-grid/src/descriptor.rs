//! Versioned geometry shared by storage, sampling, rendering and export.
use crate::{GridOps, PatchGrid, SphereHexGrid, SphericalPatch};
use serde::{Deserialize, Serialize};
use tf_core::geo::UnitVec3;

pub const GRID_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GridDescriptor {
    Sphere {
        frequency: u32,
        radius_m: f64,
        version: u32,
    },
    Patch {
        center_lat: f64,
        center_lon: f64,
        width_m: f64,
        size: u32,
        radius_m: f64,
        version: u32,
    },
}

impl GridDescriptor {
    pub fn sphere(frequency: u32, radius_m: f64) -> Self {
        Self::Sphere {
            frequency,
            radius_m,
            version: GRID_VERSION,
        }
    }
    pub fn patch(lat: f64, lon: f64, width: f64, size: u32, radius: f64) -> Self {
        Self::Patch {
            center_lat: lat,
            center_lon: lon,
            width_m: width,
            size,
            radius_m: radius,
            version: GRID_VERSION,
        }
    }
    pub fn cell_count(&self) -> usize {
        match self {
            Self::Sphere { frequency, .. } => 10 * (*frequency as usize).pow(2) + 2,
            Self::Patch { size, .. } => (*size as usize).pow(2),
        }
    }
    pub fn radius_m(&self) -> f64 {
        match self {
            Self::Sphere { radius_m, .. } | Self::Patch { radius_m, .. } => *radius_m,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if !self.radius_m().is_finite() || !(1e6..=5e7).contains(&self.radius_m()) {
            return Err("星球半径无效".into());
        }
        match self {
            Self::Sphere {
                frequency, version, ..
            } => {
                if *version != GRID_VERSION || ![256, 512, 768].contains(frequency) {
                    return Err("不支持的球面网格版本或频率".into());
                }
            }
            Self::Patch {
                center_lat,
                center_lon,
                width_m,
                size,
                radius_m,
                version,
            } => {
                if *version != GRID_VERSION
                    || !center_lat.is_finite()
                    || !(-90.0..=90.0).contains(center_lat)
                    || !center_lon.is_finite()
                    || !(-180.0..=180.0).contains(center_lon)
                    || !width_m.is_finite()
                    || *width_m <= 0.
                    || *width_m > *radius_m * 0.8
                    || !(4..=4096).contains(size)
                {
                    return Err("局部网格的坐标、尺寸或版本无效".into());
                }
            }
        }
        Ok(())
    }
    pub fn build(&self) -> MapGrid {
        match *self {
            Self::Sphere {
                frequency,
                radius_m,
                ..
            } => MapGrid::Sphere(SphereHexGrid::build(
                frequency,
                radius_m,
                (glam::DVec3::Z, 0.),
            )),
            Self::Patch {
                center_lat,
                center_lon,
                width_m,
                size,
                radius_m,
                ..
            } => MapGrid::Patch(PatchGrid::new(&SphericalPatch::with_radius(
                center_lat, center_lon, width_m, size, radius_m,
            ))),
        }
    }
}

pub enum MapGrid {
    Sphere(SphereHexGrid),
    Patch(PatchGrid),
}
impl MapGrid {
    pub fn cell_at(&self, dir: UnitVec3) -> Option<usize> {
        match self {
            Self::Sphere(g) => Some(g.cell_at(dir)),
            Self::Patch(g) => g.cell_at(dir),
        }
    }
    pub fn render_center(&self, i: usize) -> [f32; 3] {
        match self {
            Self::Sphere(g) => {
                let p = g.center(i).as_vec();
                [p.x as f32, p.y as f32, p.z as f32]
            }
            Self::Patch(g) => {
                let (x, y) = g.local_center(i);
                [x as f32, y as f32, 0.]
            }
        }
    }
}
impl GridOps for MapGrid {
    fn radius_m(&self) -> f64 {
        match self {
            Self::Sphere(g) => g.radius_m(),
            Self::Patch(g) => g.radius_m(),
        }
    }
    fn cell_count(&self) -> usize {
        match self {
            Self::Sphere(g) => g.cell_count(),
            Self::Patch(g) => g.cell_count(),
        }
    }
    fn neighbors(&self, i: usize) -> &[u32] {
        match self {
            Self::Sphere(g) => g.neighbors(i),
            Self::Patch(g) => g.neighbors(i),
        }
    }
    fn distance_m(&self, a: usize, b: usize) -> f64 {
        match self {
            Self::Sphere(g) => g.distance_m(a, b),
            Self::Patch(g) => g.distance_m(a, b),
        }
    }
    fn area_m2(&self, i: usize) -> f64 {
        match self {
            Self::Sphere(g) => g.area_m2(i),
            Self::Patch(g) => g.area_m2(i),
        }
    }
    fn center_dir(&self, i: usize) -> UnitVec3 {
        match self {
            Self::Sphere(g) => g.center_dir(i),
            Self::Patch(g) => g.center_dir(i),
        }
    }
    fn latitude_deg(&self, i: usize) -> f64 {
        match self {
            Self::Sphere(g) => g.latitude_deg(i),
            Self::Patch(g) => g.latitude_deg(i),
        }
    }
    fn boundary_mask(&self) -> &bitvec::prelude::BitVec {
        match self {
            Self::Sphere(g) => g.boundary_mask(),
            Self::Patch(g) => g.boundary_mask(),
        }
    }
}
