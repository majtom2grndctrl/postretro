//! Sandboxed data-script evaluation for animated baked-light membership.
//!
//! The script compiler is the only build-side crate allowed to embed the
//! scripting VMs. `prl-build` passes a resolved light table in and consumes a
//! resolved sidecar out; it never links a VM or reinterprets script data.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result, anyhow, bail};
use mlua::{
    Compiler as LuaCompiler, Function as LuaFunction, Lua, Table as LuaTable, Value as LuaValue,
};
use postretro_level_format::light_membership::{
    LightAnimationSnapshot, LightComponentSnapshot, LightMembershipManifest, LightMembershipRecord,
    LightTable, LightTableLight,
};
use rquickjs::{
    CatchResultExt, Context as JsContext, Ctx as JsCtx, Function as JsFunction, IntoJs,
    Object as JsObject, Runtime as JsRuntime, Value as JsValue,
};
use serde_json::{Map as JsonMap, Value as JsonValue, json};

use crate::bundle_prelude;

const SDK_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdk/lib");

// Luau modules are intentionally embedded here rather than borrowed from
// scripting-core. The dependency direction is scripting-core -> script-compiler,
// so reusing that runtime module would form a cycle.
const WORLD_LUAU: &str = include_str!("../../../sdk/lib/world.luau");
const LIGHTS_LUAU: &str = include_str!("../../../sdk/lib/entities/lights.luau");
const FOG_VOLUMES_LUAU: &str = include_str!("../../../sdk/lib/entities/fog_volumes.luau");
const MOVERS_LUAU: &str = include_str!("../../../sdk/lib/entities/movers.luau");
const TRIGGERS_LUAU: &str = include_str!("../../../sdk/lib/entities/triggers.luau");
const KEYFRAMES_LUAU: &str = include_str!("../../../sdk/lib/util/keyframes.luau");
const EMITTERS_LUAU: &str = include_str!("../../../sdk/lib/entities/emitters.luau");
const DATA_SCRIPT_LUAU: &str = include_str!("../../../sdk/lib/data_script.luau");
const RUNTIME_LUAU: &str = include_str!("../../../sdk/lib/runtime.luau");
const GAME_STATE_LUAU: &str = include_str!("../../../sdk/lib/game_state.luau");
const BRAIN_LUAU: &str = include_str!("../../../sdk/lib/brain.luau");
const UI_REACTIONS_LUAU: &str = include_str!("../../../sdk/lib/ui/reactions.luau");
const UI_WIDGETS_LUAU: &str = include_str!("../../../sdk/lib/ui/widgets.luau");
const UI_LAYOUT_LUAU: &str = include_str!("../../../sdk/lib/ui/layout.luau");
const UI_TREE_LUAU: &str = include_str!("../../../sdk/lib/ui/tree.luau");
const UI_PRESENTATION_LUAU: &str = include_str!("../../../sdk/lib/ui/presentation.luau");
const UI_STATE_LUAU: &str = include_str!("../../../sdk/lib/ui/state.luau");
const UI_THEME_LUAU: &str = include_str!("../../../sdk/lib/ui/theme.luau");

const MAX_VIRTUAL_MODULE_COPY_DEPTH: usize = 32;

/// Evaluate a compiled data script against `light_table` and derive the
/// map-light membership sidecar. The script path selects QuickJS for `.ts` /
/// `.js` input and Luau for `.luau` input; callers pass the already compiled
/// bytes represented as UTF-8 source.
pub fn emit_light_membership_manifest(
    compiled_source: &str,
    script_path: &Path,
    mod_root: &Path,
    light_table: &LightTable,
) -> Result<LightMembershipManifest> {
    light_table.validate_version().map_err(|e| anyhow!(e))?;
    validate_light_table(light_table)?;

    let stubs = Rc::new(RefCell::new(BTreeSet::new()));
    let returned = match script_path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("luau") => evaluate_luau(
            compiled_source,
            script_path,
            mod_root,
            light_table,
            stubs.clone(),
        )?,
        _ => evaluate_quickjs(compiled_source, script_path, light_table, stubs.clone())?,
    };

    let records = collect_membership(&returned, light_table)?;
    let stubbed_primitives = stubs.borrow().iter().cloned().collect();
    Ok(LightMembershipManifest::new(records, stubbed_primitives))
}

fn validate_light_table(light_table: &LightTable) -> Result<()> {
    let mut indexes = BTreeSet::new();
    for light in &light_table.lights {
        if !indexes.insert(light.index) {
            bail!(
                "light table contains duplicate map-light index {}; stable script handle ids would be ambiguous",
                light.index
            );
        }
    }
    Ok(())
}

fn evaluate_quickjs(
    source: &str,
    script_path: &Path,
    light_table: &LightTable,
    stubs: StubInventory,
) -> Result<JsonValue> {
    let runtime =
        JsRuntime::new().context("failed to create QuickJS manifest-evaluation runtime")?;
    let context = JsContext::full(&runtime)
        .context("failed to create QuickJS manifest-evaluation context")?;
    let source_name = script_path.display().to_string();
    let mut result = Err(anyhow!(
        "data script `{source_name}` did not return a manifest"
    ));

    context.with(|ctx| {
        let evaluation = (|| -> Result<JsonValue> {
            install_js_determinism(&ctx)?;
            install_js_game_state(&ctx)?;
            install_js_primitives(&ctx, light_table, stubs.clone())?;

            let prelude = bundle_prelude(Path::new(SDK_ROOT))
                .context("failed to assemble TypeScript SDK prelude for manifest evaluation")?;
            ctx.eval::<(), _>(prelude)
                .catch(&ctx)
                .map_err(|caught| anyhow!("SDK prelude threw: {caught}"))?;

            ctx.eval::<(), _>(source)
                .catch(&ctx)
                .map_err(|caught| anyhow!("data script `{source_name}` threw: {caught}"))?;

            let setup: JsFunction = ctx.globals().get("setupLevel").with_context(|| {
                format!("data script `{source_name}` did not export `setupLevel`")
            })?;
            let arg =
                JsObject::new(ctx.clone()).context("failed to allocate setupLevel context")?;
            let returned: JsValue = setup.call((arg,)).catch(&ctx).map_err(|caught| {
                anyhow!("data script `{source_name}` setupLevel threw: {caught}")
            })?;
            js_to_json(returned).context("setupLevel returned a non-manifest value")
        })();
        result = evaluation;
    });
    result
}

