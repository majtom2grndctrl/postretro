# UI Focus Accessibility And Visuals

## Goal

Make UI focus a first-class accessibility state and a designer-controlled visual state. The same focused node must drive keyboard/gamepad navigation, pointer focus, assistive-technology metadata, and the authored focus treatment. Renderer-owned fallback focus remains available, but it is no longer the only visual expression.

## Scope

### In scope

- Internal accessibility snapshot for the active UI tree.
- AccessKit projection behind an app-side adapter seam.
- Focus state included in accessibility metadata.
- Resolved names, roles, bounds, disabled, selected, checked, and live-region announcements in the accessibility snapshot.
- Author-facing focus visual descriptors for focusable widgets.
- New theme tokens for default/highlight text, button backgrounds, dark translucent panels, and contrast outline fallback.
- Default interactive label color changed from literal white to the default text token.
- Renderer support for at least background-color, text-color, and outline focus visuals.
- Demo frontend menu rethemed to use a dark translucent panel, near-white default text, and focused button background highlights.
- Tests for focus navigation, accessibility snapshot contents, theme fallback, and focus visual rendering data.

### Out of scope

- DOM/CSS-style cascade.
- Per-frame script callbacks for focus or accessibility.
- Runtime writes from UI presentation state back into game state.
- AccessKit types in script descriptors or SDK authoring APIs.
- Browser or HTML UI.
- Rich accessibility actions beyond the existing button activation and slider stepping.
- Localization beyond the current `LocalizedText` string shape.
- Full screen-reader parity testing on every OS in CI.
- Full increased-contrast settings UI. This plan defines the contrast outline token and resolver hook; preference plumbing can land in a separate player-options plan if it does not already exist.

## Acceptance criteria

- [ ] Focused UI node is exported in an accessibility snapshot with its role, resolved accessible name, bounds, focus state, disabled state, and interaction kind.
- [ ] Active tree `accessibleName` and role are present on the accessibility snapshot root.
- [ ] Button and slider accessible names resolve from either inline `label` or `labelledBy`.
- [ ] `selected`, `checked`, and `disabled` states still resolve from the existing descriptor fields and are present in the accessibility snapshot.
- [ ] `Announce` widgets produce live-region entries when visible, with `polite` default and authored `assertive` priority.
- [ ] Hidden widgets and their descendants are absent from both focus export and accessibility snapshot output.
- [ ] Focus navigation and activation behavior does not change: disabled nodes remain unreachable by nav/pointer focus and do not activate.
- [ ] An app-side accessibility adapter receives the active snapshot, focus changes, and live-region entries; a no-op adapter preserves behavior when no platform backend is linked.
- [ ] A widget can author a focus visual that changes background color on focus without drawing an outline.
- [ ] A widget can author a focus visual that changes text color on focus without drawing an outline.
- [ ] A widget can author an outline focus visual using authored color, thickness, and inset.
- [ ] Buttons can author an optional default background color without changing focus, activation, or accessibility behavior.
- [ ] Omitted focus visual uses the engine contrast outline fallback so existing menus remain navigable and visible.
- [ ] Unknown focus visual color tokens degrade to opaque magenta.
- [ ] Unknown focus visual spacing tokens resolve to zero.
- [ ] The engine default theme includes near-white default text, white highlighted text, default/highlight button backgrounds, dark translucent panel, and contrast outline tokens.
- [ ] The demo frontend menu uses tokenized panel fill, text colors, and button backgrounds; focused level buttons highlight by background color rather than a heavy outline.
- [ ] TypeScript and Luau SDK helpers expose the same focus visual fields and reject malformed focus visual descriptors.
- [ ] Generated SDK typedefs match the Rust wire shape.
- [ ] `cargo check` passes.
- [ ] New and migrated tests pass (`cargo test -p postretro` and SDK validation tests green).

## Tasks

### Task 1: Split Focus Export From UI Tree

Move focus-rect export types and traversal helpers out of the oversized `crates/postretro/src/render/ui/tree.rs` path into a focused UI module. Preserve behavior and tests. The split must keep `UiTree` as the owner of computed layout and keep `FocusRectList` as the frame-to-frame readback consumed by the app-side focus engine.

### Task 2: Define Accessibility Snapshot

Add renderer-side data types for an internal accessibility snapshot. It should be built from the same laid-out active tree and frame snapshot used for focus export. Nodes include stable id, role, resolved name, bounds, focus state, disabled state, selected/checked state, and interaction kind (the existing `NodeInteraction` carried on `FocusRect`). Live-region entries come from visible `Announce` widgets.

