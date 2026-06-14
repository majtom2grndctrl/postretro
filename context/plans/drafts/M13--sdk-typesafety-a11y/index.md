# M13 Goal G2 — Reactive UI (Selection + Visibility) + A11y / Type-Safety Foundation

> Wave plan 2 of 2 (sibling: **SE**, `drafts/M13--screen-space-effects/`). Both
> ship in one /orchestrate; independent except the seams under Sequencing.
> Downstream convergence: **BIS** authors pause/dialog/death/settings/HUD against
> this contract — landing G2 before BIS means those screens are authored once
> against the final reactive + a11y + type surface, never retrofitted. Grounding:
> `research.md`, `ui-layer.md` §15–§16, §19–§20, `lib/ui.md`. Prereqs: G1 + F
> shipped (`done/M13--*`).

## Goal

Ship the **reactive-UI primitives** that make selection-driven widgets (tabs,
segmented controls, radio groups, toggles) buildable — a **selection-predicate
bind** over `localState` and a **conditional-visibility** field — *plus* the
**a11y metadata + SDK type-safety contract**. The two are one spec because the
a11y state (`selected`/`checked`) must be **derived from the same predicate** that
drives the visual highlight and the content swap — computed once, fed to all
three, correct by construction. A static `selected: true` flag would duplicate
runtime state and desync the instant the player clicks; this spec refuses that
footgun. Two layers ship together: **(1) reactive** selection + visibility
(consumed now by highlight, content-swap, focus, and a11y-state-by-construction)
and **(2) static a11y metadata** (name/role/alt/modal/announce — shape nothing
reads yet, the durable contract BIS authors against).

## Scope

### In scope — reactive layer

- **Selection-predicate bind.** A `Predicate` = an existing bind source
  (`{ local }` cell or `{ slot }`) + optional `equals` comparand → resolves to
  **0/1** at the app/render stage on the existing cell/slot resolution path (the
  `equals` compare is the only new logic). A boolean cell without `equals`
  resolves directly; a selection cell with `equals` compares its active key. The
  0/1 result is usable as any numeric value bind, so an existing `styleRanges`
  drives the highlight — **no new visual primitive**. Idiomatic over `localState`
  (presentation state), per `lib/ui.md`; `{ slot }` works too.
- **`selected` / `checked` as predicate-bound state.** Both accept a `Predicate`,
  never a static bool. The resolved 0/1 is the a11y state *and* (via the same
  predicate) the highlight — identical by construction, no desync.
- **Conditional visibility — `visibleWhen: Predicate`** on any widget. False
  excludes the node + its subtree from layout (no taffy node), draw (zero
  rects/glyphs), **and the focus-rect list** (hidden focusables drop out; the F
  focus-reconcile already handles the structural change). It does **not** tear
  down `localState` scopes — cells declared above the toggle persist, so toggling
  visibility preserves state (declare the scope above the `Switch`; that is the
  idiom). `Switch(cell, { key: subtree })` is **pure SDK sugar** expanding to
  children each carrying `visibleWhen` for their key.
- **`disabled` honoring** (static, the one state with teeth): skipped by focus
  navigation + initial-focus, activation blocked. Flows into the `FocusRect`
  built in `render/ui/tree.rs`, consumed in `input/ui_focus.rs`.

### In scope — static a11y metadata (consume-deferred)

- **Accessible name.** `label` / `labelledBy` on interactive widgets, **exactly
  one** required (factory throw + bridge named-error); relaxes today's required
  `label` to one-of. `labelledBy`/`accessibleName` target an *authored* node id.
- **Role.** Optional `role` override on any widget; defaults to the kind's
  implicit role (Button→button, Slider→slider, Bar→progressbar, Image→image,
  containers→group, Text→none). Closed `Role` enum incl. `tab`/`tablist`/
  `checkbox`/`radio`/`listitem` — now genuinely meaningful with the selection
  bind, not hand-waves. A `role` override does **not** introduce a name
  requirement (name-precondition keys off interactive kinds + Image only).
- **Image alt / decorative.** `Image` requires `label` **xor** `decorative: true`.
- **Modal name + role.** `AnchoredTree` carries `accessibleName` + `role`
  (alongside `capture_mode`/`initial_focus`/`text_entry_target`).
- **`Announce` node.** Net-new non-visual `Widget::Announce { text, priority:
  polite|assertive }`; layout/draw skip it; bridge reads it.

### In scope — SDK type-safety

