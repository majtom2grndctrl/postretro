use super::*;

/// `defineReaction` (M13 G1a) widens to accept an optional `name`: both the
/// `(body)` overload (deterministic auto-id) and the `(name, body)` overload
/// surface in the root TypeScript output, and the Luau UI module prop types
/// (`ButtonProps.onPress`, crossing `fire`) accept a typed handle or a bare
/// string. The wire form (`onPress: string`) is unchanged.
#[test]
fn reaction_handle_authoring_types_widen_in_both_outputs() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);

    // The name-optional `(body)` overload is present alongside `(name, body)`.
    assert_eq!(
        ts.matches("export function defineReaction(").count(),
        2,
        "ts must declare both defineReaction overloads"
    );
    assert!(
        ts.contains("levels?: string[]")
            && ts.contains("export function scopeReactions(")
            && ts.contains("list: NamedReactionDescriptor[]"),
        "ts must expose reaction levels and scopeReactions:\n{ts}"
    );
    assert!(
        ts.contains("fire: string[];\n    levels?: string[];"),
        "ts must expose crossing levels for ModManifest.crossings:\n{ts}"
    );
    assert!(
        luau.contains("levels: {string}?")
            && luau.contains("declare function scopeReactions(tags: {string}, list: {NamedReactionDescriptor}): {NamedReactionDescriptor}")
            && luau.contains("scopeReactions: typeof(scopeReactions),"),
        "luau must expose reaction levels and scopeReactions:\n{luau}"
    );
    assert!(
        luau.contains("fire: {string}, levels: {string}?"),
        "luau must expose crossing levels for ModManifest.crossings:\n{luau}"
    );
    // Widened reaction-reference props still appear in the Luau UI module
    // prop types. TypeScript root no longer exposes UI prop types in Task 1.
    assert!(
        luau.contains("onPress: ReactionHandleRef | string"),
        "luau ButtonProps.onPress must accept a handle or string"
    );
    assert!(
        luau.contains("fire: {NamedReactionDescriptor | string}"),
        "luau onStateCrossing.fire must accept handles or strings"
    );
}

#[test]
fn root_type_outputs_do_not_expose_ui_authoring_helpers() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);
    let ts_root = ts_module_block(&ts, "postretro");

    const UI_FUNCTIONS: &[&str] = &[
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
        "bindState",
        "stateEquals",
        "createLocalState",
        "Switch",
        "defineTheme",
        "onStateCrossing",
        "playSound",
        "rumble",
        "flashScreen",
        "vignette",
        "screenShake",
        "showDialog",
        "openMenu",
        "closeDialog",
        "loadLevel",
        "restartLevel",
        "returnToFrontend",
        "openTextEntry",
        "updateState",
        "appendText",
        "backspaceText",
        "clearText",
    ];
    for f in UI_FUNCTIONS {
        assert!(
            !ts_root.contains(&format!("export function {f}(")),
            "root TS d.ts must not expose UI helper `{f}`"
        );
        assert!(
            !luau.contains(&format!("declare function {f}(")),
            "Luau d.luau must not expose UI bare global `{f}`"
        );
    }
    assert!(
        !ts_root.contains("export const ui:")
            && !luau.contains("declare ui:")
            && !luau.contains("declare bindState:"),
        "root outputs must not expose UI state helper globals"
    );
    assert!(
        !ts_root.contains("export const KEYBOARD_TREE")
            && !ts_root.contains("export const CLOSE_DIALOG_ACTION")
            && !luau.contains("declare KEYBOARD_TREE")
            && !luau.contains("declare CLOSE_DIALOG_ACTION"),
        "root outputs must not expose UI reaction constants"
    );
    assert!(
        ts_root
            .contains("export type WidgetDescriptor = { kind: string; [field: string]: unknown };")
            && ts_root.contains("export type AnchoredTreeDescriptor = {")
            && luau.contains("export type WidgetDescriptor = { [string]: any }")
            && luau.contains("export type AnchoredTreeDescriptor ="),
        "manifest UI wire types must remain available for ModUiTree declarations"
    );
    assert!(
        !ts_root.contains("export function setState("),
        "ts must not expose raw-string setState as an author-facing helper"
    );
    assert!(
        !luau.contains("declare function setState("),
        "luau must not expose raw-string setState as an author-facing helper"
    );
    assert!(
        !ts.contains("storeHandle"),
        "ts must not expose storeHandle"
    );
    assert!(
        !luau.contains("storeHandle"),
        "luau must not expose storeHandle"
    );

    assert!(luau.contains("export type PostretroUiModule = {"));
}