### Task 3: Resolve Accessible Names

Resolve `label`, `labelledBy`, and tree `accessibleName` into strings during accessibility snapshot build. `labelledBy` names an authored node id whose text content supplies the name. Missing or hidden label targets warn once and degrade to an empty name; they do not panic or block rendering.

### Task 4: Add Focus Visual Descriptor

Add authored visual descriptors to focusable widgets. Buttons gain an optional `background` color field for their normal surface. Widgets gain an optional `focusVisual` field. The initial focus visual variants are `backgroundColor`, `textColor`, `outline`, and `none`. `backgroundColor` carries a focused background color. `textColor` carries a focused text color. `outline` carries `color`, `thickness`, and `inset` fields. `color` is a color-token-capable untagged union (token name or literal RGBA); `thickness` and `inset` are spacing-token-capable untagged unions (token name or literal logical px), so an unknown spacing token resolves to zero and an unknown color token to opaque magenta per the theme degrade rules. `none` suppresses visual drawing but does not suppress focus or accessibility metadata. Descriptor factories may accept `focusVisual` on focusable-capable widgets; it applies only when the node is actually focusable.

### Task 5: Render Authored Focus Visuals

Replace the hardcoded top-layer focus-ring append with a focus-visual resolver. The resolver reads the focused node id, finds that node's exported visual descriptor, and emits the chosen presentation with correct layer ordering. Button `background` draws behind the button label in the normal state. `backgroundColor` draws behind the focused widget content or recolors its owned background quad. `textColor` recolors the focused widget's own text run. `outline` uses the existing quad path above content. If a visual cannot be applied to the focused widget, use the contrast outline fallback.

### Task 6: Broaden Theme Tokens

Add default theme tokens for `text.default`, `text.highlight`, `button.background.default`, `button.background.highlight`, and `controlOutline.contrast`. `text.default` is very light gray. `text.highlight` is white; it is an author-available convenience token — this plan defines it in the default theme but does not wire an engine consumer for it. `button.background.default` is the normal button surface. `button.background.highlight` is the focused button surface. `controlOutline.contrast` is the high-contrast and fallback focus outline color. Update `panel.default` to very dark gray with opacity. Migrate the required-token set from `focus.ring` to `controlOutline.contrast`: there is no token-validation function, so update the required-token assertion in `render/ui/theme.rs` (the `engine_default` token list test) and the sole runtime fallback site in `render/mod.rs` (`push_focus_ring`, which currently resolves `color("focus.ring")`). `focus.ring` is retired from the required set; update the semantic required set in `context/lib/ui.md` §2 on promotion. Default `Text` color and interactive button/slider label color resolve through `text.default`. TypeScript and Luau `Text` factories and interactive button/slider label construction must emit or resolve `text.default` rather than literal white.

### Task 7: SDK And Typedef Parity

Expose button `background` and widget `focusVisual` in TypeScript and Luau widget factories. Add validation for each focus visual variant and token-capable field. Validation rejects only structural malformations (unknown variant kind, missing or wrong-typed sub-field); it does not reject unknown token names — those remain valid authored input and degrade visibly at tree build per the theme contract. Regenerate typedefs so `sdk/types/postretro.d.ts` and the Luau typedef output match the SDK and Rust descriptors.

### Task 8: Demo Theme And Menu Update

Consolidate the active `hudTheme` from `content/dev/scripts/hud.ts` into `content/dev/scripts/theme.ts`. Export one dev theme. Update `content/dev/start-script.ts`, `content/dev/scripts/hud.ts`, and `content/dev/scripts/frontend-menu.ts` imports/usages accordingly. Level-select buttons author `focusVisual: { kind: "backgroundColor", color: color.button.background.highlight }`, use `color.button.background.default` when not focused, use the default near-white label color, and place content on `color.panel.default`.

### Task 9: Accessibility Backend Seam

Add an app-side accessibility adapter boundary that consumes the internal accessibility snapshot. Prefer AccessKit through the winit adapter unless version or platform constraints block it. The first implementation may be a no-op backend when no platform adapter is linked, but the data flow must be real: the active snapshot is produced each frame, focus changes are observable at the boundary, and live-region entries are delivered once per visibility activation. The app-side adapter holds the prior-frame announce visibility to compute the visible edge, since the per-frame snapshot is stateless.

