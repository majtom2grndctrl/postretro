// Tests: `fluids` manifest drains in both runtimes and their warn-and-drop policy.

use super::super::*;
use super::common::*;
use postretro_test_log_capture::LogCapture;

const SCOPE: &str = "mod manifest";

fn drain_js(src: &str) -> FluidRegistry {
    eval_js(src, |ctx, value| {
        let obj = rquickjs::Object::from_value(value).unwrap();
        drain_fluids_js(ctx, &obj, SCOPE).unwrap()
    })
}

/// [`drain_js`], also asserting the drain left no exception pending on the
/// context for later manifest reads to trip over.
fn drain_js_leaving_no_exception(src: &str) -> FluidRegistry {
    eval_js(src, |ctx, value| {
        let obj = rquickjs::Object::from_value(value).unwrap();
        let registry = drain_fluids_js(ctx, &obj, SCOPE).unwrap();
        assert!(!ctx.has_exception(), "the drain left an exception pending");
        registry
    })
}

fn drain_lua(src: &str) -> FluidRegistry {
    eval_lua(src, |value| {
        let LuaValue::Table(table) = value else {
            panic!("manifest is a table");
        };
        drain_fluids_lua(&table, SCOPE).unwrap()
    })
}

/// One fluid entry in JS object syntax; Luau syntax swaps `:` for ` =` and
/// `[...]` for `{...}`. An override replaces a default field's value; one for
/// any other key adds that field.
fn js_fluid(name: &str, overrides: &[(&str, &str)]) -> String {
    let defaults = [
        ("viscosity", "2.0"),
        ("viewTint", "[0.1, 0.3, 0.4, 0.35]"),
        ("muffle", "0.8"),
        ("surfaceOpacity", "0.6"),
        ("enterSound", "\"sfx/splash_in\""),
    ];
    let mut fields = vec![format!("name: \"{name}\"")];
    for (key, value) in defaults {
        let value = overrides
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(value, |(_, v)| *v);
        fields.push(format!("{key}: {value}"));
    }
    for (key, value) in overrides {
        if !defaults.iter().any(|(k, _)| k == key) {
            fields.push(format!("{key}: {value}"));
        }
    }
    format!("{{ {} }}", fields.join(", "))
}

fn lua_fluid(name: &str, overrides: &[(&str, &str)]) -> String {
    js_fluid(name, overrides)
        .replace(": ", " = ")
        .replace('[', "{")
        .replace(']', "}")
}

fn js_manifest(entries: &[String]) -> String {
    format!("({{ fluids: [{}] }})", entries.join(", "))
}

fn lua_manifest(entries: &[String]) -> String {
    format!("return {{ fluids = {{ {} }} }}", entries.join(", "))
}

/// Run the same entries through both drains; return both registries.
fn drain_both(entries: &[(&str, &[(&str, &str)])]) -> (FluidRegistry, FluidRegistry) {
    let js: Vec<String> = entries.iter().map(|(n, o)| js_fluid(n, o)).collect();
    let lua: Vec<String> = entries.iter().map(|(n, o)| lua_fluid(n, o)).collect();
    (drain_js(&js_manifest(&js)), drain_lua(&lua_manifest(&lua)))
}

fn warnings(capture: &LogCapture, needle: &str) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| record.level == log::Level::Warn && record.message.contains(needle))
        .map(|record| record.message)
        .collect()
}

#[test]
fn declared_fluids_index_by_declaration_order_in_both_runtimes() {
    let (js, lua) = drain_both(&[("water", &[]), ("sludge", &[("viscosity", "6.0")])]);
    assert_eq!(js, lua);
    assert_eq!(js.index_of("water"), Some(0));
    assert_eq!(js.index_of("sludge"), Some(1));
    let water = js.get(0).unwrap();
    assert_eq!(water.view_tint, [0.1, 0.3, 0.4, 0.35]);
    assert_eq!(water.enter_sound.as_deref(), Some("sfx/splash_in"));
    assert_eq!(water.exit_sound, None);
}

#[test]
fn absent_fluids_declare_none() {
    assert!(drain_js("({})").is_empty());
    assert!(drain_lua("return {}").is_empty());
}

