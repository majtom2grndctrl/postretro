// Luau SDK prelude source and export inventory.
// See: context/lib/scripting.md

use mlua::{Lua, LuaSerdeExt as _, Table};

use super::error::ScriptError;
use super::luau_virtual_modules::LuauVirtualModuleRegistry;

/// Member-handle wrapper modules, evaluated in this order before
/// `map_entities.luau`. Each returns a table whose wrapper function is installed
/// as a temporary global of the same name for `map_entities.luau` to capture as
/// an upvalue, then nil'd out before author code runs. Their verbs live on the
/// handles; none is promoted to a bare global.
/// `(wrapper global, source, sdk/lib-relative path)`.
const MEMBER_WRAPPER_SOURCES: &[(&str, &str, &str)] = &[
    (
        "wrapLightEntity",
        include_str!("../../../sdk/lib/entities/lights.luau"),
        "entities/lights.luau",
    ),
    (
        "wrapFogVolumeEntity",
        include_str!("../../../sdk/lib/entities/fog_volumes.luau"),
        "entities/fog_volumes.luau",
    ),
    (
        "wrapMoverEntity",
        include_str!("../../../sdk/lib/entities/movers.luau"),
        "entities/movers.luau",
    ),
    (
        "wrapTriggerVolumeEntity",
        include_str!("../../../sdk/lib/entities/triggers.luau"),
        "entities/triggers.luau",
    ),
    (
        "wrapSpawnerEntity",
        include_str!("../../../sdk/lib/entities/spawners.luau"),
        "entities/spawners.luau",
    ),
];

/// SDK library prelude — `map_entities.luau` returns `getMapEntities`, which
/// captures the member wrappers above. Embedded at compile time; SDK changes
/// require an engine rebuild.
const MAP_ENTITIES_LUAU_SRC: &str = include_str!("../../../sdk/lib/map_entities.luau");

/// SDK library prelude — `gravity.luau` returns `getGravity` / `setGravity`.
const GRAVITY_LUAU_SRC: &str = include_str!("../../../sdk/lib/gravity.luau");

/// SDK library prelude — `util/keyframes.luau` returns a table whose fields
/// (`timeline`, `sequence`) are destructured into globals.
const KEYFRAMES_LUAU_SRC: &str = include_str!("../../../sdk/lib/util/keyframes.luau");

/// SDK library prelude — `entities/emitters.luau` returns a table whose fields
/// are destructured into globals so authors can call them by bare name.
const EMITTERS_LUAU_SRC: &str = include_str!("../../../sdk/lib/entities/emitters.luau");

/// SDK library prelude — `data_script.luau` returns a table of public
/// descriptor builders that are destructured into globals so data-script authors
/// call them by bare name.
/// Pure descriptor builders; no FFI happens until the mod manifest or `setupLevel`
/// returns.
const DATA_SCRIPT_LUAU_SRC: &str = include_str!("../../../sdk/lib/data_script.luau");

/// Part chunks of `data_script.luau`, evaluated in this order before it. Each
/// returns a table whose `builders` the main module merges into its own; the
/// temporary [`DATA_SCRIPT_PARTS_GLOBAL`] bridge carries them (and the shared
/// subject tokens and dispatch params) across chunks and is cleared once the
/// main module returns. `(bridge key, source, sdk/lib-relative path)`.
const DATA_SCRIPT_PART_SOURCES: &[(&str, &str, &str)] = &[
    (
        "commands",
        include_str!("../../../sdk/lib/data_script/commands.luau"),
        "data_script/commands.luau",
    ),
    (
        "reactions",
        include_str!("../../../sdk/lib/data_script/reactions.luau"),
        "data_script/reactions.luau",
    ),
    (
        "triggerEvents",
        include_str!("../../../sdk/lib/data_script/trigger_events.luau"),
        "data_script/trigger_events.luau",
    ),
];
const DATA_SCRIPT_PARTS_GLOBAL: &str = "__postretroDataScriptParts";

/// Private identity-map implementation shared by the pure descriptor builders.
const EXPRESSION_REFS_LUAU_SRC: &str = include_str!("../../../sdk/lib/util/expression_refs.luau");
const EXPRESSION_REFS_GLOBAL: &str = "__postretroExpressionRefs";

/// Frozen weapon-action namespace, shared by global and virtual-module exports.
const ACTIVATION_LUAU_SRC: &str = include_str!("../../../sdk/lib/activation.luau");