### Task 10: Tests And Regression Coverage

Add tests for wire round-trip, SDK validation, accessibility snapshot name/state resolution, hidden node exclusion, disabled focus behavior preservation, render-data assertions for each focus visual variant (`backgroundColor`, `textColor`, `outline`), focus visual fallback (both omitted and present-but-inapplicable), and demo theme token use. Keep GPU work out of unit tests; assert draw-list/text-run data instead.

## Sequencing

**Phase 1 (sequential):** Task 1 — isolates focus export before adding more behavior to the oversized UI tree file.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4 — snapshot shape, name resolution, and descriptor shape are independent after the split.
**Phase 3 (sequential):** Task 5 — consumes the focus visual descriptor and focus export from Tasks 1 and 4.
**Phase 4 (concurrent):** Task 6, Task 7 — theme token expansion and SDK parity can proceed once descriptor names are fixed. Both edit the TS/Luau widget factories: Task 6 owns default text/label color resolution, Task 7 owns the `focusVisual`/`background` fields — partition by field so the concurrent edits do not collide.
**Phase 5 (concurrent):** Task 8, Task 9 — demo authoring and backend seam consume the settled snapshot/theme contracts.
**Phase 6 (sequential):** Task 10 — closes coverage across the final contracts.

## Rough Sketch

Current state:

- `UiReadSnapshot.focused_id` carries the app-side focused node into rendering.
- `FocusRectList` carries focusable rects, groups, interaction metadata, and `selected`/`checked`/`disabled` state back to the app.
- `input/ui_focus.rs` owns navigation, pointer focus, repeat, disabled skipping, and activation intent.
- `render/mod.rs` appends a hardcoded `focus.ring` outline for the focused node on the top UI layer.
- `descriptor/accessibility.rs` defines `Role` and `implicit_role`, but no platform-facing accessibility tree exists yet.
- `Announce` exists in SDK and descriptor surfaces but lays out as a non-visual zero-size node.

Proposed model:

- Focus truth stays app-side. Renderer receives only the focused id and draws presentation.
- Focus visual metadata is renderer-local resolver data. `FocusRectList` remains app-side focus readback, not visual metadata export.
- Accessibility snapshot builds from the active laid-out tree and the same slot/cell values used for draw and focus export. It is produced renderer-side and returned to the app as a per-frame readback alongside `FocusRectList`, not serialized to scripts.
- Accessibility snapshot is internal engine data first. A platform adapter can project it later without changing script authoring.
- Designer-authored visuals never affect focus behavior or accessibility state.
- Normal focus styling is a design treatment. Increased-contrast focus styling may augment or override it with `controlOutline.contrast`.
- Theme token names do not decide accessibility mode. User preferences or engine fallback policy choose when contrast outline is applied.
- AccessKit is a backend projection target, not PostRetro's UI model.
- Assistive-technology actions re-enter the existing app-side activation/value-step paths. They do not mutate game or UI state directly.

Integration rules:

- Do build accessibility from descriptors, focus export, layout bounds, and resolved frame state.
- Do keep AccessKit behind a narrow adapter. The rest of the engine talks in PostRetro accessibility snapshot terms.
- Do keep platform accessibility calls out of GPU/render-pass code.
- Do preserve frame ordering. Assistive-technology actions are an input source that reaches gameplay through the same queued seams as other UI actions.
- Do require stable authored ids for interactive widgets and any node referenced by `labelledBy`.
- Do not infer semantics from draw lists, glyph runs, colors, or focus visuals.
- Do not make AccessKit types part of the scripting surface.
- Do not let visual focus style decide whether a node is focusable or exposed to assistive technology.
- Do not add a platform-specific authoring vocabulary.

Implementation notes:

- Split first because `crates/postretro/src/render/ui/tree.rs`, `crates/postretro/src/render/ui/mod.rs`, and `crates/postretro/src/input/ui_focus.rs` are already large. Avoid mixing behavior changes with module movement.
- Keep renderer GPU ownership intact. Accessibility data construction is CPU-side; OS adapter calls belong outside renderer GPU code.
- Preserve the N to N+1 focus latency. Accessibility focus may trail the same way as rendering; do not introduce a new timing path.
- Keep descriptor fields additive and skip-serialized when omitted.
- Treat `focusVisual: "none"` as a visual override only. It must not remove the node from focus export or the accessibility snapshot.
- For `backgroundColor`, draw or mutate only the focused widget's own background surface. Do not recolor parent panels or sibling button surfaces. If no owned background surface exists, use a rect fill behind the focused widget content.
- For `textColor`, carry enough node/run identity in draw data to mutate only the focused widget's own text run. Do not recolor external `labelledBy` text or unrelated matching strings. If no owned text run exists, fall back to outline.
- Keep a renderer hook that can add or substitute the `controlOutline.contrast` outline when an increased-contrast preference is active. If that preference does not exist yet, leave the hook driven by the existing fallback path and do not add a settings UI in this plan.
- Changing global default focus visuals from contrast outline to a design treatment can be future work.

