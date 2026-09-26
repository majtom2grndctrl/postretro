# Epic 23 — Accessibility (Epic hub)

> **Status:** draft — epic index. Records cross-cutting decisions and each unit's contract; no task decomposition. Each unit gets its own brief in a sibling `E23--<unit-slug>/` folder, drafted when the unit comes up.
> **Research:** `research.md` — standards thresholds, OS-preference reach, dependency costs, source findings.
> **Related:** `context/lib/ui.md` §1.1, §2, §4, §5, §6 · `context/lib/player_options.md` · `context/lib/input.md` · `context/lib/rendering_pipeline.md` §7.8 · `context/lib/audio.md` · `context/lib/networking.md` · `context/lib/entity_model.md` · `context/lib/boot_sequence.md` §1 · `context/research/ui-layer.md` §15, §19 · `context/research/combat-events.md` §2 · roadmap Epic 12, Epic 13, Epic 16 §Weapon Feel.

## Goal

Meet the Game Accessibility Guidelines basic tier and the common FPS accessibility failures as engine mechanisms. Mods author where content must (theme variants, captions); OS accessibility settings seed defaults; menus reach native screen readers; gamepad menu navigation meets console conventions.

## Scope

### In scope

- **U1 Preferences and comfort floor** — accessibility preference substrate with OS seeding and readonly live `accessibility.*` slots, engine accessibility panel, frame-level flash limiter, reduce motion, per-bus volume, mono audio.
- **U2 Visual accessibility** — theme variants, tokenized engine visuals, authored focus visuals, contrast diagnostic, text scale.
- **U3 Gamepad and input** — console menu conventions, full remapping, gamepad look options, hold/toggle.
- **U4 Screen reader** — engine accessibility snapshot projected to native assistive technology.
- **U5 Hearing and directional cues** — captions, subtitles, a damage-bearing fact the mod HUD draws, sound-direction cues.

### Out of scope

- Co-op text chat / ping system — a new capability, not an accommodation over an existing one; no chat exists to make accessible. Sibling roadmap item.
- Aim assist — owned by roadmap Epic 16 §Weapon Feel.
- Difficulty and game speed — scripts own difficulty (`scripting.md`, "engine owns the floor; scripts own the taste").
- Self-voicing TTS — door kept open behind U4's snapshot, not built.
- Reading OS flashing-lights, caption on/off, mono audio, or the Windows screen-reader flag — no safe API (`research.md` §3); in-game options only.
- Whole-UI reference-space scaling — owner chose text scale.
- Mod-declared input actions — the action set stays engine-closed.
- The focus-group defect — fixed by a pre-epic direct build; the epic builds on it (Invariant I5).

## Direction

**Problem.** The UI epic built the authoring side of accessibility (G2 metadata: names, roles, `Announce`, selected/checked) and deliberately deferred its consumption. No epic owns player-side accommodations. So the metadata has no reader, and the mechanisms only the engine can supply do not exist: OS preference seeding, a photosensitivity floor, an assistive-technology bridge, rebinding, captions, gamepad menu conventions.