// An out-of-range presentation field rejects that fluid only, naming the field.
#[test]
fn out_of_range_view_tint_muffle_or_surface_opacity_rejects_that_fluid_naming_the_field() {
    for (field, js_value) in [
        ("viewTint", "[0.1, 0.3, 1.4, 0.35]"),
        ("muffle", "1.2"),
        ("surfaceOpacity", "-0.25"),
    ] {
        let capture = LogCapture::start();
        let js = drain_js(&js_manifest(&[
            js_fluid("water", &[]),
            js_fluid("bad", &[(field, js_value)]),
            js_fluid("sludge", &[]),
        ]));
        let lua = drain_lua(&lua_manifest(&[
            lua_fluid("water", &[]),
            // `lua_fluid` swaps brackets itself; pass the JS spelling.
            lua_fluid("bad", &[(field, js_value)]),
            lua_fluid("sludge", &[]),
        ]));
        assert_eq!(js, lua);
        assert_eq!(js.names(), ["water", "sludge"], "{field}: the rest stands");
        let rejected = warnings(&capture, &format!("fluids[1].{field}"));
        assert_eq!(rejected.len(), 2, "one warning per runtime: {rejected:?}");
        assert_eq!(
            rejected[0], rejected[1],
            "the runtimes' diagnostics diverged"
        );
        assert!(
            rejected[0].contains("fluid 'bad' was rejected"),
            "{}",
            rejected[0]
        );
    }
}

// Negative viscosity rejects the fluid, naming the field.
#[test]
fn negative_viscosity_rejects_the_fluid_naming_the_field() {
    let capture = LogCapture::start();
    let (js, lua) = drain_both(&[("water", &[("viscosity", "-1.0")]), ("sludge", &[])]);
    assert_eq!(js, lua);
    assert_eq!(js.names(), ["sludge"]);
    assert_eq!(js.index_of("sludge"), Some(0));
    let rejected = warnings(&capture, "fluids[0].viscosity must be a finite value >= 0");
    assert_eq!(rejected.len(), 2, "{rejected:?}");
}

// A second declaration of a name is rejected; the first declaration stands.
#[test]
fn a_second_define_fluid_with_an_existing_name_is_rejected_and_the_first_stands() {
    let capture = LogCapture::start();
    let (js, lua) = drain_both(&[
        ("water", &[("viscosity", "2.0")]),
        ("sludge", &[]),
        ("water", &[("viscosity", "9.0")]),
    ]);
    assert_eq!(js, lua);
    assert_eq!(js.names(), ["water", "sludge"]);
    assert_eq!(
        js.get(0).unwrap().viscosity,
        2.0,
        "the first declaration stands"
    );
    let duplicate = warnings(
        &capture,
        "fluids[2].name duplicates fluid 'water' declared at fluids[0]; the first declaration stands",
    );
    assert_eq!(duplicate.len(), 2, "{duplicate:?}");
}

#[test]
fn a_malformed_fluid_entry_is_skipped_and_its_siblings_keep_declaration_order() {
    let capture = LogCapture::start();
    let js = drain_js(
        r#"({ fluids: [
        { name: "water", viscosity: 2, viewTint: [0, 0, 0, 0], muffle: 0, surfaceOpacity: 1 },
        { name: "bad", viscosity: 2, viewTint: [0, 0, 0], muffle: 0, surfaceOpacity: 1 },
        { name: "sludge", viscosity: Infinity, viewTint: [0, 0, 0, 0], muffle: 0, surfaceOpacity: 1 },
        { name: "lava", viscosity: 9, viewTint: [1, 0.2, 0, 0.8], muffle: 0.5, surfaceOpacity: 1, glow: 3 },
        { name: "oil", viscosity: 4, viewTint: [0, 0, 0, 0.9], muffle: 0.2, surfaceOpacity: 1 }
    ] })"#,
    );
    let lua = drain_lua(
        r#"return { fluids = {
        { name = "water", viscosity = 2, viewTint = {0, 0, 0, 0}, muffle = 0, surfaceOpacity = 1 },
        { name = "bad", viscosity = 2, viewTint = {0, 0, 0}, muffle = 0, surfaceOpacity = 1 },
        { name = "sludge", viscosity = math.huge, viewTint = {0, 0, 0, 0}, muffle = 0, surfaceOpacity = 1 },
        { name = "lava", viscosity = 9, viewTint = {1, 0.2, 0, 0.8}, muffle = 0.5, surfaceOpacity = 1, glow = 3 },
        { name = "oil", viscosity = 4, viewTint = {0, 0, 0, 0.9}, muffle = 0.2, surfaceOpacity = 1 }
    } }"#,
    );
    assert_eq!(js, lua);
    assert_eq!(js.names(), ["water", "oil"]);
    for index in 1..=3 {
        let skipped = warnings(
            &capture,
            &format!("fluids[{index}] is malformed and was skipped"),
        );
        assert_eq!(skipped.len(), 2, "entry {index}: {skipped:?}");
    }
}