fn evaluate_luau(
    source: &str,
    script_path: &Path,
    mod_root: &Path,
    light_table: &LightTable,
    stubs: StubInventory,
) -> Result<JsonValue> {
    let lua = Lua::new();
    let source_name = script_path.display().to_string();
    install_lua_denylist(&lua)
        .map_err(|error| anyhow!("failed to restrict Luau manifest-evaluation globals: {error}"))?;
    install_lua_determinism(&lua)
        .map_err(|error| anyhow!("failed to install deterministic Luau clock and RNG: {error}"))?;
    install_lua_game_state(&lua)
        .map_err(|error| anyhow!("failed to install Luau getGameState bridge: {error}"))?;
    install_lua_primitives(&lua, light_table, stubs).map_err(|error| {
        anyhow!("failed to install Luau manifest-evaluation primitives: {error}")
    })?;
    install_lua_prelude(&lua, mod_root)
        .map_err(|error| anyhow!("failed to assemble Luau SDK prelude: {error}"))?;
    lua.sandbox(true)
        .map_err(|error| anyhow!("failed to freeze Luau manifest-evaluation globals: {error}"))?;

    let bytecode = LuaCompiler::new()
        .compile(source)
        .map_err(|e| anyhow!("data script `{source_name}` threw: {e}"))?;
    lua.load(&bytecode)
        .set_name(&source_name)
        .set_mode(mlua::ChunkMode::Binary)
        .exec()
        .map_err(|e| anyhow!("data script `{source_name}` threw: {e}"))?;

    let setup: LuaFunction = lua.globals().get("setupLevel").map_err(|error| {
        anyhow!("data script `{source_name}` did not export `setupLevel`: {error}")
    })?;
    let arg = lua
        .create_table()
        .map_err(|error| anyhow!("failed to allocate setupLevel context: {error}"))?;
    let returned: LuaValue = setup
        .call(arg)
        .map_err(|e| anyhow!("data script `{source_name}` setupLevel threw: {e}"))?;
    lua_to_json(returned)
        .map_err(|error| anyhow!("setupLevel returned a non-manifest value: {error}"))
}

type StubInventory = Rc<RefCell<BTreeSet<String>>>;

#[derive(Clone, Copy)]
enum StubReturn {
    Null,
    False,
    Gravity,
}

impl<'js> IntoJs<'js> for StubReturn {
    fn into_js(self, ctx: &JsCtx<'js>) -> rquickjs::Result<JsValue<'js>> {
        match self {
            Self::Null => Ok(JsValue::new_null(ctx.clone())),
            Self::False => false.into_js(ctx),
            Self::Gravity => (-9.81_f64).into_js(ctx),
        }
    }
}

/// An owned JSON-compatible value returned through the QuickJS function
/// adapter. Keeping it owned lets rquickjs perform the lifetime-sensitive
/// conversion after the callback has returned.
struct JsJsonValue(JsonValue);

impl<'js> IntoJs<'js> for JsJsonValue {
    fn into_js(self, ctx: &JsCtx<'js>) -> rquickjs::Result<JsValue<'js>> {
        json_to_js(ctx, &self.0)
    }
}

const STUBBED_PRIMITIVES: &[(&str, StubReturn)] = &[
    ("entityExists", StubReturn::False),
    ("getEntityProperty", StubReturn::Null),
    ("setLightAnimation", StubReturn::Null),
    ("storeRead", StubReturn::Null),
    ("storeWrite", StubReturn::Null),
    ("worldGetGravity", StubReturn::Gravity),
    ("worldSetGravity", StubReturn::Null),
    // Runtime-only primitives occasionally appear in author setup helpers.
    // Keep them non-throwing so membership extraction can degrade cleanly.
    ("fireTick", StubReturn::Null),
    ("getPlayerTransform", StubReturn::Null),
    ("setEmitterRate", StubReturn::Null),
    ("setSpinRate", StubReturn::Null),
    ("setFogDensity", StubReturn::Null),
    ("setFogGlow", StubReturn::Null),
    ("setFogEdgeSoftness", StubReturn::Null),
    ("setFogFalloff", StubReturn::Null),
    ("setFogParams", StubReturn::Null),
    ("setFogAnimation", StubReturn::Null),
    ("moverStart", StubReturn::Null),
    ("moverStop", StubReturn::Null),
    ("moverReverse", StubReturn::Null),
    ("moverGoToPathNode", StubReturn::Null),
    ("applyDamage", StubReturn::Null),
    ("spawnFromSpawner", StubReturn::Null),
];

fn install_js_determinism(ctx: &JsCtx<'_>) -> Result<()> {
    ctx.eval::<(), _>(
        r#"
        (() => {
          const NativeDate = Date;
          function PinnedDate(...args) {
            if (new.target) {
              return Reflect.construct(NativeDate, args.length ? args : [0], new.target);
            }
            // Native Date is both callable and constructible. Its callable
            // form ignores arguments and returns the current instant as text.
            return new NativeDate(0).toString();
          }
          Object.setPrototypeOf(PinnedDate, NativeDate);
          PinnedDate.prototype = NativeDate.prototype;
          Object.defineProperty(PinnedDate, "now", { value: () => 0 });
          globalThis.Date = PinnedDate;
          let state = 0x6d2b79f5;
          Math.random = () => {
            state = (state * 1664525 + 1013904223) >>> 0;
            return state / 4294967296;
          };
        })();
        "#,
    )
    .context("failed to install deterministic QuickJS clock and RNG")?;
    Ok(())
}

fn install_js_game_state(ctx: &JsCtx<'_>) -> Result<()> {
    let bridge = game_state_refs_json();
    ctx.globals()
        .set("__postretroGameStateRefs", json_to_js(ctx, &bridge)?)
        .context("failed to install QuickJS getGameState bridge")?;
    Ok(())
}

fn install_js_primitives(
    ctx: &JsCtx<'_>,
    light_table: &LightTable,
    stubs: StubInventory,
) -> Result<()> {
    let globals = ctx.globals();
    for (name, returns) in STUBBED_PRIMITIVES {
        let primitive_name = (*name).to_string();
        let return_value = *returns;
        let stubs = stubs.clone();
        let f = JsFunction::new(
            ctx.clone(),
            move |_ctx: JsCtx<'_>| -> rquickjs::Result<StubReturn> {
                stubs.borrow_mut().insert(primitive_name.clone());
                Ok(return_value)
            },
        )?;
        globals.set(*name, f)?;
    }

    let light_table = light_table.clone();
    let stubs_for_query = stubs.clone();
    let f = JsFunction::new(
        ctx.clone(),
        move |ctx: JsCtx<'_>, filter: JsObject<'_>| -> rquickjs::Result<JsJsonValue> {
            let component: String = filter.get("component")?;
            let tag: Option<String> = filter.get("tag")?;
            query_world_json(&light_table, &component, tag.as_deref(), &stubs_for_query)
                .map(JsJsonValue)
                .map_err(|message| rquickjs::Exception::throw_message(&ctx, &message))
        },
    )?;
    globals.set("worldQuery", f)?;
    Ok(())
}

fn install_lua_denylist(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    for name in ["io", "package", "require", "dofile", "loadfile", "load"] {
        globals.set(name, LuaValue::Nil)?;
    }
    if let Ok(os) = globals.get::<LuaTable>("os") {
        for name in ["execute", "exit", "getenv", "date"] {
            os.set(name, LuaValue::Nil)?;
        }
    }
    Ok(())
}

fn install_lua_determinism(lua: &Lua) -> mlua::Result<()> {
    lua.load(
        r#"
        local state = 0x6d2b79f5
        local function nextRandom()
          state = (state * 1664525 + 1013904223) % 4294967296
          return state / 4294967296
        end
        math.random = function(lower, upper)
          local sample = nextRandom()
          if lower == nil then
            return sample
          end
          if upper == nil then
            upper = lower
            lower = 1
          end
          if type(lower) ~= "number" or type(upper) ~= "number"
              or lower % 1 ~= 0 or upper % 1 ~= 0 or lower > upper then
            error("bad argument(s) to 'random' (interval is empty or bounds are not integers)")
          end
          return lower + math.floor(sample * (upper - lower + 1))
        end
        os.time = function(...) return 0 end
        os.clock = function() return 0 end
        "#,
    )
    .exec()?;
    Ok(())
}

fn install_lua_game_state(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "__postretroGameStateRefs",
        json_to_lua(lua, &game_state_refs_json())?,
    )?;
    Ok(())
}

