// Dock state: which waypoint a mover rests at, read from its phase every tick.
// Navigation gates and links switch on this; collision never does.
// See: context/lib/movement.md §6 · context/lib/build_pipeline.md §Navigation bake

use postretro_entities::{KinematicMoverComponent, KinematicMoverMode};
use postretro_foundation::TICK_DURATION;
use postretro_level_format::kinematic_geometry::KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH;

use super::segment_indices;

/// The shortest wait at a stop, in fixed ticks, that surely holds a self-driven
/// mover at rest for one whole tick. A mover arrives part way through a tick, so
/// a shorter wait can begin and end between two tick boundaries, which is no dock.
const RESTING_WAIT_TICKS: f32 = 2.0;

/// [`RESTING_WAIT_TICKS`] at the fixed simulation step, which every mover tick
/// runs at, so the table needs no caller's dt.
const RESTING_WAIT_MS: f32 = RESTING_WAIT_TICKS * TICK_DURATION.as_secs_f32() * 1000.0;

/// The waypoint `mover` rests at this tick, if any. Resting means it began and
/// ended the tick there: a timed wait, a finished run, a `goToPathNode` hold,
/// never started, or held there by a pin after a command to leave. Touching a
/// waypoint in passing moves it that tick, so it is no dock. Camera occlusion
/// keeps its stricter `mover_is_docked_closed`.
pub fn mover_docked_waypoint(mover: &KinematicMoverComponent) -> Option<u32> {
    if mover.current_linear_velocity.length_squared() != 0.0 {
        return None;
    }
    if mover.segment_elapsed_ms <= f32::EPSILON {
        return Some(u32::from(mover.segment_index));
    }
    waypoint_at_segment_end(mover)
}

/// The waypoint a mover stands on when its phase sits at the far end of its
/// segment. A command back the way it came re-expresses a mover resting at a
/// waypoint as the end of the segment running back to it, elapsed at the
/// segment's full duration, so a pin that holds it there leaves it docked.
/// Duration and destination are derived as the phase driver derives them.
fn waypoint_at_segment_end(mover: &KinematicMoverComponent) -> Option<u32> {
    // Completed or waiting, the driver places the mover at `segment_index`.
    if mover.completed || mover.wait_remaining_ms > 0.0 {
        return None;
    }
    let (from, to) = segment_indices(mover)?;
    let length = (mover.waypoints[to] - mover.waypoints[from]).length();
    if !length.is_finite()
        || length <= KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH
        || mover.speed_mps <= 0.0
    {
        return None;
    }
    let duration_ms = (length / mover.speed_mps) * 1000.0;
    if !duration_ms.is_finite() {
        return None;
    }
    let short_of_end = duration_ms - mover.segment_elapsed_ms;
    (short_of_end <= f32::EPSILON * duration_ms.max(1.0)).then_some(to as u32)
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

/// Whether `mover`'s own motion ends each leg docked: a run that completes, or
/// a wait at each stop of at least [`RESTING_WAIT_TICKS`]. Mode and wait are
/// authored and never change, so this is fixed for the mover's life.
fn mover_rests_at_its_stops(mover: &KinematicMoverComponent) -> bool {
    mover.mode == KinematicMoverMode::Once || mover.wait_ms >= RESTING_WAIT_MS
}

/// One watched mover's dock state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MoverDock {
    mover_id: u32,
    waypoint: Option<u32>,
    moves_on_its_own: bool,
    /// Set from the component before each [`MoverDockTable::record`]. Fixed
    /// once the mover's first tick sets it, so it bumps no generation itself.
    rests_at_stops: bool,
    /// Updated by the current mover tick. An entry the tick never reaches has
    /// lost its mover, and is undocked once the tick ends.
    seen: bool,
}

/// Live dock state of the movers navigation records name. Host-only. A mover
/// nothing watches costs nothing and never changes the generation, so a sweeper
/// or a door without records wakes no agent.
#[derive(Debug, Clone, Default)]
pub struct MoverDockTable {
    /// Sorted by mover id.
    docks: Vec<MoverDock>,
    /// (mover id, waypoint) docks that close a portal, sorted and unique.
    gated: Vec<(u32, u32)>,
    /// (mover id, waypoint) docks that open a link, sorted and unique.
    linked: Vec<(u32, u32)>,
    generation: u64,
    opened_generation: u64,
}

