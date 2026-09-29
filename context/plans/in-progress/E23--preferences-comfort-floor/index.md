# E23--preferences-comfort-floor

Brief · resumable · Epic 23 (U1) · reads: `context/plans/ready/E23--accessibility/index.md` (Brief inputs), `context/lib/player_options.md` §2 §4 §5, `context/lib/ui.md` §1.1 §3 §4 §4.1, `context/lib/input.md` §5 §7, `context/lib/boot_sequence.md` §1, `context/lib/rendering_pipeline.md` §7.8 §12, `context/lib/audio.md` §1 · read at 683e363ba

## Problem

An anticipated need, raised by the developer through Epic 23: the engine offers players no accommodations of its own. No store holds accessibility preferences. Nothing reads OS accessibility settings. No photosensitivity floor stands between content and the screen. No settings surface is guaranteed to exist under an arbitrary mod. Bus volume and mono have no player control. Nothing reduces motion beyond `view_feel_scale`. And `settings.toml` parses as one document, so a single bad value resets every setting (`PlayerOptions::load_with_status`). When this is done, every player meets every accessibility preference in an engine panel at first launch, which no mod can remove or replace. Afterwards the mod's own menus carry the settings, and the engine fallback menus and any mod control firing `ui.openAccessibility` reopen the panel. OS reduced-motion seeds the default before the player chooses. `screen.flash` and `screen.vignette` pass a flash limiter mods cannot disable; a source-level floor for every other primitive follows this brief (hub D4). Motion, volume and mono follow the player's choices. A bad settings value costs only that field. U2–U5 add their fields to this substrate; it is the epic's foundation unit.

## Decisions