**Prior commitments.**
- `research/ui-layer.md` §15 — accessibility as a type-system precondition; `Announce` is a node. Consumed, not changed.
- `research/ui-layer.md` §19 — screen readers "out of scope for v1; revisit before public release." This epic is that revisit.
- `done/M13--sdk-typesafety-a11y` — selected/checked ride `FocusRect`, "where a screen reader will later" read them. U4's snapshot reads them there.
- `ui.md` §6 (no per-frame script callbacks) and §4 (activation resolves to named reactions or reserved UI actions). AT actions route to the existing activation and slider-step paths, never a script callback.
- `ui.md` §4 and §5 — closed reserved `ui.*` actions (`ui.exitToDesktop`, `ui.quitToMenu`) and engine-owned `core/ui/` descriptors (the on-screen keyboard, `done/M13--text-entry`). Followed: `ui.openAccessibility` joins the reserved set and opens an engine-owned panel (D11).
- `ui.md` §1.1 — tree precedence is engine < mod: a mod registration under an engine built-in's name shadows it. **Diverges:** the accessibility panel's registry name is reserved; a mod registration under it is rejected with a load-time diagnostic and the engine panel remains (D11). The panel is the one surface guaranteed to reach every accessibility field and the only limiter control; a shadowable panel would let content remove the reach D11 exists to guarantee. Other built-in names keep engine < mod precedence.
- `ui.md` §2 — theme precedence is engine default < mod override. **Diverges:** a third tier is added — engine default → mod base → selected variant. The owner chose this (D3): mods ship accessible variants, the player selects one, and the engine supplies a high-contrast fallback when the mod ships none. The variant tier sits above the mod so a player's accommodation is never shadowed by the content it accommodates.
- `ui.md` §3 and `player_options.md` §4 — `options.*` slots are writable, menu-scoped working copies, seeded when a menu opens and bridged back to `PlayerOptions`; readonly slots warn and no-op on write. Followed: accessibility fields get `options.*` working copies with that behavior unchanged. Mod content that honors a preference during play reads the new `accessibility.*` namespace instead — readonly, always live, resolved values (D2).
- `player_options.md` §4 — an unknown enum value fails the whole-document parse; the `surface_depth_quality` alias is the precedent for treating stored values as player data. U1 generalizes it: accessibility fields and U3 bindings fall back per field.
- `player_options.md` §6 — "keybind remapping … separate spec." U3 is that spec.
- `done/M13--input-breadth` — hold-to-repeat is per-container policy, `restoreOnReturn` is opt-in, and remappable UI bindings are out of scope ("UI nav reads fixed actions"). **Diverges:** U3 makes `restoreOnReturn` default on, adds an engine-default hold-to-repeat, and makes UI nav actions remappable. Reason: console-convention defaults serve gamepad players in every tree, including trees whose authors never set them; GAG basic-tier remapping includes menu actions. Cost: both default flips change behavior for every existing mod tree (One-way doors).
- `input.md` §7 — UI dispatch precedes action mapping. Followed: UI dispatch still runs first; it resolves nav intents from the rebindable binding table instead of hard-wired keys and buttons. Mechanism pinned by U3 brief.
- `done/M13--screen-space-effects` — the resolve pass is the gameplay path's sole swapchain writer and the seam later post passes extend. Followed: the frame limiter runs there (D4).
- `boot_sequence.md` §Window visibility — window created visible to avoid a Windows hang. **Diverges:** `accesskit_winit` requires a not-yet-visible window at adapter construction. U4 creates the window hidden and shows it immediately after the adapter exists — before the first redraw is requested, unlike the reverted reveal-after-first-present scheme that starved on `RedrawRequested`. Bound to a manual Windows boot proof (AC 31); a hang blocks U4 and returns to the owner.
- `done/movement--view-feel` D6 — the comfort scale lives in `PlayerOptions`, not descriptors. Followed: reduce motion drives `view_feel_scale`.
- `audio.md` / `networking.md` — sound is host-local presentation; no sound key on the wire. Captions derive client-side.
- `research/combat-events.md` §2 and `done/E16--combat-presentation-substrate` — facts, not policy: the engine renders; the author decides what a hit shows. Followed: the engine publishes a per-owner damage bearing; the mod HUD draws the indicator.
- `entity_model.md` latest-hit provenance — damage recency does not imply a spatial source. Followed: the player's bearing pairs with a source-known flag, the shape of the enemy brain's `damageBearing` and latest-hit provenance facts (`scripting.md` §11).
- `networking.md` §Presentation events vs. replicated state and `done/E16--combat-presentation-substrate` — transient combat feedback rides the unreliable Presentation channel; durable per-player state rides owner-private slots, the path `player.health` rides (`scripting.md` per-owner slots). **Diverges:** the damage bearing is transient feedback, yet it rides an owner-private replicated slot; no new message kind. Reason: the presentation layer is world-anchored only (`research.md` §5), so a screen-space bearing indicator has no anchor there, while a per-owner slot binds directly to a HUD `Ring`. Cost: a retained slot does not change on a repeat hit at the same bearing, so the HUD needs a recency signal (Open questions, U5).
- `index.md` "no `unsafe`" — new dependencies (`mundy`, `accesskit_winit`) keep `unsafe` internal; the engine adds none.
- E12 step 1 (Positional sound events, ready) — U5 builds on its spatial chokepoint without reopening its decisions.

**Alternatives rejected.**
- *Screen reader only* (the literal Epic 13 deferred line) — leaves every GAG basic-tier gap open.
- *Self-voicing TTS instead of native AT* — overrides the player's own screen-reader voice and rate, and duplicates what AccessKit exposes. Kept as an open door behind the same snapshot.
- *Mod-opt-in authority for preferences* — cannot guarantee photosensitivity safety; the engine frame limiter does, for every source (D4).
- *Channel-scoped limiter only* (on `screen.flash` and `screen.vignette`) — cheaper and never dims world content, but cannot guarantee Invariant I2 against a strobing UI panel color, light animation, or emissive. Kept as the limiter's first stage.
- *Live `options.*` slots for accessibility fields* — blurs honoring a preference with editing it, and diverges from the menu-scoped working-copy model of `player_options.md` §4. The readonly `accessibility.*` namespace separates the two.
- *Mod menus only for accessibility settings* — a mod whose menus omit a setting strands the player, and a script-writable limiter lets content disable the floor; the engine panel guarantees reach and makes D4 hold by construction.
- *Engine-drawn damage indicator behind an engine option* — puts hit presentation policy in the engine; a bearing fact lets each mod's HUD decide what a hit shows.
- *Whole-UI reference-space scale* — owner chose text scale.
- *One monolithic spec* — five subsystems and several executors exceed one execution contract.
- *Independent per-unit plans with no index* — the preference model, OS seeding, and authority rules would be re-decided in every unit.

**One-way doors.**

| Door | Decided in | Undo cost |
|---|---|---|
| Persisted `settings.toml` shape for bindings and OS-unset fields | U1 (unset fields), U3 (bindings) | Lives on players' machines; a later change needs migration or per-field fallback. |
| `themeVariants` manifest shape, caption authoring surface, `accessibility.*` slot names, new `options.*` working-copy names | U2 (manifest); U5 (captions); U1–U5 (slots) | Modder-facing primitive surface (`index.md` "Primitive surface is a contract"). |
| Damage-bearing slots: names and semantics — a single latest bearing, a source-known flag, and a recency signal, on an owner-private replicated slot | U5 (pinned by U5 brief) | Every mod HUD indicator binds to them. A later per-hit event lane or multi-hit list changes what each HUD binds; SDK types and docs update in the same pass. |
| `restoreOnReturn` default on; engine-default hold-to-repeat | U3 | Changes behavior for every existing mod tree that authors neither. Reversible pre-stable; the primitive-surface contract (`index.md` §2) puts SDK docs and types in the same pass as the flip. |
| Engine accessibility panel under `core/ui/`, its reserved registry name, and `ui.openAccessibility` | U1 | Reversible now; costlier once mods build menu flows around the reserved action. |
| `mundy`, `accesskit_winit` dependencies | U1, U4 | Reversible: each sits behind an app-side seam. |

