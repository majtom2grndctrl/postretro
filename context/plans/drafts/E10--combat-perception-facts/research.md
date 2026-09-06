# Combat Perception Facts — Research

Derivation notes, lifecycle, orderings, and code-grounding for the spec. Not
consumed by task agents directly; the spec's task paragraphs carry what they
need.

## Lifecycle — a hit becomes a graph reaction

Damage lands after the AI compute pass within a tick, so the graph reads the
memory one tick later. This mirrors how attack cooldowns are aged in compute and
consumed the next tick.

```mermaid
sequenceDiagram
    participant W as Weapon/AI-melee (stage 8 / AI apply)
    participant CK as apply_damage_with_context (entities/health.rs)
    participant B as BrainComponent
    participant AI as AI compute pass (stage 5, next tick)
    participant G as Behavior graph (guards)

    Note over AI: Tick N, stage 5 — age timers by dt_ms, cache last-known if visible, eval graph
    W->>CK: damage to enemy (attacker id in DamageContext)
    CK->>B: time_since_damage_ms = 0
    CK->>B: last_known_target_pos = attacker Transform.position
    CK->>B: damage_bearing = yaw(enemy facing, dir→attacker)
    Note over AI: Tick N+1, stage 5
    AI->>B: time_since_damage_ms += dt_ms  (≈16ms), clamp at sentinel
    AI->>B: if target_visible: last_known = target.position; time_since_target_visible = 0<br/>else: time_since_target_visible += dt_ms
    AI->>AI: distance_to_last_known = last_known ? distance_xz(pos, last_known) : sentinel
    AI->>G: BrainScope.refresh → guards read the 4 facts
    G->>G: patrol→investigate / engage→investigate / startle …
```

Stage numbers are `entity_model.md` §5: AI brain tick is stage 5; weapon reload
and fire tick is stage 8. Enemy melee applies within the AI tick; the
`applyDamage` script reaction resolves at the frame-end drain. All three route
through `apply_damage_with_context`.

## Orderings

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Hit lands stage 8, AI compute ran stage 5 | damage after compute, same tick | 1-tick latency: graph sees `timeSinceDamageMs ≈ dt_ms` next tick. Acceptable (~16ms). |
| Two hits between consecutive AI ticks | both reset the timer to 0; last write wins for `last_known`/`damage_bearing` | Timer reads ~`dt_ms` next tick; memory/bearing reflect the most recent attacker. |
| Target visible **and** enemy hit same tick | compute-pass visible-cache (stage 5) writes `last_known = target.pos`; damage-seed (stage 8) overwrites with attacker.pos | Next visible tick re-caches target.pos. Seed only "sticks" while unseen. Attacker == target ⇒ identical, no observable difference. |
| Catch-up: N fixed ticks one frame | each tick ages timers by its own `dt_ms` | Aging is per-tick, not per-frame; recency stays wall-consistent. |
| Duration/timer authored at 0-ish window | `timeSinceDamageMs.le(0)` | Effectively never true after the reset tick ages it; author picks a real window. Spec's constants illustrate. |
| Long session | timer aged unbounded | Clamp at the sentinel prevents overflow / wrap; a clamped timer still reads "long ago." |
| `moveToLastKnown` with empty memory | `last_known_target_pos == None` | Steering `Clear` (hold); `distanceToLastKnown` = sentinel; no crash, enemy stands and plays locomotion anim. |
| Lost target then hit by a new attacker | target dropped, `last_known` seeded from attacker | `distanceToLastKnown` jumps to the attacker's spot; `investigate` re-triggers. |
| Reached the spot, then re-shot from same origin | walked to spot ⇒ `distanceToLastKnown ≈ 0`; new hit re-seeds ≈ current position | No re-investigate loop (distance stays small); if the shooter is now in sight, `targetVisible` → engage resolves it. |

## Design derivation

