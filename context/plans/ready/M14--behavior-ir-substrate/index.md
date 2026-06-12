# M14 Behavior IR — Substrate + Evaluator

## Goal

Realize the Typed Command Buffer (`scripting.md` §11): authored behavior that depends on live state crosses the FFI as a **typed, serializable IR tree**, the VM drops, and a Rust **total evaluator** binds the named leaves to live state and evaluates each tick. This plan ships the substrate — the opcode vocabulary, the wire format, the pure/total/bounded evaluator, the **scoped binding abstraction**, and the **version stamp** — with no behavior adopting it yet. It is the foundation the rest of Milestone 14 rides.

This is **plan 1 of a sequential chain** (1 substrate → 2 versioning [folded here] → 3 first adopter: movement → 4 consolidation, demand-driven). There is no concurrent milestone wave; each plan consumes the prior one's settled output.

## Scope

### In scope

- A closed-vocabulary **IR node tree** — discriminated-union-per-opcode (one `op`-tagged variant per node, the `registry.rs::ComponentValue` tag+payload precedent), serde, crossing the FFI as JSON data and returned through `setupMod` / `setupLevel` like reactions (`{name, JSON args}`, `scripting.md` §11). All v1 opcode tags and field names are single lowercase words.
- The **minimal node set** (`scripting.md` §11, the minimal-node-set paragraph): constant + named-input **leaves**; arithmetic (`add` / `sub` / `mul` / `div`); `clamp`; `lerp`; `select(cond, a, b)`; comparisons (`lt` / `le` / `gt` / `ge` / `eq` / `ne`). A closed two-type value model — number (`f32`) and boolean — with operand/result types pinned in the Type system section.
- A two-phase evaluator: **bind** (once — type-check the tree against the Type system table, resolve every named input and output to a scope-provided handle, produce an eval-ready program) and **eval** (per tick — pure, total, bounded; the eval pass that computes the root value performs **zero heap allocation**).
- A **scoped binding abstraction**: the evaluator binds names through a pluggable scope, *not* a hardwired global namespace. The mod state store is one scope; a movement-local scope (plan 3) is another. A scope may be read-only or read+write, and (for store scopes) carries a capability mode (see Write path).
- Both a **read** path (input leaves → values) and a **named-output write** path (the root's evaluated value → a named output the scope writes). Both shipped and unit-tested against a stub scope; **only the read path has a real consumer later** (movement). The write path is the home for the deferred UI `setState` IR (script-capability) and engine policy writes such as shield recharge (engine-capability) — designed and tested, not wired.
- **Degrade visibly, never panic** (the UI unknown-token rule applied here), split two ways: structural problems caught once at **bind** (unknown input/output name, type-table violation, an input/output bound to a store slot whose declared type does not project to number/boolean, malformed tree) return an error the caller logs once and falls back from; numeric edge cases (÷0, NaN/Inf, inverted `clamp` bounds, a read with no current value) are absorbed by **total eval** per the pinned semantics, with no per-tick logging.
- **Builder vocabulary** authored in both runtimes (TS + Luau), emitted into `postretro.d.ts` / `postretro.d.luau` so the **typedef is the contract** (`scripting.md` §11, the typedef-is-the-contract paragraph). Building a node is pure data assembly — no live binding, no eval, no FFI side effect.
- **Versioning** (folded here as a required section): a `u32` version stamp on a baked/serialized IR envelope, checked at **envelope load**, mirroring the persist format exactly (`CURRENT_STATE_VERSION` scheme); unsupported version → ignored with a warning. One versioning story shared with the state-store persist format and the deferred `setState` IR — not a parallel scheme.

### Out of scope

- **Any adopter wiring.** Movement, shields, UI `setState`, reactions, animation channels — all untouched. The first adopter (movement) is plan 3; consolidation of existing special-cases is plan 4 (demand-driven).
- **A non-logging per-tick write variant.** The store engine-write path (`write_store_slot`) logs + allocates when it clamps an out-of-range value. The write path here is exercised only by tests (not a hot per-tick writer), so this is acceptable; a non-logging, alloc-free per-tick write variant is the first *hot* write adopter's concern (plan 3+), not this plan's.
- **Boolean combinators** (`and` / `or` / `not`) and **vector values / vec ops**. The minimal set ships scalar+bool only. Boolean combinators are deferred but **near-certain at plan 4** (the movement `all`/`any` trigger vocabulary is a boolean proto-command-buffer consolidation absorbs). The plan-3 value path is unlikely to need them. They stay out of v1 (minimalism, `scripting.md` §11) but the node enum and type table must admit a combinator opcode **purely additively**. Vec ops likewise wait for an adopter with whole-vector intent.
- **Stateful / temporal nodes** (previous-value, integrators, wall-clock, prior ticks). Per-tick state arrives as scope *inputs* (e.g. movement's `elapsedMs`, `chargesRemaining`), never as evaluator-held state. No `while` / unbounded-loop node (`scripting.md` §11, the no-unbounded-loop paragraph).
- **Migrating any existing format.** The state-store persist format is unchanged; this plan only *matches its versioning shape* for the new IR envelope.
- **A binary/PRL section.** The IR crosses and bakes as JSON, like reactions and `state.json`.

## Type system

Two value types: `number` (`f32`) and `boolean`. Every node has a static result type; **bind** type-checks the whole tree once, **eval** never re-checks. Names are opaque strings to the scope (dotted like `player.health` is the store-scope convention; the movement scope uses bare names like `speed`).

| Opcode | Operands | Result |
|---|---|---|
| `const` | `value`: a number or boolean literal | the literal's type |
| `input` | `name` | the bound source's projected type (number or boolean), resolved at bind |
| `add` `sub` `mul` `div` | `a`, `b`: number | number |
| `clamp` | `x`, `lo`, `hi`: number | number |
| `lerp` | `a`, `b`, `t`: number | number |
| `lt` `le` `gt` `ge` | `a`, `b`: number | boolean |
| `eq` `ne` | `a`, `b`: `T`, `T` ∈ {number, boolean}, both same type | boolean |
| `select` | `cond`: boolean; `a`, `b`: `T`, same type | `T` |

A tree that violates a row — `clamp` over a boolean, a numeric `select` condition, mismatched `select`/`eq` arms, or an `input`/output bound to a store slot whose declared type is `String` / `Enum` / `Array` (no projection to number/boolean) — fails bind with a logged reason. Never a panic, never silent coercion.

**Store projection.** A store-backed scope projects `SlotValue::Number(f32)` ↔ `IrValue::Number` and `SlotValue::Boolean` ↔ `IrValue::Bool`; `String` / `Enum` / `Array` slots have no IR projection and fail bind when referenced.

**Total evaluation semantics** (eval is total — no panic, no divergence; the value-computing pass allocates nothing):

- A read resolving to **no current value** (a slot still `None`, e.g. an engine slot not yet written) yields the type's zero — `Number → 0.0`, `Bool → false`. The store scope reads the slot's `Option<SlotValue>` directly (no error path), so this allocates and logs nothing.
- `div` by zero → `0`.
- Any node whose arithmetic yields a non-finite result (`NaN`, `±Inf`) coerces that result to `0` (per-node finite guard).
- `clamp(x, lo, hi)` is `min(max(x, lo), hi)` — total for any bounds; inverted bounds (`lo > hi`) return the `hi` operand.
- `lerp(a, b, t) = a + (b - a) * t`, then finite-guarded.

## Acceptance criteria

- [ ] The minimal opcode set deserializes from JSON into a typed IR tree and re-serializes identically (round-trip), with `const`/`input` as struct variants and bare-scalar `value`.
- [ ] Every opcode in the vocabulary appears in generated `postretro.d.ts` and `postretro.d.luau` (snapshot test). The "an author cannot reference an opcode outside the vocabulary" half is a **typedef review gate** (closure of the emitted union), not a runtime test. Operand-type validity is enforced by Rust **bind**, the authority — the typedef guarantees vocabulary closure, not full operand typing.
- [ ] The same authored expression built in TypeScript and in Luau produces identical IR after **canonicalization**: deserialize each runtime's emitted JSON into the Rust `IrNode`, re-serialize through one canonical serializer, assert byte-identical.
- [ ] **Bind** rejects, without panicking, a tree that references an unknown input/output name, violates the Type system table, or is structurally malformed. The runnable assertion: each returns an `Err` carrying the reason, no panic, no process failure. (The single warning log is review-observable unless a log-capture harness is wired — do not gate the test on it.)
- [ ] The **eval pass** (reads + arithmetic producing the root value) is total per the pinned semantics — missing-value read → type-zero, ÷0 → 0, non-finite → 0, inverted `clamp` returns the `hi` operand — each a defined finite result, no panic; and performs **zero heap allocations**, verified under the counting-allocator harness Task 2 builds, over a tree containing at least one of every opcode nested at least two levels deep (the counter scoped to the eval pass, bind excluded).
- [ ] The **same IR tree** binds against two different scopes — a store-backed scope and a movement-like stub scope exposing a movement-local input set — and reads each scope's values, proving the namespace is pluggable, not store-hardwired.
- [ ] **Write path (stub):** an IR envelope with a named output, bound to a read+write stub scope, writes the root's evaluated value to that output. An envelope targeting an output the scope does **not** grant fails to bind (degrades with a logged reason), proving writability is a bind-time scope capability.
- [ ] **Write path (store), both modes:** an engine-capability store scope writes into an engine-owned (readonly) slot via the validated engine write-by-name path; a script-capability store scope denies a readonly slot's write handle at bind and writes only writable slots.
- [ ] A baked IR envelope carries a `u32` version stamp. Runnable assertions: a current-version envelope round-trips; an unsupported-version envelope is ignored and the adopter falls back, no panic. (The warning is review-observable, not gated on.)
- [ ] Movement, shields, UI, reactions, and animation-channel code are unchanged — a **grep/diff review gate**, not a runnable test.

## Tasks

### Task 1: IR node model, value type, wire format, bind seam

Define the closed opcode tree as a serde **internally-tagged** enum (`#[serde(tag = "op", rename_all = "snake_case")]`) using **struct variants** — internally-tagged serde cannot represent a newtype variant carrying a primitive, so `const`/`input` are `Const { value: IrValue }` / `Input { name: String }`, and inner nodes carry named `Box<IrNode>` operands (per the Type system field names). `IrValue` is `#[serde(untagged)]` (`Number(f32)` | `Bool(bool)`) so it emits a bare JSON scalar. Define the **`BindingScope` trait** here — it is the seam bind validates against: bind-time `resolve_input(name) -> Option<InputHandle>` / `resolve_output(name) -> Option<OutputHandle>`, eval-time `read(InputHandle) -> IrValue` / `write(OutputHandle, IrValue)`; the handle types are scope-defined (a store scope's handle carries the owned name, a movement scope's an index). Implement the **bind** pass over `BakedIr` (it owns the optional output and root): type-check the tree against the Type system table, resolve each named input and output against the scope, produce an eval-ready `BoundProgram` holding handles, not strings. Bind returns `Result` — every structural / type / name / projection fault becomes a single logged error and a caller fallback, never a panic (version is validated earlier, at load — Task 4). Owns the Boundary Inventory and the envelope shape (`BakedIr { version, output: Option<String>, root }`).

**Typing and wire field names — restated here because task agents do not see the Type system section, and Task 3 must match these byte-for-byte:** `const {value: number|bool}` → literal's type; `input {name}` → bound source's projected type; `add`/`sub`/`mul`/`div` `{a, b: number}` → number; `clamp {x, lo, hi: number}` → number; `lerp {a, b, t: number}` → number; `lt`/`le`/`gt`/`ge` `{a, b: number}` → boolean; `eq`/`ne` `{a, b: T}` (`T` ∈ {number, boolean}, both same) → boolean; `select {cond: boolean, a, b: T}` (same type) → `T`. Bind rejects any row violation — plus an input/output projecting from a `String`/`Enum`/`Array` store slot — with a logged `Err`. Store projection: `SlotValue::Number ↔ IrValue::Number`, `Boolean ↔ Bool`; a `None`/absent read yields type-zero (`0.0`/`false`).

### Task 2: Evaluator + concrete scopes

Implement the **eval** pass over a `BoundProgram`: pure, total, bounded; the value-computing pass reads inputs through pre-resolved handles and allocates nothing. Eval follows the pinned total semantics (missing → type-zero, ÷0 → 0, non-finite → 0, `clamp = min(max(x,lo),hi)`, finite-guarded `lerp`). **Writability is a bind-time scope capability:** `resolve_output` grants a handle only for outputs the scope permits, so a forbidden output fails to bind rather than being policed each tick. Ship two concrete scopes:

1. A **store-backed adapter** holding a `ScriptCtx` clone (cheap; it owns the `Rc<RefCell<SlotTable>>` and is the receiver the store read/write APIs already take). **Read:** the handle is the validated owned dotted-name captured at bind; eval reads the slot's current `Option<SlotValue>` directly via `SlotTable::get(&str)` (alloc-free re-hash, no new store API), mapping `None`/absent → type-zero. **Write:** parameterized by capability mode — **engine mode** (engine-policy IR) grants a handle for any slot and delegates to `write_store_slot` (bypasses readonly, validates/clamps); **script mode** (the deferred UI `setState`) grants a handle only for non-readonly slots, denying readonly at bind. The two modes mirror the existing `write_store_slot` (engine-bypass) vs script-gated write split.
2. A read+write **test stub scope** with a fixed input/output set, exercising both paths. The stub exposes **indexed** handles (distinct from the store scope's owned-name handles) so AC #6 proves the namespace is pluggable, not store-shaped.

This task also **builds the zero-alloc test harness** (it is owned nowhere else): a safe counting global allocator — a `GlobalAlloc` that delegates to `System` and increments atomic counters — installed `#[global_allocator]` in the test binary, asserting zero allocations across the **eval pass only** (the counter is armed after bind, disarmed before any write). The only `unsafe` is the allocator trait's required `alloc`/`dealloc` delegation; this needs the `development_guide.md` §3.5 unsafe-gate nod (see Open questions).

No real adopter consumes the evaluator in this plan.

### Task 3: Builder vocabulary + typedef emission + dual-runtime parity

Emit the IR vocabulary into `postretro.d.ts` / `postretro.d.luau` via `typedef.rs` (`generate_typescript` / `generate_luau`) — the typedef is the contract. **The current `register_tagged_union` / `TypeShape::TaggedUnion` builder cannot express this union** (it renders `({ tag } & Ty)` with one payload *type name* per variant and a fixed tag key; it has no per-variant struct fields and no recursive self-reference). So this task includes the named emitter work: (1) register each opcode payload as a struct type via `register_type` — `IrConst { value }`, `IrAdd { a, b }`, `IrClamp { x, lo, hi }`, `IrLerp { a, b, t }`, `IrSelect { cond, a, b }`, comparison/arith structs — with operands typed `IrNode` (recursive); (2) extend the tagged-union path to accept a configurable tag key (`"op"`) and per-variant *registered-struct* payloads, emitting `({ op: "add" } & IrAdd) | …`; if extending the emitter proves heavier than a thin static block, fall back to appending the `IrNode` union alias to the SDK static type block (the `SequenceStep` static-block precedent) — either way the operand field names match Task 1's restated list byte-for-byte. The builders themselves are **pure `sdk/lib` constructors** in each runtime, typed against the emitted union: constructing a node is data assembly returned through the normal `setupMod` / `setupLevel` path, **not** an FFI primitive call (`scripting.md` §12 — no side-effect FFI from imports). TS/Luau types guard at author time, Rust serde + bind validation is the authority. Add the **parity contract test**: author the same expression in TS and in Luau, capture each runtime's emitted IR by returning JSON through `run_script` / `run_source` + the `conv` bridge (no new collection sink), canonicalize Rust-side (per the AC), assert byte-identical.

### Task 4: Versioned IR envelope

Finish the `BakedIr` envelope (Task 1 defines its shape) with a `u32` version field plus `CURRENT_IR_VERSION` under the *"increment only with a defined migration path"* discipline (mirroring `state_persistence.rs`). The **load-time** version check mirrors the persist loader (`state_persistence.rs:109` — unsupported version ignored with a warning, the adopter falls back); this is the sole version seam — bind assumes an already-version-validated tree. Document the opcode-vocabulary evolution rule: adding an opcode is additive (new variant, no bump); removing or changing one is breaking and gated by a version bump + migration. State explicitly that this is the *same* versioning story as the state-store persist format and the deferred `setState` IR.

### Task 5: End-to-end + round-trip test harness (the test gate)

The proof the substrate composes: author an expression via the builders in each runtime, capture the emitted IR JSON by returning it through `run_script` / `run_source` + the `conv` bridge (the same crossing path Task 3's parity test uses — no new sink), deserialize → bind against the stub scope → evaluate → assert the expected value; plus the wire-format round-trip and the version-stamp round-trip/rejection. Consumes Tasks 2–4.

## Sequencing

**Phase 1 (sequential):** Task 1 — node model, `IrValue`, `BindingScope` trait, bind seam, envelope shape; everything else builds on it.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4 — evaluator + concrete scopes, builders/typedef, and the versioned envelope are independent surfaces over Task 1 (different modules: evaluator vs. `sdk/lib` + `typedef.rs` vs. the envelope serde).
**Phase 3 (sequential):** Task 5 — end-to-end harness consumes the evaluator (2), builders (3), and envelope (4).

## Rough sketch

- **Node tree.** `IrNode` internally-tagged enum, struct variants: `Const { value: IrValue }` / `Input { name: String }` / `Add { a, b }` (… `Sub`/`Mul`/`Div`/`Lt`/`Le`/`Gt`/`Ge`/`Eq`/`Ne`) / `Clamp { x, lo, hi }` / `Lerp { a, b, t }` / `Select { cond, a, b }`, operands `Box<IrNode>`. `IrValue` is `#[serde(untagged)]`. Tagged-union decode mirrors `registry.rs::ComponentValue` (`tag = "kind"`, snake_case); the `serde_json::Value` JSON-payload precedent is `data_descriptors.rs::PrimitiveDescriptor.args`.
- **Bind → program.** `bind(&BakedIr, &impl BindingScope) -> Result<BoundProgram, BindError>`. `BoundProgram` is the flattened, name-resolved, eval-ready form (leaves hold scope handles, not strings). On `Err`, the adopter logs once and uses its native fallback (movement → native intent; a slot writer → no-op) — the persist loader's "warn and defaults stand" shape (`state_persistence.rs`).
- **Scope.** `BindingScope` trait (defined in Task 1; read + write, both optional-capability; scope-defined handle types). Store adapter holds a `ScriptCtx` clone; reads via `SlotTable::get(&str)` (None → type-zero), writes via `write_store_slot` (engine mode) or the script-gated path (script mode). Movement's scope (plan 3) exposes `speed` / `grounded` / `chargesRemaining` / … from the movement component as indexed inputs, **without** routing through `worldQuery` — the engine-internal invariant (`entity_model.md` §7b) holds because the scope is an engine-side binding, not the script-facing slot table.
- **Versioning.** `BakedIr { version: u32, output: Option<String>, root: IrNode }`, `const CURRENT_IR_VERSION: u32 = 1`, serde_json — a direct mirror of `PersistedState` / `CURRENT_STATE_VERSION` (`state_persistence.rs`); version checked at load, not bind.
- **Typedef.** Register the `IrNode` union through the type registry so `typedef.rs::generate_typescript` / `generate_luau` emit it — the typedef is the limit-of-vocabulary documentation by construction.

## Boundary inventory

| Name | Rust | Wire / serde | TS builder | Luau builder |
|---|---|---|---|---|
| node discriminant | `IrNode` struct variant | `"op"` tag (`"clamp"`, `"select"`, `"lt"`, …) | builder fn per op (`clamp`, `select`, `lt`, `add`, …) | same names, identical IR |
| constant leaf | `Const { value: IrValue }` | `{ "op": "const", "value": <number\|bool> }` | numeric/boolean literal → `const` node | same |
| named input | `Input { name: String }` | `{ "op": "input", "name": "speed" }` | `input("speed")` | `input("speed")` |
| binary op | `Add { a, b }` (etc.) | `{ "op": "add", "a": <node>, "b": <node> }` | `add(x, y)` | same |
| clamp / lerp / select | `Clamp { x, lo, hi }` / `Lerp { a, b, t }` / `Select { cond, a, b }` | `{ "op": "clamp", "x":…, "lo":…, "hi":… }`, etc. | `clamp(x, lo, hi)`, `lerp(a, b, t)`, `select(c, a, b)` | same |
| write target | `BakedIr.output: Option<String>` | envelope field `"output": "player.shield"` (omitted = read-only) | envelope output field | same |
| value model | `IrValue::{Number(f32),Bool(bool)}`, untagged | bare JSON number / bool | `number` / `boolean` | `number` / `boolean` |
| version stamp | `BakedIr.version: u32` | `{ "version": 1, "output": …, "root": … }` | n/a (engine-baked) | n/a |

Names follow the slot store's dotted convention (`player.health`); numbers are `f32` in-engine, JSON-number on the wire (the persist format's `f64`→`f32` narrowing precedent applies if a baked IR is ever read back as `f64`).

## Wire format

No binary/PRL section. The IR crosses the FFI as JSON (serde, the reaction/descriptor precedent) and bakes as JSON in the versioned envelope (serde_json, the `state.json` precedent). Pin: internally-tagged on `"op"` with struct-variant payloads; operand field names per the Boundary inventory; `const` carries a bare-scalar `"value"`; numbers as JSON numbers; the envelope is `{ "version": <u32>, "output"?: <name>, "root": <node> }` mirroring `PersistedState` and extending it with the optional write target. `output` absent ⇒ a read-only (value-producing) buffer. An unsupported `version` is ignored-with-warning at load, never an error.

## Decisions (resolved by project principles)

Settled by zooming out to the engine's core rule — *declare as data, Rust evaluates, the VM drops; the IR is not special, it is the consolidation of the existing substrate* (`scripting.md` §11).

- **Builder mechanism → pure SDK constructors**, not registry primitives. Constructing a node has no runtime effect, so an FFI primitive call would violate the no-side-effect-import rule (§12) for no gain. The union type in the typedef carries the contract.
- **Write-path granularity → bind-time scope capability.** Writability is decided when a scope grants/denies a write handle in `resolve_output`. The store scope's **capability mode** (engine vs script) maps directly onto the engine's existing `write_store_slot`-bypass vs script-gated split — engine-policy writes (shield recharge) use engine mode, the deferred UI `setState` uses script mode — so the readonly-deny rule applies to script mode without foreclosing engine writes.
- **Write target → envelope field, not a node**, so every node stays value-producing (no statement/expression split).
- **Dual-runtime parity → canonicalize Rust-side**, sidestepping cross-runtime float/key-order differences.

## Open questions

- **Typedef-emitter extension scope (Task 3).** The struct-variant-with-recursion union is not expressible by today's `register_tagged_union`. Task 3 names the work (extend the emitter for a configurable tag key + registered-struct payloads, or a static-block union alias). Which path is cheaper is an implementation call to make against `typedef.rs` when the task lands; the contract (the emitted shape + byte-matching field names) is fixed either way.
- **Counting-allocator unsafe gate.** The zero-alloc harness (Task 2) implements `GlobalAlloc` (delegating to `System`), whose trait methods are `unsafe fn`. Per `CLAUDE.md` / `development_guide.md` §3.5 this needs an explicit unsafe-approval nod before implementation. If withheld, AC #5's zero-alloc clause degrades to a by-construction/review guarantee.
- **Compound conditions — empirical, designed-for.** Plan 3 (movement value expressions) likely needs only single-comparison `select` conditions; `and`/`or`/`not` are deferred from v1 but near-certain by plan 4 (trigger-predicate consolidation). Decision taken: keep them out of v1, keep the node enum and type table open to an additive combinator opcode. The only thing left to confirm is empirical — that plan 3's real expressions don't pull them forward.
