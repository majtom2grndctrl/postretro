// Luau SDK prelude source and export inventory.
// See: context/lib/scripting.md

use mlua::{Lua, Table};

use super::error::ScriptError;
use super::luau_virtual_modules::LuauVirtualModuleRegistry;

/// SDK library prelude — `world.luau` returns the `world` table; we promote
/// it to global `world`. Embedded at compile time; SDK changes require an
/// engine rebuild.
const WORLD_LUAU_SRC: &str = include_str!("../../../sdk/lib/world.luau");

/// SDK library prelude — `entities/lights.luau` returns a table whose only
/// promoted field is `wrapLightEntity`, installed as a temporary global for
/// `world.luau` to capture and then nil'd out before the sandbox freezes.
/// Capability methods (`pulse`, `fade`, `flicker`, `colorShift`, `sweep`)
/// live on the handle returned from `wrapLightEntity`; no bare globals.
const LIGHTS_LUAU_SRC: &str = include_str!("../../../sdk/lib/entities/lights.luau");

/// SDK library prelude — `util/keyframes.luau` returns a table whose fields
/// (`timeline`, `sequence`) are destructured into globals.
const KEYFRAMES_LUAU_SRC: &str = include_str!("../../../sdk/lib/util/keyframes.luau");

/// SDK library prelude — `entities/emitters.luau` returns a table whose fields
/// are destructured into globals so authors can call them by bare name.
const EMITTERS_LUAU_SRC: &str = include_str!("../../../sdk/lib/entities/emitters.luau");

/// SDK library prelude — `entities/fog_volumes.luau` returns a table whose
/// only promoted field is `wrapFogVolumeEntity`, installed as a temporary
/// global for `world.luau` to capture and then nil'd out before the sandbox
/// freezes. Capability methods (`pulse`, `fade`, `flicker`,
/// `pulseSaturation`, `fadeSaturation`) live on the handle returned from
/// `wrapFogVolumeEntity`; no bare globals.
pub(super) const FOG_VOLUMES_LUAU_SRC: &str =
    include_str!("../../../sdk/lib/entities/fog_volumes.luau");

/// SDK library prelude — `data_script.luau` returns a table whose fields
/// (`defineReaction`, `defineEntity`, `defineMod`, `defineMapCatalog`, `defineStore`)
/// are destructured into globals so data-script authors call them by bare name.
/// Pure descriptor builders; no FFI happens until the mod manifest or `setupLevel`
/// returns.
const DATA_SCRIPT_LUAU_SRC: &str = include_str!("../../../sdk/lib/data_script.luau");

/// SDK library prelude — `runtime.luau` returns the runtime-value builder table,
/// promoted whole to global `runtime` (mirroring `world`). Pure data assembly: a
/// builder assembles a `RuntimeValue` table and never calls back into Rust.
const RUNTIME_LUAU_SRC: &str = include_str!("../../../sdk/lib/runtime.luau");

/// SDK library prelude — `ui/reactions.luau` returns state-crossing, system
/// reaction, UI-stack, and text-entry descriptor builders. These are evaluated
/// for the `postretro/ui` virtual module only; Task 1 of the UI SDK split keeps
/// them out of author-visible bare globals and out of `require("postretro")`.
const UI_REACTIONS_LUAU_SRC: &str = include_str!("../../../sdk/lib/ui/reactions.luau");

/// SDK library prelude — `ui/widgets.luau` returns widget factories for the
/// `postretro/ui` virtual module. The capitalized `Text`/`Panel`/… constructors
/// are no longer promoted to bare Luau globals.
const UI_WIDGETS_LUAU_SRC: &str = include_str!("../../../sdk/lib/ui/widgets.luau");

/// SDK library prelude — `ui/layout.luau` returns container factories for
/// `postretro/ui`; these are no longer promoted to bare Luau globals.
const UI_LAYOUT_LUAU_SRC: &str = include_str!("../../../sdk/lib/ui/layout.luau");

