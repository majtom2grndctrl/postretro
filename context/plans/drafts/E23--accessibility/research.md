# Epic 23 — Accessibility: Research

> Investigation behind `index.md`. Decisions live in the index; this file holds standards thresholds, platform API reach, dependency costs, and source findings that shaped them. Read at commit `2542ac2`.

---

## 1. Retired draft

`drafts/ui-focus-accessibility-visuals` is retired: its focus-visual / theme-token track was absorbed into U2 and its accessibility-snapshot track into U4.

## 2. Standards thresholds

Numbers the unit briefs cite. WCAG pages are normative; game-guideline pages are advisory.

| Topic | Threshold | Source |
|---|---|---|
| Text contrast | ≥ 4.5:1; large text ≥ 3:1 | WCAG 2.2 SC 1.4.3 — https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html |
| Non-text / focus contrast | ≥ 3:1 against adjacent colors | WCAG 2.2 SC 1.4.11 — https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html |
| Focus indicator size | Area ≥ a 2 CSS px perimeter of the unfocused component; ≥ 3:1 change between focused and unfocused states | WCAG 2.2 SC 2.4.13 (AAA) — https://www.w3.org/WAI/WCAG22/Understanding/focus-appearance.html |
| Flash rate | ≤ 3 flashes in any 1 s | WCAG 2.2 SC 2.3.1 — https://www.w3.org/WAI/WCAG22/Understanding/three-flashes-or-below-threshold.html |
| Saturated red | R / (R + G + B) ≥ 0.8 counts as saturated red; red-flash transitions are limited independently of luminance | WCAG 2.2 SC 2.3.1 (definition "general flash and red flash thresholds") |
| Flash area | Flashes smaller than ~25% of any 10° visual field are exempt (341×256 px at 1024×768 in the WCAG worked example) | WCAG 2.2 SC 2.3.1 |
| Captions | Adjustable background opacity 0–100%; ≤ 40 characters per line; speaker identification; direction for off-screen sources | Game Accessibility Guidelines (subtitles) — https://gameaccessibilityguidelines.com/ · Xbox Accessibility Guideline 104 — https://learn.microsoft.com/gaming/accessibility/xbox-accessibility-guidelines/104 |
| Text scale | ~100–200% without loss of content | WCAG 2.2 SC 1.4.4 — https://www.w3.org/WAI/WCAG22/Understanding/resize-text.html · XAG 101 |
| First launch | Accessibility settings reachable before the first audio or flash | Industry practice recorded by the standards pass; no single normative source pinned |

**GAG tier labels unconfirmed.** The standards pass marked the tier of three Game Accessibility Guidelines items uncertain: hold-to-toggle alternatives, support for multiple input devices, and settings saved/remembered. Confirm each on the live GAG page before any doc cites its tier.

## 3. OS accessibility preferences

What the engine can read without `unsafe` in its own code.

| Preference | Windows | macOS | Linux | Route |
|---|---|---|---|---|
| Increased contrast | read + change event | read + change event | read + change event (XDG portal) | `mundy` 0.2.3 |
| Reduced motion | read + change event | read + change event | read + change event (XDG portal) | `mundy` 0.2.3 |
| Color scheme | read + change event | read + change event | read + change event (XDG portal) | `mundy` 0.2.3 |
| Text scale | `UISettings::TextScaleFactor()` + changed event | — | — | `windows` 0.62.2 — in the lock only transitively (`wgpu-hal`, `cpal`, `gilrs-core`); U1 adds a direct Windows-only declaration with the `UI_ViewManagement` feature |
| VoiceOver running, Differentiate Without Color | — | safe read | — | macOS API via safe bindings |
| Flashing-lights, caption on/off, mono audio, Windows screen-reader flag | — | — | — | Unreadable without `unsafe` or no API → in-game options only |

Unverified: `mundy`'s macOS main-thread requirement and its Windows `SetWindowsHookExW` use when hosted under winit's event loop in this app (U1 open question).

