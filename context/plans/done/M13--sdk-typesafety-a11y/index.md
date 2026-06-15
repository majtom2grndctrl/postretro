# M13 Goal G2 — Reactive UI (Selection + Visibility) + A11y / Type-Safety Foundation

> Wave plan 2 of 2 (sibling: **SE**, `ready/M13--screen-space-effects/`). Both
> ship in one /orchestrate; independent except the seams under Sequencing.
> Downstream convergence: **BIS** authors pause/dialog/death/settings/HUD against
> this contract — landing G2 before BIS means those screens are authored once
> against the final reactive + a11y + type surface, never retrofitted. Grounding:
> `research.md`, `ui-layer.md` §15–§16, §19–§20, `lib/ui.md` §3 (retained tree /
> rebuild model — load-bearing for visibility). Prereqs: G1 + F shipped (`done/M13--*`).

## Goal

Ship the **reactive-UI primitives** that make selection-driven widgets (tabs,
segmented controls, radio groups, toggles) buildable — a **selection-predicate
bind** over `localState` and a **conditional-visibility** field — *plus* the
**a11y metadata + SDK type-safety contract**. One spec because the a11y state
(`selected`/`checked`) and the visual highlight are both driven by the **same
author predicate**: a `Predicate` is a bind source, so the author wires the
highlight with `styleRanges` and tags the widget's `selected`/`checked` from one
expression — each resolves deterministically, so they agree by construction. The
engine draws no `selected`/`checked` styling itself (the highlight is author-wired,
like the deferred `disabled` dim). A static `selected: true` would duplicate
runtime state and desync; this spec refuses it. Two layers:
**(1) reactive** selection + visibility (consumed now by highlight, content-swap,
focus, a11y-by-construction); **(2) static a11y metadata** (name/role/alt/modal/
announce — shape nothing reads yet, the durable contract BIS authors against).

## Scope

### In scope — reactive layer

- **Selection-predicate bind.** A `Predicate` = a bind source (`{ local }` cell or
  `{ slot }`) + optional `equals` comparand → resolves to **`f32` 0.0/1.0** via a new
  `resolve_predicate(source, equals, scope, slots, cells) -> f32` helper riding the
  existing `lookup_bound` (`render/ui/tree.rs:43`). Without `equals` the source must
  be a `Boolean` (toggle cell → truthiness); with `equals` the resolved `SlotValue`
  is compared. A `Predicate` is a valid **`bind` source for `styleRanges`-capable
  widgets** (Text / Panel / Bar / Button), so the author drives the highlight with
  the existing `styleRanges` (a `max:1` band) — no new visual primitive. The same
  `Predicate` is the type of `selected`/`checked`/`visibleWhen`. Each consumer
  resolves independently (cheap, deterministic) — no shared per-node storage.
  Idiomatic over `localState`; `{ slot }` works too.
- **`selected` / `checked` are a11y state** (`Option<Predicate>`; **no static-bool
  form is ever defined**). They resolve in the focus-rect build and are carried on
  **`FocusRect`** — the renderer→app readback (`export_ui_focus_rects`), where a
  test reads them today and a screen reader will later. The engine draws **no**
  highlight from them; the author wires the visual with a `Predicate`-bound
  `styleRanges` referencing the same predicate, so a11y and highlight agree by
  construction.
- **Conditional visibility — `visibleWhen: Predicate`** on any widget, via taffy
  **`Display::None`** (`Display` already imported `tree.rs:11`). A false predicate
  sets the node `Display::None` (it **stays in the taffy tree** — preserving the
  descriptor↔taffy 1:1 lockstep that `export_focus_rects` depends on,
  `tree.rs:908`); the draw walk (`collect_node :1041`) and focus walk
  (`collect_focus_node :950`) **skip `Display::None`** subtrees. Applied in the
  layout/draw/focus walks **only — never in the descriptor walk `reconcile()` uses**
  (`scripting/systems/presentation_cells.rs:74` walks `tree.root` descriptors), so
  `localState` scopes are **not** torn down (cells persist across hide/show). A
  **change in a visibility predicate's resolved value** marks layout dirty +
  re-exports the cached `FocusRectList` (a targeted invalidation — visibility flips
  are rare, authored-frequency events per `lib/ui.md` §3). `Switch(cell, map)` is
  pure SDK sugar (Task 4) expanding to the map's subtrees each with an injected
  `visibleWhen: cell.is(key)`.
