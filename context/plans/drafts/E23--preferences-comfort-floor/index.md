# E23--preferences-comfort-floor

Brief · resumable · Epic 23 (U1) · reads: `context/plans/ready/E23--accessibility/index.md` (Brief inputs), `context/lib/player_options.md` §2 §4 §5, `context/lib/ui.md` §1.1 §3 §4 §4.1, `context/lib/input.md` §5 §7, `context/lib/boot_sequence.md` §1, `context/lib/rendering_pipeline.md` §7.8 §12, `context/lib/audio.md` §1 · read at 683e363ba

## Problem

An anticipated need, raised by the developer through Epic 23: the engine offers players no accommodations of its own. No store holds accessibility preferences. Nothing reads OS accessibility settings. No photosensitivity floor stands between content and the screen. No settings surface is guaranteed to exist under an arbitrary mod. Bus volume and mono have no player control. Nothing reduces motion beyond `view_feel_scale`. And `settings.toml` parses as one document, so a single bad value resets every setting (`PlayerOptions::load_with_status`). When this is done, every player reaches every accessibility preference from an engine panel that no mod can remove. OS reduced-motion seeds the default before the player chooses. Every presented gameplay frame passes a flash limiter mods cannot disable. Motion, volume and mono follow the player's choices. A bad settings value costs only that field. U2–U5 add their fields to this substrate; it is the epic's foundation unit.

## Decisions

- **The epic binds.** The hub's Cross-cutting decisions (D2 preference model, D4 photosensitivity floor, D11 settings reach), Invariants, Pinned orderings O1–O12, Boundary inventory and U1 Brief form apply as written. This brief pins what the hub left to it.
- **An unset field is an absent key, and saving keeps what it does not understand.** Saving never writes a resolved value (I1). An OS-seedable toggle cycles back to System, so a player can always return to following the OS, and a readonly source slot tells the panel and mod menus which state holds. Saving round-trips the loaded table: an unrecognised value or an unknown key survives until the player writes that field, while a retired alias this build still reads is rewritten to its current name (`player_options.md` §4). An unrecognised value in an OS-seedable field resolves as unset. The source slot diverges from the hub's "unset is a storage state, not a slot value" (hub Boundary inventory): the resolved slots stay resolved, and one readonly bool per OS-seedable field exposes whether the OS is followed, because a player cannot use a System choice the panel cannot show. This is the persisted-shape one-way door the hub assigns to U1.
- **Reduce motion suppresses fully.** While on, screen shake and view feel apply 0, and UI and presentation-template tweens reach their targets the frame they start. Off, each effect follows its own slider.
- **The OS reader never delays the splash or blocks a frame.** It starts after the first splash frame presents. Whatever follows mod init — the splash clear to the frontend or the first-launch panel, or the CLI boot-map enqueue — waits at most 150 ms, counted from the end of mod init, and only when no reply has arrived; a later reply applies as a live change.
- **Screen-effect accommodations apply to the packed effect uniform on the presenting machine,** never to a slot a script reads. A peer that runs a screen-effect reaction starts and decays the effect itself, and `screen.*` slots never replicate, so the placement is client-local by construction (I9).
- **The frame limiter measures the composited frame without a second full-resolution target.** A compute pass ahead of the resolve samples `scene_color` through the resolve's own composite, so screen-effect and scene flashes share one budget (D4), and its history stays on the GPU. Suppression scales each cell's luminance and saturation toward the last limited output; it never holds a frame, which would freeze gameplay. A resolve term that samples neighbouring pixels or varies over time, such as a CRT filter, moves the limiter to the intermediate-target rival, since later post passes compose before it (`rendering_pipeline.md` §7.8). Rivals and prior art: `research.md` §Limiter mechanism.
- **Flash definitions follow WCAG 2.2 with IRIS's transition model.** A transition is a same-sign change in linear relative luminance accumulated from the last extremum, counted once it reaches 0.1 with the darker state below 0.8. A red transition is detected on chromaticity alone, so an equal-brightness red flicker counts: one state saturated red (R/(R+G+B) ≥ 0.8) and a same-sign red-measure change accumulated the same way. Red transitions share the flash budget and are limited by desaturation. A flash counts only when the cells changing together reach the flash-area threshold, 0.111 of the frame area (WCAG's 341×256 at 1024×768), so a smaller change passes, however many cells it touches. The cell grid is a fixed fraction of the frame, so it survives any resolution. A full-screen intensity change, one covering at least the threshold area, is capped at 4.0 linear relative luminance per second: black to white takes 250 ms. The test counter for hub AC 4 and 5 implements WCAG 2.2 red flash (u′v′ > 0.2) independently of the limiter's detector.
- **Limiter time is presented-frame time,** never script time, which dev tools freeze. A hitch clamps only the intensity allowance; window aging takes the full elapsed time. A resize keeps the budget and history.
- **Splash frames enter limiter history through a hand-off.** The App tracks each stretch of splash frames, its presented level and its duration, and hands both to the first resolve frame after it. That frame records the gameplay → splash drop as a transition at the stretch's start, ages the window by the duration, and limits every cell against the splash level, so both edges of a load count (hub D4). The splash path gains no limiter work.
- **Limiter attribution names the firing tree.** The limiter action is honored only when the press resolved against the panel's own export at engine tier and the panel is the active tree at activation, checked before a same-frame close or push changes the stack (hub limiter verdict table). Engine-routed slider steps pass the same tier check.
- **The global input reads ahead of text entry, acts on the press edge, and never latches** across a splash or Loading frame. No input captured before the toggle activates a control in the tree the toggle revealed or pushed.
- **The first-launch hold is its own boot state.** It drains no level requests, keeps the transport alive, and runs the settled save. A host `Relevel` received during the hold or the OS-reader wait outranks the frontend backdrop and a CLI boot map (hub D11).
- **Every panel write persists like a menu write.** A panel action, the limiter's included, schedules the bridge's settled save, and closing the panel flushes it.
- **Mono folds after spatial panning** on the main track, and crossfades on toggle over at least 10 ms, the minimum hub AC 9 leaves to this brief.
- **The panel's registry name is reserved on every registration path:** mod init, level load, staged reload.
- **Non-goals.**
  - Trigger-fired screen effects reaching co-op clients. Trigger reactions run host-only, so a client never sees the effect. That is a presentation-routing defect with nothing client-side to reduce, and each machine's limiter still holds. `drafts/coop-trigger-screen-effects` owns it.
  - A player FOV slider: no FOV setting exists (`camera::HFOV`). It would be a new capability, not an accommodation.
  - `--map` with `--connect` dropping a `Relevel` during the boot-map load: pre-existing and outside the hold. Unowned; recorded in `research.md` §Pin symbols for a follow-up.

