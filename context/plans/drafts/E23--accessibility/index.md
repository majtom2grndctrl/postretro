# Epic 23 — Accessibility (Epic hub)

> **Status:** draft — epic index. Records cross-cutting decisions and each unit's contract; no task decomposition. Each unit gets its own brief in a sibling `E23--<unit-slug>/` folder, drafted when the unit comes up.
> **Research:** `research.md` — standards thresholds, OS-preference reach, dependency costs, source findings.
> **Related:** `context/lib/ui.md` §2, §4, §6 · `context/lib/player_options.md` · `context/lib/input.md` · `context/lib/audio.md` · `context/lib/boot_sequence.md` §1 · `context/research/ui-layer.md` §15, §19 · roadmap Epic 12, Epic 13, Epic 16 §Weapon Feel.

## Goal

Meet the Game Accessibility Guidelines basic tier and the common FPS accessibility failures as engine mechanisms. Mods author where content must (theme variants, captions); OS accessibility settings seed defaults; menus reach native screen readers; gamepad menu navigation meets console conventions.

## Scope

### In scope

- **U1 Preferences and comfort floor** — accessibility preference substrate with OS seeding, flash limiter, reduce motion, per-bus volume, mono audio.
- **U2 Visual accessibility** — theme variants, tokenized engine visuals, authored focus visuals, contrast diagnostic, text scale.
- **U3 Gamepad and input** — focus-group fix, console menu conventions, full remapping, gamepad look options, hold/toggle.
- **U4 Screen reader** — engine accessibility snapshot projected to native assistive technology.
- **U5 Hearing and directional cues** — captions, subtitles, directional damage indicator, sound-direction cues.

### Out of scope

- Co-op text chat / ping system — a new capability, not an accommodation over an existing one; no chat exists to make accessible. Sibling roadmap item.
- Aim assist — owned by roadmap Epic 16 §Weapon Feel.
- Difficulty and game speed — scripts own difficulty (`scripting.md`, "engine owns the floor; scripts own the taste").
- Self-voicing TTS — door kept open behind U4's snapshot, not built.
- Reading OS flashing-lights, caption on/off, mono audio, or the Windows screen-reader flag — no safe API (`research.md` §3); in-game options only.
- Whole-UI reference-space scaling — owner chose text scale.
- Mod-declared input actions — the action set stays engine-closed.

## Direction

**Problem.** The UI epic built the authoring side of accessibility (G2 metadata: names, roles, `Announce`, selected/checked) and deliberately deferred its consumption. No epic owns player-side accommodations. So the metadata has no reader, and the mechanisms only the engine can supply do not exist: OS preference seeding, a photosensitivity floor, an assistive-technology bridge, rebinding, captions, gamepad menu conventions.

**Prior commitments.**
- `research/ui-layer.md` §15 — accessibility as a type-system precondition; `Announce` is a node. Consumed, not changed.
- `research/ui-layer.md` §19 — screen readers "out of scope for v1; revisit before public release." This epic is that revisit.
- `done/M13--sdk-typesafety-a11y` — selected/checked ride `FocusRect`, "where a screen reader will later" read them. U4's snapshot reads them there.
- `ui.md` §6 (no per-frame script callbacks) and §4 (activation resolves to named reactions or reserved UI actions). AT actions route to the existing activation and slider-step paths, never a script callback.
- `ui.md` §2 — theme precedence is engine default < mod override. **Diverges:** a third tier is added — engine default → mod base → selected variant. The owner chose this (D3): mods ship accessible variants, the player selects one, and the engine supplies a high-contrast fallback when the mod ships none. The variant tier sits above the mod so a player's accommodation is never shadowed by the content it accommodates.
- `player_options.md` §4 — `options.*` slots are seeded when the options menu opens, not kept in continuous sync. **Diverges** for accessibility fields: mods must honor reduce motion and similar settings in their own content during play (D2), so these slots stay current while the menu is closed.
- `player_options.md` §4 — an unknown enum value fails the whole-document parse; the `surface_depth_quality` alias is the precedent for treating stored values as player data. U1 generalizes it: accessibility fields and U3 bindings fall back per field.
- `player_options.md` §6 — "keybind remapping … separate spec." U3 is that spec.
- `boot_sequence.md` §Window visibility — window created visible to avoid a Windows hang. **Diverges:** `accesskit_winit` requires a not-yet-visible window at adapter construction. U4 creates the window hidden and shows it immediately after the adapter exists — before the first redraw is requested, unlike the reverted reveal-after-first-present scheme that starved on `RedrawRequested`. Bound to a manual Windows boot proof (AC 26); a hang blocks U4 and returns to the owner.
- `done/movement--view-feel` D6 — the comfort scale lives in `PlayerOptions`, not descriptors. Followed: reduce motion drives `view_feel_scale`.
- `audio.md` / `networking.md` — sound is host-local presentation; no sound key on the wire. Captions derive client-side. The directional damage bearing is new owner-addressed presentation data, not a sound key.
- `index.md` "no `unsafe`" — new dependencies (`mundy`, `accesskit_winit`) keep `unsafe` internal; the engine adds none.
- E12 step 1 (Positional sound events, ready) — U5 builds on its spatial chokepoint without reopening its decisions.

