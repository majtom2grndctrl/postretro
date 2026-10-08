// The texture tool's default height terraces agree with the engine's prefix
// table. See: context/lib/resource_management.md §4.6
//
// The tool ships as source in SDK bundles without `crates/`, so it cannot
// depend on this crate. This test reads the tool's table from its source
// instead: the engine side is the one that can see both.

use postretro_render_data::material::{MATERIAL_PREFIXES, Material};

const TOOL_SOURCE: &str = include_str!("../../../tools/texture-tool/src/lib.rs");

/// The body of `pub const <name>: <ty> = <body>;` in the tool's source.
fn tool_const(name: &str) -> &'static str {
    let at = TOOL_SOURCE
        .find(&format!("pub const {name}:"))
        .unwrap_or_else(|| panic!("the texture tool must declare `{name}`"));
    let rest = &TOOL_SOURCE[at..];
    let body = &rest[rest.find('=').expect("a const has a value") + 1..];
    &body[..body.find(';').expect("a const terminates")]
}

/// The tool's `(prefix, levels)` table.
fn tool_table() -> Vec<(String, u32)> {
    let body = tool_const("HEIGHT_QUANTIZE_LEVELS_BY_PREFIX");
    body.split('(')
        .skip(1)
        .map(|entry| {
            let entry = &entry[..entry.find(')').expect("an entry closes")];
            let (name, levels) = entry.split_once(',').expect("an entry is a pair");
            (
                name.trim().trim_matches('"').to_owned(),
                levels.trim().parse().expect("levels is an integer"),
            )
        })
        .collect()
}

/// Twice the engine's terraces per direction: the tool counts both directions.
fn engine_levels(material: Material) -> u32 {
    2 * material.surface_depth().quantize_levels
}

#[test]
fn the_texture_tool_terraces_every_prefix_like_the_engine() {
    let table = tool_table();
    let fallback: u32 = tool_const("DEFAULT_HEIGHT_QUANTIZE_LEVELS")
        .trim()
        .parse()
        .expect("the fallback is an integer");
    assert_eq!(fallback, engine_levels(Material::Default));

    let tool_levels = |prefix: &str| {
        table
            .iter()
            .find(|(name, _)| name == prefix)
            .map_or(fallback, |&(_, levels)| levels)
    };
    for (prefix, material) in MATERIAL_PREFIXES {
        // A flat material marches nothing, so its terraces never reach the
        // screen; the tool's fallback is harmless there.
        if material.surface_depth().is_enabled() {
            assert_eq!(
                tool_levels(prefix),
                engine_levels(material),
                "`{prefix}`: the tool's default height terraces disagree with the engine",
            );
        }
    }
    for (name, _) in &table {
        assert!(
            MATERIAL_PREFIXES
                .iter()
                .any(|&(prefix, material)| prefix == name && material.surface_depth().is_enabled()),
            "the tool lists `{name}`, which is not an engine prefix with depth",
        );
    }
}
