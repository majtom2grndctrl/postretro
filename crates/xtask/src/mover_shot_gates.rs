// Repository grep gates for movers blocking shots and sight: every shot,
// splash and sight site calls a combined (static plus mover) query, static
// line of sight stays in host hit validation, no consumer branches on a hit's
// source, client shots read the predicted mover table, and the human docs no
// longer say movers pass shots, splash or sight. Run by preflight's
// `cargo test`, beside the other surface gates.
// See: context/lib/networking.md §Combat authority, §World-LOS-only validation ·
//      context/lib/entity_model.md §Components (Splash), §7c

use std::path::Path;

use crate::surface_gates::{line_of, relative, text_files, workspace_root};

/// Production code that resolves a shot, a splash or an NPC sightline.
const SHOT_AND_SIGHT_SOURCES: &[&str] = &[
    "crates/sim/src/weapon",
    "crates/sim/src/sim/projectile_stage.rs",
    "crates/sim/src/sim/splash.rs",
    "crates/sim/src/sim/mover_blast.rs",
    "crates/sim/src/sim/weapon_stage",
    "crates/sim/src/combat_positioning.rs",
    "crates/sim/src/combat_positioning",
    "crates/ai/src",
    "crates/netcode/src/splash_march.rs",
];

/// Each of these must call the combined query it names.
const COMBINED_CALLS: &[(&str, &str)] = &[
    ("crates/sim/src/weapon/mod.rs", "cast_ray_combined("),
    (
        "crates/sim/src/weapon/mod.rs",
        "cast_sphere_exact_combined(",
    ),
    (
        "crates/sim/src/sim/projectile_stage.rs",
        "cast_sphere_exact_combined(",
    ),
    ("crates/sim/src/sim/splash.rs", "line_of_sight_combined("),
    (
        "crates/sim/src/sim/mover_blast.rs",
        "mover_enclosing_point(",
    ),
    (
        "crates/netcode/src/splash_march.rs",
        "mover_enclosing_point(",
    ),
    ("crates/ai/src/perception.rs", "line_of_sight_combined("),
    (
        "crates/sim/src/combat_positioning.rs",
        "line_of_sight_combined(",
    ),
    (
        "crates/netcode/src/splash_march.rs",
        "cast_sphere_exact_combined(",
    ),
];

/// Static-only shot and sight queries. None may appear in a shot or sight site.
const STATIC_QUERIES: &[&str] = &["line_of_sight(", "cast_sphere_exact(", "cast_ray("];

/// The only shot and sight functions that may build a world with no movers,
/// as (file, enclosing function). None fires against a level's movers.
const STATIC_ONLY_WORLDS: &[(&str, &str)] = &[
    // Test-support wrapper; the level path passes the tick's combined world.
    (
        "crates/sim/src/sim/weapon_stage/commands/local.rs",
        "run_local_weapon_command",
    ),
    // Equip never fires.
    (
        "crates/sim/src/sim/weapon_stage/commands/local.rs",
        "run_client_weapon_equip",
    ),
    // `#[cfg(test)]` fire helper, outside the cut test module.
    ("crates/sim/src/weapon/mod.rs", "tick_resolved"),
];

/// The only production home of static line of sight outside physics: host
/// hit validation (`networking.md` §World-LOS-only validation).
const VALIDATION_SOURCE: &str = "crates/netcode/src/lib.rs";

/// Reads of a hit's source that no shot, splash or sight consumer may branch on.
const SOURCE_READS: &[&str] = &[".source()", ".is_mover()", "CollisionSource"];

/// Client shot paths and the call each makes with its mover world.
const CLIENT_SHOT_CALLS: &[(&str, &str)] = &[
    (
        "crates/postretro/src/client_weapon/mod.rs",
        "weapon::resolve_client_shot(",
    ),
    ("crates/postretro/src/main.rs", "sim::advance_predicted("),
];

