# M13 Goal G2 — A11y Metadata Foundation + SDK Type-Safety

> Wave plan 2 of 2 (sibling: **SE**, `drafts/M13--screen-space-effects/`). Both
> ship in one /orchestrate; mutually independent except the typedef/barrel seam
> noted under Sequencing. Downstream convergence: **BIS** authors
> pause/dialog/death/settings/HUD against this contract — landing G2 before BIS
> means those screens are correct-by-construction, authored once against the
> final a11y + type surface, never retrofitted. Grounding: `research.md`,
> `ui-layer.md` §15–§16, §19–§20. Prereqs: G1 + F shipped (`done/M13--*`).

## Goal

Ship the **complete a11y metadata contract** for the UI descriptor surface —
accessible name, role, state, value semantics, image alt/decorative, modal
name+role, and live-region announcements — as typed, validated, emitted
descriptor fields with authoring preconditions, plus per-kind type narrowing.
Screen-reader *consumption* stays deferred (§19–§20); G2 ships the durable
*shape* nothing reads yet, so BIS authors its screens once against a finished
contract. The one behavior exception is `disabled` (a no-op "disabled" widget is
a footgun), honored by the existing focus/activation path.

## Scope

### In scope

- **Accessible name.** `label` / `labelledBy` on interactive widgets, **exactly
  one** required — factory throw + bridge named-error. `label` already required
  on Button/Slider; this adds the `labelledBy` alternative and relaxes `label`
  to one-of.
- **Role.** Optional `role` override on any widget; defaults to the kind's
  implicit role (Button→button, Slider→slider, Bar→progressbar, Image→image,
  containers→group, Text→none). Lets a composed component stamp roles —
  `tablist`/`tab`, `dialog`, `menu`/`menuitem`, `list`/`listitem`, `heading`,
  `checkbox`/`radio`, `group`, `none` — onto the primitives it returns. A small
  closed `Role` enum, not open ARIA.
- **State.** `disabled` / `selected` / `checked` / `expanded` optional flags on
  widgets. **`disabled` is honored** by the focus engine (skip in navigation +
  initial-focus, block activation); the rest are metadata-only (no built-in
  consumer yet — a modder component sets them; a future toggle widget reads
  `checked`).
- **Value semantics.** Slider/Bar already bind a value with min/max; role +
  bind is the announceable contract. Verify it is fully expressible; add nothing
  if so.
- **Image alt / decorative.** `Image` requires an accessible name (`label`)
  **xor** explicit `decorative: true` — no silently-unnamed meaningful image.
- **Modal name + role.** `AnchoredTree` envelope carries `accessibleName` +
  `role` (`dialog`/`menu`/`hud`/…) so a pushed modal announces its context.
- **`Announce` node.** Net-new non-visual `Widget::Announce { text, priority:
  polite|assertive }`; layout/draw skip it (zero rects/glyphs); bridge reads it.
- **Per-kind narrowing + fixtures.** Verify/tighten that emitted types forbid
  cross-kind prop leakage and make an unnamed interactive widget a type error;
  `@ts-expect-error` author fixtures + typedef snapshots (the G1a no-`tsc`-CI
  pattern). Regenerate typedefs; TS/Luau parity; `docs/scripting-reference.md`.

### Out of scope

- **Screen-reader / AT consumption** — deferred (§19–§20). G2 ships shape only;
  the sole behavior is `disabled` honoring (a metadata-only disabled is unsafe).
- **ARIA breadth beyond the closed role set** — landmarks, `describedBy`,
  heading-level hierarchy, focus-trap semantics. Additive later if a consumer
  justifies.
- **Template-literal nav intents** — TS shipped (F); Luau impossible (no
  template-literal types). Documentation only, no code change.
- **JSX-via-SWC** — no tooling exists; a separate optional tooling spec.
- **Localization runtime** — `LocalizedText` (G1a) is the seam; unchanged.

## Acceptance criteria

