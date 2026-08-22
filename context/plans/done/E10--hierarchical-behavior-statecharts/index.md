# Hierarchical Behavior (Statecharts)

## Goal

Grow the flat enemy behavior-state graph into a recursive Harel statechart: a brain and a stateful
(nested-graph) layer share one `{ initial, activities, transitions }` envelope, while a composite
activity composes orthogonal `layers` — so behaviors nest and run at once. The root brain is that
envelope plus the graph-wide fields (`moveSpeed`, `attacks`, …). This unlocks the one capability the
flat graph cannot express — a phase a brain cannot be routed out of until it completes (attack
windup→commit→recover) — the mechanism three shipped/drafted specs deferred here by name. The
proving consumer: the reference enemy fights with a committed slam and rotates between two melee
attacks, all authored content on the new substrate.

## Scope

### In scope

- **Recursive envelope, single canonical spelling.** The envelope `{ initial, activities,
  transitions }` — `transitions` a source-keyed adjacency map (activity name or `"*"`), values
  ordered guarded rows (first-true-wins) — is the **shared recursive shape** used by two shapes: the
  root brain, and a nested-graph layer. The envelope type itself carries **no** graph-wide fields. An
  `activity` is a node: a leaf, or a **composite** carrying `layers` (not the envelope shape). The
  flat inner-`transitions`-per-state shape is retired — there is no lowering sugar; all authored
  graphs re-author.
- **Retained graph-wide fields (root only).** `candidate_filter`, `patrol`, `attacks`,
  `engagement_radius`, and the required `move_speed` stay at the **root** — siblings of the root
  envelope's `activities`, unchanged in shape and meaning (the per-activity `speed`/`route` migration
  is deferred — see Out of scope). A nested-graph layer's envelope carries none of them (a required
  `move_speed` cannot live on a nested layer). An `action:` verb and an `offense` layer resolve an
  attack by name against the root `attacks` map, exactly as the flat graph does today.
- **Layers.** An activity's `layers` run orthogonally. A layer is a **selector list** (stateless,
  first-match per tick) or a **nested graph** (the same envelope, its own state). `motion:` sugars a
  single-entry `move` layer; `action:` sugars a single-entry `offense` layer — the flat leaf verb and
  the layered form are one model. A selector `move` layer is evaluated first-match per tick to yield
  the tick's `MotionVerb` for steering and **requires a trailing motion fallback** (locomotion is
  never undefined); a selector `offense` layer yields an attack handle and may have no match (no
  attack this tick). A nested-graph layer is walked by `select_transition` (below). **A composite
  carries at most one nested-graph (stateful) layer** — selector layers are unlimited and orthogonal,
  but the active state is a single linear path, so a second stateful region has nowhere to live and is
  a parse-time error (AC18). Multiple parallel stateful regions (Harel AND-states) are a foreclosed
  future extension: reviving them means a per-region active-path forest and is additive.
- **Animation source, single-name collapse.** Leaf activities carry `animation` as today; a composite
  **may** carry an optional locomotion `animation` (the clip shown while it moves). The host resolves
  a layered entity's animation to **one** mesh `current_state` name per tick by precedence: the
  offense-layer active leaf's `animation` when that layer is driving a clip, else the composite's
  locomotion `animation`. Per-layer clip *composition* (blending a move clip with an offense clip) is
  a pre-E21 bridge: E21's pose-stack (per-layer clip composition) can leave the optional composite
  `animation` vestigial without a format break. Until then the resolved name is a single string, and
  that string is the only thing that replicates (wire unchanged — `BrainComponent` never crosses the
  wire; only `WireMeshAnimationState.current_state` does).
- **Scoped transitions.** `"*"` at a level = "while this composite is active" — the flat `interrupts`
  generalized. Outer scope beats inner. A source-keyed row targets siblings (exits); a nested `"*"`
  targets children (internal). Cross-boundary transitions are foreclosed (same-level targets only).
- **Bounded nesting depth.** Nesting is capped at a small constant (`MAX_BEHAVIOR_NESTING_DEPTH` —
  the reference enemy needs ~3; the cap sits above with headroom). A graph nested deeper is a
  parse-time validation error. Derived from project commitments, not taste: it makes the active path
  a **fixed-capacity array** (zero-heap by construction — the alloc-probe invariant) and holds the
  line against a general-purpose statechart engine (`index.md` §Non-Goals); behavior is a "shallow"
  design stance (roadmap), so the cap constrains nothing real.
- **Per-activity `timeInActivityMs`, evaluator-resolved.** `@brain.timeInStateMs` generalizes to a
  **scope-relative** per-activity elapsed-time fact: a guard reads the elapsed time of the activity
  whose transitions it belongs to. Because one `select_transition` pass evaluates guards at several
  levels, this fact is **not** a plain fixed slot fed once by `refresh`: `refresh` populates the
  per-level timer array (in the fixed-capacity path), and the evaluator re-points the fact's scope
  slot to the currently-evaluating activity before each level's guard eval — allocation-free (writing
  an existing fixed slot). The seam stays whole (transition ordering lives entirely inside
  `select_transition`); its scope is no longer strictly immutable across the descent. Renamed from
  `@brain.timeInStateMs` — a breaking published-`@brain.*`-name change with a widened meaning
  (brain-global → scope-relative), per `index.md` §2 (primitive surface is a contract).
- **Guard algebra.** `and`/`or`/`not` IR opcodes (retiring the `select(cond, false, true)` negation
  idiom), a fluent guard surface (`.le/.ge/.lt/.gt/.eq/.ne/.and/.or/.not/.between`) over the existing
  `brain.*`/`state()` leaves. The native-boolean-ops hazard (`sample.ts`) is mitigated by typing — a
  `when` position rejects a bare `boolean`, catching `!node` — and documented for `&&`/`||` (TS cannot
  reject truthy operands and there is no lint harness). The `1e9` no-target sentinel is retained.
