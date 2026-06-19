# UI Focus Accessibility And Visuals

## Goal

Make UI focus a first-class accessibility state and a designer-controlled visual state. The same focused node must drive keyboard/gamepad navigation, pointer focus, assistive-technology metadata, and the authored focus treatment. Renderer-owned fallback focus remains available, but it is no longer the only visual expression.

## Scope

### In scope

- Internal accessibility snapshot for the active UI tree.
- Focus state included in accessibility metadata.
- Resolved names, roles, bounds, disabled, selected, checked, and live-region announcements in the accessibility snapshot.
- Author-facing focus visual descriptors for focusable widgets.
- Theme tokens for default text, focus-highlight text, panel background, and fallback outline.
- Default interactive label color changed from literal white to the default text token.
- Renderer support for at least outline and text-color focus visuals.
- Demo frontend menu rethemed to use a dark translucent panel, near-white default text, and pure-white focused button text.
- Tests for focus navigation, accessibility snapshot contents, theme fallback, and focus visual rendering data.

### Out of scope

- DOM/CSS-style cascade.
- Per-frame script callbacks for focus or accessibility.
- Runtime writes from UI presentation state back into game state.
- Browser or HTML UI.
- Rich accessibility actions beyond the existing button activation and slider stepping.
- Localization beyond the current `LocalizedText` string shape.
- Full screen-reader parity testing on every OS in CI.

## Acceptance criteria

- [ ] Focused UI node is exported in an accessibility snapshot with its role, resolved accessible name, bounds, focus state, disabled state, and interaction state.
- [ ] Active tree `accessibleName` and role are present on the accessibility snapshot root.
- [ ] Button and slider accessible names resolve from either inline `label` or `labelledBy`.
- [ ] `selected`, `checked`, and `disabled` states still resolve from the existing descriptor fields and are present in the accessibility snapshot.
- [ ] `Announce` widgets produce live-region entries when visible, with `polite` default and authored `assertive` priority.
- [ ] Hidden widgets and their descendants are absent from both focus export and accessibility snapshot output.
- [ ] Focus navigation and activation behavior does not change: disabled nodes remain unreachable by nav/pointer focus and do not activate.
- [ ] An app-side accessibility adapter receives the active snapshot, focus changes, and live-region entries; a no-op adapter preserves behavior when no platform backend is linked.
- [ ] A widget can author a focus visual that changes text color on focus without drawing an outline.
- [ ] A widget can author an outline focus visual using authored color, thickness, and inset.
- [ ] Omitted focus visual uses the engine fallback outline so existing menus remain navigable and visible.
- [ ] Unknown focus visual color or spacing tokens degrade through the existing visible fallback path.
- [ ] The engine default theme includes near-white default text, white focus-highlight text, dark translucent panel, and fallback outline tokens.
- [ ] The demo frontend menu uses tokenized panel fill and text colors, and its focused level buttons highlight by text color rather than a heavy outline.
- [ ] TypeScript and Luau SDK helpers expose the same focus visual fields and reject malformed focus visual descriptors.
- [ ] Generated SDK typedefs match the Rust wire shape.
- [ ] `cargo check` passes.

## Tasks

### Task 1: Split Focus Export From UI Tree

Move focus-rect export types and traversal helpers out of the oversized `crates/postretro/src/render/ui/tree.rs` path into a focused UI module. Preserve behavior and tests. The split must keep `UiTree` as the owner of computed layout and keep `FocusRectList` as the frame-to-frame readback consumed by the app-side focus engine.

### Task 2: Define Accessibility Snapshot

Add renderer-side data types for an internal accessibility snapshot. It should be built from the same laid-out active tree and frame snapshot used for focus export. Nodes include stable id, role, resolved name, bounds, focus state, disabled state, selected/checked state, and interaction kind. Live-region entries come from visible `Announce` widgets.

### Task 3: Resolve Accessible Names

Resolve `label`, `labelledBy`, and tree `accessibleName` into strings during accessibility snapshot build. `labelledBy` names an authored node id whose text content supplies the name. Missing or hidden label targets warn once and degrade to an empty name; they do not panic or block rendering.

### Task 4: Add Focus Visual Descriptor

Add an authored `focusVisual` descriptor to focusable widgets. The initial variants are `outline`, `textColor`, and `none`. `outline` carries color, thickness, and inset fields. `textColor` carries a focused text color. `none` suppresses visual drawing but does not suppress focus or accessibility metadata.

### Task 5: Render Authored Focus Visuals

Replace the hardcoded top-layer focus-ring append with a focus-visual resolver. The resolver reads the focused node id, finds that node's exported visual descriptor, and appends the chosen presentation to the same top-layer draw data. `outline` uses the existing quad path. `textColor` updates the focused text run color for text-bearing widgets.

### Task 6: Broaden Theme Tokens

Add default theme tokens for `text.default`, `focus.text`, `panel.default`, and `focus.ring`. `text.default` is very light gray. `focus.text` is white. `panel.default` is very dark gray with opacity. Existing required tokens remain valid. Default `Text` color and interactive button/slider label color resolve through `text.default`.

### Task 7: SDK And Typedef Parity

