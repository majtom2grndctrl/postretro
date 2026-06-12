# M14 Behavior IR — Movement Dash Adopter (`runtime` Rename + Expression-Capable Dash Fields)

## Goal

First real adopter of the M14 IR substrate (plan 3 of the sequential chain): six dash descriptor fields accept either a plain literal or a `RuntimeValue` expression the engine re-evaluates at an engine-pinned moment, binding a movement-local scope. Proves the read path, the pluggable-namespace seam, and per-tick eval on the hottest path against real authored surface. Prepended: the modder-facing rename `ir` → `runtime` — free now (the substrate shipped with zero adopters; the wire format carries no "ir"), breaking after this plan ships.

## Scope

### In scope

- **SDK rename + sugar (T1).** `ir` namespace → `runtime`; `input()` builder → `read()`; typedef type names `IrNode` → `RuntimeValue`, per-op `Ir*` → `Runtime*` (`IrInput` → `RuntimeRead`), interface `Ir` → `Runtime`. Builder params widen to accept bare `number`/`boolean` literals, auto-wrapped into `const` nodes — in both runtimes, emitting identical IR. SDK surface only: Rust `ir` module names (`IrNode`, `BakedIr`, …) and the wire format (op tags incl. `"op": "input"`, field names) are unchanged.
- **Six expression-capable dash fields.** `boostSpeed`, `momentumRetention`, `steerControl`, `dashDrag`, `cooldownMs` accept `number | RuntimeValue`; `preserveVertical` accepts `boolean | RuntimeValue`. `airDashes` stays plain `u32` (structural budget — a noun, not derived behavior; IR has no integer type).
- **Engine-pinned evaluation moments.** Entry-moment (evaluated once in the dash-entry path): `boostSpeed`, `momentumRetention`, `cooldownMs`, `preserveVertical`. Per-tick (evaluated each tick while dashing): `steerControl`, `dashDrag`.
- **Movement-local binding scope.** Fixed read-only namespace mirroring the `movement.md` §2 trigger-vocabulary nouns: `speed` (horizontal), `verticalSpeed`, `grounded` (bool), `chargesRemaining`, `cooldownMs`, `elapsedMs`. Indexed handles over a fixed-size snapshot array — engine-side binding; the `entity_model.md` §7b script-opacity invariant holds by construction.
- **Declaration-time validation, no runtime fallback.** Expressions parse and bind (type-check + name-resolve against the fixed namespace) at descriptor declaration, failing loud as `DescriptorError` like every other malformed descriptor field. A descriptor that validates cannot fail at runtime.
- **Range clamps at consumption.** Literal fields keep their declaration-time range checks; expression results clamp to the same documented ranges at the eval site (silent, alloc-free, per-tick).
- **Hot reload.** Expression edits rebind through the existing movement refresh path.
- **Dev content + modder docs + empirical findings** (combinator demand recorded for plan 4; `ui.localState()` naming note recorded for the G1 draft).

### Out of scope

- **Any store-scope adoption.** Dash expressions bind only the movement scope; `read("player.health")` is unresolvable here by design. Cross-scope composition waits for a motivating mod.
- **The write path.** No expression writes anything; movement is the read-path adopter. The first hot write adopter (shield recharge, with its non-logging alloc-free write variant) is the Shields milestone.
- **Boolean combinators (`and`/`or`/`not`), vector values, stateful nodes.** Vocabulary unchanged; T5 records whether real expressions wanted combinators.
- **Other movement fields.** Ground/air/fall/crouch/forgiveness tuning stays literal-only; the `NumberOrIr` pattern is built to extend, extension is demand-driven (plan 4 discipline).
- **Rust-internal renames.** `scripting/ir/` module, `IrNode`, `BakedIr` keep their names; context docs keep "Behavior IR".
- **The `BakedIr` envelope for descriptor fields.** Descriptor expressions are re-emitted from mod source every load and never persist, so they cross as bare node trees; the versioned envelope stays reserved for persisted IR (`load_baked_ir` is untouched).
- **Roadmap/G1 wording edits.** Recorded here; applied at promotion per convention.

