//! Mod-manifest SDK registration and its runtime-shape drift guard.

use postretro_scripting_core::primitives_registry::PrimitiveRegistry;

pub(crate) fn register_sdk_type(registry: &mut PrimitiveRegistry) {
    registry
        .register_enum("BloomResolution")
        .doc("Base resolution used by the mod's bloom chain. Lower resolutions produce chunkier bloom and reduce bloom-pass work.")
        .variant("half", "Start the bloom chain at half the scene dimensions. This is the default.")
        .variant("quarter", "Start the bloom chain at one quarter of the scene dimensions.")
        .variant("eighth", "Start the bloom chain at one eighth of the scene dimensions.")
        .finish();
    registry
        .register_type("BloomRenderProfile")
        .doc("Static bloom presentation preferences for the entire mod. Optional fields use the current half-resolution smooth defaults.")
        .field(
            "resolution?",
            "BloomResolution",
            "Bloom chain base resolution. Optional; defaults to `\"half\"`.",
        )
        .field(
            "pixelated?",
            "bool",
            "Use pixelated bloom upsampling and compositing. Optional; defaults to false.",
        )
        .finish();
    registry
        .register_type("RenderProfile")
        .doc("Static renderer preferences declared once for the entire mod.")
        .field(
            "bloom?",
            "BloomRenderProfile",
            "Bloom presentation preferences. Optional; defaults to half-resolution smooth bloom.",
        )
        .finish();
    registry
        .register_type("MoverDefaults")
        .doc("Static kinematic-mover defaults for the mod. Authored mover `auto_close_ms` values take precedence.")
        .field(
            "autoCloseMs?",
            "f32",
            "Automatic return delay after a mover reaches its open terminus, in milliseconds. Optional; defaults to 0 (disabled).",
        )
        .finish();
    registry
        .register_type("SwitchingDescriptor")
        .doc("Mod-global switching policy. Omit the whole block to preserve immediate direct selection, zero cycle dwell, and reload interruption.")
        .field(
            "commitOnDirectSelect",
            "bool",
            "Whether a direct slot-select action emits a commit immediately. Input-layer policy only.",
        )
        .field(
            "cycleCommitDwellMs",
            "f32",
            "Cycle-selection dwell in milliseconds. Must be finite and >= 0. Input-layer policy only.",
        )
        .field(
            "blockDuringReload",
            "bool",
            "Whether a weapon without its own override must finish reload activity before a switch can begin.",
        )
        .finish();
    registry
        .register_type("ModManifest")
        .doc("Mod manifest consumed from `start-script.ts`'s default export or `start-script.luau`'s chunk return. `defineMod(config)` is a pure typed identity helper for this object; the engine commits its data only after manifest validation and required durable-identity validation succeed.")
        .field("name", "String", "Human-readable mod name used for diagnostics and UI. Required.")
        .field(
            "id",
            "String",
            "Required stable mod identity used for connection admission. Peers must declare the same id to connect. Must match `[A-Za-z0-9_.-]{1,64}`; `:` is not allowed, and the id may not consist entirely of dots. Declared identity is not a security mechanism.",
        )
        .field(
            "version",
            "String",
            "Required mod version for display and diagnostics. It is never compared for admission and is not a security mechanism; any non-empty string is valid.",
        )
        .field(
            "render?",
            "RenderProfile",
            "Static renderer preferences for the entire mod. Optional; defaults to half-resolution smooth bloom.",
        )
        .field(
            "movers?",
            "MoverDefaults",
            "Static kinematic-mover defaults. Optional; authored mover auto_close_ms overrides this delay.",
        )
        .field(
            "switching?",
            "SwitchingDescriptor",
            "Mod-global switching policy. Optional; omission preserves immediate direct selection, zero cycle dwell, and reload interruption.",
        )
        .field(
            "entities?",
            "Vec<EntityTypeDescriptor>",
            "Engine-global entity-type registrations. Optional; survive level unload and are committed only after manifest validation and required durable-identity validation succeed.",
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
            "Engine-global state-store declarations returned by `defineStore(...).declaration`. Optional; commit atomically only after manifest validation and required durable-identity validation succeed, and preserve existing values when the schema is identical.",
        )
        .finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::slot_table::StoreDeclarationSet;
    use postretro_scripting_core::data_descriptors::{
        ModFontAssets, ModThemeTokens, SwitchingDescriptor,
    };
    use postretro_scripting_core::primitives_registry::TypeShape;
    use postretro_scripting_core::runtime::{
        ModManifestResult, ModMoverDefaults, ModRenderProfile,
    };

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
            id: String::new(),
            version: String::new(),
            render: ModRenderProfile::default(),
            movers: ModMoverDefaults::default(),
            switching: SwitchingDescriptor::default(),
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
            "id",
            "version",
            "render",
            "movers",
            "switching",
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

    #[test]
    fn bloom_render_profile_sdk_types_are_closed_and_optional() {
        let mut registry = PrimitiveRegistry::new();
        register_sdk_type(&mut registry);

        let resolution = registry
            .iter_types()
            .find(|registered| registered.name == "BloomResolution")
            .expect("BloomResolution must be registered");
        match &resolution.shape {
            TypeShape::StringEnum { variants } => {
                let names: Vec<&str> = variants.iter().map(|variant| variant.name).collect();
                assert_eq!(names, ["half", "quarter", "eighth"]);
            }
            other => panic!("BloomResolution must be a StringEnum, got {other:?}"),
        }

        for (name, expected_fields) in [
            (
                "BloomRenderProfile",
                ["resolution?", "pixelated?"].as_slice(),
            ),
            ("RenderProfile", ["bloom?"].as_slice()),
        ] {
            let registered = registry
                .iter_types()
                .find(|registered| registered.name == name)
                .unwrap_or_else(|| panic!("{name} must be registered"));
            let TypeShape::Struct { fields } = &registered.shape else {
                panic!("{name} must be a Struct, got {:?}", registered.shape);
            };
            let names: Vec<&str> = fields.iter().map(|field| field.name).collect();
            assert_eq!(names, expected_fields);
        }
    }
}