impl MoverDockTable {
    /// Replace the watched set: every mover `gated` or `linked` names, as
    /// (mover id, waypoint) docks that close a portal or open a link. Spawned-
    /// closed movers read as docked from the first mover tick, before any
    /// consumer of the tick. The record sets are built here, once per level, so
    /// the per-tick update allocates nothing.
    pub(super) fn watch(&mut self, gated: &[(u32, u32)], linked: &[(u32, u32)]) {
        fn sorted(into: &mut Vec<(u32, u32)>, records: &[(u32, u32)]) {
            into.clear();
            into.extend_from_slice(records);
            into.sort_unstable();
            into.dedup();
        }
        sorted(&mut self.gated, gated);
        sorted(&mut self.linked, linked);
        self.docks.clear();
        self.docks.extend(
            self.gated
                .iter()
                .chain(self.linked.iter())
                .map(|&(mover_id, _)| MoverDock {
                    mover_id,
                    waypoint: None,
                    moves_on_its_own: false,
                    rests_at_stops: false,
                    seen: false,
                }),
        );
        self.docks.sort_unstable_by_key(|dock| dock.mover_id);
        self.docks.dedup_by_key(|dock| dock.mover_id);
    }

    pub(super) fn clear(&mut self) {
        self.docks.clear();
        self.gated.clear();
        self.linked.clear();
        self.generation = 0;
        self.opened_generation = 0;
    }

    /// Start a mover tick: no watched mover has been seen yet.
    pub(super) fn begin_tick(&mut self) {
        for dock in &mut self.docks {
            dock.seen = false;
        }
    }

    /// End a mover tick: a watched mover the tick never updated has despawned
    /// or lost its component, so it docks nowhere. Left as it was, its last
    /// dock would gate routes for the rest of the level.
    pub(super) fn undock_unseen(&mut self) {
        for index in 0..self.docks.len() {
            if !self.docks[index].seen {
                self.docks[index].rests_at_stops = false;
                self.record(index, None, false);
            }
        }
    }

    /// Record `mover`'s dock state as of this tick. Allocation-free.
    pub fn update(&mut self, mover: &KinematicMoverComponent) {
        let Some(index) = self.index(mover.mover_id) else {
            return;
        };
        let dock = &mut self.docks[index];
        dock.seen = true;
        dock.rests_at_stops = mover_rests_at_its_stops(mover);
        self.record(
            index,
            mover_docked_waypoint(mover),
            mover_moves_on_its_own(mover),
        );
    }

    /// Store one entry's new state, bumping the generations only on a change.
    fn record(&mut self, index: usize, waypoint: Option<u32>, moves_on_its_own: bool) {
        let before = self.docks[index];
        if before.waypoint == waypoint && before.moves_on_its_own == moves_on_its_own {
            return;
        }
        let opens = self.change_opens_a_route(before, waypoint, moves_on_its_own);
        let dock = &mut self.docks[index];
        dock.waypoint = waypoint;
        dock.moves_on_its_own = moves_on_its_own;
        self.generation += 1;
        if opens {
            self.opened_generation = self.generation;
        }
    }

    /// Whether moving `before` to the new state can open a route: it leaves a
    /// dock that closes a portal, docks where a link opens (a walk-across, on
    /// any mover), or, docked where a link opens, starts moving on its own so a
    /// ride can begin there. Only a mover that rests at its stops is ridden, so
    /// for any other that start opens nothing.
    fn change_opens_a_route(
        &self,
        before: MoverDock,
        waypoint: Option<u32>,
        moves_on_its_own: bool,
    ) -> bool {
        let mover_id = before.mover_id;
        let leaves_a_gate = before
            .waypoint
            .is_some_and(|left| waypoint != Some(left) && self.is_gated(mover_id, left));
        let link_opens = waypoint.is_some_and(|docked| {
            self.is_linked(mover_id, docked)
                && (before.waypoint != Some(docked)
                    || (before.rests_at_stops && moves_on_its_own && !before.moves_on_its_own))
        });
        leaves_a_gate || link_opens
    }

    fn is_gated(&self, mover_id: u32, waypoint: u32) -> bool {
        self.gated.binary_search(&(mover_id, waypoint)).is_ok()
    }

    fn is_linked(&self, mover_id: u32, waypoint: u32) -> bool {
        self.linked.binary_search(&(mover_id, waypoint)).is_ok()
    }

    fn index(&self, mover_id: u32) -> Option<usize> {
        self.docks
            .binary_search_by_key(&mover_id, |dock| dock.mover_id)
            .ok()
    }

