//! A genuine finite Euclidean domain. Geographic coordinates only locate climate bands.
use crate::GridOps;
use bitvec::prelude::BitVec;
use glam::DVec3;
use tf_core::geo::UnitVec3;

pub struct FlatGrid {
    pub cols: u32,
    pub rows: u32,
    pub spacing: f64,
    pub lat: f64,
    pub lon: f64,
    pub radius: f64,
    pub reference_lat: f64,
    centers: Vec<UnitVec3>,
    offsets: Vec<usize>,
    adjacent: Vec<u32>,
    boundary: BitVec,
}
impl FlatGrid {
    pub fn new(cols: u32, rows: u32, width: f64, lat: f64, lon: f64, radius: f64) -> Self {
        Self::with_reference_lat(cols, rows, width, lat, lon, radius, lat)
    }
    pub fn with_reference_lat(
        cols: u32,
        rows: u32,
        width: f64,
        lat: f64,
        lon: f64,
        radius: f64,
        reference_lat: f64,
    ) -> Self {
        assert!(cols >= 4 && rows >= 4 && width > 0. && radius > 0.);
        let half_lat =
            (rows as f64 * width / cols as f64 * 3_f64.sqrt() / 2. / (2. * radius)).to_degrees();
        assert!(
            width.is_finite()
                && radius.is_finite()
                && reference_lat.abs() <= 85.
                && lat.is_finite()
                && lon.is_finite()
                && reference_lat.is_finite()
                && lat.abs() + half_lat <= 89.9,
            "flat domain crosses a geographic pole"
        );
        let mut grid = Self {
            cols,
            rows,
            spacing: width / cols as f64,
            lat,
            lon,
            radius,
            reference_lat,
            centers: Vec::new(),
            offsets: vec![0],
            adjacent: Vec::new(),
            boundary: BitVec::new(),
        };
        for r in 0..rows as i32 {
            for c in 0..cols as i32 {
                let i = r as usize * cols as usize + c as usize;
                let (x, y) = grid.xy(i);
                grid.centers.push(grid.direction_at(x, y));
                let shifts = if r % 2 == 0 {
                    [(0, -1), (0, 1), (-1, -1), (-1, 0), (1, -1), (1, 0)]
                } else {
                    [(0, -1), (0, 1), (-1, 0), (-1, 1), (1, 0), (1, 1)]
                };
                let before = grid.adjacent.len();
                for (dr, dc) in shifts {
                    let (rr, cc) = (r + dr, c + dc);
                    if (0..rows as i32).contains(&rr) && (0..cols as i32).contains(&cc) {
                        grid.adjacent.push(rr as u32 * cols + cc as u32);
                    }
                }
                grid.boundary.push(grid.adjacent.len() - before < 6);
                grid.offsets.push(grid.adjacent.len());
            }
        }
        grid
    }
    pub fn xy(&self, i: usize) -> (f64, f64) {
        let (r, c) = (i / self.cols as usize, i % self.cols as usize);
        (
            ((c as f64 + (r % 2) as f64 * 0.5) - (self.cols as f64 - 1.) / 2. - 0.25)
                * self.spacing,
            ((self.rows as f64 - 1.) / 2. - r as f64) * self.spacing * 3_f64.sqrt() / 2.,
        )
    }
    pub fn width(&self) -> f64 {
        self.cols as f64 * self.spacing
    }
    pub fn height(&self) -> f64 {
        self.rows as f64 * self.spacing * 3_f64.sqrt() / 2.
    }
    pub fn direction_at(&self, x: f64, y: f64) -> UnitVec3 {
        UnitVec3::from_lat_lon_deg(
            self.lat + (y / self.radius).to_degrees(),
            self.lon
                + (x / (self.radius * self.reference_lat.to_radians().cos().max(0.05)))
                    .to_degrees(),
        )
    }
    pub fn cell_at_xy(&self, x: f64, y: f64) -> Option<usize> {
        if x.abs() > self.width() / 2. || y.abs() > self.height() / 2. {
            return None;
        }
        let row =
            ((self.rows as f64 - 1.) / 2. - y / (self.spacing * 3_f64.sqrt() / 2.)).round() as i32;
        let mut best = None;
        let mut distance = f64::INFINITY;
        for r in row - 1..=row + 1 {
            if !(0..self.rows as i32).contains(&r) {
                continue;
            }
            let col = (x / self.spacing + (self.cols as f64 - 1.) / 2. + 0.25
                - (r % 2) as f64 * 0.5)
                .round() as i32;
            for c in col - 1..=col + 1 {
                if !(0..self.cols as i32).contains(&c) {
                    continue;
                }
                let i = r as usize * self.cols as usize + c as usize;
                let (a, b) = self.xy(i);
                let d = (x - a).powi(2) + (y - b).powi(2);
                if d < distance {
                    best = Some(i);
                    distance = d
                }
            }
        }
        best
    }
    pub fn project(&self, dir: UnitVec3) -> (f64, f64) {
        let (lat, lon) = dir.to_lat_lon_deg();
        let dl = (lon - self.lon + 180.).rem_euclid(360.) - 180.;
        (
            dl.to_radians() * self.radius * self.reference_lat.to_radians().cos().max(0.05),
            (lat - self.lat).to_radians() * self.radius,
        )
    }
    pub fn cell_at(&self, dir: UnitVec3) -> Option<usize> {
        let (x, y) = self.project(dir);
        self.cell_at_xy(x, y)
    }
}
impl GridOps for FlatGrid {
    fn locate_local(&self, x: f64, y: f64) -> Option<usize> {
        self.cell_at_xy(x, y)
    }
    fn field_is_planar(&self) -> bool {
        true
    }
    fn locate(&self, dir: UnitVec3) -> Option<usize> {
        self.cell_at(dir)
    }
    fn radius_m(&self) -> f64 {
        self.radius
    }
    fn cell_count(&self) -> usize {
        self.cols as usize * self.rows as usize
    }
    fn neighbors(&self, i: usize) -> &[u32] {
        &self.adjacent[self.offsets[i]..self.offsets[i + 1]]
    }
    fn distance_m(&self, a: usize, b: usize) -> f64 {
        let (x, y) = self.xy(a);
        let (u, v) = self.xy(b);
        (x - u).hypot(y - v)
    }
    fn area_m2(&self, _i: usize) -> f64 {
        self.spacing * self.spacing * 3_f64.sqrt() / 2.
    }
    fn center_dir(&self, i: usize) -> UnitVec3 {
        self.centers[i]
    }
    fn latitude_deg(&self, i: usize) -> f64 {
        self.lat + (self.xy(i).1 / self.radius).to_degrees()
    }
    fn boundary_mask(&self) -> &BitVec {
        &self.boundary
    }
    fn field_position(&self, i: usize) -> DVec3 {
        let (x, y) = self.xy(i);
        DVec3::new(x, y, 0.)
    }
    fn extent_m(&self) -> Option<(f64, f64)> {
        Some((self.width(), self.height()))
    }
    fn local_center_m(&self, i: usize) -> Option<(f64, f64)> {
        Some(self.xy(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[should_panic(expected = "flat domain crosses a geographic pole")]
    fn direct_construction_rejects_a_folded_polar_domain() {
        FlatGrid::new(96, 69, 8_000_000., 85., 0., 6371000.);
    }
    #[test]
    fn flat_metric_spacing_and_coordinates_are_invertible() {
        let g = FlatGrid::new(24, 12, 24000., 30., 179.99, 6371000.);
        for i in 0..g.cell_count() {
            assert_eq!(g.cell_at(g.center_dir(i)), Some(i));
            for &j in g.neighbors(i) {
                assert!((g.distance_m(i, j as usize) - 1000.).abs() < 1e-8)
            }
        }
        assert!(g.cell_at_xy(13000., 0.).is_none());
        assert!((g.height() - 12000. * 3_f64.sqrt() / 2.).abs() < 1e-6);
    }
}
