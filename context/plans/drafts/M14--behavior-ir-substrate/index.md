# M14 Behavior IR — Substrate + Evaluator

## Goal

Realize the Typed Command Buffer (`scripting.md` §11): authored behavior that depends on live state crosses the FFI as a **typed, serializable IR tree**, the VM drops, and a Rust **total evaluator** binds the named leaves to live state and evaluates each tick. This plan ships the substrate — the opcode vocabulary, the wire format, the pure/total/bounded evaluator, the **scoped binding abstraction**, and the **version stamp** — with no behavior adopting it yet. It is the foundation the rest of Milestone 14 rides.

This is **plan 1 of a sequential chain** (1 substrate → 2 versioning [folded here] → 3 first adopter: movement → 4 consolidation, demand-driven). There is no concurrent milestone wave; each plan consumes the prior one's settled output.

## Scope

### In scope

- A closed-vocabulary **IR node tree** — discriminated-union-per-opcode, serde, camelCase wire — crossing the FFI as JSON data (the reaction `{name, JSON args}` and descriptor precedents).
- The **minimal node set** (`scripting.md` §253): constant + named-input **leaves**; arithmetic (`add` / `sub` / `mul` / `div`); `clamp`; `lerp`; `select(cond, a, b)`; comparisons (`lt` / `le` / `gt` / `ge` / `eq` / `ne`). A closed two-type value model: number (`f32`) and boolean.
- A two-phase evaluator: **bind** (once — validate well-typedness, resolve every named leaf and output to a scope-provided handle, produce an eval-ready program) and **eval** (per tick — pure, total, bounded, **zero heap allocation**, no string lookups).
- A **scoped binding abstraction**: the evaluator binds names through a pluggable scope, *not* a hardwired global namespace. The mod state store is one scope (reads/writes slots by dotted name); a movement-local scope (plan 3) is another. A scope may be read-only or read+write.
- Both a **read** path (input leaves → values) and a **named-output write** path (evaluated value → a named output the scope writes). Both shipped and unit-tested against a stub scope; **only the read path has a real consumer later** (movement). The write path is the home for the deferred UI `setState` IR and engine policy writes (shield recharge) — designed and tested, not wired.
- **Degrade visibly, never panic** (the UI unknown-token rule applied here), split two ways: structural problems caught once at **bind** (unknown leaf/output name, type mismatch, malformed tree, unsupported version) return an error the caller logs once and falls back from; numeric edge cases (÷0, NaN/Inf, inverted `clamp` bounds) are absorbed by **total arithmetic** at eval with no per-tick logging.
- **Builder vocabulary** authored in both runtimes (TS + Luau), emitted into `postretro.d.ts` / `postretro.d.luau` so the **typedef is the contract** (`scripting.md` §255). Building a node is pure data assembly — no live binding, no eval, no FFI side effect.
- **Versioning** (the §2-of-the-milestone obligation, folded here as a required section): a `u32` version stamp on a baked/serialized IR envelope, mirroring the persist format exactly (`CURRENT_STATE_VERSION` scheme); unsupported version → ignored with a warning. One versioning story shared with the state-store persist format and the deferred `setState` IR — not a parallel scheme.

### Out of scope

