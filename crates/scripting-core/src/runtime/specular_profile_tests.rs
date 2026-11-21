// Tests: `render.specular.style` through mod init in both runtimes.
// See: context/lib/scripting.md §2 (Mod render profile)

use std::path::Path;

use postretro_test_log_capture::LogCapture;

use super::{run_mod_init_luau, run_mod_init_quickjs};
use crate::primitives_registry::PrimitiveRegistry;
use crate::quickjs::{QuickJsConfig, QuickJsSubsystem};
use crate::runtime::{
    ModReflectionProfile, ModReflectionStyle, ModRenderProfile, ModSpecularProfile,
    ModSpecularStyle,
};

const STYLE_WARNING: &str = "`render.specular.style` must be `pixel` or `smooth`; using `pixel`";
const BLOCK_WARNING: &str = "`render.specular` must be";

fn quickjs() -> QuickJsSubsystem {
    QuickJsSubsystem::new(&PrimitiveRegistry::new(), &QuickJsConfig::default())
        .expect("QuickJS subsystem should initialize")
}

/// The manifests' render profiles, each runtime given its own spelling.
fn render_profiles(js_render: &str, luau_render: &str) -> (ModRenderProfile, ModRenderProfile) {
    let js_source = format!(
        "globalThis.__postretroModManifest = {{ name: 'SpecMod', id: 'spec-mod', version: '1', render: {js_render} }};"
    );
    let luau_source = format!(
        "return {{ name = 'SpecMod', id = 'spec-mod', version = '1', render = {luau_render} }}"
    );
    let js = run_mod_init_quickjs(&quickjs(), &js_source, "spec-mod.js")
        .expect("a render profile never rejects the manifest");
    let luau = run_mod_init_luau(&[], &luau_source, "spec-mod.luau", Path::new("."))
        .expect("a render profile never rejects the manifest");
    (js.render, luau.render)
}

fn specular(style: ModSpecularStyle) -> ModRenderProfile {
    ModRenderProfile {
        specular: ModSpecularProfile { style },
        ..ModRenderProfile::default()
    }
}

fn warnings(capture: &LogCapture, needle: &str) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| record.level == log::Level::Warn && record.message.contains(needle))
        .map(|record| record.message)
        .collect()
}

/// The diagnostic body after the manifest scope, which each runtime names
/// after its own export.
fn body(message: &str) -> Option<String> {
    message.split_once(": ").map(|(_, body)| body.to_owned())
}

// Each authored style reaches the profile; omitted resolves to pixel,
// in both runtimes, with no warning.
#[test]
fn specular_style_omitted_is_pixel_and_each_authored_style_is_kept_in_both_runtimes() {
    let capture = LogCapture::start();
    assert_eq!(ModSpecularStyle::default(), ModSpecularStyle::Pixel);
    for (js_render, luau_render, expected) in [
        ("{}", "{}", ModSpecularStyle::Pixel),
        (
            "{ specular: {} }",
            "{ specular = {} }",
            ModSpecularStyle::Pixel,
        ),
        (
            "{ specular: null }",
            "{ specular = nil }",
            ModSpecularStyle::Pixel,
        ),
        (
            "{ specular: undefined }",
            "{ specular = nil }",
            ModSpecularStyle::Pixel,
        ),
        (
            "{ specular: { style: undefined } }",
            "{ specular = { style = nil } }",
            ModSpecularStyle::Pixel,
        ),
        (
            "{ specular: { style: null } }",
            "{ specular = { style = nil } }",
            ModSpecularStyle::Pixel,
        ),
        (
            "{ specular: { style: 'pixel' } }",
            "{ specular = { style = 'pixel' } }",
            ModSpecularStyle::Pixel,
        ),
        (
            "{ specular: { style: 'smooth' } }",
            "{ specular = { style = 'smooth' } }",
            ModSpecularStyle::Smooth,
        ),
    ] {
        let (js, luau) = render_profiles(js_render, luau_render);
        assert_eq!(js, specular(expected), "{js_render}");
        assert_eq!(luau, specular(expected), "{luau_render}");
    }
    // No render block at all is the default profile, pixel style.
    let js = run_mod_init_quickjs(
        &quickjs(),
        "globalThis.__postretroModManifest = { name: 'M', id: 'm', version: '1' };",
        "m.js",
    )
    .unwrap();
    let luau = run_mod_init_luau(
        &[],
        "return { name = 'M', id = 'm', version = '1' }",
        "m.luau",
        Path::new("."),
    )
    .unwrap();
    assert_eq!(js.render.specular.style, ModSpecularStyle::Pixel);
    assert_eq!(luau.render.specular.style, ModSpecularStyle::Pixel);
    assert!(warnings(&capture, "render").is_empty());
}