#[test]
fn a_non_array_fluids_field_warns_and_declares_none() {
    let capture = LogCapture::start();
    assert!(drain_js("({ fluids: 3 })").is_empty());
    assert!(drain_lua("return { fluids = 3 }").is_empty());
    assert_eq!(warnings(&capture, "`fluids` must be an array").len(), 2);
}

// A hole in the list costs only its own slot, in both runtimes: the entries
// around it keep their declaration order and their diagnostic indices.
#[test]
fn a_hole_in_fluids_warns_for_that_entry_only_in_both_runtimes() {
    let capture = LogCapture::start();
    let water = js_fluid("water", &[]);
    let sludge = js_fluid("sludge", &[]);
    let js = drain_js(&format!("({{ fluids: [{water}, undefined, {sludge}] }})"));
    let js_sparse = drain_js(&format!("({{ fluids: [{water}, , {sludge}] }})"));
    let lua = drain_lua(&format!(
        "return {{ fluids = {{ {}, nil, {} }} }}",
        lua_fluid("water", &[]),
        lua_fluid("sludge", &[])
    ));
    assert_eq!(js, lua);
    assert_eq!(js_sparse, lua);
    assert_eq!(lua.names(), ["water", "sludge"]);
    let skipped = warnings(&capture, "fluids[1] is malformed and was skipped");
    assert_eq!(skipped.len(), 3, "one per drain: {skipped:?}");
    assert!(
        skipped.iter().all(|warning| *warning == skipped[0]),
        "the runtimes' diagnostics diverged: {skipped:?}"
    );
    assert!(
        warnings(&capture, "must be an array").is_empty(),
        "the field itself stands"
    );
}

// `fluids: {}` is an empty list in QuickJS, as an empty Luau table is.
#[test]
fn an_empty_fluids_object_declares_none_without_a_warning() {
    let capture = LogCapture::start();
    assert!(drain_js("({ fluids: {} })").is_empty());
    assert!(drain_js("({ fluids: Object.create(null) })").is_empty());
    assert!(drain_lua("return { fluids = {} }").is_empty());
    assert!(warnings(&capture, "fluids").is_empty());
}

// Review panel 3: the plain-object check read the mutable global `Object`, so
// a mod that replaced it turned `fluids: {}` into a rejected manifest.
#[test]
fn an_empty_fluids_object_declares_none_after_the_global_object_is_replaced() {
    let capture = LogCapture::start();
    for replaced in ["undefined", "{ prototype: null }"] {
        let registry = drain_js_leaving_no_exception(&format!(
            "(() => {{ const fluids = {{}}; globalThis.Object = {replaced}; return {{ fluids }}; }})()"
        ));
        assert!(registry.is_empty(), "Object = {replaced}");
    }
    assert!(warnings(&capture, "fluids").is_empty());
}

// Review panel 3: a proxy whose trap throws made the plain-object check reject
// the manifest. It warns and ignores the field, with no exception left pending.
#[test]
fn a_fluids_proxy_whose_trap_throws_warns_and_is_ignored() {
    for trap in ["getPrototypeOf", "ownKeys"] {
        let capture = LogCapture::start();
        let registry = drain_js_leaving_no_exception(&format!(
            "({{ fluids: new Proxy({{}}, {{ {trap}() {{ throw new Error('trap'); }} }}) }})"
        ));
        assert!(registry.is_empty(), "{trap}");
        let ignored = warnings(
            &capture,
            "mod manifest: `fluids` must be an array of fluids; ignoring the field",
        );
        assert_eq!(ignored.len(), 1, "{trap}: {ignored:?}");
    }
}