**Why memory, not perception override.** `entity_model.md` §7c: candidacy can
only narrow the offer set. So an authored graph cannot perceive an attacker it
couldn't otherwise sense — the floor must supply something. Two floor options:
(a) force the attacker into the perceived/selectable set for a window; (b) store
where the stimulus came from as a position the graph investigates. (b) reuses
the shipped `moveToAnchor` position-goal path, avoids wallhack tracking, and
keeps the *reaction* authored (the graph decides to investigate). Chosen.

**Why `distanceToLastKnown` doubles as the consumption signal.** A level fact
(`timeSinceDamageMs` low) would re-trigger `patrol → investigate` forever after a
give-up. Gating `patrol → investigate` on `distanceToLastKnown.gt(ARRIVE)`
("there is an unreached spot") and `investigate → patrol` on
`distanceToLastKnown.le(ARRIVE)` ("reached it") makes reaching the spot consume
the alert, and a new hit (which re-seeds the memory) re-arm it — no engine pulse
or latch, unlike `E10--enemy-stagger`'s one-tick `@state.staggered` flag.

**Why turn-to-face needs no bearing fact, yet `damageBearing` still ships.**
Once an enemy engages or investigates, facing slews toward the target/goal, so
"turn toward the shooter" is emergent. `damageBearing` earns its slot only for
choosing a *directional* reaction (a left vs. right vs. rear startle) before or
without engaging. The owner elected to ship it here; the full directional-pain
clip set stays with `E10--enemy-stagger`.

## Code grounding (verified against current tree)

Line numbers are ephemeral; identifiers and file homes are the durable anchors.

**Fact table (append-only).** `crates/foundation/src/brain.rs` — `BRAIN_INPUTS:
[(&str, IrType); 15]` (~:116). Prefix `BRAIN_INPUT_PREFIX = "@brain."`. Sentinel
`BRAIN_NO_TARGET_DISTANCE: f32 = 1.0e9`. Per-slot index-stability tests exist
(e.g. slot 14 `targetVisible`). Validation twin `BrainValidationScope`;
`resolve_brain_input`. Adding a fact: new `BRAIN_*_INPUT` const + tail entry +
length bump + slot test + validation-twin entry.

**Live scope.** `crates/postretro/src/scripting/systems/ai/brain_scope.rs` —
`struct BrainScope { fixed: [IrValue; 15], … }`; `BrainFacts` struct
(`distance_from_anchor: f32` is the mirror for `distance_to_last_known`);
`refresh` writes all fixed slots in `BRAIN_INPUTS` order; `expected_fixed_value`
test has a no-`_` match arm (compile tripwire on a new slot).

**Compute pass.** `crates/postretro/src/scripting/systems/ai/mod.rs` — entry
`run_ai_tick_with_navigation_and_impact(registry, runtime, tick_dt, inputs,
on_impact)`; `dt_ms = tick_dt.max(0.0) * 1000.0` (~:474); cooldown aging (~:655)
and `brain.tick_activity_timers(dt_ms)` (~:664) are the aging-site precedents.
`distance_from_anchor = nav::distance_xz(snap.position, brain.home_anchor)`
(~:562) is the compute-site precedent. Debounced `target_visible` verdict and
the selected target's world position (`selected_target`'s third tuple element,
`target.position`) resolve ~:673–705 — the memory-cache/age site. Two `refresh`
calls (~:759 zero-cooldown seed, ~:783 final) both take the `BrainFacts`.

**Position-goal steering.** `position_goal_steering(motion, &mut BrainComponent,
position) -> SteeringIntent` (mod.rs ~:161) — the `MoveToAnchor` arm
(arrival→`Clear`, else `MoveTo(home_anchor)`, epsilon
`POSITION_GOAL_ARRIVAL_EPSILON = 0.5` in `engine_floor.rs`) is the exact mirror
for a `MoveToLastKnown` arm. `steering_for(motion)` (`graph_eval.rs` ~:336) is
also exhaustive and must gain the arm.