## Acceptance criteria

- [ ] Generated `postretro.d.ts` / `postretro.d.luau` declare a `runtime` global with `read`, `constant`, and all op builders; no `ir` global and no `Ir`-prefixed type names remain in generated typedefs; the type-definition drift test passes.
- [ ] The same expression authored with bare literals in TS and in Luau canonicalizes to IR byte-identical to its explicit-`constant` form (parity test extended with a literal-sugar case).
- [ ] Existing IR wire round-trip and version tests pass unmodified — the rename touches no wire bytes.
- [ ] Each of the six fields accepts a bare literal or an expression in both runtimes; a literal-only dash block parses and behaves exactly as today (existing dash descriptor and movement tests pass unchanged).
- [ ] Declaration rejects, as `DescriptorError` without panicking: an unknown `read` name, a type-table violation (e.g. a boolean operand to `clamp`), and a malformed node object. Literal out-of-range rejection is unchanged.
- [ ] An authored expression observably changes dash behavior: a `momentumRetention` select on `grounded` produces different entry velocities grounded vs airborne; a `steerControl` ramp over `elapsedMs` produces increasing steer authority across a dash (unit tests over the entry/intent paths).
- [ ] Expression results clamp to field ranges: `momentumRetention` evaluating to 3.0 behaves as 1.0; `cooldownMs` evaluating negative arms as 0.
- [ ] Snapshot semantics hold: `chargesRemaining` at entry reads the post-spend value; `elapsedMs` reads 0 at entry and the live value per-tick.
- [ ] Zero heap allocations across the eval pass of a dash tick with all six fields authored as expressions (`AllocSnapshot` harness; counter armed after snapshot refresh, per-tick path only).
- [ ] Hot-reloading an edited expression in the player descriptor updates live dash behavior (refresh-plan path unit test).
- [ ] `docs/scripting-reference.md` documents runtime values (the load-vs-runtime teaching line, the builder vocabulary, the six dash fields, validation errors) in example-led human prose; `content/dev/scripts/player.ts` ships at least two fields authored as expressions.
- [ ] The combinator finding (did any authored expression want `and`/`or`/`not`?) is recorded in this plan folder — review gate, feeds plan 4.

## Tasks

### Task 1: SDK rename + literal auto-wrap sugar

Rename the authoring surface while it has zero consumers. `sdk/lib/ir.ts` → `sdk/lib/runtime.ts`, `ir.luau` → `runtime.luau`; the exported object becomes `runtime`, `input(name)` becomes `read(name)`, `constant` stays. Widen every builder parameter to accept `RuntimeValue | number | boolean` (TS) / the Luau equivalent, auto-wrapping bare literals into `{ op: "const", value }` — wrapping logic must be identical in both runtimes so parity holds. Update `sdk/lib/index.ts` (the `export { ir }` line) and `scripting/luau.rs`, which embeds `ir.luau` via `include_str!` (`IR_LUAU_SRC`) and installs the table as global `"ir"` — both the include path and the installed global name move to `runtime` (the TS prelude needs no hand edit; `build.rs` regenerates it from `index.ts`). In `typedef.rs`, rewrite the embedded static blocks (`TS_SDK_LIB_BLOCK`, `LUAU_SDK_LIB_BLOCK`): per-op types `IrConst` → `RuntimeConst`, `IrInput` → `RuntimeRead`, `IrAdd` → `RuntimeAdd` (…all 15), union `IrNode` → `RuntimeValue`, interface `Ir` → `Runtime`, global declaration → `export const runtime: Runtime;` — and widen the declared builder signatures to match the sugar. Update the authored-source strings in `ir/e2e_tests.rs` and `ir/parity_tests.rs` (`ir.input(...)` → `runtime.read(...)`), add a bare-literal parity case, and regenerate `sdk/types/` via `gen-script-types`. The wire is untouched: serde op tags (including `"op": "input"`) and the Rust `ir` module keep their names; only doc comments referencing the `ir` global update.