// Only a plain `{}` reads as an empty list: a function, Map or Date has no own
// keys either, but warns like a Luau non-table field.
#[test]
fn a_function_map_or_date_fluids_field_warns_like_a_non_array() {
    for value in ["() => []", "new Map()", "new Date(0)"] {
        let capture = LogCapture::start();
        assert!(drain_js(&format!("({{ fluids: {value} }})")).is_empty());
        let ignored = warnings(
            &capture,
            "mod manifest: `fluids` must be an array of fluids; ignoring the field",
        );
        assert_eq!(ignored.len(), 1, "{value}: {ignored:?}");
    }
    let capture = LogCapture::start();
    assert!(drain_lua("return { fluids = function() return {} end }").is_empty());
    assert_eq!(
        warnings(
            &capture,
            "mod manifest: `fluids` must be an array of fluids; ignoring the field"
        )
        .len(),
        1
    );
}

// Regression: a sparse `fluids` keyed far out warned once per hole and sized
// its buffer by the key, in both runtimes.
#[test]
fn a_fluids_array_past_4096_slots_warns_once_and_is_ignored_in_both_runtimes() {
    let capture = LogCapture::start();
    let water = js_fluid("water", &[]);
    let js = drain_js(&format!(
        "(() => {{ const fluids = []; fluids[1000000] = {water}; return {{ fluids }}; }})()"
    ));
    let lua = drain_lua(&format!(
        "return {{ fluids = {{ [1000000] = {} }} }}",
        lua_fluid("water", &[])
    ));
    let lua_huge_key = drain_lua(&format!(
        "return {{ fluids = {{ [2 ^ 53] = {} }} }}",
        lua_fluid("water", &[])
    ));
    // Regression: past `i32::MAX` QuickJS stores the length as a float64, and
    // reading it through `Array::len` panicked at mod-init before the cap.
    let js_float_index = drain_js(&format!(
        "(() => {{ const fluids = []; fluids[2 ** 31] = {water}; return {{ fluids }}; }})()"
    ));
    let js_float_length =
        drain_js("(() => { const fluids = []; fluids.length = 2 ** 31; return { fluids }; })()");
    assert!(js.is_empty() && lua.is_empty() && lua_huge_key.is_empty());
    assert!(js_float_index.is_empty() && js_float_length.is_empty());
    let oversized = warnings(
        &capture,
        "mod manifest: `fluids` exceeds 4096 array slots; ignoring the field",
    );
    assert_eq!(oversized.len(), 5, "one per drain: {oversized:?}");
    assert!(oversized.iter().all(|warning| *warning == oversized[0]));
    assert!(
        warnings(&capture, "is malformed").is_empty(),
        "no warning per hole"
    );

    drop(capture);

    // The last slot inside the bound still declares its fluid.
    let capture = LogCapture::start();
    let js = drain_js(&format!(
        "(() => {{ const fluids = []; fluids[4095] = {water}; return {{ fluids }}; }})()"
    ));
    let lua = drain_lua(&format!(
        "return {{ fluids = {{ [4096] = {} }} }}",
        lua_fluid("water", &[])
    ));
    assert_eq!(js, lua);
    assert_eq!(js.names(), ["water"]);
    assert!(warnings(&capture, "exceeds").is_empty());
}

