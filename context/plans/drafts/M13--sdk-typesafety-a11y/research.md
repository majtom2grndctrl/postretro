# G2 — Research Notes

Grounding for the G2 draft. Confirmed against source 2026-06-14. Ephemeral.

## Headline: most of the original G2 bullet is already shipped or impossible

The roadmap G2 bullet lists: a11y compile-precondition (`label`/`labelledBy`,
`Announce`), template-literal nav intents, discriminated unions per kind,
optional JSX-via-SWC. Against source:

| Item | Status |
|---|---|
| `label` required on interactive widgets | **Shipped** — `ButtonProps.label` / `SliderProps.label` are required `LocalizedText` (`sdk/lib/ui/widgets.ts:518,573`; Rust `label: String` `render/ui/descriptor.rs:631,663`). |
| `labelledBy` alternative | **Missing** — no `labelledBy` on any widget (TS or Rust). G2's main net-new. |
| `Announce` node | **Missing** — no variant in `Widget`, no factory. G2's other net-new. |
| Template-literal nav intents (TS) | **Shipped** by Goal F — `export type NavIntent = \`nav.${NavIntentName}\`` in `typedef.rs` TS block. Typos are already type errors. |
| Template-literal nav intents (Luau) | **Impossible** — Luau has no template-literal types; F emits a flat string union. Permanent language limit, not a gap. |
| Discriminated union per kind | **Mostly shipped** — Rust `Widget` is an internally-tagged enum (`#[serde(tag="kind")]`, `descriptor.rs:242`); TS has per-factory `Props` types. G2 verifies/tightens + adds fixtures. |
| Optional JSX-via-SWC | **No tooling** — no `@swc/core`, no JSX config, no bundler. Genuinely optional; deferrable. |
| Text-alias chokepoint (`LocalizedText`) | **Shipped** by G1a — `sdk/lib/ui/text.ts:12` `export type LocalizedText = string`. G2's i18n note builds on it; no new work. |

Net-new substance for G2: **`labelledBy` + `Announce` + verification/fixtures.**
**Owner decision (2026-06-14): build the full a11y *metadata* foundation**, not
the lean v1 — the cuts (image alt/decorative, modal name+role, role/state
vocabulary) are foundation-shaped (BIS authors against them), so completeness now
beats a second BIS retrofit. Consumption (screen reader) stays deferred; G2 ships
the complete descriptor *shape* nothing reads yet, plus `disabled` honoring (a
no-op disabled is a footgun).

## Owner decision (2026-06-14 #2): pull reactive selection + visibility into G2

Reviewing the tabs example, the owner rejected static `selected: true` — selection
is runtime state, and a static flag desyncs the moment the player clicks. The grep
confirmed the descriptor model has **no conditional-visibility / selection /
equality primitive** today (only `bind` value/color/text, monotonic `styleRanges`
bands, and `localState` cells). So a reactive tab strip is not buildable at all —
not just the a11y part. Decision: **pull the reactive primitives into G2** so
`selected`/`checked` are *derived* from the same bind that drives the visual.

Design (grounded in `sdk/lib/ui/state.ts` + `widgets.ts`):
- **`localState` already scopes to the descendant subtree** of the declaring
  container (`.get()` resolves "the nearest enclosing `localState` declaration",
  `state.ts:82`). So a tab group (strip + content under one container) shares a
  cell with **no new Context primitive**. Cross-*disjoint*-subtree sharing (a
  React-Context provider) is the genuine gap — **deferred to a later spec** (owner).
- **`Predicate`** = existing `BindSource` (`{ local }`/`{ slot }`) + optional
  `equals` → resolves **0/1** on the existing cell/slot resolution path (the
  `equals` compare is the only new logic). 0/1 is a valid numeric value bind, so
  existing `styleRanges` (`widgets.ts:286`) drives the highlight — **no new visual
  primitive**. `selected`/`checked` consume the same `Predicate` → a11y state and
  highlight identical by construction.
- **`visibleWhen: Predicate`** is the one new structural field: false excludes the
  subtree from layout/draw/focus-rect-list; does NOT tear down `localState` scopes
  (cells persist; declare the scope above the toggle). `Switch(cell, map)` = pure
  SDK sugar over `visibleWhen` (cellWrite/`{local}` infra already exists,
  `state.ts:138`). Owner chose to **include content-swap** in this wave.