## 4. Dependency costs

| Crate | Version | License | Notes |
|---|---|---|---|
| `mundy` | 0.2.3 | Apache-2.0 | Safe API; `unsafe` internal. Linux path pulls `zbus` 5 (shared with `accesskit_unix`). |
| `accesskit_winit` | 0.33.x | MIT / Apache-2.0 | Matches winit 0.30.13, workspace `rust-version` 1.85, and `accesskit` 0.24.0 already in `Cargo.lock` via the dev-tools egui feature. Constructors panic if the window is already visible, so the window must be created hidden. No caller `unsafe`. Linux AT-SPI pulls `zbus` 5 + `async-io`. |

`zbus` is not in the lock today; both crates bring it on Linux, once. Binary-size delta per platform is unmeasured (U1 and U4 open question).

## 5. Source findings

**A11y metadata has one reader.** Descriptors carry `label` / `labelledBy`, `role`, image alt/decorative, tree `accessibleName`, `Announce`, and `selected` / `checked` predicates (`scripting-core/src/ui/descriptor/`). Only `disabled` is read at runtime (focus engine and activation gate). `labelledBy` is validated (`validate_interactive_name`) but never resolved; `implicit_role` is dead outside tests; `AnnounceWidget` builds as a zero-size leaf and drops its text and priority. `selected` / `checked` reach `FocusRect` through `widget_a11y_state` and stop there.

**Focus-group defect.** `collect_focus_node` (`crates/ui/src/tree/ui_tree_focus.rs`) sets `focusable = authored_id.is_some() || group.is_some()`, and group membership propagates to every descendant. The linear, spatial, initial-focus, and hit-test paths in `UiFocusEngine` filter only `disabled`. Result: a `Text` or `VStack` inside a group becomes a nav stop — in the dev title menu, nav down from EXIT lands on the "POSTRETRO" title. Test `focus_export_auto_generates_ids_from_tree_position` asserts the current behavior and changes with the fix. The fix is a pre-epic direct build, not an epic unit; U3 and U4 build on it (index Invariant I5).

**Theme chokepoint.** `apply_mod_ui_theme_to_renderer` (`main.rs`) is the single place the renderer theme is composed: engine default with the mod override. `Renderer::set_ui_theme` bumps `ui_theme_generation`, so stale trees rebuild next frame and tween state snaps. `commit_mod_ui_theme` replaces the override on staged reload — a player variant stored inside the override would be lost, so the variant selection must live outside it. Literals no theme reaches: `INTERACTIVE_LABEL_COLOR` and slider track/thumb in `crates/ui/src/tree/build.rs`; dev `frontend-menu.ts` color constants; `combat-presentation.ts` colors; `arena-lights.ts` flash and vignette RGB. The `focus.ring` token also colors slider fill.

**Text measurement.** `measure_run` sizes text for taffy, so a font-size multiplier reflows containers. Text never wraps today (`set_size(None, None)`); the retained gate needs a forced rebuild on scale change; no clipping exists. Smallest authored text: 11 logical px (frontend options note).

**Screen effects.** `pack_effect_uniform` (`crates/render-cpu/src/screen_effects.rs`) passes slot values through unchanged. The decay systems (`crates/sim/src/scripting_systems/{flash_decay,vignette_decay,shake_decay}.rs`) read no option. The vignette shader is radially symmetric — it cannot carry direction.

