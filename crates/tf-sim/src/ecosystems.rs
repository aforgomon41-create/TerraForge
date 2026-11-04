//! IUCN GET v2.1 hierarchy with explicit abstention and separate local habitats.
//! Environment-based estimates, not observations of species or ecological processes.
use crate::{ecology::Biome, WorldFields};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tf_grid::GridOps;

pub const VERSION: &str = "iucn-get-v2.1-tf1";
pub const CATALOGUE_JSON: &str = include_str!("../../../data/ecosystems/catalogue.json");
pub const CHANNEL_NAMES: [&str; 6] = [
    "ecosystem_biome",
    "ecosystem_group",
    "ecosystem_habitat_biome",
    "ecosystem_habitat_group",
    "ecosystem_variant",
    "ecosystem_evidence",
];
const GROUP_COUNTS: [u16; 25] = [
    7, 10, 5, 3, 10, 5, 7, 2, 3, 4, 2, 1, 2, 1, 2, 2, 3, 4, 6, 4, 5, 5, 5, 5, 7,
];
pub const fn group(biome: u8, child: u8) -> u16 {
    (biome as u16) * 16 + child as u16
}
pub fn valid_group(id: u16) -> bool {
    id == 0
        || ((1..=25).contains(&(id / 16))
            && id % 16 > 0
            && id % 16 <= GROUP_COUNTS[id as usize / 16 - 1])
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogueBiome {
    pub id: u8,
    pub code: String,
    pub realm: String,
    pub name_en: String,
    pub name_zh: String,
    pub color: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogueGroup {
    pub id: u16,
    pub code: String,
    pub biome: String,
    pub biome_id: u8,
    pub name_en: String,
    pub name_zh: String,
    pub support: String,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct Catalogue {
    pub version: String,
    pub source: serde_json::Value,
    pub biomes: Vec<CatalogueBiome>,
    pub groups: Vec<CatalogueGroup>,
}
pub fn catalogue() -> &'static Catalogue {
    static CATALOGUE: OnceLock<Catalogue> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        serde_json::from_str(CATALOGUE_JSON).expect("committed ecosystem catalogue")
    })
}

/// Group ids are biome*16 + child, hence MUST remain u16 (up to 407).
#[derive(Debug, Clone, Default)]
pub struct Ecosystems {
    pub biome: Vec<u8>,
    pub group: Vec<u16>,
    pub habitat_biome: Vec<u8>,
    pub habitat_group: Vec<u16>,
    /// moisture (bits 0..1), relief (2..3), soil context (4..5).
    pub variant: Vec<u8>,
    /// Inputs: monthly=1, terrain=2, water=4, ice=8, soil=16, coast=32.
    /// Bit8 marks an environmental estimate; bit9 marks a saved physical ice state.
    pub evidence: Vec<u16>,
}
impl Ecosystems {
    pub fn channels(&self) -> [Vec<f32>; 6] {
        [
            self.biome.iter().map(|&v| v as f32).collect(),
            self.group.iter().map(|&v| v as f32).collect(),
            self.habitat_biome.iter().map(|&v| v as f32).collect(),
            self.habitat_group.iter().map(|&v| v as f32).collect(),
            self.variant.iter().map(|&v| v as f32).collect(),
            self.evidence.iter().map(|&v| v as f32).collect(),
        ]
    }
    pub fn validate(&self, n: usize) -> bool {
        self.biome.len() == n
            && self.group.len() == n
            && self.habitat_biome.len() == n
            && self.habitat_group.len() == n
            && self.variant.len() == n
            && self.evidence.len() == n
            && (0..n).all(|i| {
                self.biome[i] <= 25
                    && self.habitat_biome[i] <= 25
                    && valid_group(self.group[i])
                    && valid_group(self.habitat_group[i])
                    && (self.group[i] == 0 || self.group[i] / 16 == self.biome[i] as u16)
                    && (self.habitat_group[i] == 0
                        || self.habitat_group[i] / 16 == self.habitat_biome[i] as u16)
                    && self.variant[i] <= 63
                    && self.evidence[i] & !831 == 0
            })
    }
}

