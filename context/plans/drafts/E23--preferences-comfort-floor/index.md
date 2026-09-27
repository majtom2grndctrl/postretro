# E23--preferences-comfort-floor

Brief · resumable · Epic 23 (U1) · reads: `context/plans/ready/E23--accessibility/index.md` (Brief inputs), `context/lib/player_options.md` §2 §4 §5, `context/lib/ui.md` §1.1 §3 §4 §4.1, `context/lib/input.md` §5 §7, `context/lib/boot_sequence.md` §1, `context/lib/rendering_pipeline.md` §7.8 §12, `context/lib/audio.md` §1 · read at 683e363ba

## Problem

An anticipated need, raised by the developer through Epic 23: the engine offers players no accommodations of its own. No store holds accessibility preferences. Nothing reads OS accessibility settings. No photosensitivity floor stands between content and the screen. No settings surface is guaranteed to exist under an arbitrary mod. Bus volume and mono have no player control. Nothing reduces motion beyond `view_feel_scale`. And `settings.toml` parses as one document, so a single bad value resets every setting (`PlayerOptions::load_with_status`). When this is done, every player reaches every accessibility preference from an engine panel that no mod can remove. OS reduced-motion seeds the default before the player chooses. Every presented gameplay frame passes a flash limiter mods cannot disable. Motion, volume and mono follow the player's choices. A bad settings value costs only that field. U2–U5 add their fields to this substrate; it is the epic's foundation unit.

## Decisions

- **The epic binds.** The hub's Cross-cutting decisions (D2 preference model, D4 photosensitivity floor, D11 settings reach), Invariants I1, I2, I7, I9, I10, I11, Pinned orderings O1–O12 and the Boundary inventory apply as written. This brief pins what the hub left to it; it does not restate the hub.
- **Three checkpointed stages, substrate first:** preference substrate (group, slots, per-field storage, OS reader, reduce motion, bus volume, mono), then the panel (descriptor, reserved actions, global input, missing-entry warning, first-launch hold), then the flash limiter (both stages, timing, strobe fixtures). Each stage ships its slice of the dev-mod consumer (hub U1).
- **An unset field is an absent key.** `settings.toml` omits a field the player never set, and saving never writes a resolved value (I1, I7). The accessibility fields live in an `[accessibility]` table; `view_feel_scale` keeps its top-level key (hub Boundary inventory). An OS-seedable toggle's panel control cycles System → On → Off, labeled with the resolved value ("System (On)"), so a player can always return to following the OS. This is the persisted-shape one-way door the hub assigns to U1.
- **Reduce motion suppresses fully.** While the switch is on, screen shake and view feel apply 0 and UI and presentation-template tweens reach their targets the frame they start. Off, each effect follows its own slider. OS-seeded from `mundy` reduced motion.
- **The OS reader is asynchronous and never delays the splash.** The reader subscribes on the main thread (macOS requires it) after the first splash frame presents, and forwards readings over a channel the App polls each frame without blocking. The splash clear waits at most 150 ms, and only when mod init finishes before the first reply. A later reply applies as a live change. Windows `TextScaleFactor`, through a direct Windows-only `windows` dependency, rides the same seam. A test fake replaces the seam (hub AC 1). The reader exposes contrast and text-scale readings; U2 binds them.
- **Screen-effect accommodations apply where the effect uniform packs.** Reduce-motion shake scaling and the channel clamp act on the packed effect uniform, on the machine that presents it. Every peer starts and decays its own screen effects, and `screen.*` slots never replicate, so this placement is client-local by construction (I9). The scaling never writes a slot a script can read.
- **The frame limiter measures the composited frame without a second full-resolution target.** A compute pass ahead of the resolve samples `scene_color` per cell through the resolve's own composite — shake offset, tonemap, vignette, flash — shared as one WGSL function, so a `screen.flash` strobe and a scene strobe share one budget (D4). Per-cell state stays on the GPU: last limited output luminance, transition accumulator, transition times. The pass writes a per-cell gain and desaturation the resolve applies. Suppression scales luminance toward the last limited output. It never holds a frame, because a held frame freezes gameplay. Rival and prior art: `research.md` §Limiter mechanism.
- **Flash definitions follow WCAG 2.2 with IRIS's transition model.** A transition is a same-sign change in linear relative luminance accumulated from the last extremum, counted once it reaches 0.1 with the darker state below 0.8. Saturated red (R/(R+G+B) ≥ 0.8) transitions are desaturated. Cells are sized so the WCAG flash-area threshold always covers whole cells at any resolution. The test counter for hub AC 4 and 5 implements WCAG 2.2 red flash (u′v′ > 0.2) independently of the limiter's detector.
- **Limiter time is presented-frame time.** The App passes raw frame time to the renderer, not script time, which dev tools can freeze. The limiter clamps one frame's allowance to a hitch ceiling. A resize keeps the budget and resamples per-cell history to the new grid.
- **Limiter attribution uses a named export.** The focus export carries the exporting tree's registry name and scope tier. The limiter action is honored only when that export names the panel at engine tier and the panel is the active tree at activation, checked before a same-frame close or drained push changes the stack (hub limiter verdict table). Engine-routed slider steps use the same tier.
- **The global input reads ahead of text entry, on the press edge only, and is dropped on splash and Loading frames.** F1 is the keyboard default beside gamepad Select/Back. A press never latches across a Loading frame, and no confirm, click or direction captured before the toggle activates a control in the tree the toggle revealed or pushed.
- **The first-launch hold is its own boot state.** It drains no level requests and polls the world-less transport. A host `Relevel` received during the hold or the OS-reader wait outranks the frontend backdrop. The record is `accessibility_panel_shown`, top-level, written on any panel close.
- **A panel write schedules the bridge's settled save.** A panel action, the limiter's included, schedules the same debounced save a menu write does, even though the reseed advances the observed generation. Closing the panel flushes it.
- **Bus volumes join the group,** stored linear 0–1, default 1.0, mapped to decibels at the audio seam. Master drives the main track. The mono fold is a main-track effect added where `Audio::new` builds the manager. It composes after spatial panning and crossfades on toggle.
- **Reserved panel names.** Registry name `accessibilityPanel`, descriptor `core/ui/accessibilityPanel.json`. Mod- and level-scope registrations under the name are rejected on every registration path: mod init, level load, staged reload.
- **Split first, each its own behavior-preserving commit:** `main.rs`, `startup/lifecycle.rs`, `ui/src/modal_stack.rs`, `options/bridge.rs`, before this brief extends them. `startup/lifecycle.rs` is absent from the hub's split-first list but qualifies.
- **Non-goals.**
  - Trigger-fired screen effects reaching co-op clients. Trigger reactions run host-only, so a client never sees the effect. That is a presentation-routing defect with nothing client-side to reduce, and each machine's limiter still holds. `drafts/coop-trigger-screen-effects` owns it.
  - A player FOV slider: no FOV setting exists (`camera::HFOV`). It would be a new capability, not an accommodation.
  - `--map` with `--connect` dropping a `Relevel` during the boot-map load: pre-existing, and outside the hold.
  - Every U2–U5 field, even where this brief's seams carry its reading.