**Resolve pass.** Every gameplay scene and UI pass — presentation-layer instances included — renders into `scene_color`; `ScreenEffectsPass::encode_resolve` (`crates/renderer/src/render/screen_effects.rs`, shader `screen_effects.wgsl`) samples it, tonemaps, applies flash/vignette/shake, and is the gameplay path's sole swapchain writer, run every frame (`done/M13--screen-space-effects`). It already receives the frame's slot snapshot. Other swapchain writers: the boot splash (`render_splash_frame`: near-black clear plus static logo), which also paints every runtime Loading frame as the clear alone, since the boot → content transition drops the logo (`clear_splash`), and the dev-tools egui overlay (a separate submission after the resolve, `LoadOp::Load`). Frame capture tonemaps through its own pass, `encode_capture_tonemap`; it stops before the UI pass and the resolve and packs an at-rest effect uniform. The resolve records no GPU timestamp pair (`timestamp_writes: None`), so `POSTRETRO_GPU_TIMING=1` does not report it today.

**Dev-mod flash content.** `arena-lights.ts` fires one `flashScreen([1,0,0,0.5], 250)` plus a red vignette and a shake on the low-health crossing — saturated red, but a single flash, not a strobe. Its light "sweep" animates world lights; it is not a strobe either. The strobe proofs need authored content (index U1 dev-mod consumer).

**Audio.** Every sound enters `Audio::play`; today only from the scripted `playSound` reaction and a diagnostic tone. kira 0.12.0 accepts main-track effects only at manager build; a custom `Effect` is safe Rust and toggles at runtime through handle atomics. Main-track effects run after sub-track spatialization, so a mono fold composes after E12's panning regardless of landing order. `Audio::set_bus_volume` exists with a test-only caller; master volume is the main track (`Audio::set_main_volume`), not a bus.

**Panel write path and reach.** Every `options.*` catalog slot is script-writable, and `setState` gates on readonly alone, so a working copy takes writes from any tree; a slider write carries no originating tree. A descriptor button fires either a closed reserved `ui.*` action the App intercepts (`classify_ui_button_action`) or a named reaction a script registers — the engine on-screen keyboard's key reactions are registered by the dev mod. The modal stack records each stacked tree's scope tier. Mods shadow the engine fallback frontend and pause menus by registering the same names, the production path, so entries in the fallbacks alone do not reach the panel. Gamepad Select produces `nav.options` only while a capturing tree is top; no key produces it; nothing acts on it, though a focused slider whose `capturesNav` names it swallows it without a step.

**Boot and first launch.** With no CLI map, the frame the splash clears presents the frontend and requests its background level; with a CLI map, install fires `levelLoad` before the splash clears. A capturing modal pauses neither simulation nor audio (`ui.md` §4), so a first-launch panel opened over either path leaves level sound and effects running beneath it.

**Damage direction.** All damage flows through `apply_damage_with_context` (`crates/entities/src/components/health.rs`). `DamageContext` names the attacker but no position; the attacker's transform yields a bearing only into `BrainComponent.damage_bearing`, and the player has no brain. The presentation layer is world-anchored only; off-screen instances are invisible. The `Ring` widget can draw a bearing arc but is not admitted in presentation templates. Together these route the bearing over an owner-private slot a HUD `Ring` binds, not the Presentation channel (index Prior commitments).

**Window.** `window_attributes` creates the window visible on purpose (`boot_sequence.md` §Window visibility): the reverted hidden-until-first-present scheme hung Windows boot because an invisible window receives no `RedrawRequested`. U4's scheme shows the window right after adapter construction, before the first redraw is requested — medium confidence it avoids the hang; proven only on real Windows.

## Brief pins (from review)

Unit-internal mechanics the index states only as outcomes. Each bullet is a question the unit's brief must settle against live source, with the reviewer's evidence (read at the review-round tree) and finding id. Re-verify every line citation when the brief is drafted.

### U1