- **Attacks-fired counter fact.** A new `@brain.attacksFiredInActivity` guard input — a
  scope-relative count of **successful** attack fires (not commit entries) since the evaluating
  activity was entered, resolved per level the same way `timeInActivityMs` is, reset on entry. Read
  as-of-tick-start (a fire this tick is not visible until next tick's refresh). The vocabulary forced
  rotation needs, absent from every existing `@brain.*` input. **Authoring hazard, recorded:** because
  the fire is cooldown-gated and edge-triggered, an attack whose `cooldownMs` exceeds its
  windup→commit→recover cycle never re-fires, so a fire-count rotation stalls — author `cooldownMs`
  at or below the phase-cycle sum, or rotate on a different fact. No load-time lint (the phase-window
  sum is authored as guard IR, not a plain scalar, so a precise lint is fragile); the hazard is
  documented, not engine-enforced.
- **Edge-triggered attack fire.** An attack fires **once, on entry into the activity whose `action`
  names it** (the `commit` phase) — entry reached by a transition or by a composite's initial
  descent, not per eligible tick. The fire is evaluated exactly once on that entry tick and not
  retried within the hold. The per-attack cooldown map is an engine floor: the fire happens on entry
  only if that attack's cooldown is ready and the target is in range, then re-arms it; authored phase
  durations (chiefly `recover`) own the inter-attack pacing. This supersedes, for nested `offense`
  graphs, the flat graph's per-tick cooldown-gated fire.
- **First consumer — commit + rotation.** The reference enemy re-authored into the envelope with a
  nested `offense` layer running windup→commit→recover on its slam (edge-fire on commit entry), and
  jab↔slam rotation via the counter fact. Relocated from `sdk/behaviors/reference/` to
  `content/dev/scripts/reference-enemy.{ts,luau}`, hand-dual-authored. `pose_fixture_enemy`
  re-authored to the recursive shape, staying in the SDK.

### Out of scope

- **Per-activity `speed` / `route` migration** (graph-wide `moveSpeed`→`speed`,
  `patrol`→`route`). **Witting divergence, owner-approved.** Three sources assign this migration to
  *this* spec: `roadmap.md` line 61, `sample.ts`'s `[E10-fact]` hand-off, and
  `enemy-multi-attack/index.md`. Deferred to a follow-up by explicit owner decision — `moveSpeed` is
  a shipped required field whose migration touches descriptor, validation, SDK, replication, and
  every authored enemy (`sample.ts` flags exactly this cost), orthogonal to the hold this spec
  delivers. Kept additive: graph-wide `moveSpeed`/`patrol` stay as retained top-level fields (In
  scope), and per-activity `speed`/`route` land over them later as optional overrides with no format
  break. Cost carried now: the reference enemy's `patrol`/`engage`/`retreat` share one speed.
- **Remote telegraph fidelity.** The host resolves one `current_state` per tick, but the wire samples
  per snapshot interval — a windup/commit shorter than the interval may never be sampled, so a co-op
  client can see an un-telegraphed hit (damage is host-authoritative). Remote telegraph fidelity is
  **not guaranteed here**; the telegraph *presentation* layer (Epic 16) owns any minimum-window or
  extra-signal fix. Recorded, not claimed benign.
- **Telegraph** — `onExit` and a one-shot clip verb (the launcher-raises feel). Epic 16 combat
  stances; the envelope must not foreclose it (leaf `onEnter` stays; no `onExit` added here).
- **Combat `stance` and the planner.** The `stance` word stays reserved; `select_transition` stays
  the replaceable-whole planner seam (no `goal` spent on nodes). Epic 16.
- **Perception / LOS facts, state-scoped search states.** Enemy line-of-sight + cover — the first
  real consumer of `"*"` scoped transitions arrives there. No new perception inputs here.
- **Faction & relationship model, mutable home anchor, melee lunge.** Their own roadmap items.
- **Layer history** (resume a nested graph on composite re-entry). Restart-at-initial is the decided
  default; a `history:` flag is a later additive option.
- **`agent.target.*` auto-existence-conjunction context** (retiring the `1e9` sentinel). The fluent
  surface wraps the existing `brain.*` leaves; the sentinel stays. Deferred to perception/faction.

## Direction

**Problem.** The flat graph re-evaluates every guard every tick and always takes the first true one,
so a brain can never hold a phase it cannot be immediately routed out of. Observed as a cluster:
`E10--enemy-multi-attack` deferred windup→commit→recover, forced rotation, and lunge-safety here by
name; the `E10--enemy-stagger` draft's commitment window is a `timeInStateMs`-guard approximation of
the same hold; `context/research/enemy-attack-modes.md` names the missing hold as the sole
system-feel gap and the attacks-fired counter as the sole missing fact. The cause is the flat
single-`state_index` representation with no nesting — not any one missing behavior.

**Prior commitments.** `E10--behavior-state-graph` established `select_transition` as the pure
pluggable planner seam ("replaces this function WHOLE — the ordering contract lives entirely inside
it") and the rule that nothing a state is doing ever blocks guard evaluation (commitment windows are
authored guards, not an engine latch). This spec preserves both: nesting is expressed in the graph
the seam walks, not a latch beside it, and the seam stays whole. One relaxation, stated: the seam's
scope is no longer strictly immutable across the descent — the evaluator re-points the two
scope-relative fact slots per level (see Scope). It also listed `and`/`or`/`not` opcodes and
hierarchical sub-states as named additive follow-ups — this is their delivery. Bundling the fluent
algebra rather than deferring it answers the `sample.ts` `[E10-syntax]` owner call: the reference
graphs re-author once, in the fluent dialect. `E10--enemy-multi-attack` shipped the `attacks` map as
"the endpoint the migration wants"; it stays a retained top-level map here, and the nested `offense`
layer references its entries by name — additive. `context/lib/index.md` §2 (*Primitive surface is a
contract*): the new opcodes, fluent methods, counter fact, the `timeInStateMs`→`timeInActivityMs`
rename, and recursive descriptor types update SDK types, validators, and typedef fixtures in the same
pass; the `E10--enemy-stagger` draft's `timeInStateMs` guard is updated when it re-targets onto this
substrate. **Divergence 1:** the flat inner-`transitions` authoring shape is retired outright rather
than kept as lowering sugar (the `components.ai`→graph precedent). Warranted: only one dual-authored
file carries `behavior` graphs (`reference_enemy`, relocating, + `pose_fixture_enemy`), so
re-authoring both costs less than a permanent lowering layer — verified by grep, no authored
`components.ai` remains. **Divergence 2 (from `sample.ts`):** row targets are **string names** — keys
into `activities` and the `transitions` map — not `sample.ts`'s identity handles. Warranted: the flat
graph already cross-references states by string name; string names keep the recursive shape
authorable as raw object literals (no `defineBrain`/`defineActivity`/`on` factory surface) and keep
membership validation a name-resolution check.

**Alternatives rejected.** A narrow per-state `commitMs` latch on the flat graph — a field that
suppresses transition evaluation for a window — delivers windup→commit→recover without nesting or
layers, and is far cheaper. Rejected: it reintroduces exactly the latent-action coupling
`E10--behavior-state-graph` designed out (a state that both evaluates guards and holds an engine
latch), delivers no orthogonal layers (rotation and the ogre stance need `move` ‖ `offense` running
at once), and is the local hack the roadmap named and deferred *to statecharts*. It buys a smaller
diff now at the cost of returning for the real substrate within one spec — the baby-step the "build
more right faster" principle warns against when the destination (the `sample.ts` `[HFSM]` envelope)
is already drawn.

## Acceptance criteria

- [ ] **AC1 — Recursive parse, both runtimes.** A `components.behavior` in the recursive envelope
  (`initial`, `activities`, source-keyed `transitions` incl. a `"*"` key, a composite with `layers`,
  the retained top-level `moveSpeed`/`attacks`/etc.) parses to an identical descriptor in QuickJS and
  Luau. Pathed, wire-cased rejections in both: a `transitions` key or row target naming no declared
  activity at its level; empty `activities`; a `"*"` row that self-targets
  its own composite; a `move` selector layer with no trailing motion fallback. **Duplicate activity
  names** are rejected on the **raw-JSON boundary only** — `activities` stays an object-map and reuses
  the shipped `deserialize_states` pattern (its `a_duplicate_state_key_in_raw_json_is_rejected` test is
  the precedent). Both script runtimes collapse duplicate object/table keys before the bridge, so the
  JS/Luau authoring path cannot present one (TypeScript additionally flags it at author time, `ts1117`);
  this is the accepted, documented limitation every descriptor map (`states`, `attacks`,
  `mesh.animations`) already carries — `activities` is **not** switched to an ordered list.
- [ ] **AC2 — Single spelling; flat retired.** The retired flat shape — a state carrying its own
  inline `transitions`, or a top-level `interrupts` list — fails to parse in both runtimes with a
  pathed error, not a silent drop. No graph anywhere authors the flat shape, and no evaluator-side
  flat `interrupts`/`states` program structure survives (it is replaced, not left dormant).
- [ ] **AC3 — Membership + unreachable lint.** A row targeting an activity not returned in
  `activities` at its level is a load error (never a silent vanish). An activity declared in
  `activities` but reachable from no row nor `initial` emits an "unreachable activity" lint warning.
- [ ] **AC4 — Cross-boundary foreclosed.** A row whose target names an activity at a different
  nesting level (a parent's row naming a grandchild, or a nested row naming an outer sibling) is a
  pathed load error in both runtimes.
- [ ] **AC5 — New opcodes, every exhaustive site.** `and`, `or`, `not` round-trip through the
  `{"op":…}` serde shape; bind as Bool operands → Bool (a non-Bool operand is a pathed bind error);
  evaluate correctly (`not` inverts; `and`/`or` truth tables). Arms are added to **every**
  compiler-enforced exhaustive `IrNode` match — `dispatch_input_names`, `bind_node`, `hash_ir_node`
  (`content_hash.rs`, with a chosen canonical byte tag per opcode — a mod content-hash compatibility
  decision), and the `exhaustive_domain_sentinel` (`mod_digest.rs`) — plus the parallel `BoundNode`
  arm read by `eval_node`, none with a `_` fallback.
- [ ] **AC6 — Fluent surface + native-op hazard.** `brain.targetDistance.le(2).and(brain.targetHostile)`
  and `.not()` emit the corresponding opcode IR in both runtimes, identical descriptors. A `when`/guard
  position rejects a bare `boolean` — so native `!node` (which returns `boolean`) is a compile error,
  pinned by a `@ts-expect-error` in `sdk/type-tests/`. The residual native `&&`/`||` hazard (TS cannot
  reject truthy operands; no lint harness exists) is documented in `docs/scripting-reference.md`, not
  enforced — a review/doc gate, not a runnable test.
- [ ] **AC7 — Commit holds, fires once on entry.** In a nested windup→commit→recover `offense` graph,
  `commit` does not transition out before its `timeInActivityMs` window elapses even though the tick
  keeps evaluating; the window exit fires on the first tick the threshold is crossed and never
  before. The attack fires **at most once, on entry into `commit`** (edge-triggered) — and only if
  the attack's cooldown is ready and the target in range; a longer commit window never double-fires,
  and a re-entry while the cooldown is still counting does not fire. Composite entry seats the **full
  recursive initial descent atomically on the entry tick** (through every nested-graph layer), so no
  tick observes an active composite whose nested layer holds no active leaf; each seated node's own
  transition rows are first evaluated the following tick (the flat single-transition-per-tick
  `state_index` rule), so every nested phase — a 0 ms window included — is observed ≥1 tick. An action
  leaf reached as a nested graph's `initial` fires on its entry tick, like one reached by transition.
  Each clause here — commit-holds, fires-at-most-once-on-entry (cooldown/range-gated), atomic full
  descent, every-phase-observed-≥1-tick, initial-action-fire — is a **named sub-assertion** in the
  Task 2 fixture, not one opaque green box.
- [ ] **AC8 — Scope-relative `timeInActivityMs`.** Within one evaluation pass, a guard on a nested
  activity reads that activity's elapsed time and a guard on its parent composite reads the
  composite's — the two differ when the composite has been active longer than the nested phase.
  Entering an activity resets its clock.
- [ ] **AC9 — Restart on any entry.** Any entry into a composite — a transition, an aggro-gate-forced
  return to `initial`, or a graph hot-reload/reseat — restarts its nested graph at `initial` (no
  history), collapses the active path depth to that initial descent, and zeroes every descendant
  `timeInActivityMs`; the zeroing is part of seating the descent and **strictly precedes** that
  entry tick's edge-fire/increment (so a fire on the entry tick counts as 1, never 0). While a
  composite is inactive its descendants' clocks are frozen and unobservable. (The descendant
  `attacksFiredInActivity` reset rides the same entry path; verified with the counter in AC11.)
- [ ] **AC10 — Outer beats inner.** An outer `"*"` row (or a parent source-keyed exit) whose guard is
  true preempts a committed inner phase the same tick it fires. A lethal hit (E16 despawn) also
  preempts — the quiescent entity is excluded from the AI pass and no committed phase persists.
- [ ] **AC11 — Counter fact drives rotation.** `@brain.attacksFiredInActivity` reads 0 on activity
  entry and increments once per **successful** fire within that activity (a cooldown-skipped commit
  entry does not count); it is read as-of-tick-start, so a rotation authored
  `on(attacksFiredInActivity.ge(N), other)` fires the tick **after** the N-th successful fire, not on
  it. It reads 0 again after the composite re-enters (the reset rides the entry path AC9 establishes).
- [ ] **AC12 — Animation collapse, replicated wire unchanged.** A layered entity (move ‖ offense both
  active) resolves to exactly one mesh `current_state` name per tick — the offense active leaf's clip
  when it drives one, else the composite's locomotion `animation`. **No replicated wire-mirror struct
  changes** (`WireMeshAnimationState`/the snapshot payloads) — host-only `BrainComponent` and the
  descriptor serde do change; a grep/review gate confirms no wire-mirror struct changed. A connected
  client renders the entity from that single replicated string; distinct nested activities mapping to
  distinct animation-state names produce distinct replicated names.
- [ ] **AC13 — Zero-heap per tick.** Per-tick guard binding-refresh, the per-level slot re-pointing,
  and `select_transition` over a nested graph allocate zero heap (the existing alloc-probe assertions
  extended to the nested path). The nested program structure is flattened at bind time and the active
  path walked through a fixed-capacity buffer, so zero-heap holds by construction.
- [ ] **AC14 — Reference enemy, relocated + dual-authored.** `reference_enemy` (export
  `referenceEnemyEntity`, `canonicalName` from `REFERENCE_ENEMY_CLASSNAME`) lives at
  `content/dev/scripts/reference-enemy.{ts,luau}`, registered by the dev mod, authored by hand in
  both languages to identical descriptors. It runs a committed slam (windup→commit→recover) and
  rotates jab↔slam by the counter fact. `poseFixtureEnemyEntity` is re-authored to the recursive
  shape in the SDK. Both drift guards pass, re-pointed: the Rust oracle matches the relocated Luau,
  and the TS≡Luau twin holds on the relocated file.
- [ ] **AC15 — Overlay + determinism + typedefs.** The agent-diagnostics overlay names the **full**
  active nested activity path (e.g. `engage/offense/commit`) — a new path→names walk against the
  recursive `activities`, not the single `state_name()` lookup. Sim determinism tests stay green. SDK
  typedef drift tests pass with the recursive `activities`/`layers`/`transitions` types, the three
  opcodes, the fluent methods, `timeInActivityMs`, and `attacksFiredInActivity` in both committed
  fixtures. The overlay path-walk, the determinism suite, and the typedef-drift gate are three
  **separate** assertions, not one bundled check.
- [ ] **AC16 — Nesting depth cap.** A graph nested deeper than `MAX_BEHAVIOR_NESTING_DEPTH` is a
  pathed parse-time validation error in both runtimes; a graph at the cap parses. The reference
  enemy's depth sits below the cap with headroom.
- [ ] **AC17 — Selector move layer steers.** A composite's `move` selector layer moves the agent:
  first-match per tick yields the tick's `MotionVerb` (a matched row's motion, else the trailing
  fallback), driving steering — verified by the reference enemy's `engage.move` chasing the target
  and holding at range, and by at most one attack firing per enemy per tick when the active path
  carries more than one action (offense-layer precedence).
