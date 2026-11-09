// Direct clause: with `setpiece_room` raised, at every interior
// probe valid in both bakes, each light the static-room reference's walls
// block composes S₁ = q₁ · T within one q step (T / 255) of the reference's
// term for that light. The reference is the setpiece with the room made a
// static brush at its raised pose; the base clause is
// `tests/mover_probe_channel_bounce.rs`
// `raised_room_bounce_is_nearer_the_static_room_reference_than_today`.

use std::collections::HashMap;
use std::path::Path;

use glam::DVec3;

use super::direct::ProbeCells;
use super::fixture_scene::Scene;
use crate::fixture_pipeline::{fixture_path, load_map};

const ROOM: &str = "setpiece_room";
const RAISED: &str = "setpiece_room_raised";
/// Quake map units to engine meters.
const UNIT: f64 = 0.0254;

/// The raised room's interior, Quake units: inside its walls, floor and
/// roof, the authored lowered brushes lifted 176 units.
fn in_raised_room(engine: DVec3) -> bool {
    const MIN: [f64; 3] = [848.0, 176.0, 0.0];
    const MAX: [f64; 3] = [1136.0, 464.0, 160.0];
    let quake = [-engine.z / UNIT, -engine.x / UNIT, engine.y / UNIT];
    (0..3).all(|axis| MIN[axis] < quake[axis] && quake[axis] < MAX[axis])
}

/// Top-level `{ … }` entity blocks of a map, comments dropped.
fn entities(map: &str) -> Vec<String> {
    let (mut out, mut depth, mut current) = (Vec::new(), 0, String::new());
    for line in map.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        if trimmed == "{" {
            depth += 1;
        }
        if depth > 0 {
            current.push_str(line);
            current.push('\n');
        }
        if trimmed == "}" {
            depth -= 1;
            if depth == 0 {
                out.push(std::mem::take(&mut current));
            }
        }
    }
    out
}

fn key_of(entity: &str, key: &str) -> Option<String> {
    let prefix = format!("\"{key}\" \"");
    entity.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix('"'))
            .map(str::to_string)
    })
}

fn origin_of(entity: &str) -> [f64; 3] {
    let values: Vec<f64> = key_of(entity, "origin")
        .expect("an origin")
        .split_whitespace()
        .map(|value| value.parse().expect("a coordinate"))
        .collect();
    [values[0], values[1], values[2]]
}

/// The brush blocks of one entity block, each with every plane point moved
/// by `offset`.
fn moved_brushes(entity: &str, offset: [f64; 3]) -> Vec<String> {
    let mut brushes = Vec::new();
    let mut current: Option<Vec<String>> = None;
    for line in entity.lines().skip(1) {
        let trimmed = line.trim();
        match (trimmed, current.as_mut()) {
            ("{", None) => current = Some(vec!["{".to_string()]),
            ("}", Some(lines)) => {
                lines.push("}".to_string());
                brushes.push(lines.join("\n"));
                current = None;
            }
            (side, Some(lines)) if side.starts_with('(') => {
                let mut out = String::new();
                let mut rest = side;
                for _ in 0..3 {
                    let open = rest.find('(').expect("a plane point");
                    let close = rest.find(')').expect("a closed plane point");
                    let point: Vec<f64> = rest[open + 1..close]
                        .split_whitespace()
                        .map(|value| value.parse().expect("a coordinate"))
                        .collect();
                    out.push_str(&format!(
                        "( {} {} {} ) ",
                        point[0] + offset[0],
                        point[1] + offset[1],
                        point[2] + offset[2]
                    ));
                    rest = &rest[close + 1..];
                }
                out.push_str(rest.trim());
                lines.push(out);
            }
            (_, Some(lines)) => lines.push(trimmed.to_string()),
            _ => {}
        }
    }
    brushes
}

/// `map` with mover `name` made static at waypoint `waypoint`: its brushes
/// join the worldspawn moved by that waypoint's offset from its path's first,
/// and the mover and its waypoints leave the map.
fn staticize(map: &str, name: &str, waypoint: &str) -> String {
    let blocks = entities(map);
    let named = |wanted: &str| {
        blocks
            .iter()
            .find(|block| key_of(block, "name").as_deref() == Some(wanted))
            .unwrap_or_else(|| panic!("no entity `{wanted}`"))
    };
    let mover = named(name);
    let first = key_of(mover, "path").expect("a mover path");
    let (from, to) = (origin_of(named(&first)), origin_of(named(waypoint)));
    let moved = moved_brushes(mover, [to[0] - from[0], to[1] - from[1], to[2] - from[2]]);
    let mut path = vec![first];
    while let Some(next) = key_of(named(path.last().unwrap()), "next")
        .filter(|next| !next.is_empty() && !path.contains(next))
    {
        path.push(next);
    }
    let mut out = Vec::new();
    for block in &blocks {
        let classname = key_of(block, "classname");
        if classname.as_deref() == Some("worldspawn") {
            let body = block.trim().strip_suffix('}').expect("a closed worldspawn");
            out.push(format!("{body}{}\n}}", moved.join("\n")));
        } else if !std::ptr::eq(block, mover)
            && !(classname.as_deref() == Some("kinematic_waypoint")
                && key_of(block, "name").is_some_and(|name| path.contains(&name)))
        {
            out.push(block.clone());
        }
    }
    out.join("\n") + "\n"
}

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
                raised.insert((light.light_index, probe), entry.visibility[2 * slot + 1]);
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