/// Temporary bridge captured by `data_script.luau` so descriptor arrays retain
/// their sequence shape even when empty. The SDK clears it before author code runs.
const ARRAY_METATABLE_GLOBAL: &str = "__postretroArrayMetatable";

/// SDK library prelude — `runtime.luau` returns the runtime-value builder table,
/// promoted whole to global `runtime`. Pure data assembly: a
/// builder assembles a `RuntimeValue` table and never calls back into Rust.
const RUNTIME_LUAU_SRC: &str = include_str!("../../../sdk/lib/runtime.luau");

/// SDK library prelude — `brain.luau` returns the behavior-graph guard-input
/// sugar (`brain`, `candidate`, `state`). Pure data assembly with no primitive dependency
/// and no ordering constraint: it builds its IR input leaves literally rather
/// than through the `runtime` global.
const BRAIN_LUAU_SRC: &str = include_str!("../../../sdk/lib/brain.luau");

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

/// SDK library prelude — passive presentation-template authoring for the
/// `postretro/ui` virtual module. These descriptors never become modal trees.
const UI_PRESENTATION_LUAU_SRC: &str = include_str!("../../../sdk/lib/ui/presentation.luau");

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

/// Map-member SDK fields lifted to globals after evaluating `map_entities.luau`.
const MAP_ENTITIES_FIELDS: &[&str] = &["getMapEntities"];

/// Gravity SDK fields lifted to globals after evaluating `gravity.luau`.
const GRAVITY_FIELDS: &[&str] = &["getGravity", "setGravity"];

/// Keyframe-utility SDK fields lifted to globals after evaluating
/// `util/keyframes.luau`.
const KEYFRAMES_LUAU_FIELDS: &[&str] = &["timeline", "sequence"];

/// Emitter SDK fields lifted to globals after evaluating
/// `entities/emitters.luau`.
const EMITTERS_LUAU_FIELDS: &[&str] = &["emitter", "smokeEmitter", "sparkEmitter", "dustEmitter"];

/// Data-script SDK fields lifted to globals after evaluating
/// `data_script.luau`.
/// No lifted name may shadow a Luau builtin
/// (`lifted_globals_never_shadow_a_luau_builtin`).
const DATA_SCRIPT_FIELDS: &[&str] = &[
    "defineReaction",
    "defineImpactEvent",
    "defineTriggerEvent",
    "npcs",
    "players",
    "wait",
    "fire",
    "scopeReactions",
    "defineEntity",
    "defineMod",
    "defineFaction",
    "sentiment",
    "defineMapCatalog",
    "defineWeaponPlacement",
    "defineTriggerPool",
    "defineStore",
    "read",
    "fromRuntime",
    "set",
    "update",
    "when",
];

/// UI-reactions SDK fields exported through `require("postretro/ui")`.
/// `onStateCrossing` builds a state-crossing watcher; the
/// rest are system-reaction body constructors that pair with `defineReaction` to
/// emit `playSound` / `rumble` / `flashScreen` / `vignette` / `screenShake`,
/// the UI-stack (`showDialog` /
/// `openMenu` / `closeDialog`) primitives, the `updateState` slot write (Goal F),
/// live directional faction-sentiment writes (`setSentiment` /
/// `adjustSentiment`),
/// the text-entry helpers (`openTextEntry` wraps `showDialog` for the engine
/// keyboard; `KEYBOARD_TREE` is its registry name constant), reserved button
/// actions (`CLOSE_DIALOG_ACTION`, `EXIT_TO_DESKTOP_ACTION`,
/// `QUIT_TO_MENU_ACTION`, `OPEN_ACCESSIBILITY_ACTION`, `accessibilityAction`),
/// and the text-edit
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
    "OPEN_ACCESSIBILITY_ACTION",
    "accessibilityAction",
    "displayModeAction",
    "loadLevel",
    "restartLevel",
    "returnToFrontend",
    "setSentiment",
    "adjustSentiment",
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
    "Text", "Panel", "Image", "Spacer", "Button", "Slider", "Bar", "Ring", "Announce",
];

/// UI layout-factory SDK fields exported through `require("postretro/ui")`.
const UI_LAYOUT_FIELDS: &[&str] = &["VStack", "HStack", "Grid"];

/// UI tree SDK fields exported through `require("postretro/ui")`.
const UI_TREE_FIELDS: &[&str] = &["Tree", "defineUiTree"];

