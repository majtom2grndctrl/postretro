// Dock state: which waypoint a mover rests at, read from its phase every tick.
// Navigation gates and links switch on this; collision never does.
// See: context/lib/movement.md §6 · context/lib/build_pipeline.md §Navigation bake

use postretro_entities::{KinematicMoverComponent, KinematicMoverMode};

/// The waypoint `mover` rests at this tick, if any. Resting means it began and
/// ended the tick there: a timed wait, a finished run, a `goToPathNode` hold,
/// never started, or held there by a pin after a command to leave. Touching a
/// waypoint in passing moves it that tick, so it is no dock. Camera occlusion
/// keeps its stricter `mover_is_docked_closed`.
pub fn mover_docked_waypoint(mover: &KinematicMoverComponent) -> Option<u32> {
    (mover.segment_elapsed_ms <= f32::EPSILON
        && mover.current_linear_velocity.length_squared() == 0.0)
        .then_some(u32::from(mover.segment_index))
}

/// Whether `mover`'s own motion will take it from dock to dock. A mover that
/// moves only on a trigger, or that a script or auto-close has parked, will not:
/// parking clears `started` or sets `completed`, and a `goToPathNode` hold sets
/// `target_segment`, so phase alone answers.
pub fn mover_moves_on_its_own(mover: &KinematicMoverComponent) -> bool {
    mover.mode == KinematicMoverMode::PingPong
        && mover.started
        && !mover.completed
        && mover.target_segment.is_none()
}

/// One watched mover's dock state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MoverDock {
    mover_id: u32,
    waypoint: Option<u32>,
    moves_on_its_own: bool,
}

/// Live dock state of the movers navigation records name. Host-only. A mover
/// nothing watches costs nothing and never changes the generation, so a sweeper
/// or a door without records wakes no agent.
#[derive(Debug, Clone, Default)]
pub struct MoverDockTable {
    /// Sorted by mover id.
    docks: Vec<MoverDock>,
    generation: u64,
}

impl MoverDockTable {
    /// Replace the watched set. Spawned-closed movers read as docked from the
    /// first mover tick, before any consumer of the tick.
    pub(super) fn watch(&mut self, mover_ids: &[u32]) {
        self.docks.clear();
        self.docks
            .extend(mover_ids.iter().map(|&mover_id| MoverDock {
                mover_id,
                waypoint: None,
                moves_on_its_own: false,
            }));
        self.docks.sort_unstable_by_key(|dock| dock.mover_id);
        self.docks.dedup_by_key(|dock| dock.mover_id);
    }

    pub(super) fn clear(&mut self) {
        self.docks.clear();
        self.generation = 0;
    }

    pub(super) fn update(&mut self, mover: &KinematicMoverComponent) {
        let Some(index) = self.index(mover.mover_id) else {
            return;
        };
        let dock = &mut self.docks[index];
        let waypoint = mover_docked_waypoint(mover);
        let moves_on_its_own = mover_moves_on_its_own(mover);
        if dock.waypoint != waypoint || dock.moves_on_its_own != moves_on_its_own {
            dock.waypoint = waypoint;
            dock.moves_on_its_own = moves_on_its_own;
            self.generation += 1;
        }
    }

    fn index(&self, mover_id: u32) -> Option<usize> {
        self.docks
            .binary_search_by_key(&mover_id, |dock| dock.mover_id)
            .ok()
    }

    /// The waypoint a watched mover rests at.
    pub fn docked_waypoint(&self, mover_id: u32) -> Option<u32> {
        self.index(mover_id)
            .and_then(|index| self.docks[index].waypoint)
    }

    /// Whether a watched mover will leave its dock by itself.
    pub fn moves_on_its_own(&self, mover_id: u32) -> bool {
        self.index(mover_id)
            .is_some_and(|index| self.docks[index].moves_on_its_own)
    }

    /// Bumped whenever a watched mover's dock state changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}