fn install_lua_primitives(
    lua: &Lua,
    light_table: &LightTable,
    stubs: StubInventory,
) -> mlua::Result<()> {
    let globals = lua.globals();
    for (name, returns) in STUBBED_PRIMITIVES {
        let primitive_name = (*name).to_string();
        let stubs = stubs.clone();
        let f = lua.create_function(move |_lua, _: mlua::MultiValue| {
            stubs.borrow_mut().insert(primitive_name.clone());
            match returns {
                StubReturn::Null => Ok(LuaValue::Nil),
                StubReturn::False => Ok(LuaValue::Boolean(false)),
                StubReturn::Gravity => Ok(LuaValue::Number(-9.81)),
            }
        })?;
        globals.set(*name, f)?;
    }

    let light_table = light_table.clone();
    let stubs_for_query = stubs.clone();
    let f = lua.create_function(move |lua, filter: LuaTable| {
        let component: String = filter.get("component")?;
        let tag: Option<String> = filter.get("tag")?;
        let value = query_world_json(&light_table, &component, tag.as_deref(), &stubs_for_query)
            .map_err(mlua::Error::RuntimeError)?;
        json_to_lua(lua, &value)
    })?;
    globals.set("worldQuery", f)?;
    Ok(())
}

/// Ordered Luau SDK construction. The wrapper bridges are visible only long
/// enough for `world.luau` to capture them, exactly like the runtime prelude.
fn install_lua_prelude(lua: &Lua, mod_root: &Path) -> mlua::Result<()> {
    let globals = lua.globals();

    let game_state = eval_lua_table(lua, GAME_STATE_LUAU, "sdk/lib/game_state.luau")?;
    copy_lua_fields(&globals, &game_state, &["getGameState"])?;

    let lights = eval_lua_table(lua, LIGHTS_LUAU, "sdk/lib/entities/lights.luau")?;
    globals.set(
        "wrapLightEntity",
        lights.get::<LuaValue>("wrapLightEntity")?,
    )?;

    let fog = eval_lua_table(lua, FOG_VOLUMES_LUAU, "sdk/lib/entities/fog_volumes.luau")?;
    globals.set(
        "wrapFogVolumeEntity",
        fog.get::<LuaValue>("wrapFogVolumeEntity")?,
    )?;

    let movers = eval_lua_table(lua, MOVERS_LUAU, "sdk/lib/entities/movers.luau")?;
    globals.set(
        "wrapMoverEntity",
        movers.get::<LuaValue>("wrapMoverEntity")?,
    )?;

    let triggers = eval_lua_table(lua, TRIGGERS_LUAU, "sdk/lib/entities/triggers.luau")?;
    globals.set(
        "wrapTriggerVolumeEntity",
        triggers.get::<LuaValue>("wrapTriggerVolumeEntity")?,
    )?;

    let world: LuaValue = lua.load(WORLD_LUAU).set_name("sdk/lib/world.luau").eval()?;
    globals.set("world", world.clone())?;
    for name in [
        "wrapLightEntity",
        "wrapFogVolumeEntity",
        "wrapMoverEntity",
        "wrapTriggerVolumeEntity",
    ] {
        globals.set(name, LuaValue::Nil)?;
    }

    let keyframes = eval_lua_table(lua, KEYFRAMES_LUAU, "sdk/lib/util/keyframes.luau")?;
    copy_lua_fields(&globals, &keyframes, &["timeline", "sequence"])?;

    let emitters = eval_lua_table(lua, EMITTERS_LUAU, "sdk/lib/entities/emitters.luau")?;
    copy_lua_fields(
        &globals,
        &emitters,
        &["emitter", "smokeEmitter", "sparkEmitter", "dustEmitter"],
    )?;

    let data = eval_lua_table(lua, DATA_SCRIPT_LUAU, "sdk/lib/data_script.luau")?;
    const DATA_FIELDS: &[&str] = &[
        "defineReaction",
        "onTriggerEvent",
        "damage",
        "addSlot",
        "enemies",
        "spawner",
        "armTrigger",
        "disarmTrigger",
        "scopeReactions",
        "defineEntity",
        "defineMod",
        "defineMapCatalog",
        "defineTriggerPool",
        "defineStore",
    ];
    copy_lua_fields(&globals, &data, DATA_FIELDS)?;

    // Keep the SDK's virtual-module construction in its runtime order. In
    // particular, widgets and layouts capture the temporary theme-token
    // validator before it is hidden from author code.
    let ui_reactions = eval_lua_table(lua, UI_REACTIONS_LUAU, "sdk/lib/ui/reactions.luau")?;
    let ui_theme = eval_lua_table(lua, UI_THEME_LUAU, "sdk/lib/ui/theme.luau")?;
    globals.set(
        "__postretroUnwrapThemeToken",
        ui_theme.get::<LuaValue>("__unwrapThemeToken")?,
    )?;
    let ui_widgets = eval_lua_table(lua, UI_WIDGETS_LUAU, "sdk/lib/ui/widgets.luau")?;
    let ui_layout = eval_lua_table(lua, UI_LAYOUT_LUAU, "sdk/lib/ui/layout.luau")?;
    globals.set("__postretroUnwrapThemeToken", LuaValue::Nil)?;
    let ui_tree = eval_lua_table(lua, UI_TREE_LUAU, "sdk/lib/ui/tree.luau")?;
    let ui_presentation =
        eval_lua_table(lua, UI_PRESENTATION_LUAU, "sdk/lib/ui/presentation.luau")?;
    let ui_state = eval_lua_table(lua, UI_STATE_LUAU, "sdk/lib/ui/state.luau")?;

    let runtime: LuaValue = lua
        .load(RUNTIME_LUAU)
        .set_name("sdk/lib/runtime.luau")
        .eval()?;
    globals.set("runtime", runtime.clone())?;

    let brain = eval_lua_table(lua, BRAIN_LUAU, "sdk/lib/brain.luau")?;
    copy_lua_fields(&globals, &brain, &["brain", "candidate", "state"])?;

    let root = lua.create_table()?;
    root.set("world", world)?;
    root.set("runtime", runtime)?;
    copy_lua_fields(&root, &game_state, &["getGameState"])?;
    copy_lua_fields(&root, &brain, &["brain", "candidate", "state"])?;
    copy_lua_fields(&root, &keyframes, &["timeline", "sequence"])?;
    copy_lua_fields(
        &root,
        &emitters,
        &["emitter", "smokeEmitter", "sparkEmitter", "dustEmitter"],
    )?;
    copy_lua_fields(&root, &data, DATA_FIELDS)?;
    let root = copy_readonly_lua_table(lua, root, 0)?;
    let ui = lua.create_table()?;
    copy_lua_fields(
        &ui,
        &ui_widgets,
        &[
            "Text", "Panel", "Image", "Spacer", "Button", "Slider", "Bar", "Announce",
        ],
    )?;
    copy_lua_fields(&ui, &ui_layout, &["VStack", "HStack", "Grid"])?;
    copy_lua_fields(&ui, &ui_tree, &["Tree", "defineUiTree"])?;
    copy_lua_fields(&ui, &ui_presentation, &["definePresentationTemplate"])?;
    copy_lua_fields(&ui, &data, &["present"])?;
    copy_lua_fields(
        &ui,
        &ui_state,
        &[
            "bindState",
            "stateEquals",
            "createLocalState",
            "ui",
            "Switch",
        ],
    )?;
    copy_lua_fields(
        &ui,
        &ui_reactions,
        &[
            "onStateCrossing",
            "playSound",
            "rumble",
            "flashScreen",
            "vignette",
            "screenShake",
            "showDialog",
            "openMenu",
            "closeDialog",
            "openTextEntry",
            "KEYBOARD_TREE",
            "CLOSE_DIALOG_ACTION",
            "EXIT_TO_DESKTOP_ACTION",
            "QUIT_TO_MENU_ACTION",
            "loadLevel",
            "restartLevel",
            "returnToFrontend",
            "updateState",
            "appendText",
            "backspaceText",
            "clearText",
        ],
    )?;
    copy_lua_fields(&ui, &game_state, &["getGameState"])?;
    copy_lua_fields(&ui, &ui_theme, &["defineTheme", "getDesignTokens"])?;
    let ui = copy_readonly_lua_table(lua, ui, 0)?;
    // Match scripting-core's runtime resolver: relative paths are rooted at
    // the mod, checked lexically for traversal, and evaluated in this state.
    // The ordinary data-script runtime does not enable dependency tracking,
    // so it likewise does not canonicalize each required file.
    let mod_root = mod_root.to_path_buf();
    let require = lua.create_function(move |lua, name: String| match name.as_str() {
        "postretro" => Ok(LuaValue::Table(root.clone())),
        "postretro/ui" => Ok(LuaValue::Table(ui.clone())),
        _ => evaluate_luau_required_module(lua, &mod_root, &name),
    })?;
    globals.set("require", require)?;
    Ok(())
}