### Task 2: Expression-capable dash descriptor fields

Add two field types beside `DashParams` in `data_descriptors.rs`: `NumberOrIr` (`Literal(f32)` | `Ir(IrNode)`) and `BoolOrIr`, serde-untagged so `DashParams`'s existing `Serialize`/`Deserialize` derives round-trip them (bare scalar ↔ literal, op-tagged object ↔ node). Retype the five scalar fields and `preserve_vertical`; `air_dashes` stays `u32`. The parsers are hand-written, so extend `dash_params_from_js` and `dash_params_from_lua` symmetrically (the missing-Luau-arm parity trap): a field value that is an object/table converts through the existing conv bridge to `serde_json::Value` and deserializes into `IrNode` (an object lacking a recognizable node shape → `DescriptorError::InvalidShape`); a plain number/bool takes the existing literal path with its current range validators unchanged. Expression validation at declaration: wrap the node in a `BakedIr { version: CURRENT_IR_VERSION, output: None, root }` and `bind` it against the movement scope's validation instance (Task 3) — any `BindError` maps to `DescriptorError::InvalidShape` with the reason. Update the dash field registrations in `primitives/mod.rs` (the `.field("boostSpeed", "f32", …)` rows) so generated typedefs declare `number | RuntimeValue` / `boolean | RuntimeValue`, and refresh the field doc strings to name each field's evaluation moment. Extend the dash parser test battery (the `JS_DASH_FULL` / `LUA_DASH_FULL` set) with expression-bearing and malformed-expression cases.

### Task 3: Movement binding scope

