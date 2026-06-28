// Shared test helpers and fixtures.

use super::super::*;

pub(crate) fn eval_js<F, R>(src: &str, f: F) -> R
where
    F: for<'js> FnOnce(&Ctx<'js>, JsValue<'js>) -> R,
{
    let rt = rquickjs::Runtime::new().unwrap();
    let ctx = rquickjs::Context::full(&rt).unwrap();
    ctx.with(|jsctx| {
        let value: JsValue = jsctx.eval(src).unwrap();
        f(&jsctx, value)
    })
}

pub(crate) fn install_ui_theme_token_validator(lua: &mlua::Lua) {
    const THEME_SRC: &str = include_str!("../../../../../../sdk/lib/ui/theme.luau");
    let theme: mlua::Table = lua
        .load(THEME_SRC)
        .set_name("theme.luau")
        .eval()
        .expect("theme.luau must evaluate to a module table");
    let unwrap: mlua::Value = theme
        .get("__unwrapThemeToken")
        .expect("theme.luau must expose internal token validator");
    lua.globals()
        .set("__postretroUnwrapThemeToken", unwrap)
        .expect("install temporary token validator");
}

pub(crate) fn eval_lua<F, R>(src: &str, f: F) -> R
where
    F: FnOnce(LuaValue) -> R,
{
    let lua = mlua::Lua::new();
    let value: LuaValue = lua.load(src).eval().unwrap();
    f(value)
}

pub(crate) const JS_PLAYER_MOVEMENT: &str = r#"({
    canonicalName: "player",
    components: {
        movement: {
            capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
            ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
            air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
            fall: { terminalVelocity: 40.0 }
        }
    }
})"#;

pub(crate) const UI_ALL_KINDS_JS: &str = r#"({
    anchor: "center",
    offset: [10.0, -20.0],
    root: {
        kind: "vstack", gap: 4.0, padding: 8.0, align: "start",
        children: [
            { kind: "text", content: "hello", fontSize: 18.0, color: [1.0, 1.0, 1.0, 1.0] },
            { kind: "panel", fill: [0.1, 0.2, 0.3, 1.0],
              border: { texture: "ui/frame", slice: [8.0, 8.0, 8.0, 8.0], tint: [1.0, 1.0, 1.0, 1.0] } },
            { kind: "hstack", gap: 2.0, padding: 0.0, align: "center",
              children: [
                  { kind: "image", asset: "ui/logo", decorative: true },
                  { kind: "spacer", flexGrow: 1.0 }
              ] },
            { kind: "grid", gap: 1.0, padding: 3.0, align: "stretch", cols: 2,
              children: [ { kind: "image", asset: "ui/icon", decorative: true } ] }
        ]
    }
})"#;

/// Expected wire form. Mirrors `descriptor::tests::ALL_KINDS_JSON` but the two
/// images carry `decorative: true` — the bridge enforces the M13 G2 image
/// name-XOR-decorative precondition, so a bare-asset image is no longer valid
/// authored input through the live path.
pub(crate) const UI_ALL_KINDS_WIRE: &str = r#"{"anchor":"center","offset":[10.0,-20.0],"root":{"kind":"vstack","gap":4.0,"padding":8.0,"align":"start","children":[{"kind":"text","content":"hello","fontSize":18.0,"color":[1.0,1.0,1.0,1.0]},{"kind":"panel","fill":[0.1,0.2,0.3,1.0],"border":{"texture":"ui/frame","slice":[8.0,8.0,8.0,8.0],"tint":[1.0,1.0,1.0,1.0]}},{"kind":"hstack","gap":2.0,"padding":0.0,"align":"center","children":[{"kind":"image","asset":"ui/logo","decorative":true},{"kind":"spacer","flexGrow":1.0}]},{"kind":"grid","gap":1.0,"padding":3.0,"align":"stretch","cols":2,"children":[{"kind":"image","asset":"ui/icon","decorative":true}]}]}}"#;