**Alternatives rejected.**
- *Screen reader only* (the literal Epic 13 deferred line) — leaves every GAG basic-tier gap open.
- *Self-voicing TTS instead of native AT* — overrides the player's own screen-reader voice and rate, and duplicates what AccessKit exposes. Kept as an open door behind the same snapshot.
- *Mod-opt-in authority for preferences* — cannot guarantee photosensitivity safety.
- *Whole-UI reference-space scale* — owner chose text scale.
- *One monolithic spec* — five subsystems and several executors exceed one execution contract.
- *Independent per-unit plans with no index* — the preference model, OS seeding, and authority rules would be re-decided in every unit.

**One-way doors.**

| Door | Decided in | Undo cost |
|---|---|---|
| Persisted `settings.toml` shape for bindings and OS-unset fields | U1 (unset fields), U3 (bindings) | Lives on players' machines; a later change needs migration or per-field fallback. |
| `themeVariants` manifest shape, caption authoring surface, new readable `options.*` slot names | U2, U5, U1–U5 | Modder-facing primitive surface (`index.md` "Primitive surface is a contract"). |
| Focus groups admit only interactive widgets | U3 | Trees relying on non-interactive group stops change behavior. Pre-stable; acceptable. |
| `mundy`, `accesskit_winit` dependencies | U1, U4 | Reversible: each sits behind an app-side seam. |

## Cross-cutting decisions

**Preference model (D2).** An `accessibility` group in `PlayerOptions`, exposed as readable `options.*` slots. Each OS-seedable field stores *unset* until the player changes it. Unset resolves to the OS value, else the engine default. OS change events update unset fields live. Stored values use a tolerant format: an unknown value falls back for that field alone; the file is never discarded. U1 builds the substrate; later units add their own fields to it. Resolution order is Invariant I1.