// A missing or mistyped field names `fluids[i].<field>` in the fluid's own
// wording, never the generic reaction-descriptor message.
#[test]
fn a_missing_or_mistyped_fluid_field_names_the_entry_field() {
    for (js_entry, lua_entry, needle) in [
        (
            "{ name: \"water\", viewTint: [0, 0, 0, 0], muffle: 0, surfaceOpacity: 1 }",
            "{ name = \"water\", viewTint = {0, 0, 0, 0}, muffle = 0, surfaceOpacity = 1 }",
            "`fluids[0].viscosity` is required",
        ),
        (
            "{ name: \"water\", viscosity: \"thick\", viewTint: [0, 0, 0, 0], muffle: 0, surfaceOpacity: 1 }",
            "{ name = \"water\", viscosity = \"thick\", viewTint = {0, 0, 0, 0}, muffle = 0, surfaceOpacity = 1 }",
            "`fluids[0].viscosity` must be a number",
        ),
        (
            "{ name: \"water\", viscosity: 2, viewTint: [0, 0, 0, 0], muffle: 0, surfaceOpacity: 1, exitSound: 4 }",
            "{ name = \"water\", viscosity = 2, viewTint = {0, 0, 0, 0}, muffle = 0, surfaceOpacity = 1, exitSound = 4 }",
            "`fluids[0].exitSound` must be a string",
        ),
    ] {
        let capture = LogCapture::start();
        assert!(drain_js(&format!("({{ fluids: [{js_entry}] }})")).is_empty());
        assert!(drain_lua(&format!("return {{ fluids = {{ {lua_entry} }} }}")).is_empty());
        let skipped = warnings(
            &capture,
            &format!("mod manifest: fluids[0] is malformed and was skipped: {needle}"),
        );
        assert_eq!(skipped.len(), 2, "{needle}: {skipped:?}");
        assert_eq!(skipped[0], skipped[1], "the runtimes' diagnostics diverged");
        assert!(
            !skipped[0].contains("reaction descriptor"),
            "{}",
            skipped[0]
        );
    }
}

// --- Fog and wave fields ---------------------------------------------------

/// Drain `[water, bad, sludge]` in both runtimes, where `bad` carries the
/// given JS and Luau overrides; return both registries.
fn drain_with_bad(
    js_overrides: &[(&str, &str)],
    lua_overrides: &[(&str, &str)],
) -> (FluidRegistry, FluidRegistry) {
    let js = drain_js(&js_manifest(&[
        js_fluid("water", &[]),
        js_fluid("bad", js_overrides),
        js_fluid("sludge", &[]),
    ]));
    let lua = drain_lua(&lua_manifest(&[
        lua_fluid("water", &[]),
        lua_fluid("bad", lua_overrides),
        lua_fluid("sludge", &[]),
    ]));
    (js, lua)
}

/// The one fluid `overrides` declare, drained in both runtimes; both must
/// accept it and agree.
fn accepted_in_both(overrides: &[(&str, &str)]) -> FluidDescriptor {
    let capture = LogCapture::start();
    let (js, lua) = drain_both(&[("water", overrides)]);
    assert_eq!(js, lua, "{overrides:?}: the runtimes diverged");
    assert_eq!(
        js.names(),
        ["water"],
        "{overrides:?} is accepted: {:?}",
        warnings(&capture, "fluids[0]")
    );
    js.get(0).unwrap().clone()
}

// The brief's scripting-surface example, in TS and its Luau mirror.
#[test]
fn the_fog_and_wave_scripting_example_parses_identically_in_ts_and_luau() {
    let js = drain_js(
        r#"({ fluids: [{
        name: "water",
        viscosity: 2.0,
        viewTint: [0.1, 0.3, 0.4, 0.35],
        muffle: 0.8,
        surfaceOpacity: 0.75,
        fogDistance: 1.0,
        fogColor: [0.08, 0.2, 0.16],
        wavePeriod: 63,
        waveAmplitude: 6,
        waveLength: [48, 80],
    }] })"#,
    );
    let lua = drain_lua(
        r#"return { fluids = { {
        name = "water",
        viscosity = 2.0,
        viewTint = { 0.1, 0.3, 0.4, 0.35 },
        muffle = 0.8,
        surfaceOpacity = 0.75,
        fogDistance = 1.0,
        fogColor = { 0.08, 0.2, 0.16 },
        wavePeriod = 63,
        waveAmplitude = 6,
        waveLength = { 48, 80 },
    } } }"#,
    );
    assert_eq!(js, lua);
    assert_eq!(
        js.descriptors(),
        [FluidDescriptor {
            name: "water".into(),
            viscosity: 2.0,
            view_tint: [0.1, 0.3, 0.4, 0.35],
            muffle: 0.8,
            surface_opacity: 0.75,
            enter_sound: None,
            exit_sound: None,
            fog_distance: Some(1.0),
            fog_color: Some([0.08, 0.2, 0.16]),
            wave_period: Some(63.0),
            wave_amplitude: 6.0,
            wave_length: [48.0, 80.0],
            surface_reflectivity: 1.0,
        }]
    );
}

