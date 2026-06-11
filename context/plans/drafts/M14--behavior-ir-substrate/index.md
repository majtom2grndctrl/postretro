# M14 Behavior IR — Substrate + Evaluator

## Goal

Realize the Typed Command Buffer (`scripting.md` §11): authored behavior that depends on live state crosses the FFI as a **typed, serializable IR tree**, the VM drops, and a Rust **total evaluator** binds the named leaves to live state and evaluates each tick. This plan ships the substrate — the opcode vocabulary, the wire format, the pure/total/bounded evaluator, the **scoped binding abstraction**, and the **version stamp** — with no behavior adopting it yet. It is the foundation the rest of Milestone 14 rides.

This is **plan 1 of a sequential chain** (1 substrate → 2 versioning [folded here] → 3 first adopter: movement → 4 consolidation, demand-driven). There is no concurrent milestone wave; each plan consumes the prior one's settled output.

## Scope

### In scope

- A closed-vocabulary **IR node tree** — discriminated-union-per-opcode (one `op`-tagged variant per node, the `registry.rs::ComponentValue` tag+payload precedent), serde, crossing the FFI as JSON data and returned through `setupMod` / `setupLevel` like reactions (`{name, JSON args}`, `scripting.md` §11). All v1 opcode tags and field names are single lowercase words, so no camel/snake ambiguity arises; the convention for any future multi-word name is settled when one is added (the engine uses snake_case tags for component unions, camelCase for descriptor fields).
- The **minimal node set** (`scripting.md` §11, the minimal-node-set paragraph): constant + named-input **leaves**; arithmetic (`add` / `sub` / `mul` / `div`); `clamp`; `lerp`; `select(cond, a, b)`; comparisons (`lt` / `le` / `gt` / `ge` / `eq` / `ne`). A closed two-type value model — number (`f32`) and boolean — with operand/result types pinned in the Type system section.
- A two-phase evaluator: **bind** (once — type-check the tree against the Type system table, resolve every named leaf and output to a scope-provided handle, produce an eval-ready program) and **eval** (per tick — pure, total, bounded, **zero per-eval heap allocation**; names already resolved to scope handles at bind, so no name re-parsing).
- A **scoped binding abstraction**: the evaluator binds names through a pluggable scope, *not* a hardwired global namespace. The mod state store is one scope (reads/writes slots by dotted name); a movement-local scope (plan 3) is another. A scope may be read-only or read+write.
- Both a **read** path (input leaves → values) and a **named-output write** path (the root's evaluated value → a named output the scope writes). Both shipped and unit-tested against a stub scope; **only the read path has a real consumer later** (movement). The write path is the home for the deferred UI `setState` IR and engine policy writes (shield recharge) — designed and tested, not wired.
- **Degrade visibly, never panic** (the UI unknown-token rule applied here), split two ways: structural problems caught once at **bind** (unknown leaf/output name, type-table violation, an input/output bound to a store slot whose declared type does not project to number/boolean, malformed tree, unsupported version) return an error the caller logs once and falls back from; numeric edge cases (÷0, NaN/Inf, inverted `clamp` bounds) are absorbed by **total arithmetic** at eval per the pinned semantics, with no per-tick logging.
- **Builder vocabulary** authored in both runtimes (TS + Luau), emitted into `postretro.d.ts` / `postretro.d.luau` so the **typedef is the contract** (`scripting.md` §11, the typedef-is-the-contract paragraph). Building a node is pure data assembly — no live binding, no eval, no FFI side effect.
- **Versioning** (folded here as a required section): a `u32` version stamp on a baked/serialized IR envelope, mirroring the persist format exactly (`CURRENT_STATE_VERSION` scheme); unsupported version → ignored with a warning. One versioning story shared with the state-store persist format and the deferred `setState` IR — not a parallel scheme.

### Out of scope

- **Any adopter wiring.** Movement, shields, UI `setState`, reactions, animation channels — all untouched. The first adopter (movement) is plan 3; consolidation of existing special-cases is plan 4 (demand-driven).
- **Boolean combinators** (`and` / `or` / `not`) and **vector values / vec ops**. The minimal set ships scalar+bool only. Boolean combinators are deferred but **near-certain at plan 4**: the movement *trigger* vocabulary (`all` / `any` over predicates, `movement.md` §2) is itself a boolean proto-command-buffer, and consolidation pulls those onto this substrate. The plan-3 value path (`boost = f(...)`, evaluated only while a state is already active) is unlikely to need them — transition logic clusters in the trigger layer, not the value layer. So they stay out of v1 (minimalism, `scripting.md` §11) but the node enum and type table must admit a combinator opcode **purely additively**, no restructuring. Vec ops likewise wait for an adopter with whole-vector intent.
- **Stateful / temporal nodes** (previous-value, integrators, anything reading wall-clock or prior ticks). Per-tick state arrives as scope *inputs* (e.g. movement's `elapsedMs`, `chargesRemaining`), never as evaluator-held state. No `while` / unbounded-loop node — a request for one is the signal to reject (`scripting.md` §11, the no-unbounded-loop paragraph).
- **Migrating any existing format.** The state-store persist format is unchanged; this plan only *matches its versioning shape* for the new IR envelope.
- **A binary/PRL section.** The IR crosses and bakes as JSON, like reactions and `state.json`.

## Type system

Two value types: `number` (`f32`) and `boolean`. Every node has a static result type; **bind** type-checks the whole tree once, **eval** never re-checks.

| Opcode | Operands | Result |
|---|---|---|
| `const` | carries a number or boolean literal | the literal's type |
| `input` | dotted name | the bound source's projected type (number or boolean), resolved at bind |
| `add` `sub` `mul` `div` | (number, number) | number |
| `clamp` | (number `x`, number `lo`, number `hi`) | number |
| `lerp` | (number `a`, number `b`, number `t`) | number |
| `lt` `le` `gt` `ge` | (number, number) | boolean |
| `eq` `ne` | (`T`, `T`), `T` ∈ {number, boolean}, both operands same type | boolean |
| `select` | (boolean `cond`, `T` `a`, `T` `b`), `a` and `b` same type | `T` |

A tree that violates a row — `clamp` over a boolean, a numeric `select` condition, mismatched `select`/`eq` arms, or an `input`/output bound to a store slot whose declared type is `String` / `Enum` / `Array` (no projection to number/boolean) — fails bind with a logged reason. Never a panic, never silent coercion.

**Store projection.** A store-backed scope projects `SlotValue::Number(f32)` ↔ `IrValue::Number` and `SlotValue::Boolean` ↔ `IrValue::Bool`; `String` / `Enum` / `Array` slots have no IR projection and fail bind when referenced.

**Total evaluation semantics** (eval is total — no panic, no divergence, no per-eval allocation):

- `div` by zero → `0`.
- Any node whose arithmetic yields a non-finite result (`NaN`, `±Inf`) coerces that result to `0` (per-node finite guard).
- `clamp(x, a, b)` is defined as `min(max(x, a), b)` — total for any `a, b`; inverted bounds (`a > b`) return `b`.
- `lerp(a, b, t) = a + (b - a) * t`, then finite-guarded.

## Acceptance criteria

- [ ] The minimal opcode set deserializes from JSON into a typed IR tree and re-serializes identically (round-trip).
- [ ] Every opcode in the vocabulary appears in generated `postretro.d.ts` and `postretro.d.luau`; an author cannot reference an opcode outside the vocabulary. (Operand-type validity is enforced by Rust **bind**, the authority — the author-time typedef guarantees vocabulary closure, not full operand typing.)
- [ ] The same authored expression built in TypeScript and in Luau produces identical IR after **canonicalization**: deserialize each runtime's emitted JSON into the Rust `IrNode`, re-serialize through one canonical serializer, assert byte-identical (the contract is structural identity, not raw cross-runtime string identity).
- [ ] **Bind** rejects, without panicking, a tree that: references an unknown input or output name; violates the Type system table (mistyped operands, mismatched arms, an input/output projecting from an unrepresentable store-slot type); or is structurally malformed. Each returns an error carrying the reason; the substrate logs it once; no process failure.
- [ ] **Eval** is total per the pinned semantics: ÷0 → 0, non-finite → 0, inverted `clamp` bounds return the upper bound — each yields a defined finite result with no panic (per-opcode adversarial unit tests).
- [ ] Eval performs **zero heap allocations** per evaluation, verified under a counting allocator over a tree containing at least one of every opcode nested at least two levels deep.
- [ ] The **same IR tree** binds against two different scopes — a store-backed scope and a movement-like stub scope exposing a movement-local input set — and reads each scope's values, proving the namespace is pluggable, not store-hardwired.
- [ ] **Write path (stub):** an IR envelope with a named output, bound to a read+write stub scope, writes the root's evaluated value to that output. An envelope targeting an output the scope does **not** grant fails to bind (degrades with a logged reason), proving writability is a bind-time scope capability.
- [ ] **Write path (store):** a store-backed scope writes an evaluated value into a writable slot via the validated engine write-by-name path (value lands clamped/validated); a readonly slot denies the write handle at bind.
- [ ] A baked IR envelope carries a `u32` version stamp using the same scheme as the persist format; loading an envelope whose version is unsupported is ignored with a warning and does not panic (round-trip + bumped-version-rejection tests).
- [ ] Movement, shields, UI, reactions, and animation-channel code are unchanged (no adopter wiring).

## Tasks

### Task 1: IR node model, value type, wire format, bind seam

Define the closed opcode tree as a serde discriminated union (one variant per opcode, `op` tag), `IrValue { Number(f32), Bool(bool) }` with `Const` carrying an `IrValue` (so one `const` op serves both literal types), and the deserialize path from the JSON the builders emit. Define the **`BindingScope` trait** here — it is the seam bind validates against, so it lands with bind, not Task 2: bind-time `resolve_input(name) -> Option<InputHandle>` / `resolve_output(name) -> Option<OutputHandle>`, eval-time `read(InputHandle) -> IrValue` / `write(OutputHandle, IrValue)`. Implement the **bind** pass: type-check the tree against the Type system table (every node has a static result type; well-typedness is decidable), resolve each named-input and named-output leaf against the scope, and produce an eval-ready `IrProgram` whose leaves hold handles, not strings. Bind returns `Result` — every structural / type / name / projection / version fault becomes a single logged error and a caller fallback, never a panic. Owns the Boundary Inventory and the envelope shape (`BakedIr { version, output: Option<String>, root }`).

### Task 2: Evaluator + concrete scopes

Implement the **eval** pass over an `IrProgram` (Task 1 owns the trait and program shape): pure, total, bounded, **zero per-eval allocation**, reads inputs through pre-resolved handles (no name re-parsing). Arithmetic follows the pinned total semantics (÷0 → 0, non-finite → 0, `clamp = min(max(x,a),b)`, finite-guarded `lerp`). **Writability is a bind-time scope capability:** `resolve_output` grants a handle only for outputs the scope permits, so a forbidden output fails to bind rather than being policed each tick; declared validate/clamp always applies at the write itself, independent of writability. Ship two concrete scopes: (1) a **store-backed adapter** holding the live store handle (`Rc<RefCell<SlotTable>>`) across the bind+eval window — `resolve_input`/`resolve_output` resolve a dotted name once to a slot handle, checking the `SlotValue`→`IrValue` projection and (for outputs) declared writability; eval-time `read` reads the current value and `write` delegates to the store's validated engine write-by-name API; no per-eval heap allocation; (2) a read+write **test stub scope** exercising both paths with a fixed input/output set. No real adopter consumes the evaluator in this plan.

### Task 3: Builder vocabulary + typedef emission + dual-runtime parity

Register the `IrNode` union through the type registry (`register_tagged_union` precedent, `primitives_registry.rs`) so the vocabulary emits into `postretro.d.ts` / `postretro.d.luau` via `typedef.rs` (`generate_typescript` / `generate_luau`) — the typedef is the contract. The builders themselves are **pure `sdk/lib` constructors** in each runtime, typed against that emitted union: constructing a node is data assembly returned through the normal `setupMod` / `setupLevel` path, **not** an FFI primitive call (`scripting.md` §12 — no side-effect FFI from imports; building a node binds nothing and evaluates nothing). This mirrors descriptor authoring: TS/Luau types guard at author time, Rust serde + bind validation is the authority. Add the **parity contract test**: the same authored expression in TS and Luau, canonicalized Rust-side (per the AC), is byte-identical.

### Task 4: Versioned IR envelope

Finish the `BakedIr` envelope (Task 1 defines its shape) with a `u32` version field plus `CURRENT_IR_VERSION` under the *"increment only with a defined migration path"* discipline (mirroring `state_persistence.rs`). Load-time version check mirrors the persist loader: an unsupported version is ignored with a warning, the adopter falls back. Document the opcode-vocabulary evolution rule: adding an opcode is additive (new variant, no bump); removing or changing one is breaking and gated by a version bump + migration. State explicitly that this is the *same* versioning story as the state-store persist format and the deferred `setState` IR.

### Task 5: End-to-end + round-trip test harness (the test gate)

The proof the substrate composes: author an expression via the builders in each runtime → cross the FFI as JSON → deserialize → bind against the stub scope → evaluate → assert the expected value; plus the wire-format round-trip and the version-stamp round-trip/rejection. Consumes Tasks 2–4.

## Sequencing

**Phase 1 (sequential):** Task 1 — node model, `IrValue`, `BindingScope` trait, bind seam, envelope shape; everything else builds on it.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4 — evaluator + concrete scopes, builders/typedef, and the versioned envelope are independent surfaces over Task 1 (different modules: evaluator vs. `sdk/lib` + `typedef.rs` vs. the envelope serde).
**Phase 3 (sequential):** Task 5 — end-to-end harness consumes the evaluator (2), builders (3), and envelope (4).

## Rough sketch

- **Node tree.** `IrNode` serde enum, `#[serde(tag = "op", rename_all = "snake_case")]` (single-word ops, so the rename is moot but consistent with `registry.rs::ComponentValue`'s `tag = "kind"` pattern). Variants `Const(IrValue)` / `Input(String)` / `Add` / `Sub` / `Mul` / `Div` / `Clamp` / `Lerp` / `Select` / `Lt` / `Le` / `Gt` / `Ge` / `Eq` / `Ne`. `Input` carries a dotted name; the write target is **not** a node — it is an `Option<String>` on the envelope. Tagged-union decode mirrors `registry.rs::ComponentValue`; the `serde_json::Value` JSON-payload precedent is `data_descriptors.rs::PrimitiveDescriptor.args`.
- **Value + types.** `IrValue { Number(f32), Bool(bool) }`. Static result type per opcode per the Type system table; bind validates the type graph, eval never re-checks.
- **Bind → program.** `bind(&IrNode, &impl BindingScope) -> Result<IrProgram, BindError>`. `IrProgram` is the flattened, name-resolved, eval-ready form (leaves hold `InputHandle` / `OutputHandle`, not strings) so eval is alloc- and re-parse-free. On `Err`, the adopter logs once and uses its native fallback (movement → native intent; a slot writer → no-op) — the persist loader's "warn and defaults stand" shape (`state_persistence.rs`).
- **Scope.** `BindingScope` trait (defined in Task 1; read + write, both optional-capability). Writability lives in `resolve_output` — granting a handle *is* the authorization; a denied output fails bind. The store adapter holds `Rc<RefCell<SlotTable>>`, resolves dotted names once at bind, and delegates writes to `write_store_slot` (validated/clamped). Movement's scope (plan 3) exposes `speed` / `grounded` / `chargesRemaining` / … from the movement component **without** routing through `worldQuery` — the engine-internal invariant (`entity_model.md` §7b) holds because the scope is an engine-side binding, not the script-facing slot table.
- **Versioning.** `BakedIr { version: u32, output: Option<String>, root: IrNode }`, `const CURRENT_IR_VERSION: u32 = 1`, serde_json — a direct mirror of `PersistedState` / `CURRENT_STATE_VERSION` (`state_persistence.rs`). Do not invent a second scheme.
- **Typedef.** Register the `IrNode` union through the type registry so `typedef.rs::generate_typescript` / `generate_luau` emit it — the typedef is the limit-of-vocabulary documentation by construction.

## Boundary inventory

| Name | Rust | Wire / serde | TS builder | Luau builder |
|---|---|---|---|---|
| node discriminant | `IrNode` variant | `"op"` tag (`"clamp"`, `"select"`, `"lt"`, …) | builder fn per op (`clamp`, `select`, `lt`, `add`, …) | same names, identical IR |
| constant leaf | `IrNode::Const(IrValue)` | `{ "op": "const", "value": <number\|bool> }` | numeric/boolean literal → `const` node | same |
| named input | `IrNode::Input(String)` | `{ "op": "input", "name": "speed" }` | `input("speed")` | `input("speed")` |
| write target | `BakedIr.output: Option<String>` | envelope field `"output": "player.shield"` (omitted = read-only) | envelope output field | same |
| value model | `IrValue::{Number(f32),Bool(bool)}` | JSON number / bool | `number` / `boolean` | `number` / `boolean` |
| version stamp | `BakedIr.version: u32` | `{ "version": 1, "output": …, "root": … }` | n/a (engine-baked) | n/a |

Naming follows the slot store's dotted-name convention (`player.health`); numbers are `f32` in-engine, JSON-number on the wire (the persist format's `f64`→`f32` narrowing precedent applies if a baked IR is ever read back as `f64`).

## Wire format

No binary/PRL section. The IR crosses the FFI as JSON (serde, the reaction/descriptor precedent) and bakes as JSON in the versioned envelope (serde_json, the `state.json` precedent). Pin: discriminated union on `"op"`; constants carry a `"value"`; numbers as JSON numbers; the envelope is `{ "version": <u32>, "output"?: <dotted-name>, "root": <node> }` mirroring `PersistedState` and extending it with the optional write target. `output` absent ⇒ a read-only (value-producing) buffer. An unsupported `version` is ignored-with-warning, never an error.

## Decisions (resolved by project principles)

These were settled by zooming out to the engine's core rule — *declare as data, Rust evaluates, the VM drops; the IR is not special, it is the consolidation of the existing substrate* (`scripting.md` §11).

- **Builder mechanism → pure SDK constructors.** Opcode builders are plain `sdk/lib` constructors typed against the registry-emitted `IrNode` union, not registry primitives. Constructing a node has no runtime effect, so an FFI primitive call would violate the no-side-effect-import rule (§12) for no gain. The union type in the typedef carries the contract; the constructors author against it (the descriptor precedent).
- **Write-path granularity → bind-time scope capability.** Writability is decided when a scope grants (or denies) a write handle in `resolve_output`, not by an evaluator privilege level; declared validate/clamp always applies at the write. This unifies engine-policy writes (shield recharge) and the deferred UI `setState` under one mechanism — each binds against a scope that exposes exactly the outputs it permits — and removes the "first adopter decides" punt.
- **Write target → envelope field, not a node.** A node always produces a value; the write sink is an `Option<String>` on the `BakedIr` envelope, so "every node has a static result type" holds and no statement-vs-expression split is introduced.
- **Dual-runtime parity → canonicalize Rust-side.** Parity is asserted on the deserialized-and-re-serialized `IrNode`, not on raw runtime-emitted strings, sidestepping cross-runtime float/key-order differences.

## Open questions

- **Compound conditions — empirical, designed-for.** Plan 3 (movement value expressions) likely needs only single-comparison `select` conditions; `and`/`or`/`not` are deferred from v1 but near-certain by plan 4 (trigger-predicate consolidation). Decision already taken: keep them out of v1, keep the node enum and type table open to an additive combinator opcode (see non-goals). The only thing left to confirm is empirical — that plan 3's real expressions don't pull them forward.
