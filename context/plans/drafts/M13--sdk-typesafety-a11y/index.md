# M13 Goal G2 — SDK Type-Safety + A11y Preconditions

> Wave plan 2 of 2 (sibling: **SE**, `drafts/M13--screen-space-effects/`). Both
> ship in one /orchestrate; mutually independent except the typedef/barrel seam
> noted under Sequencing. Downstream convergence: **BIS** (built-in screens)
> authors pause/dialog/death/settings against this hardened contract — landing
> G2 before BIS means those screens are correct-by-construction, not retrofitted.
> Grounding: `research.md`, `ui-layer.md` §15–§16. Prereqs: G1 shipped
> (`done/M13--g1-sdk-core`, `…-lifecycle`).

## Goal

Harden the UI SDK authoring surface G1 shipped so accessibility is an authoring
precondition and widget props are kind-narrowed — the contract BIS binds
against. **Most of the original G2 bullet is already met** (`label` required, TS
template-literal nav intents, per-factory typing) or impossible (Luau
template-literals) or deferrable (JSX). The genuine net-new work is the
`labelledBy` accessible-name alternative and the `Announce` node, plus the
verification fixtures that lock the per-kind contract.

## Scope

### In scope

- **`labelledBy` accessible-name alternative.** Add `labelled_by: Option<String>`
  (references a node `id`) to `ButtonWidget` / `SliderWidget`; relax `label` to
  optional. Author precondition: **exactly one of `label` / `labelledBy`** —
  enforced by factory validation (field-named `Error`) and a bridge load-time
  check (named error, no panic). TS expresses the XOR via a discriminated prop
  union; Luau enforces at runtime (no XOR types). Any future interactive widget
  kind inherits the rule.
- **`Announce` node.** Net-new `Widget::Announce(AnnounceWidget { text:
  LocalizedText, priority: Polite | Assertive })`. Non-visual: layout and draw
  skip it (zero rects, zero glyphs); the bridge reads it; factory `Announce(props,
  text)` in `widgets.{ts,luau}` + barrel + typedefs. Screen-reader consumption is
  deferred — the engine accepts and ignores the node (the a11y-metadata-nothing-
  consumes-yet contract).
- **Per-kind narrowing, verified.** Confirm and test that emitted widget types
  forbid cross-kind prop leakage and that an unlabeled interactive widget is a
  type error; tighten any loose spot. Ship `@ts-expect-error` author fixtures
  (the G1a no-`tsc`-CI pattern) + typedef snapshot tests.
- Typedef regeneration; TS/Luau SDK-block parity; `docs/scripting-reference.md`
  covering `labelledBy` + `Announce`.

### Out of scope

- **Template-literal nav intents** — already shipped (TS, Goal F) and impossible
  in Luau (no template-literal types; flat union is the permanent form). G2 only
  documents the status; no code change.
- **JSX-via-SWC** — no build tooling exists; adding SWC is a separate optional
  tooling spec, not this wave. Factory-call authoring stays the one path.
- **Working screen readers / announcement playback** — deferred (`ui-layer.md`
  §19–§20). G2 ships the `Announce` shape only.
- **Wire/runtime semantic changes to existing screens** — none. `label`-only
  trees stay wire-valid; `labelledBy` and `Announce` are additive.
- **Re-labeling already-shipped trees** — the demo/built-in trees are revisited
  by BIS, not here.

## Acceptance criteria

- [ ] `Button` / `Slider` accept `labelledBy` (a node-`id` ref); `label` is
  optional at the type level. A factory call with **neither** throws a
  field-named `Error`; with **both** throws; with **exactly one** succeeds. The
  bridge surfaces a named load-time error (no panic) for a malformed/zero/both
  case from raw wire.
- [ ] A tree authored with `label` only deserializes byte-identically to its
  pre-G2 wire form (no break for existing content).
- [ ] `Announce({ priority: "polite" }, "…")` round-trips through the bridge
  byte-identically to a `Widget::Announce`; the layout pass emits zero rects and
  the draw pass zero glyphs for it (asserted); a garbled `Announce` surfaces a
  named load-time error, not a panic.
- [ ] Emitted typedefs narrow props per kind: a `Button` authored with a
  Text-only prop (`content`) is a type error, and a non-interactive `Bar`
  requires no accessible name — both shipped as `@ts-expect-error` fixtures plus
  a typedef snapshot test.
- [ ] A typedef snapshot documents the shipped template-literal TS `NavIntent`
  and the Luau flat-union form (verification AC — no code change).
