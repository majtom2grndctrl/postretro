// Tests: `render.reflections.style` and `surfaceReflectivity` through mod
// init in both runtimes, and the reflection-probes scripting-surface example.
// See: context/lib/scripting.md §2 (Mod render profile, Fluid reflectivity)

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use postretro_foundation::data_descriptors::FluidDescriptor;
use postretro_test_log_capture::LogCapture;

use super::{run_mod_init_luau, run_mod_init_quickjs};
use crate::primitives_registry::PrimitiveRegistry;
use crate::quickjs::{QuickJsConfig, QuickJsSubsystem};
use crate::runtime::{
    ModBloomProfile, ModBloomResolution, ModManifestResult, ModReflectionProfile,
    ModReflectionStyle, ModRenderProfile,
};

const STYLE_WARNING: &str = "`render.reflections.style` must be `pixel` or `smooth`; using `pixel`";

fn quickjs() -> QuickJsSubsystem {
    QuickJsSubsystem::new(&PrimitiveRegistry::new(), &QuickJsConfig::default())
        .expect("QuickJS subsystem should initialize")
}

/// The manifests' render profiles, each runtime given its own spelling.
fn render_profiles(js_render: &str, luau_render: &str) -> (ModRenderProfile, ModRenderProfile) {
    let js_source = format!(
        "globalThis.__postretroModManifest = {{ name: 'RenderMod', id: 'render-mod', version: '1', render: {js_render} }};"
    );
    let luau_source = format!(
        "return {{ name = 'RenderMod', id = 'render-mod', version = '1', render = {luau_render} }}"
    );
    let js = run_mod_init_quickjs(&quickjs(), &js_source, "render-mod.js")
        .expect("a render profile never rejects the manifest");
    let luau = run_mod_init_luau(&[], &luau_source, "render-mod.luau", Path::new("."))
        .expect("a render profile never rejects the manifest");
    (js.render, luau.render)
}

