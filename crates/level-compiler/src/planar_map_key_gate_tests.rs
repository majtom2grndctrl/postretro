// Source-scan gate: no planar map key, worldspawn key or FGD property.
// See: context/lib/rendering_pipeline.md §7.3a · context/lib/build_pipeline.md §Custom FGD

use std::path::{Path, PathBuf};

/// Whether `text` names planar as a word of its own: "coplanar" does not
/// count, `planar`, `_planar` and `Planar` do.
fn names_planar(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.match_indices("planar").any(|(at, _)| {
        !lower[..at]
            .chars()
            .next_back()
            .is_some_and(|before| before.is_ascii_alphabetic())
    })
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .expect("a source directory")
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

// AC 4: selection is automatic and the presets are engine constants, so no
// map key, worldspawn key or FGD property names planar: the map parsers, the
// source-format adapters, the map data model and the shipped FGD.
#[test]
fn planar_fields_stay_out_of_map_keys() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = crate_dir.join("src");
    let mut files = Vec::new();
    files_under(&src.join("parse"), &mut files);
    files_under(&src.join("format"), &mut files);
    assert!(files.len() > 5, "the map parsers were found");
    files.extend([
        src.join("parse.rs"),
        src.join("map_data.rs"),
        src.join("map_format.rs"),
        crate_dir.join("../../sdk/TrenchBroom/postretro.fgd"),
    ]);
    for file in &files {
        let text = std::fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        for (index, line) in text.lines().enumerate() {
            assert!(
                !names_planar(line),
                "{}:{}: a planar map key: `{}`",
                file.display(),
                index + 1,
                line.trim()
            );
        }
    }
    assert!(names_planar("\"_planar_reflection\"") && !names_planar("coplanar"));
}
