// Light and fog member builders in both runtimes: every verb emits the same
// step wire, unset animation fields omitted rather than null.
// See: context/lib/scripting.md §7 (Animation capabilities)
use super::*;

use crate::conv::{js_to_json, lua_to_json};
use crate::error::ScriptError;
use crate::primitive_adapters::{JsonValue, WorldQueryFilterInput};
use crate::primitives_registry::ContextScope;

const LIGHT_ID: u32 = 7;
const FOG_ID: u32 = 11;

/// A registry whose only primitive is a `worldQuery` answering one light and
/// one fog volume, so `getMapEntities` wraps real member handles.
fn member_registry() -> PrimitiveRegistry {
    let mut registry = PrimitiveRegistry::new();
    registry
        .register(
            "worldQuery",
            |filter: WorldQueryFilterInput| -> Result<JsonValue, ScriptError> {
                let position = serde_json::json!({ "x": 1, "y": 2, "z": 3 });
                Ok(JsonValue(match filter.component.as_str() {
                    "light" => serde_json::json!([{
                        "id": LIGHT_ID,
                        "position": position,
                        "tags": ["lamp"],
                        "isDynamic": true,
                    }]),
                    "fog_volume" => serde_json::json!([{
                        "id": FOG_ID,
                        "position": position,
                        "tags": ["mist"],
                    }]),
                    _ => serde_json::json!([]),
                }))
            },
        )
        .scope(ContextScope::Both)
        .param("filter", "WorldQueryFilter")
        .finish();
    registry
}

/// Run a TypeScript fixture that stores its result in
/// `globalThis.__memberSteps`, converting through the manifest converter.
fn quickjs_member_steps(source: &str) -> serde_json::Value {
    let fixture = TypeScriptFixture::new(source);
    let bundled = postretro_script_compiler::bundle_entry(fixture.entry())
        .expect("member builder fixture bundles through scripts-build");
    let subsys = QuickJsSubsystem::new(&member_registry(), &QuickJsConfig::default()).unwrap();
    subsys.definition_ctx().with(|ctx| {
        run_script::<()>(&ctx, &bundled, "member-builders.js").expect("quickjs fixture eval");
        let steps: rquickjs::Value = ctx.globals().get("__memberSteps").unwrap();
        js_to_json(&ctx, steps).expect("quickjs steps -> json")
    })
}

/// Run a Luau fixture returning its result, converting through the manifest
/// converter.
fn luau_member_steps(source: &str) -> serde_json::Value {
    let primitives: Vec<_> = member_registry().iter().cloned().collect();
    let lua = build_lua_state(
        &primitives,
        None,
        Some(Path::new(env!("CARGO_MANIFEST_DIR"))),
    )
    .expect("build luau member fixture state");
    let value: mlua::Value = lua
        .load(source)
        .set_name("member-builders.luau")
        .eval()
        .expect("luau fixture eval");
    lua_to_json(value).expect("luau steps -> json")
}