A `MovementScope` owned by the movement module (movement owns its scope the way the renderer owns the GPU; the `ir` module stays adopter-agnostic), implementing `BindingScope` with `InputHandle = usize` into a fixed snapshot array and `resolve_output` → `None` (read-only). Static name table, in this order: `speed` (Number — horizontal `|velocity.xz|`), `verticalSpeed` (Number — `velocity.y`), `grounded` (Bool — `is_grounded`), `chargesRemaining` (Number — `air_dashes_remaining as f32`), `cooldownMs` (Number — `dash_cooldown_ms`), `elapsedMs` (Number — the `Dash` state's `elapsed_ms`, 0.0 outside the state). A refresh method fills the array from `&PlayerMovementComponent` (+ the elapsed value passed in); a validation constructor needs no component (bind only consults names/types, never values). Unit tests: the substrate's portability AC pattern — a tree binding against `MovementScope` and `StubScope` proves nothing store-shaped leaked; snapshot refresh allocates nothing.

### Task 4: Bind at materialization, eval at the pinned moments, range clamps

`PlayerMovementComponent` gains a `DashPrograms` struct (an `Option<BoundProgram<MovementScope>>` per expression-capable field, `None` when the field is literal) built inside `from_descriptor` by binding each `NumberOrIr::Ir` against the validation scope. Derive constraint: the component flows through `ComponentValue`'s `Clone`/`PartialEq`/`Serialize`/`Deserialize` derives (`registry.rs`), and `BoundProgram`/`BoundNode` implement only `Debug` — keep those derives compiling: add `Clone` to the bound types (handles are `Clone`), mark the programs field `#[serde(skip)]` with an all-`None` default (programs are derived data; `from_descriptor` is the sole builder and rebinds them), and give `DashPrograms` a `PartialEq` that treats programs as always equal (they are derived from `dash`, which is compared). Hot reload needs no new hook: `plan_movement_replace` already rebuilds via `from_descriptor`, which rebinds. Post-validation bind failure is unreachable (same static table validated at declaration); if it ever occurs, warn once and materialize with dash disabled — degrade visibly, never panic. Eval wiring, no signature changes (both sites already hold the component): in `try_enter_dash`, after the cooldown/charge gate and air-charge spend, refresh a local `MovementScope` snapshot (`elapsedMs` = 0) and resolve the four entry values — each `eval_value` into a local before any velocity mutation (the function already computes locals first), literal fields skipping eval entirely; the `cooldown_ms` result arms `dash_cooldown_ms` as today. In `dash_intent`, refresh the snapshot once per tick and resolve `steer_control` and `dash_drag` the same way (`elapsedMs` reads the `Dash` state's `elapsed_ms` as it stands at the top of the intent — 0 on the first dash tick, accumulating thereafter; the intent increments it later in the tick). Clamp every evaluated result at the consumption site, silently and allocation-free: `boostSpeed`, `dashDrag`, `cooldownMs` to ≥ 0; `momentumRetention`, `steerControl` to [0, 1] (eval's finite-guard already excludes NaN/Inf). One deliberate divergence: `boostSpeed`'s literal bound is exclusive (finite > 0, `validate_positive_finite`) — no clamp can reproduce an open bound, so the eval-site floor is 0 and an expression evaluating to 0 yields a zero-boost dash, while a literal 0 still rejects at declaration. Literal paths must remain bit-identical to today. Zero-alloc AC test lives here, plus the entry/per-tick behavior tests and the refresh-plan hot-reload test (edited expression rebinds and changes behavior).

### Task 5: Dev content, modder docs, findings

Author at least two expression fields in `content/dev/scripts/player.ts` (suggested: the `momentumRetention` grounded/air select and the `steerControl` ramp over `elapsedMs` — exercising bool inputs, `select`, and per-tick eval). Document in `docs/scripting-reference.md` (human-facing, example-led — not context-library register): the one-line teaching ("your script runs once at load; a `RuntimeValue` crosses into the engine and is re-evaluated from live gameplay state"), the `runtime.*` builder vocabulary, `read` names available to dash fields, the per-field evaluation moments, literal-sugar examples, and the declaration-error rows (mirroring the fog-primitive error-table precedent). Record in a `findings.md` in this plan folder: whether any authored expression wanted `and`/`or`/`not` (plan-4 input), plus the naming reservation — the M13 G1 roadmap term `liveValue()` should become a state-named hook under the `ui` namespace (working name `ui.localState()`), since `Runtime` now means computed-not-stored; roadmap wording amended at promotion.

## Sequencing

**Phase 1 (sequential):** Task 1 — the rename must land before any new surface (typedef rows, docs, content) references the vocabulary; also touches the parity/e2e tests (and adds the bare-literal parity case).
**Phase 2 (concurrent):** Task 2 (descriptor types, parsers, typedef rows), Task 3 (movement scope — new code against the existing `ir` module; no shared files with Task 2). One-way dependency: Task 2's declaration-time bind call consumes Task 3's validation constructor — Task 3 lands first, or Task 2 wires that one call last.
**Phase 3 (sequential):** Task 4 — consumes Task 2's field types and Task 3's scope; owns `movement/mod.rs` + `player_movement.rs` edits.
**Phase 4 (sequential):** Task 5 — consumes everything.

## Rough sketch

- **Field union decode rule.** Wire value is a JSON/Luau scalar → literal; an object/table → must deserialize as an op-tagged node, else `DescriptorError`. Serde mirror: `#[serde(untagged)]` over `Literal(f32)` / `Ir(IrNode)` (the substrate's untagged-`IrValue` precedent).
- **Scope shape** (`// Proposed design`): `MovementScope { values: [IrValue; 6] }`; `const INPUTS: [(&str, IrType); 6]`; `resolve_input` scans `INPUTS`, returns `ResolvedInput { handle: idx, ir_type }`; `read` indexes `values`. Refresh writes all six slots from the component each call.
- **Programs on the component.** `DashPrograms { boost_speed, momentum_retention, steer_control, dash_drag, cooldown_ms, preserve_vertical: Option<BoundProgram<MovementScope>> }`, sibling to `dash: Option<DashParams>`; built in `from_descriptor`, rebuilt by the existing refresh path. Eval order inside the intent fns: snapshot → eval into locals → mutate, so program borrows never overlap the velocity writes.
- **Helper** `fn resolve_number(field: &NumberOrIr, program: &Option<BoundProgram<MovementScope>>, scope: &MovementScope, lo: f32, hi: f32) -> f32` (and a bool sibling) keeps the six call sites uniform: literal → value, expr → `eval_value` + clamp.
- **Typedef rows.** The registry `.field(...)` calls take a type string today (`"f32"`); the union needs the emitter to print `number | RuntimeValue` — either a dedicated field-type token or a raw-type passthrough, implementer's call against `primitives/mod.rs` + `typedef.rs`; the emitted shape is the contract.

## Boundary inventory

Wire/serde is unchanged by this plan — rows below pin the renamed author surface against the existing wire and Rust names.

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| builder namespace | n/a (pure SDK) | n/a | `runtime` global | `runtime` global |
| named-input leaf | `IrNode::Input { name }` | `{ "op": "input", "name": "speed" }` | `runtime.read("speed")` | `runtime.read("speed")` |
| node union type | `IrNode` | op-tagged object | `RuntimeValue` | `RuntimeValue` |
| per-op types | `IrNode` variants | `"op"` tags (unchanged) | `RuntimeConst`, `RuntimeRead`, `RuntimeAdd`, … `RuntimeSelect` | same |
| builder interface | n/a | n/a | `Runtime` | same |
| literal sugar | n/a | emits `{ "op": "const", "value": … }` | builder args accept bare `number`/`boolean` | same |
| expression field | `NumberOrIr` / `BoolOrIr` | bare scalar ⇔ literal; op-object ⇔ node | `number \| RuntimeValue` / `boolean \| RuntimeValue` | same |
| scope inputs | indexed handles | n/a (names in IR JSON) | `"speed"`, `"verticalSpeed"`, `"grounded"`, `"chargesRemaining"`, `"cooldownMs"`, `"elapsedMs"` | same |

## Decisions

- **Rename depth → SDK surface only** (owner, 2026-06). Rust keeps `ir`/`IrNode`/`BakedIr`; "Behavior IR" stays the architecture term. This inventory records the mapping — the established pattern for cross-layer name differences.
- **`runtime` means computed, `State` means stored** (owner, 2026-06). The taxonomy rule the SDK teaches: `StateValue`/`defineStore` = stored state; `RuntimeValue` = engine-computed derivation, never stored. Hence the G1 `liveValue()` reservation moves to a state-named `ui`-namespace hook (recorded in Task 5's findings; roadmap wording at promotion).
- **No envelope in descriptors.** Versioning guards persisted IR; load-time-rebuilt descriptor expressions fail loud at declaration instead.
- **Declaration-time validation, not runtime fallback.** The namespace is static, so unknown-name/type errors are author errors caught where every other descriptor error is caught.
- **Clamp evaluated results to literal ranges at consumption.** Declared ranges are field contracts; expressions can't be range-checked at declaration, so the eval site enforces them — silent and alloc-free, mirroring eval's total-semantics posture (not the store's logging clamp). One documented exception: `boostSpeed`'s open literal bound — see Task 4.
- **Entry snapshot reads post-spend `chargesRemaining`** — "charges you have now," matching the motivating last-charge-costs-more example.

## Open questions

- None blocking. The `primitives/mod.rs` union-type emission mechanics (field-type token vs raw passthrough) and `RuntimeOperand` alias naming are implementation calls within pinned contracts; combinator demand is the empirical finding Task 5 records.