**Authority (D3).** Mods present accessible variants of their theme; players enable them in options; OS settings are the default before the player chooses. High contrast with no mod variant → an engine high-contrast token set layered over the mod theme. Layering: engine default → mod base → selected variant (mod's, or the engine fallback). Applied at the single theme chokepoint and preserved across staged reload (Invariant I6).

**Photosensitivity floor (D4).** Engine flash limiter, on by default. The player may disable it; mods cannot. Enforced engine-side on `screen.flash` and `screen.vignette`: ≤ 3 flashes per second, saturated-red transitions (R/(R+G+B) ≥ 0.8) desaturated, full-screen intensity capped. Authored world-light animation is not limited.

**Each unit ships its dev-mod consumer in the same unit (D1).** A unit is not done until `content/dev` exercises it.

## Units

### U1 — Preferences and comfort floor

**Purpose.** The substrate every other unit writes into, plus the comfort mechanisms that need no UI rework.

**Contents.**
- `accessibility` group in `PlayerOptions`; readable `options.*` slots current during play; tolerant per-field storage; unset/OS/default resolution (Cross-cutting: preference model).
- OS reader: `mundy` for contrast, reduced motion, color scheme; Windows `TextScaleFactor` via the existing `windows` crate. Change events update unset fields live.
- Flash limiter (Cross-cutting: photosensitivity floor). The options menu is mod-authored and writes `options.*` through `setState`, so a script-writable limiter slot would let a mod disable it; the U1 brief pins the engine-owned path by which only the player turns it off.
- Reduce motion (D5): one switch, OS-seeded, driving `view_feel_scale`, a new screen-shake scale, and UI tween snapping; per-effect sliders beneath it.
- Per-bus volume (Master / SFX / Music / UI) and a mono fold as a kira main-track custom effect with a crossfaded toggle (D10).

**Depends on.** Nothing.

**Dev-mod consumer.** `content/dev` options menu gains the accessibility group; `arena-lights.ts` low-health flash and vignette exercise the limiter.

**Key acceptance.** AC 1–7.

**Brief.** `context/plans/drafts/E23--preferences-comfort-floor/` — drafted when the unit comes up.

### U2 — Visual accessibility

**Purpose.** Make every engine-drawn color themeable, let players pick accessible variants, and make text scale. Absorbs the retired `ui-focus-accessibility-visuals` focus-visual track (D7).

**Contents.**
- Theme variants per the authority decision: `themeVariants` manifest key; variant selection stored as a player option outside the mod override.
- Tokenize engine literals: interactive label color, slider track / thumb / fill (fill decoupled from `focus.ring`). New text, button, and panel tokens.
- Authored focus visuals: background, text color, outline. An omitted focus visual keeps the engine outline.
- Contrast floor as a theme-drain diagnostic: text ≥ 4.5:1, focus indicator ≥ 3:1 (`research.md` §2). A warning, never a load failure.
- Colorblind-safe variant slot.
- Text scale (D6): text-only multiplier ~100–200% through the text measure path, authored max-width wrapping, applied to presentation-layer text too. OS-seeded from Windows text scale through U1.

**Depends on.** U1 — variant selection and text scale are fields on U1's substrate, OS-seeded through its reader.

**Dev-mod consumer.** Dev menus (`frontend-menu.ts`, `pause-menu.ts`), `combat-presentation.ts`, and `arena-lights.ts` migrate color literals to tokens; the dev mod ships a high-contrast variant.

**Key acceptance.** AC 8–12.

**Brief.** `context/plans/drafts/E23--visual-accessibility/` — drafted when the unit comes up.

### U3 — Gamepad and input

**Purpose.** Console-convention menu navigation and full input remapping (D8).

**Contents.**
- Focus groups admit only interactive widgets — a contract change; the test asserting the old behavior changes with it.
- `restoreOnReturn` on by default; engine-default hold-to-repeat; slider hold-repeat with acceleration.
- LB/RB tab/page intent and a tabbed pattern; scroll container with scroll-into-view.
- Device-family button-prompt glyphs following input mode; confirm/cancel layout swap option; on-screen-keyboard shortcuts.
- Confirmation dialogs for destructive actions, default focus on the safe choice.
- Full remapping — keyboard/mouse and gamepad, UI nav actions included — with conflict detection, reset, a guard keeping confirm and cancel reachable, and a raw-capture input path.
- Gamepad look sensitivity, dead zone, invert-Y; generalized hold/toggle (sprint) on the crouch-mode pattern.

**Depends on.** Nothing. Bindings and gamepad options are fields on U1's substrate; U1 and U3 run concurrently, so whichever brief lands first pins the tolerant per-field storage and the other adopts it.

**Dev-mod consumer.** Dev menus restructured: options as a spatial grid with tabs, level select spatial; EXIT/QUIT gain confirmation; options menu gains remapping.

**Key acceptance.** AC 13–20.

**Brief.** `context/plans/drafts/E23--gamepad-input/` — drafted when the unit comes up.

### U4 — Screen reader

**Purpose.** Native assistive-technology support for menus (D9).

**Contents.**
- `accesskit_winit`, always on for desktop. Window created hidden, shown right after the adapter exists.
- Engine-built accessibility snapshot: resolved names (inline label and `labelledBy`), roles, states, bounds, slider values, `Announce` live regions. Behind an app-side adapter seam; AccessKit types never enter descriptors or the SDK (Invariant I3).
- AT actions (activate, increment/decrement) re-enter the existing activation and slider-step paths.
- Self-voicing TTS stays an open door behind the same snapshot.

**Depends on.** U3's focus-group fix — the snapshot's focusable set is the fixed focus export.

**Dev-mod consumer.** Frontend, options, pause, and on-screen keyboard carry complete names and roles; an `Announce` reports level-load or state changes in a dev menu.

**Key acceptance.** AC 21–27.

**Brief.** `context/plans/drafts/E23--screen-reader/` — drafted when the unit comes up.

### U5 — Hearing and directional cues

**Purpose.** Make audio information visible (D10).

**Contents.**
- Captions keyed per sound asset — covers today's `playSound` and E12's per-event sound fields without reshaping them — plus a scripted subtitle primitive with speaker.
- Caption box: background opacity option, size tied to text scale, direction arrow computed where the sound plays and updates.
- Directional damage indicator from a player bearing computed at the damage chokepoint, delivered owner-addressed to co-op clients.
- Sound-direction cues for positional sounds.
- Caption keys never go on the wire; captions derive client-side from sounds the client plays (Invariant I4).

**Depends on.** Damage-direction part: U1 (its option field). Caption and cue part: E12 step 1's spatial chokepoint (positional plays carry the emitter point the arrow needs) and U2 (caption size follows text scale).

**Dev-mod consumer.** Dev sounds (`sfx/test_tone` and E12's dev descriptor sounds) gain captions; a dev script uses the subtitle primitive with a speaker; the dev HUD shows the damage indicator.

**Key acceptance.** AC 28–31.

**Brief.** `context/plans/drafts/E23--hearing-cues/` — drafted when the unit comes up.

## Acceptance criteria

Unit tag in brackets. "Manual" rows are verified by a person on real hardware.

**U1**
- [ ] 1. [U1] An OS-seedable setting the player never changed follows the OS value at boot and follows a live OS change without restart. Once the player changes it, a later OS change leaves it unchanged. With no OS value available it resolves to the engine default.
- [ ] 2. [U1] A `settings.toml` with an unrecognized value in one accessibility field loads every other setting intact; that field falls back to unset (or its default) with a warning. A file with no accessibility group loads with every field unset. Saving writes a never-changed field as unset, not as the value it resolved to.
- [ ] 3. [U1] A mod script reads each accessibility option's resolved value during play with the options menu closed, and sees a player or OS change without a level reload.
- [ ] 4. [U1] With the limiter on (the first-boot default): four flashes authored inside 1 s put at most three on screen, while three inside 1 s all appear; a saturated-red flash or vignette appears desaturated while a non-red one keeps its authored hue; full-screen intensity never exceeds the cap. With the limiter off, authored flash and vignette values reach the screen unchanged. Manual: the limiter visibly tames a dev-mod strobe.
- [ ] 5. [U1] No mod-authored content — reactions, slot writes, manifest data — turns the limiter off or raises its limits; only the player's control does.
- [ ] 6. [U1] Reduce motion on: a UI tween reaches its target the frame it starts, and view feel and screen shake apply the reduced scale. Off: each effect follows its own per-effect slider, and those slider values survive toggling the switch.
- [ ] 7. [U1] Changing one bus volume changes only that bus's output level. Mono on: a hard-panned source outputs equal left and right; off: stereo returns. Toggling mono mid-sound produces no click.

**U2**
- [ ] 8. [U2] Selecting a mod-shipped variant restyles the UI from it; tokens the variant omits resolve from the mod base, then the engine default. High contrast with no mod variant applies the engine high-contrast set over the mod theme. Deselecting restores the mod base. The selection survives a staged reload, including one that changes the mod's base theme; a reload that removes the mod's high-contrast variant falls back to the engine set.
- [ ] 9. [U2] Theme drain warns on a text/background pair at 3:1 and is silent at 4.5:1; warns on a focus-indicator pair below 3:1 and is silent at 3:1. The theme still loads either way.
- [ ] 10. [U2] Overriding the button-label, slider track, thumb, and fill tokens restyles those parts; changing the focus-ring token no longer changes slider fill. Background, text-color, and outline focus visuals each render on focus; an omitted focus visual draws the engine outline.
- [ ] 11. [U2] At 200% text scale, text renders at twice its size, containers reflow to fit it, and text with an authored max width wraps at that width; presentation-layer text scales too. At 100% layout matches the pre-epic layout. A scale change applies on the next frame without restart.
- [ ] 12. [U2] Dev frontend, options, and pause menus draw no literal colors; under the dev high-contrast variant the contrast diagnostic reports no warnings. Manual: high-contrast and colorblind-safe variants are legible in play.

**U3**
- [ ] 13. [U3] Inside a focus group, only interactive widgets receive focus: `Text` and `VStack` are skipped; `Button`, `Slider`, and authored-id interactive widgets are reachable. In the dev title menu, nav down from EXIT lands on PLAY.
- [ ] 14. [U3] Closing a submenu returns focus to the widget that opened it with no authoring; a tree that opts out lands on its initial focus.
- [ ] 15. [U3] A tap on a nav direction moves focus one step with zero repeats. A hold moves one step, then repeats after the delay at the interval, and stops on release — with no container authoring. A held slider step accelerates with hold time and clamps at its bounds without overshoot.
- [ ] 16. [U3] A remapped binding round-trips through `settings.toml`. An unknown key string falls back to that binding's default without discarding other bindings or settings. Unbinding the last confirm or last cancel binding is refused. A conflicting assignment is reported. Reset restores defaults. The capture prompt receives keys the UI otherwise swallows.
- [ ] 17. [U3] Gamepad look sensitivity, dead zone, and invert-Y change gamepad look and leave mouse look unchanged. Sprint in toggle mode latches on press and releases on the next press; in hold mode it follows the button.
- [ ] 18. [U3] Button-prompt glyphs match the device family of the last input and switch on the first input from another family. The confirm/cancel swap option swaps both the behavior and the glyphs.
- [ ] 19. [U3] LB/RB switches tabs in a tabbed menu. Moving focus to an off-screen child of a scroll container scrolls it into view. EXIT and QUIT open a confirmation with focus on the safe choice; cancel dismisses without acting. On-screen-keyboard shortcut buttons act without moving focus to the key they stand for.
- [ ] 20. [U3] Manual: a gamepad-only pass completes every dev menu on Xbox and on PlayStation or Nintendo layouts.

**U4**
- [ ] 21. [U4] The accessibility snapshot names a widget from its inline label and from `labelledBy`; a missing `labelledBy` target yields an empty name and a warning, never a panic. Hidden subtrees are absent. Selected, checked, disabled, bounds, and slider value are present.
- [ ] 22. [U4] An `Announce` that becomes visible produces one live-region entry at its priority — polite by default, assertive when authored. Staying visible produces no repeat; a hidden `Announce` produces none.
- [ ] 23. [U4] Activating a button through AT runs the same reaction keyboard confirm runs; a disabled button does nothing. AT increment/decrement steps a slider exactly as one gamepad step does.
- [ ] 24. [U4] A focus change reaches the platform adapter no later than the frame the focus ring shows it; no new focus latency is added.
- [ ] 25. [U4] Generated SDK typedefs and the descriptor wire contain no platform-accessibility type.
- [ ] 26. [U4] Manual: Windows boot with the hidden-at-create window reaches the frontend without hanging.
- [ ] 27. [U4] Manual: NVDA (Windows), VoiceOver (macOS), and Orca (Linux) read and operate frontend, options, pause, and the on-screen keyboard. Binary-size delta of the new dependencies is recorded per platform.

**U5**
- [ ] 28. [U5] A sound with an authored caption shows it while playing; a sound without one shows none. A positional sound's caption carries a direction arrow that tracks the listener's turn; a non-positional sound's carries none. A scripted subtitle shows its speaker. Background opacity 0% draws no box; 100% draws an opaque box. Manual: captions readable at 100% and 200% text scale.
- [ ] 29. [U5] A hit from the player's left shows the damage indicator on the left, from behind shows it behind; damage with no known source position shows no direction. In co-op the damaged player's client receives the bearing and other clients do not.
- [ ] 30. [U5] A co-op client shows captions and cues for sounds it plays locally; U5 adds no caption or sound key to any wire message.
- [ ] 31. [U5] With cues on, a positional sound off-screen produces a cue on its side; a 2D, UI, or music sound produces none; with cues off, none appear.

## Sequencing

**Phase 1 (concurrent):** U1, U3 — independent; U3's focus fix also unblocks U4.
**Phase 2 (concurrent):** U2 after U1 — its fields and OS seeding sit on U1's substrate. U4 after U3's focus-group fix — the snapshot's focusable set is the fixed focus export. U5 damage-direction part after U1 — its option lives on U1's substrate.
**Phase 3:** U5 caption and cue part after E12 step 1 lands its spatial chokepoint — the arrow needs the emitter point — and after U2 — caption size follows text scale.

Each unit is re-grounded against the live tree when its brief is drafted.

## Invariants

| # | Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|---|
| I1 | Preference resolution: player-set > OS value > engine default. An OS change never overrides a player-set value. | U1 | Every unit adding an OS-seedable field (U2 variant, text scale); the save path writing a resolved value as player-set | AC 1, 2 |
| I2 | Flash floor enforced engine-side; no mod bypasses it while enabled. | U1 | Mod-authored options menus writing `options.*`; U2 variants recoloring flash/vignette; any new full-screen effect surface | AC 4, 5 |
| I3 | AccessKit types never enter descriptors or the SDK. | U4 | U2 focus-visual descriptors; U5 caption authoring surface | AC 25 |
| I4 | Caption keys never go on the wire; captions derive client-side from local playback. | U5 | U5 damage-bearing delivery; E12 step 4 peer audio, which may carry sounds but never captions | AC 29, 30 |
| I5 | Focus groups admit only interactive widgets. | U3 | U3 tabs and scroll containers; U4 snapshot built from focus export | AC 13, 21 |
| I6 | The player's selected theme variant survives staged reload. | U2 | Staged-reload theme commit replacing the mod override | AC 8 |
| I7 | Stored settings degrade per field; the file is never discarded. | U1 | U3 bindings; U2 and U5 new enum fields | AC 2, 16 |
| I8 | AT actions route to the existing activation and slider-step paths, never a script callback. | U4 | U3 raw-capture and new intents | AC 23 |

## Boundary inventory

Existing convention: TOML keys snake_case (`player_options.md` §2); slots camelCase under `options.*` (`options.viewFeelScale`). Rust `PlayerOptions` fields match the TOML key. Slots carry the **resolved** value; unset is a storage state, not a slot value.

| Name | TOML key / Rust field | Slot (JS / Luau) | Unit | Still pinned by brief |
|---|---|---|---|---|
| Reduce motion | `reduce_motion` | `options.reduceMotion` | U1 | — |
| Screen-shake scale | `screen_shake_scale` | `options.screenShakeScale` | U1 | range |
| Flash limiter | `flash_limiter` | `options.flashLimiter` | U1 | player-only write path |
| Bus volumes | `master_volume`, `sfx_volume`, `music_volume`, `ui_volume` | `options.masterVolume`, `options.sfxVolume`, `options.musicVolume`, `options.uiVolume` | U1 | unit and range |
| Mono audio | `mono_audio` | `options.monoAudio` | U1 | — |
| OS-unset representation | — | — | U1 | TOML encoding; how a player returns a field to "follow OS" |
| Text scale | `text_scale` | `options.textScale` | U2 | range and step |
| Theme variant | `theme_variant` | `options.themeVariant` (variant id) | U2 | "none" value; whether colorblind-safe has an engine fallback |
| Theme variants manifest key | `ModManifest.theme_variants` | `themeVariants` (same key in Luau) | U2 | map value shape; open vs. closed id set |
| Variant ids | — | `highContrast`, `colorblindSafe` | U2 | — |
| Focus visual field and kinds | — | — | U2 | names |
| New theme tokens (text, button, panel, slider) | — | — | U2 | names |
| Gamepad look | `gamepad_look_sensitivity`, `gamepad_dead_zone`, `gamepad_invert_y` | `options.gamepadLookSensitivity`, `options.gamepadDeadZone`, `options.gamepadInvertY` | U3 | ranges |
| Sprint mode | `sprint_mode` (`hold` / `toggle`) | `options.sprintMode` | U3 | — |
| Confirm/cancel swap | `swap_confirm_cancel` | `options.swapConfirmCancel` | U3 | — |
| Bindings | `[bindings]` table | — | U3 | table shape; physical-input string vocabulary |
| Captions | `captions` | `options.captions` | U5 | — |
| Caption background opacity | `caption_background_opacity` | `options.captionBackgroundOpacity` | U5 | range |
| Damage indicator | `damage_indicator` | `options.damageIndicator` | U5 | — |
| Sound cues | `sound_cues` | `options.soundCues` | U5 | — |
| Caption authoring (per sound asset) and subtitle primitive with speaker | — | — | U5 | names and shape |
| Damage bearing delivery | — | — | U5 | owner-addressed message; any readable `player.*` bearing slot |

## Rough sketch

Key modules per unit, from source at `2542ac2`. Files past ~800 lines that a unit extends are flagged for split-first.

- **U1** — `crates/postretro/src/options/mod.rs` (`PlayerOptions`), `options/bridge.rs` (`OptionsBridge`, slot constants), `crates/entities/src/engine_state_catalog.rs`; limiter placement between the decay systems (`crates/sim/src/scripting_systems/{flash_decay,vignette_decay,shake_decay}.rs`) and `crates/render-cpu/src/screen_effects.rs` (`pack_effect_uniform`); audio in `crates/postretro/src/audio/mod.rs` (`Audio::set_bus_volume`, main-track effect at manager build).
- **U2** — theme chokepoint `apply_mod_ui_theme_to_renderer` and `commit_mod_ui_theme` in `main.rs` (14k lines — split-first); `Renderer::set_ui_theme`; literals in `crates/ui/src/tree/build.rs` (`INTERACTIVE_LABEL_COLOR`); text measure `measure_run` (`crates/ui/src/text.rs`); focus ring `push_focus_ring` (`crates/renderer/src/render/ui/mod.rs`, ~1.8k lines — split-first); theme drain `drain_theme_js` / Lua.
- **U3** — `collect_focus_node` (`crates/ui/src/tree/ui_tree_focus.rs`); `UiFocusEngine` (`crates/postretro/src/input/ui_focus.rs`, ~2.2k lines — split-first); `nav_intent_for_gamepad_button`, `StickNavTracker` (`input/ui_nav.rs`); `Binding`, `Action` (`input/types.rs`); `DEAD_ZONE` (`input/gamepad.rs`); `GAMEPAD_LOOK_SENSITIVITY` (`input/look.rs`); `UiIntentPayload` gains a raw-capture path; `resolve_crouch_intent` is the hold/toggle pattern.
- **U4** — new app-side adapter module; `window_attributes` and `App`'s `ApplicationHandler` (`main.rs`); snapshot built beside focus export (`FocusRect`, `widget_a11y_state` in `crates/ui/src/tree/`); `AnnounceWidget` and `labelledBy` from `crates/scripting-core/src/ui/descriptor/`.
- **U5** — `Audio::play` / `Audio::update`; `apply_damage_with_context` and `DamageContext` (`crates/entities/src/components/health.rs`); presentation layer (`PresentationSpawn`, world-anchored only today); `Ring` widget for a bearing arc.

## Open questions

Each resolved by the unit named.

- **U1** — Whether a player FOV setting exists or camera FOV is configurable. Verify before adding an FOV slider.
- **U1** — `mundy`'s macOS main-thread requirement and Windows `SetWindowsHookExW` behavior under winit in this app.
- **U1, U4** — Binary-size delta of `mundy` (U1) and `accesskit_winit` (U4) per platform.
- **U1** — The handoff's manual proof names an `arena-lights.ts` strobe. Current source has none: it fires one saturated-red flash on the low-health crossing, and its light sweep animates world lights, which the limiter does not touch. The U1 brief authors strobe content for the manual proof.
- **U4** — Whether creating the window hidden and showing it right after adapter construction avoids the `boot_sequence.md` Windows hang. Proven by AC 26 on real Windows.
- **U1, U3** — GAG tier labels for hold-to-toggle and multiple input devices (U3) and settings saved (U1). Confirm on live pages before docs cite a tier.
