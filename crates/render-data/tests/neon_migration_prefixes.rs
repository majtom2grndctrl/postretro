// The neon-prefix migration tool refuses a rename into a known material
// prefix, so its copy of the prefix table must match the engine's.
// See: context/lib/resource_management.md §3
//
// The tool ships as source in SDK bundles without `crates/`, so it cannot read
// this crate. This test reads the tool's table from its source instead: the
// engine side is the one that can see both.

use postretro_render_data::material::MATERIAL_PREFIXES;

const TOOL_SOURCE: &str = include_str!("../../../tools/migrate_neon_prefix.py");

/// The string literals of the tool's `KNOWN_MATERIAL_PREFIXES = (...)` tuple.
fn tool_prefixes() -> Vec<String> {
    let at = TOOL_SOURCE
        .find("KNOWN_MATERIAL_PREFIXES = (")
        .expect("the migration tool must declare `KNOWN_MATERIAL_PREFIXES`");
    let rest = &TOOL_SOURCE[at..];
    let body = &rest[rest.find('(').expect("a tuple opens") + 1..];
    let body = &body[..body.find(')').expect("a tuple closes")];
    body.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_neon_migration_tool_knows_every_engine_material_prefix() {
    let engine: Vec<String> = MATERIAL_PREFIXES
        .iter()
        .map(|(prefix, _)| (*prefix).to_owned())
        .collect();
    assert_eq!(tool_prefixes(), engine);
}
