//! Mod-manifest SDK registration and its runtime-shape drift guard.

use postretro_scripting_core::primitives_registry::PrimitiveRegistry;

pub(crate) fn register_sdk_type(registry: &mut PrimitiveRegistry) {
    registry
        .register_type("ModManifest")
        .doc("Mod manifest consumed from `start-script.ts`'s default export or `start-script.luau`'s chunk return. `defineMod(config)` is a pure typed identity helper for this object; the engine commits its data only after manifest validation succeeds.")
        .field("name", "String", "Human-readable mod name used for diagnostics and UI. Required.")
        .field(
            "entities?",
            "Vec<EntityTypeDescriptor>",
            "Engine-global entity-type registrations. Optional; survive level unload and are committed only after the manifest validates.",
        )
        .field(
            "uiTrees?",
            "Vec<ModUiTree>",
            "Script-registered UI trees (name + `AnchoredTree` + `alwaysOn`). Optional; malformed entries are logged and skipped without aborting boot.",
        )
        .field(
            "theme?",
            "ThemeTokens",
            "Theme token overrides (colors/fonts/spacing). Optional; merged per-token into the engine default.",
        )
        .field(
            "fonts?",
            "FontFamilyMap",
            "Font assets: family name → TTF asset path. Optional; changing custom font assets requires an engine restart.",
        )
        .field(
            "maps?",
            "Vec<ModMapEntry>",
            "Pre-load-discoverable map catalog. Optional; use catalog ids with `loadLevel(id)` and `frontend.backgroundLevel`.",
        )
        .field(
            "frontend?",
            "Frontend",
            "Mod-defined frontend menu declaration. Optional; omission clears the mod frontend and presents the engine fallback menu.",
        )
        .field(
            "reactions?",
            "Vec<NamedReactionDescriptor>",
            "Engine-global reaction definitions. Optional; survive level unload and compose into active level behavior by `levels` tag selectors.",
        )
        .field(
            "events?",
            "Vec<ImpactEvent>",
            "Pure mod-global impact-policy declarations. Optional; `levels` selects map tags, setupLevel events append level-local declarations, and base plus matching last-registered override resolve by author-assigned id. Override filters narrow the base filter.",
        )
        .field(
            "crossings?",
            "Vec<CrossingDescriptor>",
            "Engine-global state-crossing watchers. Optional; survive level unload and compose into active level behavior by `levels` tag selectors.",
        )
        .field(
            "triggerEvents?",
            "Vec<TriggerEventDescriptor>",
            "Trigger-volume enter/exit observers. Optional; compose by level tags.",
        )
        .field(
            "triggerPools?",
            "Vec<TriggerPoolDescriptor>",
            "Trigger-volume arming pools. Optional; compose by level tags.",
        )
        .field(
            "stores?",
            "Vec<StoreDeclaration>",
            "Engine-global state-store declarations returned by `defineStore(...).declaration`. Optional; commit atomically after the manifest validates and preserve existing values when the schema is identical.",
        )
        .finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::slot_table::StoreDeclarationSet;
    use postretro_scripting_core::data_descriptors::{ModFontAssets, ModThemeTokens};
    use postretro_scripting_core::primitives_registry::TypeShape;
    use postretro_scripting_core::runtime::ModManifestResult;

    #[test]
    fn mod_manifest_registered_type_matches_mod_manifest_result() {
        // Parity guard: the `ModManifest` shape emitted to the SDK
        // (`gen-script-types`) must mirror `ModManifestResult` in
        // `runtime.rs`. If the canonical struct grows a field, this test
        // forces the registered type to follow.
        //
        // The expected field list is derived from `ModManifestResult`'s
        // definition. Field-presence assertions below construct a value of
        // that struct so any rename or removal in `runtime.rs` is a compile
        // error here.

        // Compile-time anchor for manifest fields. `stores` is authored as
        // `ModManifest.stores` and lands in `store_declarations` after
        // validation, so the expected field list below maps that Rust field
        // back to its script-visible name.
        let _shape_anchor = ModManifestResult {
            name: String::new(),
            entities: Vec::new(),
            ui_trees: Vec::new(),
            theme: ModThemeTokens::default(),
            frontend: None,
            fonts: ModFontAssets::default(),
            maps: Vec::new(),
            reactions: Vec::new(),
            crossings: Vec::new(),
            events: Vec::new(),
            trigger_events: Vec::new(),
            trigger_pools: Vec::new(),
            store_declarations: StoreDeclarationSet::default(),
        };
        let expected_fields: &[&str] = &[
            "name",
            "entities",
            "uiTrees",
            "theme",
            "frontend",
            "fonts",
            "maps",
            "reactions",
            "crossings",
            "events",
            "triggerEvents",
            "triggerPools",
            "stores",
        ];

        let mut registry = PrimitiveRegistry::new();
        register_sdk_type(&mut registry);
        let registered = registry
            .iter_types()
            .find(|registered| registered.name == "ModManifest")
            .expect("ModManifest must be registered");
        let fields = match &registered.shape {
            TypeShape::Struct { fields } => fields,
            other => panic!("ModManifest must be a Struct, got {other:?}"),
        };
        // Strip the optional-marker suffix so `entities?` matches `entities`.
        let got_names: Vec<&str> = fields
            .iter()
            .map(|field| field.name.trim_end_matches('?'))
            .collect();
        for expected in expected_fields {
            assert!(
                got_names.contains(expected),
                "ModManifest registered type missing field `{expected}`; has {got_names:?}",
            );
        }
        for got in &got_names {
            assert!(
                expected_fields.contains(got),
                "ModManifest registered type has extra field `{got}` not in ModManifestResult; expected {expected_fields:?}",
            );
        }
    }
}
