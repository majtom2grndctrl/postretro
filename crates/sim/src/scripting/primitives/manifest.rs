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
        .register_enum("ReflectionStyle")
        .doc("Look of every fluid reflection layer.")
        .variant("pixel", "One reflection direction per water texel cell, tilted by that cell's shimmer and ripples, so the reflection breaks into the blocks the water's texture moves in. Drift and a declared wave tilt nothing. This is the default.")
        .variant("smooth", "Reflect the flat surface per pixel: a still mirror.")
        .finish();
    registry
        .register_type("ReflectionRenderProfile")
        .doc("Static fluid-reflection presentation preferences for the entire mod. How strongly each fluid reflects is its own `surfaceReflectivity`.")
        .field(
            "style?",
            "ReflectionStyle",
            "Reflection look. Optional; defaults to `\"pixel\"`. An unknown value warns and uses `\"pixel\"`.",
        )
        .finish();
    registry
        .register_enum("GhostTransparency")
        .doc("How every ghost-link ghost is made translucent, mod-wide. Projectile billboard ghosts blend under both.")
        .variant("blend", "Alpha-blend each ghost at its presented opacity. This is the default.")
        .variant("dither", "Screen-door: an ordered screen-space pattern keeps or discards each ghost pixel, keeping about its presented opacity's share of them, so ghosts read as stippled rather than see-through.")
        .finish();
    registry
        .register_type("RenderProfile")
        .doc("Static renderer preferences declared once for the entire mod.")
        .field(
            "bloom?",
            "BloomRenderProfile",
            "Bloom presentation preferences. Optional; defaults to half-resolution smooth bloom.",
        )
        .field(
            "reflections?",
            "ReflectionRenderProfile",
            "Fluid reflection presentation preferences. Optional; defaults to pixel-style reflections.",
        )
        .field(
            "ghosts?",
            "GhostTransparency",
            "How ghosts are made translucent. Optional; defaults to `\"blend\"`. An unknown value warns and uses `\"blend\"`.",
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
        .register_enum("AttenuationCurve")
        .doc("How a positional sound's level falls off between `minDistance` and `maxDistance`.")
        .variant(
            "linear",
            "Fall off evenly, in decibels, across the range. This is the default.",
        )
        .variant(
            "quadratic",
            "Hold level near `minDistance`, then fall off faster toward `maxDistance`.",
        )
        .finish();
    registry
        .register_type("AudioAttenuation")
        .doc("Mod-wide distance attenuation for positional sounds, in metres. Applies to sounds that start after the manifest commits; sounds already playing keep theirs. A malformed field, a negative distance, or `minDistance` >= `maxDistance` warns naming the field and uses the whole default (2, 60, `\"linear\"`).")
        .field(
            "minDistance?",
            "f32",
            "Distance at and below which a sound plays at full level, in metres. Finite and >= 0. Optional; defaults to 2.",
        )
        .field(
            "maxDistance?",
            "f32",
            "Distance at and beyond which a sound is silent, in metres. Finite and greater than `minDistance`. Optional; defaults to 60.",
        )
        .field(
            "curve?",
            "AttenuationCurve",
            "Falloff shape between the two distances. Optional; defaults to `\"linear\"`.",
        )
        .finish();
    registry
        .register_type("AudioGhostProfile")
        .doc("Mod-wide mix of the Ghost bus, the shared bus ghost-link echoes play through. It sits under SFX, so the player's SFX volume and the underwater muffle apply on top. Every field is optional; an omitted field takes the floor value. A malformed field — not a number, non-finite or out of range — or a `highPassHz` at or above `lowPassHz` once omitted fields are filled warns naming the field and applies the whole floor profile (0.6, 200 Hz, 4000 Hz, drive 0).")
        .field(
            "volume?",
            "f32",
            "Bus volume, 0 to 1, on the same perceptual taper as the player's volume options; 0 silences ghost audio. Optional; defaults to 0.6.",
        )
        .field(
            "highPassHz?",
            "f32",
            "High-pass cutoff in Hz, 20 to 20000, below `lowPassHz`: thins out the low end. Optional; defaults to 200.",
        )
        .field(
            "lowPassHz?",
            "f32",
            "Low-pass cutoff in Hz, 20 to 20000, above `highPassHz`: muffles the top end. Optional; defaults to 4000.",
        )
        .field(
            "drive?",
            "f32",
            "Distortion drive, 0 to 1; 0 applies no distortion, and higher values saturate the echo harder. Optional; defaults to 0.",
        )
        .finish();
    registry
        .register_type("AudioProfile")
        .doc("Static audio preferences declared once for the entire mod.")
        .field(
            "attenuation?",
            "AudioAttenuation",
            "Distance attenuation for positional sounds. Optional; defaults to 2 to 60 metres, linear.",
        )
        .field(
            "ghost?",
            "AudioGhostProfile",
            "Ghost bus mix for ghost-link echoes. Optional; defaults to the floor profile: volume 0.6, 200 Hz to 4000 Hz, no drive.",
        )
        .finish();
    register_input_types(registry);
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
        .register_type("FactionDescriptor")
        .doc("A stable named faction declared in `ModManifest.factions`. The engine assigns authored declarations indices from 2 upward; player absence remains index 0 and the built-in default enemy faction remains index 1.")
        .field("name", "String", "Stable non-empty faction name. Entity archetypes refer to this name through `components.faction`.")
        .finish();
    registry
        .register_type("FactionSentimentDescriptor")
        .doc("One directed relationship from `fromFaction` toward `toFaction`. Negative sentiment is hostile, zero is neutral, and positive is allied. Both endpoint names must be declared in `ModManifest.factions`.")
        .field("fromFaction", "String", "Evaluating faction name (the directional source).")
        .field("toFaction", "String", "Offered candidate faction name (the directional destination).")
        .field("sentiment", "f32", "Finite directional sentiment: negative hostile, zero neutral, positive allied.")
        .field("tolerance", "f32", "Finite per-pair tolerance reserved for the engine-owned retaliation term.")
        .field("decay?", "f32", "Optional non-negative rate that eases this live pair back to its authored sentiment. Overrides `factionSentimentDecay`; zero holds this pair.")
        .finish();
    registry
        .register_type("FluidDescriptor")
        .doc("A substance declared in `ModManifest.fluids`, built with `defineFluid`. A map's `fluid_volume` names it through its `fluid` key; the volume owns the place, this owns what being in it is like. Declaration order assigns each fluid's index. A field out of range, or a second declaration of an existing name, is rejected with a warning naming the field; the first declaration stands and the rest of the manifest is kept. Fluids never affect co-op compatibility.")
        .field("name", "String", "Non-empty name a `fluid_volume`'s `fluid` key refers to. Unique within the manifest; a later duplicate is rejected.")
        .field("viscosity", "f32", "Drag while swimming, 1/s. Finite and >= 0. A volume naming no declared fluid loads dry.")
        .field("viewTint", "[f32; 4]", "Full-screen tint while the camera eye is inside: red, green, blue, then strength. Each finite and in [0, 1].")
        .field("muffle", "f32", "World-sound low-pass while the camera eye is inside: 0 none, 1 maximum. Finite and in [0, 1].")
        .field("surfaceOpacity", "f32", "Opacity of the fluid's visible surface faces. Finite and in [0, 1].")
        .field("surfaceReflectivity?", "f32", "How strongly the surface reflects the room around it: a scale on its Fresnel base reflectance (0.02 x value). 0 reflects nothing, 1 is water, about 2 reads as glass, about 10 as wet polished stone, 50 a mirror at every angle. Opacity never dims the reflection, so a zero-opacity fluid still reflects. Finite and in [0, 50]. Omitted: 1.")
        .field("enterSound?", "String", "Optional sound key played when a swim starts in this fluid.")
        .field("exitSound?", "String", "Optional sound key played when a swim in this fluid ends.")
        .field("fogDistance?", "f32", "Metres of view path into the fluid, seen from outside it, that hide 95% of what lies beyond. Finite and > 0. Omitted: clear. Never applies to the view from inside, which `viewTint` alone covers.")
        .field("fogColor?", "[f32; 3]", "Murk colour as `[r, g, b]`, in `viewTint`'s rgb space; the engine lights it with the surface's own lighting. Each finite and in [0, 1]. Omitted: `viewTint`'s rgb.")
        .field("wavePeriod?", "f32", "Seconds per cycle of an average-length surface wave crest. Finite and >= 6. Omitted: no wave.")
        .field("waveAmplitude?", "f32", "Peak sideways shift of the surface wave, in texture texels at the face's texture scale; 0 is no wave. Finite and in [0, 16]. Omitted: 8.")
        .field("waveLength?", "NumberOrRange", "Crest-to-crest length, in texture texels: one number L, meaning [L, L], or a `[min, max]` range from which each crest takes its own length. Finite, 0 < min <= max <= 128, and min >= 8 x `waveAmplitude` (the default 8 included). Omitted: [max(48, 8A), max(80, 8A)] for amplitude A.")
        .finish();
    registry
        .register_type("GhostStyleDescriptor")
        .doc("A ghost style declared in `ModManifest.ghostStyles`, built with `defineGhostStyle`: how a ghost link's ghosts look and whether their sounds carry. A map's `ghost_link` names it through its `ghost_style` key. Every field but `name` is optional; an omitted field takes the floor style's value (neutral, translucent, no sound). A malformed field drops the whole style with a warning naming it, and a second declaration of an existing name is dropped; the first declaration stands and the rest of the manifest is kept. A link naming no declared style uses the floor style. Styles resolve on each machine by name and never affect co-op compatibility.")
        .field("name", "String", "Non-empty name a `ghost_link`'s `ghost_style` key refers to. Unique within the manifest; a later duplicate is dropped.")
        .field("tint?", "[f32; 3]", "Colour multiplier as `[r, g, b]`, each finite and in [0, 1]. Omitted: `[1, 1, 1]`.")
        .field("opacity?", "f32", "Presented opacity, finite and in [0, 1]. Omitted: 0.5.")
        .field("saturation?", "f32", "Colour saturation, finite and in [0, 1]: 0 is greyscale, 1 the member's own colour. Omitted: 0.")
        .field("audio?", "bool", "Members' sounds also play at the ghost through the Ghost bus. Omitted: false.")
        .field("impactAudio?", "bool", "Impact sounds from members' shots ghost too. Takes effect only when `audio` is true. Omitted: false.")
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
            "Static renderer preferences for the entire mod. Optional; defaults to half-resolution smooth bloom, pixel-style fluid reflections and alpha-blended ghosts.",
        )
        .field(
            "movers?",
            "MoverDefaults",
            "Static kinematic-mover defaults. Optional; authored mover auto_close_ms overrides this delay.",
        )
        .field(
            "audio?",
            "AudioProfile",
            "Static audio preferences for the entire mod. Optional; defaults to 2 to 60 metre linear attenuation and the floor ghost bus profile.",
        )
        .field(
            "input?",
            "ModInput",
            "The game's commands, default bindings, and glyph art. Optional; omission keeps the engine's default bindings. Malformed entries warn and fall back per command and device class; they never reject the manifest.",
        )
        .field(
            "switching?",
            "SwitchingDescriptor",
            "Mod-global switching policy. Optional; omission preserves immediate direct selection, zero cycle dwell, and reload interruption.",
        )
        .field(
            "defaultWeaponPlacement?",
            "WeaponPlacementDescriptor",
            "Optional mod-global first-person weapon placement. It is the lowest authored tier in whole-value resolution: per-instance (future) > per-weapon > character (future) > this default > legacy BASE_OFFSET with zero rotation. v1 supplies no character or per-instance placement. It never changes the third-person hand socket.",
        )
        .field(
            "entities?",
            "Vec<EntityTypeDescriptor>",
            "Engine-global entity-type registrations. Optional; survive level unload and are committed only after manifest validation and required durable-identity validation succeed.",
        )
        .field(
            "factions?",
            "Vec<FactionDescriptor>",
            "Engine-global named faction declarations. Optional; survive level unload and resolve optional archetype `components.faction` names during manifest commit.",
        )
        .field(
            "sentiment?",
            "Vec<FactionSentimentDescriptor>",
            "Optional directional faction relationships. Unlisted pairs preserve compatibility: different factions are hostile and same factions are neutral.",
        )
        .field(
            "factionSentimentDecay?",
            "f32",
            "Optional non-negative default rate for live faction sentiment to ease back to authored baselines. Defaults to 0 (hold); an authored pair `decay` overrides it.",
        )
        .field(
            "fluids?",
            "Vec<FluidDescriptor>",
            "Engine-global fluid declarations, each built with `defineFluid`. Optional; declaration order assigns each fluid's index. A rejected entry warns and is skipped without rejecting the manifest. A hot reload may add, remove or retune fluids.",
        )
        .field(
            "ghostStyles?",
            "Vec<GhostStyleDescriptor>",
            "Engine-global ghost style declarations, each built with `defineGhostStyle`. Optional. A rejected entry warns and is skipped without rejecting the manifest. A hot reload may add, remove or retune styles.",
        )
        .field(
            "uiTrees?",
            "Vec<ModUiTree>",
            "Script-registered UI trees (name + `AnchoredTree` + optional `alwaysOn` / `hideBelow`). Optional; malformed entries are logged and skipped without aborting boot.",
        )
        .field(
            "presentationTemplates?",
            "Vec<PresentationTemplate>",
            "Passive world-presentation templates. They never participate in modal UI input or focus.",
        )
        .field(
            "presentationOverlays?",
            "PresentationOverlay",
            "One fact-driven enemy-status overlay, shared from the host in co-op and local in single player; arrays and malformed descriptors are ignored with a warning.",
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
            "uiImages?",
            "UiImageMap",
            "UI images: image name → PNG path relative to the mod root. Optional. Each loads into the UI image registry under its name, so any tree can show it with `Image({ asset: name })` or a `Tree` `background`. Most load at mod init and after a hot reload; an image only loading trees draw (a map's `loadingTree`, `loading.tree`, or your `loadingScreen`) loads in the background while its loading screen shows and is released when the load ends, unless a level's own trees draw it too. Names beginning `engine/` are reserved. A reserved name, a non-string value, a path that leaves the mod, a missing file, or an undecodable PNG warns and skips that entry.",
        )
        .field(
            "loading?",
            "ModLoading",
            "Mod-wide loading screen. Optional; omission shows the engine fallback loading screen.",
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
            "Mod-global trigger events, each a standing rule keyed by tag: build each with `defineTriggerEvent({ tag, event, fire, levels? })`. Optional; `levels` selects the map tags it binds in. A volume-keyed entry (a trigger member's `on`) belongs in `setupLevel` and is rejected here.",
        )
        .field(
            "playerEvents?",
            "Vec<PlayerEventDescriptor>",
            "Mod-global player events, each a player event built with `players().on` (Luau: `players():on`) from `becomes(cond)` or `ceases(cond)`, a `fire` list and `{ levels? }`. Optional; evaluated on the host once per player per tick, and `levels` selects the map tags it installs in. Mod-global entries compose before a level's own; a repeated condition, edge and reaction binds once.",
        )
        .field(
            "triggerPools?",
            "Vec<TriggerPoolDescriptor>",
            "Trigger-volume arming pools. Optional; compose by level tags.",
        )
        .field(
            "stores?",
            "Vec<StoreDeclaration>",
            "Engine-global state-store declarations resolved from `defineStore(...)` handles by `defineMod`. Optional; commit atomically only after manifest validation and required durable-identity validation succeed, and preserve existing values when the schema is identical.",
        )
        .finish();
}