- Per-kind prop narrowing (cross-kind prop leakage + unnamed interactive widget
  are type errors); the predicate helpers (`LocalStateHandle.is(v)`,
  `StoreHandle.is(v)`); `@ts-expect-error` fixtures + typedef snapshots (the G1a
  no-`tsc`-CI pattern); typedef regen; TS/Luau parity; `docs/scripting-reference.md`.

### Out of scope

- **Cross-subtree sharing (Context API).** `localState` resolves to the *nearest
  enclosing declaration* — visible to a declaring container's whole descendant
  subtree, which covers a tab group (strip + content under one container). Sharing
  a cell across *disjoint* branches (a strip in a header, content in a far panel)
  needs a Context-style provider — a later spec.
- **`expanded` / disclosure.** Pairs with show/hide *animation* and accordion
  semantics beyond a binary `visibleWhen`; deferred.
- **Screen-reader / AT consumption** of the *static* metadata (name/role/alt/modal/
  Announce) — deferred (§19–§20). The *reactive* `selected`/`checked`/`visibleWhen`
  ARE consumed now (highlight, content-swap, focus).
- **ARIA breadth** beyond the closed role set; **template-literal nav intents**
  (TS shipped, Luau impossible — doc only); **JSX-via-SWC**; **localization
  runtime** (`LocalizedText` is the seam).

## Acceptance criteria

- [ ] A `Predicate` (`{ local | slot, equals? }`) resolves to `0.0`/`1.0` on the
  bind path: a boolean cell without `equals` resolves its value; a cell/slot with
  `equals` resolves `1.0` iff equal. The 0/1 result is a valid numeric value bind
  (an existing `styleRanges` consuming it highlights correctly).
- [ ] `selected` / `checked` accept a `Predicate`; their resolved value equals the
  same predicate's value used for the highlight (a test asserts one resolution,
  not two) — no static-flag form exists.
- [ ] `visibleWhen: false` excludes a node + subtree from layout (zero taffy
  nodes), draw (zero rects/glyphs), and the focus-rect list (its focusables are
  unreachable + not selected as initial focus); `visibleWhen: true` restores all
  three. Toggling visibility does NOT reset `localState` cells declared above the
  toggle (a test round-trips a value across a hide/show).
- [ ] `Switch(cell, map)` expands to the same wire as hand-written `visibleWhen`
  children (byte-identical), and a `Switch` over a 3-key cell shows exactly the
  matching child.
- [ ] `disabled` widgets are skipped by focus navigation + initial-focus and
  cannot be activated (`FocusRect.disabled` in `render/ui/tree.rs`, honored in
  `input/ui_focus.rs`).
- [ ] `Button`/`Slider` accept `labelledBy`; `label` optional at type level;
  neither/both throws a field-named `Error`, exactly one succeeds; the bridge
  surfaces a named load-time error (no panic) for the zero/both raw-wire case.
- [ ] `Image` requires `label` xor `decorative: true` — neither/both is a factory
  throw + named bridge error.
- [ ] A widget accepts an optional `role` from the closed set; `role` round-trips,
  absent by default; implicit-role resolution is a pure `implicit_role(kind) ->
  Role` helper with a unit test; a `role` override does not force a name.
- [ ] `AnchoredTree` carries optional `accessibleName` + `role` (alongside
  `capture_mode`/`initial_focus`/`text_entry_target`); a tree without the new
  fields deserializes byte-identically to its pre-G2 wire form.
- [ ] `Announce({}, "…")` (polite, priority omitted on the wire) round-trips to
  `Widget::Announce` byte-identically; `{ priority: "assertive" }` round-trips with
  the field present; layout emits zero rects + draw zero glyphs; a garbled
  `Announce` is a named load-time error, not a panic.
- [ ] Every pre-G2 tree deserializes byte-identically — all new descriptor fields
  skip-serialized when absent (`Predicate`/`Option` standard skip; `disabled` bool
  `skip_serializing_if = "std::ops::Not::not"`).
- [ ] Emitted typedefs narrow props per kind (a `Button` with Text-only `content`
  is a type error; `Bar` needs no name); `LocalStateHandle.is(v)`/`StoreHandle.is(v)`
  are typed to the cell/slot value type — `@ts-expect-error` fixtures + a typedef
  snapshot; `gen-script-types` reports no drift; TS/Luau parity-checked.
- [ ] A working **tabs demo** in the dev UI: a `localState` cell + `role:"tablist"`
  strip whose buttons are `selected`-highlighted, with `Switch` swapping the
  content panel — manual verification.
- [ ] `docs/scripting-reference.md` covers the predicate bind, `selected`/`checked`,
  `visibleWhen`/`Switch`, name/role/disabled, image alt/decorative, modal naming,
  and `Announce`.

## Tasks

