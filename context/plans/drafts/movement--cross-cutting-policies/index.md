# movement--cross-cutting-policies

## Goal

Settle the two foundation-level policies `movement.md` §6 requires before any further traversal state, plus the refactor that keeps the tick from rotting as states are added:

- **Input forgiveness** — coyote time and jump buffering, derived once and consumed as edges by every state.
- **Momentum conservation** — velocity carry across state transitions, owned by the transition layer as a closed carry-rule vocabulary.
- **State-data ownership** — per-state live data owned through one uniform convention, so adding a state never widens the dispatch.

Second in the `movement--*` series, after `movement--state-machine`. Settle the **policy and seam, not the full breadth** (`movement.md` §6) — breadth grows with the states that consume these.

## Scope

### In scope
- An edge-input / input-forgiveness layer: coyote time + jump buffering, descriptor-tuned, derived before the state intents run, consumed in place of raw button bits.
- A transition-layer velocity-carry seam: a closed carry-rule vocabulary applied at the transition edge by the dispatch point, plus the transform library and its unit tests.
- Re-expressing the existing `Normal`↔`Dash` transitions through the carry seam, behavior-identical (parity gate).
- The non-wall carry vocabulary slide will consume: keep, zero, scale (horizontal), plus boost-carry (keep/drop the D4 boost vector).
- A state-data-ownership refactor: remove the copy-out/copy-back payload threading; own per-state live data in one place behind a uniform access convention. No behavior change.
- Descriptor + SDK surface for the forgiveness tuning (windows), with type emission and drift-test coverage.

### Out of scope
- The **author-facing declarative transition graph** — descriptor-authored `{from, to, when, carry}` rows and the full trigger/carry vocabularies exposed to scripts. This goal builds the *internal* carry seam only; the author surface firms up across later `movement--*` specs (`movement.md` §2).
- **Wall-relative carry rules** (`projectOntoWallPlane`, `reflect`) and the **environment-contact-data carry** — both land with `movement--wall-run`, their first consumer. Direction decided here (D8), implementation deferred.
- Crouch, slide, wall-run, vault states themselves.
- Any change to the collision substrate's behavior. The substrate result *gains* nothing in this goal beyond what input forgiveness needs (D8 contact fields are deferred).
- Imperative per-tick script movement. The surface stays declarative (`movement.md` §2, §7).

## Decisions

**D5 — Input forgiveness is descriptor-tuned, derived once, consumed as edges.** Coyote and jump-buffer windows are author tuning, not engine constants — Neon White and Ultrakill want different forgiveness, so they sit on the flexibility band (`movement.md` §3). The grounded-jump and buffered-jump edges are derived before the per-state intent runs and read by intents in place of raw `jump_pressed`. Intents never re-derive forgiveness. Engine defaults apply when the tuning is absent; the regression fixtures pin the windows to zero to preserve exact edge timing.

**D6 — Momentum carry lives at the transition layer, as a closed carry-rule vocabulary.** The velocity transform applied when one state hands off to another is owned by the dispatch point that applies the transition, not by the individual state intents. The vocabulary is a closed, engine-owned enum of transforms (`movement.md` §2 carry-rules) — no author-shipped code. This goal settles the seam, the policy (each transition declares its carry-rule), and the transform library; `movement--slide` is the first state to *consume* a non-trivial carry (slide→jump keeps slide speed). Settling it now, before slide, prevents the slide-shaped carry logic `movement.md` §6 warns against.

**D7 — Per-state live data is owned through one uniform convention; the dispatch resolves the borrow once.** Today the `Dash` payload is copied out of the enum variant, threaded through the intent's parameter list, and re-packed each tick — because the active state lives on the same component the intent mutates. The refactor owns per-state live data in one place and resolves that borrow once in the dispatch, so a new state adds its data without widening `tick`'s match arms or any intent signature. Pure refactor, regression-gated. Mechanism (in-place ownership vs. a single take/restore helper vs. an intent trait) is an implementation choice; the constraint is no per-state copy-out/copy-back and no payload threading.