- **`disabled` honoring** (static, the one state with teeth): a `disabled` bit on
  `FocusRect`/`NodeInteraction` (`tree.rs:2126/2147`), populated in the focus-rect
  build. Navigation-skip in `input/ui_focus.rs` (`move_focus :443`, `linear_step`,
  `spatial_step`, `initial_focus_id :619`) and the pointer/hover paths
  (`hit_test_topmost`, `tick`); **activation-block in `main.rs::fire_focused_button_activation`**
  (`main.rs:2923`, reads `NodeInteraction::Button`) — App-side, not `ui_focus.rs`.

### In scope — static a11y metadata (consume-deferred)

- **Accessible name.** `label` / `labelledBy` on interactive widgets, **exactly
  one** required (factory throw + bridge named-error). This **is** a real
  migration: `ButtonWidget.label`/`SliderWidget.label` are currently required
  `String` (`descriptor.rs:631/663`) — relax to one-of. `labelledBy`/`accessibleName`
  target an *authored* node id.
- **Role.** Optional `role` override; defaults to the kind's implicit role
  (Button→button, Slider→slider, Bar→progressbar, Image→image, containers→group,
  Text→none) via a pure `implicit_role(kind) -> Role`. Closed `Role` enum incl.
  `tab`/`tablist`/`checkbox`/`radio`/`listitem` — now meaningful with the selection
  bind. A `role` override does **not** introduce a name requirement
  (name-precondition keys off interactive kinds + Image only).
- **Image alt / decorative.** `Image` (no name field today, `descriptor.rs:522`)
  requires `label` **xor** `decorative: true`.