- **The epic binds.** The hub's Cross-cutting decisions (D2 preference model, D4 photosensitivity floor, D11 settings reach), Invariants, Pinned orderings O1–O12, Boundary inventory and U1 Brief form apply as written, except where a Decision below declares a divergence. This brief pins what the hub left to it.
- **An unset field is an absent key, and saving keeps what it does not understand.** Saving never writes a resolved value (I1). An OS-seedable toggle cycles back to System, so a player can always return to following the OS, and a readonly source slot tells the panel and mod menus which state holds. Saving round-trips the loaded table: an unrecognised value or an unknown key survives until the player writes that field, while a retired alias this build still reads is rewritten to its current name (`player_options.md` §4). An unrecognised value in an OS-seedable field resolves as unset. The source slot diverges from the hub's "unset is a storage state, not a slot value" (hub Boundary inventory): the resolved slots stay resolved, and one readonly bool per OS-seedable field exposes whether the OS is followed, because a player cannot use a System choice the panel cannot show. Cycling a field back to System is the one player action that unsets it, a second divergence from hub D2's "a panel action marks its field player-set". This is the persisted-shape one-way door the hub assigns to U1.
- **Reduce motion suppresses fully.** While on, screen shake and view feel apply 0, UI tweens reach their targets the frame they start, and a presentation instance spawns at its full rise. Scatter (a fixed spawn offset) and fade (opacity) are not motion and stay. Off, each effect follows its own slider. The App passes the switch to the presentation pool at its frame-time call site; no simulation code reads a preference (I9).
- **The OS reader never delays the splash or blocks a frame.** It starts after the first splash frame presents. Whatever follows mod init — the splash clear to the frontend or the first-launch panel, or the CLI boot-map enqueue — waits at most 150 ms, counted from the end of mod init, and only when no reply has arrived; a later reply applies as a live change.
- **Screen-effect accommodations apply to the packed effect uniform on the presenting machine,** never to a slot a script reads. A peer that runs a screen-effect reaction starts and decays the effect itself, and `screen.*` slots never replicate, so the placement is client-local by construction (I9).
- **The flash limiter is the channel clamp alone (owner decision C, 2026-09-28).** This brief built a GPU frame limiter over the composited frame, then withdrew it: a motion test found its 16×9 cell means read ordinary camera motion as flashes and held cells as visible boxes. Hub D4 now places the floor at the sources content drives. This brief ships the CPU channel clamp on `screen.flash` and `screen.vignette`; `drafts/E23--photosensitivity-source-floor` owns every other primitive. Evidence and what was removed: `plan.md` §Corrections, owner decision C.
- **Flash definitions follow WCAG 2.2 with IRIS's transition model.** A transition is a same-sign change in linear relative luminance accumulated from the last extremum, counted once it reaches 0.1 with the darker state below 0.8. A red transition is detected on chromaticity alone, so an equal-brightness red flicker counts: one state saturated red (R/(R+G+B) ≥ 0.8) and a same-sign red-measure change accumulated the same way. Red transitions share the flash budget and are limited by desaturation, only over budget. An intensity change is capped at 4.0 linear relative luminance per second: black to white takes 250 ms. The clamp judges each channel by its blend strength (flash alpha, vignette strength) as a full-screen change of that size; flash and vignette each keep their own window. The test counter for hub AC 4 implements WCAG 2.2 red flash (u′v′ > 0.2) independently of the clamp's detector.
- **Limiter time is presented-frame time,** never script time, which dev tools freeze. A frame longer than 1/30 s is a hitch: its intensity allowance is clamped to 1/30 s of the rate cap, while window aging takes the full elapsed time. Hub AC 4 proves 30–240 Hz, and below 30 fps ramps slow, the protective direction.
- **Players reach settings through the mod's menus (owner decision D, 2026-09-28).** The engine panel shows at the first-launch hold, from the engine fallback frontend and pause menus, and from any mod control firing `ui.openAccessibility`. No reserved input opens it: `nav.options` keeps its pre-U1 behavior. The missing-entry warning fires when the mod registers no tree offering accessibility, meaning no control firing `ui.openAccessibility` or a `ui.accessibility.*` field action; this brief pins when it runs (`plan.md` §Corrections). In the panel and in mod options screens, labels are plain text on the left and widgets on the right. A toggle or cycle field is one button whose text is the field's current value ("SYSTEM (ON)", "OFF"); a press cycles it, focus stays on it, and its accessible name comes from its label. Evidence and scope: `plan.md` §Corrections, owner decision D.
- **A player's button press is the limiter's only write path (owner decision D).** A press on a control firing `ui.accessibility.cycle.flashLimiter` toggles the limiter from any tree, the engine panel or a mod menu. No working copy, script, reaction or manifest path reaches it. Engine-routed slider steps keep the firing-tree check: a step routes only when the press resolved against the panel's own export at engine tier, and a mod slider bound to a readonly `accessibility.*` slot warns and no-ops.
- **UI input never latches across a splash or Loading frame.** Any UI input pressed on one is dropped, never delivered later, so a buffered cancel cannot close the first-launch panel on its first frame.
- **The first-launch hold is its own boot state.** It drains no level requests, keeps the transport alive, and runs the settled save. A host `Relevel` received during the hold or the OS-reader wait outranks the frontend backdrop and a CLI boot map (hub D11).
- **Every panel write persists like a menu write.** A field action from the panel or a mod tree, the limiter's included, schedules the bridge's settled save, and closing the panel flushes it.
- **Mono folds after spatial panning** on the main track, and crossfades on toggle over at least 10 ms, the minimum hub AC 9 leaves to this brief.
- **The panel's registry name is reserved on every registration path:** mod init, level load, staged reload.
- **Non-goals.**
  - Trigger-fired screen effects reaching co-op clients. Trigger reactions run host-only, so a client never sees the effect. That is a presentation-routing defect with nothing client-side to reduce, and each machine's limiter still holds. `drafts/coop-trigger-screen-effects` owns it.
  - A player FOV slider: no FOV setting exists (`camera::HFOV`). It would be a new capability, not an accommodation.
  - A launch flag that skips the first-launch hold. The hold stops only a profile that has never closed the panel, and `--headless` and `--capture` runs bypass the App, so no unattended path needs one today; a dev-only bypass can follow when one does.
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
// accessibilityAction("flashLimiter", "cycle") type-checks too: a player's press toggles the limiter from any tree.

// A mod options menu edits the working copy, exactly like existing options.
Text({ id: "shakeLabel", content: "SCREEN SHAKE" });
Slider({ id: "shake", labelledBy: "shakeLabel", bind: options.screenShakeScale, min: 0, max: 1, step: 0.1, capturesNav: ["nav.left", "nav.right"] });

