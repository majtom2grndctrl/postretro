// Source-scan gates on the planar layer: one frame encoder, no spike citations, dev-only scaffolding.
// See: context/lib/rendering_pipeline.md §7.3a · context/lib/development_guide.md §6.4

use std::path::{Path, PathBuf};

/// The workspace's `crates/` directory.
fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the renderer sits under crates/")
        .to_path_buf()
}

/// Every file under `dir` with one of `extensions`, build trees skipped.
fn source_files(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "target") {
                source_files(&path, extensions, out);
            }
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| extensions.contains(&ext))
        {
            out.push(path);
        }
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// `source` before its trailing inline `#[cfg(test)] mod tests` block, so a
/// production file's own tests do not count as production text.
fn production_text(source: &str) -> &str {
    source
        .find("#[cfg(test)]\nmod tests {")
        .map_or(source, |at| &source[..at])
}

/// The modules `mod.rs` text declares under `#[cfg(test)]`.
fn test_only_modules(mod_rs: &str) -> Vec<&str> {
    let lines: Vec<&str> = mod_rs.lines().collect();
    lines
        .windows(2)
        .filter(|pair| pair[0].trim() == "#[cfg(test)]")
        .filter_map(|pair| {
            pair[1]
                .trim()
                .strip_prefix("mod ")
                .and_then(|rest| rest.strip_suffix(';'))
        })
        .collect()
}

// AC 4: every planar pass source records on the frame's one encoder, single
// threaded (Tencent CN116778049B): no thread, no rayon, no render bundle and
// no second command encoder in the planar pass module's production files,
// the per-draw plane routing, the mirror's light records or its sprites.
#[test]
fn planar_recording_uses_only_the_frame_encoder() {
    let render = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/render");
    let planar = render.join("planar_reflection");
    let mod_rs = read(&planar.join("mod.rs"));
    let test_only = test_only_modules(&mod_rs);
    let mut sources = vec![
        render.join("fluid_surface/planar_routing.rs"),
        render.join("renderer_mirror_lights.rs"),
    ];
    let mut planar_files = Vec::new();
    source_files(&planar, &["rs"], &mut planar_files);
    for file in planar_files {
        let stem = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("a source file name");
        if !test_only.contains(&stem) {
            sources.push(file);
        }
    }
    for required in ["mod", "api", "record", "mirror_draws", "mirror_sprites"] {
        assert!(
            sources
                .iter()
                .any(|file| file.file_stem().is_some_and(|stem| stem == required)),
            "the scan covers planar_reflection/{required}.rs"
        );
    }
    for file in &sources {
        let source = read(file);
        let text = production_text(&source);
        for forbidden in [
            "std::thread",
            "thread::spawn",
            "rayon",
            "RenderBundle",
            "create_render_bundle_encoder",
            "create_command_encoder",
        ] {
            assert!(
                !text.contains(forbidden),
                "{}: planar recording stays on the frame encoder, found `{forbidden}`",
                file.display()
            );
        }
    }
}

// AC 4: no source comment in crates/ cites the spike contract; planar code
// cites context/lib instead.
#[test]
fn planar_sources_cite_no_spike_contract() {
    let needle = ["planar-water", "-spike"].concat();
    let mut files = Vec::new();
    source_files(&crates_dir(), &["rs", "wgsl"], &mut files);
    assert!(files.len() > 100, "the scan reads the workspace's sources");
    let citing: Vec<_> = files
        .iter()
        .filter(|file| read(file).contains(&needle))
        .map(|file| file.display().to_string())
        .collect();
    assert!(citing.is_empty(), "spike citations in: {citing:?}");
}