// An unknown string or a wrong type warns once per runtime and
// resolves to pixel without rejecting the manifest; both runtimes word the
// warning identically. A non-object block warns about the block.
#[test]
fn a_malformed_specular_style_warns_and_resolves_to_pixel() {
    for (js_render, luau_render) in [
        (
            "{ specular: { style: 'glossy' } }",
            "{ specular = { style = 'glossy' } }",
        ),
        (
            "{ specular: { style: 'Smooth' } }",
            "{ specular = { style = 'Smooth' } }",
        ),
        (
            "{ specular: { style: '' } }",
            "{ specular = { style = '' } }",
        ),
        ("{ specular: { style: 1 } }", "{ specular = { style = 1 } }"),
        (
            "{ specular: { style: true } }",
            "{ specular = { style = true } }",
        ),
        (
            "{ specular: { style: ['smooth'] } }",
            "{ specular = { style = { 'smooth' } } }",
        ),
    ] {
        let capture = LogCapture::start();
        let (js, luau) = render_profiles(js_render, luau_render);
        assert_eq!(js, ModRenderProfile::default(), "{js_render}");
        assert_eq!(luau, ModRenderProfile::default(), "{luau_render}");
        let warned = warnings(&capture, STYLE_WARNING);
        assert_eq!(warned.len(), 2, "{js_render}: one per runtime: {warned:?}");
        assert_eq!(
            body(&warned[0]),
            body(&warned[1]),
            "the runtimes' diagnostics diverged"
        );
    }
    for (js_render, luau_render) in [
        ("{ specular: 'smooth' }", "{ specular = 'smooth' }"),
        ("{ specular: 1 }", "{ specular = 1 }"),
        ("{ specular: ['smooth'] }", "{ specular = true }"),
    ] {
        let capture = LogCapture::start();
        let (js, luau) = render_profiles(js_render, luau_render);
        assert_eq!(js, ModRenderProfile::default(), "{js_render}");
        assert_eq!(luau, ModRenderProfile::default(), "{luau_render}");
        assert_eq!(
            warnings(&capture, BLOCK_WARNING).len(),
            2,
            "{js_render}: one per runtime"
        );
        assert!(warnings(&capture, STYLE_WARNING).is_empty());
    }
}

// `render.specular` and `render.reflections` degrade independently. A
// malformed specular block or style keeps the authored reflection style, and
// a malformed reflections block or style keeps the authored specular style,
// each warning only about its own field.
#[test]
fn specular_and_reflections_degrade_independently_in_both_runtimes() {
    let smooth_reflections = ModRenderProfile {
        reflections: ModReflectionProfile {
            style: ModReflectionStyle::Smooth,
        },
        ..ModRenderProfile::default()
    };
    for (js_render, luau_render) in [
        (
            "{ reflections: { style: 'smooth' }, specular: { style: 'glossy' } }",
            "{ reflections = { style = 'smooth' }, specular = { style = 'glossy' } }",
        ),
        (
            "{ reflections: { style: 'smooth' }, specular: 7 }",
            "{ reflections = { style = 'smooth' }, specular = 7 }",
        ),
    ] {
        let capture = LogCapture::start();
        let (js, luau) = render_profiles(js_render, luau_render);
        assert_eq!(js, smooth_reflections, "{js_render}");
        assert_eq!(luau, smooth_reflections, "{luau_render}");
        assert!(warnings(&capture, "`render.reflections").is_empty());
        assert_eq!(warnings(&capture, "`render.specular").len(), 2);
    }
    for (js_render, luau_render) in [
        (
            "{ reflections: { style: 'glossy' }, specular: { style: 'smooth' } }",
            "{ reflections = { style = 'glossy' }, specular = { style = 'smooth' } }",
        ),
        (
            "{ reflections: 7, specular: { style: 'smooth' } }",
            "{ reflections = 7, specular = { style = 'smooth' } }",
        ),
    ] {
        let capture = LogCapture::start();
        let (js, luau) = render_profiles(js_render, luau_render);
        assert_eq!(js, specular(ModSpecularStyle::Smooth), "{js_render}");
        assert_eq!(luau, specular(ModSpecularStyle::Smooth), "{luau_render}");
        assert!(warnings(&capture, "`render.specular").is_empty());
        assert_eq!(warnings(&capture, "`render.reflections").len(), 2);
    }
    // Both authored at once: each keeps its own value.
    let (js, luau) = render_profiles(
        "{ reflections: { style: 'smooth' }, specular: { style: 'smooth' } }",
        "{ reflections = { style = 'smooth' }, specular = { style = 'smooth' } }",
    );
    let both = ModRenderProfile {
        specular: ModSpecularProfile {
            style: ModSpecularStyle::Smooth,
        },
        ..smooth_reflections
    };
    assert_eq!(js, both);
    assert_eq!(luau, both);
}

// Drift guard: every style's authored spelling round-trips, and `ALL` holds
// every variant (an exhaustive match, so a new variant fails to compile).
#[test]
fn every_specular_style_round_trips_its_authored_name() {
    for style in ModSpecularStyle::ALL {
        let listed = match style {
            ModSpecularStyle::Pixel => "pixel",
            ModSpecularStyle::Smooth => "smooth",
        };
        assert_eq!(style.authored_name(), listed);
        assert_eq!(ModSpecularStyle::from_authored(listed), Some(style));
    }
    assert_eq!(ModSpecularStyle::ALL.len(), 2);
    assert_eq!(ModSpecularStyle::from_authored(""), None);
}
