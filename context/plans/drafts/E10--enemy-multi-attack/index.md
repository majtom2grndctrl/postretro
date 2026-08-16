# Enemy Multi-Attack

## Goal

A behavior graph carries a **named map of melee attacks** — e.g. a short fast jab plus a
longer-reach kick or two-hand slam (the Fiend's two distinct melee attacks) — each with its own
tuning, referenced by name from the `attack` action verb on whichever states fire it. The map is the
API from day one: no privileged "primary" entry, and it generalizes to N attacks. Every attack is a
contact attack — cooldown-gated damage applied directly to the selected target within reach, the
shipped enemy contact path — with no windup, commit, or recovery phase. That instantaneity is what
keeps selection on the flat graph sufficient: reach-based routing between attacks is authored
`@brain.targetDistance` transitions, the same first-true-wins mechanism every other state choice
already uses. This is action-vocabulary growth on the shipped behavior state graph, not a new
selection engine.

## Scope

### In scope

- `components.behavior.attacks`: a named map (`BTreeMap<String, AttackParams>`) of attack entries.
  Each entry is inline: `damage`, a `maxRange` reach ceiling (the sole engine-enforced distance gate
  — the damage-reach ceiling), `cooldownMs`, and an optional per-attack `engagementRadius` (standoff
  for the state that fires it).
- The `attack` action verb takes a required `attack` parameter naming a map entry:
  `action: { attack: "claw" }`. A state fires exactly the named attack; nothing else about how action
  verbs run inside the tick changes.
- Per-attack engagement radius: the state firing an attack takes its standoff from that attack's
  entry; every other state (and an attackless graph) takes the graph-level `engagementRadius`
  default.
- Per-attack cooldown state on `BrainComponent`, keyed by attack name, so each attack's cooldown
  counts independently.
- `@brain.attackCooldownMs` reports the current state's attack's remaining cooldown (0 when the
  current state fires no attack).
- Reach-based attack selection as graph content: an archetype authors one attack-firing state per
  attack (or per attack cluster) with ordered `@brain.targetDistance` transitions routing into the
  state whose action names the attack that should fire. A floor, where an author wants one, is an
  authored `ge(@brain.targetDistance, …)` guard on that transition — selection is authored
  transitions, not an engine-enforced band. The graph's first-true-wins evaluation is the whole
  selection mechanism.
- Reference enemy archetype gains a second melee attack with distinct reach/damage/cooldown; the
  agent-diagnostics overlay state label names the firing state's attack.

### Out of scope, and who owns each

- **Ranged / hitscan enemy attacks.** `weapon`-referencing attack entries, the shared
  weapon-descriptor substrate (the roadmap's "attacks are weapons/wieldables" model), nearest-of ray
  resolution, and its load-bearing prerequisite — making the player a first-class hitscan target,
  since the player is deliberately hitbox-less today (`content/dev/scripts/player.ts`) and enemy
  damage reaches it by direct apply to the selected target. Owned by a future **Epic 16 › Resolution
  Modes** combat-layer spec; design intent in `context/research/enemy-ranged-attacks.md`.
- **Windup / telegraph, commit (don't-bail-mid-attack), recovery.** These need a nested/scoped state
  layer so a brain can enter a phase it cannot be routed out of mid-swing. Owned by **Hierarchical
  behavior (statecharts)** (`roadmap.md`, Epic 10 — "a nested graph when a layer needs its own state
  (attack windup→commit→recover)").
- **Forced N>1 attack rotation** (fire attack A, then B, regardless of distance). A rotation counter
  is per-activity state the flat graph has no place to hold; it belongs to the same statechart layer
  and to **Epic 16 combat stances** (`roadmap.md`, Epic 10 statecharts entry — "the behavior
  substrate Epic 16's combat stances and planner build on").
- **Melee standoff with lunge** (park outside reach, then close under a committed impulse). The
  lunge is a combat↔movement impulse owned by **Epic 16 › Resolution Modes › melee** (`roadmap.md` —
  "melee and quick-melee with a lunge"); the commit that makes parking-then-closing safe is the
  statechart layer above.
- Damage synced to animation frames (damage stays cooldown-gated at fire time).
- Perception/LOS for target selection — the `visible` predicate stays `None`.
- **Friendly fire / enemy-on-enemy damage policy.** Enemy melee applies damage directly to the
  selected target, so it never strikes a bystander; enemy-on-enemy impacts become reachable only
  once ranged lands. Whether and how much an enemy's attack damages another is a per-game,
  per-faction-pair policy owned by the **Faction & relationship model** (`roadmap.md`, Epic 10);
  design intent in `context/research/enemy-aggro-model.md`.
- Leash/pursuit policy, squad coordination, faction (other slices of the behavior-descriptors
  thread).
- Stagger/pain interrupts (`E10--enemy-stagger`).

## Direction

**Problem.** The behavior graph can name only one attack (`components.behavior.attack`, a single
`AttackParams` block), so an archetype cannot mix two distinct melee attacks — a short fast jab and a
longer-reach slam — the way a Fiend-class enemy demands. The graph already routes between states by
distance; what it lacks is a way for different states to fire different attacks.

**Prior commitments.** The shipped graph makes `components.mesh.animations` a graph-wide named
vocabulary that states reference by name; this spec models `attacks` the same way, so the two read
alike. The statecharts successor (`roadmap.md`, Epic 10) commits to migrating graph-wide blocks
(`patrol` → per-activity `route`, `moveSpeed` → per-activity `speed`) into per-activity groupings
additively — a named `attacks` map is the endpoint that migration wants, not a shape it has to
unwind: statecharts later group the same named entries per activity. Keeping attacks instantaneous is
the load-bearing commitment: the roadmap places windup→commit→recover in the statechart layer, so an
instantaneous attack that never needs commit is exactly the slice the flat graph can own. Combat-slot
standoff (`E10--enemy-combat-positioning`, shipped) already resolves a per-agent `engagement_radius`;
per-attack standoff feeds that seam rather than adding one.

**Alternatives rejected.** A privileged `primary` attack plus an optional `secondary` would carry a
hidden default nobody authored and would not generalize past two attacks — the map costs nothing more
and generalizes to N. A per-tick "eligible attacks" selection pass (engine ranks the fireable attacks
and picks one) is the mechanism statecharts and combat stances exist to provide; adding it here would
build the selection engine this spec's whole framing says is unnecessary while attacks are
instantaneous. A graph-level engagement radius that the active attack cannot override (today's single
resolver) produces the "enemy walks to the ring and never swings" failure once a graph mixes a
long-reach slam with a short-reach jab, because one standoff cannot suit both.

## Acceptance criteria

- [ ] **AC1 — Parse validation.** A `components.behavior.attacks` map with two entries parses and
  validates identically in QuickJS and Luau. Rejections carry pathed, wire-cased errors in both: an
  empty `attacks` map when any state's action references it; an entry missing `damage`, `maxRange`,
  or `cooldownMs`; an entry with non-finite or out-of-range `damage` (`< 0`), `maxRange` (`<= 0`), or
  `cooldownMs` (`<= 0`); an entry whose authored `engagementRadius > maxRange`; an `action.attack`
  naming no entry. Validation is parse-time only — there are no spawn-time attack checks.
- [ ] **AC2 — Reference cadence preserved.** The reference enemy's melee tuning, expressed as a
  single-entry `attacks` map with `action: { attack: "..." }`, behaves exactly as the shipped singular
  `attack` block: same distance at which its swing connects, same damage per swing, same cooldown
  interval, same attack-clip replay — asserted as concrete numeric checks, not shape-parsing alone. The
  reference trace fixture (`trace_reference_fixture`/`BrainTrace`, `ai_tests.rs`), which runs the
  hand-authored reference oracle — asserted equal to the shipped Luau archetype — through a scripted
  approach and records per-tick `player_hp`/state/animation, is the vehicle: extend it to pin the
  connect distance, per-swing damage, and cooldown interval, rather than resting on suite-green alone.
  The trace runs the single-entry form only — the second attack never enters it. The pose-fixture
  enemy's migration holds the same parity.
- [ ] **AC3 — Two-reach routing.** On the movement-feel fixture, with the two-attack reference enemy:
  at a distance within only the longer-reach attack's reach the player takes that attack's damage
  once per that attack's cooldown with the hosting state's animation active; within the shorter-reach
  attack's reach the shorter attack's damage and animation apply instead — routing driven entirely by
  the authored per-state distance guards.
- [ ] **AC4 — Independent cooldowns.** Per-attack cooldowns are independent: firing one attack
  neither resets nor delays another's cooldown, and switching between two attack-firing states
  mid-cooldown leaves each attack's remaining cooldown untouched. Every attack's cooldown decrements
  every tick regardless of the current state — "untouched" means not re-armed, not frozen.
- [ ] **AC5 — Per-attack standoff.** The state firing an attack takes its combat-slot standoff from
  that attack's engagement radius (its authored `engagementRadius`, else its `maxRange`); a non-attack
  state (and an attackless graph) takes the graph-level `engagementRadius` default. An enemy whose
  active attack has a longer reach stands off at that reach rather than crowding to the shorter-reach
  ring.
- [ ] **AC6 — Cooldown fact.** `@brain.attackCooldownMs` reads the current state's attack's remaining
  cooldown, and reads 0 while the current state fires no attack or has no cooldown-map entry yet. On a
  tick where the state switches, the fact reflects the pre-transition (current) state's attack, fed
  before the transition is selected — it can differ from the attack that actually fires this tick.
- [ ] **AC7 — Deterministic selection.** When the graph's transitions could route to more than one
  attack state, declaration order wins (the first-true-wins evaluator guarantee); sim determinism
  tests stay green.
- [ ] **AC8 — Hold when no reach satisfied.** With no attack's reach currently satisfied (every
  attack's `maxRange` ceiling exceeded) but an attack-firing state still current, the enemy holds in
  that state facing the target (no authored transition guard is true yet).
- [ ] **AC9 — SDK typedef drift.** SDK typedef drift tests pass with `attacks`, the per-entry fields,
  and `action: { attack }` present in both `postretro.d.ts` and `postretro.d.luau` committed
  fixtures.
- [ ] **AC10 — Overlay label.** The agent-diagnostics overlay state label shows the firing state's
  attack name for an enemy in an attack-firing state.
- [ ] **AC11 — Replicated attack state.** Grep/review gate, not a runtime-asserted positive: confirm
  no serde/snapshot struct changed (no wire-format change), and that distinct attack states produce
  distinct replicated `current_state` names. A switch that persists at least one replication snapshot
  interval shows up as that name change; a switch that reverts within one interval aliases away — a
  stated sampling limit, the same one in-state clip restarts already have, not a defect to fix here.

## Tasks

### Task 1: Attack vocabulary and validation descriptors

In `postretro-foundation`, `crates/foundation/src/data_descriptors/types/behavior.rs`: replace
`BehaviorGraphDescriptor::attack: Option<AttackParams>` with `attacks: BTreeMap<String,
AttackParams>`. Rework `AttackParams` to `{ damage: f32, max_range: f32, cooldown_ms: f32,
engagement_radius: Option<f32> }` — renaming the base `range` role to `max_range` (the reach/damage
ceiling) and adding the optional per-attack `engagement_radius`. Every field is a scalar, so
`AttackParams` **keeps `#[derive(Copy)]`**. Do NOT touch `combat.rs` or `ResolutionMode`.
`ActionVerb::Attack(String)` drops `#[derive(Copy)]` from `ActionVerb` itself — a `String` payload
cannot be `Copy` — and makes the existing `const ActionVerb::ALL` array un-constructible with a
`String`-carrying variant; rework `ALL` and the `action_verb_all_is_exhaustive` walk to carry a
representative payload (or restructure the exhaustiveness guard so it no longer needs a `const`
array). `graph_eval::action_for_state`'s `Option<ActionVerb>` return now borrows or clones the small
`String` rather than copying it.
Parameterize `ActionVerb::Attack` as the newtype variant `Attack(String)`; the wire shape becomes
the object `action: { attack: "<name>" }` (externally-tagged serde wraps a newtype variant's payload
directly under the variant key — a struct variant would double-nest), so `ActionVerb::ALL`, the
`action_verb_all_is_exhaustive` successor-chain walk, and the round-trip test
(`the_wire_shape_round_trips_through_camel_case_json`, whose `"action": "attack"` becomes `"action":
{ "attack": "..." }`) all carry the field. Rework the engagement-radius resolver:
`BehaviorGraphDescriptor::engagement_radius()` loses its `self.attack.map(|a| a.range)` rung (there
is no singular block) and resolves the graph-level default only (`self.engagement_radius`, else
`DEFAULT_ENGAGEMENT_RADIUS`). Per-state standoff resolves separately, from the graph's `attacks` map
directly — every stat is inline, so no spawn-time table and no descriptor-slice threading are needed;
expose it as a descriptor-level resolution the combat-slot consumer (Task 3) reads. Pin the shape
both tasks share: for a firing state, standoff is
`attacks.get(name).engagement_radius.unwrap_or(max_range)`; a non-attack state, and an attackless
graph, take the graph-level default.

Add validation in `BehaviorGraphDescriptor::validate` (all pathed, wire-cased —
`components.behavior.attacks.claw.maxRange`): `attacks` is non-empty when any state declares the
attack action; every `action.attack` resolves to a map entry; each entry's `damage` is finite and
`>= 0`, `maxRange` finite and `> 0`, `cooldownMs` finite and `> 0`; an entry's authored
`engagementRadius`, when present, is `<= maxRange` (the reach constraint — see Invariants). Both
runtimes inherit parsing through the shared `behavior` serde funnel
(`crates/scripting-core/src/data_descriptors/js/entity.rs`, `lua/entity.rs` — verify neither needs a
per-runtime shim).

Migrate the descriptor-level unit tests in this task: `behavior.rs`'s own tests (`attack_numerics_*`, `the_attack_action_requires_*`,
`the_engagement_radius_resolves_*`, `position_goal_states_reject_actions_*`, the round-trip test) and
`crates/entities/src/components/brain.rs`'s `from_graph` test (`authored_graph()` and its callers,
including the `engagement_radius() == 2.0` assertion). State that `crates/postretro` is left
non-compiling — production `ai/mod.rs` reads `graph.attack` — until Task 3/4, so Task 1's completion
bar is `-p postretro-foundation -p postretro-entities` green.

### Task 2: Per-attack brain cooldown map

In `crates/entities/src/components/brain.rs`, replace `BrainComponent::attack_cooldown_remaining_ms`
(the single scalar) with a name-indexed cooldown map (`BTreeMap<String, f32>`, `#[serde(default)]`).
The cooldown is transient sim state re-armed on first fire, so a restored brain lacking the field
loads empty (every attack ready), consistent with the existing `target_reachable`/anchor defaults; a
lookup miss for the current state's attack reads 0 (ready), the same value `@brain.attackCooldownMs`
reports for it. The map is not pruned on a graph swap or re-seat: a same-named attack in a
newly-seated graph inherits its remaining sub-second timer (self-correcting within one cooldown), and
a name with no counterpart in the new graph is a harmless dead entry — clean-swap pruning is additive
if a consumer ever needs it. No weapon resolution and no derived tuning table: every stat is inline
on the descriptor.

Enumerate and convert every reader of the old scalar:

- `crates/postretro/src/scripting/systems/ai/mod.rs` — the per-tick decrement (which runs before the
  aggro-gate branch, so it must count **every** map entry down every tick regardless of the current
  state or the aggro gate), the fire gate's `<= 0.0` check, the re-arm on fire, and the
  `BrainFacts::attack_cooldown_ms` feed. (The AI-tick edits land in Task 3; Task 2 changes the field
  and the decrement mechanics the tick reads.)
- `crates/postretro/src/spawner.rs` — the spawn-time attack **windup**
  (`brain.attack_cooldown_remaining_ms.max(MAX_DELAY_MICROS as f32 / 1000.0)`), which stops a freshly
  spawned enemy attacking before remote interpolation's maximum delay elapses. An empty map reads
  0-ready and would let the enemy attack immediately, a regression — so the seed must populate a
  windup entry for **every attack the graph declares**. Migrate its test
  (`spawned_enemy_cannot_attack_before_interpolation_windup_expires`, asserting "the descriptor
  attachment must not overwrite the interpolation windup"), and extend it to a two-attack fixture
  whose spawned enemy routes into a second, non-initial attack state — proving the windup seed gates
  every attack the graph declares, not only the initial state's fired attack.
- `authored_graph()` in `brain.rs` (migrated in Task 1) and the AI/scope fixtures (migrated in Task
  4).

Task 2's completion bar is `-p postretro-entities` green; its `crates/postretro` edits (`spawner.rs`,
and the decrement mechanics in `ai/mod.rs`) stay non-compiling until Task 3, which restores that crate
to compiling.

### Task 3: Attack action verb firing, standoff, and brain-fact feed

Extend the attack seam in `crates/postretro/src/scripting/systems/ai/mod.rs` — the
`action_for_state` cooldown gate in the compute pass and the apply-pass `apply_damage_with_context` +
`ENEMY_ATTACK_EVENT`/`ENEMY_ATTACK_SOURCE_ID` + `restart_animation_clip`. Resolve the post-transition
(`next_index`) state's named attack from the graph's `attacks` map (via the `Attack(name)` payload
that `action_for_state` returns), gate on that attack's own cooldown-map entry and its `maxRange`,
apply its `damage` directly to the selected target (unchanged `DamageContext`), re-arm only that
attack's cooldown-map entry, and restart the in-state clip. No ray, no `nearest_entity_hit`, no
shooter-exclusion, no zone scaling — this is the shipped contact path, now keyed by name.

Feed `@brain.attackCooldownMs` from the current state's attack at the scope refresh
(`BrainFacts::attack_cooldown_ms`), resolving the current state from the reseated `current_index`
(the value `select_transition` walks from) rather than raw `brain.state_index`, which can name a
different (unaddressable) state on a reseat tick; keying off `current_index` keeps the fed value and
the transition source in agreement. It reads 0 when the current state fires no attack — the fact's
single-number shape is unchanged; only which attack's timer it reports generalizes across states. The
refresh runs before the tick's transition selects the next state, so the fed value is the
pre-transition (current) state's attack; the fire gate that follows reads the post-transition
(`next_index`) state's attack — on a switch tick these name different attacks by design (see Ordering
pins).

In `crates/postretro/src/scripting/systems/ai/combat_slots.rs`, resolve each engaged agent's standoff
from the firing state's attack — its authored `engagementRadius`, else its `maxRange` — instead of
the graph-wide `engagement_radius()`, falling back to the graph default for non-attack states and
attackless graphs. The firing state is `outcome.brain.state_index`: `resolve_combat_slots` runs after
the AI tick writes `brain.state_index = next_index`, so by the time combat slots read it the index is
already committed to the post-transition state — distinct from the brain-fact feed above, which reads
`current_index` before that same tick's transition runs. The two are not in tension: one reads before
the transition, the other after.

This is the first phase where `crates/postretro` compiles, so it also owns the SDK typedef work:
regenerate via `cargo run -p postretro --bin gen-script-types`, update the registrations in
`crates/postretro/src/scripting/primitives/mod.rs`, and update the committed fixtures under
`crates/postretro/src/scripting/typedef/tests/fixtures/` (drift-gated by `committed.rs`). That
registry is hand-declared, decoupled from the Rust enum: register `ActionVerb` as a struct —
`register_type("ActionVerb").field("attack", "String")` — which emits `{ attack: string }` with no
generator change needed; the `attacks` `Record` follows the existing `"BehaviorStates"` map-alias
registration precedent. AC9 (typedef drift) is verified here.

### Task 4: Reference archetype, overlay, fixture verification

In `sdk/behaviors/reference/entities.{ts,luau}`, this task proceeds in three separated steps, so the
second attack never perturbs AC2's numeric trace:

1. Migrate the reference enemy and the pose-fixture enemy from the singular `attack` block +
   `action: "attack"` to a **single-entry** `attacks` map + `action: { attack: "<name>" }`, holding
   each one's cadence (Task 2/3 preserve the contact path; this is a shape change, not a tuning
   change — the preserved entry keeps `damage: 8, maxRange: 2, cooldownMs: 1200`). Extend
   `trace_reference_fixture`/`BrainTrace` with concrete numeric assertions on connect distance,
   per-swing damage, and cooldown interval, and pin AC2's equivalence on this single-entry trace —
   migrating the fixture's shape is not enough on its own. Confirm the pose-fixture enemy's migration
   holds the same cadence and overlay-label parity.
2. Give the reference enemy its **second melee attack** (the deliverable): a second `attacks` entry
   with distinct reach/damage/cooldown (a longer-reach slam), a second attack-firing state whose
   distance-guard routing to and from the existing melee state fires it, and its animation-state
   added to `mesh.animations`. Author the reach guards so the shorter-reach attack is declared first,
   winning by first-true-wins when both reaches are satisfied (AC7). The spawner windup test (Task 2,
   extended to a second attack-firing state) now exercises this real second attack rather than a
   synthetic fixture. The KayKit knight model is pruned to one attack clip
   (`1H_Melee_Attack_Slice_Horizontal`), so the second state's animation reuses that clip through a
   distinct `mesh.animations` key — a distinct animation-state name (distinct replicated name and
   overlay label) backed by the same clip. (A genuinely distinct second-attack clip is a content
   dependency — see Open questions.)
3. Verify AC3's two-reach routing on the movement-feel fixture with the now-two-attack reference
   enemy — a fixture separate from step 1's trace, so the second attack never enters the AC2 trace.

Migrate every AI test fixture to the new shape: `ai/mod.rs` tests, `brain_scope.rs`, the
`reference_behavior_graph()` oracle (`ai_tests.rs`), and the scripting-core TS≡Luau twin
(`crates/scripting-core/src/data_descriptors/tests/behavior.rs`) — the shared `js_behavior`/
`lua_behavior` fixture templates, `both_runtimes_reject_actions_on_position_goals_and_accept_chase_actions`,
every `range:`/`action: "attack"` occurrence, and
`the_shipped_reference_enemy_descriptor_is_identical_in_both_authorings`. Rewrite the resolver tests
asserting `engagement_radius() == 2` "via the `attack.range` fallback" (the resolver-comment fixture
and the `== 2` assertions around it) to assert `DEFAULT_ENGAGEMENT_RADIUS` explicitly — once the
fallback rung is gone, passing via the coincidental 2.0 default is not a real assertion. Extend
`assemble_agent_overlay_label` (`crates/postretro/src/agent_diagnostics.rs`) with the firing state's
attack name (AC10).

Cover the remaining ACs with fixtures — AC4 and AC6 already state the ordering semantics these
fixtures assert:

- **AC3** — on the movement-feel fixture, verify the two-reach routing (long-reach slam vs. shorter
  slice), and confirm co-op clients show the distinct attack animation states via the replicated
  state name (no wire change expected — AC11).
- **AC4** — two attack-firing states: firing one leaves the other's remaining cooldown untouched
  (decrement-only, not reset or re-armed), and a mid-cooldown state switch leaves each entry
  untouched too (independent-cooldown and decrement-every-tick pins).
- **AC5** — an enemy in the long-reach attack-firing state parks at that attack's engagement radius,
  and a non-attack/attackless graph parks at the graph-level default.
- **AC6** — a fixture graph reads `@brain.attackCooldownMs` across an attack-firing state and a
  non-attack state, asserting both the reported value and the switch-tick semantics (pre-transition
  fact, post-transition fire).
- **AC7** — two attack-firing states whose transition guards are simultaneously true; resolution
  lands on the first-declared state (rides the shipped first-true-wins evaluator).
- **AC8** — an enemy in an attack-firing state with the target outside every attack's `maxRange`
  ceiling and no true transition guard holds in place and faces the target.

Task 4's completion bar is `-p postretro -p postretro-scripting-core` green — scripting-core is
exercised directly via its own migrated fixtures, not transitively through `postretro`.

## Sequencing

**Builds on shipped code:** the behavior state graph (`components.behavior`, the `attack` action
verb, the split `crates/postretro/src/scripting/systems/ai/` layout), combat positioning
(`CombatQuery::engagement_radius` in `combat_slots.rs`), and facing slew (`FACING_TURN_RATE`) — all
in `context/plans/done/`.

**Phase 1 (sequential):** Task 1 — the descriptor shape and validation everything else consumes; it
also falsifies the wire-shape and resolver assumptions the later tasks rest on. Bar:
`-p postretro-foundation -p postretro-entities`.
**Phase 2 (sequential):** Task 2 — consumes Task 1's descriptor shape; converts the brain cooldown
scalar to the name-indexed map and its spawn/decrement readers. Bar: `-p postretro-entities`; its
`crates/postretro` edits compile-verify in Task 3.
**Phase 3 (sequential):** Task 3 — consumes Task 2's cooldown map; wires firing, the brain fact, and
the per-attack standoff resolver, restoring `crates/postretro` to compiling. As the first phase where
that crate compiles, it also owns the SDK typedef regeneration, the `primitives/mod.rs`
registrations, and the committed typedef fixtures (AC9).
**Phase 4 (sequential):** Task 4 — exercises Task 3 end to end and migrates the reference content and
fixtures.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| attacks map | `BehaviorGraphDescriptor::attacks: BTreeMap<String, AttackParams>` | `"attacks"` | `attacks: Record<string, AttackParams>` | `attacks: {[string]: AttackParams}` | n/a |
| reach ceiling | `AttackParams::max_range: f32` | `"maxRange"` | `maxRange` | `maxRange` | n/a |
| attack damage | `AttackParams::damage: f32` | `"damage"` | `damage` | `damage` | n/a |
| cooldown | `AttackParams::cooldown_ms: f32` | `"cooldownMs"` | `cooldownMs` | `cooldownMs` | n/a |
| per-attack standoff | `AttackParams::engagement_radius: Option<f32>` | `"engagementRadius"` | `engagementRadius?` | `engagementRadius?` | n/a |
| graph default standoff | `BehaviorGraphDescriptor::engagement_radius: Option<f32>` | `"engagementRadius"` | `engagementRadius?` | `engagementRadius?` | n/a |
| action parameter | `ActionVerb::Attack(String)` | `action: { "attack": "<name>" }` | `action: { attack: string }` | `action: { attack: string }` | n/a |

Two distinct `engagementRadius` keys exist: one on each `attacks` entry (the firing state's standoff)
and one on the graph (the default for non-attack states). They never collide — one is nested under
`attacks.<name>`, the other is top-level under `behavior`.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Each attack's cooldown counts independently; firing or switching attacks touches only the fired attack's entry | Task 2 (name-indexed cooldown map), Task 3 (re-arm only the fired entry) | A shared or mis-keyed decrement would couple two attacks' timers | AC4 |
| An attack's engagement radius never exceeds its reach (`engagementRadius <= maxRange`) | Task 1 (parse-time check per entry) | An attack parked beyond `maxRange` can never land — closing the gap needs commit, which the flat graph cannot express; this is the statecharts boundary | AC1, AC5 |
| Attack selection is declaration-order deterministic | shipped first-true-wins evaluator; Task 4 authors ordered guards | Adding attack-firing states must not introduce a non-deterministic tiebreak | AC7 |

## Ordering pins

Each row is concrete enough to write a test from. Task 4's verification references these rows by name
rather than restating them.

| Scenario | Ordering | Expected outcome |
|---|---|---|
| A tick's transition switches from one attack-firing state to another | `@brain.attackCooldownMs` is fed from the pre-transition (current) state's attack at the scope refresh, before the transition selects the next state; fire and cooldown re-arm act on the post-transition (selected) state | The fact this tick names the old attack while the new attack is what actually fires (if ready) — the two differ on a switch tick, by design |
| An attack-firing state holds while a different attack's cooldown is still counting down from an earlier fire | Every cooldown-map entry decrements every tick, regardless of which state or attack is current | The idle attack's cooldown reaches 0 on schedule even though its firing state is never revisited in the meantime |
| A spawner-materialized enemy spawns with the player already in reach | `spawn_from_spawner` seeds a windup entry (`existing.max(MAX_DELAY_MICROS/1000)`, 250 ms) into the cooldown map for every attack the graph declares, after `attach_descriptor_components`; the per-tick decrement counts every entry down; the fire gate reads the post-transition attack's own entry | No attack lands until that attack's entry reaches 0, for whichever attack state the enemy routes into — including an attack its spawn/initial state never fires |
| A brain is materialized via `from_graph`/`attach_brain_graph` (map-install placement, a restore with an empty map, or a reseat introducing a new attack name) — no windup seed | A lookup miss reads 0 (ready) | The attack fires on its first eligible tick; the map gains an entry only once it re-arms |
| The post-transition state names an attack whose cooldown is ready | The fire gate evaluates exactly the post-transition state's named attack, once | At most one attack fires per enemy per tick, never two |
| An authored cooldown resolves to 0 or otherwise not `> 0` | Validated finite and `> 0` at parse time (Task 1) | Validation fails rather than producing a same-tick or every-tick refire from a non-positive cooldown; a positive sub-tick cooldown (`0 < cooldownMs < dt`) is a legal author choice that fires each eligible tick |
| The brain's aggro gate is closed (stood down) for several ticks | The cooldown-map decrement runs every tick, before the aggro-gate branch, independent of aggro state | Every attack's cooldown keeps counting down while stood down; a re-aggroing enemy may fire on its first re-armed tick |
| A hot reload or re-seat swaps the brain's behavior graph mid-play | The cooldown map is not pruned on the swap | A same-named attack in the new graph inherits its remaining sub-second timer (self-correcting within one cooldown); a name absent from the new graph is a harmless dead entry |
| The current tick both decrements cooldowns and fires/re-arms a ready attack | The fact is fed after this tick's decrement but before this tick's fire/re-arm | `@brain.attackCooldownMs` on the firing tick itself reports the post-decrement, pre-fire value, not the freshly re-armed cooldown |
| An enemy switches attack-firing states and switches back within one replication snapshot interval | Only `Transform` and the mesh animation-state name cross the wire, sampled once per snapshot interval | The round-trip switch never appears on the wire — the same sampling limit as in-state clip restarts (AC11) |
| Attack A fires and re-arms | Only A's cooldown-map entry is written | Every other attack's remaining cooldown is exactly what its own decrement schedule already produced — untouched by A's fire |

## Script syntax examples

```ts
// Proposed design
import { defineEntity, brain, runtime } from "postretro";

export const fiend = defineEntity({
  canonicalName: "fiend",
  components: {
    health: { max: 60 },
    mesh: { /* model, animation states incl. attack_jab / attack_slam */ },
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      // Graph-wide attack vocabulary; states reference entries by name, the way
      // they already reference `mesh.animations` keys.
      attacks: {
        // Short fast jab: crowds to its reach, low damage, quick cooldown.
        jab:  { damage: 6, maxRange: 1.8, cooldownMs: 700 },
        // Longer-reach slam: stands off farther, higher damage, slower.
        slam: { damage: 14, maxRange: 3.5, cooldownMs: 1800, engagementRadius: 3.5 },
      },
      // Default standoff for non-attack states (e.g. `chase`).
      engagementRadius: 8,
      states: {
        idle: {
          animation: "idle", motion: "hold",
          transitions: [{ to: "chase", when: runtime.le(brain.targetDistance, 16) }],
        },
        chase: {
          animation: "walk", motion: "chaseTarget",
          transitions: [
            // Shorter reach declared first: at close range both are true and jab wins.
            { to: "attack_jab", when: runtime.le(brain.targetDistance, 1.8) },
            { to: "attack_slam", when: runtime.le(brain.targetDistance, 3.5) },
          ],
        },
        attack_jab: {
          animation: "attack_jab", motion: "chaseTarget", action: { attack: "jab" },
          transitions: [{ to: "chase", when: runtime.gt(brain.targetDistance, 1.8) }],
        },
        attack_slam: {
          animation: "attack_slam", motion: "chaseTarget", action: { attack: "slam" },
          transitions: [
            { to: "attack_jab", when: runtime.le(brain.targetDistance, 1.8) },
            { to: "chase", when: runtime.gt(brain.targetDistance, 3.5) },
          ],
        },
      },
    },
  },
});
```

## Open questions

- **Distinct second-attack clip.** The reference enemy's KayKit knight model is pruned to one attack
  clip (`1H_Melee_Attack_Slice_Horizontal`), so the second melee attack reuses it through a distinct
  `mesh.animations` key — a distinct animation-state name (distinct replicated name and overlay
  label) backed by the same visible swing. A genuinely distinct second swing is a content dependency:
  re-prune the KayKit knight to include another melee clip. Not this spec's work. **Owner:** content
  pass, if a distinct swing is wanted.
- **Repeat-attack replication.** Re-firing the same attack restarts the clip via
  `restart_animation_clip`, which changes no state name and so produces no wire delta — remote clients
  see the first swing clamp. Pre-existing gap (single-attack enemies have it today); distinct attacks
  mask it, since routing between them does change the replicated state name. A wire restart signal is
  future netcode work. **Owner:** netcode.
- **Authored floor-guard thrash.** A far-reach enemy chasing the target's center point can drive
  itself inside an authored floor guard (a `ge(@brain.targetDistance, …)` transition condition) and
  oscillate between attack states. Per-attack engagement radius mitigates this — the enemy stands off
  at the firing attack's radius rather than closing to center — but a graph whose authored floor guard
  sits shorter than the neighbouring attack's standoff can still chatter. Playtest whether authored
  floor guards need a hysteresis margin; do not add one ahead of an observed thrash. **Owner:** this
  spec's playtest, escalating to combat-positioning if it recurs.