Promotion notes:

- Add a durable UI principle: accessibility is semantic, not visual.
- Capture that UI descriptors declare names, roles, state, focus behavior, and announcements.
- Capture that rendering chooses presentation, including focus visuals.
- Capture that accessibility adapters project engine-owned semantics to OS APIs.
- Capture that assistive-technology actions obey the same frame-order and reaction seams as keyboard/gamepad/pointer UI input.

## Boundary Inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Focus visual field | `focus_visual` | `focusVisual` | `focusVisual` | `focusVisual` | n/a |
| Button background field | `background` | `background` | `background` | `background` | n/a |
| Background-color visual | `FocusVisual::BackgroundColor` | `{ "kind": "backgroundColor", ... }` | `{ kind: "backgroundColor", ... }` | `{ kind = "backgroundColor", ... }` | n/a |
| Outline visual | `FocusVisual::Outline` | `{ "kind": "outline", ... }` | `{ kind: "outline", ... }` | `{ kind = "outline", ... }` | n/a |
| Text-color visual | `FocusVisual::TextColor` | `{ "kind": "textColor", ... }` | `{ kind: "textColor", ... }` | `{ kind = "textColor", ... }` | n/a |
| No visual | `FocusVisual::None` | `{ "kind": "none" }` | `{ kind: "none" }` | `{ kind = "none" }` | n/a |
| Background visual color | `color` | `color` | `color` | `color` | n/a |
| Outline color | `color` | `color` | `color` | `color` | n/a |
| Outline thickness | `thickness` | `thickness` | `thickness` | `thickness` | n/a |
| Outline inset | `inset` | `inset` | `inset` | `inset` | n/a |
| Focus text color | `color` | `color` | `color` | `color` | n/a |
| Default text token | `text.default` | `text.default` | `color.text.default` via `getDesignTokens` | `color.text.default` via `getDesignTokens` | n/a |
| Highlight text token | `text.highlight` | `text.highlight` | `color.text.highlight` via `getDesignTokens` | `color.text.highlight` via `getDesignTokens` | n/a |
| Default button background token | `button.background.default` | `button.background.default` | `color.button.background.default` via `getDesignTokens` | `color.button.background.default` via `getDesignTokens` | n/a |
| Highlight button background token | `button.background.highlight` | `button.background.highlight` | `color.button.background.highlight` via `getDesignTokens` | `color.button.background.highlight` via `getDesignTokens` | n/a |
| Panel token | `panel.default` | `panel.default` | `color.panel.default` via `getDesignTokens` | `color.panel.default` via `getDesignTokens` | n/a |
| Contrast outline token | `controlOutline.contrast` | `controlOutline.contrast` | `color.controlOutline.contrast` via `getDesignTokens` | `color.controlOutline.contrast` via `getDesignTokens` | n/a |
| Accessible name (tree envelope) | `accessible_name` | `accessibleName` | `accessibleName` | `accessibleName` | n/a |
| Label (widget) | `label` | `label` | `label` | `label` | n/a |
| Labelled by | `labelled_by` | `labelledBy` | `labelledBy` | `labelledBy` | n/a |
| Role | `role` | `role` | `role` | `role` | n/a |
| Announce widget | `Announce` | `announce` | `announce` | `announce` | n/a |
| Announce priority | `priority` | `priority` | `priority` | `priority` (`polite` default, `assertive` authored) | n/a |
| Accessibility snapshot | internal app/renderer type | not serialized to scripts | not exposed | not exposed | n/a |

## Open Questions

- Whether AccessKit through `accesskit_winit` can be linked at the current workspace dependency versions without expanding platform support or binary size beyond the feature's budget. It is the preferred first backend unless implementation research finds a blocker. Note `accesskit` core is already present transitively via egui's dev-tools feature; no postretro crate depends on `accesskit` or `accesskit_winit` directly today.