- [ ] TS and Luau SDK blocks stay parity-checked and `gen-script-types` reports
  no drift after regeneration.
- [ ] `docs/scripting-reference.md` covers `labelledBy` and `Announce`.

## Tasks

### Task 1: `labelledBy` + accessible-name precondition
`render/ui/descriptor.rs`: add `labelled_by: Option<String>` to `ButtonWidget` /
`SliderWidget`, change `label` to `Option<String>`, keep wire camelCase
(`labelledBy`). Bridge (`data_descriptors.rs`): enforce exactly-one-of with a
named error. Factories (`widgets.{ts,luau}`): TS discriminated prop union
(label-variant XOR labelledBy-variant), Luau runtime check; field-named throws.
Update the `typedef.rs` widget SDK-block. `Depends on` nothing. **Wave seam
(SE):** coordinate `typedef.rs` SDK-block + `sdk/lib/index.ts` barrel with SE.

### Task 2: `Announce` node
`descriptor.rs`: new `Widget::Announce(AnnounceWidget { text, priority })`
variant (`#[serde(rename="announce")]`); layout/draw skip it. Bridge reads it.
Factory `Announce(props, text)` in `widgets.{ts,luau}`, barrel export, typedef
emission. Layout test asserting zero contribution. `Depends on` Task 1 (shares
`descriptor.rs` / `widgets.*` / bridge — sequence to avoid churn).

### Task 3: narrowing verification + fixtures + docs
Typedef snapshot tests proving per-kind narrowing and the unlabeled-interactive
type error; `@ts-expect-error` author fixtures (no `tsc` CI); document the
nav-intent template-literal status (TS) and Luau limitation; regenerate
typedefs; update `docs/scripting-reference.md`. `Depends on` Tasks 1–2.

## Sequencing

**Phase 1 (sequential):** Task 1.
**Phase 2 (sequential):** Task 2 — shares `descriptor.rs` / `widgets.*` / bridge
with Task 1.
**Phase 3 (sequential):** Task 3 — consumes Tasks 1–2 for fixtures/snapshots.
**Wave seam (SE):** Tasks 1–2 edit the **widget** SDK-block in `typedef.rs` and
the `sdk/lib/index.ts` barrel; SE edits the **reaction** block in the same files
— different sections. Coordinate and regenerate typedefs once both land. G2 owns
all `descriptor.rs` / tree-bridge edits (SE touches neither).

## Rough sketch

- `labelledBy`: TS XOR via `type ButtonProps = ButtonBase & ({ label: LocalizedText; labelledBy?: never } | { labelledBy: NodeId; label?: never })`; Luau validates in the factory body. Bridge mirrors the existing field-reader error path.
- `Announce`: smallest possible variant; the layout walker returns early on it (no taffy node), the draw walker skips it. `NodeId = string` (a widget `id`).
- All three edits land in `descriptor.rs` + `widgets.{ts,luau}` + `typedef.rs`
  SDK-block + `data_descriptors.rs` bridge — the same four seams G1a established.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| labelledBy | `labelled_by: Option<String>` | `"labelledBy"` | `labelledBy: NodeId` | `labelledBy` |
| label (relaxed) | `label: Option<String>` | `"label"` (omitted when absent) | `label?: LocalizedText` | `label?` |
| Announce node | `Widget::Announce(AnnounceWidget)` | `{ "kind": "announce", … }` | `Announce(props, text)` | `Ui.Announce(props, text)` |
| announce text | `text: String` | `"text"` | `LocalizedText` | `LocalizedText` |
| announce priority | `Priority::{Polite, Assertive}` | `"polite"` / `"assertive"` | `"polite" \| "assertive"` | same |
| node ref | `String` (a widget `id`) | string | `NodeId` (= `string`) | `string` |

## Open questions

- **Validate that a `labelledBy` target id exists at load?** A nicety (catches a
  dangling ref) but needs a tree-wide id pass. Recommend deferring — ship the
  XOR-presence check now, add target-existence validation only if it earns its
  place. Decide at implementation.
- **Is G2 worth a separate spec, or fold the `labelledBy` + `Announce` delta
  into BIS?** Given how small the genuine net-new is, the owner may prefer
  folding. Kept separate here because it is the correct-by-construction gate BIS
  authors against, and it parallelizes cleanly with SE. Owner call.
- **Confirm JSX deferral.** The roadmap marks JSX "optional"; this draft defers
  it. If the owner wants JSX in the wave, it is a fourth task (SWC dependency +
  transform config) and materially enlarges G2.