struct Climate {
    cold: f32,
    warm: f32,
    mean: f32,
    rain: f32,
    aridity: f32,
    dry: usize,
    growing: usize,
    summer: f32,
    winter: f32,
    summer_pet: f32,
    wet_all: bool,
}
fn climate(t: &[f32; 12], p: &[f32; 12], pet: &[f32; 12]) -> Climate {
    let mut months = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
    months.sort_by(|&a, &b| t[b].total_cmp(&t[a]));
    let rain = p.iter().sum::<f32>();
    Climate {
        cold: t.iter().copied().fold(f32::INFINITY, f32::min),
        warm: t.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        mean: t.iter().sum::<f32>() / 12.,
        rain,
        aridity: rain / pet.iter().sum::<f32>().max(1.),
        dry: p.iter().zip(pet).filter(|(p, e)| **p < **e * 0.5).count(),
        growing: t.iter().filter(|&&v| v >= 5.).count(),
        summer: months[..6].iter().map(|&m| p[m]).sum(),
        winter: months[6..].iter().map(|&m| p[m]).sum(),
        summer_pet: months[..6].iter().map(|&m| pet[m]).sum(),
        wet_all: p.iter().zip(pet).all(|(p, e)| *p >= *e * 0.6),
    }
}

/// A connected, actually overflowing lake is a flushing freshwater proxy. A
/// closed lake remains F2/unknown; lack of salinity is never guessed as salt/fresh.
fn lakes<G: GridOps>(grid: &G, f: &WorldFields) -> (Vec<f64>, Vec<bool>) {
    let n = grid.cell_count();
    let mut area = vec![0.; n];
    let mut fresh = vec![false; n];
    let mut seen = vec![false; n];
    for start in 0..n {
        if seen[start] || f.elevation[start] <= 0. || f.lake_depth[start] < 0.1 {
            continue;
        }
        let mut cells = vec![start];
        seen[start] = true;
        let mut index = 0;
        let mut total = 0.;
        let mut overflow = false;
        while index < cells.len() {
            let i = cells[index];
            index += 1;
            total += grid.area_m2(i);
            let surface = f.elevation[i] + f.lake_depth[i];
            for &j in grid.neighbors(i) {
                let j = j as usize;
                if f.elevation[j] <= 0. {
                    overflow = true;
                    continue;
                }
                if f.lake_depth[j] < 0.1 {
                    overflow |= f.elevation[j] <= surface + 0.002;
                    continue;
                }
                if !seen[j] && (f.elevation[j] + f.lake_depth[j] - surface).abs() < 0.02 {
                    seen[j] = true;
                    cells.push(j);
                }
            }
        }
        for i in cells {
            area[i] = total;
            fresh[i] = overflow;
        }
    }
    (area, fresh)
}

fn terrestrial(
    c: &Climate,
    height: f32,
    latitude: f64,
    slope: f64,
    relief: f64,
    coast: f32,
) -> (u8, u16) {
    if c.warm < 10. {
        if slope <= 0.35 && (c.warm <= 0. || c.aridity < 0.2) {
            return (
                23,
                if latitude.abs() >= 45. && (height <= 700. || relief <= 100.) {
                    group(23, 3)
                } else {
                    0
                },
            );
        }
        let child = if slope > 0.35 {
            2
        } else if height > 700. && relief > 100. {
            if latitude.abs() < 23.5 {
                5
            } else {
                4
            }
        } else {
            3
        };
        return (23, group(23, child));
    }
    if c.aridity < 0.05 {
        return (22, group(22, 5));
    }
    if c.aridity < 0.2 {
        return (22, if c.mean < 10. { group(22, 4) } else { 0 });
    }
    if c.aridity < 0.5 {
        return (22, group(22, 1));
    }
    if c.cold >= 18. {
        if c.aridity < 0.65 {
            return (20, group(20, 1));
        }
        if c.rain >= 1500. && c.dry <= 1 {
            return (
                18,
                group(
                    18,
                    if height >= 800. && (c.warm < 25. || relief > 200.) {
                        3
                    } else {
                        1
                    },
                ),
            );
        }
        if c.aridity >= 0.8 && c.dry <= 6 {
            return (18, group(18, 2));
        }
        // Savanna drivers (herbivory/fire/grass traits) have not been simulated.
        return (21, 0);
    }
    if c.cold > 0. && c.summer < c.summer_pet * 0.5 && c.winter > c.summer * 1.8 {
        return (20, group(20, 2));
    }
    if c.aridity < 0.65 {
        return (21, group(21, 5));
    }
    if c.cold < -3. && c.growing < 7 {
        return (19, group(19, 1));
    }
    if c.wet_all && c.rain >= 1200. && c.warm - c.cold < 18. && coast < 300000. && c.mean < 12. {
        return (19, group(19, 3));
    }
    if c.wet_all && c.rain >= 1000. && c.cold > 5. && c.mean >= 12. {
        return (19, group(19, 4));
    }
    if c.cold < 5. {
        return (19, group(19, 2));
    }
    // Humid warm forests could depend on fire or additional leaf-trait evidence.
    (19, 0)
}

