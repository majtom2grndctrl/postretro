// Mover light state, game half: what the collector hands the renderer each
// frame from the registry's phase, and the grep gate on that hand-over.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use glam::Vec3;
use postretro_entities::{EntityRegistry, KinematicMoverComponent, MoverCommand};
use postretro_level_loader::{
    KinematicGeometry, LevelWorld, LoadedKinematicMover, LoadedKinematicWaypoint, MoverLightRamp,
};
use postretro_visibility::VisibleCells;

use super::light_state::{MoverLightStateCollector, ramped_travel_fraction};
use super::tests::{mover, single_cell_world};
use super::{KinematicMoverRenderCollector, mover_entity, spawn_from_geometry};
use crate::alloc_probe::AllocSnapshot;
use crate::kinematic_mover::{MoverTickStateTable, apply_mover_command, run_kinematic_mover_tick};
use crate::render::MoverLightState;

const EPSILON: f32 = 1.0e-5;

fn waypoint(name: &str, next: &str, origin: Vec3) -> LoadedKinematicWaypoint {
    LoadedKinematicWaypoint {
        name: name.to_string(),
        next: next.to_string(),
        origin,
    }
}

/// A loaded mover `mover_id` on the two-waypoint path `{path}0 → {path}1`,
/// `length` metres long, travelling at `speed` m/s with no endpoint wait.
fn loaded_mover(
    mover_id: u32,
    path: &str,
    length: f32,
    speed: f32,
    start_on_spawn: bool,
) -> (LoadedKinematicMover, [LoadedKinematicWaypoint; 2]) {
    let mut loaded = mover(0);
    loaded.mover_id = mover_id;
    loaded.name = format!("mover_{mover_id}");
    loaded.path = format!("{path}0");
    loaded.speed_mps = speed;
    loaded.wait_ms = 0.0;
    loaded.start_on_spawn = start_on_spawn;
    let waypoints = [
        waypoint(&format!("{path}0"), &format!("{path}1"), Vec3::ZERO),
        waypoint(&format!("{path}1"), "", Vec3::new(0.0, 0.0, length)),
    ];
    (loaded, waypoints)
}

fn world_of(movers: Vec<(LoadedKinematicMover, [LoadedKinematicWaypoint; 2])>) -> LevelWorld {
    let mut geometry = KinematicGeometry::default();
    for (loaded, waypoints) in movers {
        geometry.movers.push(loaded);
        geometry.waypoints.extend(waypoints);
    }
    single_cell_world(geometry)
}

fn spawned(world: &LevelWorld) -> EntityRegistry {
    let mut registry = EntityRegistry::new();
    spawn_from_geometry(&mut registry, &world.kinematic_geometry).expect("fixture movers spawn");
    registry
}

fn tick(registry: &mut EntityRegistry, table: &mut MoverTickStateTable, dt: f32, ticks: usize) {
    for _ in 0..ticks {
        run_kinematic_mover_tick(registry, table, dt);
    }
}

fn t_of(states: &[MoverLightState], mover_id: u32) -> f32 {
    let matching: Vec<f32> = states
        .iter()
        .filter(|state| state.mover_id == mover_id)
        .map(|state| state.t)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "mover {mover_id} has one state in {states:?}"
    );
    matching[0]
}

fn assert_t(states: &[MoverLightState], mover_id: u32, expected: f32, context: &str) {
    let t = t_of(states, mover_id);
    assert!(
        (t - expected).abs() <= EPSILON,
        "{context}: mover {mover_id} expected t' = {expected}, got {t}"
    );
}

/// Write `edit` into mover `mover_id`'s live phase, as a snapshot apply or a
/// script command does between frames.
fn edit_phase(
    registry: &mut EntityRegistry,
    mover_id: u32,
    edit: impl FnOnce(&mut KinematicMoverComponent),
) {
    let entity = mover_entity(registry, mover_id).expect("fixture mover is present");
    let mut component = registry
        .get_component::<KinematicMoverComponent>(entity)
        .expect("mover component")
        .clone();
    edit(&mut component);
    registry
        .set_component(entity, component)
        .expect("mover phase writes back");
}

#[test]
fn ramped_travel_fraction_reshapes_t_inside_the_author_window_only() {
    let identity = MoverLightRamp::IDENTITY;
    for t in [0.0, 0.25, 0.6, 0.75, 1.0] {
        assert_eq!(
            ramped_travel_fraction(t, identity),
            t,
            "default ramp keeps t"
        );
    }
    let window = MoverLightRamp {
        start: 0.6,
        end: 0.9,
    };
    for (t, expected) in [(0.0, 0.0), (0.6, 0.0), (0.75, 0.5), (0.9, 1.0), (1.0, 1.0)] {
        let ramped = ramped_travel_fraction(t, window);
        assert!(
            (ramped - expected).abs() <= EPSILON,
            "t = {t}: got {ramped}"
        );
    }
    let refused = MoverLightRamp {
        start: 0.5,
        end: 0.5,
    };
    assert_eq!(
        ramped_travel_fraction(0.3, refused),
        0.3,
        "an empty window reads as identity"
    );
    assert_eq!(ramped_travel_fraction(f32::NAN, window), 0.0, "never NaN");
}