- [ ] **AC18 — One stateful region per composite.** A composite declaring two or more nested-graph
  layers is a pathed parse-time error in both runtimes; a composite with one nested-graph layer plus
  any number of selector layers parses. This is the single-active-path contract Task 2's fixed-capacity
  path depends on.

## Tasks

### Task 1: Recursive descriptor + single-spelling parse

Reshape the behavior descriptor in `postretro-foundation`
(`crates/foundation/src/data_descriptors/types/behavior.rs`, 1256 lines — factor the recursive types
into a submodule as it reshapes; do not extend the flat types in place). Replace the flat
`BehaviorGraphDescriptor { initial, states: BTreeMap<name, State-with-inline-transitions>, interrupts
}` with three shapes. (1) A shared **envelope** type `{ initial, activities, transitions }` —
`transitions` a source-keyed adjacency map keyed by activity name or the `"*"` literal, values
`Vec<GuardedRow { when: IrNode, to: String }>` — carrying **no** graph-wide fields; used by the root
brain and by a nested-graph layer alike. (2) An `Activity`: a leaf (`animation`, `motion`/`action`
sugar, `on_enter`) or a composite carrying `layers` and an **optional composite `animation`** (the
locomotion clip) — a composite is *not* envelope-shaped. (3) A `Layer`: a selector list or a nested
envelope. The **root descriptor** (`BehaviorGraphDescriptor`) is the envelope **plus** the retained
graph-wide fields; a nested-graph layer is the bare envelope. **Retain unchanged**, at the root only,
the current fields `candidate_filter`, `patrol`, `attacks`, `engagement_radius`, and required
`move_speed` (a required field cannot live on a nested layer, so the envelope type must not carry
it) — an `action:` verb / `offense` layer resolves against the root `attacks` map by name, as today. `motion:`/`action:` desugar to single-entry
`move`/`offense` layers; a selector `move` layer carries a required trailing `MotionVerb` fallback
(enforced in TypeScript via a `[...Row[], MotionVerb]` tuple; Luau relies on the AC1 runtime
rejection — the shipped const-generic asymmetry). Retire the flat inner-`transitions` and top-level
`interrupts` shapes — both become parse errors. Validation (all pathed, wire-cased): membership
authority (every row key and target resolves to a declared activity at its own level — cross-level
targets rejected, Invariants table), duplicate-activity-name rejection on the raw-JSON boundary
(`activities` stays an object-map; port the shipped `deserialize_states` pattern — both runtimes
collapse duplicate keys before the bridge, so do **not** switch to an ordered list), empty-`activities`
rejection,
`"*"`-self-target rejection, motion-fallback presence, **at most one nested-graph layer per composite**
(selector layers unlimited — the single-active-path contract, AC18), and nesting depth ≤
`MAX_BEHAVIOR_NESTING_DEPTH` (a new small constant — AC16 — the contract that lets Task 2 keep the
active path a fixed-capacity array); plus an "unreachable activity" lint in `behavior_lints`. Both
twin parsers funnel through the shared serde path
(`crates/scripting-core/src/data_descriptors/{js,lua}/entity.rs` — verify no per-runtime shim);
migrate the scripting-core parse tests (`crates/scripting-core/src/data_descriptors/tests/behavior.rs`)
to the recursive shape. Register the new `activities`/`layers`/`transitions`/guarded-row types (and
the retained-field rows) for typedef emission alongside `BehaviorGraphDescriptor` in
`crates/postretro/src/scripting/primitives/mod.rs`; regenerate the committed
`sdk/types/postretro.d.{ts,luau}` and fixtures (drift-gated, verified in Task 5). **Drift-oracle
sequencing:** the shipped-content oracles that `include_str!` + parse `sdk/behaviors/reference/
entities.{ts,luau}` — `the_shipped_reference_enemy_descriptor_is_identical_in_both_authorings` +
`shipped_reference_enemy_from_{typescript,luau}` (scripting-core) and (in Task 2's crate)
`the_reference_oracle_matches_the_shipped_authored_graph` / `shipped_reference_behavior_graph` /
`trace_reference_fixture` (`ai_tests.rs`) — parse the **still-flat** shipped reference enemy and pose
fixture, which only Task 5 re-authors. The moment the flat shape becomes a parse error (AC2) these
panic. Mark them `#[ignore]` (reason: "pending statecharts re-author, Task 5") and exclude them from
the Tasks 1-2 completion bars; Task 5 re-authors the content and removes the `#[ignore]`. This task
leaves `crates/postretro` non-compiling; completion bar is `-p postretro-foundation
-p postretro-scripting-core` green **excluding the parked oracles**.

### Task 2: Evaluator generalization + thin nested slice

Consume Task 1's recursive types in `crates/postretro`. Generalize the evaluator to walk one active
path through the nesting rather than a single `state_index`. `BrainComponent`
(`crates/entities/src/components/brain.rs`) `state_index: usize` becomes a **fixed-capacity**
active-activity path (bounded by Task 1's `MAX_BEHAVIOR_NESTING_DEPTH`; the exact encoding is the
implementer's), and `time_in_state_ms: f32` becomes a **per-level timer array** over that path,
feeding the scope-relative `@brain.timeInActivityMs`. This task **owns the full `timeInStateMs`→
`timeInActivityMs` rename surface**: the `BRAIN_INPUTS` const `BRAIN_TIME_IN_STATE_MS_INPUT` (→
`BRAIN_TIME_IN_ACTIVITY_MS_INPUT`) and its wire string in `crates/foundation/src/brain.rs`, plus the
hand-written SDK surfaces `sdk/lib/brain.{ts,luau}`, `sdk/types/postretro.d.ts`, and both static
templates (`crates/scripting-core/src/typedef/templates/sdk_lib.{d.ts,luau}`) — a breaking published
`@brain.*`-name change with a widened meaning.

Replace, don't extend, the flat structures: the bound-program side-table
(`crates/postretro/src/scripting/systems/ai/brain_programs.rs`, 931 lines — split first if the
nesting bloats it) currently holds `interrupts: Vec<Option<BoundProgram>>` and `states:
Vec<Vec<…>>`; these are replaced by a **bind-time-flattened** nested structure (built in `sync`,
which already owns allocation-on-graph-change) so no dormant flat path survives (AC2). Generalize
`select_transition` (`ai/graph_eval.rs`) to evaluate, per active composite level, its `"*"` rows then
its active child's source-keyed rows, outer scope first (outer-beats-inner), keeping it the single
planner seam. **Scope-relative facts:** build a **generic per-level re-point mechanism** whose registration API
takes a **list** of scope-relative slots (so Task 4 appends one without re-generalizing); before
evaluating each level's guards, the evaluator re-points every registered slot to that level's array
entry — writing existing fixed slots, no allocation (AC8, AC13). Phase 1 registers only
`timeInActivityMs`; Task 4 appends `attacksFiredInActivity` to the same list. **Transition
budget:** at most one transition per active level per tick, and an entered activity's own rows are
first evaluated the *following* tick (the flat single-transition-per-tick `state_index` rule), so
every nested phase including a 0 ms window is observed ≥1 tick (AC7). **Enter-composite routine:** a
shared routine seats the **full recursive `initial` descent atomically** (through every nested-graph
layer, so no tick observes a composite with an unseated child — AC7), collapses the active-path depth
to that descent, and zeroes descendant timers (the counter zero lands in Task 4 on this same
routine); the zeroing precedes the entry-tick edge-fire (AC9). It is invoked at **three** sites: a
transition entry (inside `select_transition`), the aggro-gate forced-`initial` branch
(`ai/mod.rs`), and graph reseat (`take_reseat`). The gate branch and reseat **bypass**
`select_transition`, so they call the routine **out-of-band**, collapsing path *depth* — not just
slot 0 — else stale deeper indices/timers survive a gate close or reseat (Invariants table).

Enumerate and convert every current-state consumer beyond `select_transition`: the six resolvers in
`graph_eval.rs` — three index-keyed (`engages`, `animation_for_state`, `action_for_state`) and three
graph/verb-derived (`steering_for` by `MotionVerb`, `locomotion_animation`/`rest_animation`
graph-wide); `ai/mod.rs`
`prior_state_index`/`persisted_state_index`/reseat machinery; and `combat_slots.rs`, which reads
`outcome.brain.state_index` for standoff — now reads the **offense/attack-firing active-leaf** level
of the path. Add **selector-layer evaluation** distinct from transition eval: a `move` selector is
first-match per tick → the tick's `MotionVerb` for steering (AC17); a selector `offense` → an attack
handle. **Animation resolution:** one `current_state` per tick by precedence — the offense active
leaf's `animation` when it drives one, else the active composite's locomotion `animation` (AC12), the
value the host writes and the only thing replicated.

**Edge-triggered fire:** an attack fires once on entry into the activity whose `action` names it, gated
on that attack's cooldown-map entry (`attack_cooldown_remaining_ms`) being ready and the target in
range, then re-arms the cooldown — not the flat per-tick fire (AC7). At most one attack fires per
enemy per tick, resolved by offense-layer precedence over the active path (AC17).

Add the `not` opcode as the minimal algebra proof. The `IrNode`/`BoundNode` variants and the
`dispatch_input_names`, `bind_node`, `eval_node` arms land in `postretro-foundation` (`ir/mod.rs`,
`ir/bind.rs`, `ir/eval.rs`); the two remaining exhaustive `IrNode` matches are in `crates/postretro` —
`hash_ir_node` in `content_hash.rs` (with a chosen canonical byte tag, a mod content-hash
compatibility decision) and `exhaustive_domain_sentinel` in `mod_digest.rs`. No `_` fallback at any
site (AC5). Migrate the
`crates/postretro` graph-literal fixtures mechanically to the recursive shape so the crate compiles
(the reference oracle's *values* update in Task 5). Build a registry-constructed synthetic nested
fixture — one composite with a `move` selector layer and an `offense` nested windup→commit→recover
graph driven by `timeInActivityMs`, one guard using `not` — and drive it end to end
(parse→bind→eval→selector-steer→edge-fire→animation-name resolution), asserting AC7, AC8, AC10, AC12,
AC13, AC17 and the timer-reset half of AC9 on it. Completion bar: `cargo test -p postretro` green.

### Task 3: Fluent guard algebra + remaining opcodes

Complete the guard algebra on Task 2's `not` foundation. Add the `and` and `or` opcodes (both Bool
operands → Bool) across every site Task 2 touched for `not` — `IrNode`, `BoundNode`, `bind_node`,
`eval_node`, `dispatch_input_names`, `hash_ir_node` (canonical byte tags), `exhaustive_domain_sentinel`.
Add the fluent method surface to the SDK prelude implementations (`sdk/lib/runtime.ts`, `runtime.luau`,
and the `brain.*`/`state()` leaves in `sdk/lib/brain.{ts,luau}`): comparison
(`.le/.ge/.lt/.gt/.eq/.ne/.between`) and boolean (`.and/.or/.not`) methods emitting the corresponding
opcode IR, over the existing pre-wrapped leaves — the `1e9` sentinel and the leaf set unchanged. Add
the hand-written `Runtime<Op>` node types, union members, and `runtime`-interface methods to the
static typedef templates (`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts`, `sdk_lib.luau` —
hand-maintained, no generator link). **Native-boolean-ops hazard** (`sample.ts`): a guard node reaching
native `&&`/`||`/`!` silently collapses to one operand. TS cannot reject `nodeA && nodeB` (truthy
operands are always legal) and the repo has no ESLint harness, so a general enforced lint has no home;
instead type the fluent combinators (`.and/.or/.not`) as the sanctioned path, ensure a `when`/guard
position rejects a bare `boolean` (so `!node`, which returns `boolean`, is a compile error) with a
`@ts-expect-error` assertion in `sdk/type-tests/`, and document the residual `&&`/`||` hazard in
`docs/scripting-reference.md`. Luau has no compile-time equivalent. Independent of Task 4; both build
on the Task 1/2 representation. Bar: fluent guards emit identical IR
in both runtimes (AC6), opcode round-trip/bind/eval (AC5), typedef drift green.

### Task 4: Attacks-fired counter fact

Add the `@brain.attacksFiredInActivity` guard input in `postretro-foundation`
(`crates/foundation/src/brain.rs`): a new `pub const` name, a `BRAIN_INPUTS` entry appended at index
13 (typed `IrType::Number`), the matching `BrainFacts` field, and the per-level feed. Like
`timeInActivityMs` it is **scope-relative** — **register it into Task 2's generic per-level re-point
mechanism** (Task 2 built the mechanism and registered only `timeInActivityMs`; this adds the second
slot). It is read **as-of-tick-start** (a fire this tick is invisible until next tick's refresh — so
a counter-driven rotation lands the tick after the qualifying fire, AC11). The per-level counter
array lives in the fixed-capacity path (`BrainComponent`); **add its zeroing to Task 2's shared
enter-composite routine** (all three entry sites), before that tick's edge-fire. **Increment** the
counter on the edge-triggered fire in the AI tick (`crates/postretro/src/scripting/systems/ai/mod.rs`,
948 lines — the same fire seam that re-arms the cooldown map), counting **successful** fires at most
once per enemy per tick. Add the SDK prelude entries
(`sdk/lib/brain.{ts,luau}`) and the static template `BrainInputs` interface or the coverage test
`brain_sdk_helpers_cover_every_brain_input` fails. Independent of Task 3. Bar: the counter reads 0 on
entry, increments per fire, resets on re-entry, drives a rotation guard with the one-tick lag (AC11);
coverage and typedef drift green.

### Task 5: Reference enemy re-authoring, relocation, drift tests

Consume Tasks 1–4. Re-author `reference_enemy` (current export `referenceEnemyEntity`) into the
recursive envelope and relocate it from `sdk/behaviors/reference/entities.{ts,luau}` to
`content/dev/scripts/reference-enemy.{ts,luau}`, authored by hand in both languages (the
`frontend-level-select-fixture.luau` dual-authoring precedent), registered in
`content/dev/start-script.ts` alongside the other dev enemies (drop it from the spread-imported
`referenceEntities`). Its `engage` composite gains a `move` selector layer (with a locomotion
`animation`) and an `offense` nested windup→commit→recover graph on the slam (edge-fire on commit
entry), plus jab↔slam rotation authored as an `@brain.attacksFiredInActivity` guard; guards use the
fluent algebra. Re-author `poseFixtureEnemyEntity` to the recursive shape, staying in
`sdk/behaviors/reference/entities.{ts,luau}` (E21 pose tests depend on it there). Re-point both drift
guards: the Rust oracle `shipped_reference_behavior_graph`'s `include_str!`
(`crates/postretro/src/scripting/systems/ai_tests.rs`) to `content/dev/scripts/reference-enemy.luau`,
and update the hand-transcription oracle `reference_behavior_graph()` values to the re-authored nested
graph; the TS≡Luau twin `the_shipped_reference_enemy_descriptor_is_identical_in_both_authorings` +
`shipped_reference_enemy_from_{typescript,luau}` (`crates/scripting-core/.../tests/behavior.rs`) to the
relocated file. **Remove the `#[ignore]` Tasks 1-2 parked on the shipped-content drift oracles** (they
now parse the re-authored recursive content). Update `trace_reference_fixture`/`BrainTrace` for the
nested graph — its `state` field becomes the full active-path string, coordinated with the overlay
format. Author the offense phase windows so the slam's `cooldownMs` does not exceed the
windup→commit→recover cycle sum (else the fire-count rotation stalls — the recorded authoring hazard).
Extend the agent-diagnostics overlay (`crates/postretro/src/agent_diagnostics.rs`, `assemble_agent_overlay_label`
— today a single `state_name()` lookup) with a path→names walk over the recursive `activities` to name
the full active path (e.g. `engage/offense/commit`), consistent with the existing `state:<name>`
format (AC15). Add the AC12 grep/review gate confirming no wire-mirror struct changed. Regenerate and
commit the SDK typedef fixtures. Bar: `cargo test -p postretro -p postretro-scripting-core` green,
including both drift guards, determinism, and typedef drift; AC14, AC15, and AC17's reference-enemy
clause (`engage.move` chases and holds at range) verified.

## Sequencing

**Phase 1 (sequential):** Task 1 → Task 2 — the thin slice. Task 1 stands up the recursive
representation and single-spelling parse (foundation + twin); Task 2 completes the slice through the
evaluator, per-level fact re-pointing, selector-layer steering, edge-fire, and animation collapse on a
synthetic nested fixture, falsifying the representation, evaluator, zero-heap, fact-feed, and
replication-collapse assumptions before any breadth. Task 2 consumes Task 1's types.

**Phase 2 (concurrent):** Task 3, Task 4 — the guard algebra and the counter fact, independent
surfaces on the Phase-1 representation (Task 3 touches the IR/SDK guard surface; Task 4 touches the
brain-input table, the per-level counter, and the fire seam). Both consume Task 2 (Task 3 extends its
opcode sites; Task 4 rides its fire seam and composite-entry reset path).

**Phase 3 (sequential):** Task 5 — re-authors the reference content graph, relocates it, and
re-points the drift tests, consuming the representation (1/2), the fluent algebra (3), and the counter
fact (4). The integration payoff; verifies AC14/AC15 end to end.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| recursive envelope (root + nested layer) | envelope type `{ initial, activities, transitions }`, no graph-wide fields; the root descriptor embeds it plus the retained fields | `{ "initial", "activities", "transitions" }` | `{ initial, activities, transitions }` | same | n/a |
| activities map | envelope `::activities` | `"activities"` | `activities: Record<string, Activity>` | `activities: {[string]: Activity}` | n/a |
| adjacency transitions | envelope `::transitions` | `"transitions"` | `transitions: Record<string, GuardedRow[]>` | same | n/a |
| scope-all key | the `"*"` map key | `"*"` | `"*"` | `"*"` | n/a |
| layers | `Activity::layers` | `"layers"` | `layers?: Record<string, Layer>` | same | n/a |
| leaf animation | `Activity::animation` (leaf) | `"animation"` | `animation?: string` | same | n/a |
| composite locomotion animation | `Activity::animation` (composite, optional) | `"animation"` | `animation?: string` | same | n/a |
| motion sugar | `"motion"` (single-entry move layer) | `"motion"` | `motion?: MotionVerb` | same | n/a |
| action sugar | `"action"` (single-entry offense layer) | `"action"` | `action?: { attack: string }` | same | n/a |
| on-enter event | `Activity::on_enter` | `"onEnter"` | `onEnter?: string` | same | n/a |
| move fallback | trailing `MotionVerb` in a `move` selector | (last array element) | `MotionVerb` | `MotionVerb` | n/a |
| guarded row | `GuardedRow { when: IrNode, to: String }` | `{ "when": …, "to": … }` | `{ when: RuntimeValue, to: string }` | same | n/a |
| retained move speed | `BehaviorGraphDescriptor::move_speed` | `"moveSpeed"` | `moveSpeed` | `moveSpeed` | n/a |
| retained attacks map | `…::attacks` | `"attacks"` | `attacks: Record<string, AttackParams>` | same | n/a |
| retained patrol / filter / radius | `…::patrol` / `candidate_filter` / `engagement_radius` | `"patrol"`/`"candidateFilter"`/`"engagementRadius"` | same | same | n/a |
| not opcode | `IrNode::Not { a }` | `{"op":"not","a":…}` | `RuntimeNot` | `RuntimeNot` | n/a |
| and / or opcodes | `IrNode::And/Or { a, b }` | `{"op":"and"/"or",…}` | `RuntimeAnd`/`RuntimeOr` | same | n/a |
| per-activity timer fact | `BRAIN_TIME_IN_ACTIVITY_MS_INPUT` | `"@brain.timeInActivityMs"` | `brain.timeInActivityMs` | same | n/a |
| attacks-fired counter | `BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT` | `"@brain.attacksFiredInActivity"` | `brain.attacksFiredInActivity` | same | n/a |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A row targets only same-level activities; `"*"` targets children, source-keyed rows target siblings — cross-boundary transitions foreclosed | Task 1 (membership validation) | A target resolved against the wrong level would silently mis-route or vanish | AC3, AC4 |
| `select_transition` stays the sole transition-ordering seam (a planner replaces it whole); it re-points only the two scope-relative fact slots per level, ordering stays inside | Task 2 (recursive walk + per-level re-point live inside it) | Ordering logic leaking into the tick would fork the seam | AC7, AC8, AC10 |
| Outer scope beats inner: a committed inner phase won't self-route out, but an outer `"*"`/sibling-exit/death preempts it | Task 2 (level-ordered evaluation) | Evaluating inner rows before outer would let a commit ignore stand-down | AC10 |
| Nesting depth ≤ `MAX_BEHAVIOR_NESTING_DEPTH` | Task 1 (parse-time cap) | Task 2 relies on the cap for a fixed-capacity path; an unbounded graph forces a dynamic per-tick allocation | AC16 |
| At most one nested-graph (stateful) layer per composite; selector layers unlimited | Task 1 (parse-time rejection) | Task 2's single linear active path cannot represent sibling nested graphs; the animation-collapse precedence assumes one offense stateful region | AC18 |
| Per-tick guard refresh, per-level slot re-pointing, and evaluation allocate zero heap | Task 2 (bind-time-flattened programs; fixed-capacity path via Task 1's depth cap) | A per-tick `Vec`/`HashMap` over the path, or a heap re-refresh, would allocate | AC13, AC16 |
| Attack fires at most once per `commit` entry (edge-triggered), cooldown-gated | Task 2 (edge-fire seam), Task 4 (counter increment on the same edge) | The flat per-tick cooldown fire would double-fire a long commit or zero-fire a re-entry | AC7 |
| A layered entity's animation is exactly one mesh `current_state` name; no replicated wire-mirror struct changes | Task 2 (offense-active precedence resolver) | A layer writing a second animation identity has nowhere to replicate | AC12 |
| Any composite entry (transition / gate-forced / reseat) runs the shared enter-composite routine: atomic full initial descent, active-path depth collapsed to it, descendant timers + counter zeroed before the entry-tick fire | Task 2 (routine + transition + gate + reseat call sites; timer zero), Task 4 (counter zero in the same routine) | The gate branch and reseat bypass `select_transition`; a reset living only inside the seam leaves stale deeper indices/timers/counter after a gate close or reseat | AC9, AC11 |
| Single canonical spelling — no flat `interrupts`/`states` shape in the descriptor or the bound-program side-table | Task 1 (descriptor rejects it), Task 2 (side-table replaced) | A surviving flat parse or program path forks authoring/eval into two shapes | AC2 |

## Ordering table

Each row is concrete enough to write a test from.

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Per-level clock read in one pass | One `select_transition` call evaluates an outer-composite guard (composite active 2000 ms) and a nested-leaf guard (leaf active 100 ms), both referencing `timeInActivityMs` | The evaluator re-points the slot per level: the outer guard reads 2000, the nested reads 100 — different values, same pass |
| Nested node entry tick | Tick T selects a composite whose `initial` nested node has an immediately-true own row (`timeInActivityMs.ge(0)`) | The node is entered on T (its clip may become `current_state`); its own rows are first evaluated on T+1 — it cannot enter and exit on the same tick; every nested phase is observed ≥1 tick |
| Composite with a nested-graph layer entered | Root transition seats `engage` on T; `engage`'s `offense` nested-graph layer descends to its `initial` leaf (`windup`) on the **same tick T**, atomically through the layer | No tick observes `engage` active with the `offense` layer holding no leaf; `windup`'s clip resolves as `current_state` on T; `offense`'s own rows (child + `"*"`) first evaluated T+1 |
| Entry whose initial descent lands on an action leaf | On the entry tick, the enter-composite routine's descendant zeroing precedes the edge-fire increment | Counter reads 1 after the entry-tick fire, never 0; an action leaf reached by initial descent fires on its entry tick, like one reached by transition |
| Attack cooldown longer than the phase cycle | Each commit entry after the first finds the cooldown pending → edge-fire skipped → counter not incremented (it counts successful fires) | `attacksFiredInActivity.ge(N)` never becomes true; the enemy loops without rotating — a recorded authoring hazard (author `cooldownMs ≤` the phase-cycle sum), not engine-enforced |
| Commit window ≥ attack cooldown | `commit` window authored longer than the slam's `cooldownMs`; enemy sits in commit past the cooldown | Edge-fire fires once on commit entry only; it does NOT re-fire mid-commit (the flat per-tick model would) |
| Commit re-entry while cooldown pending | Rotation re-enters `windup→commit` before the shared attack cooldown elapsed | Commit entry does not fire and the counter does not increment on that entry — the edge-fire is cooldown-gated |
| N-th qualifying fire drives rotation | On the fire tick, refresh feeds the counter (start-of-tick) before the edge-fire increments it | The rotation guard `attacksFiredInActivity.ge(N)` sees N-1 on the fire tick and N on the next; the rotation fires the tick AFTER the N-th fire |
| Counter increments at most once per tick | Any tick with an eligible fire | Single fire seam, no loop — the counter rises by at most one; the invariant holds for N=0 (no fire, no change) |
| Outer stand-down vs. nested commit, same tick | Outer `"*"`/sibling-exit rows evaluate before the inner child's rows | The outer exit wins; the committed phase is abandoned that tick |
| Target lost mid-commit | The `!hasTarget`-derived outer stand-down row fires | Enemy leaves `engage` (and its nested `offense`) in one tick, not after the window |
| Composite re-entered via any path | Exit via (a) a sibling exit transition (inside `select_transition`), (b) an aggro-gate close (bypasses `select_transition`), or (c) a graph hot-reload/reseat (bypasses it) — then re-entered | All three run the shared enter-composite routine, collapsing the full active-path depth to the `initial` descent and zeroing every descendant `timeInActivityMs` and `attacksFiredInActivity`; while inactive, descendants' clocks/counter are frozen and unobservable |
| Graph reseat mid-commit | Hot-reload swaps the graph while the enemy is in `engage/offense/commit`; persisted nested indices would address different activities | The whole active path reseats to the new graph's `initial` descent, every level's timer and the counter zeroed; no stale nested index is walked |
| Lethal hit during `commit` | E16 queues deferred despawn; the quiescent entity is excluded from the AI pass | No committed phase persists; no interrupt-ordering race |
| move ‖ offense both drive a clip | Host resolves one `current_state`: offense active-leaf's clip when driving one, else the composite's locomotion `animation` | One replicated string; a client renders from it; a round-trip switch within one snapshot interval aliases away (the shipped sampling limit) |
| Telegraph shorter than a snapshot interval | windup 50 ms → commit 100 ms → recover, whole sweep inside one snapshot interval; the snapshot boundary lands during recover | The remote client's `current_state` may step idle→recover→idle, never sampling windup/commit — a co-op client can see an un-telegraphed hit. Remote telegraph fidelity is out of scope (Epic 16 owns any fix) |
| Two attacking active leaves in one tick | Two orthogonal layers each with an active leaf naming an attack, both cooldown-ready, target in range | Exactly one attack fires (offense-layer precedence over the active path); the other does not also fire; the counter rises at most once |

## Script syntax examples

Authored as raw object literals (the flat graph's style), string-name targets, string motion verbs
(the shipped `MotionVerb` serde values — `"chaseTarget"`, `"hold"`, …), and the fluent guard surface.
A selector list is `[…rows, trailingMotionVerb]`; a `GuardedRow` is `{ when, to }`.

```ts
// Proposed design — the reference enemy's engage composite (abridged).
import { defineEntity, brain } from "postretro";

const engage = {
  // composite locomotion clip, shown while moving and the offense layer drives no clip.
  animation: "walk",
  layers: {
    // selector list: first match wins; trailing bare motion verb is the required fallback.
    move: [{ when: brain.targetDistance.le(2), motion: "hold" }, "chaseTarget"],
    // nested graph: its own state — the windup→commit→recover latch.
    offense: {
      initial: "windup",
      activities: {
        windup:  { animation: "windup" },
        commit:  { animation: "slam", action: { attack: "slam" } },  // edge-fires on entry
        recover: { animation: "recover" },
      },
      transitions: {
        windup:  [{ when: brain.timeInActivityMs.ge(300), to: "commit" }],   // telegraph window
        commit:  [{ when: brain.timeInActivityMs.ge(120), to: "recover" }],
        recover: [{ when: brain.timeInActivityMs.ge(500), to: "windup" }],   // recover owns pacing
        // rotate after two fires; scope-relative counter, reset on re-entry, one-tick read lag.
        "*":     [{ when: brain.attacksFiredInActivity.ge(2), to: "recover" }],
      },
    },
  },
};

export const referenceEnemyEntity = defineEntity({
  canonicalName: "reference_enemy",
  components: {
    health: { max: 70 },
    mesh: { /* model, animation states */ },
    behavior: {
      initial: "patrol",
      moveSpeed: 3,                              // retained graph-wide field
      attacks: { /* jab, slam — retained top-level named map */ },
      activities: { patrol: { /*…*/ }, engage, retreat: { /*…*/ } },
      transitions: {
        patrol:  [{ when: brain.targetHostile.and(brain.targetDistance.le(16)), to: "engage" }],
        engage:  [{ when: brain.distanceFromAnchor.gt(100), to: "retreat" },
                  { when: brain.targetHostile.not(), to: "patrol" }],   // stand-down, engage-scoped
        retreat: [{ when: brain.distanceFromAnchor.le(1), to: "patrol" }],
        "*":     [],   // root-scoped interrupts — empty; stand-down is correctly engage-scoped
      },
    },
  },
});
```