fn evaluate_luau_required_module(
    lua: &Lua,
    mod_root: &Path,
    request: &str,
) -> mlua::Result<LuaValue> {
    let resolved =
        resolve_luau_require_path(mod_root, request).map_err(mlua::Error::RuntimeError)?;
    let source = std::fs::read_to_string(&resolved).map_err(|error| {
        mlua::Error::RuntimeError(format!(
            "require(`{request}`): failed to read `{}`: {error}",
            resolved.display()
        ))
    })?;
    let bytecode = LuaCompiler::new().compile(&source).map_err(|error| {
        mlua::Error::RuntimeError(format!("require(`{request}`): compile failed: {error}"))
    })?;
    lua.load(&bytecode)
        .set_name(resolved.to_string_lossy().as_ref())
        .set_mode(mlua::ChunkMode::Binary)
        .eval()
}

fn resolve_luau_require_path(
    mod_root: &Path,
    request: &str,
) -> std::result::Result<PathBuf, String> {
    let trimmed = request.trim();
    if trimmed.is_empty() {
        return Err("require: empty path".to_owned());
    }
    if trimmed.contains('\\') {
        return Err(format!(
            "require(`{request}`): backslashes are not permitted in require paths"
        ));
    }
    if trimmed.split('/').any(|segment| segment == "..") {
        return Err(format!(
            "require(`{request}`): `..` segments are not permitted (mod root escape)"
        ));
    }
    let stripped = trimmed.strip_prefix("./").unwrap_or(trimmed);
    let candidate = Path::new(stripped);
    if candidate.is_absolute() {
        return Err(format!(
            "require(`{request}`): absolute paths are not permitted"
        ));
    }
    if candidate
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!(
            "require(`{request}`): `..` segments are not permitted (mod root escape)"
        ));
    }
    let mut resolved = mod_root.join(candidate);
    if resolved.extension().is_none() {
        resolved.set_extension("luau");
    }
    Ok(resolved)
}

fn eval_lua_table(lua: &Lua, source: &str, name: &str) -> mlua::Result<LuaTable> {
    lua.load(source)
        .set_name(name)
        .eval::<LuaTable>()
        .map_err(|error| {
            mlua::Error::RuntimeError(format!(
                "failed to evaluate Luau SDK prelude `{name}`: {error}"
            ))
        })
}

fn copy_lua_fields(target: &LuaTable, source: &LuaTable, fields: &[&str]) -> mlua::Result<()> {
    for field in fields {
        target.set(*field, source.get::<LuaValue>(*field)?)?;
    }
    Ok(())
}

/// Copy an SDK virtual module into compiler-owned tables and recursively mark
/// every nested table read-only. This mirrors scripting-core's runtime module
/// registry without introducing the reverse dependency that the compiler/VM
/// boundary forbids.
fn copy_readonly_lua_value(lua: &Lua, value: LuaValue, depth: usize) -> mlua::Result<LuaValue> {
    match value {
        LuaValue::Table(table) => {
            copy_readonly_lua_table(lua, table, depth + 1).map(LuaValue::Table)
        }
        other => Ok(other),
    }
}

fn copy_readonly_lua_table(lua: &Lua, source: LuaTable, depth: usize) -> mlua::Result<LuaTable> {
    if depth > MAX_VIRTUAL_MODULE_COPY_DEPTH {
        return Err(mlua::Error::RuntimeError(format!(
            "virtual Luau module table nesting exceeds {MAX_VIRTUAL_MODULE_COPY_DEPTH} levels"
        )));
    }

    let table = lua.create_table()?;
    for pair in source.pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair?;
        let key = copy_readonly_lua_value(lua, key, depth)?;
        let value = copy_readonly_lua_value(lua, value, depth)?;
        table.set(key, value)?;
    }
    table.set_readonly(true);
    Ok(table)
}

fn game_state_refs_json() -> JsonValue {
    json!({
        "input": { "mode": { "slot": "input.mode" } },
        "player": {
            "ammo": { "slot": "player.ammo" },
            "ammoReserve": { "slot": "player.ammo_reserve" },
            "health": { "slot": "player.health" },
            "maxHealth": { "slot": "player.max_health" },
            "reloadActive": { "slot": "player.reload_active" },
            "reloadProgress": { "slot": "player.reload_progress" },
            "weaponCooldownMs": { "slot": "player.weapon_cooldown_ms" }
        },
        "screen": {
            "flash": { "slot": "screen.flash" },
            "shake": { "slot": "screen.shake" },
            "vignette": { "slot": "screen.vignette" }
        },
        "ui": { "textEntry": { "slot": "ui.text_entry" } }
    })
}

const WORLD_QUERY_COMPONENTS: &[&str] = &[
    "light",
    "transform",
    "emitter",
    "fog_volume",
    "kinematic_mover",
    "trigger_volume",
    "particle",
    "sprite_visual",
];