/// Classify after final lakes, soil and cryosphere; never rewrites physical fields.
pub fn classify<G: GridOps>(grid: &G, f: &WorldFields, legacy: &[u8], soil: &[u8]) -> Ecosystems {
    let n = grid.cell_count();
    let source = f.climate.as_ref().map(|a| a.grid.build());
    let (lake_area, fresh) = lakes(grid, f);
    let rows: Vec<[u16; 6]> = (0..n)
        .into_par_iter()
        .map(|i| {
            let h = f.elevation[i];
            let dir = grid.center_dir(i);
            let latitude = dir.to_lat_lon_deg().0;
            let mut slope = 0_f64;
            let mut relief = 0_f64;
            let mut near_sea = false;
            for &j in grid.neighbors(i) {
                let j = j as usize;
                let delta = (h - f.elevation[j]) as f64;
                slope = slope.max(delta.abs() / grid.distance_m(i, j).max(1.));
                relief = relief.max(delta.abs());
                near_sea |= f.elevation[j] <= 0.;
            }
            let monthly = if let (Some(a), Some(g)) = (&f.climate, &source) {
                Some(a.sample(g, dir, f.temperature[i], f.precipitation[i]))
            } else {
                None
            };
            let c = monthly
                .as_ref()
                .filter(|(t, p, pet)| {
                    t.iter().all(|v| v.is_finite())
                        && p.iter().chain(pet).all(|v| v.is_finite() && *v >= 0.)
                        && (t.iter().all(|&v| v <= 5.) || pet.iter().sum::<f32>() > 0.)
                })
                .map(|(t, p, pet)| climate(t, p, pet));
            let coast = f.distance_to_sea.get(i).copied().unwrap_or(f32::INFINITY);
            let mut evidence = 2_u16
                | if c.is_some() { 1 } else { 0 }
                | if f.cryosphere.is_some() { 8 } else { 0 }
                | if soil.get(i).is_some_and(|&s| s > 0) {
                    16
                } else {
                    0
                }
                | if near_sea { 32 } else { 0 };
            let (mut biome, mut efg) = if h <= 0. {
                // Stage 3 needs seabed morphology, substrate and ocean processes.
                // Depth alone cannot turn an entire deep basin into a hadal trench.
                (if h >= -200. { 5 } else { 7 }, 0)
            } else if let Some(c) = &c {
                terrestrial(c, h, latitude, slope, relief, coast)
            } else {
                (
                    match legacy[i] {
                        0 => 6,
                        1 | 2 | 11 => 23,
                        3 | 4 => 19,
                        5 | 8 => 21,
                        6 => 20,
                        7 => 22,
                        9 | 10 => 18,
                        12 => 25,
                        13 => 2,
                        _ => 0,
                    },
                    0,
                )
            };
            let (mut habitat, mut habitat_group) = (0, 0);
            if h <= 0. {
                habitat = 6;
                habitat_group = if f
                    .cryosphere
                    .as_ref()
                    .is_some_and(|ice| ice.sea_ice_cover[i] >= 0.15)
                {
                    group(6, 5)
                } else {
                    group(6, 1)
                };
            } else {
                let thick_ice = f
                    .cryosphere
                    .as_ref()
                    .is_some_and(|ice| ice.land_ice_m[i] >= 1. || ice.snow_cover[i] > 0.8);
                if thick_ice {
                    biome = 23;
                    efg = group(23, 1);
                    evidence |= 512;
                } else if f.lake_depth[i] >= 0.1 {
                    biome = if legacy[i] == Biome::Wetland as u8 {
                        25
                    } else {
                        2
                    };
                    efg = 0;
                    evidence |= 4;
                    if fresh[i] {
                        efg = if biome == 25 {
                            if c.as_ref().is_some_and(|c| c.wet_all) {
                                group(25, 3)
                            } else {
                                0
                            }
                        } else if c.as_ref().is_some_and(|c| c.cold < 0. && c.warm > 0.) {
                            group(2, 4)
                        } else if c.as_ref().is_some_and(|c| c.wet_all) {
                            group(2, if lake_area[i] >= 50e6 { 1 } else { 2 })
                        } else {
                            0
                        };
                    }
                } else if f.flow_accumulation[i] as f64 >= (grid.area_m2(i) * 40.).max(1e6) {
                    habitat = 1;
                    habitat_group = if c.as_ref().is_some_and(|c| c.cold < 0. && c.warm > 0.) {
                        group(1, 3)
                    } else {
                        0
                    };
                    evidence |= 4;
                }
                if !thick_ice && f.lake_depth[i] < 0.1 && near_sea && h < 5. {
                    habitat = 10;
                    habitat_group = 0;
                    evidence |= 32;
                }
            }
            if efg > 0 || habitat_group > 0 {
                evidence |= 256;
            }
            let moisture = if h <= 0. {
                3
            } else {
                c.as_ref().map_or(0, |c| {
                    if f.lake_depth[i] >= 0.1 {
                        3
                    } else if c.aridity < 0.5 {
                        0
                    } else if c.dry >= 3 {
                        1
                    } else {
                        2
                    }
                })
            };
            let terrain = if slope > 0.35 {
                3
            } else if relief > 200. || h > 1500. {
                2
            } else if slope > 0.02 {
                1
            } else {
                0
            };
            let substrate = match soil.get(i).copied().unwrap_or(0) {
                8 => 1,
                5 => 2,
                7 => 3,
                _ => 0,
            };
            [
                biome as u16,
                efg,
                habitat as u16,
                habitat_group,
                (moisture | terrain << 2 | substrate << 4) as u16,
                evidence,
            ]
        })
        .collect();
    let out = Ecosystems {
        biome: rows.iter().map(|r| r[0] as u8).collect(),
        group: rows.iter().map(|r| r[1]).collect(),
        habitat_biome: rows.iter().map(|r| r[2] as u8).collect(),
        habitat_group: rows.iter().map(|r| r[3]).collect(),
        variant: rows.iter().map(|r| r[4] as u8).collect(),
        evidence: rows.iter().map(|r| r[5]).collect(),
    };
    debug_assert!(out.validate(n));
    out
}