/// S10 / P4, game half: a `start_on_spawn` mover and one held at its seeded
/// terminus both hand over their seeded t before any tick, so the first
/// composed frame is gated.
#[test]
fn collector_hands_over_the_seeded_t_before_any_tick() {
    let world = world_of(vec![
        loaded_mover(1, "spawn", 1.0, 1.0, true),
        loaded_mover(2, "held", 1.0, 1.0, false),
    ]);
    let registry = spawned(&world);
    let mut collector = MoverLightStateCollector::default();
    collector.collect(&registry, Some(&world));
    let states = collector.states();
    assert_eq!(states.len(), 2, "every present mover hands over a state");
    assert_eq!(t_of(states, 1), 0.0);
    assert_eq!(t_of(states, 2), 0.0);

    collector.collect(&registry, None);
    assert!(collector.states().is_empty(), "no level, no states");
}

/// S4: two movers advancing on one tick each report their own t.
#[test]
fn collector_reports_each_movers_own_t_for_the_tick() {
    let world = world_of(vec![
        loaded_mover(1, "slow", 1.0, 1.0, true),
        loaded_mover(2, "fast", 1.0, 2.0, true),
    ]);
    let mut registry = spawned(&world);
    let mut table = MoverTickStateTable::default();
    tick(&mut registry, &mut table, 0.25, 1);
    let mut collector = MoverLightStateCollector::default();
    collector.collect(&registry, Some(&world));
    assert_t(collector.states(), 1, 0.25, "1 m/s for a quarter second");
    assert_t(collector.states(), 2, 0.5, "2 m/s for a quarter second");
}

/// S12 / P10, game half: frames between two ticks hand over one t, and a
/// frame that ran two ticks hands over only the second tick's.
#[test]
fn collector_hands_over_the_last_post_tick_phase_however_many_frames_or_ticks() {
    let world = world_of(vec![loaded_mover(1, "door", 1.0, 1.0, true)]);
    let mut registry = spawned(&world);
    let mut table = MoverTickStateTable::default();
    let mut collector = MoverLightStateCollector::default();
    tick(&mut registry, &mut table, 0.25, 1);
    collector.collect(&registry, Some(&world));
    let first_frame = collector.states().to_vec();
    collector.collect(&registry, Some(&world));
    assert_eq!(
        collector.states(),
        first_frame.as_slice(),
        "no tick, same t"
    );

    tick(&mut registry, &mut table, 0.25, 2);
    collector.collect(&registry, Some(&world));
    assert_t(
        collector.states(),
        1,
        0.75,
        "the second tick's t, never the first's 0.5",
    );
}

/// S2, game half: the author ramp reshapes the hand-over rising and after a
/// reversal; a default-ramp mover hands over t itself.
#[test]
fn collector_applies_the_author_ramp_rising_and_after_a_reversal() {
    let (mut ramped, ramped_path) = loaded_mover(1, "ramped", 1.0, 1.0, true);
    ramped.light_ramp = MoverLightRamp {
        start: 0.6,
        end: 0.9,
    };
    let world = world_of(vec![
        (ramped, ramped_path),
        loaded_mover(2, "plain", 1.0, 1.0, true),
    ]);
    let mut registry = spawned(&world);
    let mut table = MoverTickStateTable::default();
    let mut collector = MoverLightStateCollector::default();
    let dt = 0.05;
    let mut check = |registry: &EntityRegistry, t: f32, ramped: f32, context: &str| {
        collector.collect(registry, Some(&world));
        assert_t(collector.states(), 2, t, context);
        assert_t(collector.states(), 1, ramped, context);
    };

    tick(&mut registry, &mut table, dt, 10);
    check(&registry, 0.5, 0.0, "rising below the window");
    tick(&mut registry, &mut table, dt, 2);
    check(&registry, 0.6, 0.0, "rising at the window start");
    tick(&mut registry, &mut table, dt, 3);
    check(&registry, 0.75, 0.5, "rising mid-window");
    tick(&mut registry, &mut table, dt, 3);
    check(&registry, 0.9, 1.0, "rising at the window end");
    tick(&mut registry, &mut table, dt, 1);
    check(&registry, 0.95, 1.0, "rising above the window");

    for mover_id in [1, 2] {
        edit_phase(&mut registry, mover_id, |mover| {
            apply_mover_command(mover, &MoverCommand::Reverse);
        });
    }
    check(&registry, 0.95, 1.0, "the reversal frame holds t");
    tick(&mut registry, &mut table, dt, 4);
    check(&registry, 0.75, 0.5, "falling mid-window");
    tick(&mut registry, &mut table, dt, 3);
    check(&registry, 0.6, 0.0, "falling at the window start");
    tick(&mut registry, &mut table, dt, 2);
    check(&registry, 0.5, 0.0, "falling below the window");
}