/// Passive presentation-template builder exported through `postretro/ui`.
const UI_PRESENTATION_FIELDS: &[&str] = &[
    "definePresentationTemplate",
    "damagedEnemies",
    "defineOverlay",
    "fact",
];

/// Data-script impact builder exposed only through `postretro/ui`, not as a
/// root/bare global alongside the ordinary data declarations.
const UI_DATA_SCRIPT_FIELDS: &[&str] = &["present"];

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

/// Behavior-graph guard-input SDK fields lifted to globals after evaluating
/// `brain.luau`.
const BRAIN_LUAU_FIELDS: &[&str] = &["brain", "candidate", "state"];

/// Authoritative runtime export names for `require("postretro/ui")`.
pub const POSTRETRO_UI_MODULE_EXPORTS: &[&str] = &[
    "Text",
    "Panel",
    "Image",
    "Spacer",
    "Button",
    "Slider",
    "Bar",
    "Ring",
    "Announce",
    "VStack",
    "HStack",
    "Grid",
    "Tree",
    "defineUiTree",
    "definePresentationTemplate",
    "damagedEnemies",
    "defineOverlay",
    "fact",
    "present",
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
    "OPEN_ACCESSIBILITY_ACTION",
    "accessibilityAction",
    "displayModeAction",
    "loadLevel",
    "restartLevel",
    "returnToFrontend",
    "setSentiment",
    "adjustSentiment",
    "updateState",
    "appendText",
    "backspaceText",
    "clearText",
];

/// Authoritative runtime export names for `require("postretro")`.
pub const POSTRETRO_ROOT_MODULE_EXPORTS: &[&str] = &[
    "runtime",
    "activation",
    "getMapEntities",
    "getGravity",
    "setGravity",
    "getGameState",
    "timeline",
    "sequence",
    "defineReaction",
    "defineImpactEvent",
    "defineTriggerEvent",
    "npcs",
    "players",
    "wait",
    "fire",
    "scopeReactions",
    "defineEntity",
    "defineMod",
    "defineFaction",
    "sentiment",
    "defineMapCatalog",
    "defineWeaponPlacement",
    "defineTriggerPool",
    "defineStore",
    "read",
    "fromRuntime",
    "set",
    "update",
    "when",
    "brain",
    "candidate",
    "state",
    "emitter",
    "smokeEmitter",
    "sparkEmitter",
    "dustEmitter",
];