### Scripting surface

```ts
import { Button, Slider, Text, getGameState, stateEquals, OPEN_ACCESSIBILITY_ACTION, accessibilityAction } from "postretro/ui";

const { options, accessibility } = getGameState();

// A mod menu entry to the engine panel.
Button({ id: "openA11y", label: "ACCESSIBILITY", onPress: OPEN_ACCESSIBILITY_ACTION });

// A panel field action on a mod button.
Button({ id: "reduceMotion", label: "REDUCE MOTION", onPress: accessibilityAction("reduceMotion", "cycle") });
// Numeric fields step: accessibilityAction("screenShakeScale", "increase" | "decrease").
// accessibilityAction("flashLimiter", "cycle") is a type error: only the engine panel fires the limiter action.

// A mod options menu edits the working copy, exactly like existing options.
Text({ id: "shakeLabel", content: "SCREEN SHAKE" });
Slider({ id: "shake", labelledBy: "shakeLabel", bind: options.screenShakeScale, min: 0, max: 1, step: 0.1, capturesNav: ["nav.left", "nav.right"] });

// Content honoring a preference binds the readonly resolved slot; a menu can show "System" from the source slot.
Text({ content: "MOTION REDUCED", visibleWhen: stateEquals(accessibility.reduceMotion, true) });
Text({ content: "FOLLOWING SYSTEM", visibleWhen: stateEquals(accessibility.reduceMotionFollowsSystem, true) });
```

The TS surface ships with its Luau mirror. Names, wire values and the SDK field type are in §Boundary inventory.

## Acceptance

Hub AC 1–13 bind as written, plus AC 32's panel pass if U1 lands after U4 and AC 21's armed-capture clause if it lands after U3. The rows below add this brief's pins.