### Task 0: split `descriptor.rs` (behavior-preserving)
`render/ui/descriptor.rs` is 1574 lines; G2 adds many fields + a variant + enums.
Split first along existing seams — widget structs + the `AnchoredTree` envelope
into a `descriptor/` submodule, the `Widget` enum + serde wire contract unchanged
— so the round-trip tests in the `#[cfg(test)] mod tests` block
(`descriptor.rs:742-~1574`) stay byte-identical. No behavior change. `Depends on`
nothing.

### Task 1: descriptor vocabulary + bridge
In the split modules add: the `Predicate` type (`{ slot|local, equals? }`);
`selected`/`checked: Option<Predicate>` + `visible_when: Option<Predicate>` on the
relevant widgets; `label`/`labelled_by`, `role: Option<Role>` + closed `Role`
enum, `disabled: bool`, `Image` `label` xor `decorative: bool`, `AnchoredTree`
`accessible_name`+`role`, and `Widget::Announce(AnnounceWidget { text, priority })`
(non-visual). Extend the bridge (`data_descriptors.rs`) to read all of them +
enforce preconditions (exactly-one name; image name-xor-decorative; Announce
shape; well-formed predicate) with named load-time errors, no panic. New fields
skip-serialized when absent. `Depends on` Task 0.

### Task 2: predicate resolution + conditional visibility (reactive core)
The novel task. On the app/render value-resolution path (`render/ui/`): resolve a
`Predicate` to 0/1 (reuse cell/slot resolution; add the `equals` compare); make
the 0/1 a valid numeric value-bind result (so `styleRanges` highlights); resolve
`selected`/`checked` to the a11y value from the same predicate. Implement
`visible_when`: a false predicate excludes the node + subtree from the taffy
layout, the draw walk, and the `FocusRect` build (`render/ui/tree.rs`), without
tearing down `localState` scopes declared above it. Extend the `FocusRect` build
to also carry the `disabled` bit (consumed by Task 3). `Depends on` Task 1.
**Sizing note:** this spans resolution + visibility + focus-rect build; the
implementability pass should confirm it is one task or split it (e.g. predicate
resolution vs visibility-exclusion).

### Task 3: `disabled` focus honoring
`input/ui_focus.rs`: skip `FocusRect.disabled` nodes in `move_focus`/`linear_step`/
`spatial_step`/`initial_focus_id`, and block activation in
`fire_focused_button_activation` (reading `NodeInteraction::Button`). `Depends on`
Task 2 (the `FocusRect.disabled` field). Disjoint file from Task 4 — concurrent.

### Task 4: SDK factories + types + typedefs
Mirror Tasks 1–2 in `sdk/lib/ui/{widgets,tree,state}.{ts,luau}` + barrel: the
name XOR unions, `role`/`disabled` props, `selected`/`checked`/`visibleWhen`
predicate props, `Image` label-xor-decorative, the `Tree` envelope, the `Announce`
factory; add `LocalStateHandle.is(v)` + `StoreHandle.is(v)` predicate helpers and
the `Switch(cell, map)` sugar (expands to `visibleWhen` children). Update the
`typedef.rs` widget SDK-block + regenerate. `Depends on` Task 1 (field shapes).
**Wave seam (SE):** coordinate `typedef.rs` (widget block) + `sdk/lib/index.ts`
barrel with SE (reaction block — different sections); regenerate once both land.

### Task 5: narrowing fixtures + docs + demo
Typedef snapshot tests (per-kind narrowing; unnamed-interactive type error;
typed `.is()`); `@ts-expect-error` fixtures; the nav-intent template-literal doc;
the working tabs demo (cell + `tablist` + `selected` highlight + `Switch` swap);
regenerate typedefs; update `docs/scripting-reference.md`. `Depends on` Tasks 2–4.

## Sequencing

**Phase 0:** Task 0 (split). **Phase 1:** Task 1 (descriptor backbone).
**Phase 2:** Task 2 (reactive core — resolution + visibility + focus-rect build).
**Phase 3 (concurrent):** Task 3 (focus honoring, `ui_focus.rs`), Task 4
(SDK/types, `sdk/`) — disjoint files. **Phase 4:** Task 5 (fixtures + docs +
demo). **Wave seams (SE):** (a) `typedef.rs` widget-vs-reaction blocks + the
`sdk/lib/index.ts` barrel; (b) G2 Task 2 edits the `render/ui/` value-resolution /
snapshot path that SE Task 4 *reads* effect slots from — coordinate the
`render/ui` edits (different concerns, same area). G2 owns all
`descriptor.rs`/tree-bridge edits + the `FocusRect` build in `render/ui/tree.rs`.