/// Engine command IDs an `input` block may key, with their hover docs. The
/// set is engine-closed and mirrors the binary's `input::commands::Command`
/// table; a drift guard beside that table compares it against the registered
/// `CommandId` union.
const COMMAND_IDS: &[(&str, &str)] = &[
    (
        "move_forward",
        "Move forward. Digital; a half-axis stick input carries its magnitude. Accepts `press` or `hold`.",
    ),
    (
        "move_back",
        "Move backward. Digital; a half-axis stick input carries its magnitude. Accepts `press` or `hold`.",
    ),
    (
        "move_left",
        "Strafe left. Digital; a half-axis stick input carries its magnitude. Accepts `press` or `hold`.",
    ),
    (
        "move_right",
        "Strafe right. Digital; a half-axis stick input carries its magnitude. Accepts `press` or `hold`.",
    ),
    (
        "move_up",
        "Fly-cam up. Dev-only: always bound to engine defaults and hidden from the controls panel.",
    ),
    (
        "move_down",
        "Fly-cam down. Dev-only: always bound to engine defaults and hidden from the controls panel.",
    ),
    (
        "look_x",
        "Look horizontally; positive looks right. Analog: accepts axes only (`mouse_x`, stick axes) and only `press`.",
    ),
    (
        "look_y",
        "Look vertically; positive looks up. Analog: accepts axes only (`mouse_y`, stick axes) and only `press`.",
    ),
    ("sprint", "Sprint. Accepts `press` or `hold`."),
    ("jump", "Jump."),
    (
        "dash",
        "Dash. Relevant only when a movement descriptor declares dash.",
    ),
    (
        "crouch",
        "Crouch. Accepts `press` or `hold`. Relevant only when a movement descriptor declares crouch.",
    ),
    ("use", "Use or interact."),
    ("drop", "Drop the wielded item."),
    ("shoot", "Primary fire. Accepts only `press`."),
    (
        "alt_fire",
        "Secondary fire. Accepts only `press`. Relevant only when a weapon declares a secondary activation.",
    ),
    (
        "reload",
        "Reload. Relevant only when a weapon uses a magazine resource.",
    ),
    ("select_wieldable_1", "Select wieldable slot 1."),
    ("select_wieldable_2", "Select wieldable slot 2."),
    ("select_wieldable_3", "Select wieldable slot 3."),
    ("select_wieldable_4", "Select wieldable slot 4."),
    ("select_wieldable_5", "Select wieldable slot 5."),
    ("select_wieldable_6", "Select wieldable slot 6."),
    ("select_wieldable_7", "Select wieldable slot 7."),
    ("select_wieldable_8", "Select wieldable slot 8."),
    ("select_wieldable_9", "Select wieldable slot 9."),
    ("select_wieldable_10", "Select wieldable slot 10."),
    (
        "cycle_wieldable_next",
        "Cycle to the next wieldable, one step per wheel notch or press. Accepts only `press`.",
    ),
    (
        "cycle_wieldable_previous",
        "Cycle to the previous wieldable, one step per wheel notch or press. Accepts only `press`.",
    ),
    (
        "toggle_last_wieldable",
        "Switch back to the previously wielded item.",
    ),
    (
        "nav_up",
        "Menu: move focus up. UI commands are always shown.",
    ),
    ("nav_down", "Menu: move focus down."),
    ("nav_left", "Menu: move focus left."),
    ("nav_right", "Menu: move focus right."),
    ("nav_next", "Menu: move focus to the next control."),
    ("nav_prev", "Menu: move focus to the previous control."),
    (
        "nav_tab_next",
        "Menu: activate the next tab, wrapping. Steps Next in a menu with no tabs.",
    ),
    (
        "nav_tab_prev",
        "Menu: activate the previous tab, wrapping. Steps Prev in a menu with no tabs.",
    ),
    (
        "nav_confirm",
        "Menu: confirm. Must stay bound on each device class, and cannot be hidden.",
    ),
    (
        "nav_cancel",
        "Menu: cancel or back. Must stay bound on each device class, and cannot be hidden.",
    ),
    (
        "nav_menu",
        "Open the pause menu. Must stay bound on each device class, and cannot be hidden.",
    ),
    ("nav_options", "Menu: options."),
    (
        "text_backspace",
        "On-screen keyboard: backspace, live while a text-entry menu is on top.",
    ),
    (
        "text_space",
        "On-screen keyboard: space, live while a text-entry menu is on top.",
    ),
    (
        "text_commit",
        "On-screen keyboard: commit, live while a text-entry menu is on top.",
    ),
];