### Automated
Storage
- [ ] Saving writes no key for a never-set OS-seedable field. A file with that key absent loads it as unset, and a later OS change moves it.
- [ ] Cycling an OS-seedable toggle from Off returns it to System: the key disappears on the next save, and the field follows the OS again.
- [ ] With reduce motion unset and the OS reporting on, the panel's reduce-motion control reads "System (On)" and `accessibility.reduceMotionFollowsSystem` is true; after one cycle step it reads "On" and the source slot is false.
- [ ] A file with an unrecognised value in one field and a key this build does not know keeps both keys and values after a save that changes another field. A player write to the field with the unrecognised value replaces it.
- [ ] A panel write, the limiter's included, persists after the 250 ms settle with no menu open, and persists immediately when the panel closes first.
- [ ] A mod-menu write, or a panel cycle step, that sets an unset field to the value it already resolves to marks it player-set: the next save writes the key, and a later OS change leaves the field alone.
- [ ] With a `settings.toml` that is not valid TOML, a panel write, a mod-menu write, and a panel close each leave the file byte for byte. No `accessibility_panel_shown` is written, and the next launch shows the panel again.
- [ ] First launch with no settings file writes a file that holds no key for any OS-seedable field and no `accessibility_panel_shown` until the panel closes; a later OS change still moves each one.

OS reader
- [ ] The first splash frame presents before the reader starts.
- [ ] A reply arriving within the 150 ms bound, after mod init finishes, is applied before the splash clears. A reply after the bound applies as a live change on a later frame.
- [ ] Splash frames keep presenting while the clear waits. A reply that is already in when mod init finishes adds no frames.
- [ ] With a mod init that takes 400 ms and a fake reader replying 100 ms after it finishes, the reply is applied before the splash clears. (UO9)
- [ ] An OS reply and a mod-menu write to the same field in one frame leave the field player-set at the menu's value, and the next save writes it. An OS reply in the frame a panel cycle returns the field to System resolves the field to that reply. (UO1)

Reduce motion and screen effects
- [ ] Switch on with the shake and view-feel sliders at 1.0: the packed shake offset and presented view feel are zero.
- [ ] Reduce motion turned on mid-shake zeroes the next packed shake offset while `screen.shake` keeps its authored decay. Turned off before the decay ends, the packed offset resumes at the remaining decayed amplitude. A UI tween still running when the switch turns on reaches its target that frame. (UO2)
- [ ] A bus volume of 0 silences that bus, and 1.0 plays at unity gain. Master at 0 silences every bus.
- [ ] Toggling mono off halfway through its on-crossfade reverses from the current mix: no captured sample-to-sample step exceeds the largest step in the same signal untoggled, and stereo returns within the elapsed half of the crossfade length. (UO3)
- [ ] The Scripting surface example installs and runs as a `content/dev` fixture in TS and Luau. Its buttons fire the reserved actions, and its `visibleWhen` bindings track the reduce-motion switch and its source slot. A type test rejects `accessibilityAction("flashLimiter", "cycle")`.

Limiter
- [ ] A strobe below the flash-area threshold that straddles several cell boundaries passes unchanged, while the same strobe at the threshold area is limited.
- [ ] A 5 Hz sine strobe presented at 240 Hz is held to at most three flashes per second, the same count as at 30 Hz.
- [ ] Two consecutive frames straddling a 2 s hitch change luminance by no more than one hitch-ceiling allowance.
- [ ] With the limiter on or off, a panel control fired while the export names a mod tree, or names the panel at a tier other than engine, is ignored, warns, and leaves the setting unchanged.
- [ ] The resolve and limiter each report a GPU timing entry under `POSTRETRO_GPU_TIMING=1`, and report it as absent, never zero, on an adapter without timestamp support.
- [ ] The resolve and the limiter measure pass each own a `TIMING_PAIR_*` index and a pass label, and `scene_recording_prefills_timing_queries_before_any_pass_or_resolve` covers both (no adapter needed).
- [ ] With both stages on, a four-flash full-screen `screen.flash` strobe inside 1 s presents the same frames as the same sequence run with the clamp bypassed and the clamp's recorded output fed to the frame limiter. The onset the clamp suppressed uses no frame-limiter budget. (UO4)
- [ ] Turning the limiter off mid-strobe and on again 0.5 s later limits against the last presented frame: the first frame after re-enabling presents unchanged when it matches that frame, and no transition from before the off counts toward the budget. (UO5)
- [ ] After a 2 s stretch of splash frames or a 2 s hitch, no transition from before it counts toward the flash budget, while that frame's intensity change stays within one hitch-ceiling allowance. (UO6)
- [ ] Both limiter stages run when the resolve's slot snapshot omits `accessibility.flashLimiter` or carries it as a non-boolean. Only an explicit `false` passes content unchanged.
- [ ] Frame capture with the limiter on produces the same bytes as with it off, and capturing leaves the limiter's per-cell history unchanged.
- [ ] Hub AC 4 and 5's automated halves run as GPU tests that drive synthetic `scene_color` and effect-slot sequences, splash hand-offs included, through the measure pass and the resolve into an offscreen target read back every frame. A run that skips for lack of an adapter leaves them outstanding.
- [ ] An equal-brightness red/green full-screen flicker at 5 Hz is held to at most three flashes per second and presents desaturated.
- [ ] A gameplay → splash → gameplay cycle counts two transitions: the drop into the splash and the return.
- [ ] The measure pass and the resolve compose through one WGSL function: a shader-source test fails when either entry point applies shake, tonemap, vignette or flash outside it.