/// Evaluate the Luau SDK prelude in `lua` and promote the return values to
/// globals. Must be called after primitives are installed and before
/// `sandbox(true)` (which freezes `_G`). The primitive dependency applies
/// to the member and gravity SDK modules — they call primitives like
/// `worldQuery` and `worldGetGravity`. The member wrappers evaluate before
/// `map_entities.luau` captures them.
/// `data_script.luau` is also evaluated as a prelude step but has no
/// primitive dependencies; its exported builders are pure data assembly.
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

    // Step 1: evaluate the member-handle wrappers and install each wrapper as
    // a temporary global for `map_entities.luau` to capture as an upvalue.
    for (wrapper, source, path) in MEMBER_WRAPPER_SOURCES {
        let module = eval_sdk_table(lua, source, path)?;
        let wrap: mlua::Value = module
            .get(*wrapper)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("{path} missing `{wrapper}`: {e}"),
            })?;
        set_global(&globals, wrapper, wrap)?;
    }

    // Step 2: evaluate `map_entities.luau`, lift `getMapEntities`, then nil out
    // the wrapper bridges so author scripts never see them once
    // `sandbox(true)` freezes `_G`. The captured upvalues keep working.
    let map_entities_sdk = eval_sdk_table(lua, MAP_ENTITIES_LUAU_SRC, "map_entities.luau")?;
    lift_fields(
        &globals,
        &map_entities_sdk,
        MAP_ENTITIES_FIELDS,
        "map_entities.luau",
    )?;
    for (wrapper, _, _) in MEMBER_WRAPPER_SOURCES {
        set_global(&globals, wrapper, mlua::Value::Nil)?;
    }

    // Step 3: evaluate `gravity.luau` and lift `getGravity` / `setGravity`.
    let gravity_sdk = eval_sdk_table(lua, GRAVITY_LUAU_SRC, "gravity.luau")?;
    lift_fields(&globals, &gravity_sdk, GRAVITY_FIELDS, "gravity.luau")?;

    // Step 4: evaluate `util/keyframes.luau` and lift its fields to globals.
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

    // Step 5: evaluate `entities/emitters.luau` and lift its fields to globals.
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

    // Share the private fluent-ref identity map between data and activation
    // builders. Each captures the functions before the bridge is hidden.
    let expression_refs: Table = lua
        .load(EXPRESSION_REFS_LUAU_SRC)
        .set_name("postretro/sdk/util/expression_refs.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `util/expression_refs.luau`: {e}"),
            source_name: "sdk/lib/util/expression_refs.luau".to_string(),
        })?;
    globals
        .set(EXPRESSION_REFS_GLOBAL, expression_refs)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install temporary expression-ref bridge: {e}"),
        })?;

    // Step 6: evaluate `data_script.luau` and lift its fields to globals. The
    // private array metatable lets pure builders distinguish empty descriptor
    // arrays from empty descriptor maps when the result crosses through serde.
    globals
        .set(ARRAY_METATABLE_GLOBAL, lua.array_metatable())
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install temporary descriptor-array metatable: {e}"),
        })?;
    let data_sdk = evaluate_data_script_sdk(lua)?;
    globals
        .set(ARRAY_METATABLE_GLOBAL, mlua::Value::Nil)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to clear temporary descriptor-array metatable: {e}"),
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

    let activation: mlua::Value = lua
        .load(ACTIVATION_LUAU_SRC)
        .set_name("postretro/sdk/activation.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `activation.luau`: {e}"),
            source_name: "sdk/lib/activation.luau".to_string(),
        })?;
    globals
        .set("activation", activation.clone())
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install global `activation`: {e}"),
        })?;
    globals
        .set(EXPRESSION_REFS_GLOBAL, mlua::Value::Nil)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to clear temporary expression-ref bridge: {e}"),
        })?;

    // Step 6b: evaluate `ui/reactions.luau` for the `postretro/ui` virtual
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

    // Step 6c–6f: evaluate the M13 G1a UI factory modules for the
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

    let ui_presentation_sdk: Table = lua
        .load(UI_PRESENTATION_LUAU_SRC)
        .set_name("postretro/sdk/ui/presentation.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `ui/presentation.luau`: {e}"),
            source_name: "sdk/lib/ui/presentation.luau".to_string(),
        })?;

    let ui_state_sdk: Table = lua
        .load(UI_STATE_LUAU_SRC)
        .set_name("postretro/sdk/ui/state.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `ui/state.luau`: {e}"),
            source_name: "sdk/lib/ui/state.luau".to_string(),
        })?;

    // Step 7: evaluate `runtime.luau` and promote its table to global `runtime`.
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

    // Step 8: evaluate `brain.luau` and lift its behavior-graph guard-input
    // sugar to globals. Pure data assembly like `runtime.luau`; the ordering is
    // free.
    let brain_sdk: Table = lua
        .load(BRAIN_LUAU_SRC)
        .set_name("postretro/sdk/brain.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `brain.luau`: {e}"),
            source_name: "sdk/lib/brain.luau".to_string(),
        })?;
    for field in BRAIN_LUAU_FIELDS {
        let value: mlua::Value =
            brain_sdk
                .get(*field)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("brain.luau missing `{field}`: {e}"),
                })?;
        globals
            .set(*field, value)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to install global `{field}`: {e}"),
            })?;
    }

    if let Some(virtual_modules) = virtual_modules {
        populate_virtual_modules(
            lua,
            virtual_modules,
            LuauSdkExportInventory {
                map_entities_sdk,
                gravity_sdk,
                runtime,
                activation,
                game_state_sdk,
                brain_sdk,
                keyframes_sdk,
                emitters_sdk,
                data_sdk,
                ui_reactions_sdk,
                ui_widgets_sdk,
                ui_layout_sdk,
                ui_tree_sdk,
                ui_presentation_sdk,
                ui_state_sdk,
                ui_theme_sdk,
            },
        )?;
    }

    Ok(())
}