/// SDK library prelude — `ui/tree.luau` returns pure UI tree helpers for
/// `postretro/ui`; these are no longer promoted to bare Luau globals.
const UI_TREE_LUAU_SRC: &str = include_str!("../../../sdk/lib/ui/tree.luau");

/// SDK library prelude — `ui/state.luau` returns state-reference helpers plus
/// the presentation-local state namespace for `postretro/ui`. Authoritative
/// helpers are pure descriptor composers; local cell handles remain
/// presentation-only.
const UI_STATE_LUAU_SRC: &str = include_str!("../../../sdk/lib/ui/state.luau");

/// SDK library prelude — `ui/theme.luau` returns theme helpers for
/// `postretro/ui`; they are no longer promoted to bare Luau globals.
const UI_THEME_LUAU_SRC: &str = include_str!("../../../sdk/lib/ui/theme.luau");

/// SDK library prelude — `game_state.luau` captures the temporary frozen
/// engine-state reference tree bridge and returns `getGameState`.
const GAME_STATE_LUAU_SRC: &str = include_str!("../../../sdk/lib/game_state.luau");

/// Lights SDK fields lifted to globals after evaluating
/// `entities/lights.luau`. Empty: the public vocabulary lives on the handle
/// returned from `wrapLightEntity`, which is itself installed as a
/// temporary bridge (not a bare global) before `world.luau` evaluates and
/// nil'd out afterward.
const LIGHTS_LUAU_FIELDS: &[&str] = &[];

/// Keyframe-utility SDK fields lifted to globals after evaluating
/// `util/keyframes.luau`.
const KEYFRAMES_LUAU_FIELDS: &[&str] = &["timeline", "sequence"];

/// Emitter SDK fields lifted to globals after evaluating
/// `entities/emitters.luau`.
const EMITTERS_LUAU_FIELDS: &[&str] = &["emitter", "smokeEmitter", "sparkEmitter", "dustEmitter"];

/// Fog-volume SDK fields lifted to globals after evaluating
/// `entities/fog_volumes.luau`. Empty: the public vocabulary lives on the
/// handle returned from `wrapFogVolumeEntity`, which is itself installed
/// as a temporary bridge (not a bare global) before `world.luau`
/// evaluates and nil'd out afterward.
const FOG_VOLUMES_LUAU_FIELDS: &[&str] = &[];

/// Data-script SDK fields lifted to globals after evaluating
/// `data_script.luau`.
const DATA_SCRIPT_FIELDS: &[&str] = &[
    "defineReaction",
    "scopeReactions",
    "defineEntity",
    "defineMod",
    "defineMapCatalog",
    "defineStore",
];

/// UI-reactions SDK fields exported through `require("postretro/ui")`.
/// `onStateCrossing` builds a state-crossing watcher; the
/// rest are system-reaction body constructors that pair with `defineReaction` to
/// emit `playSound` / `rumble` / `flashScreen` / `vignette` / `screenShake`,
/// the UI-stack (`showDialog` /
/// `openMenu` / `closeDialog`) primitives, the `updateState` slot write (Goal F),
/// the text-entry helpers (`openTextEntry` wraps `showDialog` for the engine
/// keyboard; `KEYBOARD_TREE` is its registry name constant), reserved button
/// actions (`CLOSE_DIALOG_ACTION`, `EXIT_TO_DESKTOP_ACTION`,
/// `QUIT_TO_MENU_ACTION`), and the text-edit
/// reactions (`appendText` / `backspaceText` / `clearText`, M13 Text Entry).
pub(super) const UI_REACTIONS_FIELDS: &[&str] = &[
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
];

/// UI widget-factory SDK fields exported through `require("postretro/ui")`.
/// The capitalized leaf-widget constructors only —
/// `validateBorder` / `resolveReactionName` are internal helpers that
/// `layout.luau` redeclares locally, so they stay off the module table.
const UI_WIDGETS_FIELDS: &[&str] = &[
    "Text", "Panel", "Image", "Spacer", "Button", "Slider", "Bar", "Announce",
];

