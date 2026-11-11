// Direct clause: with `setpiece_room` raised, at every interior
// probe valid in both bakes, each light the static-room reference's walls
// block composes S₁ = q₁ · T within one q step (T / 255) of the reference's
// term for that light. The reference is the setpiece with the room made a
// static brush at its raised pose; the base clause is
// `tests/mover_probe_channel_bounce.rs`
// `raised_room_bounce_is_nearer_the_static_room_reference_than_today`.

use std::collections::HashMap;
use std::path::Path;

use super::direct::ProbeCells;
use super::fixture_scene::Scene;
use super::reference_maps::{in_raised_room, staticize};
use crate::fixture_pipeline::{fixture_path, load_map};

const ROOM: &str = "setpiece_room";
const RAISED: &str = "setpiece_room_raised";

fn max_abs(terms: &[f32; 27]) -> f32 {
    terms.iter().fold(0.0, |max, value| max.max(value.abs()))
}

#[test]
fn raised_room_direct_composes_within_one_q_step_of_the_static_room_reference() {
    let map = std::fs::read_to_string(fixture_path("mover-setpiece")).expect("setpiece map");
    let dir = tempfile::tempdir().expect("reference directory");
    let reference_path = dir.path().join("mover-setpiece-room-raised.map");
    std::fs::write(&reference_path, staticize(&map, ROOM, RAISED)).expect("write reference");
    let today = Scene::of(load_map(&fixture_path("mover-setpiece")));
    let reference = Scene::of(load_map(Path::new(&reference_path)));
    assert_eq!(
        (today.layout.dims, today.layout.world_min),
        (reference.layout.dims, reference.layout.world_min),
        "both builds lay one probe grid"
    );
    let room = today
        .fixture
        .kinematic_movers
        .iter()
        .find(|mover| mover.name == ROOM)
        .expect("the setpiece's room")
        .mover_id;

    // Every light, gated as a static light under its own seed axis: the
    // channel's q and both terms trace the same joint sample set.
    let lights = today.static_channel_lights();
    let cells = ProbeCells::of_layout(&today.layout);
    let bake = today.bake(&lights, &cells);
    // q₁ per (light, probe) for the room raised; an unstored probe keeps 255.
    let mut raised: HashMap<(u32, usize), u8> = HashMap::new();
    for light in &bake.lights {
        for entry in light.entries.iter().filter(|entry| entry.mover_id == room) {
            for (slot, probe) in cells
                .valid_probes(entry.cell as usize)
                .into_iter()
                .enumerate()
            {
                let last = entry.pose_count - 1;
                raised.insert((light.light_index, probe), entry.poses_at(slot)[last]);
            }
        }
    }

    let (mut compared, mut blocked_pairs, mut worst) = (0, 0, 0.0f32);
    let mut blocked_lights = std::collections::BTreeSet::new();
    let mut misses = Vec::new();
    for probe in 0..today.layout.probe_positions.len() {
        if today.layout.validity[probe] == 0
            || reference.layout.validity[probe] == 0
            || !in_raised_room(today.layout.probe_positions[probe])
        {
            continue;
        }
        compared += 1;
        for light in &lights {
            let (term, reference_term) = (today.term(light, probe), reference.term(light, probe));
            let step = max_abs(&term) / 255.0;
            let mut gap = [0f32; 27];
            for (k, value) in gap.iter_mut().enumerate() {
                *value = term[k] - reference_term[k];
            }
            if max_abs(&gap) <= step {
                continue; // The reference's walls do not block this light here.
            }
            blocked_pairs += 1;
            blocked_lights.insert(light.light_index);
            let q = raised
                .get(&(light.light_index, probe))
                .copied()
                .unwrap_or(255);
            let scale = f32::from(q) / 255.0;
            let mut composed_gap = [0f32; 27];
            for (k, value) in composed_gap.iter_mut().enumerate() {
                *value = scale * term[k] - reference_term[k];
            }
            let miss = max_abs(&composed_gap) / step;
            worst = worst.max(miss);
            if miss > 1.0 + 1.0e-4 {
                let reference_q = 255.0 * max_abs(&reference_term) / max_abs(&term);
                misses.push(format!(
                    "probe {probe} at {:?}, light {}: q₁ {q} against the reference's {reference_q:.1}, \
                     {miss:.2} q steps",
                    today.layout.probe_positions[probe],
                    light.light_index
                ));
            }
        }
    }
    eprintln!(
        "B7 direct: {compared} interior probes valid in both, {blocked_pairs} blocked \
         (probe, light) pairs over {} light(s); worst |S₁ − reference| {worst} q steps",
        blocked_lights.len()
    );
    assert!(
        misses.is_empty(),
        "{} of {blocked_pairs} blocked pair(s) miss one q step:\n{}",
        misses.len(),
        misses.join("\n")
    );
    assert!(compared > 100, "the room holds its probes: {compared}");
    assert!(
        blocked_lights.len() > 1,
        "the reference's walls block more than one light"
    );
}