pub fn color(biome: u8, efg: u16, variant: u8, mode: u8) -> [u8; 3] {
    let b = if biome > 0 {
        catalogue().biomes.get(biome as usize - 1)
    } else {
        None
    };
    let hex = b.map_or("#90938b", |b| b.color.as_str());
    let mut rgb = [0.; 3];
    for i in 0..3 {
        rgb[i] = u8::from_str_radix(&hex[1 + i * 2..3 + i * 2], 16).unwrap_or(140) as f32;
    }
    if mode > 0 && efg > 0 {
        let shift = (efg % 16) as f32 - 3.;
        rgb[0] += shift * 2.5;
        rgb[1] += shift * 1.5;
        rgb[2] -= shift;
    }
    if mode == 2 {
        let wet = (variant & 3) as f32;
        let relief = ((variant >> 2) & 3) as f32;
        rgb[0] += (1.5 - wet) * 8. + relief * 2.;
        rgb[1] += (wet - 1.5) * 5.;
        rgb[2] += (wet - 1.5) * 4. + relief * 3.;
    }
    rgb.map(|c| c.clamp(0., 255.) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(t: [f32; 12], p: [f32; 12], pet: [f32; 12]) -> Climate {
        climate(&t, &p, &pet)
    }
    #[test]
    fn catalogue_hierarchy_ids_and_support_are_consistent() {
        let c = catalogue();
        assert_eq!(c.biomes.len(), 25);
        assert_eq!(c.groups.len(), 110);
        assert_eq!(c.version, VERSION);
        let mut ids = std::collections::HashSet::new();
        for g in &c.groups {
            assert!(ids.insert(g.id));
            assert!(valid_group(g.id));
            assert_eq!(g.id / 16, g.biome_id as u16);
            assert_eq!(g.biome, c.biomes[g.biome_id as usize - 1].code);
        }
        assert!(!valid_group(255));
        assert!(!valid_group(408));
        assert_eq!(group(23, 1), 369);
        assert!(group(25, 3) > 255);
    }
    #[test]
    fn seasonality_and_elevation_distinguish_forests_without_inventing_fire() {
        let wet = c([24.; 12], [180.; 12], [100.; 12]);
        assert_eq!(terrestrial(&wet, 100., 0., 0., 0., 1e5).1, group(18, 1));
        assert_eq!(terrestrial(&wet, 1200., 0., 0., 250., 1e5).1, group(18, 3));
        let warm = c([16.; 12], [200.; 12], [90.; 12]);
        assert_eq!(terrestrial(&warm, 100., 30., 0., 0., 1e5).1, group(19, 4));
        let uncertain = c([16.; 12], [80.; 12], [90.; 12]);
        assert_eq!(terrestrial(&uncertain, 100., 30., 0., 0., 1e5), (19, 0));
        let cold = c([5.; 12], [40.; 12], [30.; 12]);
        assert_eq!(terrestrial(&cold, 200., 70., 0., 0., 1e5).1, group(23, 3));
        assert_eq!(
            terrestrial(&cold, 2000., 10., 0.05, 200., 1e5).1,
            group(23, 5)
        );
    }
    #[test]
    fn hydrologic_habitats_do_not_replace_landcover_and_no_monthly_data_abstains() {
        let grid = tf_grid::GridDescriptor::plane(0., 0., 20000., 20, 20, 6371000.).build();
        let n = grid.cell_count();
        let mut f = WorldFields::with_capacity(n);
        f.elevation.fill(100.);
        f.temperature.fill(20.);
        f.precipitation.fill(900.);
        f.flow_accumulation.fill(1e13);
        let e = classify(&grid, &f, &vec![4; n], &vec![1; n]);
        assert!(e.validate(n));
        assert!(e.biome.iter().all(|&b| b == 19));
        assert!(e.group.iter().all(|&g| g == 0));
        assert!(e.habitat_biome.iter().all(|&b| b == 1));
        f.lake_depth.fill(20.);
        let e = classify(&grid, &f, &vec![13; n], &vec![0; n]);
        assert!(e.biome.iter().all(|&b| b == 2));
        assert!(e.group.iter().all(|&g| g == 0));
    }

    #[test]
    fn dry_alpine_terrain_and_deep_basins_do_not_invent_grasslands_or_trenches() {
        let dry = c([5.; 12], [0.; 12], [30.; 12]);
        assert_eq!(terrestrial(&dry, 3000., 10., 0.05, 300., 1e5), (23, 0));
        assert_eq!(terrestrial(&dry, 3000., 70., 0.05, 300., 1e5), (23, 0));
        assert_eq!(
            terrestrial(&dry, 200., 70., 0., 0., 1e5),
            (23, group(23, 3))
        );
        let grid = tf_grid::GridDescriptor::plane(0., 0., 20000., 20, 20, 6371000.).build();
        let n = grid.cell_count();
        let mut f = WorldFields::with_capacity(n);
        f.elevation.fill(-7000.);
        let e = classify(&grid, &f, &vec![0; n], &vec![0; n]);
        assert!(e.biome.iter().all(|&b| b == 7));
        assert!(e.group.iter().all(|&g| g == 0));
    }
}