/// P15: a mover culled from the frame's drawn cells, and one with no draw
/// geometry, still hand over their state.
#[test]
fn collector_includes_movers_culled_from_view_and_movers_without_geometry() {
    let (mut bare, bare_path) = loaded_mover(2, "bare", 1.0, 1.0, true);
    bare.vertices.clear();
    bare.indices.clear();
    let world = world_of(vec![
        loaded_mover(1, "drawn", 1.0, 1.0, true),
        (bare, bare_path),
    ]);
    let registry = spawned(&world);
    let mut collector = KinematicMoverRenderCollector::new();
    let nothing_visible = VisibleCells::Culled(Vec::new());
    collector.collect_with_mirror(&registry, &world, &nothing_visible, &nothing_visible, 0.0);
    assert!(
        collector.instances().is_empty(),
        "both movers are culled from beauty"
    );
    let shadow_ids: Vec<u32> = collector
        .shadow_instances()
        .iter()
        .map(|instance| instance.mover_id)
        .collect();
    assert_eq!(shadow_ids, vec![1], "the geometry-less mover draws nothing");

    collector.collect_light_states(&registry, Some(&world));
    let mut ids: Vec<u32> = collector
        .light_states()
        .iter()
        .map(|state| state.mover_id)
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![1, 2], "every present mover hands over its state");
}

/// S13 / P13, game half: after a relevel that reuses a mover id, and after a
/// restart that re-seeds a mover, the hand-over is the new seeded t under the
/// new level's ramp, never the old level's or the pre-restart value.
#[test]
fn collector_hands_over_the_new_seeded_t_after_relevel_and_restart() {
    let world_a = world_of(vec![loaded_mover(7, "a", 1.0, 1.0, true)]);
    let mut registry_a = spawned(&world_a);
    let mut table = MoverTickStateTable::default();
    tick(&mut registry_a, &mut table, 0.25, 4);
    let mut collector = KinematicMoverRenderCollector::new();
    collector.collect_light_states(&registry_a, Some(&world_a));
    assert_t(collector.light_states(), 7, 1.0, "level A's mover arrived");

    // Level B reuses mover id 7 on a 2 m path with a ramp of its own. No
    // teardown clear runs here: the new mover list alone re-keys the ramps.
    let (mut reused, reused_path) = loaded_mover(7, "b", 2.0, 1.0, true);
    reused.light_ramp = MoverLightRamp {
        start: 0.5,
        end: 1.0,
    };
    let world_b = world_of(vec![(reused, reused_path)]);
    let mut registry_b = spawned(&world_b);
    collector.collect_light_states(&registry_b, Some(&world_b));
    assert_t(collector.light_states(), 7, 0.0, "level B's seeded t");
    let mut table = MoverTickStateTable::default();
    tick(&mut registry_b, &mut table, 0.25, 6);
    collector.collect_light_states(&registry_b, Some(&world_b));
    assert_t(
        collector.light_states(),
        7,
        0.5,
        "1.5 m of 2 m under B's ramp, not A's",
    );

    // Restart: the movers respawn at their seeded phase on the same level.
    let entity = mover_entity(&registry_b, 7).expect("mover 7 is present");
    registry_b.despawn(entity).expect("mover despawns");
    spawn_from_geometry(&mut registry_b, &world_b.kinematic_geometry).expect("respawn");
    collector.collect_light_states(&registry_b, Some(&world_b));
    assert_t(collector.light_states(), 7, 0.0, "the re-seeded t");

    // The teardown clear also forgets the level outright.
    collector.clear();
    assert!(collector.light_states().is_empty());
}