- **Any adopter wiring.** Movement, shields, UI `setState`, reactions, animation channels — all untouched. The first adopter (movement) is plan 3; consolidation of existing special-cases is plan 4 (demand-driven).
- **Boolean combinators** (`and` / `or` / `not`) and **vector values / vec ops**. The minimal set ships scalar+bool only; these are the most likely first additions, added deliberately when an adopter demands them (`scripting.md` §253), not speculatively.
- **Stateful / temporal nodes** (previous-value, integrators, anything reading wall-clock or prior ticks). Per-tick state arrives as scope *inputs* (e.g. movement's `elapsedMs`, `chargesRemaining`), never as evaluator-held state. No `while` / unbounded-loop node — a request for one is the signal to reject (`scripting.md` §251).
- **Migrating any existing format.** The state-store persist format is unchanged; this plan only *matches its versioning shape* for the new IR envelope.
- **A binary/PRL section.** The IR crosses and bakes as JSON, like reactions and `state.json`.

## Acceptance criteria

- [ ] The minimal opcode set deserializes from camelCase JSON into a typed IR tree and re-serializes identically (round-trip).
- [ ] The opcode vocabulary and the IR value type appear in generated `postretro.d.ts` and `postretro.d.luau` — an author cannot type a node that is not in the vocabulary.
- [ ] The same authored expression built in TypeScript and in Luau produces byte-identical IR JSON (dual-runtime parity contract test).
- [ ] **Bind** rejects, without panicking, a tree that: references an unknown input or output name; is mistyped (e.g. `clamp` over a boolean, `select` whose condition is numeric); or is structurally malformed. Each returns an error carrying the reason; the substrate logs it once; no process failure.
- [ ] **Eval** is total: ÷0, NaN/Inf inputs, and inverted `clamp` bounds each yield a defined finite result with no panic (per-opcode adversarial unit tests).
- [ ] Eval performs **zero heap allocations** per evaluation (verified under a counting allocator over a representative tree).
- [ ] The **same IR tree** binds against two different scopes — a store-backed scope and a movement-like stub scope exposing a movement-local input set — and reads each scope's values, proving the namespace is pluggable, not store-hardwired.
- [ ] An IR buffer with a named output, bound to a read+write stub scope, writes its evaluated value to that output (write path unit-tested via stub; no real adopter).
- [ ] A baked IR envelope carries a `u32` version stamp using the same scheme as the persist format; loading an envelope whose version is unsupported is ignored with a warning and does not panic (round-trip + bumped-version-rejection tests).
- [ ] Movement, shields, UI, reactions, and animation-channel code are unchanged (no adopter wiring).

## Tasks

### Task 1: IR node model, value type, wire format, bind-time validation

Define the closed opcode tree as a serde discriminated union (one variant per opcode, camelCase `op` tag), the two-type `IrValue` (number `f32` / boolean), and the deserialize path from the JSON the builders emit. Implement the **bind** pass: walk the tree, type-check arity and operand types per opcode (well-typedness is decidable — every node has a static result type), resolve each named-input and named-output leaf against a `BindingScope` (Task 2 provides the trait; Task 1 may define it as the seam it validates against), and produce an eval-ready `IrProgram`. Bind returns `Result` — structural/type/name/version faults become a single logged error and a caller fallback, never a panic. Owns the Boundary Inventory below.

### Task 2: Evaluator + scoped binding abstraction

Define the `BindingScope` trait — bind-time `resolve_input(name) -> Option<InputHandle>` / `resolve_output(name) -> Option<OutputHandle>`, eval-time `read(InputHandle) -> IrValue` / `write(OutputHandle, IrValue)` — so the evaluator never holds a global namespace. Implement the **eval** pass over an `IrProgram`: pure, total, bounded, zero per-eval allocation, no string lookups (names already resolved to handles at bind). Arithmetic is total (÷0 → 0, non-finite guarded, `clamp` tolerates inverted bounds). Ship a store-backed scope adapter (reads/writes slots by dotted name via the store's existing engine read/write-by-name API; respects slot validation) and a read+write **test stub scope** exercising both paths. No real adopter consumes the evaluator in this plan.

### Task 3: Builder vocabulary + typedef emission + dual-runtime parity

Author the opcode builders in both runtimes so the vocabulary is typed at authoring time and emitted into `postretro.d.ts` / `postretro.d.luau` via the existing typedef path (register the IR union and builder signatures through the type registry — `register_tagged_union` precedent). Builders construct plain IR-node data; constructing a node binds nothing and evaluates nothing. Add the **parity contract test**: the same authored expression in TS and Luau produces identical IR JSON (the dual-runtime test pattern the per-runtime install tests seed).

### Task 4: Versioned IR envelope

Wrap a serialized IR tree in a versioned envelope mirroring `PersistedState` — a `u32` version field plus `CURRENT_IR_VERSION` with the *"increment only with a defined migration path"* discipline. Load-time version check mirrors the persist loader: an unsupported version is ignored with a warning, the adopter falls back. Document the opcode-vocabulary evolution rule: adding an opcode is additive (new variant, no bump); removing or changing one is breaking and gated by a version bump + migration. State explicitly that this is the *same* versioning story as the state-store persist format and the deferred `setState` IR.

### Task 5: End-to-end + round-trip test harness (the test gate)

The proof the substrate composes: author an expression via the builders in each runtime → cross the FFI as JSON → deserialize → bind against the stub scope → evaluate → assert the expected value; plus the wire-format round-trip and the version-stamp round-trip/rejection. Consumes Tasks 2–4.

## Sequencing

**Phase 1 (sequential):** Task 1 — the node model + wire format + bind seam; everything else builds on it.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4 — evaluator, builders/typedef, and versioned envelope are independent surfaces over Task 1's tree (different modules: evaluator vs. `sdk/lib` + `typedef.rs` vs. the envelope serde). Task 4 wraps Task 1's tree additively.
**Phase 3 (sequential):** Task 5 — end-to-end harness consumes the evaluator (2), builders (3), and envelope (4).

## Rough sketch

- **Node tree.** `IrNode` serde enum, `#[serde(tag = "op", rename_all = "camelCase")]`, variants `Const` / `Input` / `Add` / `Sub` / `Mul` / `Div` / `Clamp` / `Lerp` / `Select` / `Lt` / `Le` / `Gt` / `Ge` / `Eq` / `Ne`. `Input` and the named output carry a dotted-name `String`. Mirrors the tagged-union decode pattern in `conv.rs::ComponentValue` and `data_descriptors.rs::PrimitiveDescriptor`.
- **Value + types.** `IrValue { Number(f32), Bool(bool) }`. Static result type per opcode (arithmetic/clamp/lerp/const-num/numeric-input → number; comparisons → bool; `select` → its branch type; bool input/const → bool). Bind validates the type graph; eval never re-checks.
- **Bind → program.** `bind(&IrNode, &impl BindingScope) -> Result<IrProgram, BindError>`. `IrProgram` is the flattened, name-resolved, eval-ready form (leaves hold `InputHandle`/`OutputHandle`, not strings) so eval is alloc- and string-free. On `Err`, the adopter logs once and uses its native fallback (movement → native intent; a slot writer → no-op) — the persist loader's "warn and defaults stand" shape (`state_persistence.rs:109`).
- **Scope.** `BindingScope` trait (read + write, both optional-capability). Store adapter delegates to `read_store_slot` / `write_store_slot` (engine write, validated/clamped). Movement's scope (plan 3) exposes `speed` / `grounded` / `chargesRemaining` / … from the movement component **without** routing through `worldQuery` — the engine-internal invariant (`entity_model.md` §7b) holds because the scope is an engine-side binding, not the script-facing slot table.
- **Versioning.** `BakedIr { version: u32, root: IrNode }`, `const CURRENT_IR_VERSION: u32 = 1`, serde_json — a direct mirror of `PersistedState` / `CURRENT_STATE_VERSION` (`state_persistence.rs:14`, `:38`, `:109`). Do not invent a second scheme.
- **Typedef.** Register the `IrNode` union + builder signatures through the type registry so `typedef.rs::generate_typescript` / `generate_luau` emit them — the typedef is the limit-of-vocabulary documentation by construction (`scripting.md` §255).

## Boundary inventory

| Name | Rust | Wire / serde | TS builder | Luau builder |
|---|---|---|---|---|
| node discriminant | `IrNode` variant | `"op"` tag, camelCase (`"clamp"`, `"select"`, `"lt"`, …) | builder fn per op (`clamp`, `select`, `lt`, …) | same names, identical JSON |
| numeric leaf | `IrNode::Const(f32)` | `{ "op": "const", "value": <number> }` | numeric literal → `const` node | same |
| boolean leaf | `IrNode::Const(bool)` / bool const | `{ "op": "const", "value": <bool> }` | boolean literal → `const` node | same |
| named input | `IrNode::Input(String)` | `{ "op": "input", "name": "speed" }` | `input("speed")` | `input("speed")` |
| named output | output `String` on the buffer | `{ "output": "player.shield", … }` | output target field | same |
| value model | `IrValue::{Number(f32),Bool(bool)}` | JSON number / bool | `number` / `boolean` | `number` / `boolean` |
| version stamp | `BakedIr.version: u32` | `{ "version": 1, "root": … }` | n/a (engine-baked) | n/a |

Naming follows the slot store's dotted-name convention (`player.health`); numbers are `f32` in-engine, JSON-number on the wire (the persist format's `f64`→`f32` narrowing precedent applies if a baked IR is ever read back as `f64`).

## Wire format

No binary/PRL section. The IR crosses the FFI as JSON (serde, the reaction/descriptor precedent) and bakes as JSON in the versioned envelope (serde_json, the `state.json` precedent). Pin: camelCase keys; discriminated union on `"op"`; numbers as JSON numbers; the envelope is `{ "version": <u32>, "root": <node> }` mirroring `PersistedState`. Empty/edge encodings follow serde defaults; an unsupported `version` is ignored-with-warning, never an error.

## Open questions

- **Builder mechanism.** Whether opcode builders are registry primitives that return node data or pure `sdk/lib` constructors typed against the emitted `IrNode` union — implementer's choice within the two hard requirements (vocabulary appears in the typedef; constructing a node has no runtime effect). Pure SDK constructors look simplest (no FFI call to assemble data) and still satisfy the typedef contract.
- **Compound conditions for plan 3.** Confirm movement's first real expressions (`boost = f(speed, charges, grounded)`) need only single-comparison `select` conditions in v1. If they need `grounded AND speedAbove`, `and`/`or`/`not` get added deliberately in plan 3 — flagged so it is a decision, not a surprise.
- **Write-path contract granularity.** The write path is designed and stub-tested here; the exact contract for a *real* slot write (engine-bypass vs. readonly-gated; clamp/validate semantics) is settled by the first write adopter (shield recharge or UI `setState`), not this plan. The store adapter delegates to the existing engine write-by-name API, which already validates and clamps.