// Omitted: no fog, no wave, murk in `viewTint`'s rgb, amplitude 8, length
// [64, 80]. `fogColor` stays unset; readers resolve it against `viewTint`.
#[test]
fn omitted_fog_and_wave_fields_resolve_to_their_defaults_in_both_runtimes() {
    let water = accepted_in_both(&[]);
    assert_eq!(water.fog_distance, None, "no fog");
    assert_eq!(water.wave_period, None, "no wave");
    assert_eq!(water.fog_color, None);
    assert_eq!(water.resolved_fog_color(), [0.1, 0.3, 0.4]);
    assert_eq!(water.wave_amplitude, 8.0);
    assert_eq!(water.wave_length, [64.0, 80.0]);
}

// `null` reads as omitted, as it does for the optional sounds.
#[test]
fn null_fog_and_wave_fields_read_as_omitted() {
    let js = drain_js(&js_manifest(&[js_fluid(
        "water",
        &[
            ("fogDistance", "null"),
            ("fogColor", "null"),
            ("wavePeriod", "null"),
            ("waveAmplitude", "null"),
            ("waveLength", "null"),
        ],
    )]));
    assert_eq!(js, drain_js(&js_manifest(&[js_fluid("water", &[])])));
}

// Boundary values on the accepted side of each rule, in both runtimes.
#[test]
fn fog_and_wave_boundary_values_are_accepted_in_both_runtimes() {
    // `1e-45` rounds to the smallest positive f32 (a subnormal): the least
    // fog distance the f64 -> f32 conversion can yield above 0.
    let water = accepted_in_both(&[("fogDistance", "1e-45")]);
    assert_eq!(water.fog_distance, Some(f32::from_bits(1)));
    assert_eq!(
        accepted_in_both(&[("fogColor", "[0, 1, 0]")]).fog_color,
        Some([0.0, 1.0, 0.0])
    );
    assert_eq!(
        accepted_in_both(&[("fogColor", "[1, 0, 1]")]).fog_color,
        Some([1.0, 0.0, 1.0])
    );
    assert_eq!(
        accepted_in_both(&[("waveAmplitude", "0")]).wave_amplitude,
        0.0
    );
    assert_eq!(
        accepted_in_both(&[("wavePeriod", "6")]).wave_period,
        Some(6.0)
    );
    let limit = accepted_in_both(&[("waveAmplitude", "16"), ("waveLength", "128")]);
    assert_eq!(
        (limit.wave_amplitude, limit.wave_length),
        (16.0, [128.0, 128.0])
    );
    assert_eq!(
        accepted_in_both(&[("waveLength", "[64, 128]")]).wave_length,
        [64.0, 128.0]
    );
    // A minimum of exactly 8 x amplitude.
    assert_eq!(
        accepted_in_both(&[("waveAmplitude", "6.5"), ("waveLength", "[52, 80]")]).wave_length,
        [52.0, 80.0]
    );
    // Pin P12: the default amplitude 8 sets the minimum at 64.
    assert_eq!(
        accepted_in_both(&[("waveLength", "64")]).wave_length,
        [64.0, 64.0]
    );
    assert_eq!(
        accepted_in_both(&[("waveAmplitude", "6"), ("waveLength", "50")]).wave_length,
        [50.0, 50.0]
    );
}

// One number L means [L, L]; a pair is [min, max]; both runtimes agree. An
// omitted length follows the amplitude.
#[test]
fn wave_length_forms_and_defaults_resolve_identically_in_both_runtimes() {
    let single = accepted_in_both(&[("waveLength", "72")]);
    assert_eq!(single.wave_length, [72.0, 72.0]);
    let pair = accepted_in_both(&[("waveAmplitude", "6"), ("waveLength", "[48, 80]")]);
    assert_eq!(pair.wave_length, [48.0, 80.0]);
    let low = accepted_in_both(&[("waveAmplitude", "4")]);
    assert_eq!(low.wave_length, [48.0, 80.0]);
    let high = accepted_in_both(&[("waveAmplitude", "16")]);
    assert_eq!(high.wave_length, [128.0, 128.0]);
}