/// UI layout-factory SDK fields exported through `require("postretro/ui")`.
const UI_LAYOUT_FIELDS: &[&str] = &["VStack", "HStack", "Grid"];

/// UI tree SDK fields exported through `require("postretro/ui")`.
const UI_TREE_FIELDS: &[&str] = &["Tree", "defineUiTree"];

/// UI state-helper SDK fields exported through `require("postretro/ui")`.
const UI_STATE_MODULE_FIELDS: &[&str] = &[
    "bindState",
    "stateEquals",
    "createLocalState",
    "ui",
    "Switch",
];

/// UI theme-helper SDK fields exported through `require("postretro/ui")`.
const UI_THEME_FIELDS: &[&str] = &["defineTheme", "getDesignTokens"];

/// Engine-state SDK fields lifted to globals after evaluating
/// `game_state.luau`.
const GAME_STATE_FIELDS: &[&str] = &["getGameState"];

/// Authoritative runtime export names for `require("postretro/ui")`.
pub const POSTRETRO_UI_MODULE_EXPORTS: &[&str] = &[
    "Text",
    "Panel",
    "Image",
    "Spacer",
    "Button",
    "Slider",
    "Bar",
    "Announce",
    "VStack",
    "HStack",
    "Grid",
    "Tree",
    "defineUiTree",
    "getGameState",
    "bindState",
    "stateEquals",
    "createLocalState",
    "ui",
    "Switch",
    "defineTheme",
    "getDesignTokens",
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
];

/// Authoritative runtime export names for `require("postretro")`.
pub const POSTRETRO_ROOT_MODULE_EXPORTS: &[&str] = &[
    "world",
    "runtime",
    "getGameState",
    "timeline",
    "sequence",
    "defineReaction",
    "scopeReactions",
    "defineEntity",
    "defineMod",
    "defineMapCatalog",
    "defineStore",
    "emitter",
    "smokeEmitter",
    "sparkEmitter",
    "dustEmitter",
];