#[test]
fn luau_virtual_module_types_and_require_overloads_are_generated() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;
    use postretro_scripting_core::luau_prelude::{
        POSTRETRO_ROOT_MODULE_EXPORTS, POSTRETRO_UI_MODULE_EXPORTS,
    };

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let luau = generate_luau(&r);

    assert!(
        luau.contains("export type PostretroUiModule = {")
            && luau.contains("export type PostretroModule = {")
            && luau.contains(
                "export type ColorToken = { __postretroToken: \"color\", token: string }"
            )
            && luau
                .contains("export type FontToken = { __postretroToken: \"font\", token: string }")
            && luau.contains(
                "export type SpacingToken = { __postretroToken: \"spacing\", token: string }"
            )
            && luau.contains(
                "export type DesignTokenTree<Token> = Token & { [string]: DesignTokenTree<Token> }"
            )
            && luau.contains("color: DesignTokenTree<ColorToken>,")
            && luau.contains("export type WidgetColor = {number} | ColorToken")
            && luau.contains("export type WidgetSpacing = number | SpacingToken")
            && luau.contains("font: FontToken?"),
        "luau output missing virtual module aliases:\n{luau}"
    );
    assert!(
        luau.contains("declare require: ((path: \"postretro\") -> PostretroModule)")
            && luau.contains("& ((path: \"postretro/ui\") -> PostretroUiModule)")
            && luau.contains("& ((path: string) -> any)"),
        "luau output missing literal require overloads with string fallback:\n{luau}"
    );

    for export in POSTRETRO_ROOT_MODULE_EXPORTS {
        assert!(
            luau.contains(&format!("{export}:")),
            "PostretroModule missing root export `{export}`"
        );
    }

    for export in POSTRETRO_UI_MODULE_EXPORTS {
        assert!(
            luau.contains(&format!("{export}:")),
            "PostretroUiModule missing UI export `{export}`"
        );
    }
}

#[test]
fn typescript_ui_module_declaration_is_generated() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let ui_module = ts_module_block(&ts, "postretro/ui");
    let root_module = ts_module_block(&ts, "postretro");

    assert!(
        ui_module.contains("export function Text(props: TextProps): WidgetDescriptor;")
            && ui_module.contains("export function defineTheme<const T extends ThemeDefinition>")
            && ui_module
                .contains("export function getDesignTokens<const T extends ThemeDefinition>")
            && ui_module.contains("export function createLocalState")
            && ui_module.contains("export const ui:")
            && ui_module.contains("export function showDialog(")
            && ui_module.contains("export function getGameState(): GameStateRefs;")
            && ui_module.contains(
                "export type ThemeToken<Category extends \"color\" | \"font\" | \"spacing\">"
            )
            && ui_module.contains("export type ColorToken = ThemeToken<\"color\">;")
            && ui_module.contains("export type FontToken = ThemeToken<\"font\">;")
            && ui_module.contains("export type SpacingToken = ThemeToken<\"spacing\">;")
            && ui_module.contains(
                "export type WidgetColor = [number, number, number, number] | ColorToken;"
            )
            && ui_module.contains("export type WidgetSpacing = number | SpacingToken;")
            && ui_module.contains("font?: FontToken;"),
        "TypeScript `postretro/ui` module missing UI authoring surface:\n{ui_module}"
    );
    assert!(
        !root_module.contains("export function Text(props: TextProps): WidgetDescriptor;")
            && !root_module.contains("export function defineTheme("),
        "root TypeScript module leaked UI authoring helpers:\n{root_module}"
    );
}

