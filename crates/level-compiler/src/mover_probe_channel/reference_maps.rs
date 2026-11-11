// Static-brush reference maps for the mover probe channel's fixture tests: a
// dev map with one mover made worldspawn brushes at a pose along its path.
// See: context/lib/testing_guide.md §4 (Test fixtures)

use glam::DVec3;

/// Quake map units to engine meters.
const UNIT: f64 = 0.0254;

/// The raised room's interior, Quake units: inside its walls, floor and
/// roof, the authored lowered brushes lifted 176 units.
pub(crate) fn in_raised_room(engine: DVec3) -> bool {
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
pub(crate) fn moved_brushes(entity: &str, offset: [f64; 3]) -> Vec<String> {
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
pub(crate) fn staticize(map: &str, name: &str, waypoint: &str) -> String {
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
    staticize_at(
        map,
        name,
        [to[0] - from[0], to[1] - from[1], to[2] - from[2]],
    )
}

/// An engine-space point or offset (metres) in Quake map units.
pub(crate) fn quake_offset(engine: DVec3) -> [f64; 3] {
    [-engine.z / UNIT, -engine.x / UNIT, engine.y / UNIT]
}

/// `map` with mover `name` made static at `offset` (Quake units) from its
/// path's first waypoint: its brushes join the worldspawn moved by it, and
/// the mover and its waypoints leave the map. The static-brush reference for
/// a pose anywhere along the path.
pub(crate) fn staticize_at(map: &str, name: &str, offset: [f64; 3]) -> String {
    let blocks = entities(map);
    let named = |wanted: &str| {
        blocks
            .iter()
            .find(|block| key_of(block, "name").as_deref() == Some(wanted))
            .unwrap_or_else(|| panic!("no entity `{wanted}`"))
    };
    let mover = named(name);
    let first = key_of(mover, "path").expect("a mover path");
    let moved = moved_brushes(mover, offset);
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