- [ ] `Button`/`Slider` accept `labelledBy` (node-`id` ref); `label` is optional
  at the type level; a call with **neither** throws a field-named `Error`, with
  **both** throws, with **exactly one** succeeds; the bridge surfaces a named
  load-time error (no panic) for the zero/both case from raw wire.
- [ ] `Image` requires `label` **xor** `decorative: true` — neither/both is a
  factory throw and a named bridge error; a decorative image carries no name.
- [ ] A widget accepts an optional `role` from the closed `Role` set; absent
  resolves to the kind's implicit role; an interactive `role` override on a
  non-interactive kind makes a name required (or this generalization is
  explicitly deferred — see Open questions).
- [ ] `disabled` widgets are skipped by focus navigation and initial-focus
  selection and cannot be activated; `selected`/`checked`/`expanded` round-trip
  as metadata with no behavioral effect.
- [ ] `AnchoredTree` carries optional `accessibleName` + `role`; a tree without
  them deserializes byte-identically to its pre-G2 wire form.
- [ ] `Announce({ priority: "polite" }, "…")` round-trips to `Widget::Announce`
  byte-identically; layout emits zero rects and draw zero glyphs for it; a
  garbled `Announce` is a named load-time error, not a panic.
- [ ] Every pre-G2 tree (no new a11y fields) deserializes byte-identically — all
  additions are skip-serialized when absent (no wire break for shipped content).
- [ ] Emitted typedefs narrow props per kind (a `Button` with a Text-only
  `content` prop is a type error; a `Bar` needs no name) — `@ts-expect-error`
  fixtures + a typedef snapshot test; `gen-script-types` reports no drift; TS and
  Luau SDK blocks stay parity-checked.
- [ ] A typedef snapshot documents the shipped TS template-literal `NavIntent`
  and the Luau flat union (verification only).
- [ ] `docs/scripting-reference.md` covers name/role/state, image
  alt/decorative, modal naming, and `Announce`.

## Tasks

### Task 0: split `descriptor.rs` (behavior-preserving)
`render/ui/descriptor.rs` is 1574 lines; G2 adds fields across ~6 widget structs
plus the envelope and two enums. Split first along existing seams — widget
structs and the `AnchoredTree` envelope into a `descriptor/` submodule, the
`Widget` enum + serde wire contract unchanged — so the round-trip tests
(`descriptor.rs:742-848`) stay byte-identical. No behavior change; the gate is
green tests + identical wire. Sequenced right before the extension. `Depends on`
nothing.

### Task 1: a11y descriptor vocabulary + bridge
In the split modules: add `label: Option<String>` + `labelled_by: Option<String>`
(interactive widgets), `role: Option<Role>` (all widgets) with a closed `Role`
enum, the state flags (`disabled`/`selected`/`checked`/`expanded`), `Image`
`label` xor `decorative: bool`, `AnchoredTree` `accessible_name` + `role`, and
the `Widget::Announce(AnnounceWidget { text, priority })` variant (non-visual —
layout/draw skip it). Extend the bridge (`data_descriptors.rs`) to read them and
enforce the preconditions (exactly-one name; image name-xor-decorative; Announce
shape) with named load-time errors, no panic. All new fields skip-serialized when
absent. `Depends on` Task 0.

### Task 2: SDK factories + types + typedefs
Mirror Task 1 in `sdk/lib/ui/{widgets,tree}.{ts,luau}` + barrel: the name XOR
prop unions, `role`/state props, `Image` label-xor-decorative, the `Tree`
envelope `accessibleName`/`role`, and the `Announce` factory. Update the
`typedef.rs` widget SDK-block + regenerate. Field-named throws; documented
defaults. `Depends on` Task 1. **Wave seam (SE):** coordinate `typedef.rs`
SDK-block + `sdk/lib/index.ts` barrel with SE (different sections); regenerate
once both land.

### Task 3: `disabled` interaction honoring
`input/ui_focus.rs`: a `disabled` node is skipped by `move_focus` and
`initial_focus_id`, and its activation is blocked. Flows the `disabled` flag into
the focus-rect list the engine consumes. The only behavior G2 ships. `Depends on`
Task 1 (the field). Disjoint file from Task 2 — concurrent.