- **Panel write persistence** (B1). How does a panel write — and the limiter, which has no working copy — schedule the settled atomic save and flush on panel close? The bridge schedules a save only when `observe_changes` sees a new slot generation, and the reseed that follows a panel write advances the observed generation, so the bridge never sees the change. Reviewer's shape: the panel write schedules the bridge's settled save as an accepted menu change does, and closing the panel flushes it. Evidence: `options/bridge.rs:135-152` (`update_with_save` saves only `if changed`), `bridge.rs:312-323` (`changed_value` returns `None` at the observed generation), `bridge.rs:271-281` (`flush_with_save` saves only when a save is pending), `main.rs:5352-5362` (close flush keyed to `frontend.options` only).
- **Limiter firing-tree attribution** (A1, T2, T3, T5, T18, P-new-2). What identifies the tree whose control fired the limiter action? A press resolves against the focus rects the renderer exported for the top layer the frame before; that export names no tree. A text-entry commit or cancel pops the stack before the focus tick in the same frame, so the stack's top at activation can be the panel while the press hit a popped mod tree. Candidate: stamp the export with registry name and scope tier, and require the export's tree to be the panel and the panel to be the active tree. Settle the verdict for a panel control whose tree stopped being active earlier in the frame (the index table says ignored), and run the check at activation, before a same-frame global-input close or a drained `PushTree` changes the stack (T18). Evidence: `render/ui/mod.rs:1207-1222` (export of `gameplay_trees.last()` only), `main.rs:4477-4480` (export stored after render N), `ui/src/tree/draw.rs:146-157` (`FocusRectList` has no tree name or tier), `main.rs:2465` (`resolve_text_entry_intents` before the focus tick), `main.rs:5942-5945`, `main.rs:6021-6023` (text-entry cancel pops), `main.rs:2512-2535` (focus tick keys on N+1 `active_name()` against the N export), `input/ui_focus.rs:311-316`, `ui_focus.rs:342-347`, `main.rs:1032-1053`, `main.rs:1008-1016`, `modal_stack.rs:258-265` (`StackedTree` carries name and tier), `main.rs:5846-5851` (reserved actions resolve in `fire_focused_button_activation`).
- **Global input routing** (A3, T6, T7, T9, P-new-1, P-new-9; outcome-level parts landed with B5, B6, B7). Settle the routing order that delivers D11's outcomes:
  - *Text entry.* While a text-entry tree is top, every key-down routes through `text_entry_key`, which returns `None` for F1; `nav_intent_for_key` is never consulted. Read the keyboard default before the text-entry branch, whichever of U1 and U3 lands first. Evidence: `main.rs:2078-2107`, `main.rs:2117`, `input/ui_nav.rs:128-148`, `ui_nav.rs:82-98`; the gamepad path has no text-entry branch (`main.rs:2395-2406`).
  - *Key repeat.* OS repeat is honored inside text entry (`main.rs:2086`: `pressed && (!key_event.repeat || text_entry_open)`). Act on the press edge only, so a held F1 toggles once.
  - *Loading frames.* `nav.menu`'s `pending_menu_toggle` is set in any boot state and cleared only by Running game logic or Frontend UI logic, so a Loading-frame press survives to the first Running frame; gilrs holds a Select press until the next gamepad poll, which runs only on Frontend and Running frames. Drop the press, never latch it. Evidence: `main.rs:2126-2128`, `main.rs:2152-2154`, `main.rs:2415`, `main.rs:2555-2557`, `main.rs:5619`, `startup/lifecycle.rs:218-219`, `main.rs:2352-2358`, `main.rs:5518-5545`, `input/gamepad.rs:133-143`; no other clear except constructors (`lifecycle.rs:1773`, `session.rs:295`).
  - *Same-frame input.* The punch-through toggle applies at frame N while other intents captured in the same Input stage resolve at N+1 against the new top tree; `nav.menu` is safe today only because it opens from an empty `Passthrough` stack. Outcome to pin: no confirm, click, or direction captured before the toggle activates a control in the tree the toggle revealed or pushed, mirror case included (close + confirm activates nothing beneath). Evidence: `main.rs:2447-2450`, `main.rs:2555-2557`, `main.rs:1068-1073`, `main.rs:4477-4479`, `input/ui_focus.rs:311-316`, `ui_focus.rs:357-362`.
  - *Slider capture.* A queued `nav.options` reaching `apply_slider_nav_capture` on the tree beneath must not step the slider while the panel opens (`main.rs:2504`; `input/ui_focus.rs:53-62`).
  - *Close paths* (context for B3, fixed in index). Today Running-frame `nav.cancel` pops only `pauseMenu` or a frontend submenu, while Frontend-frame cancel pops any non-root modal; the panel and the first-launch hold need one policy. Evidence: `main.rs:2555-2568`, `main.rs:5612-5617`, `context/lib/input.md` ("cancel closes only an active `pauseMenu`").
