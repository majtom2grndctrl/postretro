// Tests: `render.ghosts` and `ghostStyles` through mod init in both runtimes,
// and the ghost-style scripting-surface example.
// See: context/lib/scripting.md §2 (Mod render profile, Ghost style declarations)

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use postretro_foundation::data_descriptors::GhostStyle;
use postretro_test_log_capture::LogCapture;

use super::{run_mod_init_luau, run_mod_init_quickjs};
use crate::primitives_registry::PrimitiveRegistry;
use crate::quickjs::{QuickJsConfig, QuickJsSubsystem};
use crate::runtime::{
    ModBloomProfile, ModBloomResolution, ModGhostTransparency, ModManifestResult,
    ModReflectionProfile, ModReflectionStyle, ModRenderProfile,
};

const GHOSTS_WARNING: &str = "`render.ghosts` must be `blend` or `dither`; using `blend`";

fn quickjs() -> QuickJsSubsystem {
    QuickJsSubsystem::new(&PrimitiveRegistry::new(), &QuickJsConfig::default())
        .expect("QuickJS subsystem should initialize")
}

/// Both runtimes' manifests, each given its own spelling of the extra fields.
fn manifests(js_fields: &str, luau_fields: &str) -> (ModManifestResult, ModManifestResult) {
    let js_source = format!(
        "globalThis.__postretroModManifest = {{ name: 'GhostMod', id: 'ghost-mod', version: '1', {js_fields} }};"
    );
    let luau_source =
        format!("return {{ name = 'GhostMod', id = 'ghost-mod', version = '1', {luau_fields} }}");
    let js = run_mod_init_quickjs(&quickjs(), &js_source, "ghost-mod.js")
        .expect("ghost presentation never rejects the manifest");
    let luau = run_mod_init_luau(&[], &luau_source, "ghost-mod.luau", Path::new("."))
        .expect("ghost presentation never rejects the manifest");
    (js, luau)
}

fn render_profiles(js_render: &str, luau_render: &str) -> (ModRenderProfile, ModRenderProfile) {
    let (js, luau) = manifests(
        &format!("render: {js_render}"),
        &format!("render = {luau_render}"),
    );
    (js.render, luau.render)
}

fn warnings(capture: &LogCapture, needle: &str) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| record.level == log::Level::Warn && record.message.contains(needle))
        .map(|record| record.message)
        .collect()
}

fn ghosts(mode: ModGhostTransparency) -> ModRenderProfile {
    ModRenderProfile {
        ghosts: mode,
        ..ModRenderProfile::default()
    }
}

// `render.ghosts` absent or `"blend"` is blend, `"dither"`
// is dither, in both runtimes, with no warning.
#[test]
fn render_ghosts_absent_or_blend_is_blend_and_dither_is_dither_in_both_runtimes() {
    let capture = LogCapture::start();
    assert_eq!(ModGhostTransparency::default(), ModGhostTransparency::Blend);
    for (js_render, luau_render, expected) in [
        ("{}", "{}", ModGhostTransparency::Blend),
        (
            "{ ghosts: null }",
            "{ ghosts = nil }",
            ModGhostTransparency::Blend,
        ),
        (
            "{ ghosts: undefined }",
            "{ ghosts = nil }",
            ModGhostTransparency::Blend,
        ),
        (
            "{ ghosts: 'blend' }",
            "{ ghosts = 'blend' }",
            ModGhostTransparency::Blend,
        ),
        (
            "{ ghosts: 'dither' }",
            "{ ghosts = 'dither' }",
            ModGhostTransparency::Dither,
        ),
    ] {
        let (js, luau) = render_profiles(js_render, luau_render);
        assert_eq!(js, ghosts(expected), "{js_render}");
        assert_eq!(luau, ghosts(expected), "{luau_render}");
    }
    let (js, luau) = manifests("", "");
    assert_eq!(js.render.ghosts, ModGhostTransparency::Blend);
    assert_eq!(luau.render.ghosts, ModGhostTransparency::Blend);
    assert!(warnings(&capture, "render").is_empty());
}