/// The parent module file declaring `file` and the declaration's attribute
/// lines, or `None` at a crate root. Follows `#[path]` declarations.
fn declaration(file: &Path) -> Option<(PathBuf, Vec<String>)> {
    let name = file.file_name()?.to_str()?;
    if name == "lib.rs" || name == "main.rs" {
        return None;
    }
    let dir = file.parent()?;
    let (module, owner_dir) = if name == "mod.rs" {
        (dir.file_name()?.to_str()?.to_owned(), dir.parent()?)
    } else {
        (file.file_stem()?.to_str()?.to_owned(), dir)
    };
    let mut candidates = vec![
        owner_dir.join("mod.rs"),
        owner_dir.join("lib.rs"),
        owner_dir.join("main.rs"),
    ];
    if let (Some(parent), Some(stem)) = (owner_dir.parent(), owner_dir.file_name()) {
        candidates.push(parent.join(format!("{}.rs", stem.to_str()?)));
    }
    let wanted = [format!("mod {module};"), format!("mod {module} {{")];
    let path_suffix = format!("{name}\"]");
    let mut siblings = Vec::new();
    source_files(dir, &["rs"], &mut siblings);
    siblings.retain(|sibling| sibling.parent() == Some(dir));
    candidates.extend(siblings);
    for candidate in candidates {
        let Ok(source) = std::fs::read_to_string(&candidate) else {
            continue;
        };
        let lines: Vec<&str> = source.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let line = line.trim();
            let declares = wanted.iter().any(|decl| {
                line.strip_suffix(decl.as_str())
                    .is_some_and(|vis| vis.is_empty() || vis.trim_end().starts_with("pub"))
            });
            let by_path = line.starts_with("#[path = \"")
                && (line.ends_with(&format!("\"{path_suffix}"))
                    || line.ends_with(&format!("/{path_suffix}")));
            if !declares && !by_path {
                continue;
            }
            // The attribute block around the declaration.
            let mut start = index;
            while start > 0 && lines[start - 1].trim().starts_with("#[") {
                start -= 1;
            }
            let mut end = index;
            while by_path && end + 1 < lines.len() && lines[end].trim().starts_with("#[") {
                end += 1;
            }
            let attrs = lines[start..=end]
                .iter()
                .map(|line| line.trim().to_owned())
                .collect();
            return Some((candidate, attrs));
        }
    }
    panic!("no declaration of {} found", file.display());
}

/// Whether `file` compiles only under dev-tools, capture or test: some
/// module on its path from the crate root carries such a `cfg`.
fn module_is_gated(file: &Path) -> bool {
    let mut current = file.to_path_buf();
    while let Some((parent, attrs)) = declaration(&current) {
        if attrs
            .iter()
            .any(|attr| attr.starts_with("#[cfg(") && gates(attr))
        {
            return true;
        }
        current = parent;
    }
    false
}

/// A `cfg` naming dev-tools, capture or test (a test build ships nothing).
fn gates(attr: &str) -> bool {
    attr.contains("feature = \"dev-tools\"")
        || attr.contains("feature = \"capture\"")
        || attr == "#[cfg(test)]"
        || attr.starts_with("#[cfg(any(test")
        || attr.starts_with("#[cfg(all(test")
}

// AC 4: the planar launch seeds (`POSTRETRO_PLANAR_*`), the diagnostics
// panel's planar section and every dev or capture planar control compile only
// under dev-tools or capture (or test, which ships nothing). A file naming
// one is gated as a module, or each mention sits under a gating `cfg` a few
// lines up.
#[test]
fn planar_seeds_and_panel_compile_only_under_dev_tools() {
    let seed = ["POSTRETRO_", "PLANAR_"].concat();
    let controls = [
        seed.as_str(),
        "planar_dev_",
        "planar_tuned_preset",
        "set_planar_preset_override",
        "PlanarDevState",
        "PlanarLaunchSeed",
        "launch_settings",
        "preset_override",
        "set_crop(",
        "CapturePlanarReflection",
    ];
    let render = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/render");
    for gated in [
        render.join("debug_ui/planar_reflection.rs"),
        render.join("planar_reflection/launch_settings.rs"),
        render.join("planar_reflection/tuning.rs"),
        crates_dir().join("postretro/src/capture/planar.rs"),
    ] {
        assert!(module_is_gated(&gated), "{} is not gated", gated.display());
    }
    // The resolver itself: production planar files are not gated, a
    // `#[path]` test module is.
    assert!(!module_is_gated(&render.join("planar_reflection/api.rs")));
    assert!(!module_is_gated(
        &crates_dir().join("postretro/src/frame_loop/planar.rs")
    ));
    assert!(module_is_gated(
        &crates_dir().join("postretro/src/frame_loop/planar_tests.rs")
    ));
    let panel = read(&render.join("debug_ui/planar_reflection.rs"));
    assert!(panel.contains("\"Planar reflection\"") && !panel.to_lowercase().contains("spike"));

    let mut files = Vec::new();
    source_files(&crates_dir(), &["rs"], &mut files);
    let mut seen_seed = false;
    for file in files
        .iter()
        .filter(|file| file.components().any(|c| c.as_os_str() == "src"))
    {
        let source = read(file);
        if !controls.iter().any(|control| source.contains(control)) {
            continue;
        }
        seen_seed |= source.contains(&seed) && file.ends_with("launch_settings.rs");
        if module_is_gated(file) {
            continue;
        }
        let lines: Vec<&str> = source.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !controls.iter().any(|control| line.contains(control)) {
                continue;
            }
            let under_cfg = lines[index.saturating_sub(4)..index]
                .iter()
                .any(|above| above.trim().starts_with("#[cfg(") && gates(above.trim()));
            assert!(
                under_cfg,
                "{}:{}: `{}` compiles without dev-tools or capture",
                file.display(),
                index + 1,
                line.trim()
            );
        }
    }
    assert!(seen_seed, "the launch seeds were found and scanned");
}
