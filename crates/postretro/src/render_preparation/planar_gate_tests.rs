// Source-scan gate: no planar field in the player options catalog.
// See: context/lib/rendering_pipeline.md §7.3a · context/lib/player_options.md §4

use std::path::{Path, PathBuf};

/// Whether `text` names planar as a word of its own: "coplanar" and the like
/// do not count, `planar`, `_planar` and `Planar` do.
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

// AC 4: the planar presets are engine constants, never player settings. No
// options module, option key, `options.*` slot, settings document, options
// menu, render profile or scripting options type names a planar field.
#[test]
fn planar_fields_stay_out_of_the_player_options_catalog() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = crate_dir.join("src");
    let mut files = Vec::new();
    files_under(&src.join("options"), &mut files);
    assert!(
        files.iter().any(|file| file.ends_with("graphics.rs")),
        "the options catalog was found"
    );
    files.extend([
        src.join("app/options_menu.rs"),
        src.join("startup/render_profile.rs"),
        crate_dir.join("../../sdk/types/postretro.d.ts"),
        crate_dir.join("../../sdk/types/postretro.d.luau"),
    ]);
    for file in &files {
        let text = std::fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        for (index, line) in text.lines().enumerate() {
            assert!(
                !names_planar(line),
                "{}:{}: a planar field in the player options catalog: `{}`",
                file.display(),
                index + 1,
                line.trim()
            );
        }
    }
    assert!(names_planar("planar_scale") && names_planar("\"_planar\""));
    assert!(!names_planar("coplanar faces"));
}