// A malformed `render.ghosts` warns and uses blend,
// leaving `bloom` and `reflections` as authored; both runtimes judge it
// alike, word for word.
#[test]
fn a_malformed_render_ghosts_warns_uses_blend_and_keeps_bloom_and_reflections() {
    let authored = ModRenderProfile {
        bloom: ModBloomProfile {
            resolution: ModBloomResolution::Quarter,
            pixelated: true,
        },
        reflections: ModReflectionProfile {
            style: ModReflectionStyle::Smooth,
        },
        ghosts: ModGhostTransparency::Blend,
    };
    for (js_ghosts, luau_ghosts) in [
        ("'stipple'", "'stipple'"),
        ("'Dither'", "'Dither'"),
        ("''", "''"),
        ("1", "1"),
        ("true", "true"),
        ("['dither']", "{ 'dither' }"),
        ("{ mode: 'dither' }", "{ mode = 'dither' }"),
    ] {
        let capture = LogCapture::start();
        let (js, luau) = render_profiles(
            &format!(
                "{{ bloom: {{ resolution: 'quarter', pixelated: true }}, reflections: {{ style: 'smooth' }}, ghosts: {js_ghosts} }}"
            ),
            &format!(
                "{{ bloom = {{ resolution = 'quarter', pixelated = true }}, reflections = {{ style = 'smooth' }}, ghosts = {luau_ghosts} }}"
            ),
        );
        assert_eq!(js, authored, "{js_ghosts}");
        assert_eq!(luau, authored, "{luau_ghosts}");
        let warned = warnings(&capture, GHOSTS_WARNING);
        assert_eq!(warned.len(), 2, "{js_ghosts}: one per runtime: {warned:?}");
        // Only the manifest scope (each runtime names its export) differs.
        let body = |message: &str| message.split_once(": ").map(|(_, body)| body.to_owned());
        assert_eq!(
            body(&warned[0]),
            body(&warned[1]),
            "the runtimes' diagnostics diverged"
        );
        assert_eq!(
            capture
                .records()
                .iter()
                .filter(|record| record.level == log::Level::Warn)
                .count(),
            2,
            "{js_ghosts}: bloom and reflections stay silent"
        );
    }
}

// `ghosts` parses apart from the other render fields: a malformed bloom or
// reflections block keeps the authored mode.
#[test]
fn render_ghosts_parses_independently_of_bloom_and_reflections() {
    let (js, luau) = render_profiles(
        "{ bloom: 7, reflections: { style: 'glossy' }, ghosts: 'dither' }",
        "{ bloom = 7, reflections = { style = 'glossy' }, ghosts = 'dither' }",
    );
    assert_eq!(js, ghosts(ModGhostTransparency::Dither));
    assert_eq!(luau, ghosts(ModGhostTransparency::Dither));
}

// Drift guard: every mode's authored spelling round-trips, and `ALL` holds
// every variant (an exhaustive match, so a new variant fails to compile).
#[test]
fn every_ghost_transparency_round_trips_its_authored_name() {
    for mode in ModGhostTransparency::ALL {
        let listed = match mode {
            ModGhostTransparency::Blend => "blend",
            ModGhostTransparency::Dither => "dither",
        };
        assert_eq!(mode.authored_name(), listed);
        assert_eq!(ModGhostTransparency::from_authored(listed), Some(mode));
    }
    assert_eq!(ModGhostTransparency::from_authored(""), None);
}

// `ghostStyles` is never a reason to reject mod init: a malformed list or
// entry warns, and the rest of the manifest commits in both runtimes.
#[test]
fn malformed_ghost_styles_never_reject_mod_init_in_either_runtime() {
    let capture = LogCapture::start();
    let (js, luau) = manifests(
        "ghostStyles: [{ name: 'echo', opacity: 0.4 }, { name: 'bad', audio: 'yes' }, 5], fluids: [{ name: 'water', viscosity: 2, viewTint: [0, 0, 0, 0], muffle: 0, surfaceOpacity: 1 }]",
        "ghostStyles = { { name = 'echo', opacity = 0.4 }, { name = 'bad', audio = 'yes' }, 5 }, fluids = { { name = 'water', viscosity = 2, viewTint = { 0, 0, 0, 0 }, muffle = 0, surfaceOpacity = 1 } }",
    );
    assert_eq!(js.ghost_styles, luau.ghost_styles);
    assert_eq!(js.ghost_styles.names(), ["echo"]);
    assert_eq!(
        js.fluids.names(),
        ["water"],
        "the rest of the manifest commits"
    );
    assert_eq!(warnings(&capture, "ghostStyles[1].audio").len(), 2);
    assert_eq!(warnings(&capture, "ghostStyles[2]").len(), 2);

    let (js, luau) = manifests("ghostStyles: 'echo'", "ghostStyles = 'echo'");
    assert!(js.ghost_styles.is_empty());
    assert!(luau.ghost_styles.is_empty());
}