/// Evaluate the Luau SDK prelude in `lua` and promote the return values to
/// globals. Must be called after primitives are installed and before
/// `sandbox(true)` (which freezes `_G`). The primitive dependency applies
/// to `entities/lights.luau`, `world.luau`, and `fog_volumes.luau` — they
/// reference primitives like `worldQuery` and `setLightAnimation`.
/// `data_script.luau` is also evaluated as a prelude step but has no
/// primitive dependencies; it's pure data builders (`defineReaction`,
/// `defineEntity`).
/// The prelude source uses type annotations declared in postretro.d.luau (luau-lsp only); the runtime evaluates the .luau source without loading the declaration file.
pub fn evaluate_prelude(
    lua: &Lua,
    virtual_modules: Option<&LuauVirtualModuleRegistry>,
) -> Result<(), ScriptError> {
    super::game_state_refs::install_luau_bridge(lua)?;

    // Step 0: capture the engine-owned state reference tree into the pure
    // `getGameState` closure, then hide the temporary bridge before any author
    // code runs or `_G` is frozen.
    let game_state_sdk: Table = lua
        .load(GAME_STATE_LUAU_SRC)
        .set_name("postretro/sdk/game_state.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `game_state.luau`: {e}"),
            source_name: "sdk/lib/game_state.luau".to_string(),
        })?;
    let globals = lua.globals();
    for field in GAME_STATE_FIELDS {
        let value: mlua::Value =
            game_state_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("game_state.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 1: evaluate `entities/lights.luau`. The only exported field is
    // the `wrapLightEntity` bridge — capability methods (`pulse`, `fade`,
    // `flicker`, `colorShift`, `sweep`) live on the handle it produces,
    // not as bare globals. `wrapLightEntity` itself is installed below as
    // a temporary global so `world.luau` can capture it as an upvalue,
    // then nil'd out in step 4.
    let lights_sdk: Table = lua
        .load(LIGHTS_LUAU_SRC)
        .set_name("postretro/sdk/entities/lights.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `entities/lights.luau`: {e}"),
            source_name: "sdk/lib/entities/lights.luau".to_string(),
        })?;
    let wrap_light_entity: mlua::Value =
        lights_sdk
            .get("wrapLightEntity")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("entities/lights.luau missing `wrapLightEntity`: {e}"),
            })?;
    globals
        .set("wrapLightEntity", wrap_light_entity)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install temporary global `wrapLightEntity`: {e}"),
        })?;

    // Step 2: install the public lights fields as globals.
    // `LIGHTS_LUAU_FIELDS` is empty in the capability-handle world; the
    // loop is retained so adding a future bare global is a one-line
    // change in the slice declaration.
    for field in LIGHTS_LUAU_FIELDS {
        let value: mlua::Value =
            lights_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("entities/lights.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 2b: evaluate `entities/fog_volumes.luau`. Mirrors lights.luau:
    // the only exported field is `wrapFogVolumeEntity`. Capability
    // methods (`pulse`, `fade`, `flicker`, `pulseSaturation`,
    // `fadeSaturation`) live on the handle, not as bare globals.
    let fog_volumes_sdk: Table = lua
        .load(FOG_VOLUMES_LUAU_SRC)
        .set_name("postretro/sdk/entities/fog_volumes.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `entities/fog_volumes.luau`: {e}"),
            source_name: "sdk/lib/entities/fog_volumes.luau".to_string(),
        })?;
    let wrap_fog_volume_entity: mlua::Value =
        fog_volumes_sdk
            .get("wrapFogVolumeEntity")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("entities/fog_volumes.luau missing `wrapFogVolumeEntity`: {e}"),
            })?;
    globals
        .set("wrapFogVolumeEntity", wrap_fog_volume_entity)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install temporary global `wrapFogVolumeEntity`: {e}"),
        })?;
    for field in FOG_VOLUMES_LUAU_FIELDS {
        let value: mlua::Value =
            fog_volumes_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("entities/fog_volumes.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 3: evaluate `world.luau`. Its `query` closure captures
    // `wrapLightEntity` and `wrapFogVolumeEntity` as upvalues at evaluation
    // time, so step 4's nil-out does not break the closure.
    let world: mlua::Value = lua
        .load(WORLD_LUAU_SRC)
        .set_name("postretro/sdk/world.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `world.luau`: {e}"),
            source_name: "sdk/lib/world.luau".to_string(),
        })?;
    globals
        .set("world", world.clone())
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install global `world`: {e}"),
        })?;

    // Step 4: nil out the temporary `wrapLightEntity` / `wrapFogVolumeEntity`
    // bridges so author scripts never see them as bare globals once
    // `sandbox(true)` freezes `_G`.
    globals
        .set("wrapLightEntity", mlua::Value::Nil)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to clear temporary global `wrapLightEntity`: {e}"),
        })?;
    globals
        .set("wrapFogVolumeEntity", mlua::Value::Nil)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to clear temporary global `wrapFogVolumeEntity`: {e}"),
        })?;

    // Step 5: evaluate `util/keyframes.luau` and lift its fields to globals.
    let keyframes_sdk: Table = lua
        .load(KEYFRAMES_LUAU_SRC)
        .set_name("postretro/sdk/util/keyframes.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `util/keyframes.luau`: {e}"),
            source_name: "sdk/lib/util/keyframes.luau".to_string(),
        })?;
    for field in KEYFRAMES_LUAU_FIELDS {
        let value: mlua::Value =
            keyframes_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("util/keyframes.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 6: evaluate `entities/emitters.luau` and lift its fields to globals.
    let emitters_sdk: Table = lua
        .load(EMITTERS_LUAU_SRC)
        .set_name("postretro/sdk/entities/emitters.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `entities/emitters.luau`: {e}"),
            source_name: "sdk/lib/entities/emitters.luau".to_string(),
        })?;
    for field in EMITTERS_LUAU_FIELDS {
        let value: mlua::Value =
            emitters_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("entities/emitters.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 7: evaluate `data_script.luau` and lift its fields to globals.
    let data_sdk: Table = lua
        .load(DATA_SCRIPT_LUAU_SRC)
        .set_name("postretro/sdk/data_script.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `data_script.luau`: {e}"),
            source_name: "sdk/lib/data_script.luau".to_string(),
        })?;
    for field in DATA_SCRIPT_FIELDS {
        let value: mlua::Value =
            data_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("data_script.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    // Step 7b: evaluate `ui/reactions.luau` for the `postretro/ui` virtual
    // module. Do not lift its fields to globals: Task 1 of the UI SDK split
    // removes author-visible Luau UI bare globals.
    let ui_reactions_sdk: Table = lua
        .load(UI_REACTIONS_LUAU_SRC)
        .set_name("postretro/sdk/ui/reactions.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `ui/reactions.luau`: {e}"),
            source_name: "sdk/lib/ui/reactions.luau".to_string(),
        })?;

    let ui_theme_sdk: Table = lua
        .load(UI_THEME_LUAU_SRC)
        .set_name("postretro/sdk/ui/theme.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `ui/theme.luau`: {e}"),
            source_name: "sdk/lib/ui/theme.luau".to_string(),
        })?;
    let unwrap_theme_token: mlua::Value =
        ui_theme_sdk
            .get("__unwrapThemeToken")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("ui/theme.luau missing internal token validator: {e}"),
            })?;
    globals
        .set("__postretroUnwrapThemeToken", unwrap_theme_token)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install temporary theme-token validator: {e}"),
        })?;

    // Step 7c–7f: evaluate the M13 G1a UI factory modules for the
    // `postretro/ui` virtual module. Widget/layout modules capture the
    // temporary theme-token validator as an upvalue so token records cannot be
    // forged structurally by author code.
    let ui_widgets_sdk: Table = lua
        .load(UI_WIDGETS_LUAU_SRC)
        .set_name("postretro/sdk/ui/widgets.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `ui/widgets.luau`: {e}"),
            source_name: "sdk/lib/ui/widgets.luau".to_string(),
        })?;

    let ui_layout_sdk: Table = lua
        .load(UI_LAYOUT_LUAU_SRC)
        .set_name("postretro/sdk/ui/layout.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `ui/layout.luau`: {e}"),
            source_name: "sdk/lib/ui/layout.luau".to_string(),
        })?;
    globals
        .set("__postretroUnwrapThemeToken", mlua::Value::Nil)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to clear temporary theme-token validator: {e}"),
        })?;

    let ui_tree_sdk: Table = lua
        .load(UI_TREE_LUAU_SRC)
        .set_name("postretro/sdk/ui/tree.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `ui/tree.luau`: {e}"),
            source_name: "sdk/lib/ui/tree.luau".to_string(),
        })?;

    let ui_state_sdk: Table = lua
        .load(UI_STATE_LUAU_SRC)
        .set_name("postretro/sdk/ui/state.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `ui/state.luau`: {e}"),
            source_name: "sdk/lib/ui/state.luau".to_string(),
        })?;

    // Step 8: evaluate `runtime.luau` and promote its table to global `runtime`.
    // The builders are pure (no primitive dependency), so ordering relative to
    // the other steps is irrelevant.
    let runtime: mlua::Value = lua
        .load(RUNTIME_LUAU_SRC)
        .set_name("postretro/sdk/runtime.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `runtime.luau`: {e}"),
            source_name: "sdk/lib/runtime.luau".to_string(),
        })?;
    globals
        .set("runtime", runtime.clone())
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install global `runtime`: {e}"),
        })?;

    if let Some(virtual_modules) = virtual_modules {
        populate_virtual_modules(
            lua,
            virtual_modules,
            LuauSdkExportInventory {
                world,
                runtime,
                game_state_sdk,
                keyframes_sdk,
                emitters_sdk,
                data_sdk,
                ui_reactions_sdk,
                ui_widgets_sdk,
                ui_layout_sdk,
                ui_tree_sdk,
                ui_state_sdk,
                ui_theme_sdk,
            },
        )?;
    }

    Ok(())
}