- **Raw-capture decision stage** (T8, P-new-4; shared with U3). A rebind confirm captured at N arms the capture at N+1's game-logic activation, after N+1's Input stage has read the global input. Decide the exception when the App consumes the press, after that frame's activations, and require the capture prompt to be the active tree, so the panel never opens over an armed prompt. Evidence: `main.rs:2447-2450`, `main.rs:2535`, `main.rs:2126`, `main.rs:2555`.
- **First-launch hold boot state** (A4, B9). The hold must be its own `BootState`, not a Frontend frame with the panel pushed: Loading, Frontend, and Running frames drain the level-request queue at the frame top, which would start the host's `Relevel` load under the panel. Endpoint polling is per-state code, so the hold must call `poll_world_less_transport` or the keepalive lapses. Evidence: `startup/lifecycle.rs:203-208`, `main.rs:240`, `lifecycle.rs:241-282`, `startup/splash_lifecycle.rs:57-64`, `splash_lifecycle.rs:103-109`, `lifecycle.rs:500`, `main.rs:2319-2324`, `main.rs:4831-4836`, `startup/mod.rs:26-32`, `context/lib/networking.md` (held admitted slot lives while its keepalive survives).
- **`--connect` request vs. backdrop** (T10, P-new-3). `enqueue_level_request` keeps only the latest `Load`, and `populate_frontend` enqueues the backdrop when the frontend presents. With the hold, `Relevel` is queued first and the backdrop evicts it on close; the host sends the current map once on admission and does not resend. The same reversal occurs in the OS-reader splash-wait frames on any launch if they poll the transport. Settle which request outranks the other so the host's map wins. Evidence: `startup/lifecycle.rs:232-256`, `main.rs:5256-5264`, `startup/splash_lifecycle.rs:119`, `splash_lifecycle.rs:154-160`, `lifecycle.rs:261-281`, `context/lib/networking.md` (current map sent once on admission).
- **Boot module anchors** (A5, A6). The OS reader's bounded wait adds splash frames between mod init and the splash clear or boot-map enqueue, which today run in one presented frame; the hold adds a `BootState` and its dispatch and level-request gating in `startup/lifecycle.rs` (~4.3k lines, production code alone past ~800 — split-first, absent from the index's split-first list). Evidence: `startup/splash_lifecycle.rs:119-205`, `startup/mod.rs:26-32`, `startup/lifecycle.rs:198-230`, `lifecycle.rs:1473-1480`.
- **Missing-entry warning placement** (A2, B15). `UiTreeRegistry::register_with_presentation` sees only name, tree, and tier; the frontend menu is `ModManifest.frontend.menuTree`, held on `session.frontend.menu_tree`, and both mod init and staged reload commit trees before the frontend declaration. Run the check App-side once both have committed, on mod init, level load, and staged reload. Evidence: `ui/src/modal_stack.rs:125-151`, `startup/splash_lifecycle.rs:335-347`, `main.rs:5199-5206`, `main.rs:5236-5242`.
- **Load-loop splash edges** (T1, P-new-5; A9). The splash path writes the swapchain itself, so a gameplay → splash edge is an onset the limiter cannot suppress; under a "returns always pass" rule the first resolve frame after a load is a return and passes, so every load cycle presents a full flash. Reviewer's shape: a presented splash frame becomes the rest level; the first resolve frame after a load is an onset against the splash, suppressed when over budget and released at the capped rate. Runtime Loading frames are the uniform clear with no logo — the luminance to enter. Evidence: `context/lib/boot_sequence.md` §1 (splash clears the swapchain directly; Loading keeps painting it), `startup/lifecycle.rs:517-519`, `lifecycle.rs:560-575`, `render/splash_pass.rs:266-271`, `render/renderer_splash.rs:28`, `startup/splash_lifecycle.rs:155-157`, `lifecycle.rs:569-571`.
- **Resize** (T12, T13, T14, P-new-6). Every `WindowEvent::Resized` recreates `scene_color`; a reseed on resize restarts the 1 s budget (six flashes in 1 s after one alt-tab) and disables limiting for a whole drag. Keep the flash count and window across a resize; resample previous-frame luminance to the new grid. Evidence: `main.rs:1929-1933`, `render/renderer_frame.rs:167-192`, `render/screen_effects.rs:168-179`.
- **Smooth strobes and refresh rate** (T15, T16, P-new-7). A per-frame luminance delta misses a smooth strobe at high refresh: a 5 Hz sine of amplitude 0.5 changes at most 0.52 per frame at 30 Hz but 0.065 at 240 Hz, under a WCAG-style 0.1 threshold. Define a transition as an excursion accumulated across frames from the last extremum. AC 4 needs a smooth-strobe fixture; the dev strobes are square. Evidence: `sim/src/sim/frame_timing.rs:60-64`.
- **Hitch ceiling** (T17, P-new-8). `frame_dt` is raw elapsed time; the frame after a level install includes the install. Clamp one frame's rate-cap allowance to a frame-time ceiling. Evidence: `sim/src/sim/frame_timing.rs:60-64`, `frame_timing.rs:68-85`, `startup/lifecycle.rs:560-575`.
- **GPU timing pairs** (B16, B17). The luminance reduction runs as its own pass ahead of the resolve, so one timestamp pair cannot report both the limiter's time and the resolve's time with the limiter on and off, as AC 6 requires. The resolve records `timestamp_writes: None` today. Evidence: `render/screen_effects.rs:189-225`.
- **Strobe fixtures** (B20). AC 5 needs below-threshold and quarter-screen variants of the strobes and a mod whose `levelLoad` restarts the level on every load. The restart loop cannot live in `content/dev` without breaking the dev map; name its home (a test-fixture mod).