// Content honoring a preference binds the readonly resolved slot; a menu can show "System" from the source slot.
Text({ content: "MOTION REDUCED", visibleWhen: stateEquals(accessibility.reduceMotion, true) });
Text({ content: "FOLLOWING SYSTEM", visibleWhen: stateEquals(accessibility.reduceMotionFollowsSystem, true) });
```

The TS surface ships with its Luau mirror. Names, wire values and the SDK field type are in §Boundary inventory.

## Acceptance

Hub AC 1–13 bind as written, plus AC 32's panel pass if U1 lands after U4. The rows below add this brief's pins.

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
- [ ] Switch on with the shake and view-feel sliders at 1.0: the packed shake offset and presented view feel are zero, and a presentation instance with a rise spawns at its full rise while keeping its scatter offset and fade.
- [ ] Reduce motion turned on mid-shake zeroes the next packed shake offset while `screen.shake` keeps its authored decay. Turned off before the decay ends, the packed offset resumes at the remaining decayed amplitude. A UI tween still running when the switch turns on reaches its target that frame. (UO2)
- [ ] A bus volume of 0 silences that bus, 1.0 plays at unity gain, and 0.5 plays at −12 dB (±0.1). Master at 0 silences every bus.
- [ ] Toggling mono off halfway through its on-crossfade reverses from the current mix: no captured sample-to-sample step exceeds the largest step in the same signal untoggled, and stereo returns within the elapsed half of the crossfade length. (UO3)
- [ ] The Scripting surface example installs and runs as a `content/dev` fixture in TS and Luau. Its buttons fire the reserved actions, and its `visibleWhen` bindings track the reduce-motion switch and its source slot. A type test accepts `accessibilityAction("flashLimiter", "cycle")` (owner decision D).

Limiter (channel clamp; the frame-limiter rows were withdrawn by owner decision C)
- [ ] A 5 Hz sine `screen.flash` strobe packed at 240 Hz is held to at most three flashes per second, the same count as at 30 Hz, and so is a square one.
- [ ] Two consecutive frames straddling a 2 s hitch change a full-strength `screen.flash` by at most 4.0 × 1/30. At a steady 20 fps, zero to full strength takes at least 375 ms.
- [ ] A player's press on a mod tree's button firing the limiter action toggles the limiter as the panel's control does (owner decision D). An engine-routed slider step fired while the export names a mod tree, or names the panel at a tier other than engine, is ignored, warns, and leaves the setting unchanged. Another field's action fired from a mod tree still writes that field.
- [ ] The resolve reports a GPU timing entry under `POSTRETRO_GPU_TIMING=1`, and reports it as absent, never zero, on an adapter without timestamp support.
- [ ] The resolve owns a `TIMING_PAIR_*` index and a pass label, and `scene_recording_prefills_timing_queries_before_any_pass_or_resolve` covers it (no adapter needed).
- [ ] Turning the limiter off mid-strobe and on again 0.5 s later starts a fresh window: no transition from before the off counts, and the first frame after re-enabling packs unchanged. (UO5, as restated by owner decision A)
- [ ] After a 2 s hitch, no transition from before it counts toward the flash budget, while that frame's change stays within one hitch-ceiling allowance. (UO6, without the splash)
- [ ] The clamp runs when the resolve's slot snapshot omits `accessibility.flashLimiter` or carries it as a non-boolean. Only an explicit `false` packs the effects unchanged.
- [ ] Hub AC 4's automated half runs as CPU tests on the packed effect uniform, counted by a WCAG counter independent of the clamp's detector. No adapter is needed.
- [ ] A saturated-red `screen.flash` strobe at 5 Hz is held to at most three flashes per second and packs desaturated over budget. A single red flash keeps its hue.
- [ ] No GPU flash-limiter work remains: no measure or limit pass, no per-cell resolve term, and no limiter timing pair or splash hand-off. The resolve's shader is the merge-base composite again (review gate).

Panel
- [ ] A panel write at the first-launch hold persists after the 250 ms settle with the panel still open. A quit before the close leaves that field saved and `accessibility_panel_shown` absent. (UO7)
- [ ] A `--connect` client launched with a CLI map, whose host names a different map during the first-launch hold, loads the host's map when the hold ends.
- [ ] A mod that registers no tree offering accessibility, meaning no control firing `ui.openAccessibility` or a `ui.accessibility.*` field action, draws one load-time warning. A staged reload that adds such a control draws none. (UO8, restated by owner decision D)
- [ ] The dev options screen's Accessibility tab carries every `accessibility` group field the engine panel carries, the limiter included. In both, each toggle or cycle button shows its field's current value and keeps focus after a press. (owner decision D)

Input on splash and Loading frames (the global-input rows were withdrawn by owner decision D)
- [ ] A UI input pressed on a Loading frame activates nothing on the first Running frame.
- [ ] A gamepad cancel pressed during the splash leaves the first-launch panel open.

### Manual
- [ ] Hub AC 6 visual pass on hardware: the strobe map's `screen.flash` pads are tamed, and ordinary play (pans, turns, shake, a muzzle flash, dark to lit rooms) looks the same with the limiter on and off.
- [ ] Hub AC 6 GPU timing for the resolve on an adapter with timestamp support.
- [ ] Hub AC 1 OS reduced-motion toggle on Windows, macOS and Linux, including Linux with no desktop portal (resolves to the engine default).
- [ ] Hub AC 12 loopback `--connect` hold and hub AC 13 two-instance co-op pass.

## Path

- **Re-grounded pins.** Every hub U1 brief pin (`E23--accessibility/research.md` §Brief pins U1) still holds at `683e363ba`. Line citations there are stale; `research.md` §Pin symbols maps each to its current symbol.
- **Substrate.** Per-field parse: deserialize to `toml::Table`, then take each field with its own fallback and a warning. Keep the table for the save. `PlayerOptions::save` today serializes the struct whole (`toml::to_string_pretty`). Rewrite `an_out_of_vocabulary_surface_depth_state_falls_back_to_defaults` and `load_returns_defaults_and_preserves_file_when_graphics_quality_is_unknown` to assert that only the bad field falls back. Slots follow `options.viewFeelScale` and `screen.vignette` in `BUILTIN_ENGINE_STATE`.
- **OS reader.** `mundy` `Preferences::subscribe` on the main thread once the winit `EventLoop` exists (macOS requires it), forwarding to a channel the App polls at the top of each frame (UO1). Its `once_blocking` ignores the timeout on Windows, so the bound is the App's own deadline. Windows `TextScaleFactor` rides the same seam through a direct Windows-only `windows` dependency. `research.md` §OS reader.
- **Panel wiring.** The focus export gains the exporting tree's registry name and scope tier (`FocusRectList`, `export_top_focus_rects`); the engine-routed slider's tier check reads it. A panel write must schedule the settled save explicitly, because the reseed advances the bridge's observed generation (`OptionsBridge::changed_value`). The hold state calls `poll_world_less_transport` and `update_player_options` itself.
- **Audio.** `Audio::set_bus_volume` and `Audio::set_main_volume` already exist with no production caller. Stored volume v maps to 40·log₁₀ v dB (gain v², the usual perceptual taper); 0 maps to kira silence. The mono fold goes in `AudioManagerSettings::main_track_builder` at `Audio::new` (`research.md` §Pin symbols, mono fold seam).
- **Tweens.** UI binding tweens drive through `drive_tween_f32` and `drive_tween_rgba`. Presentation rise is sampled per frame in `PresentationPool::advance_and_collect_inputs` (`crates/sim/src/sim/presentation_pool.rs`, eased `rise_pixels`), called from the App's frame loop; snapping takes the end-of-life rise from spawn.
- **Limiter (historical).** The frame limiter's Path — timing pairs for the measure and limit passes, the hoisted GPU harness, the shared WGSL composite, and a first slice that falsified shared-composite measurement — was built and then withdrawn by owner decision C (`plan.md` §Corrections). What remains: the channel clamp in `crates/render-cpu/src/flash_clamp.rs`, packed in `encode_resolve`; the resolve's own `TIMING_PAIR_*`; and the hoisted `render/gpu_test_harness.rs`, which the UI tests keep using.
- **Split first,** each its own behavior-preserving commit, before this brief extends them: `main.rs`, `startup/lifecycle.rs` (absent from the hub's split-first list but past the threshold), `ui/src/modal_stack.rs`, `options/bridge.rs`, `renderer/src/render/ui/mod.rs`, and `input/ui_focus.rs` if the slider-capture change lands there rather than in the App ahead of capture. The hub's `audio/mod.rs` flag is moot: audio moved to `crates/audio`, whose `lib.rs` is under the threshold.
- **SDK mirror.** The new reserved names join `crates/ui/src/actions.rs`, `sdk/lib/ui/reactions.{ts,luau}`, the typedef templates, `docs/scripting-reference.md`, and the Luau `UI_REACTIONS_FIELDS` list (`crates/scripting-core/src/luau_prelude.rs`).
- **Concurrency.** `cpu-frame-profiling` (in progress) adds CPU scopes in `renderer_render_frame.rs` around the resolve and in the `main.rs` frame loop. Rebase the limiter stage onto it once it lands.

## Open questions

- Missing-entry warning wording — **delegated**: it says the mod offers no accessibility settings and names the fix (a control firing `ui.openAccessibility` or a `ui.accessibility.*` field action). Restated by owner decision D; the first wording named trees and the global input.
- Restart-loop fixture home — **delegated**: a level-scoped `levelLoad` reaction on its own uncatalogued map if level scripts can register one, else a separate fixture mod root; report which.

## Boundary inventory

Resolves the hub inventory's "Still pinned by brief" column for U1. Rows the hub fully names stand as written there.

| Name | TOML key | Working copy (`options.*`) | Resolved slot (`accessibility.*`) | Pin |
|---|---|---|---|---|
| Reduce motion | `[accessibility] reduce_motion`, absent = unset | `options.reduceMotion` | `accessibility.reduceMotion` | engine default off; on: shake 0, view feel 0, tweens snap |
| Screen-shake scale | `[accessibility] screen_shake_scale` | `options.screenShakeScale` | `accessibility.screenShakeScale` | `[0, 1]`, default 1.0, step 0.1 |
| View-feel scale (step) | `view_feel_scale` (top-level, existing) | `options.viewFeelScale` | `accessibility.viewFeelScale` | panel `increase`/`decrease` step 0.1 |
| Bus volumes | `[accessibility] master_volume`, `sfx_volume`, `music_volume`, `ui_volume` | `options.masterVolume` … `options.uiVolume` | `accessibility.masterVolume` … `accessibility.uiVolume` | linear `[0, 1]`, step 0.05, default 1.0; 40·log₁₀ v dB, 0 = silence; in the group |
| OS-unset representation | key absent | — | resolved value | panel cycle System → On → Off, labeled with the resolved value ("System (On)") |
| Follows-system source | — (derived: field unset — key absent or its stored value unrecognised) | none | `accessibility.<field>FollowsSystem` (bool) per OS-seedable field, e.g. `accessibility.reduceMotionFollowsSystem` | readonly, `ReplicationScope::None`; the panel label binds it |
| Panel field-action family | — | `ui.accessibility.<op>.<field>`, op ∈ `cycle` `increase` `decrease`, field = the slot's camelCase suffix; SDK `accessibilityAction(field, op)` | — | SDK types each op by field kind (`cycle` for toggles and enums, `increase`/`decrease` for numerics), `flashLimiter` included; a mismatched op at runtime warns and does nothing; any tree may fire the limiter's action (owner decision D) |
| Open-panel action | — | `ui.openAccessibility`; SDK `OPEN_ACCESSIBILITY_ACTION` | — | — |
| Panel global input | withdrawn (owner decision D) | — | — | `nav.options` keeps its pre-U1 behavior; no key or button opens the panel |
| Panel descriptor / registry name | `core/ui/accessibilityPanel.json` / `accessibilityPanel` | — | — | reserved at mod and level tier |
| First-launch record | `accessibility_panel_shown` (top-level bool) | — | — | written on any panel close |
| Strobe fixture map | `content/dev/maps/a11y-strobe-test.map`, outside the mod catalog | — | — | pads 1–2 (full-screen `screen.flash`, white and saturated red) exercise the clamp; pads 3–6 (UI panels, light animation) stay as the source floor's first fixtures and are unlimited until it lands; loaded only by name, so no player payload ships it |