fn query_world_json(
    light_table: &LightTable,
    component: &str,
    tag: Option<&str>,
    stubs: &StubInventory,
) -> std::result::Result<JsonValue, String> {
    match component {
        "light" => Ok(JsonValue::Array(
            light_table
                .lights
                .iter()
                .filter(|light| match tag {
                    Some(tag) => light.tags.iter().any(|candidate| candidate == tag),
                    None => true,
                })
                .map(light_handle_json)
                .collect(),
        )),
        component if WORLD_QUERY_COMPONENTS.contains(&component) => {
            // The v1 compiler seam carries only map lights. Other valid
            // runtime component kinds degrade to an empty query, and the
            // inventory makes any branch-sensitive under-derivation visible.
            stubs.borrow_mut().insert(format!("worldQuery:{component}"));
            Ok(JsonValue::Array(Vec::new()))
        }
        other => Err(format!(
            "invalid argument: worldQuery: unknown component `{other}`; supported: {}",
            WORLD_QUERY_COMPONENTS
                .iter()
                .map(|component| format!("\"{component}\""))
                .collect::<Vec<_>>()
                .join(" | ")
        )),
    }
}

fn light_handle_json(light: &LightTableLight) -> JsonValue {
    json!({
        "id": light.index,
        "position": vec3_json(light.position),
        "tags": &light.tags,
        "isDynamic": light.is_dynamic,
        "component": component_json(&light.component),
    })
}

fn component_json(component: &LightComponentSnapshot) -> JsonValue {
    json!({
        "origin": vec3_json(component.origin),
        "lightType": &component.light_type,
        "intensity": component.intensity,
        "color": vec3_json(component.color),
        "falloffModel": &component.falloff_model,
        "falloffRange": component.falloff_range,
        "coneAngleInner": component.cone_angle_inner,
        "coneAngleOuter": component.cone_angle_outer,
        "coneDirection": component.cone_direction.map(vec3_json),
        "isDynamic": component.is_dynamic,
        "animation": component.animation.as_ref().map(animation_json),
    })
}

fn animation_json(animation: &LightAnimationSnapshot) -> JsonValue {
    json!({
        "periodMs": animation.period_ms,
        "phase": animation.phase,
        "playCount": animation.play_count,
        "startActive": animation.start_active,
        "brightness": &animation.brightness,
        "color": animation.color.as_ref().map(|values| values.iter().copied().map(vec3_json).collect::<Vec<_>>()),
        "direction": animation.direction.as_ref().map(|values| values.iter().copied().map(vec3_json).collect::<Vec<_>>()),
    })
}

fn vec3_json(value: [f32; 3]) -> JsonValue {
    json!({ "x": value[0], "y": value[1], "z": value[2] })
}

fn collect_membership(
    returned: &JsonValue,
    light_table: &LightTable,
) -> Result<Vec<LightMembershipRecord>> {
    let manifest = returned
        .as_object()
        .ok_or_else(|| anyhow!("setupLevel must return an object"))?;
    let reactions = match manifest.get("reactions") {
        Some(JsonValue::Array(reactions)) => reactions.as_slice(),
        // Luau has one empty-table literal for both arrays and objects. At
        // this typed manifest boundary an empty `reactions = {}` is the empty
        // dense array, matching the runtime descriptor parser.
        Some(JsonValue::Object(reactions)) if reactions.is_empty() => &[],
        Some(_) => bail!("setupLevel.reactions must be an array"),
        None => &[],
    };

    let lights_by_id: BTreeMap<u32, &LightTableLight> = light_table
        .lights
        .iter()
        .map(|light| (light.index, light))
        .collect();
    let mut records: BTreeMap<u32, LightMembershipRecord> = BTreeMap::new();

    for reaction in reactions {
        // Runtime manifest drains warn and skip malformed reaction siblings.
        // Membership extraction must inspect exactly that degraded subset or
        // it can reserve bake data for a reaction runtime discards.
        let Some(reaction) = reaction.as_object() else {
            continue;
        };
        if reaction.get("name").and_then(JsonValue::as_str).is_none() {
            continue;
        }
        // Runtime discriminator priority is progress, sequence, primitive.
        if reaction.contains_key("progress") {
            continue;
        }
        let level_load = reaction.get("name").and_then(JsonValue::as_str) == Some("levelLoad");
        let Some(sequence) = reaction.get("sequence") else {
            continue;
        };
        let sequence = match sequence {
            JsonValue::Array(sequence) => sequence.as_slice(),
            JsonValue::Object(sequence) if sequence.is_empty() => &[],
            _ => continue,
        };
        if !sequence.iter().all(runtime_sequence_step_shape_is_valid) {
            continue;
        }
        for step in sequence {
            let step = step
                .as_object()
                .expect("sequence shape was validated before membership collection");
            if step.get("primitive").and_then(JsonValue::as_str) != Some("setLightAnimation") {
                continue;
            }
            let Some(id) = step
                .get("id")
                .and_then(JsonValue::as_u64)
                .and_then(|id| u32::try_from(id).ok())
            else {
                // Runtime accepts dispatch sentinels as sequence targets, but
                // they do not identify a map light and reserve no bake slot.
                continue;
            };
            let light = lights_by_id.get(&id).ok_or_else(|| {
                anyhow!(
                    "setLightAnimation targets unknown light handle id {id}; the supplied light table has no matching map-light index"
                )
            })?;
            let record = records
                .entry(light.index)
                .or_insert_with(|| LightMembershipRecord {
                    index: light.index,
                    is_dynamic: light.is_dynamic,
                    start_active: None,
                    start_active_conflict: false,
                });

            if level_load {
                // Runtime `None` means the descriptor defaults to active. The
                // sidecar reserves null exclusively for "no levelLoad write".
                let next = step
                    .get("args")
                    .and_then(JsonValue::as_object)
                    .and_then(|args| args.get("startActive"))
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(true);
                if let Some(previous) = record.start_active
                    && previous != next
                {
                    record.start_active_conflict = true;
                }
                record.start_active = Some(next);
            }
        }
    }
    Ok(records.into_values().collect())
}

fn runtime_sequence_step_shape_is_valid(step: &JsonValue) -> bool {
    let Some(step) = step.as_object() else {
        return false;
    };
    let Some(primitive) = step.get("primitive").and_then(JsonValue::as_str) else {
        return false;
    };
    if primitive.is_empty() {
        return false;
    }

    match step.get("id") {
        Some(JsonValue::String(target)) => {
            matches!(target.as_str(), "@activators" | "@trigger")
                && !(target == "@activators" && matches!(primitive, "armTrigger" | "disarmTrigger"))
        }
        Some(value) => value
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .is_some(),
        None => false,
    }
}

fn json_to_js<'js>(ctx: &JsCtx<'js>, value: &JsonValue) -> rquickjs::Result<JsValue<'js>> {
    match value {
        JsonValue::Null => Ok(JsValue::new_null(ctx.clone())),
        JsonValue::Bool(value) => value.into_js(ctx),
        JsonValue::Number(value) => value.as_f64().unwrap_or(0.0).into_js(ctx),
        JsonValue::String(value) => value.as_str().into_js(ctx),
        JsonValue::Array(values) => {
            let array = rquickjs::Array::new(ctx.clone())?;
            for (index, value) in values.iter().enumerate() {
                array.set(index, json_to_js(ctx, value)?)?;
            }
            Ok(array.into_value())
        }
        JsonValue::Object(values) => {
            let object = JsObject::new(ctx.clone())?;
            for (key, value) in values {
                object.set(key.as_str(), json_to_js(ctx, value)?)?;
            }
            Ok(object.into_value())
        }
    }
}