/// Human-facing files the Docs decision lists, and phrases that said movers
/// pass shots, splash or sight.
const DOC_FILES: &[&str] = &[
    "docs/scripting-reference.md",
    "docs/level_design.md",
    "sdk/lib/brain.ts",
    "sdk/lib/brain.luau",
    "sdk/types/postretro.d.ts",
    "sdk/types/postretro.d.luau",
    "crates/scripting-core/src/typedef/templates/sdk_lib.d.ts",
    "crates/scripting-core/src/typedef/templates/sdk_lib.luau",
    "crates/sim/src/scripting/primitives/mod.rs",
    "content/dev/maps/mover-setpiece.README.md",
];
const RETIRED_DOC_PHRASES: &[&str] = &[
    "do not yet block",
    "does not yet stop its line of sight",
    "static-world sightline",
    "static-world LOS",
    "static-world occlusion",
    "static-world collision ray",
    "movers and\nother entities do not",
    "movers and other entities do not",
];

/// A file's production text: test-only files are skipped, and a trailing
/// `#[cfg(test)]` test module is cut.
fn production_text(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let in_tests_dir = path.components().any(|part| part.as_os_str() == "tests");
    if name.contains("test") || in_tests_dir || path.extension()? != "rs" {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let cut = [
        "#[cfg(test)]\nmod tests",
        "#[cfg(test)]\npub(crate) mod tests",
    ]
    .iter()
    .filter_map(|marker| text.find(marker))
    .min()
    .unwrap_or(text.len());
    Some(text[..cut].to_string())
}

fn sources(root: &Path, entries: &[&str]) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in entries {
        let path = root.join(entry);
        if path.is_dir() {
            files.extend(text_files(&path));
        } else {
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    files
}

/// Byte offsets of `needle` not preceded by an identifier character, so
/// `line_of_sight(` does not match `line_of_sight_combined(`'s prefix and
/// `cast_ray(` does not match `cast_ray_against_segment(`.
fn calls(text: &str, needle: &str) -> Vec<usize> {
    text.match_indices(needle)
        .filter(|(at, _)| {
            !text[..*at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
        .map(|(at, _)| at)
        .collect()
}

/// The name of the last `fn` item that starts before `at`: the function a
/// call at `at` sits in, for files without nested `fn` items.
fn enclosing_fn(text: &str, at: usize) -> Option<&str> {
    let before = &text[..at];
    let mut end = before.len();
    while let Some(found) = before[..end].rfind("fn ") {
        let at_boundary = before[..found]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if at_boundary {
            let name = &before[found + "fn ".len()..];
            let len = name
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(name.len());
            return Some(&name[..len]);
        }
        end = found;
    }
    None
}

/// The argument text of the call starting at `open` (the byte after its `(`).
fn call_arguments(text: &str, open: usize) -> &str {
    let mut depth = 1;
    for (offset, c) in text[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open..open + offset];
                }
            }
            _ => {}
        }
    }
    &text[open..]
}

#[test]
fn shot_splash_and_sight_sites_call_combined_queries_only() {
    let root = workspace_root();
    let mut hits = Vec::new();
    for path in sources(&root, SHOT_AND_SIGHT_SOURCES) {
        let Some(text) = production_text(&path) else {
            continue;
        };
        for query in STATIC_QUERIES {
            for at in calls(&text, query) {
                hits.push(format!(
                    "{}:{}: {query}",
                    relative(&root, &path),
                    line_of(&text, at)
                ));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "static shot or sight queries remain:\n{}",
        hits.join("\n")
    );

    for (file, call) in COMBINED_CALLS {
        let text = production_text(&root.join(file)).expect("a production source");
        assert!(
            !calls(&text, call).is_empty(),
            "{file} no longer calls {call}"
        );
    }
}

#[test]
fn static_only_worlds_stay_in_sites_that_never_fire() {
    let root = workspace_root();
    let mut stray = Vec::new();
    for path in sources(&root, SHOT_AND_SIGHT_SOURCES) {
        let Some(text) = production_text(&path) else {
            continue;
        };
        let file = relative(&root, &path);
        for at in calls(&text, "static_only(") {
            let function = enclosing_fn(&text, at).unwrap_or("");
            let allowed = STATIC_ONLY_WORLDS
                .iter()
                .any(|(site, name)| *site == file && *name == function);
            if !allowed {
                stray.push(format!(
                    "{file}:{}: static_only( in {function}",
                    line_of(&text, at)
                ));
            }
        }
    }
    assert!(
        stray.is_empty(),
        "shot or sight sites build a world without movers:\n{}",
        stray.join("\n")
    );
}

#[test]
fn static_line_of_sight_remains_only_in_host_hit_validation() {
    let root = workspace_root();
    let mut sites = Vec::new();
    for crate_dir in [
        "crates/sim/src",
        "crates/ai/src",
        "crates/netcode/src",
        "crates/postretro/src",
    ] {
        for path in text_files(&root.join(crate_dir)) {
            let Some(text) = production_text(&path) else {
                continue;
            };
            for at in calls(&text, "line_of_sight(") {
                sites.push((relative(&root, &path), line_of(&text, at)));
            }
        }
    }
    assert!(
        !sites.is_empty(),
        "host validation still reads static line of sight"
    );
    let stray: Vec<_> = sites
        .iter()
        .filter(|(file, _)| file != VALIDATION_SOURCE)
        .collect();
    assert!(
        stray.is_empty(),
        "static line of sight outside validation: {stray:?}"
    );
}

#[test]
fn no_shot_or_sight_consumer_branches_on_a_hits_source_and_the_read_is_public() {
    let root = workspace_root();
    let mut reads = Vec::new();
    for path in sources(&root, SHOT_AND_SIGHT_SOURCES) {
        let Some(text) = production_text(&path) else {
            continue;
        };
        for read in SOURCE_READS {
            for (at, _) in text.match_indices(read) {
                reads.push(format!("{}:{}", relative(&root, &path), line_of(&text, at)));
            }
        }
    }
    assert!(
        reads.is_empty(),
        "consumers branch on a hit's source:\n{}",
        reads.join("\n")
    );

    let moving = std::fs::read_to_string(root.join("crates/physics/src/collision/moving.rs"))
        .expect("physics combined queries");
    for public in [
        "pub fn source(&self) -> CollisionSource",
        "pub fn is_mover(&self) -> bool",
        "pub enum CollisionSource",
    ] {
        assert!(
            moving.contains(public),
            "the hit-source read is public: {public}"
        );
    }
}

#[test]
fn client_shot_paths_read_the_predicted_mover_table_never_a_presented_pose() {
    let root = workspace_root();
    for (file, call) in CLIENT_SHOT_CALLS {
        let text = production_text(&root.join(file)).expect("a production source");
        let at = calls(&text, call);
        assert!(!at.is_empty(), "{file} no longer calls {call}");
        for start in at {
            // The aim may be the presented one; the movers the shot casts
            // against may not.
            let arguments = call_arguments(&text, start + call.len());
            let world_at = arguments
                .find("CombinedCollisionWorld::new(")
                .unwrap_or_else(|| {
                    panic!(
                        "{file}:{}: {call} casts against a combined world",
                        line_of(&text, start)
                    )
                });
            let world = call_arguments(arguments, world_at + "CombinedCollisionWorld::new(".len());
            assert!(
                world.contains("&self.kinematic_mover_tick_states"),
                "{file}:{}: {call} builds its movers from the prediction table",
                line_of(&text, start)
            );
            assert!(
                !world.contains("present"),
                "{file}:{}: {call} must not pose movers from a presentation",
                line_of(&text, start)
            );
        }
    }
}

#[test]
fn listed_docs_no_longer_say_movers_pass_shots_splash_or_sight() {
    let root = workspace_root();
    let mut found = Vec::new();
    for file in DOC_FILES {
        let text = std::fs::read_to_string(root.join(file)).expect("a listed doc");
        for phrase in RETIRED_DOC_PHRASES {
            for (at, _) in text.match_indices(phrase) {
                found.push(format!("{file}:{}: {phrase:?}", line_of(&text, at)));
            }
        }
    }
    assert!(
        found.is_empty(),
        "docs still say movers pass shots, splash or sight:\n{}",
        found.join("\n")
    );
}

#[test]
fn gate_helpers_match_whole_calls_and_their_arguments() {
    assert_eq!(
        calls("line_of_sight_combined(a)", "line_of_sight("),
        Vec::<usize>::new()
    );
    assert_eq!(calls("x(line_of_sight(a))", "line_of_sight("), vec![2]);
    assert_eq!(
        calls("cast_ray_against_segment(", "cast_ray("),
        Vec::<usize>::new()
    );
    assert_eq!(call_arguments("f(a, g(b), c) d", 2), "a, g(b), c");
    let text = "fn outer() {}\npub(crate) fn inner_fn(x: impl FnMut()) { g(); }";
    assert_eq!(
        enclosing_fn(text, text.find("g()").unwrap()),
        Some("inner_fn")
    );
    assert_eq!(enclosing_fn("let defn = 1; g()", 14), None);
}