**D8 — Environment contact data (deferred-but-decided).** When a state needs surface contact (wall normal for wall-run, floor normal for slope-aware slide), the substrate carries that contact forward in its result for the next tick's intent to read. Intents never call `cast_capsule` themselves — that would break the clean intent/substrate split (`movement.md` §4). The substrate already computes these normals (`ShapeCastHit::normal2`) and discards them today. Fields on the substrate result and the first consumer land with `movement--wall-run`; the direction is fixed here so wall-run cannot accidentally take the ad-hoc-cast path.

## Acceptance criteria

### Input forgiveness
- [ ] With a coyote window > 0, a jump pressed within the window after walking off a ledge (no prior jump) launches a normal grounded jump; the same jump pressed after the window does not.
- [ ] Coyote does not re-arm after a jump: once a ground or air jump is spent, leaving the ground grants no fresh coyote ground-jump.
- [ ] With a jump-buffer window > 0, a jump pressed within the window before landing fires exactly once on the landing tick — not zero times, not twice.
- [ ] A single press near a ledge yields exactly one jump; coyote and buffer never combine into two launches.
- [ ] Coyote and buffer windows are descriptor-tunable; zero disables each independently; an absent forgiveness sub-object applies the documented engine defaults.
- [ ] The SDK type-drift test passes with the new forgiveness descriptor type present in `sdk/types/postretro.d.ts` and `.d.luau`.
- [ ] The existing movement regression suite passes; fixtures pin the forgiveness windows to zero so exact edge timing is unchanged.

### Momentum carry
- [ ] The velocity at a transition edge is determined by the transition's carry-rule, applied by the dispatch layer — not by logic inside a state intent.
- [ ] Carry-rule transforms are unit-tested: `keep` preserves horizontal velocity; `zero` zeroes it; `scale(k)` scales it by `k`; boost-carry preserves or drops the D4 boost vector as specified.
- [ ] The `Normal`↔`Dash` transitions, routed through the carry seam, reproduce current dash behavior exactly: the full `movement--state-machine` dash AC suite passes unchanged.
- [ ] The carry vocabulary is a closed engine-owned enum; no author-shipped code crosses the FFI to define a transform (`movement.md` §2).

### State-data ownership
- [ ] The full existing movement + dash regression suite passes unchanged after the refactor — behavior-identical (D7).
- [ ] Adding a state's live data no longer threads per-state payload through intent parameter lists or copies it out of and back into the enum each tick (verified by the `Dash` path as the worked example).

## Tasks

### Task 1: State-data ownership refactor
Resolve the component/active-state borrow once in the dispatch so per-state live data is owned in a single place behind a uniform convention. Remove the `Dash` payload copy-out (at dispatch) and re-pack (at the no-exit return). Adding a future state's live data must not widen `tick`'s match arms with new parameter threading nor reintroduce copy-out/copy-back. Behavior-identical: the existing regression and dash suites are the gate. This lands first because it reshapes the dispatch and intent seam the other tasks build on. Mechanism is the implementer's call (in-place `&mut` to owned per-state data, a single take/restore helper, or an intent trait) — the constraint is the borrow is resolved once, not hand-rolled per state.

### Task 2: Transition-layer carry seam + vocabulary
Give the transition-application point a carry step: a transition returns its next state *and* the carry-rule to apply, and the dispatch applies that transform to the component velocity (and the D4 boost vector) at the edge. Define the closed carry-rule enum for the non-wall set (keep, zero, scale, plus boost keep/drop) and unit-test each transform. Re-express the current `Normal`→`Dash` and `Dash`→`Normal` handoffs to declare their behavior-equivalent carry-rule, so dash behavior is unchanged (parity gate). Plumbing: the intent step already returns `Option<MovementState>`; extend that return to pair the next state with its carry-rule, and apply the carry where the transition is currently written back. Wall-relative rules (D8) are out of scope. Honest note: with only `Normal`/`Dash` today the consumer is thin — slide is the first real consumer; this task ships the seam, policy, and transform library it will use.