/// The runtime-value vocabulary (scripting.md §11) is a closed union: every
/// opcode tag must be typed in both `.d.ts` and `.d.luau` so an author
/// cannot name an op outside it. Asserts each tag appears in both outputs
/// generated through the same path the `gen-script-types` bin uses. The
/// author surface is `RuntimeValue`; the wire `op` tags are unchanged.
#[test]
fn runtime_opcode_vocabulary_appears_in_both_type_outputs() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);

    // The closed opcode set, matching `IrNode`'s snake_case wire tags.
    // Assertions are anchored on the discriminant field form, not a bare
    // quoted string, so they can only pass when the opcode appears as the
    // `op` field value in the emitted union variant:
    //   TS:   `op: "const";`  (semicolon-separated struct field)
    //   Luau: `op: "const",`  (comma-separated struct field)
    const OPCODES: &[&str] = &[
        "const", "input", "add", "sub", "mul", "div", "clamp", "lerp", "select", "lt", "le", "gt",
        "ge", "eq", "ne",
    ];
    for op in OPCODES {
        let ts_discriminant = format!("op: \"{op}\";");
        let luau_discriminant = format!("op: \"{op}\",");
        assert!(
            ts.contains(&ts_discriminant),
            "ts d.ts missing runtime opcode discriminant `{ts_discriminant}`"
        );
        assert!(
            luau.contains(&luau_discriminant),
            "luau d.luau missing runtime opcode discriminant `{luau_discriminant}`"
        );
    }

    // The union alias itself must surface so the closure is nameable.
    assert!(
        ts.contains("export type RuntimeValue ="),
        "ts missing RuntimeValue union"
    );
    assert!(
        luau.contains("export type RuntimeValue ="),
        "luau missing RuntimeValue union"
    );
}

/// `WidgetAnchor` in both typedef outputs must enumerate EXACTLY the variants
/// of `crate::render::ui::layout::Anchor` — no more, no less.
///
/// The expected union is DERIVED from `Anchor::ALL`/`Anchor::wire()` (the
/// single source of truth), not a hand-copied list. `wire()` is an
/// exhaustive `match` with no catch-all arm, so adding a variant to `Anchor`
/// is a compile error until its wire string is defined; the new variant then
/// joins `ALL` and this test fails unless the emitted `WidgetAnchor` union is
/// updated to match. `parse_anchor` in `data_descriptors.rs` maps the same
/// wire strings back to variants.
#[test]
fn widget_anchor_typedef_matches_layout_anchor_variants() {
    use crate::render::ui::layout::Anchor;
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let ts = generate_typescript(&r);
    let luau = generate_luau(&r);

    // Derive the expected union body straight from the enum's source of truth.
    // `Anchor::ALL` enumerates the variants; `wire()` is the exhaustive
    // variant→camelCase map that `parse_anchor` and serde also honor.
    let expected_union_body: String = Anchor::ALL
        .iter()
        .map(|a| format!("\"{}\"", a.wire()))
        .collect::<Vec<_>>()
        .join(" | ");

    // Assert each derived variant appears in both outputs.
    for anchor in Anchor::ALL {
        let wire = anchor.wire();
        assert!(
            ts.contains(&format!("\"{wire}\"")),
            "ts d.ts WidgetAnchor union missing anchor variant \"{wire}\""
        );
        assert!(
            luau.contains(&format!("\"{wire}\"")),
            "luau d.luau WidgetAnchor union missing anchor variant \"{wire}\""
        );
    }

    // Assert the union contains no extras by matching the full type alias line.
    // TS terminates with a semicolon; Luau omits it.
    let ts_union_line = format!("export type WidgetAnchor = {expected_union_body};");
    let luau_union_line = format!("export type WidgetAnchor = {expected_union_body}");
    assert!(
        ts.contains(&ts_union_line),
        "ts d.ts WidgetAnchor union does not exactly match `Anchor::ALL`/`wire()`.\n\
         Expected line: {ts_union_line}"
    );
    assert!(
        luau.contains(&luau_union_line),
        "luau d.luau WidgetAnchor union does not exactly match `Anchor::ALL`/`wire()`.\n\
         Expected line: {luau_union_line}"
    );
}

