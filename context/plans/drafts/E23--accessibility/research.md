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
| Text scale | `UISettings::TextScaleFactor()` + changed event | — | — | existing `windows` 0.62.2 dependency |
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

**Dev-mod flash content.** `arena-lights.ts` fires one `flashScreen([1,0,0,0.5], 250)` plus a red vignette and a shake on the low-health crossing — saturated red, but a single flash, not a strobe. Its light "sweep" animates world lights, which D4 does not limit. The manual strobe proof needs authored content (index Open questions, U1).

**Audio.** Every sound enters `Audio::play`; today only from the scripted `playSound` reaction and a diagnostic tone. kira 0.12.0 accepts main-track effects only at manager build; a custom `Effect` is safe Rust and toggles at runtime through handle atomics. Main-track effects run after sub-track spatialization, so a mono fold composes after E12's panning regardless of landing order. `Audio::set_bus_volume` exists with a test-only caller.

**Damage direction.** All damage flows through `apply_damage_with_context` (`crates/entities/src/components/health.rs`). `DamageContext` names the attacker but no position; the attacker's transform yields a bearing only into `BrainComponent.damage_bearing`, and the player has no brain. The presentation layer is world-anchored only; off-screen instances are invisible. The `Ring` widget can draw a bearing arc but is not admitted in presentation templates.

**Window.** `window_attributes` creates the window visible on purpose (`boot_sequence.md` §Window visibility): the reverted hidden-until-first-present scheme hung Windows boot because an invisible window receives no `RedrawRequested`. U4's scheme shows the window right after adapter construction, before the first redraw is requested — medium confidence it avoids the hang; proven only on real Windows.