fn js_to_json(value: JsValue<'_>) -> rquickjs::Result<JsonValue> {
    js_to_json_inner(value, 0)
}

fn js_to_json_inner(value: JsValue<'_>, depth: usize) -> rquickjs::Result<JsonValue> {
    if depth >= 64 {
        return Err(rquickjs::Error::new_from_js_message(
            "value",
            "JSON-compatible manifest",
            "maximum nesting depth exceeded",
        ));
    }
    if value.is_null() || value.is_undefined() {
        return Ok(JsonValue::Null);
    }
    if let Some(value) = value.as_bool() {
        return Ok(JsonValue::Bool(value));
    }
    if let Some(value) = value.as_int() {
        return Ok(JsonValue::from(value));
    }
    if let Some(value) = value.as_float() {
        if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0 {
            return Ok(if value >= 0.0 {
                JsonValue::from(value as u64)
            } else {
                JsonValue::from(value as i64)
            });
        }
        return Ok(serde_json::Number::from_f64(value)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null));
    }
    if let Some(value) = value.as_string() {
        return Ok(JsonValue::String(value.to_string()?));
    }
    if let Some(array) = value.as_array() {
        let mut values = Vec::with_capacity(array.len());
        for index in 0..array.len() {
            values.push(js_to_json_inner(array.get(index)?, depth + 1)?);
        }
        return Ok(JsonValue::Array(values));
    }
    if let Some(object) = value.as_object() {
        let mut values = JsonMap::new();
        for entry in object.props::<String, JsValue>() {
            let (key, value) = entry?;
            if !value.is_undefined() {
                values.insert(key, js_to_json_inner(value, depth + 1)?);
            }
        }
        return Ok(JsonValue::Object(values));
    }
    Ok(JsonValue::Null)
}

fn json_to_lua(lua: &Lua, value: &JsonValue) -> mlua::Result<LuaValue> {
    match value {
        JsonValue::Null => Ok(LuaValue::Nil),
        JsonValue::Bool(value) => Ok(LuaValue::Boolean(*value)),
        JsonValue::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(LuaValue::Integer(value))
            } else {
                Ok(LuaValue::Number(value.as_f64().unwrap_or(0.0)))
            }
        }
        JsonValue::String(value) => Ok(LuaValue::String(lua.create_string(value)?)),
        JsonValue::Array(values) => {
            let table = lua.create_table()?;
            for (index, value) in values.iter().enumerate() {
                table.set(index + 1, json_to_lua(lua, value)?)?;
            }
            Ok(LuaValue::Table(table))
        }
        JsonValue::Object(values) => {
            let table = lua.create_table()?;
            for (key, value) in values {
                table.set(key.as_str(), json_to_lua(lua, value)?)?;
            }
            Ok(LuaValue::Table(table))
        }
    }
}

fn lua_to_json(value: LuaValue) -> mlua::Result<JsonValue> {
    lua_to_json_inner(value, 0)
}