### Scripting surface

```ts
import { Button, Slider, stateEquals, OPEN_ACCESSIBILITY_ACTION, accessibilityAction, accessibility, options } from "postretro";

// A mod menu entry to the engine panel. Frontend-menu and pauseMenu trees without one draw a load-time warning.
Button({ id: "openA11y", label: "ACCESSIBILITY", onPress: OPEN_ACCESSIBILITY_ACTION }); // "ui.openAccessibility"

// A panel field action on a mod button: cycle a toggle or enum; OS-seedable fields cycle System → On → Off.
Button({ id: "reduceMotion", label: "REDUCE MOTION", onPress: accessibilityAction("reduceMotion", "cycle") });
// Numeric fields step: accessibilityAction("screenShakeScale", "increase" | "decrease").
// accessibilityAction("flashLimiter", ...) type-checks, but the engine honors it only from its own panel.

// A mod options menu edits the working copy, exactly like existing options.
Slider({ id: "shake", bind: options.screenShakeScale, min: 0, max: 1, step: 0.1, capturesNav: ["nav.left", "nav.right"] });

// Content honoring a preference during play reads the readonly resolved slot.
stateEquals(accessibility.reduceMotion, true);
```

Wire values are `ui.accessibility.<op>.<field>`, with `op` one of `cycle`, `increase` or `decrease`, and `field` the camelCase suffix the field's slot uses. The TS surface ships with its Luau mirror. Names are in §Boundary inventory.

## Acceptance

Hub AC 1–13 bind as written, plus AC 32's panel pass if U1 lands after U4 and AC 21's armed-capture clause if it lands after U3. The rows below add this brief's pins.

### Automated
Storage
- [ ] Saving writes no key for a never-set OS-seedable field. A file with that key absent loads it as unset, and a later OS change moves it.
- [ ] Cycling an OS-seedable toggle from Off returns it to System: the key disappears on the next save, and the field follows the OS again.
- [ ] A panel write, the limiter's included, persists after the 250 ms settle with no menu open, and persists immediately when the panel closes first.

OS reader
- [ ] The first splash frame presents before the reader starts.
- [ ] A reply arriving within the 150 ms bound, after mod init finishes, is applied before the splash clears. A reply after the bound applies as a live change on a later frame.
- [ ] Splash frames keep presenting while the clear waits. A reply that is already in when mod init finishes adds no frames.

Reduce motion and screen effects
- [ ] Switch on with the shake and view-feel sliders at 1.0: the packed shake offset and presented view feel are zero.
- [ ] A bus volume of 0 silences that bus, and 1.0 plays at unity gain. Master at 0 silences every bus.
- [ ] The Scripting surface example installs and runs as a `content/dev` fixture in TS and Luau. Its buttons fire the reserved actions, and its `accessibility.reduceMotion` read tracks the switch.