Expose `focusVisual` in TypeScript and Luau widget factories. Add validation for each variant and token-capable fields. Regenerate typedefs so `sdk/types/postretro.d.ts` and the Luau typedef output match the SDK and Rust descriptors.

### Task 8: Demo Theme And Menu Update

Fold `content/dev/scripts/theme.ts` into the dev mod theme path and update `content/dev/scripts/frontend-menu.ts` to use `getDesignTokens`. Level-select buttons author `focusVisual: { kind: "textColor", color: color.focus.text }`, use the default near-white label color when not focused, and place content on `color.panel.default`.

### Task 9: Accessibility Backend Seam

Add an app-side accessibility adapter boundary that consumes the internal accessibility snapshot. The first implementation may be a no-op backend when no platform adapter is linked, but the data flow must be real: the active snapshot is produced each frame, focus changes are observable at the boundary, and live-region entries are delivered once per visibility activation.

### Task 10: Tests And Regression Coverage

Add tests for wire round-trip, SDK validation, accessibility snapshot name/state resolution, hidden node exclusion, disabled focus behavior preservation, focus visual fallback, and demo theme token use. Keep GPU work out of unit tests; assert draw-list/text-run data instead.

## Sequencing

**Phase 1 (sequential):** Task 1 — isolates focus export before adding more behavior to the oversized UI tree file.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4 — snapshot shape, name resolution, and descriptor shape are independent after the split.
**Phase 3 (sequential):** Task 5 — consumes the focus visual descriptor and focus export from Tasks 1 and 4.
**Phase 4 (concurrent):** Task 6, Task 7 — theme token expansion and SDK parity can proceed once descriptor names are fixed.
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
- Focus export gains resolved visual metadata. The focused node's visual, not a global hardcoded ring, chooses what is drawn.
- Accessibility snapshot builds from the active laid-out tree and the same slot/cell values used for draw and focus export.
- Accessibility snapshot is internal engine data first. A platform adapter can project it later without changing script authoring.
- Designer-authored visuals never affect focus behavior or accessibility state.

Implementation notes:

- Split first because `crates/postretro/src/render/ui/tree.rs`, `crates/postretro/src/render/ui/mod.rs`, and `crates/postretro/src/input/ui_focus.rs` are already large. Avoid mixing behavior changes with module movement.
- Keep renderer GPU ownership intact. Accessibility data construction is CPU-side; OS adapter calls belong outside renderer GPU code.
- Preserve the N to N+1 focus latency. Accessibility focus may trail the same way as rendering; do not introduce a new timing path.
- Keep descriptor fields additive and skip-serialized when omitted.
- Treat `focusVisual: "none"` as a visual override only. It must not remove the node from focus export or the accessibility snapshot.
- For `textColor`, prefer carrying enough node/run identity in draw data to mutate only the focused node's own text run. Do not recolor every matching string.

## Boundary Inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Focus visual field | `focus_visual` | `focusVisual` | `focusVisual` | `focusVisual` | n/a |
| Outline visual | `FocusVisual::Outline` | `{ "kind": "outline", ... }` | `{ kind: "outline", ... }` | `{ kind = "outline", ... }` | n/a |
| Text-color visual | `FocusVisual::TextColor` | `{ "kind": "textColor", ... }` | `{ kind: "textColor", ... }` | `{ kind = "textColor", ... }` | n/a |
| No visual | `FocusVisual::None` | `{ "kind": "none" }` | `{ kind: "none" }` | `{ kind = "none" }` | n/a |
| Outline color | `color` | `color` | `color` | `color` | n/a |
| Outline thickness | `thickness` | `thickness` | `thickness` | `thickness` | n/a |
| Outline inset | `inset` | `inset` | `inset` | `inset` | n/a |
| Focus text color | `color` | `color` | `color` | `color` | n/a |
| Default text token | `text.default` | `text.default` | `color.text.default` via `getDesignTokens` | `color.text.default` via `getDesignTokens` | n/a |
| Focus text token | `focus.text` | `focus.text` | `color.focus.text` via `getDesignTokens` | `color.focus.text` via `getDesignTokens` | n/a |
| Panel token | `panel.default` | `panel.default` | `color.panel.default` via `getDesignTokens` | `color.panel.default` via `getDesignTokens` | n/a |
| Fallback outline token | `focus.ring` | `focus.ring` | `color.focus.ring` via `getDesignTokens` | `color.focus.ring` via `getDesignTokens` | n/a |
| Accessibility snapshot | internal app/renderer type | not serialized to scripts | not exposed | not exposed | n/a |

## Open Questions

- Which platform accessibility backend should be linked first. AccessKit is the likely candidate because the engine already uses `winit`, but the implementation task should confirm version compatibility before adding the dependency.
- Whether `focusVisual` should be allowed on passive id-bearing widgets only as authored overrides, or only on widgets that can receive focus through a focus group. The initial implementation should accept it on all focusable-capable widget descriptors and apply it only when the node is actually focusable.
- Whether the default focus visual should remain an outline for all widgets, or use text color for text-bearing buttons once `text.default` and `focus.text` exist. The conservative default is outline for backward visibility.
