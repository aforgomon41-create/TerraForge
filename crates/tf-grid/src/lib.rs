//! 六角格网格系统。
//!
//! 提供两套网格：
//! - sphere_hex：球面戈德堡多面体，用于世界层
//! - plane_hex：平面轴向坐标网格，用于大陆/区域/地点三层
//!
//! 两者都实现 GridOps trait，物理引擎只依赖这个 trait。

pub mod grid_ops;
pub mod patch_grid;
pub mod plane_hex;
pub mod snyder;
pub mod sphere_hex;

pub use grid_ops::GridOps;
pub use patch_grid::{PatchGrid, SphericalPatch};
pub use plane_hex::{AxialCoord, HexGrid2D};
pub use sphere_hex::{GridStats, SphereHexGrid};

pub mod descriptor;
pub use descriptor::{GridDescriptor, MapGrid, GRID_VERSION};

pub mod flat_grid;
pub use flat_grid::FlatGrid;
pub mod mesh;