Limiter
- [ ] A 5 Hz sine strobe presented at 240 Hz is held to at most three flashes per second, the same count as at 30 Hz.
- [ ] Two consecutive frames straddling a 2 s hitch change luminance by no more than one hitch-ceiling allowance.
- [ ] With the limiter on, a panel control fired while the export names a mod tree, or names the panel at a tier other than engine, is ignored and warns.
- [ ] The resolve and limiter each report a GPU timing entry under `POSTRETRO_GPU_TIMING=1`, and report it as absent, never zero, on an adapter without timestamp support.

Global input
- [ ] F1 held through OS key repeat inside a text-entry modal toggles the panel once.
- [ ] F1 or Select pressed on a Loading frame opens nothing on the first Running frame.
- [ ] A confirm captured in the same input stage as the F1 that opens the panel activates nothing in the panel. F1 plus a confirm while the panel is the active tree closes it and activates nothing beneath.

### Manual
- [ ] Hub AC 5 and AC 6 visual and strobe passes, on hardware.
- [ ] Hub AC 6 GPU timing on an adapter with timestamp support. This Mac lacks it; the timing pass is a Windows handoff.
- [ ] Hub AC 1 OS reduced-motion toggle on Windows, macOS and Linux, including Linux with no desktop portal (resolves to the engine default).
- [ ] Hub AC 12 loopback `--connect` hold and hub AC 13 two-instance co-op pass.

## Path

- **Re-grounded pins.** Every hub U1 brief pin (`E23--accessibility/research.md` §Brief pins U1) still holds at `683e363ba`. Line citations there are stale; `research.md` §Pin symbols maps each to its current symbol.
- **Substrate.** Per-field parse: deserialize to `toml::Table`, then take each field with its own fallback and a warning. Rewrite `an_out_of_vocabulary_surface_depth_state_falls_back_to_defaults` and `load_returns_defaults_and_preserves_file_when_graphics_quality_is_unknown` to assert that only the bad field falls back. Slots follow `options.viewFeelScale` and `screen.vignette` in `BUILTIN_ENGINE_STATE`.
- **Audio.** `Audio::set_bus_volume` and `Audio::set_main_volume` already exist with no production caller. The kira 0.12 `main_track_builder` effect API at `Audio::new` is unverified.
- **Tweens.** UI binding tweens drive through `drive_tween_f32` and `drive_tween_rgba`. The presentation-template motion site is unlocated.
- **Limiter.** Timing pairs register as a `TIMING_PAIR_*` const plus a label, and `scene_recording_prefills_timing_queries_before_any_pass_or_resolve` guards source order. For hub AC 4 and 5, hoist `render/ui/gpu_test_harness.rs` to `render/`, then drive synthetic `scene_color` and effect slots through the measure pass and `encode_resolve`, reading back each frame. A skip is not a pass.
- **Rival shape.** The resolve writes an intermediate target and a limiter pass writes the swapchain. That is exact and simple, but costs a full-resolution target and pass every frame. Take it only if sharing the composite function proves brittle.
- **First slice, limiter stage.** Measure pass plus gain on a full-screen square strobe through the hoisted harness. It falsifies the riskiest assumption, shared-composite measurement with GPU-only history, before the counting rules land.
- **Concurrency.** `cpu-frame-profiling` (in progress) adds CPU scopes in `renderer_render_frame.rs` around the resolve and in the `main.rs` frame loop. Rebase the limiter stage onto it once it lands.

## Open questions

- Hitch-ceiling value and per-cell grid size — **delegated**.
- Missing-entry warning text; bus volume slider step and dB curve floor — **delegated**.
- Where presentation-template motion (rise, scatter) animates, for reduce-motion snapping — **delegated**.

## Boundary inventory

Resolves the hub inventory's "Still pinned by brief" column for U1. Rows the hub fully names stand as written there.

| Name | TOML key | Working copy (`options.*`) | Resolved slot (`accessibility.*`) | Pin |
|---|---|---|---|---|
| Reduce motion | `[accessibility] reduce_motion`, absent = unset | `options.reduceMotion` | `accessibility.reduceMotion` | on: shake 0, view feel 0, tweens snap |
| Screen-shake scale | `[accessibility] screen_shake_scale` | `options.screenShakeScale` | `accessibility.screenShakeScale` | `[0, 1]`, default 1.0 |
| Bus volumes | `[accessibility] master_volume`, `sfx_volume`, `music_volume`, `ui_volume` | `options.masterVolume` … `options.uiVolume` | `accessibility.masterVolume` … `accessibility.uiVolume` | linear `[0, 1]`, default 1.0; in the group |
| OS-unset representation | key absent | — | resolved value | panel cycle System → On → Off |
| Panel field-action family | — | `ui.accessibility.<op>.<field>`, op ∈ `cycle` `increase` `decrease`; SDK `accessibilityAction(field, op)` | — | limiter's honored from the panel only |
| Open-panel action | — | `ui.openAccessibility`; SDK `OPEN_ACCESSIBILITY_ACTION` | — | — |
| Panel global input | `nav.options` | — | — | gamepad Select/Back, keyboard F1 |
| Panel descriptor / registry name | `core/ui/accessibilityPanel.json` / `accessibilityPanel` | — | — | reserved at mod and level tier |
| First-launch record | `accessibility_panel_shown` (top-level bool) | — | — | written on any panel close |