fn lua_to_json_inner(value: LuaValue, depth: usize) -> mlua::Result<JsonValue> {
    if depth >= 64 {
        return Err(mlua::Error::RuntimeError(
            "maximum manifest nesting depth exceeded".to_string(),
        ));
    }
    match value {
        LuaValue::Nil => Ok(JsonValue::Null),
        LuaValue::Boolean(value) => Ok(JsonValue::Bool(value)),
        LuaValue::Integer(value) => Ok(JsonValue::from(value)),
        LuaValue::Number(value) => {
            if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0 {
                Ok(if value >= 0.0 {
                    JsonValue::from(value as u64)
                } else {
                    JsonValue::from(value as i64)
                })
            } else {
                Ok(serde_json::Number::from_f64(value)
                    .map(JsonValue::Number)
                    .unwrap_or(JsonValue::Null))
            }
        }
        LuaValue::String(value) => Ok(JsonValue::String(value.to_str()?.to_string())),
        LuaValue::Table(table) => {
            let array_len = table.raw_len();
            let is_array = array_len > 0
                && table.clone().pairs::<LuaValue, LuaValue>().all(|entry| {
                    matches!(entry, Ok((LuaValue::Integer(index), _)) if index >= 1 && index as usize <= array_len)
                });
            if is_array {
                let mut values = Vec::with_capacity(array_len);
                for index in 1..=array_len {
                    values.push(lua_to_json_inner(table.get(index)?, depth + 1)?);
                }
                Ok(JsonValue::Array(values))
            } else {
                let mut values = JsonMap::new();
                for entry in table.pairs::<String, LuaValue>() {
                    let (key, value) = entry?;
                    values.insert(key, lua_to_json_inner(value, depth + 1)?);
                }
                Ok(JsonValue::Object(values))
            }
        }
        other => Err(mlua::Error::FromLuaConversionError {
            from: other.type_name(),
            to: "JSON-compatible manifest".to_string(),
            message: Some("unsupported Luau value".to_string()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(index: u32, tag: &str, dynamic: bool) -> LightTableLight {
        LightTableLight {
            index,
            tags: vec![tag.to_string()],
            position: [index as f32, 2.0, 3.0],
            is_dynamic: dynamic,
            component: LightComponentSnapshot {
                origin: [index as f32, 2.0, 3.0],
                light_type: "Point".to_string(),
                intensity: 1.0,
                color: [1.0, 0.5, 0.25],
                falloff_model: "InverseSquared".to_string(),
                falloff_range: 12.0,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: None,
                is_dynamic: dynamic,
                animated_slot: None,
                animation: None,
            },
        }
    }

    fn table() -> LightTable {
        LightTable::new(vec![light(2, "wave", false), light(9, "dynamic", true)])
    }

    #[test]
    fn quickjs_collects_static_and_dynamic_membership_from_light_handles() {
        let source = r#"
            function setupLevel() {
              const staticLights = world.query({ component: "light", tag: "wave" });
              const dynamicLights = world.query({ component: "light", tag: "dynamic" });
              return { reactions: [
                defineReaction("levelLoad", { sequence: [
                  ...staticLights[0].pulse({ min: 0.2, max: 1.0, periodMs: 1000 }),
                  ...dynamicLights[0].fade({ from: 0.0, to: 1.0, periodMs: 1000 }),
                ] }),
              ] };
            }
        "#;
        let manifest = emit_light_membership_manifest(
            source,
            Path::new("fixture.ts"),
            Path::new("."),
            &table(),
        )
        .expect("script evaluates");
        assert_eq!(manifest.lights.len(), 2);
        assert_eq!(manifest.lights[0].index, 2);
        assert_eq!(manifest.lights[0].start_active, Some(true));
        assert!(!manifest.lights[0].is_dynamic);
        assert_eq!(manifest.lights[1].index, 9);
        assert!(manifest.lights[1].is_dynamic);
    }

    #[test]
    fn level_load_start_active_filters_non_load_reactions_and_marks_conflicts() {
        let source = r#"
            function setupLevel() {
              const light = world.query({ component: "light", tag: "wave" })[0];
              return { reactions: [
                defineReaction("trigger", { sequence: [{ id: light.id, primitive: "setLightAnimation", args: { startActive: false } }] }),
                defineReaction("levelLoad", { sequence: [{ id: light.id, primitive: "setLightAnimation", args: { startActive: false } }] }),
                defineReaction("levelLoad", { sequence: [{ id: light.id, primitive: "setLightAnimation", args: { startActive: true } }] }),
              ] };
            }
        "#;
        let manifest = emit_light_membership_manifest(
            source,
            Path::new("fixture.ts"),
            Path::new("."),
            &table(),
        )
        .expect("script evaluates");
        assert_eq!(manifest.lights.len(), 1);
        assert_eq!(manifest.lights[0].start_active, Some(true));
        assert!(manifest.lights[0].start_active_conflict);
    }

    #[test]
    fn deterministic_time_and_rng_produce_identical_membership() {
        let source = r#"
            function setupLevel() {
              const pick = Math.random() < 1 && Date.now() === 0;
              const light = world.query({ component: "light", tag: pick ? "wave" : "dynamic" })[0];
              return { reactions: [defineReaction("levelLoad", { sequence: light.pulse({ min: 0, max: 1, periodMs: 1 }) })] };
            }
        "#;
        let first = emit_light_membership_manifest(
            source,
            Path::new("fixture.ts"),
            Path::new("."),
            &table(),
        )
        .expect("first evaluation");
        let second = emit_light_membership_manifest(
            source,
            Path::new("fixture.ts"),
            Path::new("."),
            &table(),
        )
        .expect("second evaluation");
        assert_eq!(first, second);
        assert_eq!(first.lights[0].index, 2);
    }

    #[test]
    fn deterministic_date_preserves_callable_and_construct_forms() {
        // Regression: replacing Date with a class pinned construction but made
        // the standard callable `Date()` form throw during setupLevel.
        let source = r#"
            function setupLevel() {
              const called = Date();
              const calledWithArgs = Date(1234);
              const constructed = new Date();
              const explicit = new Date(1234);
              const compatible = typeof called === "string"
                && called === calledWithArgs
                && called === new Date(0).toString()
                && constructed.getTime() === 0
                && explicit.getTime() === 1234
                && Date.now() === 0
                && Date.UTC(1970, 0, 1) === 0;
              const light = world.query({ component: "light", tag: compatible ? "wave" : "dynamic" })[0];
              return { reactions: [defineReaction("levelLoad", { sequence: light.pulse({ min: 0, max: 1, periodMs: 1 }) })] };
            }
        "#;
        let manifest = emit_light_membership_manifest(
            source,
            Path::new("fixture.ts"),
            Path::new("."),
            &table(),
        )
        .expect("callable and constructible deterministic Date evaluates");
        assert_eq!(manifest.lights[0].index, 2);
    }

    #[test]
    fn compiler_world_query_omits_internal_animated_slot_in_both_hosts() {
        // Regression: runtime queries exposed assigned compose slots while the
        // pre-bake compiler query could only expose null, allowing membership
        // branches to diverge between evaluation hosts.
        let mut light_table = table();
        light_table.lights[0].component.animated_slot = Some(4);

        let quickjs = r#"
            function setupLevel() {
              const light = world.query({ component: "light", tag: "wave" })[0];
              if (Object.hasOwn(light.component, "animatedSlot")) throw new Error("leaked animatedSlot");
              return { reactions: [defineReaction("levelLoad", { sequence: light.pulse({ min: 0, max: 1, periodMs: 1 }) })] };
            }
        "#;
        let luau = r#"
            function setupLevel(_)
              local light = world:query({ component = "light", tag = "wave" })[1]
              if light.component.animatedSlot ~= nil then error("leaked animatedSlot") end
              return { reactions = {
                defineReaction("levelLoad", { sequence = light:pulse({ min = 0, max = 1, periodMs = 1 }) }),
              } }
            end
        "#;

        let quickjs_manifest = emit_light_membership_manifest(
            quickjs,
            Path::new("fixture.ts"),
            Path::new("."),
            &light_table,
        )
        .expect("QuickJS query hides internal slot");
        let luau_manifest = emit_light_membership_manifest(
            luau,
            Path::new("fixture.luau"),
            Path::new("."),
            &light_table,
        )
        .expect("Luau query hides internal slot");
        assert_eq!(quickjs_manifest.lights, luau_manifest.lights);
        assert_eq!(quickjs_manifest.lights[0].index, 2);
    }

    #[test]
    fn throwing_script_reports_its_path() {
        let error = emit_light_membership_manifest(
            "function setupLevel() { throw new Error('bad setup'); }",
            Path::new("content/dev/bad-lights.ts"),
            Path::new("."),
            &table(),
        )
        .expect_err("throwing data script must fail the build");
        let message = error.to_string();
        assert!(message.contains("content/dev/bad-lights.ts"), "{message}");
        assert!(message.contains("bad setup"), "{message}");
    }

    #[test]
    fn runtime_primitive_stubs_are_recorded_without_failing_evaluation() {
        let source = r#"
            function setupLevel() {
              fireTick();
              const state = getGameState();
              if (!state.player.health.slot) throw new Error("missing state bridge");
              const light = world.query({ component: "light", tag: "wave" })[0];
              return { reactions: [defineReaction("levelLoad", { sequence: light.flicker({ min: 0, max: 1, rate: 4 }) })] };
            }
        "#;
        let manifest = emit_light_membership_manifest(
            source,
            Path::new("fixture.ts"),
            Path::new("."),
            &table(),
        )
        .expect("stubs must not throw");
        assert_eq!(manifest.stubbed_primitives, vec!["fireTick"]);
    }

    #[test]
    fn unavailable_world_queries_degrade_and_inventory_branch_sensitive_use() {
        let source = r#"
            function setupLevel() {
              const transforms = world.query({ component: "transform" });
              world.query({ component: "particle" });
              const light = world.query({ component: "light", tag: "wave" })[0];
              return { reactions: transforms.length === 0 ? [] : [
                defineReaction("levelLoad", { sequence: light.pulse({ min: 0, max: 1, periodMs: 1 }) }),
              ] };
            }
        "#;
        let manifest = emit_light_membership_manifest(
            source,
            Path::new("fixture.ts"),
            Path::new("."),
            &table(),
        )
        .expect("valid unavailable query kinds degrade");
        assert!(manifest.lights.is_empty());
        assert_eq!(
            manifest.stubbed_primitives,
            vec!["worldQuery:particle", "worldQuery:transform"]
        );
    }

    #[test]
    fn unknown_world_query_component_matches_runtime_error_contract() {
        let quickjs = emit_light_membership_manifest(
            "function setupLevel() { world.query({ component: 'decal' }); return {}; }",
            Path::new("fixture.ts"),
            Path::new("."),
            &table(),
        )
        .expect_err("unknown QuickJS component must throw")
        .to_string();
        assert!(
            quickjs.contains("invalid argument") && quickjs.contains("decal"),
            "{quickjs}"
        );

        let luau = emit_light_membership_manifest(
            "function setupLevel(_) world:query({ component = 'decal' }); return {} end",
            Path::new("fixture.luau"),
            Path::new("."),
            &table(),
        )
        .expect_err("unknown Luau component must throw")
        .to_string();
        assert!(
            luau.contains("invalid argument") && luau.contains("decal"),
            "{luau}"
        );
    }

    #[test]
    fn empty_luau_reaction_array_is_valid() {
        let manifest = emit_light_membership_manifest(
            "function setupLevel(_) return { reactions = {} } end",
            Path::new("fixture.luau"),
            Path::new("."),
            &table(),
        )
        .expect("empty Luau reaction table is the empty dense array");
        assert!(manifest.lights.is_empty());
    }

    #[test]
    fn malformed_reaction_siblings_degrade_identically_in_both_hosts() {
        // Regression: compile-time extraction aborted on shapes the runtime
        // manifest drain warns about and skips.
        let quickjs = r#"
            function setupLevel() {
              const good = world.query({ component: "light", tag: "wave" })[0];
              const discarded = world.query({ component: "light", tag: "dynamic" })[0];
              return { reactions: [
                null,
                { name: "bad-sequence", sequence: "not-an-array" },
                { name: "bad-step", sequence: [
                  { id: discarded.id, primitive: "setLightAnimation", args: {} },
                  null,
                ] },
                { name: "levelLoad", sequence: [
                  { id: good.id, primitive: "setLightAnimation", args: {} },
                ] },
              ] };
            }
        "#;
        let luau = r#"
            function setupLevel(_)
              local good = world:query({ component = "light", tag = "wave" })[1]
              local discarded = world:query({ component = "light", tag = "dynamic" })[1]
              return { reactions = {
                false,
                { name = "bad-sequence", sequence = "not-an-array" },
                { name = "bad-step", sequence = {
                  { id = discarded.id, primitive = "setLightAnimation", args = {} },
                  false,
                } },
                { name = "levelLoad", sequence = {
                  { id = good.id, primitive = "setLightAnimation", args = {} },
                } },
              } }
            end
        "#;

        let quickjs_manifest = emit_light_membership_manifest(
            quickjs,
            Path::new("fixture.ts"),
            Path::new("."),
            &table(),
        )
        .expect("QuickJS malformed siblings degrade");
        let luau_manifest = emit_light_membership_manifest(
            luau,
            Path::new("fixture.luau"),
            Path::new("."),
            &table(),
        )
        .expect("Luau malformed siblings degrade");

        assert_eq!(quickjs_manifest.lights, luau_manifest.lights);
        assert_eq!(quickjs_manifest.lights.len(), 1);
        assert_eq!(quickjs_manifest.lights[0].index, 2);
    }

    #[test]
    fn luau_random_preserves_argument_forms_and_is_deterministic() {
        let source = r#"
            function setupLevel(_)
              local unit = math.random()
              local one = math.random(1)
              local exact = math.random(2, 2)
              local tag = unit >= 0 and unit < 1 and one == 1 and exact == 2 and "wave" or "dynamic"
              local light = world:query({ component = "light", tag = tag })[1]
              return { reactions = {
                defineReaction("levelLoad", { sequence = light:pulse({ min = 0, max = 1, periodMs = 1 }) }),
              } }
            end
        "#;
        let first = emit_light_membership_manifest(
            source,
            Path::new("fixture.luau"),
            Path::new("."),
            &table(),
        )
        .expect("first Luau evaluation");
        let second = emit_light_membership_manifest(
            source,
            Path::new("fixture.luau"),
            Path::new("."),
            &table(),
        )
        .expect("second Luau evaluation");
        assert_eq!(first, second);
        assert_eq!(first.lights[0].index, 2);
    }

    #[test]
    fn luau_relative_require_uses_mod_root_and_sdk_virtual_modules() {
        let root = std::env::temp_dir().join(format!(
            "postretro-script-compiler-require-{}",
            std::process::id()
        ));
        let modules = root.join("modules");
        std::fs::create_dir_all(&modules).expect("create module fixture");
        std::fs::write(
            modules.join("membership.luau"),
            r#"
                local Postretro = require("postretro")
                return function()
                  local light = Postretro.world:query({ component = "light", tag = "wave" })[1]
                  return Postretro.defineReaction("levelLoad", {
                    sequence = light:pulse({ min = 0, max = 1, periodMs = 1 }),
                  })
                end
            "#,
        )
        .expect("write module fixture");
        let source = r#"
            local membership = require("./modules/membership")
            function setupLevel(_)
              return { reactions = { membership() } }
            end
        "#;
        let manifest = emit_light_membership_manifest(
            source,
            &root.join("maps/fixture.luau"),
            &root,
            &table(),
        )
        .expect("map-local relative module evaluates like runtime");
        assert_eq!(manifest.lights[0].index, 2);
        std::fs::remove_dir_all(root).expect("remove module fixture");
    }

    #[test]
    fn luau_virtual_sdk_modules_are_recursively_readonly() {
        // Regression: compiler-time require froze only module roots, so nested
        // SDK tables were mutable even though the runtime deep-freezes them.
        let source = r#"
            local Postretro = require("postretro")
            local Ui = require("postretro/ui")
            local rootMutationOk = pcall(function()
              Postretro.world.query = function() return {} end
            end)
            local uiMutationOk = pcall(function()
              Ui.ui.createLocalState = function() return {} end
            end)
            function setupLevel(_)
              if rootMutationOk or uiMutationOk then
                error("nested SDK module table was writable")
              end
              local light = Postretro.world:query({ component = "light", tag = "wave" })[1]
              return { reactions = {
                Postretro.defineReaction("levelLoad", { sequence = light:pulse({ min = 0, max = 1, periodMs = 1 }) }),
              } }
            end
        "#;
        let manifest = emit_light_membership_manifest(
            source,
            Path::new("fixture.luau"),
            Path::new("."),
            &table(),
        )
        .expect("nested SDK module mutations fail like runtime");
        assert_eq!(manifest.lights[0].index, 2);
    }

    #[test]
    fn luau_and_typescript_derive_identical_membership() {
        let ts = r#"
            function setupLevel() {
              const light = world.query({ component: "light", tag: "wave" })[0];
              return { reactions: [defineReaction("levelLoad", { sequence: light.colorShift({ values: [{x: 1, y: 0, z: 0}], periodMs: 1000 }) })] };
            }
        "#;
        let luau = r#"
            local Postretro = require("postretro")
            function setupLevel(_ctx)
              local light = Postretro.world:query({ component = "light", tag = "wave" })[1]
              return { reactions = {
                Postretro.defineReaction("levelLoad", { sequence = light:colorShift({ values = {{x = 1, y = 0, z = 0}}, periodMs = 1000 }) }),
              } }
            end
        "#;
        let ts_manifest =
            emit_light_membership_manifest(ts, Path::new("fixture.ts"), Path::new("."), &table())
                .expect("TS evaluates");
        let luau_manifest = emit_light_membership_manifest(
            luau,
            Path::new("fixture.luau"),
            Path::new("."),
            &table(),
        )
        .expect("Luau evaluates");
        assert_eq!(ts_manifest.lights, luau_manifest.lights);
    }
}