- Static `selected: bool` / `checked: bool` / `expanded` from decision #1 are
  **dropped**: `selected`/`checked` are now predicate-bound; `expanded` deferred
  (pairs with disclosure animation > binary `visibleWhen`). `disabled` stays static
  with teeth.

Consequence: G2 grows to ~6 tasks and a new task-area (app-side predicate
resolution + visibility in `render/ui/`), making it the wave's long pole and adding
a second SE seam (both touch `render/ui` snapshot/resolution — distinct concerns).
Task 2 (resolution + visibility + focus-rect build) is the novelty → the
implementability pass should focus there and may split it.

## Review round 2 (draft-spec, 2026-06-14): reactive-core mechanisms pinned

The reactive expansion's draft review confirmed every primitive feasible but found
the spec hand-waved three mechanisms the source has firm invariants about. Pinned
(owner-confirmed) with grounded anchors:
- **Resolution carrier.** styleRanges extractors (`style_value`, `tree.rs:2456`)
  accept only `SlotValue::Number`, and `export_focus_rects` (`tree.rs:908`) sees no
  resolved binds — so "one resolution, three consumers" needs a
  `resolve_predicate(...) -> f32` (rides `lookup_bound` `tree.rs:43`) resolved
  **once into a per-node field** during `build_draw_data_retained` (`tree.rs:674`),
  read by the draw walk + focus walk + a11y snapshot export.
- **Visibility = `Display::None` + invalidate** (owner choice). The node stays in
  taffy (preserving the descriptor↔taffy 1:1 lockstep `export_focus_rects` zips on);
  draw (`collect_node :1041`) + focus (`collect_focus_node :950`) walks skip it.
  Applied in layout/draw/focus walks **only**, never the `reconcile` descriptor walk
  (`presentation_cells.rs:74`) — so `localState` scopes survive hide/show. A change
  in a visibility predicate's resolved value re-exports the cached `FocusRectList` +
  dirties layout (`lib/ui.md` §3 rebuild model).
- **`disabled` activation is `main.rs::fire_focused_button_activation` (`:2923`)**,
  NOT `ui_focus.rs` — so Task 3 touches `main.rs` (shared) and must also skip the
  pointer (`hit_test_topmost`) + hover (`tick`) paths.
- Mechanical: the **hand-written `data_descriptors.rs` bridge** needs a reader per
  new field in BOTH js + lua converters (serde-only fields pass round-trip tests but
  drop on the live path); serde skips (`Priority::is_polite`, `is_false`,
  `Option::is_none`); Task 2 split into 2a (resolution + `FocusRect.disabled`) / 2b
  (visibility); `localState` lives only on `ContainerWidget` (vstack/hstack), not Grid.

## Owner decision (2026-06-14 #3): keep expanded G2 vs roadmap, realign roadmap

Roadmap-alignment review flagged drift: roadmap G2 is "a11y metadata nothing
consumes yet, future-proofing, **may trail BIS**" and **BIS depends on G1/E/F/SE,
not G2** — and the reactive layer (Predicate/`selected`/`visibleWhen`) was never in
roadmap G2; BIS's screens (settings via modal-stack sub-menus, not in-page tabs)
don't strictly need it. Owner reviewed and **chose to keep the expanded reactive
G2** and bring it forward as a real BIS prerequisite. Consequence applied:
`roadmap.md` updated — G2 description expanded, **BIS now depends on G2**, G2 lands
**before** BIS (no longer trails). SE stays independent of G2 (may ship in
parallel). Spec unchanged from the reactive rewrite.

## Expanded-scope anchors

- `AnchoredTree` envelope — `descriptor.rs:193` (beside `capture_mode:200`,
  `initial_focus:202`). Home for `accessible_name` + `role`.
- `ImageWidget` — `descriptor.rs:522` (`asset`, `id`, `focus_neighbors`; no name).
  Gets `label` xor `decorative`.
- `CaptureMode` — `descriptor.rs:55`; `Widget` enum — `:255`.
- Focus engine — `input/ui_focus.rs` (1753 lines): `move_focus:443`,
  `initial_focus_id:619`, activation (`focused_on_press`). Where `disabled` is
  honored (skip nav/initial-focus, block activation) via the focus-rect list.