/// S14, game half: a phase written with no local tick (a snapshot apply, or
/// a reconciliation that moves it back) hands over that phase's t the frame
/// it lands.
#[test]
fn collector_hands_over_a_snapshot_applied_phase_the_frame_it_lands() {
    let world = world_of(vec![loaded_mover(1, "door", 1.0, 1.0, false)]);
    let mut registry = spawned(&world);
    let mut collector = MoverLightStateCollector::default();
    collector.collect(&registry, Some(&world));
    assert_t(collector.states(), 1, 0.0, "seeded before the snapshot");

    edit_phase(&mut registry, 1, |mover| {
        mover.started = true;
        mover.segment_elapsed_ms = 500.0;
    });
    collector.collect(&registry, Some(&world));
    assert_t(
        collector.states(),
        1,
        0.5,
        "the snapshot's mid-travel phase",
    );

    edit_phase(&mut registry, 1, |mover| {
        mover.segment_elapsed_ms = 250.0;
    });
    collector.collect(&registry, Some(&world));
    assert_t(collector.states(), 1, 0.25, "the reconciled phase");
}

/// S15: after warm-up, a frame's pass over two travelling movers allocates
/// nothing.
#[test]
fn collector_allocates_nothing_per_frame_after_warm_up() {
    let world = world_of(vec![
        loaded_mover(1, "one", 4.0, 1.0, true),
        loaded_mover(2, "two", 4.0, 2.0, true),
    ]);
    let mut registry = spawned(&world);
    let mut table = MoverTickStateTable::default();
    let mut collector = KinematicMoverRenderCollector::new();
    collector.collect_light_states(&registry, Some(&world));
    for frame in 0..4 {
        tick(&mut registry, &mut table, 0.25, 1);
        let probe = AllocSnapshot::arm();
        collector.collect_light_states(&registry, Some(&world));
        let allocations = probe.allocs_since();
        assert_eq!(allocations, 0, "frame {frame}: the t pass allocated");
        assert_eq!(collector.light_states().len(), 2);
    }
    let t = t_of(collector.light_states(), 2);
    assert!(
        t > 0.0 && t < 1.0,
        "both movers are still travelling, t = {t}"
    );
}

/// Identifier words in `source` with `//` comments removed: identifiers
/// split on `_` and camel-case boundaries, lowercased.
fn identifier_words(source: &str) -> Vec<String> {
    let mut words = Vec::new();
    for line in source.lines() {
        let code = line.split("//").next().unwrap_or("");
        for token in code.split(|c: char| !c.is_ascii_alphanumeric()) {
            let mut word = String::new();
            let mut previous_lower = false;
            for c in token.chars() {
                if c.is_ascii_uppercase() && previous_lower && !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
                word.push(c.to_ascii_lowercase());
            }
            if !word.is_empty() {
                words.push(word);
            }
        }
    }
    words
}

/// Words naming lighting storage: SH, probes, depth tiles, PRL sections.
const STORAGE_WORDS: &[&str] = &[
    "sh",
    "probe",
    "probes",
    "depth",
    "tile",
    "tiles",
    "section",
    "sections",
    "irradiance",
    "harmonic",
    "harmonics",
    "atlas",
    "moments",
];

fn storage_words_in(source: &str) -> Vec<String> {
    identifier_words(source)
        .into_iter()
        .filter(|word| STORAGE_WORDS.contains(&word.as_str()))
        .collect()
}

fn read_source(relative_to_crates: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits under crates/")
        .join(relative_to_crates);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The scanner itself must see storage names, or the gate passes vacuously.
#[test]
fn storage_word_scanner_flags_storage_type_names() {
    assert_eq!(storage_words_in("fn f(x: ShProbe) {}"), vec!["sh", "probe"]);
    assert_eq!(
        storage_words_in("let a = probe_depth_tiles;"),
        vec!["probe", "depth", "tiles"]
    );
    assert_eq!(
        storage_words_in("MoverProbeChannelSection"),
        vec!["probe", "section"]
    );
    assert!(storage_words_in("fn refresh(push: Vec<u32>) // probe").is_empty());
}

/// B19, first half: the game-side collector and the renderer setter's
/// signature and state type name no SH, probe, depth-tile or section type.
#[test]
fn mover_light_state_hand_over_names_no_lighting_storage_type() {
    let collector = read_source("postretro/src/runtime_movers/light_state.rs");
    let state_type = read_source("renderer/src/render/mover_light_state.rs");
    let state_type = state_type
        .split("#[cfg(test)]")
        .next()
        .expect("the state type precedes its tests");
    let models = read_source("renderer/src/render/renderer_models.rs");
    let setter_start = models
        .find("pub fn set_mover_light_states")
        .expect("the renderer setter exists");
    let setter_signature = models[setter_start..]
        .split('{')
        .next()
        .expect("the setter has a body");
    assert!(setter_signature.contains("MoverLightState"));

    for (scanned, source) in [
        ("game-side collector", collector.as_str()),
        ("renderer state type", state_type),
        ("renderer setter signature", setter_signature),
    ] {
        let found = storage_words_in(source);
        assert!(
            found.is_empty(),
            "{scanned} names lighting storage: {found:?}"
        );
    }
}