### Task 4: narrowing verification + fixtures + docs
Typedef snapshot tests proving per-kind narrowing + the unnamed-interactive type
error; `@ts-expect-error` author fixtures; document the nav-intent
template-literal status (TS) and Luau limitation; regenerate typedefs; update
`docs/scripting-reference.md`. `Depends on` Tasks 2, 3.

## Sequencing

**Phase 0 (sequential):** Task 0 — split before extend.
**Phase 1 (sequential):** Task 1 — the descriptor vocabulary + bridge backbone.
**Phase 2 (concurrent):** Task 2 (SDK/types), Task 3 (focus honoring) — disjoint files.
**Phase 3 (sequential):** Task 4 — consumes Tasks 2 + 3.
**Wave seam (SE):** Task 2/4 edit the **widget** SDK-block in `typedef.rs` and
the `sdk/lib/index.ts` barrel; SE edits the **reaction** block in the same files
— different sections. Coordinate; regenerate typedefs once both land. G2 owns all
`descriptor.rs`/tree-bridge edits (SE touches neither).

## Rough sketch

- `Role` (closed enum): `button, slider, progressbar, tab, tablist, checkbox,
  radio, menu, menuitem, dialog, list, listitem, heading, group, image, none`.
  Implicit role derived from `Widget` kind; `role` overrides.
- Name XOR (TS): `type ButtonProps = ButtonBase & ({ label: LocalizedText;
  labelledBy?: never } | { labelledBy: NodeId; label?: never })`; Luau validates
  in the factory. Same XOR shape for `Image` (`label` vs `decorative`).
- `Announce`: smallest variant; layout walker returns early (no taffy node),
  draw walker skips. `NodeId = string`.
- Anchors: `descriptor.rs` Widget enum `:255`, `ImageWidget :522`, `AnchoredTree
  :193` (beside `capture_mode`/`initial_focus`); bridge in `data_descriptors.rs`
  (`anchored_tree_from_{js,lua}_value`); focus engine `ui_focus.rs`
  (`move_focus :443`, `initial_focus_id :619`, activation).

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| labelledBy | `labelled_by: Option<String>` | `"labelledBy"` | `labelledBy: NodeId` | `labelledBy` |
| label (relaxed) | `label: Option<String>` | `"label"` (omit absent) | `label?: LocalizedText` | `label?` |
| role | `role: Option<Role>` | `"role"` | `role?: Role` | `role?` |
| state flags | `disabled`/`selected`/`checked`/`expanded`: `bool` | same camelCase, omit-when-false | same | same |
| image decorative | `decorative: bool` | `"decorative"` | `decorative?: true` | `decorative?` |
| envelope name/role | `accessible_name`/`role` on `AnchoredTree` | `"accessibleName"`/`"role"` | `accessibleName?`/`role?` | same |
| Announce node | `Widget::Announce(AnnounceWidget)` | `{ "kind": "announce", "text", "priority" }` | `Announce(props, text)` | `Ui.Announce(props, text)` |
| priority | `Priority::{Polite, Assertive}` | `"polite"`/`"assertive"` | `"polite" \| "assertive"` | same |

## Open questions

- **Name-required from an interactive `role` override?** Keying the
  name-precondition off *effective role* (kind or override) is the clean
  generalization but complicates validation. Recommend v1 keys it off the
  interactive *kinds* (Button/Slider) + Image, treats `role` override as additive
  metadata, and defers the role-derived precondition. Decide at implementation.
- **Validate `labelledBy` / `accessibleName` target ids exist at load?** A
  dangling-ref nicety needing a tree-wide id pass. Defer to a follow-up unless it
  earns its place.
- **`disabled` styling.** G2 honors `disabled` behaviorally; the *visual* dim
  (a theme treatment) is a styleRanges/theme concern, not G2 — note for BIS.