// Each invalid value rejects that fluid only, with one warning per runtime
// naming `fluids[1].<field>`; the runtimes' warnings match word for word.
#[test]
fn each_invalid_fog_or_wave_value_rejects_that_fluid_naming_the_field() {
    type Case<'a> = (&'a str, &'a [(&'a str, &'a str)]);
    let cases: &[Case] = &[
        ("fogDistance", &[("fogDistance", "0")]),
        ("fogDistance", &[("fogDistance", "-1")]),
        ("fogColor", &[("fogColor", "[0.5, 1.01, 0.5]")]),
        ("fogColor", &[("fogColor", "[-0.01, 0.5, 0.5]")]),
        ("fogColor", &[("fogColor", "[0.5, 0.5]")]),
        ("fogColor", &[("fogColor", "[0.5, 0.5, 0.5, 0.5]")]),
        ("fogColor", &[("fogColor", "0.5")]),
        ("wavePeriod", &[("wavePeriod", "5.99")]),
        ("waveAmplitude", &[("waveAmplitude", "-1")]),
        (
            "waveAmplitude",
            &[("waveAmplitude", "16.01"), ("waveLength", "128")],
        ),
        ("waveLength", &[("waveLength", "0")]),
        ("waveLength", &[("waveLength", "[80, 64]")]),
        ("waveLength", &[("waveLength", "[64, 128.01]")]),
        ("waveLength", &[("waveLength", "[64]")]),
        ("waveLength", &[("waveLength", "[48, 64, 80]")]),
        ("waveLength", &[("waveLength", "\"long\"")]),
        // Just below 8 x amplitude.
        (
            "waveLength",
            &[("waveAmplitude", "6.5"), ("waveLength", "[51.99, 80]")],
        ),
        // Pin P12: the default amplitude 8 binds the 8x rule.
        ("waveLength", &[("waveLength", "50")]),
        ("waveLength", &[("waveLength", "63.99")]),
    ];
    for &(field, overrides) in cases {
        let capture = LogCapture::start();
        let (js, lua) = drain_with_bad(overrides, overrides);
        assert_eq!(js, lua);
        assert_eq!(
            js.names(),
            ["water", "sludge"],
            "{overrides:?}: the rest stands"
        );
        let rejected = warnings(&capture, &format!("fluids[1].{field}"));
        assert_eq!(
            rejected.len(),
            2,
            "{overrides:?}: one per runtime: {rejected:?}"
        );
        assert_eq!(
            rejected[0], rejected[1],
            "the runtimes' diagnostics diverged"
        );
        assert_eq!(
            warnings(&capture, "fluids[1]").len(),
            2,
            "{overrides:?}: no other warning for the entry"
        );
    }
}

// Pin P5: an out-of-range amplitude names `waveAmplitude`, never the length
// the author did not write, whether or not a period is set.
#[test]
fn an_out_of_range_amplitude_with_a_defaulted_length_names_wave_amplitude() {
    for overrides in [
        &[("waveAmplitude", "30")][..],
        &[("waveAmplitude", "17")][..],
        &[("waveAmplitude", "17"), ("wavePeriod", "63")][..],
    ] {
        let capture = LogCapture::start();
        let (js, lua) = drain_with_bad(overrides, overrides);
        assert_eq!(js, lua);
        assert_eq!(js.names(), ["water", "sludge"]);
        let rejected = warnings(
            &capture,
            "fluids[1].waveAmplitude must be a finite value in [0, 16]",
        );
        assert_eq!(rejected.len(), 2, "{overrides:?}: {rejected:?}");
        assert!(
            warnings(&capture, "waveLength").is_empty(),
            "{overrides:?}: a defaulted length never appears in a warning"
        );
    }
}