    /// Whether this mover's docks are tracked.
    pub fn watches(&self, mover_id: u32) -> bool {
        self.index(mover_id).is_some()
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

    /// Whether a watched mover's own motion ends each leg docked: `Once` mode,
    /// or a wait at each stop of at least two fixed ticks. Only such a mover
    /// can be ridden; any other is plain ground to navigation, its links open
    /// for walking across while it happens to dock. False for a mover not
    /// watched, or not yet seen by a mover tick.
    pub fn rests_at_stops(&self, mover_id: u32) -> bool {
        self.index(mover_id)
            .is_some_and(|index| self.docks[index].rests_at_stops)
    }

    /// Bumped whenever a watched mover's dock state changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The [`Self::generation`] of the latest change that could open a route: a
    /// mover leaving a dock that closes a portal, docking where a link opens, or,
    /// resting at its stops, starting to move on its own while docked there. An
    /// agent that stamped the generation before it is owed a fresh look; one
    /// change that only closes routes leaves it alone. Never ahead of the
    /// generation.
    pub fn opened_generation(&self) -> u64 {
        self.opened_generation
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use postretro_entities::{
        EntityId, EntityRegistry, KinematicMoverComponent, KinematicMoverConfig,
        KinematicMoverMode, MoverCommand, Transform,
    };

    use super::super::{
        MoverTickStateTable, advance_mover_phase_one_tick, apply_mover_command,
        run_kinematic_mover_tick,
    };
    use super::{MoverDockTable, RESTING_WAIT_MS, mover_docked_waypoint};

    const DT: f32 = 1.0 / 60.0;
    const DOOR: u32 = 4;
    const LIFT: u32 = 5;

    /// An unstarted mover on `waypoints` at 2 m/s.
    fn mover(
        mover_id: u32,
        waypoints: Vec<Vec3>,
        mode: KinematicMoverMode,
        wait_ms: f32,
    ) -> KinematicMoverComponent {
        let names = (0..waypoints.len()).map(|i| format!("node{i}")).collect();
        KinematicMoverComponent::new(
            mover_id,
            KinematicMoverConfig {
                waypoints,
                waypoint_names: names,
                speed_mps: 2.0,
                wait_ms,
                mode,
                started: false,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        )
    }

    /// A lift whose waypoints stack 2 m apart.
    fn three_stop_lift() -> KinematicMoverComponent {
        mover(
            LIFT,
            vec![Vec3::ZERO, Vec3::Y * 2.0, Vec3::Y * 4.0],
            KinematicMoverMode::Once,
            0.0,
        )
    }

    fn step(mover: &mut KinematicMoverComponent, transform: &mut Transform) {
        advance_mover_phase_one_tick(mover, transform, DT);
    }

    /// Tick until `mover` completes its run, then once more so it rests.
    fn run_to_completion(mover: &mut KinematicMoverComponent, transform: &mut Transform) {
        for _ in 0..600 {
            if mover.completed {
                break;
            }
            step(mover, transform);
        }
        assert!(mover.completed, "the run completed");
        step(mover, transform);
    }

    fn go_to(mover: &mut KinematicMoverComponent, node: &str) {
        apply_mover_command(mover, &MoverCommand::GoToPathNode(node.to_string()));
    }

    // Regression: a command back the way it came re-expressed a mover resting at
    // a mid node as the far end of the segment back, so a pin holding it there
    // read it undocked: a closed door's gates opened and riders marked departed.
    #[test]
    fn a_mover_held_at_a_mid_node_after_being_sent_back_reads_docked_there() {
        // Arrived going up, then sent back down; and arrived going down, then
        // sent back up.
        let cases = [("node1", "node1", "node0"), ("node2", "node1", "node2")];
        for (first, arrive, back) in cases {
            let mut lift = three_stop_lift();
            let mut transform = Transform::default();
            go_to(&mut lift, first);
            run_to_completion(&mut lift, &mut transform);
            if first != arrive {
                go_to(&mut lift, arrive);
                run_to_completion(&mut lift, &mut transform);
            }
            assert_eq!(
                mover_docked_waypoint(&lift),
                Some(1),
                "{first}: rests at node1"
            );

            let mut docks = MoverDockTable::default();
            docks.watch(&[(LIFT, 1)], &[]);
            docks.update(&lift);
            let generation = docks.generation();

            go_to(&mut lift, back);
            assert!(
                lift.segment_elapsed_ms > 0.0,
                "{back}: the command re-expressed the phase at a segment's end"
            );
            // A pin holds it where it stands, as the pre-tick test does.
            lift.blocked = true;
            for _ in 0..3 {
                step(&mut lift, &mut transform);
                assert!((transform.position.y - 2.0).abs() < 1.0e-4, "held at node1");
                assert_eq!(
                    mover_docked_waypoint(&lift),
                    Some(1),
                    "{back}: still docked"
                );
                docks.update(&lift);
            }
            assert_eq!(docks.generation(), generation, "{back}: no gate flip");

            // Released, it leaves on the first tick.
            lift.blocked = false;
            step(&mut lift, &mut transform);
            assert_eq!(
                mover_docked_waypoint(&lift),
                None,
                "{back}: undocked on leaving"
            );
        }
    }

    #[test]
    fn a_door_leaving_a_gated_waypoint_opens_routes_and_docking_there_does_not() {
        let mut door = mover(
            DOOR,
            vec![Vec3::ZERO, Vec3::X * 2.0],
            KinematicMoverMode::Once,
            0.0,
        );
        let mut transform = Transform::default();
        let mut docks = MoverDockTable::default();
        docks.watch(&[(DOOR, 0)], &[]);

        // Spawned closed: docked at the gated waypoint, which opens nothing.
        step(&mut door, &mut transform);
        docks.update(&door);
        assert_eq!(docks.docked_waypoint(DOOR), Some(0));
        assert_eq!(docks.generation(), 1);
        assert_eq!(docks.opened_generation(), 0);

        apply_mover_command(&mut door, &MoverCommand::Start);
        step(&mut door, &mut transform);
        docks.update(&door);
        assert_eq!(docks.docked_waypoint(DOOR), None, "it left");
        assert_eq!(docks.generation(), 2);
        assert_eq!(docks.opened_generation(), 2, "leaving a gate opens routes");

        // Open at the far waypoint, which gates nothing: no route opens.
        run_to_completion(&mut door, &mut transform);
        docks.update(&door);
        assert_eq!(docks.docked_waypoint(DOOR), Some(1));
        assert_eq!(docks.generation(), 3);
        assert_eq!(docks.opened_generation(), 2);

        // Back to the gated waypoint: it closes routes, and opens none.
        apply_mover_command(&mut door, &MoverCommand::Reverse);
        for _ in 0..120 {
            step(&mut door, &mut transform);
            docks.update(&door);
        }
        assert_eq!(docks.docked_waypoint(DOOR), Some(0), "docked closed again");
        assert!(docks.generation() > 3);
        assert_eq!(
            docks.opened_generation(),
            2,
            "docking at a gate opens nothing"
        );
    }

    #[test]
    fn a_lift_docking_at_a_linked_waypoint_opens_routes() {
        let mut lift = mover(
            LIFT,
            vec![Vec3::ZERO, Vec3::Y * 2.0],
            KinematicMoverMode::PingPong,
            1000.0,
        );
        let mut transform = Transform::default();
        let mut docks = MoverDockTable::default();
        docks.watch(&[], &[(LIFT, 1)]);
        apply_mover_command(&mut lift, &MoverCommand::Start);

        let mut docked_at = None;
        for tick in 0..120 {
            step(&mut lift, &mut transform);
            docks.update(&lift);
            if docks.docked_waypoint(LIFT) == Some(1) {
                docked_at = Some(tick);
                break;
            }
            assert_eq!(
                docks.opened_generation(),
                0,
                "tick {tick}: nothing opened before it docked"
            );
        }
        assert!(docked_at.is_some(), "the lift waits at its top");
        assert!(docks.moves_on_its_own(LIFT));
        assert_eq!(
            docks.opened_generation(),
            docks.generation(),
            "docking at a link opens routes"
        );
    }

    #[test]
    fn a_mover_rests_at_its_stops_when_its_run_ends_or_it_waits_two_ticks() {
        const ONCE: u32 = 6;
        const WAITS: u32 = 7;
        const NO_WAIT: u32 = 8;
        const SHORT_WAIT: u32 = 9;
        const NEVER_TICKED: u32 = 10;
        let path = || vec![Vec3::ZERO, Vec3::Y * 2.0];
        let movers = [
            mover(ONCE, path(), KinematicMoverMode::Once, 0.0),
            mover(WAITS, path(), KinematicMoverMode::PingPong, 1000.0),
            mover(NO_WAIT, path(), KinematicMoverMode::PingPong, 0.0),
            mover(SHORT_WAIT, path(), KinematicMoverMode::PingPong, 30.0),
        ];
        const { assert!(30.0 < RESTING_WAIT_MS, "the short wait is under two ticks") };
        let mut docks = MoverDockTable::default();
        let linked = [ONCE, WAITS, NO_WAIT, SHORT_WAIT, NEVER_TICKED].map(|mover_id| (mover_id, 1));
        docks.watch(&[], &linked);
        for mover in &movers {
            docks.update(mover);
        }

        assert!(docks.rests_at_stops(ONCE), "a run that ends rests");
        assert!(docks.rests_at_stops(WAITS), "a long wait rests");
        assert!(!docks.rests_at_stops(NO_WAIT), "a pass-through never rests");
        assert!(
            !docks.rests_at_stops(SHORT_WAIT),
            "nor does a sub-tick wait"
        );
        assert!(!docks.rests_at_stops(NEVER_TICKED), "unknown until ticked");
        assert!(!docks.rests_at_stops(DOOR), "unwatched");
    }

    // A ride begins only on a mover that rests at its stops, so one that never
    // rests opens nothing by starting at a linked dock; docking there opens a
    // walk-across on either.
    #[test]
    fn starting_at_a_linked_dock_opens_a_route_only_on_a_lift_that_rests_at_its_stops() {
        for (wait_ms, opens_a_ride) in [(1000.0, true), (0.0, false)] {
            let mut lift = mover(
                LIFT,
                vec![Vec3::ZERO, Vec3::Y * 2.0],
                KinematicMoverMode::PingPong,
                wait_ms,
            );
            let mut docks = MoverDockTable::default();
            docks.watch(&[], &[(LIFT, 0)]);

            // Never started, it rests at its first stop.
            docks.update(&lift);
            assert_eq!(docks.docked_waypoint(LIFT), Some(0));
            assert_eq!(
                docks.opened_generation(),
                docks.generation(),
                "wait {wait_ms}: docking at a link opens a walk-across"
            );
            let docked = docks.generation();

            // Started, and still at that dock this tick, as a pin holds it.
            lift.started = true;
            docks.update(&lift);
            assert_eq!(docks.docked_waypoint(LIFT), Some(0));
            assert!(docks.moves_on_its_own(LIFT));
            assert_eq!(docks.generation(), docked + 1, "wait {wait_ms}: changed");
            assert_eq!(
                docks.opened_generation() == docks.generation(),
                opens_a_ride,
                "wait {wait_ms}: a ride opens only on a lift that rests"
            );
        }
    }

    /// One registry mover the mover tick drives.
    fn spawn(registry: &mut EntityRegistry, mover: KinematicMoverComponent) -> EntityId {
        let entity = registry.spawn(Transform {
            position: mover.waypoints[0],
            ..Transform::default()
        });
        registry
            .set_component(entity, mover)
            .expect("mover attaches");
        entity
    }

    // Regression: a despawned mover was skipped by the mover tick, so its last
    // dock kept gating routes for the rest of the level.
    #[test]
    fn a_despawned_mover_undocks_once_and_only_when_its_entry_changes() {
        let mut registry = EntityRegistry::new();
        let door = spawn(
            &mut registry,
            mover(
                DOOR,
                vec![Vec3::ZERO, Vec3::X * 2.0],
                KinematicMoverMode::Once,
                0.0,
            ),
        );
        let mut moving = mover(
            LIFT,
            vec![Vec3::ZERO, Vec3::Y * 20.0],
            KinematicMoverMode::Once,
            0.0,
        );
        moving.started = true;
        let lift = spawn(&mut registry, moving);
        let mut table = MoverTickStateTable::default();
        table.watch_mover_docks(&[(DOOR, 0)], &[(LIFT, 1)]);

        run_kinematic_mover_tick(&mut registry, &mut table, DT);
        run_kinematic_mover_tick(&mut registry, &mut table, DT);
        assert_eq!(table.docks().docked_waypoint(DOOR), Some(0));
        assert_eq!(table.docks().docked_waypoint(LIFT), None, "mid-travel");
        assert!(table.docks().rests_at_stops(DOOR), "a `Once` door rests");
        let generation = table.docks().generation();

        // The lift was docked nowhere: losing it changes nothing.
        registry.despawn(lift).expect("lift despawns");
        run_kinematic_mover_tick(&mut registry, &mut table, DT);
        assert_eq!(table.docks().generation(), generation);

        // The door was docked at its gate: losing it opens the portal once.
        registry.despawn(door).expect("door despawns");
        run_kinematic_mover_tick(&mut registry, &mut table, DT);
        assert_eq!(table.docks().docked_waypoint(DOOR), None);
        assert!(
            !table.docks().rests_at_stops(DOOR),
            "a lost mover rests nowhere"
        );
        assert_eq!(table.docks().generation(), generation + 1);
        assert_eq!(table.docks().opened_generation(), generation + 1);
        for _ in 0..3 {
            run_kinematic_mover_tick(&mut registry, &mut table, DT);
        }
        assert_eq!(table.docks().generation(), generation + 1, "cleared once");
    }
}