### U2

- None this round.

### U3

- **Raw-capture decision stage** (T8, P-new-4) — see U1; the capture half is U3's.
- **Keyboard default ahead of text entry** (A3, T7). Whichever of U1 and U3 lands first, the binding-table lookup for the global input runs before the text-entry key branch and on the press edge only.

### U4

- **AT target identity** (context for B12, fixed in index). The existing activation and slider-step paths key on an id string, so re-entering them as-is would activate a same-id widget in a new top tree. Evidence: `main.rs:5834` (`fire_focused_button_activation` takes a focused id), `main.rs:5773-5790` (slider step resolves `ui_focused_id` against the focus rects).

### U5

- **Recency baseline after re-participation** (T19, T20, P-new-10). A recency signal that "changes on every hit and holds until the next" also changes when a client's cleared slot receives the host's value after admission, re-participation, or a level install. Settle how the HUD tells a baseline from a hit. Evidence: `context/lib/networking.md` (exit from participating clears slot values; re-promotion registers the slot again; a level change demotes every participating slot), `main.rs:3486-3490` (crossings fire when a replicated value arrives).
- **Caption across level unload** (P-new-11). A caption still inside its minimum hold when the level unloads — `restartLevel` 0.1 s after the sound — must not appear, with its arrow toward an old-level emitter, in the new level. Loading frames run no game logic and UI time pauses (`context/lib/ui.md` §3).