// A non-finite number never reaches validation: script conversion rejects
// it, naming the entry and the field, identically in both runtimes.
#[test]
fn a_nan_fog_or_wave_value_rejects_that_fluid_naming_the_field() {
    for (field, js_value, lua_value, path) in [
        ("fogDistance", "NaN", "0/0", "fogDistance"),
        (
            "fogColor",
            "[0.5, NaN, 0.5]",
            "{0.5, 0/0, 0.5}",
            "fogColor[1]",
        ),
        ("wavePeriod", "NaN", "0/0", "wavePeriod"),
        ("waveAmplitude", "NaN", "0/0", "waveAmplitude"),
        ("waveLength", "NaN", "0/0", "waveLength"),
        ("waveLength", "[NaN, 80]", "{0/0, 80}", "waveLength[0]"),
    ] {
        let capture = LogCapture::start();
        let (js, lua) = drain_with_bad(&[(field, js_value)], &[(field, lua_value)]);
        assert_eq!(js, lua);
        assert_eq!(js.names(), ["water", "sludge"], "{field}: the rest stands");
        let rejected: Vec<String> = warnings(&capture, "fluids[1] is malformed and was skipped")
            .into_iter()
            .filter(|message| message.contains(&format!("non-finite number at `{path}`")))
            .collect();
        assert_eq!(rejected.len(), 2, "{field}: one per runtime: {rejected:?}");
        assert_eq!(
            rejected[0], rejected[1],
            "the runtimes' diagnostics diverged"
        );
    }
}

// --- Reflectivity -----------------------------------------------------------

// Acceptance row 28: an omitted `surfaceReflectivity` resolves to 1 (water);
// 0 and 50, the range's ends, are accepted in both runtimes.
#[test]
fn surface_reflectivity_defaults_to_water_and_accepts_zero_and_fifty() {
    assert_eq!(accepted_in_both(&[]).surface_reflectivity, 1.0);
    assert_eq!(
        accepted_in_both(&[("surfaceReflectivity", "null")]).surface_reflectivity,
        1.0
    );
    for (authored, expected) in [("0", 0.0), ("50", 50.0), ("2.5", 2.5), ("10", 10.0)] {
        assert_eq!(
            accepted_in_both(&[("surfaceReflectivity", authored)]).surface_reflectivity,
            expected,
            "{authored}"
        );
    }
}

// Acceptance row 28: −0.1 and 50.1 reject that fluid with one warning per
// runtime naming the field; the rest of the manifest stands.
#[test]
fn an_out_of_range_surface_reflectivity_rejects_that_fluid_naming_the_field() {
    for authored in ["-0.1", "50.1"] {
        let capture = LogCapture::start();
        let overrides = [("surfaceReflectivity", authored)];
        let (js, lua) = drain_with_bad(&overrides, &overrides);
        assert_eq!(js, lua);
        assert_eq!(
            js.names(),
            ["water", "sludge"],
            "{authored}: the rest stands"
        );
        let rejected = warnings(
            &capture,
            "fluids[1].surfaceReflectivity must be a finite value in [0, 50]",
        );
        assert_eq!(
            rejected.len(),
            2,
            "{authored}: one per runtime: {rejected:?}"
        );
        assert_eq!(
            rejected[0], rejected[1],
            "the runtimes' diagnostics diverged"
        );
        assert!(
            rejected[0].contains("fluid 'bad' was rejected"),
            "{}",
            rejected[0]
        );
        assert_eq!(warnings(&capture, "fluids[1]").len(), 2, "no other warning");
    }
}

// Acceptance row 28: NaN never reaches validation; script conversion rejects
// it, still naming the field, and the rest of the manifest stands. A string
// is malformed the same way.
#[test]
fn a_nan_or_mistyped_surface_reflectivity_rejects_that_fluid_naming_the_field() {
    for (js_value, lua_value, needle) in [
        ("NaN", "0/0", "non-finite number at `surfaceReflectivity`"),
        (
            "\"mirror\"",
            "\"mirror\"",
            "`fluids[1].surfaceReflectivity` must be a number",
        ),
    ] {
        let capture = LogCapture::start();
        let (js, lua) = drain_with_bad(
            &[("surfaceReflectivity", js_value)],
            &[("surfaceReflectivity", lua_value)],
        );
        assert_eq!(js, lua);
        assert_eq!(
            js.names(),
            ["water", "sludge"],
            "{js_value}: the rest stands"
        );
        let rejected: Vec<String> = warnings(&capture, "fluids[1] is malformed and was skipped")
            .into_iter()
            .filter(|message| message.contains(needle))
            .collect();
        assert_eq!(
            rejected.len(),
            2,
            "{js_value}: one per runtime: {rejected:?}"
        );
        assert_eq!(
            rejected[0], rejected[1],
            "the runtimes' diagnostics diverged"
        );
    }
}
