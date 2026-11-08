//! Mover light state: each frame, one (mover id, ramped travel fraction) per
//! present mover, read from the registry's final post-tick phase, for the
//! renderer's mover state lighting. Every present mover is collected, culled
//! from the frame's view or not, with or without draw geometry.
//! See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use std::collections::HashMap;

use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry};
use postretro_level_loader::{LevelWorld, LoadedKinematicMover, MoverLightRamp};
use postretro_physics::kinematic_mover::{MoverPathArcLengths, mover_path_fraction};

use super::MoverListSource;
use crate::render::MoverLightState;

/// The mover's light state from its travel fraction `t` under its author
/// ramp: `clamp((t - start) / (end - start), 0, 1)`. A ramp load validation
/// would have refused (an empty or non-finite window) reads as the identity.
pub(crate) fn ramped_travel_fraction(t: f32, ramp: MoverLightRamp) -> f32 {
    let span = ramp.end - ramp.start;
    let ramped = if span.is_finite() && span > 0.0 && ramp.start.is_finite() {
        (t - ramp.start) / span
    } else {
        t
    };
    if ramped.is_finite() {
        ramped.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Per-mover data that holds for a whole level: its ramp and its path's arc
/// lengths, built on first sight so a frame allocates nothing.
#[derive(Debug, Default)]
struct MoverLightPath {
    ramp: MoverLightRamp,
    arc_lengths: MoverPathArcLengths,
}

/// Collects one frame's mover light states into a reused buffer.
#[derive(Debug, Default)]
pub(crate) struct MoverLightStateCollector {
    states: Vec<MoverLightState>,
    paths: HashMap<u32, MoverLightPath>,
    source: Option<MoverListSource>,
}

impl MoverLightStateCollector {
    /// Forget the level: its ramps, its paths and the last frame's states.
    pub(crate) fn clear(&mut self) {
        self.states.clear();
        self.paths.clear();
        self.source = None;
    }

    /// This frame's states, one per present mover in `registry`, from the
    /// phase the last tick (or snapshot) left. No level, no states.
    pub(crate) fn collect(&mut self, registry: &EntityRegistry, world: Option<&LevelWorld>) {
        self.states.clear();
        let Some(world) = world else {
            return;
        };
        self.refresh_ramps(&world.kinematic_geometry.movers);
        for (_, value) in registry.iter_with_kind(ComponentKind::KinematicMover) {
            let ComponentValue::KinematicMover(mover) = value else {
                continue;
            };
            let path = self.paths.entry(mover.mover_id).or_default();
            path.arc_lengths.sync_to(&mover.waypoints);
            let t = mover_path_fraction(mover, &path.arc_lengths);
            self.states.push(MoverLightState {
                mover_id: mover.mover_id,
                t: ramped_travel_fraction(t, path.ramp),
            });
        }
    }

    pub(crate) fn states(&self) -> &[MoverLightState] {
        &self.states
    }

    /// Re-key the cached ramps when the level's mover list changes.
    fn refresh_ramps(&mut self, movers: &[LoadedKinematicMover]) {
        let source = MoverListSource::from_movers(movers);
        if self.source == Some(source) {
            return;
        }
        self.paths.clear();
        self.paths.reserve(movers.len());
        for mover in movers {
            self.paths.insert(
                mover.mover_id,
                MoverLightPath {
                    ramp: mover.light_ramp,
                    arc_lengths: MoverPathArcLengths::default(),
                },
            );
        }
        self.source = Some(source);
    }
}