// Resolver half: the committed table a link resolves
// against gives a declared name its style, an empty name the floor silently,
// and an undeclared name the floor plus the stable `[Ghost]` warning the
// install path logs once.
#[test]
fn a_committed_style_table_resolves_link_styles_and_words_the_undeclared_warning() {
    let capture = LogCapture::start();
    let (js, luau) = manifests(
        "ghostStyles: [{ name: 'echo', tint: [0.6, 0.8, 1.0], audio: true }]",
        "ghostStyles = { { name = 'echo', tint = { 0.6, 0.8, 1.0 }, audio = true } }",
    );
    for styles in [&js.ghost_styles, &luau.ghost_styles] {
        let (echo, warning) = styles.resolve("echo");
        assert_eq!(warning, None);
        assert_eq!(echo.tint, [0.6, 0.8, 1.0]);
        assert!(echo.audio);
        assert_eq!(styles.resolve(""), (GhostStyle::FLOOR, None));
        let (floor, warning) = styles.resolve("phantom");
        assert_eq!(floor, GhostStyle::FLOOR);
        warning.expect("an undeclared name warns").warn();
    }
    let warned = warnings(
        &capture,
        "[Ghost] ghost_link style `phantom` is not declared; using the floor style",
    );
    assert_eq!(warned.len(), 2, "one per resolution: {warned:?}");
}

// --- The brief's scripting-surface example ----------------------------------

const SURFACE_EXAMPLE_TS: &str = r#"
import { defineGhostStyle, defineMod } from "postretro";

export default defineMod({
  name: "Dreadgate", id: "dreadgate", version: "0.1.0",
  ghostStyles: [
    defineGhostStyle({ name: "echo", tint: [0.6, 0.8, 1.0], opacity: 0.45, saturation: 0.2,
      audio: true, impactAudio: false }),
    defineGhostStyle({ name: "veil" }),
  ],
  render: { ghosts: "dither" },
});
"#;

const SURFACE_EXAMPLE_LUAU: &str = r#"
return defineMod({
  name = "Dreadgate", id = "dreadgate", version = "0.1.0",
  ghostStyles = {
    defineGhostStyle({ name = "echo", tint = { 0.6, 0.8, 1.0 }, opacity = 0.45, saturation = 0.2,
      audio = true, impactAudio = false }),
    defineGhostStyle({ name = "veil" }),
  },
  render = { ghosts = "dither" },
})
"#;

/// Bundle a TS mod entry through the production `scripts-build` path.
fn bundle_ts(source: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "postretro-ghost-example-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).expect("fixture directory");
    let entry = directory.join("start-script.ts");
    fs::write(&entry, source).expect("write fixture");
    let entry = fs::canonicalize(&entry).expect("canonicalize fixture");
    let bundled = postretro_script_compiler::bundle_entry(&entry).expect("the example bundles");
    let _ = fs::remove_dir_all(&directory);
    bundled
}

// Regression: successful SWC bundling can emit unrelated diagnostics into parser capture.
#[test]
fn the_ghost_style_scripting_example_parses_identically_in_ts_and_luau() {
    let bundled = bundle_ts(SURFACE_EXAMPLE_TS);
    let capture = LogCapture::start();
    let js = run_mod_init_quickjs(&quickjs(), &bundled, "start-script.js")
        .expect("the TS example is a valid manifest");
    let luau = run_mod_init_luau(
        &[],
        SURFACE_EXAMPLE_LUAU,
        "start-script.luau",
        Path::new("."),
    )
    .expect("the Luau mirror is a valid manifest");
    for manifest in [&js, &luau] {
        assert_eq!(manifest.render, ghosts(ModGhostTransparency::Dither));
        assert_eq!(manifest.ghost_styles.names(), ["echo", "veil"]);
        assert_eq!(
            manifest.ghost_styles.get("echo"),
            Some(&GhostStyle {
                tint: [0.6, 0.8, 1.0],
                opacity: 0.45,
                saturation: 0.2,
                audio: true,
                impact_audio: false,
            })
        );
        assert_eq!(manifest.ghost_styles.get("veil"), Some(&GhostStyle::FLOOR));
    }
    assert_eq!(js.ghost_styles, luau.ghost_styles);
    assert!(
        capture
            .records()
            .iter()
            .all(|record| record.level > log::Level::Warn),
        "the example warns nowhere: {:?}",
        capture.records()
    );
}