// Every light and fog verb, with the inputs that change which animation
// fields are present (`fade`'s finite `playCount`, `colorShift`'s color,
// `sweep`'s direction including a zero-length and a non-unit sample,
// `flicker`'s fractional period, a reversed `min`/`max`). The twins must emit
// byte-identical steps through the converters the manifest drains use.
#[test]
fn light_and_fog_member_builders_emit_byte_identical_steps_in_both_runtimes() {
    let ts = quickjs_member_steps(
        r#"
        import { getMapEntities } from "postretro";
        const light = getMapEntities("light")[0];
        const fog = getMapEntities("fog")[0];
        (globalThis as any).__memberSteps = {
          lightPulse: light.pulse({ min: 0.9, max: 0.1, periodMs: 1200 }),
          lightFade: light.fade({ from: 1, to: 0.25, periodMs: 500 }),
          lightFlicker: light.flicker({ min: 0.2, max: 1, rate: 3 }),
          lightColorShift: light.colorShift({
            values: [{ x: 1, y: 0, z: 0 }, { x: 0.5, y: 0.25, z: 1 }],
            periodMs: 2000,
          }),
          lightSweep: light.sweep({
            values: [{ x: 0, y: 0, z: 2 }, { x: 0, y: 0, z: 0 }, { x: 1, y: 1, z: 0 }],
            periodMs: 900,
          }),
          fogPulse: fog.pulse({ min: 0.1, max: 0.6, periodMs: 3000 }),
          fogFade: fog.fade({ from: 0.6, to: 0, periodMs: 750 }),
          fogFlicker: fog.flicker({ min: 0.3, max: 0.05, rate: 7 }),
          fogPulseSaturation: fog.pulseSaturation({ min: 0, max: 2, periodMs: 1600 }),
          fogFadeSaturation: fog.fadeSaturation({ from: 1, to: 0.5, periodMs: 400 }),
        };
        "#,
    );
    let luau = luau_member_steps(
        r#"
        local light = getMapEntities("light")[1]
        local fog = getMapEntities("fog")[1]
        return {
          lightPulse = light:pulse({ min = 0.9, max = 0.1, periodMs = 1200 }),
          lightFade = light:fade({ from = 1, to = 0.25, periodMs = 500 }),
          lightFlicker = light:flicker({ min = 0.2, max = 1, rate = 3 }),
          lightColorShift = light:colorShift({
            values = { { x = 1, y = 0, z = 0 }, { x = 0.5, y = 0.25, z = 1 } },
            periodMs = 2000,
          }),
          lightSweep = light:sweep({
            values = { { x = 0, y = 0, z = 2 }, { x = 0, y = 0, z = 0 }, { x = 1, y = 1, z = 0 } },
            periodMs = 900,
          }),
          fogPulse = fog:pulse({ min = 0.1, max = 0.6, periodMs = 3000 }),
          fogFade = fog:fade({ from = 0.6, to = 0, periodMs = 750 }),
          fogFlicker = fog:flicker({ min = 0.3, max = 0.05, rate = 7 }),
          fogPulseSaturation = fog:pulseSaturation({ min = 0, max = 2, periodMs = 1600 }),
          fogFadeSaturation = fog:fadeSaturation({ from = 1, to = 0.5, periodMs = 400 }),
        }
        "#,
    );

    assert_eq!(
        serde_json::to_vec(&ts).unwrap(),
        serde_json::to_vec(&luau).unwrap(),
        "TS and Luau member builder steps diverged:\n  ts:   {ts}\n  luau: {luau}"
    );

    // Each step carries exactly the fields its verb sets; nothing unset
    // appears, not even as null.
    for (verb, fields) in [
        ("lightPulse", &["brightness", "periodMs"][..]),
        ("lightFade", &["brightness", "periodMs", "playCount"][..]),
        ("lightFlicker", &["brightness", "periodMs"][..]),
        ("lightColorShift", &["color", "periodMs"][..]),
        ("lightSweep", &["direction", "periodMs"][..]),
        ("fogPulse", &["density", "periodMs"][..]),
        ("fogFade", &["density", "periodMs", "playCount"][..]),
        ("fogFlicker", &["density", "periodMs"][..]),
        ("fogPulseSaturation", &["periodMs", "saturation"][..]),
        (
            "fogFadeSaturation",
            &["periodMs", "playCount", "saturation"][..],
        ),
    ] {
        let (id, primitive) = if verb.starts_with("light") {
            (LIGHT_ID, "setLightAnimation")
        } else {
            (FOG_ID, "setFogAnimation")
        };
        let steps = ts[verb]
            .as_array()
            .unwrap_or_else(|| panic!("{verb} returns steps"));
        assert_eq!(steps.len(), 1, "{verb} returns one step");
        assert_eq!(steps[0]["id"], id, "{verb} bakes the member id");
        assert_eq!(steps[0]["primitive"], primitive, "{verb} primitive");
        let keys: Vec<&str> = steps[0]["args"]
            .as_object()
            .unwrap_or_else(|| panic!("{verb} args are an object"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, fields, "{verb} args carry exactly the fields it sets");
    }
}