- **`descriptor.rs` is 1574 lines** — ~2× the split-before-extend threshold. G2
  adds fields across ~6 structs + envelope + two enums + a variant → split first
  (Task 0), behavior-preserving, round-trip tests (`:742-848`) stay byte-identical.
- `typedef.rs` 3673 lines, `ui_focus.rs` 1753 — large but tabular/test-heavy;
  extend in place.

## SDK layout (G1a)

`sdk/lib/ui/`: `widgets.ts` (Text/Panel/Image/Spacer/Button/Slider/Bar),
`layout.ts` (VStack/HStack/Grid), `tree.ts` (`Tree`), `text.ts` (`LocalizedText`),
`state.ts` (`StoreHandle<T>`, `LocalStateHandle<T>`, `ui.createLocalState`),
`reactions.ts`. Barrel: `sdk/lib/index.ts:40-92`. Luau twins alongside.

Props (TS, `widgets.ts`):
- `ButtonProps` `:518` — `{ id: string; label: LocalizedText; onPress: ReactionHandleRef | string; repeatOnHold?; focusNeighbors? }` (label required, **no labelledBy**).
- `SliderProps` `:573` — `{ id; label: LocalizedText; bind; min; max; step; capturesNav?; focusNeighbors? }` (label required, **no labelledBy**).
- `BarProps` `:626` — non-interactive, no label.
- Text/Panel/Image/Spacer — non-interactive, no label (Text carries `content`, not `label`).

## Rust descriptor enum (Goal B)

`render/ui/descriptor.rs:242` — `#[serde(tag="kind", rename_all="camelCase")] pub enum Widget { Text, Panel, Image, VStack, HStack, Grid, Spacer, Button, Slider, Bar }` (10 variants). Flat-object wire (`{"kind":"button",…}`).
- `ButtonWidget` `:629` — `label: String` `:631`, `id: String` `:630`, `on_press: String` `:634`, `focus_neighbors: FocusNeighbors` `:638`. **No `labelled_by`.**
- `SliderWidget` `:661` — `label: String` `:663`, `id: String`, `bind: SliderBind`, `focus_neighbors`. **No `labelled_by`.**
- Round-trip tests `:742-848` (byte-identical JSON).

## Bridge (G1a)

`anchored_tree_from_js_value` / `anchored_tree_from_lua_value` in
`scripting/data_descriptors.rs` (per-runtime field readers, beside
`entity_descriptor_from_js/_from_lua`), re-exported via `conv.rs`. Named
load-time error on malformed input, no panic. G2's `labelledBy` XOR validation
and `Announce` reading land here.

## Typedef emitter

`scripting/typedef.rs`: `rust_to_ts` `:56`, `rust_to_luau` `:198`,
`emit_ts_type` `:348`, `engine_slot_groups` `:480`. UI widget types are **not**
registry-emitted — they live in the static `TS_SDK_LIB_BLOCK` (~`:700`) /
`LUAU_SDK_LIB_BLOCK` (~`:1100`), hand-written, sourced from `sdk/lib/ui/*`. G2
edits these blocks + the SDK factory files + `descriptor.rs` together. The
`gen-script-types` drift check + parity test gate them. **No `tsc` CI** — author
compile-preconditions ship as `@ts-expect-error` fixtures (the G1a pattern),
not an enforced build gate.

## Nav intents (Goal F)

`input/ui_nav.rs:31` — `enum NavIntent { Up..Options }` (10); `wire_name()` `:58`
→ `"nav.up"`… TS type already template-literal (`typedef.rs` block); Luau flat
union. No code change for G2 — verification + doc only.

## Wave seam with SE

G2 touches `render/ui/descriptor.rs` (Announce variant + `labelled_by` fields),
`scripting/data_descriptors.rs` (bridge), `sdk/lib/ui/widgets.{ts,luau}`,
`scripting/typedef.rs` **widget** SDK-block, `sdk/lib/index.ts` barrel. SE
touches the **reaction** SDK-block in the same `typedef.rs` and the same barrel
— different sections. No shared descriptor/bridge edits (SE touches neither).
Regenerate typedefs after both land.
