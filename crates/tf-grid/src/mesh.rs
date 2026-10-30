//! Shared primal triangles and their barycentric dual; never independent regular hexagons.
use crate::GridOps;
use glam::DVec3;

pub fn triangles<G: GridOps>(grid: &G) -> Vec<u32> {
    let mut out = Vec::with_capacity(grid.cell_count() * 6);
    for i in 0..grid.cell_count() {
        let neighbors = grid.neighbors(i);
        for &j in neighbors {
            if j as usize <= i {
                continue;
            }
            for &k in neighbors {
                if k <= j || !grid.neighbors(j as usize).contains(&k) {
                    continue;
                }
                let position = |a: usize| {
                    grid.local_center_m(a).map_or_else(
                        || grid.center_dir(a).as_vec(),
                        |(x, y)| DVec3::new(x, y, 0.),
                    )
                };
                let p = position(i);
                let q = position(j as usize);
                let r = position(k as usize);
                let normal = (q - p).cross(r - p);
                let positive = if grid.extent_m().is_some() {
                    normal.z > 0.
                } else {
                    normal.dot(p) > 0.
                };
                let face = if positive {
                    [i as u32, j, k]
                } else {
                    [i as u32, k, j]
                };
                out.extend_from_slice(&face);
            }
        }
    }
    out
}

/// One line per neighboring cell pair, with endpoints shared by three cells.
pub fn dual_edges<G: GridOps>(grid: &G) -> Vec<[DVec3; 2]> {
    let position = |a: usize| {
        grid.local_center_m(a).map_or_else(
            || grid.center_dir(a).as_vec(),
            |(x, y)| DVec3::new(x, y, 0.),
        )
    };
    let mut lines = Vec::new();
    for i in 0..grid.cell_count() {
        for &j in grid.neighbors(i) {
            if j as usize <= i {
                continue;
            }
            let common: Vec<_> = grid
                .neighbors(i)
                .iter()
                .copied()
                .filter(|k| grid.neighbors(j as usize).contains(k))
                .collect();
            if common.is_empty() || common.len() > 2 {
                continue;
            }
            let corner = |k: u32| {
                let mut ids = [i as u32, j, k];
                ids.sort_unstable();
                let p = (position(ids[0] as usize)
                    + position(ids[1] as usize)
                    + position(ids[2] as usize))
                    / 3.;
                if grid.extent_m().is_none() {
                    p.normalize()
                } else {
                    p
                }
            };
            if common.len() == 2 {
                lines.push([corner(common[0]), corner(common[1])]);
            } else {
                // Clip the outer dual cells to the triangulated domain hull.
                let a = position(i);
                let b = position(j as usize);
                let midpoint = (a + b) * 0.5;
                lines.push([corner(common[0]), midpoint]);
                lines.push([a, midpoint]);
                lines.push([midpoint, b]);
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    #[test]
    fn finite_dual_has_no_dangling_edges() {
        let g = crate::FlatGrid::new(24, 12, 24000., 30., 0., 6371000.);
        let mut degree = HashMap::new();
        for line in dual_edges(&g) {
            for p in line {
                let key = ((p.x * 1000.).round() as i64, (p.y * 1000.).round() as i64);
                *degree.entry(key).or_insert(0) += 1;
            }
        }
        assert!(degree.values().all(|&n| n >= 2));
    }
    #[test]
    fn sphere_is_closed_with_twelve_five_sided_dual_cells() {
        let g = crate::SphereHexGrid::build(8, 6371000., (DVec3::Z, 0.));
        let t = triangles(&g);
        assert_eq!(t.len() / 3, 20 * 8 * 8);
        let mut edges = HashMap::new();
        for face in t.chunks_exact(3) {
            for [a, b] in [[face[0], face[1]], [face[1], face[2]], [face[2], face[0]]] {
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        assert!(edges.values().all(|&count| count == 2));
        assert_eq!(
            g.cell_count() as i64 - edges.len() as i64 + (t.len() / 3) as i64,
            2
        );
        assert_eq!(dual_edges(&g).len(), edges.len());
        assert_eq!(
            (0..g.cell_count())
                .filter(|&i| g.neighbors(i).len() == 5)
                .count(),
            12
        );
    }
}