## Cross-cutting decisions

**Preference model (D2).** An `accessibility` group in `PlayerOptions`. Each OS-seedable field stores *unset* until the player changes it. Unset resolves to the OS value, else the engine default. OS change events update unset fields live. Stored values use a tolerant format: an unknown value falls back for that field alone; the file is never discarded. Resolution order is Invariant I1. Two slot surfaces expose the group:
- `accessibility.*` — engine-owned, readonly, always live. Each slot carries its field's resolved value, current during play whether or not a menu is open. Mod content reads it to honor a preference in its own content (reduce motion, captions); no script writes it (Invariant I11).
- `options.*` — the writable working copies of `player_options.md` §4, unchanged: seeded when a menu opens, written by menu controls, bridged back to the store. A mod options menu that surfaces an accessibility field writes its `options.*` working copy exactly like an existing option. The limiter's working copy takes writes from the engine panel alone (D11).

Accessibility preferences are presentation-only and client-local: honored only where presentation runs on the affected player's machine — UI trees, screen effects, audio, captions — and never fed to simulation (Invariant I9). U1 builds the substrate and the `accessibility.*` projection; later units add their own fields to both.

**Authority (D3).** Mods present accessible variants of their theme; players enable them in options; OS settings are the default before the player chooses. High contrast with no mod variant → an engine high-contrast token set layered over the mod theme. Layering: engine default → mod base → selected variant (mod's, or the engine fallback). Applied at the single theme chokepoint and preserved across staged reload (Invariant I6).

**Photosensitivity floor (D4).** Engine flash limiter, on by default. The player may disable it from the engine panel (D11); mods cannot. Two stages, both off when the limiter is off:
- *Channel clamp* (first stage, cheap): `screen.flash` and `screen.vignette` values are limited as they pack into the resolve's effect uniform, by the same rules as the frame limiter.
- *Frame limiter* (the limiter of record): runs in the screen-effects resolve pass, the gameplay path's sole swapchain writer (`done/M13--screen-space-effects`), on the whole composited frame. It reduces the frame to downsampled luminance, compares it against the previous frame, and limits: ≤ 3 flash transitions per second, saturated-red transitions (R/(R+G+B) ≥ 0.8) desaturated, full-screen intensity change per frame capped. A change covering less than the WCAG flash-area threshold (`research.md` §2) passes unchanged; the U1 brief pins the threshold in pixels and the transition and intensity thresholds.

Placement: every gameplay scene and UI pass renders into `scene_color`, which the resolve samples, so the resolve is the one point every source crosses — world geometry, light animation, emissives, UI panels, screen effects. Every source is covered and Invariant I2 holds as written. A channel-scoped limiter sees only the two effect slots. All GPU work stays in the renderer (`index.md` §2). Cost: one small GPU reduction per frame, measured (AC 6). The heuristic can dim legitimate bright content — a large, sudden, full-screen brightening reads as a flash; AC 6 checks that ordinary gameplay brightness changes pass. The boot splash writes the swapchain directly and draws only the static logo, so it carries no flash source.

**Settings reach (D11).** An engine-owned accessibility panel is the floor: a `core/ui/` descriptor opened by the reserved UI action `ui.openAccessibility`. Its registry name is reserved and cannot be shadowed: a mod registration under it is rejected with a load-time diagnostic and the engine panel remains (Prior commitments, `ui.md` §1.1). The engine fallback frontend and pause menus carry an entry to it; a mod menu reaches it through the same action. On first launch — no stored record that the panel was shown — it opens when the splash clears, before any sound plays or screen effect draws (`research.md` §2, first launch). The limiter control lives only in the panel: `options.flashLimiter` takes writes from the panel alone and `accessibility.flashLimiter` is readonly, so D4's "mods cannot" holds by construction. Every field in the `accessibility` group appears in the panel; each unit adds its fields there as it adds them to the substrate. Mods may surface the fields other than the limiter in their own menus through their `options.*` working copies (D2). The panel resolves theme tokens, so it follows the mod's look. Panel reach is Invariant I10.

**Each unit ships its dev-mod consumer in the same unit (D1).** A unit is not done until `content/dev` exercises it.

## Units

### U1 — Preferences and comfort floor

**Purpose.** The substrate every other unit writes into, plus the comfort mechanisms that need no UI rework.

**Contents.**
- `accessibility` group in `PlayerOptions`; readonly, always-live `accessibility.*` slots carrying resolved values; menu-scoped `options.*` working copies; tolerant per-field storage; unset/OS/default resolution (Cross-cutting: preference model).
- OS reader: `mundy` for contrast, reduced motion, color scheme; Windows `TextScaleFactor` via the existing `windows` crate. Change events update unset fields live.
- Flash limiter, both stages — channel clamp and frame limiter in the resolve pass — plus a GPU timing entry that reports the limiter's cost (Cross-cutting: photosensitivity floor).
- Engine accessibility panel and `ui.openAccessibility` (Cross-cutting: settings reach): the `core/ui/` descriptor, its reserved non-shadowable registry name, entries in the fallback frontend and pause menus, first-launch display, and the sole limiter control.
- Client-local preference rule (Cross-cutting: preference model), including co-op execution of screen-effect reactions (Open questions).
- Reduce motion (D5): one switch, OS-seeded, driving `view_feel_scale`, a new screen-shake scale, and UI tween snapping; per-effect sliders beneath it.
- Per-bus volume (Master / SFX / Music / UI) and a mono fold as a kira main-track custom effect with a crossfaded toggle (D10).

**Depends on.** Nothing.

**Dev-mod consumer.** `content/dev` options menu gains the accessibility group and a button that opens the panel through `ui.openAccessibility`; `arena-lights.ts` low-health flash and vignette exercise the channel clamp. The dev mod has no strobe today (`research.md` §5), so U1 authors the strobe content AC 4–6 need: a full-screen `screen.flash` strobe, a strobing UI panel color, and a strobing light animation.

**Key acceptance.** AC 1–13.

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

**Key acceptance.** AC 14–18.

**Brief.** `context/plans/drafts/E23--visual-accessibility/` — drafted when the unit comes up.

### U3 — Gamepad and input

**Purpose.** Console-convention menu navigation and full input remapping (D8).

**Contents.**
- `restoreOnReturn` on by default; engine-default hold-to-repeat where a container authors none; slider hold-repeat with acceleration. Both default flips diverge from `done/M13--input-breadth` (Prior commitments).
- LB/RB tab/page intent and a tabbed pattern; scroll container with scroll-into-view.
- Nested focus groups reachable by navigation: a focus-policy container inside another group is today reachable only by pointer, `focusNeighbors`, or `initialFocus`, and focus cannot leave it by policy. The tabbed pattern (tab strip group above a panel group) is the first consumer and pins the enter/exit rule.
- Device-family button-prompt glyphs following input mode; confirm/cancel layout swap option; on-screen-keyboard shortcuts.
- Confirmation dialogs for destructive actions, default focus on the safe choice.
- Full remapping — keyboard/mouse and gamepad, UI nav actions included — with conflict detection, reset, a guard keeping confirm and cancel reachable, and a raw-capture input path. UI dispatch still precedes action mapping; it reads nav intents from the binding table (Prior commitments, `input.md` §7).
- Gamepad look sensitivity, dead zone, invert-Y; generalized hold/toggle (sprint) on the crouch-mode pattern.

**Depends on.** Nothing in this epic. Baseline: focus-group fix landed pre-epic (direct build). Bindings and gamepad options are fields on U1's substrate; U1 and U3 run concurrently, so whichever brief lands first pins the tolerant per-field storage and the `accessibility.*` projection, and the other adopts them.

**Dev-mod consumer.** Dev menus restructured: options as a spatial grid with tabs, level select spatial; EXIT/QUIT gain confirmation; options menu gains remapping.

**Key acceptance.** AC 19–25; AC 33 if U3 lands after U4.

**Brief.** `context/plans/drafts/E23--gamepad-input/` — drafted when the unit comes up.

### U4 — Screen reader

**Purpose.** Native assistive-technology support for menus (D9).

**Contents.**
- `accesskit_winit`, always on for desktop. Window created hidden, shown right after the adapter exists.
- Engine-built accessibility snapshot: resolved names (inline label and `labelledBy`), roles, states, bounds, slider values, `Announce` live regions. Behind an app-side adapter seam; AccessKit types never enter descriptors or the SDK (Invariant I3).
- AT actions (activate, increment/decrement) re-enter the existing activation and slider-step paths.
- Self-voicing TTS stays an open door behind the same snapshot.

**Depends on.** Nothing in this epic. Baseline: focus-group fix landed pre-epic (direct build) — the snapshot's focusable set is the focus export. U3 runs concurrently; whichever of U3 and U4 lands second brings U3's tabs, scroll containers, and confirmation dialogs into the snapshot (AC 33).

**Dev-mod consumer.** Frontend, options, pause, and on-screen keyboard carry complete names and roles; an `Announce` reports level-load or state changes in a dev menu.

**Key acceptance.** AC 26–33.

**Brief.** `context/plans/drafts/E23--screen-reader/` — drafted when the unit comes up.

### U5 — Hearing and directional cues

**Purpose.** Make audio information visible (D10).

**Contents.**
- Captions keyed per sound asset — covers today's `playSound` and E12's per-event sound fields without reshaping them — plus a scripted subtitle primitive with speaker.
- Caption box: background opacity option, size tied to text scale, direction arrow computed where the sound plays and updates.
- Damage-bearing fact: a per-owner latest bearing, a source-known flag, and a recency signal, computed at the damage chokepoint and delivered to the owning client over existing owner-private replication — a declared divergence from the presentation lane (Prior commitments). The mod HUD draws the indicator. An indicator on/off, if a HUD offers one, is the mod's own setting, not an engine option.
- Sound-direction cues for positional sounds.
- Caption keys never go on the wire; captions derive client-side from sounds the client plays (Invariant I4).

**Depends on.** Damage-bearing part: nothing. Caption and cue part: E12 step 1's spatial chokepoint (positional plays carry the emitter point the arrow needs) and U2 (caption size follows text scale).

**Dev-mod consumer.** Dev sounds (`sfx/test_tone` and E12's dev descriptor sounds) gain captions; a dev script uses the subtitle primitive with a speaker; the dev HUD draws a `Ring`-based damage indicator from the bearing fact.

**Key acceptance.** AC 34–37.

**Brief.** `context/plans/drafts/E23--hearing-cues/` — drafted when the unit comes up.

## Acceptance criteria

Unit tag in brackets. "Manual" rows are verified by a person on real hardware.

**U1**
- [ ] 1. [U1] An OS-seedable setting the player never changed follows the OS value at boot and follows a live OS change without restart. Once the player changes it, a later OS change leaves it unchanged. With no OS value available it resolves to the engine default.
- [ ] 2. [U1] A `settings.toml` with an unrecognized value in one accessibility field loads every other setting intact; that field falls back to unset (or its default) with a warning. A file with no accessibility group loads with every field unset. Saving writes a never-changed field as unset, not as the value it resolved to.
- [ ] 3. [U1; each later unit for its own fields] During play with no menu open, mod content reads each `accessibility.*` slot as its field's resolved value, and sees a player change or an OS change to an unset field without a level reload. A script write to an `accessibility.*` slot warns and leaves the slot and the stored setting unchanged. An accessibility `options.*` working copy behaves as existing options do: a mod menu's write to it updates the stored setting and the matching `accessibility.*` slot; an OS change made while no menu is open reaches `accessibility.*` at once and the working copy at the next menu open.
- [ ] 4. [U1] With the limiter on (the first-boot default): four full-screen flashes inside 1 s produce at most three visible flash transitions, while three inside 1 s all appear; a saturated-red full-screen transition appears desaturated, while a non-red one keeps its hue; a full-screen intensity change larger than the cap is reduced to the cap in that frame, while a change below the cap passes unchanged. With the limiter off, both stages pass content unchanged: `screen.flash` and `screen.vignette` reach the resolve as authored, and presented frames match a resolve with no limiter stage.
- [ ] 5. [U1] The frame limiter covers every source: with the limiter on, a full-screen strobing UI panel color and a strobing light animation filling the view, each faster than 3 Hz, are each held to at most three visible transitions per second, as a `screen.flash` strobe is. The same strobe drawn over an area below the flash-area threshold passes unchanged, while its full-screen version is limited.
- [ ] 6. [U1] Limiter cost is recorded: with `POSTRETRO_GPU_TIMING=1` on the dev map, GPU timing reports the limiter's time with the limiter on and the resolve's time with it on and off; the added time is written into U1's proof. Manual: with the limiter on, a muzzle flash over a small area and walking from a dark room into a lit one look the same as with it off, while the dev-mod strobes of AC 4 and 5 are visibly tamed.
- [ ] 7. [U1] The panel's limiter control turns the limiter off and back on. No mod-authored content — a mod menu's write to `options.flashLimiter`, a write to `accessibility.flashLimiter`, a reaction, manifest data — changes the limiter setting or raises its limits.
- [ ] 8. [U1] Reduce motion on: a UI tween reaches its target the frame it starts, and view feel and screen shake apply the reduced scale. Off: each effect follows its own per-effect slider, and those slider values survive toggling the switch.
- [ ] 9. [U1] Changing one bus volume changes only that bus's output level. Mono on: a hard-panned source outputs equal left and right; off: stereo returns. Toggling mono mid-sound produces no click.
- [ ] 10. [U1; each later unit for its own fields] The engine fallback frontend and pause menus each open the accessibility panel; a mod menu button whose `onPress` is `ui.openAccessibility` opens it too. Closing the panel returns to the menu that opened it. The panel draws with the mounted mod's theme tokens and carries every field in the `accessibility` group.
- [ ] 11. [U1] A mod tree registered under the panel's reserved name — at mod scope, at level scope, or on a staged reload — is rejected with a load-time diagnostic; boot or level load continues, and every AC 10 path still opens the engine panel, not the mod tree. A mod tree registered under another engine built-in's name (`pauseMenu`) still shadows that built-in.
- [ ] 12. [U1] On first launch the panel opens before any sound plays or screen effect draws, whether boot presents the frontend or loads a map. A later launch skips it.
- [ ] 13. [U1] In co-op each machine applies its own reduce-motion and limiter settings to its own screen: a client with both on sees them applied while the host has both off, and a client with both off sees authored effects while the host has both on. No replicated message carries an accessibility setting.

**U2**
- [ ] 14. [U2] Selecting a mod-shipped variant restyles the UI from it; tokens the variant omits resolve from the mod base, then the engine default. High contrast with no mod variant applies the engine high-contrast set over the mod theme. Deselecting restores the mod base. The selection survives a staged reload, including one that changes the mod's base theme; a reload that removes the mod's high-contrast variant falls back to the engine set.
- [ ] 15. [U2] Theme drain warns on a text/background pair at 3:1 and is silent at 4.5:1; warns on a focus-indicator pair below 3:1 and is silent at 3:1. The theme still loads either way.
- [ ] 16. [U2] Overriding the button-label, slider track, thumb, and fill tokens restyles those parts; changing the focus-ring token no longer changes slider fill. Background, text-color, and outline focus visuals each render on focus; an omitted focus visual draws the engine outline.
- [ ] 17. [U2] At 200% text scale, text renders at twice its size, containers reflow to fit it, and text with an authored max width wraps at that width; presentation-layer text scales too. At 100% layout matches the pre-epic layout. A scale change applies on the next frame without restart.
- [ ] 18. [U2] Dev frontend, options, and pause menus draw no literal colors; under the dev high-contrast variant the contrast diagnostic reports no warnings. Manual: high-contrast and colorblind-safe variants are legible in play.

**U3**
- [ ] 19. [U3] Closing a submenu returns focus to the widget that opened it with no authoring; a tree that opts out lands on its initial focus.
- [ ] 20. [U3] A tap on a nav direction moves focus one step with zero repeats. A hold moves one step, then repeats after the delay at the interval, and stops on release — with no container authoring. A held slider step accelerates with hold time and clamps at its bounds without overshoot.
- [ ] 21. [U3] A remapped binding round-trips through `settings.toml`. A UI nav action remapped to a new key or button drives menu navigation from the new input and no longer from the old one. An unknown key string falls back to that binding's default without discarding other bindings or settings. Unbinding the last confirm or last cancel binding is refused. A conflicting assignment is reported. Reset restores defaults. The capture prompt receives keys the UI otherwise swallows.
- [ ] 22. [U3] Gamepad look sensitivity, dead zone, and invert-Y change gamepad look and leave mouse look unchanged. Sprint in toggle mode latches on press and releases on the next press; in hold mode it follows the button.
- [ ] 23. [U3] Button-prompt glyphs match the device family of the last input and switch on the first input from another family. The confirm/cancel swap option swaps both the behavior and the glyphs.
- [ ] 24. [U3] LB/RB switches tabs in a tabbed menu. Moving focus to an off-screen child of a scroll container scrolls it into view. EXIT and QUIT open a confirmation with focus on the safe choice; cancel dismisses without acting. On-screen-keyboard shortcut buttons act without moving focus to the key they stand for.
- [ ] 25. [U3] Manual: a gamepad-only pass completes every dev menu on Xbox and on PlayStation or Nintendo layouts.

**U4**
- [ ] 26. [U4] The accessibility snapshot names a widget from its inline label and from `labelledBy`; a missing `labelledBy` target yields an empty name and a warning, never a panic. Hidden subtrees are absent. Selected, checked, disabled, bounds, and slider value are present. The snapshot's focusable set matches the focus export: a `Text` inside a focus group is not focusable.
- [ ] 27. [U4] An `Announce` that becomes visible produces one live-region entry at its priority — polite by default, assertive when authored. Staying visible produces no repeat; a hidden `Announce` produces none.
- [ ] 28. [U4] Activating a button through AT runs the same reaction keyboard confirm runs; a disabled button does nothing. AT increment/decrement steps a slider exactly as one gamepad step does.
- [ ] 29. [U4] A focus change reaches the platform adapter no later than the frame the focus ring shows it; no new focus latency is added.
- [ ] 30. [U4] Generated SDK typedefs and the descriptor wire contain no platform-accessibility type.
- [ ] 31. [U4] Manual: Windows boot with the hidden-at-create window reaches the frontend without hanging.
- [ ] 32. [U4] Manual: NVDA (Windows), VoiceOver (macOS), and Orca (Linux) read and operate frontend, options, pause, and the on-screen keyboard. Binary-size delta of the new dependencies is recorded per platform.
- [ ] 33. [U4, or U3 if it lands after U4] U3's tabs, scroll containers, and confirmation dialogs appear in the snapshot with their roles; the selected tab reports selected.

**U5**
- [ ] 34. [U5] A sound with an authored caption shows it while playing; a sound without one shows none. A positional sound's caption carries a direction arrow that tracks the listener's turn; a non-positional sound's carries none. A scripted subtitle shows its speaker. Background opacity 0% draws no box; 100% draws an opaque box. Manual: captions readable at 100% and 200% text scale.
- [ ] 35. [U5] A hit from the player's left sets the owning client's bearing to the left with the source known, and the dev HUD indicator points left; a hit from behind does the same behind. Damage with no known source position sets the source-known flag false, and the dev HUD shows no direction. A second hit from the same bearing re-triggers the dev HUD indicator; with no new hit, it does not re-trigger. In co-op a non-owning client never receives another player's bearing.
- [ ] 36. [U5] A co-op client shows captions and cues for sounds it plays locally; U5 adds no caption or sound key to any wire message and no new message kind.
- [ ] 37. [U5] With cues on, a positional sound off-screen produces a cue on its side; a 2D, UI, or music sound produces none; with cues off, none appear.

**Baseline (pre-epic direct build)** — a regression guard; U3 and U4 preserve it.
- [ ] 38. [Pre-epic] Inside a focus group, only interactive widgets receive focus: `Text` and `VStack` are skipped; `Button`, `Slider`, and authored-id interactive widgets are reachable. In the dev title menu, nav down from EXIT lands on PLAY.

## Sequencing

**Phase 1 (concurrent):** U1, U3, U4, U5 damage-bearing part — each builds on existing substrate or the pre-epic focus baseline. U1 and U3 share settings storage and the `accessibility.*` projection (U3 Depends on); U3 and U4 share snapshot coverage (U4 Depends on).
**Phase 2:** U2 after U1 — its fields, `accessibility.*` slots, and OS seeding sit on U1's substrate.
**Phase 3:** U5 caption and cue part after E12 step 1 lands its spatial chokepoint — the arrow needs the emitter point — and after U2 — caption size follows text scale.

Each unit is re-grounded against the live tree when its brief is drafted.

## Invariants

| # | Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|---|
| I1 | Preference resolution: player-set > OS value > engine default. An OS change never overrides a player-set value. | U1 | Every unit adding an OS-seedable field (U2 variant, text scale); the save path writing a resolved value as player-set | AC 1, 2 |
| I2 | Flash floor enforced engine-side on every presented gameplay frame, whatever the source; no mod bypasses it while enabled. | U1 | Any mod write to `options.flashLimiter` or `accessibility.flashLimiter` (the panel is the sole writer); a mod tree under the panel's reserved name; any pass that writes the swapchain after or instead of the resolve (the boot splash draws only its static logo; the dev-tools egui overlay is engine-owned and dev-only); any later post pass that does not compose before the limiter | AC 4, 5, 7, 11, 13 |
| I3 | AccessKit types never enter descriptors or the SDK. | U4 | U2 focus-visual descriptors; U5 caption authoring surface | AC 30 |
| I4 | Caption keys never go on the wire; captions derive client-side from local playback. | U5 | U5 damage bearing, which rides existing owner-private slots and adds no message kind; E12 step 4 peer audio, which may carry sounds but never captions | AC 35, 36 |
| I5 | Focus groups admit only interactive widgets. | Pre-epic direct build | U3 tabs and scroll containers; U4 snapshot built from focus export | AC 26, 38 |
| I6 | The player's selected theme variant survives staged reload. | U2 | Staged-reload theme commit replacing the mod override | AC 14 |
| I7 | Stored settings degrade per field; the file is never discarded. | U1 | U3 bindings; U2 and U5 new enum fields | AC 2, 21 |
| I8 | AT actions route to the existing activation and slider-step paths, never a script callback. | U4 | U3 raw-capture and new intents | AC 28 |
| I9 | Accessibility preferences are client-local presentation: applied on the affected player's machine, never read by simulation; `accessibility.*` slots and accessibility `options.*` working copies are never replicated. | U1 | Any mod or engine sim path reading an `accessibility.*` slot; host-side execution of presentation reactions in co-op; each later unit's new fields | AC 13 |
| I10 | The engine accessibility panel is always reachable through `ui.openAccessibility` and carries every `accessibility` field; no mod registration replaces it. | U1 | Each later unit adding fields; mod- and level-scope tree registration and staged reload under the reserved name | AC 10, 11 |
| I11 | Every `accessibility` field has a readonly `accessibility.*` slot carrying its resolved value, current during play; no script writes it. | U1 | Each later unit adding fields (U2, U3, U5); the options bridge, which writes the store, never the `accessibility.*` slot directly | AC 3 |

## Boundary inventory

Existing convention: TOML keys snake_case (`player_options.md` §2); slots camelCase (`options.viewFeelScale`). Rust `PlayerOptions` fields match the TOML key. An `accessibility.*` slot takes the same camelCase suffix as its field's `options.*` working copy. Both carry the **resolved** value; unset is a storage state, not a slot value.

| Name | TOML key / Rust field | Working copy (`options.*`) | Resolved slot (`accessibility.*`) | Unit | Still pinned by brief |
|---|---|---|---|---|---|
| `accessibility.*` namespace | — (projection of the `accessibility` group, not a store) | — | camelCase on the wire; engine-owned, readonly (mod-readonly like `screen.vignette`), always live | U1 | — |
| Reduce motion | `reduce_motion` | `options.reduceMotion` | `accessibility.reduceMotion` | U1 | — |
| Screen-shake scale | `screen_shake_scale` | `options.screenShakeScale` | `accessibility.screenShakeScale` | U1 | range |
| Flash limiter | `flash_limiter` | `options.flashLimiter` (the engine panel is its only writer) | `accessibility.flashLimiter` | U1 | — |
| Bus volumes | `master_volume`, `sfx_volume`, `music_volume`, `ui_volume` | `options.masterVolume`, `options.sfxVolume`, `options.musicVolume`, `options.uiVolume` | pinned by U1 brief | U1 | unit and range; `accessibility` group membership |
| Mono audio | `mono_audio` | `options.monoAudio` | `accessibility.monoAudio` | U1 | — |
| OS-unset representation | — | — | — | U1 | TOML encoding; how a player returns a field to "follow OS" |
| Open-accessibility reserved action | — | `ui.openAccessibility` (button `onPress`) | — | U1 | — |
| Accessibility panel descriptor | `core/ui/` file | — | — | U1 | file name |
| Accessibility panel registry name | reserved tree name; a mod registration under it is rejected | — | — | U1 | name |
| First-launch record | panel-shown marker | — | — | U1 | TOML key |
| Text scale | `text_scale` | `options.textScale` | `accessibility.textScale` | U2 | range and step |
| Theme variant | `theme_variant` | `options.themeVariant` (variant id) | `accessibility.themeVariant` | U2 | "none" value; whether colorblind-safe has an engine fallback |
| Theme variants manifest key | `ModManifest.theme_variants` | `themeVariants` (same key in Luau) | — | U2 | map value shape; open vs. closed id set |
| Variant ids | — | `highContrast`, `colorblindSafe` | — | U2 | — |
| Focus visual field and kinds | — | — | — | U2 | names |
| New theme tokens (text, button, panel, slider) | — | — | — | U2 | names |
| Gamepad look | `gamepad_look_sensitivity`, `gamepad_dead_zone`, `gamepad_invert_y` | `options.gamepadLookSensitivity`, `options.gamepadDeadZone`, `options.gamepadInvertY` | pinned by U3 brief | U3 | ranges; `accessibility` group membership |
| Sprint mode | `sprint_mode` (`hold` / `toggle`) | `options.sprintMode` | pinned by U3 brief | U3 | `accessibility` group membership |
| Confirm/cancel swap | `swap_confirm_cancel` | `options.swapConfirmCancel` | pinned by U3 brief | U3 | `accessibility` group membership |
| Bindings | `[bindings]` table | — | — | U3 | table shape; physical-input string vocabulary |
| Captions | `captions` | `options.captions` | `accessibility.captions` | U5 | — |
| Caption background opacity | `caption_background_opacity` | `options.captionBackgroundOpacity` | `accessibility.captionBackgroundOpacity` | U5 | range |
| Sound cues | `sound_cues` | `options.soundCues` | `accessibility.soundCues` | U5 | — |
| Caption authoring (per sound asset) and subtitle primitive with speaker | — | — | — | U5 | names and shape |
| Damage bearing, source-known flag, recency signal | — | — | — (per-owner engine slots, not preferences) | U5 | slot names; bearing frame and units; recency signal shape |

## Rough sketch

Key modules per unit, from source at `2542ac2`. Files past ~800 lines that a unit extends are flagged for split-first. A split-first file extended by concurrent units is split by the first brief to land: `main.rs` (U1, U2, U4), `input/ui_focus.rs` (U3, U4).

- **U1** — `crates/postretro/src/options/mod.rs` (`PlayerOptions`), `options/bridge.rs` (`OptionsBridge`, slot constants), `crates/entities/src/engine_state_catalog.rs` (`accessibility.*` slots, readonly like `screen.vignette`). Limiter: channel clamp in `pack_effect_uniform` (`crates/render-cpu/src/screen_effects.rs`), downstream of the decay systems (`crates/sim/src/scripting_systems/{flash_decay,vignette_decay,shake_decay}.rs`); frame limiter in the resolve — `ScreenEffectsPass::encode_resolve` (`crates/renderer/src/render/screen_effects.rs`) and `crates/renderer/src/shaders/screen_effects.wgsl` — with a luminance reduction of `scene_color` ahead of it and previous-frame luminance plus transition history kept GPU-side, so no readback stalls the frame. The enable flag reaches the resolve through the slot snapshot `encode_resolve` already takes (`ui_snapshot.slot_values`, read by `pack_effect_uniform`). The resolve records no timestamp pair today (`timestamp_writes: None`); U1 adds one to the `POSTRETRO_GPU_TIMING` pass set for AC 6. Reserved panel name checked where mod trees register (`register_script_trees`, `crates/ui/src/modal_stack.rs`, ~1.3k lines — split-first). Audio in `crates/postretro/src/audio/mod.rs` (`Audio::set_bus_volume`, main-track effect at manager build); reserved action names in `crates/ui/src/actions.rs` (mirrored in the SDK preludes) and `classify_ui_button_action` in `main.rs` (14k lines — split-first); panel descriptor beside `core/ui/keyboard.json`, entries in `core/ui/frontendMenu.json` and `pauseMenu.json`.
- **U2** — theme chokepoint `apply_mod_ui_theme_to_renderer` and `commit_mod_ui_theme` in `main.rs` (split-first); `Renderer::set_ui_theme`; literals in `crates/ui/src/tree/build.rs` (`INTERACTIVE_LABEL_COLOR`); text measure `measure_run` (`crates/ui/src/text.rs`); focus ring `push_focus_ring` (`crates/renderer/src/render/ui/mod.rs`, ~1.8k lines — split-first); theme drain `drain_theme_js` / Lua.
- **U3** — `UiFocusEngine` (`crates/postretro/src/input/ui_focus.rs`, ~2.2k lines — split-first); `nav_intent_for_gamepad_button`, `StickNavTracker` (`input/ui_nav.rs`) — nav intents resolve from the binding table instead of fixed keys and buttons; `Binding`, `Action` (`input/types.rs`); `DEAD_ZONE` (`input/gamepad.rs`); `GAMEPAD_LOOK_SENSITIVITY` (`input/look.rs`); `UiIntentPayload` gains a raw-capture path; `resolve_crouch_intent` is the hold/toggle pattern.
- **U4** — new app-side adapter module; `window_attributes` and `App`'s `ApplicationHandler` (`main.rs`, split-first); snapshot built beside focus export (`FocusRect`, `widget_a11y_state` in `crates/ui/src/tree/`); `AnnounceWidget` and `labelledBy` from `crates/scripting-core/src/ui/descriptor/`; AT slider steps through `capture_slider_step` (`input/ui_focus.rs`, split-first).
- **U5** — `Audio::play` / `Audio::update`; bearing computed in `apply_damage_with_context` from `DamageContext` (`crates/entities/src/components/health.rs`), as the enemy path fills `BrainComponent.damage_bearing`; per-owner slot projection following `player.health`; dev HUD `Ring` for the bearing arc.

## Open questions

Each resolved by the unit named.

- **U1** — Whether a player FOV setting exists or camera FOV is configurable. Verify before adding an FOV slider.
- **U1** — `mundy`'s macOS main-thread requirement and Windows `SetWindowsHookExW` behavior under winit in this app.
- **U1, U4** — Binary-size delta of `mundy` (U1) and `accesskit_winit` (U4) per platform.
- **U1** — Whether frame capture, which tonemaps through its own pass (`encode_capture_tonemap`) rather than the resolve, runs the frame limiter. Automated proofs of AC 4 and 5 read frames; if they read through capture, capture must match the presented frame.
- **U1** — Where `flashScreen`, `screenShake`, and `vignette` reactions execute in co-op — host or affected client. Pin any gap so reduce motion applies on the affected player's screen (Invariant I9). The flash floor holds either way: each machine's resolve limits whatever that machine presents.
- **U4** — Whether creating the window hidden and showing it right after adapter construction avoids the `boot_sequence.md` Windows hang. Proven by AC 31 on real Windows.
- **U5** — How the HUD tells a new hit from a repeat hit at the same bearing: a recency fact beside the bearing (the brain scope pairs damage recency with bearing) or another signal. Pinned with the slot names.
- **U1, U3** — GAG tier labels for hold-to-toggle and multiple input devices (U3) and settings saved (U1). Confirm on live pages before docs cite a tier.