fn register_input_types(registry: &mut PrimitiveRegistry) {
    let mut command_ids = registry
        .register_enum("CommandId")
        .doc("Stable ID of an engine command that an `input` block can label, show or hide, and bind. The set is engine-closed.");
    for &(id, doc) in COMMAND_IDS {
        command_ids = command_ids.variant(id, doc);
    }
    command_ids.finish();
    registry
        .register_enum("InputActivator")
        .doc("When a binding fires. Each command accepts a fixed set; an activator outside it is diagnosed and that command's device class falls back to the engine default.")
        .variant("press", "Fire on the press. This is the default.")
        .variant("release", "Fire on release.")
        .variant("tap", "Fire on release when the input was held no longer than `threshold`.")
        .variant("hold", "Fire once the input has been held for `threshold`.")
        .finish();
    registry
        .register_type("ModInputBinding")
        .doc("One default binding for a command. Players rebind the input only: this input keeps its activator wherever it sits, and an input that replaces it takes its activator. A wheel notch always fires on press.")
        .field(
            "input",
            "String",
            "Physical input name: a W3C `KeyboardEvent.code` (`KeyW`, `ShiftLeft`), a mouse name (`mouse_left`, `wheel_up`, `mouse_x`), or a gamepad position (`south`, `left_shoulder`, `left_stick_press`, `left_stick_x`, `left_stick_up`). An unknown name is diagnosed and the device class falls back.",
        )
        .field(
            "activator?",
            "InputActivator",
            "When the binding fires. Optional; defaults to `\"press\"`.",
        )
        .field(
            "threshold?",
            "f32",
            "Seconds: a `tap`'s maximum or a `hold`'s minimum. Finite and greater than 0. Clamped to 0.05–5 s. Optional; defaults to 0.2, scaled by the player's hold-timing setting.",
        )
        .finish();
    registry
        .register_type("ModInputCommand")
        .doc("Author settings for one command. Every field is optional.")
        .field(
            "label?",
            "String",
            "Name shown in the controls panel. Optional.",
        )
        .field(
            "category?",
            "String",
            "Controls-panel group heading. Optional.",
        )
        .field(
            "order?",
            "f32",
            "Sort position within the category. Optional.",
        )
        .field(
            "show?",
            "bool",
            "`true` forces the command shown and bound, `false` hidden and unbound, overriding relevance derived from the mod's data. Optional. Ignored, with a warning, on every UI command and on the dev-only `move_up` and `move_down`.",
        )
        .field(
            "keyboardMouse?",
            "Vec<ModInputBinding>",
            "Default keyboard and mouse bindings. Optional; omission keeps the engine default, and an empty list leaves the command unbound.",
        )
        .field(
            "gamepad?",
            "Vec<ModInputBinding>",
            "Default gamepad bindings. Optional; omission keeps the engine default, and an empty list leaves the command unbound.",
        )
        .finish();
    registry
        .register_type("ModInputCommands")
        .doc("Command settings keyed by command ID. Commands left out keep their engine defaults.")
        .alias(
            "Partial<Record<CommandId, ModInputCommand>>",
            "{ [string]: ModInputCommand }",
        )
        .finish();
    registry
        .register_type("ModInputGlyphs")
        .doc("Glyph art directory per device family. A glyph's asset is `<dir>/<input>`, for example `ui/glyphs/xbox/south`. Missing art draws the input's label (`east` draws \"EAST\", `KeyW` draws \"W\").")
        .field("keyboardMouse?", "String", "Keyboard and mouse glyph directory. Optional.")
        .field("xbox?", "String", "Xbox-layout glyph directory, also used for unrecognized pads. Optional.")
        .field("playstation?", "String", "PlayStation glyph directory. Optional.")
        .field("nintendo?", "String", "Nintendo glyph directory. Optional.")
        .finish();
    registry
        .register_type("ModInput")
        .doc("The game's commands, default bindings, and glyph art. Each command and device class is validated on its own: an unknown command ID, unknown input, refused activator, or a default leaving `nav_confirm`, `nav_cancel`, or `nav_menu` unbound is diagnosed, and that command and device class fall back to the engine default.")
        .field(
            "commands?",
            "ModInputCommands",
            "Command settings keyed by command ID. Optional; omission leaves every command on its engine defaults.",
        )
        .field(
            "glyphs?",
            "ModInputGlyphs",
            "Glyph art per device family. Optional.",
        )
        .finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::{FactionRegistry, slot_table::StoreDeclarationSet};
    use postretro_foundation::{FluidRegistry, GhostStyleRegistry};
    use postretro_scripting_core::data_descriptors::{
        ModFontAssets, ModThemeTokens, PresentationOverlay, PresentationTemplate,
        SwitchingDescriptor,
    };
    use postretro_scripting_core::primitives_registry::TypeShape;
    use postretro_scripting_core::runtime::{
        ModAudioProfile, ModInputBinding, ModInputBlock, ModInputCommand, ModInputGlyphs,
        ModLoading, ModManifestResult, ModMoverDefaults, ModRenderProfile,
    };
    use std::collections::BTreeMap;

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
            audio: ModAudioProfile::default(),
            input: None,
            switching: SwitchingDescriptor::default(),
            default_weapon_placement: None,
            entities: Vec::new(),
            factions: FactionRegistry::default(),
            sentiment: Vec::new(),
            faction_sentiment_decay: 0.0,
            entity_faction_names: Vec::new(),
            fluids: FluidRegistry::default(),
            ghost_styles: GhostStyleRegistry::default(),
            ui_trees: Vec::new(),
            presentation_templates: Vec::<PresentationTemplate>::new(),
            presentation_overlays: Vec::<PresentationOverlay>::new(),
            theme: ModThemeTokens::default(),
            frontend: None,
            ui_images: BTreeMap::new(),
            loading: ModLoading::default(),
            fonts: ModFontAssets::default(),
            maps: Vec::new(),
            reactions: Vec::new(),
            crossings: Vec::new(),
            events: Vec::new(),
            trigger_events: Vec::new(),
            trigger_pools: Vec::new(),
            store_declarations: StoreDeclarationSet::default(),
            player_events: Vec::new(),
        };
        let expected_fields: &[&str] = &[
            "name",
            "id",
            "version",
            "render",
            "movers",
            "audio",
            "input",
            "switching",
            "defaultWeaponPlacement",
            "entities",
            "factions",
            "sentiment",
            "factionSentimentDecay",
            "fluids",
            "ghostStyles",
            "uiTrees",
            "presentationTemplates",
            "presentationOverlays",
            "theme",
            "frontend",
            "uiImages",
            "loading",
            "fonts",
            "maps",
            "reactions",
            "crossings",
            "events",
            "triggerEvents",
            "playerEvents",
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
            (
                "RenderProfile",
                ["bloom?", "reflections?", "ghosts?"].as_slice(),
            ),
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

    // Drift guard: the SDK's `ReflectionStyle` union is the runtime enum's
    // authored names, in order. Each variant maps through an exhaustive match,
    // so a new runtime style fails to compile until it is registered.
    #[test]
    fn reflection_render_profile_sdk_types_derive_from_the_runtime_style() {
        use postretro_scripting_core::runtime::ModReflectionStyle;
        let mut registry = PrimitiveRegistry::new();
        register_sdk_type(&mut registry);

        let style = registry
            .iter_types()
            .find(|registered| registered.name == "ReflectionStyle")
            .expect("ReflectionStyle must be registered");
        let TypeShape::StringEnum { variants } = &style.shape else {
            panic!(
                "ReflectionStyle must be a StringEnum, got {:?}",
                style.shape
            );
        };
        let names: Vec<&str> = variants.iter().map(|variant| variant.name).collect();
        let expected: Vec<&str> = ModReflectionStyle::ALL
            .into_iter()
            .map(|style| {
                let name = match style {
                    ModReflectionStyle::Pixel => "pixel",
                    ModReflectionStyle::Smooth => "smooth",
                };
                assert_eq!(style.authored_name(), name);
                name
            })
            .collect();
        assert_eq!(names, expected);
        assert_eq!(
            ModReflectionStyle::default().authored_name(),
            names[0],
            "the default is listed first, as its doc says"
        );

        let profile = registry
            .iter_types()
            .find(|registered| registered.name == "ReflectionRenderProfile")
            .expect("ReflectionRenderProfile must be registered");
        let TypeShape::Struct { fields } = &profile.shape else {
            panic!("ReflectionRenderProfile must be a Struct");
        };
        let names: Vec<&str> = fields.iter().map(|field| field.name).collect();
        assert_eq!(names, ["style?"]);
    }

    // Drift guard: the SDK's `GhostTransparency` union is the runtime enum's
    // authored names, in order, default first. An exhaustive match maps each
    // variant, so a new runtime mode fails to compile until it is registered.
    #[test]
    fn ghost_transparency_sdk_type_derives_from_the_runtime_mode() {
        use postretro_scripting_core::runtime::ModGhostTransparency;
        let mut registry = PrimitiveRegistry::new();
        register_sdk_type(&mut registry);
        let mode = registry
            .iter_types()
            .find(|registered| registered.name == "GhostTransparency")
            .expect("GhostTransparency must be registered");
        let TypeShape::StringEnum { variants } = &mode.shape else {
            panic!(
                "GhostTransparency must be a StringEnum, got {:?}",
                mode.shape
            );
        };
        let names: Vec<&str> = variants.iter().map(|variant| variant.name).collect();
        let expected: Vec<&str> = ModGhostTransparency::ALL
            .into_iter()
            .map(|mode| {
                let name = match mode {
                    ModGhostTransparency::Blend => "blend",
                    ModGhostTransparency::Dither => "dither",
                };
                assert_eq!(mode.authored_name(), name);
                name
            })
            .collect();
        assert_eq!(names, expected);
        assert_eq!(ModGhostTransparency::default().authored_name(), names[0]);
    }

    // The ghost style descriptor declares `name` required and every other
    // field optional, matching the drain's floor-filling.
    #[test]
    fn ghost_style_descriptor_sdk_type_requires_only_name() {
        let mut registry = PrimitiveRegistry::new();
        register_sdk_type(&mut registry);
        let style = registry
            .iter_types()
            .find(|registered| registered.name == "GhostStyleDescriptor")
            .expect("GhostStyleDescriptor must be registered");
        let TypeShape::Struct { fields } = &style.shape else {
            panic!("GhostStyleDescriptor must be a Struct");
        };
        let names: Vec<&str> = fields.iter().map(|field| field.name).collect();
        assert_eq!(
            names,
            [
                "name",
                "tint?",
                "opacity?",
                "saturation?",
                "audio?",
                "impactAudio?"
            ]
        );
    }

    // `surfaceReflectivity` is an optional field of the fluid descriptor,
    // beside `surfaceOpacity`.
    #[test]
    fn fluid_descriptor_sdk_type_declares_optional_surface_reflectivity() {
        let mut registry = PrimitiveRegistry::new();
        register_sdk_type(&mut registry);
        let fluid = registry
            .iter_types()
            .find(|registered| registered.name == "FluidDescriptor")
            .expect("FluidDescriptor must be registered");
        let TypeShape::Struct { fields } = &fluid.shape else {
            panic!("FluidDescriptor must be a Struct");
        };
        let names: Vec<&str> = fields.iter().map(|field| field.name).collect();
        let opacity = names
            .iter()
            .position(|name| *name == "surfaceOpacity")
            .expect("surfaceOpacity");
        assert_eq!(names[opacity + 1], "surfaceReflectivity?");
    }

    #[test]
    fn audio_profile_sdk_types_are_closed_and_optional() {
        let mut registry = PrimitiveRegistry::new();
        register_sdk_type(&mut registry);

        let curve = registry
            .iter_types()
            .find(|registered| registered.name == "AttenuationCurve")
            .expect("AttenuationCurve must be registered");
        match &curve.shape {
            TypeShape::StringEnum { variants } => {
                let names: Vec<&str> = variants.iter().map(|variant| variant.name).collect();
                assert_eq!(names, ["linear", "quadratic"]);
            }
            other => panic!("AttenuationCurve must be a StringEnum, got {other:?}"),
        }

        for (name, expected_fields) in [
            (
                "AudioAttenuation",
                ["minDistance?", "maxDistance?", "curve?"].as_slice(),
            ),
            (
                "AudioGhostProfile",
                ["volume?", "highPassHz?", "lowPassHz?", "drive?"].as_slice(),
            ),
            ("AudioProfile", ["attenuation?", "ghost?"].as_slice()),
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

    #[test]
    fn input_sdk_types_mirror_the_drained_input_block() {
        let mut registry = PrimitiveRegistry::new();
        register_sdk_type(&mut registry);

        let activator = registry
            .iter_types()
            .find(|registered| registered.name == "InputActivator")
            .expect("InputActivator must be registered");
        match &activator.shape {
            TypeShape::StringEnum { variants } => {
                let names: Vec<&str> = variants.iter().map(|variant| variant.name).collect();
                assert_eq!(names, ["press", "release", "tap", "hold"]);
            }
            other => panic!("InputActivator must be a StringEnum, got {other:?}"),
        }

        // Compile-time anchor: a field added to or renamed on the drained Rust
        // structs breaks this literal, forcing the script lists below to follow.
        let _shape_anchor = ModInputBlock {
            commands: vec![ModInputCommand {
                id: String::new(),
                label: None,
                category: None,
                order: None,
                show: None,
                keyboard_mouse: Some(vec![ModInputBinding {
                    input: String::new(),
                    activator: None,
                    threshold: None,
                }]),
                gamepad: None,
            }],
            glyphs: ModInputGlyphs {
                keyboard_mouse: None,
                xbox: None,
                playstation: None,
                nintendo: None,
            },
        };
        // Script names of the fields each drained Rust struct carries; the
        // drain reads exactly these keys.
        for (name, expected_fields) in [
            ("ModInput", ["commands?", "glyphs?"].as_slice()),
            (
                "ModInputCommand",
                [
                    "label?",
                    "category?",
                    "order?",
                    "show?",
                    "keyboardMouse?",
                    "gamepad?",
                ]
                .as_slice(),
            ),
            (
                "ModInputBinding",
                ["input", "activator?", "threshold?"].as_slice(),
            ),
            (
                "ModInputGlyphs",
                ["keyboardMouse?", "xbox?", "playstation?", "nintendo?"].as_slice(),
            ),
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