**Non-engaged gates (all four must list `MoveToLastKnown`).**
`crates/foundation/src/data_descriptors/types/behavior/recursive.rs` (~:410,
action-on-position-goal rejection); `graph_eval.rs` `action_for_path` (~:135),
`activity_can_engage` (~:177), `is_locomotion_activity` (~:328).

**Motion verb enum.** `crates/foundation/src/data_descriptors/types/
behavior.rs` — `enum MotionVerb { ChaseTarget, MoveToAnchor, Patrol, Hold,
Freeze }` (5 variants; `Freeze` included) with `MotionVerb::ALL: [MotionVerb; 5]`
(hand-maintained; length bumps to 6). SDK registration
`crates/postretro/src/scripting/primitives/mod.rs` `register_enum("MotionVerb")`
(~:472) and the `moveSpeed` doc's nav-verb list (~:591).

**Damage chokepoint.** `crates/entities/src/components/health.rs` —
`apply_damage_with_context(registry: &mut EntityRegistry, id, payload:
&DamagePayload, context: DamageContext) -> bool` (~:424). `DamageContext {
source_id, attacker: Option<EntityId>, weapon, zone, producer }` (~:58) —
attacker **identity** present, **no position/direction**. `DamagePayload {
amount }` only. Read attacker Transform via `registry.get_component::<Transform>
(attacker)` (same idiom as `targeting.rs:57`). Mutate the brain via
clone-mutate-`set_component` (precedent `health.rs:436/459`) or
`get_component_value_mut(id, ComponentKind::Brain)`. `BrainComponent` is same
crate (`crate::components::brain`).

**BrainComponent.** `crates/entities/src/components/brain.rs` — `#[derive(…,
Serialize, Deserialize)]`, no container `#[serde(default)]`; each field opts in.
`home_anchor` uses `#[serde(default = "default_home_anchor")]`; `acquired_target:
Option<EntityId>` / `combat_slot: Option<Vec3>` use bare `#[serde(default)]` →
`None`; `target_reachable` uses `#[serde(skip)]` (per-tick cache). `from_graph`
constructs field-by-field (no `..Default::default()`) — every new field needs an
explicit initializer. Legacy-defaults test template:
`deserializing_a_pre_anchor_brain_defaults_to_the_origin`; round-trip via
`ComponentValue::Brain`. New fields: `last_known_target_pos: Option<Vec3>` (bare
default `None`); `time_since_damage_ms` / `time_since_target_visible` (named
default helper returning the never-sentinel, **not** `0.0`); `damage_bearing:
f32` (bare default `0.0`).

**SDK guard-fact surface.** Static template (not registry-generated):
`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` `interface
BrainInputs` (~:954) and `sdk_lib.luau` (~:1360). Committed copies
`sdk/types/postretro.d.ts` / `.d.luau`; fixtures
`crates/postretro/src/scripting/typedef/tests/fixtures/expected.d.{ts,luau}`
checked by `committed.rs` / `surface.rs`.

**Reference enemy.** `content/dev/scripts/reference-enemy.ts` +
`reference-enemy.luau` kept byte-equal after canonical conversion by the
scripting-core twin parser test. Already uses `motion: "moveToAnchor"` and
`distanceFromAnchor` guards — the natural fixture to extend.

**Facing source for `damageBearing`.** `crates/postretro/src/scripting/systems/
ai/facing.rs` — confirm the enemy's visual facing lands on `Transform.rotation`
(the value read at the chokepoint) before relying on it for the yaw snapshot.

## Source sizes (split-before-extend gauge)

`ai/mod.rs` 1208 · `graph_eval.rs` 736 · `brain_programs.rs` 730 ·
`brain_scope.rs` 811 (~540 tests) · `targeting.rs` 461 · `perception.rs` 305 ·
`ai_tests.rs` 9270 (test file) · `impact_policy.rs` 3289 · `entities/health.rs`
(chokepoint). `mod.rs` at 1208 is the one file this spec meaningfully extends;
additions are additive and mirror existing patterns (Open Questions).