## Rough sketch

- `Predicate` (wire): `{ "local": "tab", "equals": "loadout" }` or `{ "slot":
  "x", "equals": 3 }`; resolves to `0.0`/`1.0`. SDK: `sel.cells.tab.is("loadout")`
  → that object; `selected`/`checked`/`visibleWhen` accept it.
- Tabs end-to-end (buildable under this spec):
  ```ts
  const sel = ui.createLocalState({ tab: "loadout" });
  VStack({ localState: sel.scope }, [
    HStack({ role: "tablist" }, [
      Button({ id: "t-loadout", label: "Loadout", role: "tab",
               selected: sel.cells.tab.is("loadout"), onPress: sel.cells.tab.set("loadout") }),
      Button({ id: "t-stats", label: "Stats", role: "tab",
               selected: sel.cells.tab.is("stats"),   onPress: sel.cells.tab.set("stats") }),
    ]),
    Switch(sel.cells.tab, { loadout: LoadoutPanel(), stats: StatsPanel() }),
  ]);
  ```
- Name XOR (TS): `ButtonBase & ({ label: LocalizedText; labelledBy?: never } | {
  labelledBy: NodeId; label?: never })`; same for `Image` (`label` vs
  `decorative: true`). `Announce(props, text)`: `text` positional, `priority` in
  props, Luau twin matches. Anchors (pre-split line nums; resolve by symbol
  post-Task-0): Widget enum `:255`, `ImageWidget :522`, `AnchoredTree :193`;
  bridge `data_descriptors.rs` `anchored_tree_from_{js,lua}_value`; focus engine
  `ui_focus.rs` `move_focus :443`/`initial_focus_id :619`; `FocusRect`/`FocusRectList`
  in `render/ui/tree.rs`.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| Predicate | `Predicate { source: BindSource, equals: Option<Value> }` | `{ "local"\|"slot", "equals"? }` | `{ local\|slot, equals? }` | same |
| selected/checked | `Option<Predicate>` | `"selected"`/`"checked"` (omit absent) | `selected?: Predicate` | same |
| visibleWhen | `visible_when: Option<Predicate>` | `"visibleWhen"` (omit absent) | `visibleWhen?: Predicate` | same |
| `.is(v)` helper | n/a (SDK) | n/a | `LocalStateHandle.is(v): Predicate` | `:is(v)` |
| Switch | n/a (SDK sugar → `visibleWhen`) | children w/ `visibleWhen` | `Switch(cell, map)` | `Ui.Switch` |
| disabled | `bool` (`skip_serializing_if` Not) | `"disabled"` (omit false) | `disabled?: boolean` | `disabled?` |
| labelledBy | `labelled_by: Option<String>` | `"labelledBy"` | `labelledBy: NodeId` | same |
| label (relaxed) | `label: Option<String>` | `"label"` (omit absent) | `label?: LocalizedText` | `label?` |
| role | `role: Option<Role>` | `"role"` | `role?: Role` | `role?` |
| image decorative | `decorative: bool` (XOR w/ `label`) | `"decorative"` | XOR: `{ decorative: true; label?: never }` | XOR in factory |
| envelope name/role | `accessible_name`/`role` on `AnchoredTree` | `"accessibleName"`/`"role"` | `accessibleName?`/`role?` | same |
| Announce node | `Widget::Announce(AnnounceWidget)` | `{ "kind":"announce","text","priority" }` | `Announce(props, text)` | `Ui.Announce(props, text)` |
| priority | `Priority::{Polite,Assertive}` (default Polite, `skip_serializing_if`) | `"polite"`/`"assertive"` — omit when polite | `"polite"\|"assertive"` | same |

## Open questions

- **`Predicate.equals` value type.** Reuses the `CellInit` set (number / bool /
  string / rgba). Recommend restricting `equals` to number/bool/string at v1
  (rgba equality is unusual for selection); decide at implementation.
- **Snapshot the resolved a11y `selected`/`checked` now, or with the AT consumer?**
  The resolution already runs for the visual; writing the 0/1 into a `UiReadSnapshot`
  a11y field now keeps the contract complete at near-zero cost. Recommend now;
  confirm in the implementability pass.
- **Size.** G2 is now ~6 tasks across descriptor, app-side resolution, focus, and
  SDK — the wave's long pole, no longer symmetric with SE (fine: parallel, distinct
  domains). Task 2 is the novelty and the implementability focus.
- **`disabled` styling.** Behavior only here; the visual dim is a theme/styleRanges
  concern — note for BIS. Dynamic (bound) `disabled` is a later enhancement.