struct LuauSdkExportInventory {
    world: mlua::Value,
    runtime: mlua::Value,
    game_state_sdk: Table,
    keyframes_sdk: Table,
    emitters_sdk: Table,
    data_sdk: Table,
    ui_reactions_sdk: Table,
    ui_widgets_sdk: Table,
    ui_layout_sdk: Table,
    ui_tree_sdk: Table,
    ui_state_sdk: Table,
    ui_theme_sdk: Table,
}

fn populate_virtual_modules(
    lua: &Lua,
    virtual_modules: &LuauVirtualModuleRegistry,
    inventory: LuauSdkExportInventory,
) -> Result<(), ScriptError> {
    let ui_module = lua
        .create_table()
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to allocate `postretro/ui` virtual module inventory: {e}"),
        })?;
    copy_fields_to_table(
        &ui_module,
        &inventory.ui_widgets_sdk,
        UI_WIDGETS_FIELDS,
        "ui/widgets.luau",
    )?;
    copy_fields_to_table(
        &ui_module,
        &inventory.ui_layout_sdk,
        UI_LAYOUT_FIELDS,
        "ui/layout.luau",
    )?;
    copy_fields_to_table(
        &ui_module,
        &inventory.ui_tree_sdk,
        UI_TREE_FIELDS,
        "ui/tree.luau",
    )?;
    copy_fields_to_table(
        &ui_module,
        &inventory.ui_state_sdk,
        UI_STATE_MODULE_FIELDS,
        "ui/state.luau",
    )?;
    copy_fields_to_table(
        &ui_module,
        &inventory.ui_reactions_sdk,
        UI_REACTIONS_FIELDS,
        "ui/reactions.luau",
    )?;
    copy_fields_to_table(
        &ui_module,
        &inventory.game_state_sdk,
        GAME_STATE_FIELDS,
        "game_state.luau",
    )?;
    copy_fields_to_table(
        &ui_module,
        &inventory.ui_theme_sdk,
        UI_THEME_FIELDS,
        "ui/theme.luau",
    )?;
    virtual_modules.register_from_table(lua, "postretro/ui", ui_module)?;

    let root_module = lua
        .create_table()
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to allocate `postretro` virtual module inventory: {e}"),
        })?;
    root_module
        .set("world", inventory.world)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to set `postretro.world` virtual module export: {e}"),
        })?;
    root_module
        .set("runtime", inventory.runtime)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to set `postretro.runtime` virtual module export: {e}"),
        })?;
    copy_fields_to_table(
        &root_module,
        &inventory.game_state_sdk,
        GAME_STATE_FIELDS,
        "game_state.luau",
    )?;
    copy_fields_to_table(
        &root_module,
        &inventory.keyframes_sdk,
        KEYFRAMES_LUAU_FIELDS,
        "util/keyframes.luau",
    )?;
    copy_fields_to_table(
        &root_module,
        &inventory.data_sdk,
        DATA_SCRIPT_FIELDS,
        "data_script.luau",
    )?;
    copy_fields_to_table(
        &root_module,
        &inventory.emitters_sdk,
        EMITTERS_LUAU_FIELDS,
        "entities/emitters.luau",
    )?;
    virtual_modules.register_from_table(lua, "postretro", root_module)?;

    Ok(())
}

fn copy_fields_to_table(
    target: &Table,
    source: &Table,
    fields: &[&str],
    source_name: &str,
) -> Result<(), ScriptError> {
    for field in fields {
        let value: mlua::Value = source
            .get(*field)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("{source_name} missing virtual module export `{field}`: {e}"),
            })?;
        target
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to set virtual module export `{field}`: {e}"),
            })?;
    }
    Ok(())
}
