//! Read-only observation of solver states. Observers never steer the simulation.
use crate::{Stage, WorldFields};
use tf_grid::GridOps;

pub const HEIGHT: u8 = 1;
pub const TEMPERATURE: u8 = 2;
pub const PRECIPITATION: u8 = 4;
pub const FLOW: u8 = 8;
pub const ALL: u8 = HEIGHT | TEMPERATURE | PRECIPITATION | FLOW;

#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    pub stage: Stage,
    pub progress: f64,
    pub step: u32,
    pub elapsed_myr: Option<f64>,
    pub year: Option<u32>,
    pub month: Option<u8>,
    pub detail: &'static str,
    pub channels: u8,
    pub stage_complete: bool,
}
impl FrameInfo {
    pub fn stage(stage: Stage, progress: f64) -> Self {
        Self {
            stage,
            progress,
            step: 0,
            elapsed_myr: None,
            year: None,
            month: None,
            detail: "阶段完成",
            channels: match stage {
                Stage::Tectonics | Stage::Landforms => HEIGHT,
                Stage::Erosion => HEIGHT | FLOW,
                _ => ALL,
            },
            stage_complete: true,
        }
    }
}

pub trait FrameObserver {
    fn enabled(&self) -> bool {
        true
    }
    /// Visual sampling budget, independent of the physical solver mesh.
    fn preview_cell_budget(&self) -> usize {
        usize::MAX
    }
    /// Called before allocating or computing an expensive preview.
    fn wants_frame(&mut self, info: FrameInfo) -> bool;
    fn frame(&mut self, grid: &dyn GridOps, fields: &WorldFields, info: FrameInfo);
}
pub struct NoFrames;
impl FrameObserver for NoFrames {
    fn enabled(&self) -> bool {
        false
    }
    fn wants_frame(&mut self, _: FrameInfo) -> bool {
        false
    }
    fn frame(&mut self, _: &dyn GridOps, _: &WorldFields, _: FrameInfo) {}
}
pub fn emit(
    observer: &mut dyn FrameObserver,
    grid: &dyn GridOps,
    fields: &WorldFields,
    info: FrameInfo,
) {
    if observer.wants_frame(info) {
        observer.frame(grid, fields, info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use tf_core::params::{GenerationParams, LevelKind, SimQuality};
    use tf_grid::{FlatGrid, SphereHexGrid};

    #[derive(Default)]
    struct Observer {
        times: Vec<f64>,
        heights: Vec<Vec<f32>>,
        cells: Vec<usize>,
        info: Vec<FrameInfo>,
        rain_seen: bool,
    }
    impl FrameObserver for Observer {
        fn preview_cell_budget(&self) -> usize {
            6000
        }
        fn wants_frame(&mut self, info: FrameInfo) -> bool {
            info.stage_complete
                || (info.elapsed_myr.is_some() && info.step % 4 == 0)
                || (info.stage == Stage::Erosion && info.step % 4 == 0)
                || info.month.is_some_and(|month| month % 6 == 0)
        }
        fn frame(&mut self, _: &dyn GridOps, fields: &WorldFields, info: FrameInfo) {
            self.cells.push(fields.len());
            self.info.push(info);
            if let Some(time) = info.elapsed_myr {
                self.times.push(time);
                self.heights.push(fields.elevation.clone());
            }
            if info.month.is_some() && info.channels & PRECIPITATION != 0 {
                self.rain_seen |= fields.precipitation.iter().any(|p| *p > 0.);
            }
        }
    }
    #[test]
    fn bounded_global_previews_preserve_the_final_solver_state_and_history() {
        let grid = SphereHexGrid::build(32, 6371000., (DVec3::Z, 0.));
        let params = GenerationParams {
            tectonic_age_myr: 1.,
            ..Default::default()
        };
        let mut plain = WorldFields::with_capacity(grid.cell_count());
        let mut live = plain.clone();
        let a =
            crate::evolution::generate(&grid, &params, &mut plain, &mut |_| {}, &|| false).unwrap();
        let mut observer = Observer::default();
        let b = crate::evolution::generate_observed(
            &grid,
            &params,
            &mut live,
            &mut |_| {},
            &|| false,
            &mut observer,
        )
        .unwrap();
        assert!(!observer.cells.is_empty());
        assert!(observer.cells.iter().all(|&n| n <= 6000));
        assert!(observer.heights.iter().flatten().all(|h| h.is_finite()));
        assert_eq!(plain.elevation, live.elevation);
        assert_eq!(plain.crust_thickness, live.crust_thickness);
        assert_eq!(plain.plate_id, live.plate_id);
        assert_eq!(
            serde_json::to_value(a.history).unwrap(),
            serde_json::to_value(b.history).unwrap()
        );
    }
    #[test]
    fn real_tectonic_states_change_with_time_without_changing_the_final_rock() {
        let grid = SphereHexGrid::build(8, 6371000., (DVec3::Z, 0.));
        let params = GenerationParams {
            tectonic_age_myr: 8.,
            ..Default::default()
        };
        let mut plain = WorldFields::with_capacity(grid.cell_count());
        let mut live = plain.clone();
        let a =
            crate::evolution::generate(&grid, &params, &mut plain, &mut |_| {}, &|| false).unwrap();
        let mut observer = Observer::default();
        let b = crate::evolution::generate_observed(
            &grid,
            &params,
            &mut live,
            &mut |_| {},
            &|| false,
            &mut observer,
        )
        .unwrap();
        assert!(observer.times.len() >= 3);
        assert_eq!(observer.times[0], 0.);
        assert!(observer.times.windows(2).all(|pair| pair[1] > pair[0]));
        assert!(observer.heights.windows(2).any(|pair| pair[0] != pair[1]));
        assert_eq!(plain.elevation, live.elevation);
        assert_eq!(plain.crust_thickness, live.crust_thickness);
        assert_eq!(plain.crust_type, live.crust_type);
        assert_eq!(plain.plate_id, live.plate_id);
        assert_eq!(
            serde_json::to_value(a.history).unwrap(),
            serde_json::to_value(b.history).unwrap()
        );
    }
    #[test]
    fn live_erosion_and_climate_keep_final_fields_and_progress_consistent() {
        let params = GenerationParams {
            quality: SimQuality::Economy,
            ..GenerationParams::for_level(LevelKind::Region)
        };
        let grid = FlatGrid::new(
            16,
            12,
            params.map_width_m,
            35.,
            179.8,
            params.planet.radius_m,
        );
        let plain = crate::generate(&grid, &params, &mut |_, _| {});
        let mut observer = Observer::default();
        let mut progress = Vec::new();
        let live = crate::generate_observed(
            &grid,
            &params,
            &mut |_, p| progress.push(p),
            &mut observer,
            &|| false,
        )
        .unwrap();
        assert_eq!(plain.fields.elevation, live.fields.elevation);
        assert_eq!(plain.fields.temperature, live.fields.temperature);
        assert_eq!(plain.fields.precipitation, live.fields.precipitation);
        assert_eq!(
            plain.fields.flow_accumulation,
            live.fields.flow_accumulation
        );
        assert_eq!(
            serde_json::to_value(plain.fields.climate).unwrap(),
            serde_json::to_value(live.fields.climate).unwrap()
        );
        assert!(progress.windows(2).all(|pair| pair[1] + 1e-12 >= pair[0]));
        assert!(
            observer
                .info
                .iter()
                .filter(|i| i.stage == Stage::Erosion && !i.stage_complete)
                .count()
                >= 2
        );
        assert!(observer.info.iter().filter(|i| i.month.is_some()).count() > 4);
        assert!(observer.rain_seen);
    }
    #[test]
    fn cancellation_during_observation_stops_the_solver_before_publication() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let cancelled = AtomicBool::new(false);
        struct Cancel<'a>(&'a AtomicBool, usize);
        impl FrameObserver for Cancel<'_> {
            fn wants_frame(&mut self, _: FrameInfo) -> bool {
                true
            }
            fn frame(&mut self, _: &dyn GridOps, _: &WorldFields, _: FrameInfo) {
                self.1 += 1;
                if self.1 == 3 {
                    self.0.store(true, Ordering::Relaxed);
                }
            }
        }
        let grid = SphereHexGrid::build(8, 6371000., (DVec3::Z, 0.));
        let mut fields = WorldFields::with_capacity(grid.cell_count());
        let mut observer = Cancel(&cancelled, 0);
        let result = crate::evolution::generate_observed(
            &grid,
            &GenerationParams::default(),
            &mut fields,
            &mut |_| {},
            &|| cancelled.load(Ordering::Relaxed),
            &mut observer,
        );
        assert!(result.is_none());
        assert_eq!(observer.1, 3);
    }
}