Panel
- [ ] A panel write at the first-launch hold persists after the 250 ms settle with the panel still open. A quit before the close leaves that field saved and `accessibility_panel_shown` absent. (UO7)
- [ ] A `--connect` client launched with a CLI map, whose host names a different map during the first-launch hold, loads the host's map when the hold ends.
- [ ] A staged reload that points `frontend.menuTree` at a tree without a `ui.openAccessibility` button warns naming that tree, not the previous one. A reload that adds the button to a tree that warned draws no warning. (UO8)

Global input
- [ ] F1 held through OS key repeat inside a text-entry modal toggles the panel once in both directions: a hold that closes the panel and reveals the text-entry modal beneath does not reopen it on repeat.
- [ ] F1 or Select pressed on a Loading frame opens nothing on the first Running frame.
- [ ] A confirm captured in the same input stage as the F1 that opens the panel activates nothing in the panel. F1 plus a confirm while the panel is the active tree closes it and activates nothing beneath.

### Manual
- [ ] Hub AC 5 and AC 6 visual and strobe passes, on hardware.
- [ ] Hub AC 6 GPU timing on an adapter with timestamp support. This Mac lacks it; the timing pass is a Windows handoff.
- [ ] Hub AC 1 OS reduced-motion toggle on Windows, macOS and Linux, including Linux with no desktop portal (resolves to the engine default).
- [ ] Hub AC 12 loopback `--connect` hold and hub AC 13 two-instance co-op pass.

## Path

