// Streaming-hint rows: seams resolve against the runtime portals between
// occluder-tier cells, and hint volumes never shape those cells.
// See: context/plans/in-progress/occluder-first-partition/index.md §Acceptance

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::test_fixtures::{Cells, MapText, compile_path, try_compile};
use crate::map_format::MapFormat;
use crate::streaming_hints::resolve_streaming_hints;

/// The committed maps that place streaming hints.
const HINTED_FIXTURES: [&str; 2] = [
    "sh-streaming-hinted-door.map",
    "lightmap-runtime-floor-verification.map",
];

/// Brush classes peeled before the BSP that shape SH streaming only.
const HINT_CLASSES: [&str; 3] = [
    "streaming_seam_volume",
    "stream_resident_volume",
    "stream_priority_region",
];

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/maps")
        .join(name)
}

fn parse(path: &Path) -> crate::map_data::MapData {
    crate::parse::parse_map_file(path, MapFormat::IdTech2).expect("fixture map parses")
}

/// `text` with every top-level entity of a hint class removed.
fn without_hint_entities(text: &str) -> String {
    let mut kept = String::new();
    let mut entity = String::new();
    let mut depth = 0_usize;
    for line in text.lines() {
        let trimmed = line.trim();
        if depth == 0 && trimmed != "{" {
            kept.push_str(line);
            kept.push('\n');
            continue;
        }
        entity.push_str(line);
        entity.push('\n');
        match trimmed {
            "{" => depth += 1,
            "}" => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            let is_hint = HINT_CLASSES
                .iter()
                .any(|class| entity.contains(&format!("\"classname\" \"{class}\"")));
            if !is_hint {
                kept.push_str(&entity);
            }
            entity.clear();
        }
    }
    kept
}

#[test]
fn hinted_fixtures_build_load_and_every_seam_matches_a_runtime_portal() {
    // Pin R27.
    for name in HINTED_FIXTURES {
        let path = fixture_path(name);
        let cells = Cells::of_map_data(parse(&path));
        let seams = &cells.map_data.streaming_seam_regions;
        assert!(!seams.is_empty(), "{name} places a seam");
        let (cell_section, portal_section, _) = cells.sections();

        // Each seam alone matches a runtime portal, and every portal it
        // matches joins two interior cells.
        let mut resolved = BTreeSet::new();
        for seam in seams {
            let hints = resolve_streaming_hints(
                std::slice::from_ref(seam),
                &[],
                &[],
                &cells.runtime.portals,
                &portal_section,
                &cell_section,
            )
            .unwrap_or_else(|error| panic!("{name}: {error:#}"));
            assert!(!hints.seam_portal_ids.is_empty());
            for &portal in &hints.seam_portal_ids {
                let portal = &cells.runtime.portals[portal as usize];
                assert!(cells.is_interior_cell(portal.front_leaf));
                assert!(cells.is_interior_cell(portal.back_leaf));
            }
            resolved.extend(hints.seam_portal_ids);
        }

        let compiled = compile_path(&path, &[]);
        let manifest = compiled
            .world
            .sh_stream_manifest()
            .unwrap_or_else(|| panic!("{name} loads with an SH streaming manifest"));
        let loaded: BTreeSet<u32> = manifest
            .seam_portals()
            .iter()
            .map(|seam| seam.portal_id)
            .collect();
        assert_eq!(
            loaded, resolved,
            "{name}: the loaded seams are the runtime portals each seam matches"
        );
        assert_eq!(compiled.world.portals.len(), cells.runtime.portals.len());
    }
}

#[test]
fn hinted_fixtures_cells_and_portals_match_the_maps_without_hints() {
    // Pin R29: hints are peeled before the BSP, so no hint plane reaches
    // classification, the tier or portal generation.
    for name in HINTED_FIXTURES {
        let path = fixture_path(name);
        let with = parse(&path);
        assert!(
            !with.streaming_seam_regions.is_empty()
                && !with.stream_resident_regions.is_empty()
                && !with.stream_priority_regions.is_empty(),
            "{name} places every hint class"
        );
        let dir = tempfile::tempdir().expect("temporary map directory");
        let stripped_path = dir.path().join(name);
        let text = std::fs::read_to_string(&path).expect("fixture map reads");
        std::fs::write(&stripped_path, without_hint_entities(&text)).expect("write stripped map");
        let without = parse(&stripped_path);
        assert!(
            without.streaming_seam_regions.is_empty()
                && without.stream_resident_regions.is_empty()
                && without.stream_priority_regions.is_empty(),
            "{name}: the stripped map places no hint"
        );
        assert_eq!(with.brush_volumes.len(), without.brush_volumes.len());

        let with = Cells::of_map_data(with);
        let without = Cells::of_map_data(without);
        let (view, bare) = (&with.runtime.cells, &without.runtime.cells);
        assert_eq!(view.tree.nodes.len(), bare.tree.nodes.len(), "{name}");
        assert_eq!(view.tree.leaves.len(), bare.tree.leaves.len(), "{name}");
        for (cell, bare_cell) in view.tree.leaves.iter().zip(&bare.tree.leaves) {
            assert_eq!(cell.is_solid, bare_cell.is_solid);
            assert_eq!(cell.bounds.min, bare_cell.bounds.min);
            assert_eq!(cell.bounds.max, bare_cell.bounds.max);
            assert_eq!(cell.defining_planes, bare_cell.defining_planes);
        }
        assert_eq!(view.leaf_cell, bare.leaf_cell, "{name}");
        assert_eq!(view.exterior_cells, bare.exterior_cells, "{name}");
        assert_eq!(
            with.runtime.portals.len(),
            without.runtime.portals.len(),
            "{name}"
        );
        for (portal, bare_portal) in with.runtime.portals.iter().zip(&without.runtime.portals) {
            assert_eq!(
                (portal.front_leaf, portal.back_leaf),
                (bare_portal.front_leaf, bare_portal.back_leaf)
            );
            assert_eq!(portal.polygon, bare_portal.polygon);
        }
    }
}

#[test]
fn seam_across_an_exempt_wall_fails_the_build_by_name() {
    // A seam spanning an opening in a wall that classifies exempt and forms
    // no occluding cluster crosses no runtime portal: the hall is one cell.
    // Two wall pieces 40 u (1 m) wide stand clear of each other and of the
    // side walls; the seam straddles the gap between them.
    let mut map = MapText::new();
    map.hall([640.0, 640.0, 128.0])
        .solid([250.0, 320.0, 0.0], [290.0, 328.0, 128.0])
        .solid([350.0, 320.0, 0.0], [390.0, 328.0, 128.0])
        .brush_entity(
            "streaming_seam_volume",
            &[],
            &[([292.0, 316.0, 8.0], [348.0, 332.0, 120.0])],
        );
    let error = try_compile(&map).expect_err("a seam matching no runtime portal fails the build");
    let message = format!("{error:#}");
    assert!(
        message.contains("streaming_seam_volume at (")
            && message.contains("matches no runtime portal"),
        "the error names the seam and where it stands: {message}"
    );
}