struct LuauSdkExportInventory {
    map_entities_sdk: Table,
    gravity_sdk: Table,
    runtime: mlua::Value,
    activation: mlua::Value,
    game_state_sdk: Table,
    brain_sdk: Table,
    keyframes_sdk: Table,
    emitters_sdk: Table,
    data_sdk: Table,
    ui_reactions_sdk: Table,
    ui_widgets_sdk: Table,
    ui_layout_sdk: Table,
    ui_tree_sdk: Table,
    ui_presentation_sdk: Table,
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
        &inventory.ui_presentation_sdk,
        UI_PRESENTATION_FIELDS,
        "ui/presentation.luau",
    )?;
    copy_fields_to_table(
        &ui_module,
        &inventory.data_sdk,
        UI_DATA_SCRIPT_FIELDS,
        "data_script.luau",
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
    copy_fields_to_table(
        &root_module,
        &inventory.map_entities_sdk,
        MAP_ENTITIES_FIELDS,
        "map_entities.luau",
    )?;
    copy_fields_to_table(
        &root_module,
        &inventory.gravity_sdk,
        GRAVITY_FIELDS,
        "gravity.luau",
    )?;
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
        &inventory.brain_sdk,
        BRAIN_LUAU_FIELDS,
        "brain.luau",
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
    // Fluent refs use table identity. Keep this already-frozen namespace as
    // the same singleton as the bare global, rather than the registry's deep
    // copy, so `Postretro.activation.charge` lowers even without a method call.
    let root_module =
        virtual_modules
            .get("postretro")
            .ok_or_else(|| ScriptError::InvalidArgument {
                reason: "registered `postretro` virtual module is missing".to_string(),
            })?;
    root_module.set_readonly(false);
    let result = root_module.set("activation", inventory.activation);
    root_module.set_readonly(true);
    result.map_err(|e| ScriptError::InvalidArgument {
        reason: format!("failed to set `postretro.activation` singleton export: {e}"),
    })?;

    Ok(())
}

/// Evaluate `data_script.luau` and its part chunks, returning the merged
/// builder table. The caller owns any other temporary bridges the module
/// captures (`__postretroExpressionRefs`, `__postretroArrayMetatable`).
pub(crate) fn evaluate_data_script_sdk(lua: &Lua) -> Result<Table, ScriptError> {
    let globals = lua.globals();
    let parts = lua
        .create_table()
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to allocate data-script part bridge: {e}"),
        })?;
    globals
        .set(DATA_SCRIPT_PARTS_GLOBAL, parts.clone())
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install temporary data-script part bridge: {e}"),
        })?;
    for (key, source, path) in DATA_SCRIPT_PART_SOURCES {
        let part: Table = lua
            .load(*source)
            .set_name(format!("postretro/sdk/{path}"))
            .eval()
            .map_err(|e| ScriptError::ScriptThrew {
                msg: format!("failed to evaluate SDK prelude `{path}`: {e}"),
                source_name: format!("sdk/lib/{path}"),
            })?;
        parts
            .set(*key, part)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("failed to publish data-script part `{path}`: {e}"),
            })?;
    }
    let data_sdk: Table = lua
        .load(DATA_SCRIPT_LUAU_SRC)
        .set_name("postretro/sdk/data_script.luau")
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `data_script.luau`: {e}"),
            source_name: "sdk/lib/data_script.luau".to_string(),
        })?;
    globals
        .set(DATA_SCRIPT_PARTS_GLOBAL, mlua::Value::Nil)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to clear temporary data-script part bridge: {e}"),
        })?;
    Ok(data_sdk)
}

/// Evaluate one SDK library chunk that returns a table.
fn eval_sdk_table(lua: &Lua, source: &str, path: &str) -> Result<Table, ScriptError> {
    lua.load(source)
        .set_name(format!("postretro/sdk/{path}"))
        .eval()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: format!("failed to evaluate SDK prelude `{path}`: {e}"),
            source_name: format!("sdk/lib/{path}"),
        })
}

fn set_global(globals: &Table, name: &str, value: mlua::Value) -> Result<(), ScriptError> {
    globals
        .set(name, value)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to set global `{name}`: {e}"),
        })
}