- **Re-grounded pins.** Every hub U1 brief pin (`E23--accessibility/research.md` §Brief pins U1) still holds at `683e363ba`. Line citations there are stale; `research.md` §Pin symbols maps each to its current symbol.
- **Substrate.** Per-field parse: deserialize to `toml::Table`, then take each field with its own fallback and a warning. Keep the table for the save. `PlayerOptions::save` today serializes the struct whole (`toml::to_string_pretty`). Rewrite `an_out_of_vocabulary_surface_depth_state_falls_back_to_defaults` and `load_returns_defaults_and_preserves_file_when_graphics_quality_is_unknown` to assert that only the bad field falls back. Slots follow `options.viewFeelScale` and `screen.vignette` in `BUILTIN_ENGINE_STATE`.
- **OS reader.** `mundy` `Preferences::subscribe` on the main thread once the winit `EventLoop` exists (macOS requires it), forwarding to a channel the App polls at the top of each frame (UO1). Its `once_blocking` ignores the timeout on Windows, so the bound is the App's own deadline. Windows `TextScaleFactor` rides the same seam through a direct Windows-only `windows` dependency. `research.md` §OS reader.
- **Panel wiring.** The focus export gains the exporting tree's registry name and scope tier (`FocusRectList`, `export_top_focus_rects`). A panel write must schedule the settled save explicitly, because the reseed advances the bridge's observed generation (`OptionsBridge::changed_value`). The hold state calls `poll_world_less_transport` and `update_player_options` itself.
- **Audio.** `Audio::set_bus_volume` and `Audio::set_main_volume` already exist with no production caller. The mono fold goes in `AudioManagerSettings::main_track_builder` at `Audio::new` (`research.md` §Pin symbols, mono fold seam).
- **Tweens.** UI binding tweens drive through `drive_tween_f32` and `drive_tween_rgba`. The presentation-template motion site is unlocated.
- **Limiter.** Timing pairs register as a `TIMING_PAIR_*` const plus a label, and `scene_recording_prefills_timing_queries_before_any_pass_or_resolve` guards source order. For hub AC 4 and 5, hoist `render/ui/gpu_test_harness.rs` to `render/`, then drive synthetic `scene_color` and effect slots through the measure pass and `encode_resolve`, reading back each frame. A skip is not a pass.
- **Limiter shape.** One WGSL composite function shared by the measure pass and the resolve; per-cell state holds last limited output luminance, the transition accumulator and transition times. Rival: the resolve writes an intermediate target and a limiter pass writes the swapchain. Exact and simple, but a full-resolution target and pass every frame; take it also if sharing the composite proves brittle.
- **First slice, limiter stage.** Measure pass plus gain through the hoisted harness, on a full-screen square strobe that must be limited and a below-threshold strobe straddling cell boundaries that must pass. It falsifies the riskiest assumptions, shared-composite measurement with GPU-only history and cross-cell area, before the counting rules land.
- **Split first,** each its own behavior-preserving commit, before this brief extends them: `main.rs`, `startup/lifecycle.rs` (absent from the hub's split-first list but past the threshold), `ui/src/modal_stack.rs`, `options/bridge.rs`, `renderer/src/render/ui/mod.rs`, and `input/ui_focus.rs` if the slider-capture change lands there rather than in the App ahead of capture. The hub's `audio/mod.rs` flag is moot: audio moved to `crates/audio`, whose `lib.rs` is under the threshold.
- **SDK mirror.** The new reserved names join `crates/ui/src/actions.rs`, `sdk/lib/ui/reactions.{ts,luau}`, the typedef templates, `docs/scripting-reference.md`, and the Luau `UI_REACTIONS_FIELDS` list (`crates/scripting-core/src/luau_prelude.rs`).
- **Concurrency.** `cpu-frame-profiling` (in progress) adds CPU scopes in `renderer_render_frame.rs` around the resolve and in the `main.rs` frame loop. Rebase the limiter stage onto it once it lands.

## Open questions

- Hitch-ceiling value — **delegated**.
- Cell grid size — **delegated**: each cell at most a quarter of the flash-area threshold.
- Strobe fixture homes — **delegated**: strobes (full-screen `screen.flash`, UI-panel color, light animation, sine, area variants) live on a dedicated `content/dev` map loaded only by name, never a default map. The restart loop is a level-scoped `levelLoad` reaction on its own map if level scripts can register one, else a separate fixture mod root; report which.
- Missing-entry warning text; bus volume slider step and dB curve floor — **delegated**.
- Where presentation-template motion (rise, scatter) animates, for reduce-motion snapping — **delegated**.

## Boundary inventory

Resolves the hub inventory's "Still pinned by brief" column for U1. Rows the hub fully names stand as written there.

| Name | TOML key | Working copy (`options.*`) | Resolved slot (`accessibility.*`) | Pin |
|---|---|---|---|---|
| Reduce motion | `[accessibility] reduce_motion`, absent = unset | `options.reduceMotion` | `accessibility.reduceMotion` | on: shake 0, view feel 0, tweens snap |
| Screen-shake scale | `[accessibility] screen_shake_scale` | `options.screenShakeScale` | `accessibility.screenShakeScale` | `[0, 1]`, default 1.0 |
| Bus volumes | `[accessibility] master_volume`, `sfx_volume`, `music_volume`, `ui_volume` | `options.masterVolume` … `options.uiVolume` | `accessibility.masterVolume` … `accessibility.uiVolume` | linear `[0, 1]`, default 1.0, mapped to decibels at the audio seam; in the group |
| OS-unset representation | key absent | — | resolved value | panel cycle System → On → Off, labeled with the resolved value ("System (On)") |
| Follows-system source | — (derived: key absent) | none | `accessibility.<field>FollowsSystem` (bool) per OS-seedable field, e.g. `accessibility.reduceMotionFollowsSystem` | readonly, `ReplicationScope::None`; the panel label binds it |
| Panel field-action family | — | `ui.accessibility.<op>.<field>`, op ∈ `cycle` `increase` `decrease`, field = the slot's camelCase suffix; SDK `accessibilityAction(field, op)` | — | SDK types each op by field kind (`cycle` for toggles and enums, `increase`/`decrease` for numerics) and omits `flashLimiter`; a mismatched op at runtime warns and does nothing; the engine honors the limiter's wire value from the panel only |
| Open-panel action | — | `ui.openAccessibility`; SDK `OPEN_ACCESSIBILITY_ACTION` | — | — |
| Panel global input | `nav.options` | — | — | gamepad Select/Back, keyboard F1 |
| Panel descriptor / registry name | `core/ui/accessibilityPanel.json` / `accessibilityPanel` | — | — | reserved at mod and level tier |
| First-launch record | `accessibility_panel_shown` (top-level bool) | — | — | written on any panel close |