/// M13 G2 Task 5: the emitted typedefs must NARROW props per widget kind, so
/// an author wiring the wrong prop to the wrong widget gets a compile error
/// in their editor (the no-`tsc`-CI contract — the committed `.d.ts`/`.d.luau`
/// IS the type-safety surface.
///
/// This test guards the narrowing at the typedef-block level: it asserts that
/// `content` lives ONLY on the `Text` prop type (so `Button({ content })` is a
/// type error — `ButtonProps`/`SliderProps` carry no `content`), that the
/// passive `Bar` prop type requires no accessible name (no `label`/`labelledBy`
/// XOR appended), and that the interactive `Button`/`Slider` prop types DO
/// carry the name XOR.
#[test]
fn luau_widget_props_narrow_per_kind() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let luau = generate_luau(&r);

    // `content` is a Text-only prop. The TextProps type declares it; the
    // Button/Slider/Bar prop types must NOT — that absence is exactly what
    // makes `Button({ content: "x" })` a type error (an unknown-prop excess).
    assert!(
        luau.contains("export type TextProps = { content: LocalizedText"),
        "luau TextProps must carry `content`"
    );
    // The interactive + passive widget prop types must be content-free.
    for props in ["ButtonProps", "SliderProps", "BarProps"] {
        let luau_line = extract_decl_line(&luau, &format!("export type {props} = "));
        assert!(
            !luau_line.contains("content"),
            "luau {props} must NOT carry a `content` prop (Text-only), got: {luau_line}"
        );
    }

    // A passive widget (`Bar`) needs no accessible name — its prop type ends
    // at the plain object, with no `label`/`labelledBy` name-XOR appended.
    let bar_luau = extract_decl_line(&luau, "export type BarProps = ");
    assert!(
        !bar_luau.contains("label") && !bar_luau.contains("labelledBy"),
        "luau BarProps must require no name, got: {bar_luau}"
    );

    // The interactive widgets DO carry the `label` xor `labelledBy` name
    // requirement (the union tail).
    for props in ["ButtonProps", "SliderProps"] {
        let luau_line = extract_decl_line(&luau, &format!("export type {props} = "));
        assert!(
            luau_line.contains("({ label: LocalizedText } | { labelledBy: string })"),
            "luau {props} must carry the label-xor-labelledBy union, got: {luau_line}"
        );
    }

    // `Image` narrows to `label` xor `decorative: true` (the alt-vs-decorative
    // contract): neither/both is a type error.
    let img_luau = extract_decl_line(&luau, "export type ImageProps = ");
    assert!(
        img_luau.contains("({ label: string } | { decorative: true })"),
        "luau ImageProps must narrow label xor decorative, got: {img_luau}"
    );
}

/// State equality uses `stateEquals(ref, value)` for authoritative refs, while
/// presentation-local cells keep their existing `LocalStateHandle.is(value)`.
#[test]
fn luau_predicate_helpers_are_typed_to_the_value_type() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    let mut r = PrimitiveRegistry::new();
    register_all(&mut r, ScriptCtx::new());
    let luau = generate_luau(&r);

    assert!(
        luau.contains("stateEquals: <T>(ref: ReadonlyStateRef<T>, value: T) -> Predicate,"),
        "luau stateEquals declaration must type the comparand to the ref value type"
    );
    assert!(
        luau.contains("is: (self: LocalStateHandle<T>, value: T) -> Predicate,"),
        "luau LocalStateHandle:is must be typed `(self, value: T) -> Predicate`"
    );
}