fn reflections(style: ModReflectionStyle) -> ModRenderProfile {
    ModRenderProfile {
        reflections: ModReflectionProfile { style },
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

// Acceptance row 27: omitted resolves to pixel and `"smooth"` to smooth, in
// both runtimes, with no warning.
#[test]
fn reflection_style_omitted_is_pixel_and_smooth_is_smooth_in_both_runtimes() {
    let capture = LogCapture::start();
    assert_eq!(ModReflectionStyle::default(), ModReflectionStyle::Pixel);
    for (js_render, luau_render, expected) in [
        ("{}", "{}", ModReflectionStyle::Pixel),
        (
            "{ reflections: {} }",
            "{ reflections = {} }",
            ModReflectionStyle::Pixel,
        ),
        (
            "{ reflections: null }",
            "{ reflections = nil }",
            ModReflectionStyle::Pixel,
        ),
        (
            "{ reflections: { style: undefined } }",
            "{ reflections = { style = nil } }",
            ModReflectionStyle::Pixel,
        ),
        (
            "{ reflections: { style: 'pixel' } }",
            "{ reflections = { style = 'pixel' } }",
            ModReflectionStyle::Pixel,
        ),
        (
            "{ reflections: { style: 'smooth' } }",
            "{ reflections = { style = 'smooth' } }",
            ModReflectionStyle::Smooth,
        ),
    ] {
        let (js, luau) = render_profiles(js_render, luau_render);
        assert_eq!(js, reflections(expected), "{js_render}");
        assert_eq!(luau, reflections(expected), "{luau_render}");
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
    assert_eq!(js.render.reflections.style, ModReflectionStyle::Pixel);
    assert_eq!(luau.render.reflections.style, ModReflectionStyle::Pixel);
    assert!(warnings(&capture, "render").is_empty());
}

// Acceptance row 27: an unknown string or a wrong type warns once and
// resolves to pixel without rejecting the manifest; both runtimes word the
// warning identically.
#[test]
fn an_unknown_or_mistyped_reflection_style_warns_and_resolves_to_pixel() {
    for (js_render, luau_render) in [
        (
            "{ reflections: { style: 'glossy' } }",
            "{ reflections = { style = 'glossy' } }",
        ),
        (
            "{ reflections: { style: 'Smooth' } }",
            "{ reflections = { style = 'Smooth' } }",
        ),
        (
            "{ reflections: { style: 1 } }",
            "{ reflections = { style = 1 } }",
        ),
        (
            "{ reflections: { style: true } }",
            "{ reflections = { style = true } }",
        ),
        (
            "{ reflections: { style: ['smooth'] } }",
            "{ reflections = { style = { 'smooth' } } }",
        ),
    ] {
        let capture = LogCapture::start();
        let (js, luau) = render_profiles(js_render, luau_render);
        assert_eq!(js, ModRenderProfile::default(), "{js_render}");
        assert_eq!(luau, ModRenderProfile::default(), "{luau_render}");
        let warned = warnings(&capture, STYLE_WARNING);
        assert_eq!(warned.len(), 2, "{js_render}: one per runtime: {warned:?}");
        // Only the manifest scope (each runtime names its export) differs.
        let body = |message: &str| message.split_once(": ").map(|(_, body)| body.to_owned());
        assert_eq!(
            body(&warned[0]),
            body(&warned[1]),
            "the runtimes' diagnostics diverged"
        );
    }
    // A non-object `reflections` warns about the block and keeps pixel.
    let capture = LogCapture::start();
    let (js, luau) = render_profiles("{ reflections: 'smooth' }", "{ reflections = 'smooth' }");
    assert_eq!(js, ModRenderProfile::default());
    assert_eq!(luau, ModRenderProfile::default());
    assert_eq!(
        warnings(&capture, "`render.reflections` must be").len(),
        2,
        "one per runtime"
    );
}

// The reflections block parses apart from bloom: an absent or malformed
// bloom block keeps the authored style, and a malformed style keeps the
// authored bloom.
#[test]
fn reflection_style_parses_independently_of_the_bloom_block() {
    for (js_render, luau_render) in [
        (
            "{ reflections: { style: 'smooth' } }",
            "{ reflections = { style = 'smooth' } }",
        ),
        (
            "{ bloom: 7, reflections: { style: 'smooth' } }",
            "{ bloom = 7, reflections = { style = 'smooth' } }",
        ),
        (
            "{ bloom: null, reflections: { style: 'smooth' } }",
            "{ bloom = nil, reflections = { style = 'smooth' } }",
        ),
    ] {
        let (js, luau) = render_profiles(js_render, luau_render);
        assert_eq!(js, reflections(ModReflectionStyle::Smooth), "{js_render}");
        assert_eq!(
            luau,
            reflections(ModReflectionStyle::Smooth),
            "{luau_render}"
        );
    }
    let quarter = ModBloomProfile {
        resolution: ModBloomResolution::Quarter,
        pixelated: true,
    };
    let (js, luau) = render_profiles(
        "{ bloom: { resolution: 'quarter', pixelated: true }, reflections: { style: 'glossy' } }",
        "{ bloom = { resolution = 'quarter', pixelated = true }, reflections = { style = 'glossy' } }",
    );
    let expected = ModRenderProfile {
        bloom: quarter,
        reflections: ModReflectionProfile::default(),
    };
    assert_eq!(js, expected);
    assert_eq!(luau, expected);
}

// Drift guard: every style's authored spelling round-trips, and `ALL` holds
// every variant (an exhaustive match, so a new variant fails to compile).
#[test]
fn every_reflection_style_round_trips_its_authored_name() {
    for style in ModReflectionStyle::ALL {
        let listed = match style {
            ModReflectionStyle::Pixel => "pixel",
            ModReflectionStyle::Smooth => "smooth",
        };
        assert_eq!(style.authored_name(), listed);
        assert_eq!(ModReflectionStyle::from_authored(listed), Some(style));
    }
    assert_eq!(ModReflectionStyle::ALL.len(), 2);
    assert_eq!(ModReflectionStyle::from_authored(""), None);
}

// --- Acceptance row 29: the Scripting surface example ---------------------

/// The brief's TS example, verbatim.
const SURFACE_EXAMPLE_TS: &str = r#"
// Proposed design
export default defineMod({
  name: "neon-sump",
  id: "example.neon-sump",
  version: "1.0",
  render: {
    reflections: { style: "smooth" }, // optional; "pixel" (default) or "smooth"
  },
  fluids: [
    defineFluid({
      name: "water",
      viscosity: 2.0,
      viewTint: [0.1, 0.3, 0.4, 0.35],
      muffle: 0.8,
      surfaceOpacity: 0.6,
      surfaceReflectivity: 1, // optional; default 1 (water)
    }),
  ],
});
"#;

/// Its Luau mirror, with the same names.
const SURFACE_EXAMPLE_LUAU: &str = r#"
return defineMod({
  name = "neon-sump",
  id = "example.neon-sump",
  version = "1.0",
  render = {
    reflections = { style = "smooth" }, -- optional; "pixel" (default) or "smooth"
  },
  fluids = {
    defineFluid({
      name = "water",
      viscosity = 2.0,
      viewTint = { 0.1, 0.3, 0.4, 0.35 },
      muffle = 0.8,
      surfaceOpacity = 0.6,
      surfaceReflectivity = 1, -- optional; default 1 (water)
    }),
  },
})
"#;

/// Bundle a TS mod entry through the production `scripts-build` path.
fn bundle_ts(source: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "postretro-reflection-example-{}-{}",
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

fn surface_example_manifests() -> (ModManifestResult, ModManifestResult) {
    let js = run_mod_init_quickjs(
        &quickjs(),
        &bundle_ts(SURFACE_EXAMPLE_TS),
        "start-script.js",
    )
    .expect("the TS example is a valid manifest");
    let luau = run_mod_init_luau(
        &[],
        SURFACE_EXAMPLE_LUAU,
        "start-script.luau",
        Path::new("."),
    )
    .expect("the Luau mirror is a valid manifest");
    (js, luau)
}

#[test]
fn the_reflection_scripting_example_parses_identically_in_ts_and_luau() {
    let capture = LogCapture::start();
    let (js, luau) = surface_example_manifests();
    for manifest in [&js, &luau] {
        assert_eq!(manifest.render, reflections(ModReflectionStyle::Smooth));
        assert_eq!(
            manifest.fluids.descriptors(),
            [FluidDescriptor {
                name: "water".into(),
                viscosity: 2.0,
                view_tint: [0.1, 0.3, 0.4, 0.35],
                muffle: 0.8,
                surface_opacity: 0.6,
                surface_reflectivity: 1.0,
                enter_sound: None,
                exit_sound: None,
                fog_distance: None,
                fog_color: None,
                wave_period: None,
                wave_amplitude: FluidDescriptor::DEFAULT_WAVE_AMPLITUDE,
                wave_length: FluidDescriptor::default_wave_length(
                    FluidDescriptor::DEFAULT_WAVE_AMPLITUDE
                ),
            }]
        );
    }
    assert_eq!(js.fluids, luau.fluids);
    assert!(
        capture
            .records()
            .iter()
            .all(|record| record.level > log::Level::Warn),
        "the example warns nowhere: {:?}",
        capture.records()
    );
}