- **Modal name + role.** `AnchoredTree` (`descriptor.rs:193`) carries
  `accessible_name` + `role`, following the **`initial_focus`/`text_entry_target`**
  pattern: `Option` + `#[serde(default, skip_serializing_if = "Option::is_none")]`
  (NOT `capture_mode`'s custom predicate).
- **`Announce` node.** Net-new non-visual `Widget::Announce(AnnounceWidget { text,
  priority })`; `priority: Priority` with `#[serde(default, skip_serializing_if =
  "Priority::is_polite")]` (the `CaptureMode::is_passthrough` pattern) so a polite
  Announce omits the key. Layout/draw skip it; bridge reads it.

### In scope — SDK type-safety

- Per-kind prop narrowing; the `.is(v)` predicate helpers; `@ts-expect-error`
  fixtures + typedef snapshots (the G1a no-`tsc`-CI pattern); typedef regen; TS/Luau
  parity; `docs/scripting-reference.md`.

### Out of scope

- **Cross-subtree sharing (Context API).** `localState` resolves to the nearest
  declaring ancestor — visible to a declaring container's whole descendant subtree
  (`build_node` threads `scope`, `tree.rs:1734`), which covers a tab group. Sharing
  across *disjoint* branches needs a provider — a later spec.
- **`expanded` / disclosure** (pairs with show/hide animation > binary
  `visibleWhen`); deferred — no field added.
- **Screen-reader / AT consumption** of the *static* metadata (deferred §19–§20);
  the *reactive* `selected`/`checked`/`visibleWhen` are consumed now.
- **Engine-rendered `selected`/`checked` styling.** The highlight is author-wired
  via a `Predicate`-bound `styleRanges`; an engine/theme selected-state visual is
  deferred — consistent with the deferred `disabled` dim.
- **ARIA breadth**; **template-literal nav intents** (doc only); **JSX-via-SWC**;
  **localization runtime** (`LocalizedText` is the seam).

## Acceptance criteria

- [x] `resolve_predicate` resolves a `Predicate` to `0.0`/`1.0`: a `Boolean`
  source without `equals` → its truthiness; a source with `equals` → `1.0` iff the
  resolved `SlotValue` equals the comparand. `equals` v1 admits **number / bool /
  string** only (exact compare; `String`/`Enum` match by name; a type mismatch →
  `0.0`); rgba/array comparands are a load-time error. A `Predicate` used as a
  widget `bind` resolves to a `Number` the existing `styleRanges` extractor
  (`style_value`) consumes — the author-wired highlight, no new visual primitive.
- [x] `selected`/`checked` (`Option<Predicate>`) resolve in the focus-rect build
  and are carried on `FocusRect`/`FocusRectList`; a test reads the exported
  `FocusRectList` and asserts the value matches the predicate. No static-bool form
  compiles. (The engine draws no highlight from them — see Out of scope.)
- [x] `visibleWhen: false` sets the node `Display::None` — excluded from layout
  size, the draw walk (zero rects/glyphs), and the focus-rect list (its focusables
  are unreachable + not chosen as initial focus); `true` restores all three. A
  change in the predicate's resolved value re-exports the `FocusRectList` and marks
  layout dirty. Toggling visibility does **not** reset `localState` cells declared
  above the toggle (a test round-trips a cell value across a hide/show) — because
  `reconcile` walks the descriptor, not the visible tree.
- [x] `Switch(cell, map)` expands to the map's subtrees each with `visibleWhen:
  cell.is(key)` injected, in **lexicographically-sorted key order** (pinned
  identically in TS + Luau, since Luau table iteration order is undefined) — assert
  byte-identical wire; a `Switch` over a 3-key cell shows exactly the matching child.
- [x] `disabled` widgets are skipped by focus navigation, initial-focus, and the
  pointer/hover paths, and cannot be activated (`FocusRect.disabled` populated in
  the focus-rect build; honored in `ui_focus.rs` nav + `main.rs::fire_focused_button_activation`).
- [x] `Button`/`Slider` accept `labelledBy`; `label` optional at type level;
  neither/both throws a field-named `Error`, exactly one succeeds; the bridge
  surfaces a named load-time error (no panic) for the zero/both raw-wire case.
- [x] `Image` requires `label` xor `decorative: true` — neither/both is a factory
  throw + named bridge error.
- [x] A widget accepts an optional `role`; `role` round-trips, absent by default;
  `implicit_role(kind) -> Role` has a unit test; a `role` override does not force a
  name.
- [x] `AnchoredTree` carries optional `accessibleName` + `role` (`Option` +
  `Option::is_none` skip, alongside `capture_mode`/`initial_focus`/`text_entry_target`);
  a tree without the new fields deserializes byte-identically to its pre-G2 wire.
- [x] `Announce({}, "…")` (polite, omitted via `is_polite` skip) round-trips
  byte-identically; `{ priority: "assertive" }` round-trips with the field present;
  layout zero rects + draw zero glyphs; a garbled `Announce` is a named load-time
  error, not a panic.
- [x] Every pre-G2 tree deserializes byte-identically — new fields skip-serialized
  when absent (`Option` standard skip; `disabled` bool the existing `is_false`
  helper, `descriptor.rs:570`).
- [x] Each new descriptor field is read by the **hand-written**
  `data_descriptors.rs` bridge in **both** the JS and Lua per-kind converters (a
  serde-only field would pass `descriptor.rs` round-trip tests yet silently drop on
  the live authoring path — a test authors each field through the bridge and
  asserts arrival).
- [x] Emitted typedefs narrow props per kind (a `Button` with Text-only `content`
  is a type error; `Bar` needs no name); `LocalStateHandle.is(v)`/`StoreHandle.is(v)`
  are typed to the cell/slot value type — `@ts-expect-error` fixtures + a typedef
  snapshot; `gen-script-types` reports no drift; TS/Luau parity.
- [x] A working **tabs demo** in the dev UI: a `localState` cell + `role:"tablist"`
  strip where each tab `Button` carries a `Predicate` `bind` + `styleRanges` (the
  highlight) and `selected` (a11y), with `Switch` swapping the content panel —
  manual verification.
- [x] `docs/scripting-reference.md` covers the predicate bind, `selected`/`checked`,
  `visibleWhen`/`Switch`, name/role/disabled, image alt/decorative, modal naming,
  and `Announce`.

## Tasks

### Task 0: split `descriptor.rs` (behavior-preserving)
`render/ui/descriptor.rs` is 1574 lines; G2 adds many fields + a variant + enums.
Split widget structs + the `AnchoredTree` envelope into a `descriptor/` submodule,
the `Widget` enum + serde wire contract unchanged, so the round-trip tests in the
`#[cfg(test)] mod tests` block (`descriptor.rs:742-~1574`) stay byte-identical.
`Depends on` nothing.

### Task 1: descriptor vocabulary + bridge (both languages)
In the split modules add: the `Predicate` type (`{ source: BindSource, equals:
Option<Value> }`, also accepted as a widget `bind` for `styleRanges`);
`selected`/`checked: Option<Predicate>` on `Button`, `visible_when:
Option<Predicate>` on every `Widget` variant; optional `bind` + `style_ranges` on
`Button` (passed through `build_button` to its internal `Text` `NodeContext`,
reusing `style_text_value`) so a tab button self-highlights from a `Predicate` bind;
`label`/`labelled_by`, `role:
Option<Role>` + closed `Role` enum + `implicit_role`, `disabled: bool`
(`skip_serializing_if = is_false`), `Image` `label` xor `decorative: bool`,
`AnchoredTree` `accessible_name`+`role` (`Option::is_none` skip), and
`Widget::Announce(AnnounceWidget { text, priority })` with `Priority::is_polite`
skip. **Extend the hand-written bridge (`data_descriptors.rs`) — a reader for each
new field in BOTH the JS (`widget_from_js :3357` + per-kind) and Lua
(`:4063`+) converters** (a serde-only field drops on the live path); enforce
preconditions (exactly-one name; image name-xor-decorative; Announce shape;
well-formed predicate; `equals` type ∈ {number,bool,string}) with named load-time
errors, no panic. New fields skip-serialized when absent. `Depends on` Task 0.

### Task 2a: predicate resolution + a11y state + `FocusRect.disabled`
Add `resolve_predicate(source, equals, scope, slots, cells) -> f32` riding
`lookup_bound` (`tree.rs:43`) — exact compare; `String`/`Enum` match by name; a
type mismatch (or a no-`equals` predicate over a non-`Boolean` source) → `0.0`; a
no-`equals` `Boolean` source → its truthiness. Wire it into the `styleRanges` value
path so a `Predicate`-carrying `bind` resolves to a `Number` the extractor
(`style_value`/`style_text_value :2456`) consumes — the author-wired highlight, no
new visual primitive. Resolve `selected`/`checked` in the focus-rect build
(`collect_focus_node :950` / `export_focus_rects :908`) and carry the 0/1 on
`FocusRect`/`NodeInteraction` (`tree.rs:2126/2147`) — the **renderer→app readback**
(`export_ui_focus_rects`, `main.rs:2297`), NOT `UiReadSnapshot` (which is the
app→renderer input). Add the `disabled` bit on the same `FocusRect` build. No
shared per-node store — each consumer resolves independently (cheap, deterministic).
`Depends on` Task 1. (This is the unblocking dependency for Task 3.)

### Task 2b: conditional visibility (`visibleWhen` via `Display::None`)
A false `visible_when` predicate (resolved in Task 2a) sets the taffy node
`Display::None` (node retained → lockstep preserved); the draw walk (`collect_node
:1041`) and focus walk (`collect_focus_node :950`) skip `Display::None` subtrees;
**do not** apply visibility in the `reconcile` descriptor walk
(`presentation_cells.rs:74`) so `localState` scopes survive. A change in a
visibility predicate's resolved value marks layout dirty + re-exports the cached
`FocusRectList` (cross-ref `lib/ui.md` §3 rebuild model). Detect the change via a
per-node prev-resolved-value compare in `resolve_bindings`; on change toggle the
node's taffy `Display` and `mark_dirty` so the layout gate recomputes and the
per-frame `export_ui_focus_rects` reflects it. `Depends on` Task 2a
(the resolved per-node field). Concurrent with Tasks 3, 4.

### Task 3: `disabled` focus + activation honoring
`input/ui_focus.rs`: skip `FocusRect.disabled` nodes in `move_focus`/`linear_step`/
`spatial_step`/`initial_focus_id` and the pointer (`hit_test_topmost`) + hover
(`tick`) paths. Linear nav advances **past consecutive** disabled members (loop
until a focusable one or exhaustion, respecting wrap/clamp), not a single ±1 step;
spatial nav excludes disabled from candidates. **`main.rs`:** block activation in `fire_focused_button_activation`
(`:2923`, reads `NodeInteraction::Button`). Behavior only — the visual dim (a
theme/styleRanges treatment) is out of scope (BIS); dynamic (bound) `disabled` is a
later enhancement. `Depends on` Task 2a
(`FocusRect.disabled`). **Note:** touches `main.rs` (shared) — coordinate with any
concurrent `main.rs` edits.

### Task 4: SDK factories + types + typedefs
Mirror Tasks 1–2 in `sdk/lib/ui/{widgets,tree,state}.{ts,luau}` + barrel: the name
XOR unions, `role`/`disabled` props, `selected`/`checked`/`visibleWhen` predicate
props, `Image` label-xor-decorative, the `Tree` envelope, the `Announce` factory;
add `LocalStateHandle.is(v)` + `StoreHandle.is(v)` (→ `{ local|slot, equals }`) and
`Switch(cell, map)` sugar (reads the handle's `{ local }` name, injects
`visibleWhen: cell.is(key)` per child in **lexicographically-sorted key order** —
TS/Luau identical, since Luau table iteration order is undefined).
Update the `typedef.rs` widget SDK-block + regenerate. `Depends on` Task 1.
**Wave seam (SE):** coordinate `typedef.rs` (widget block) + `sdk/lib/index.ts`
barrel with SE (reaction block — different sections).

### Task 5: narrowing fixtures + docs + demo
Typedef snapshot tests (per-kind narrowing; unnamed-interactive type error; typed
`.is()`); `@ts-expect-error` fixtures; the nav-intent doc; the working tabs demo
(cell + `tablist`; each tab `Button` with a `Predicate` `bind`+`styleRanges`
highlight and `selected` a11y; `Switch` swap); regenerate typedefs;
update `docs/scripting-reference.md`. `Depends on` Tasks 2a, 2b, 3, 4.

## Sequencing

**Phase 0:** Task 0 (split). **Phase 1:** Task 1 (descriptor + bridge backbone).
**Phase 2:** Task 2a (resolution + a11y state + `FocusRect.disabled`).
**Phase 3 (concurrent):** Task 2b (visibility), Task 3 (focus/activation honoring),
Task 4 (SDK/types) — 2b in `render/ui`, 3 in `ui_focus.rs`/`main.rs`, 4 in `sdk/`.
**Phase 4:** Task 5 (fixtures + docs + demo). **Wave seams (SE):** (a) `typedef.rs`
widget-vs-reaction blocks + `sdk/lib/index.ts` barrel; (b) Task 2a edits the
`render/ui` value-resolution / `UiReadSnapshot` path that SE Task 4 *reads* effect
slots from — coordinate (distinct concerns, same area). G2 owns all
`descriptor.rs`/tree-bridge edits + the focus-rect build in `render/ui/tree.rs`.

## Rough sketch

- `Predicate` wire: `{ "local": "tab", "equals": "loadout" }` or `{ "slot": "x",
  "equals": 3 }` → `resolve_predicate` → 0.0/1.0. SDK: `sel.cells.tab.is("loadout")`.
- Tabs end-to-end (buildable under this spec):
  ```ts
  const sel = ui.createLocalState({ tab: "loadout" });
  const tab = (key, label) => Button({           // localState lives on vstack/hstack, NOT Grid
    id: `t-${key}`, label, role: "tab",
    bind: sel.cells.tab.is(key),                 // highlight: predicate (0/1) -> styleRanges
    styleRanges: { max: 1, entries: [{ upTo: 0, color: "tabDim" }, { color: "tabHot" }] },
    selected: sel.cells.tab.is(key),             // a11y, same predicate
    onPress: sel.cells.tab.set(key),
  });
  VStack({ localState: sel.scope }, [
    HStack({ role: "tablist" }, [ tab("loadout", "Loadout"), tab("stats", "Stats") ]),
    Switch(sel.cells.tab, { loadout: LoadoutPanel(), stats: StatsPanel() }),
  ]);
  ```
- Name XOR (TS): `ButtonBase & ({ label: LocalizedText; labelledBy?: never } | {
  labelledBy: NodeId; label?: never })`; same for `Image` (`label` vs `decorative:
  true`). `Announce(props, text)`: `text` positional, `priority` in props.
- Source anchors: `lookup_bound :43`, `style_value :2456`, `collect_node :1041`,
  `collect_focus_node :950`, `export_focus_rects :908`, `build_node :1734`,
  `Display` import `:11`, `FocusRect :2126`/`NodeInteraction :2147`/`FocusRectList
  :2195` (all `render/ui/tree.rs`); `reconcile` `presentation_cells.rs:74`;
  `fire_focused_button_activation` `main.rs:2923`; `UiReadSnapshot` `mod.rs:311`;
  `SlotValue` `slot_table.rs:8`; `CellInit` `descriptor.rs:359`. Pre-split line
  numbers — resolve by symbol after Task 0.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| Predicate (also a `bind` source) | `Predicate { source: BindSource, equals: Option<Value> }` | `{ "local"\|"slot", "equals"? }` | `{ local\|slot, equals? }` | same |
| Button highlight | `bind`+`style_ranges` on `ButtonWidget` (pass-through to Text) | `"bind"`/`"styleRanges"` | `bind?: Predicate; styleRanges?` | same |
| selected/checked | `Option<Predicate>` (omit absent) | `"selected"`/`"checked"` | `selected?: Predicate` | same |
| visibleWhen | `visible_when: Option<Predicate>` (omit absent) | `"visibleWhen"` | `visibleWhen?: Predicate` | same |
| `.is(v)` helper | n/a (SDK) | n/a | `LocalStateHandle.is(v): Predicate` | `:is(v)` |
| Switch | n/a (SDK sugar → `visibleWhen`) | children w/ `visibleWhen` (sorted key order) | `Switch(cell, map)` | `Ui.Switch` |
| disabled | `bool` (`skip_serializing_if = is_false`) | `"disabled"` (omit false) | `disabled?: boolean` | `disabled?` |
| labelledBy / label | `labelled_by`/`label: Option<String>` | `"labelledBy"`/`"label"` | `labelledBy: NodeId` / `label?: LocalizedText` | same |
| role | `role: Option<Role>` | `"role"` | `role?: Role` | `role?` |
| image decorative | `decorative: bool` (XOR w/ `label`) | `"decorative"` | XOR: `{ decorative: true; label?: never }` | XOR in factory |
| envelope name/role | `accessible_name`/`role` (`Option::is_none` skip) | `"accessibleName"`/`"role"` | `accessibleName?`/`role?` | same |
| Announce / priority | `Widget::Announce(AnnounceWidget)`; `Priority` (`is_polite` skip) | `{ "kind":"announce","text","priority"? }` | `Announce(props, text)` | same |