/// Promote `fields` of an evaluated SDK table to bare globals.
fn lift_fields(
    globals: &Table,
    source: &Table,
    fields: &[&str],
    source_name: &str,
) -> Result<(), ScriptError> {
    for field in fields {
        let value: mlua::Value = source
            .get(*field)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("{source_name} missing `{field}`: {e}"),
            })?;
        set_global(globals, field, value)?;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn data_script_export_inventory_matches_returned_sdk_fields() {
        let lua = Lua::new();
        let expression_refs: Table = lua.load(EXPRESSION_REFS_LUAU_SRC).eval().unwrap();
        lua.globals()
            .set(EXPRESSION_REFS_GLOBAL, expression_refs)
            .unwrap();
        let sdk = evaluate_data_script_sdk(&lua).expect("data-script SDK must evaluate");

        let actual = sdk
            .pairs::<String, mlua::Value>()
            .map(|entry| entry.map(|(field, _)| field))
            .collect::<Result<BTreeSet<_>, _>>()
            .expect("data-script SDK fields must be string-keyed");
        let expected = DATA_SCRIPT_FIELDS
            .iter()
            .chain(UI_DATA_SCRIPT_FIELDS.iter())
            .map(|field| (*field).to_string())
            .collect::<BTreeSet<_>>();

        assert_eq!(
            actual, expected,
            "every data-script builder must be inventoried; UI-only builders remain available only through `require(\"postretro/ui\")`"
        );
    }

    #[test]
    fn activation_global_and_module_share_refs_and_hide_private_bridge() {
        let lua = Lua::new();
        let modules = LuauVirtualModuleRegistry::new();
        evaluate_prelude(&lua, Some(&modules)).expect("SDK prelude must evaluate");
        lua.globals()
            .set("Postretro", modules.get("postretro").unwrap())
            .unwrap();
        let steps: Table = lua
            .load(
                r#"
                assert(__postretroExpressionRefs == nil)
                assert(activation == Postretro.activation)
                local direct = Postretro.activation.shot({ scale = { damage = Postretro.activation.charge } })
                local fluent = activation.shot({ scale = { damage = activation.charge:times(9):plus(1) } })
                local lifted = activation.shot({ scale = { resourceCost = fromRuntime.number({ op = "input", name = "charge" }):plus(1) } })
                return { direct, fluent, lifted, activation.wait(80) }
                "#,
            )
            .eval()
            .expect("activation builders must share the fluent lowering map");
        let value: serde_json::Value = lua.from_value(mlua::Value::Table(steps)).unwrap();
        assert_eq!(
            value[0]["scale"]["damage"],
            serde_json::json!({ "op": "input", "name": "charge" })
        );
        assert_eq!(value[1]["scale"]["damage"]["a"]["op"], "mul");
        assert_eq!(value[1]["scale"]["damage"]["b"]["value"], 1);
        assert_eq!(value[2]["scale"]["resourceCost"]["op"], "add");
        assert_eq!(
            value[3],
            serde_json::json!({ "kind": "wait", "durationMs": 80 })
        );
    }

    // A lifted bare global that shadowed a Luau builtin (`select`, `type`, ...)
    // would silently break author code and the SDK chunks that call it.
    #[test]
    fn lifted_globals_never_shadow_a_luau_builtin() {
        let lua = Lua::new();
        let builtins = lua.globals();
        let lifted = ["runtime", "activation"]
            .into_iter()
            .chain(MAP_ENTITIES_FIELDS.iter().copied())
            .chain(GRAVITY_FIELDS.iter().copied())
            .chain(GAME_STATE_FIELDS.iter().copied())
            .chain(BRAIN_LUAU_FIELDS.iter().copied())
            .chain(KEYFRAMES_LUAU_FIELDS.iter().copied())
            .chain(DATA_SCRIPT_FIELDS.iter().copied())
            .chain(EMITTERS_LUAU_FIELDS.iter().copied());
        for name in lifted {
            let existing: mlua::Value = builtins.get(name).expect("global lookup");
            assert!(
                existing.is_nil(),
                "lifted SDK global `{name}` shadows a Luau builtin"
            );
        }
    }

    #[test]
    fn root_module_export_inventory_matches_composed_runtime_fields() {
        let expected = ["runtime", "activation"]
            .into_iter()
            .chain(MAP_ENTITIES_FIELDS.iter().copied())
            .chain(GRAVITY_FIELDS.iter().copied())
            .chain(GAME_STATE_FIELDS.iter().copied())
            .chain(BRAIN_LUAU_FIELDS.iter().copied())
            .chain(KEYFRAMES_LUAU_FIELDS.iter().copied())
            .chain(DATA_SCRIPT_FIELDS.iter().copied())
            .chain(EMITTERS_LUAU_FIELDS.iter().copied())
            .collect::<BTreeSet<_>>();
        let actual = POSTRETRO_ROOT_MODULE_EXPORTS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();

        assert_eq!(
            actual, expected,
            "the generated root-module type inventory must match the runtime module composition"
        );
    }
}