### Task 3: Input forgiveness (coyote + jump buffer)
Derive the grounded-jump edge and the buffered-jump edge before the state intents run, and have the `Normal` jump steps consume those derived edges instead of raw `jump_pressed`. Coyote: permit a grounded jump for a tuned window after ground is lost, gated so it cannot re-arm once any jump is spent — keyed off the airborne-duration signal the substrate already maintains plus a "ground-jump spent" flag. Jump buffer: when jump is pressed while airborne, retain it for a tuned window and fire it on the landing tick. Add the forgiveness windows to the player movement descriptor as an optional sub-object with engine defaults when absent; parse symmetrically in the JS and Luau paths; register for SDK type emission and update the committed typedefs so the drift test passes (the `movement--state-machine` Task 3 path is the template). Plumbing: forgiveness timers/flags live on `PlayerMovementComponent` (reset through the existing landing-refresh point where appropriate); the windows materialize via `from_descriptor`; the raw button bits still arrive through `MovementInput`, but the *consumed* edges are the derived ones. Pin the regression fixtures' windows to zero (D5).

## Sequencing

**Phase 1 (sequential):** Task 1 — reshapes the dispatch and intent seam; lands first so Tasks 2–3 build on stable signatures. Regression-gated, no behavior change.
**Phase 2 (sequential):** Task 2 — consumes the reshaped transition-application point to add the carry step.
**Phase 3 (sequential):** Task 3 — logically independent of Tasks 1–2, but shares `movement/mod.rs`'s intent region; sequenced last to build on the settled seam and avoid file contention.

## Boundary inventory

Forgiveness tuning crosses Rust ↔ wire (JS/Lua object) ↔ TS ↔ Luau. camelCase on every script-facing side, snake_case in Rust (the descriptor sub-structs carry no `#[serde(rename_all)]`; casing is fixed by the hand-written parsers in `data_descriptors.rs` and the `register_type().field()` chain in `primitives/mod.rs`, per the `movement--state-machine` wire-casing mechanism). No FGD KVP, no PRL section — descriptor is a script object. Final field names and the sub-object shape are the implementer's to finalize against the existing `air`/`fall` precedent; this table pins casing and intent.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| forgiveness sub-descriptor (optional) | `Option<…>` | optional nested object under `movement` | optional `?` | optional `?` | n/a |
| coyote window | `…_ms: f32` | `coyoteMs` | `coyoteMs` | `coyoteMs` | n/a |
| jump-buffer window | `…_ms: f32` | `jumpBufferMs` | `jumpBufferMs` | `jumpBufferMs` | n/a |

Units: both windows in milliseconds, advanced off the `dt` already passed to `tick` (`dt * 1000.0` ms per tick), mirroring the dash cooldown's ms accounting. Absent sub-object → documented engine defaults (feel-friendly nonzero); zero per field → that grace disabled.

## Open questions
- **Forgiveness default values.** Engine defaults when the sub-object is absent — pick feel-friendly numbers (coyote and buffer each on the order of ~100ms) during implementation, document them on the descriptor, and pin fixtures to zero. Not architectural; decided at implementation.
- **Forgiveness sub-object shape.** A dedicated `forgiveness { … }` sub-object vs. folding the windows under `air`. Recommend a dedicated optional sub-object — extensible (later forgiveness knobs land here) and reads as its own concept. Finalize against the `air`/`fall` parser precedent.
- **Scoping alternative.** Momentum carry (Task 2) could instead be deferred to the front of `movement--slide` (slide "owns and consumes" it). It is bundled here because `movement.md` §6 directs settling the carry policy *before* slide, and the roadmap groups both policies into one "cross-cutting movement policies" plan. If a smaller increment is wanted, Task 2 can split into its own draft and this goal ships input forgiveness + the refactor.
