# Enemy Stagger

## Goal

Quake-style pain interrupts as an engine API: taking enough damage knocks an enemy out of its current action into a Stagger state — pain animation, movement halted, facing frozen — for a descriptor-tuned duration, then it resumes. Opt-in per archetype via a `components.ai.stagger` block; enemies without one never stagger. Enemies only in v1.

## Scope

### In scope

- `components.ai.stagger` descriptor block: damage threshold, stagger duration, re-stagger cooldown, pain animation state. Absent block = no stagger, behavior identical to today.
- `LogicalState::Stagger` — a new latch layered outside the pure FSM, mirroring the death latch: forced entry on qualifying damage, descriptor-tuned ms countdown on `BrainComponent`, normal FSM resumes on expiry.
- A damage-taken accumulator on `BrainComponent`, written at the `apply_damage` chokepoint, consumed and cleared by the AI tick. Captures every damage source (player weapon stage, enemy melee, the `applyDamage` script reaction) between consecutive AI evaluations.
- Trigger rule: accumulated damage since the last AI evaluation ≥ threshold → stagger. Checked every tick (not think-strided), after the death check — death always wins.
- Stun-lock guard: after a stagger entry, no re-entry until the re-stagger cooldown elapses. While staggered, further qualifying damage does nothing (no clip restart, no timer extension) — this also keeps remote clients exact, since only state-name changes replicate.
- During Stagger: destination cleared (agent decelerates and halts), facing arbitration writes nothing, target retention unchanged. On expiry the normal FSM re-evaluates from the retained target — a chase resumes within a tick.
- `enemyStagger` named tick event (the `enemyAttack` precedent) so mods, and later audio, can bind reactions.
- Reference enemy archetype gains a stagger block and pain clip mapping.

### Out of scope

- Player stagger / view punch / hit feedback (view-feel surface, separate spec).
- Knockback impulse and damage types — Epic 16 "Damage & Defenses" kin; this spec must not claim `DamagePayload` fields.
- Chance-based pain rolls (Quake's random pain) — needs a seeded deterministic RNG story; threshold-only in v1.
- IR-authored stagger policy — the combat `BindingScope` is Epic 16's `CombatScope` adopter; v1 ships plain descriptor scalars, upgradeable additively to `NumberOrIr` per the dash precedent.
- Damage-based aggro (getting shot from beyond detection range does not force acquisition — perception-model territory; on expiry the enemy returns to whatever the FSM computes).
- Interrupting attack windups — no windups exist; the animation interrupt is the whole effect.
- Combat-events emission (`onImpact`/`onDamage` facts) — the accumulator is deliberately minimal and private to the brain; the events schema stays with `context/research/combat-events.md`.

## Acceptance criteria

- [ ] A descriptor with a stagger block parses and validates identically in QuickJS and Luau; rejections carry pathed errors in both: non-finite or ≤ 0 threshold, ≤ 0 duration, < 0 cooldown, empty animation state name. An unknown pain animation warns once at spawn like other unmapped states. SDK typedef drift tests pass with `stagger` in both committed fixtures.
- [ ] Descriptors without a stagger block behave bit-for-bit as today: the full existing AI test suite passes unchanged.
- [ ] A single hit ≥ threshold on an Alert or Attack enemy enters Stagger: the pain animation state becomes current, the agent's destination is cleared and it decelerates to a halt, and its facing stays fixed for the full duration.
- [ ] Two sub-threshold hits landing between consecutive AI evaluations whose sum meets the threshold also trigger stagger; a single sub-threshold hit never does, and the accumulator does not leak across evaluations (a hit followed by a quiet tick, then another sub-threshold hit, does not trigger).
- [ ] All three damage paths trigger it: player hitscan, another entity's melee, and the `applyDamage` script reaction.
- [ ] Duration expiry resumes behavior within one tick: a retained in-range target is chased again (Alert/Attack per distance); with no valid target the enemy returns to Idle.
- [ ] A lethal hit enters Death, never Stagger, even when it also meets the threshold; an enemy dying mid-stagger transitions to Death immediately.
- [ ] While staggered and during the re-stagger cooldown, qualifying damage causes no state change, no clip restart, and no timer change; after the cooldown, a qualifying hit staggers again.
- [ ] Attack cooldowns keep ticking through Stagger (an enemy staggered mid-cooldown does not get a free reset in either direction).
- [ ] The `enemyStagger` event appears in the tick's AI events exactly once per stagger entry.
- [ ] Sim determinism tests stay green; on a connected client, a staggered host enemy shows the pain state via the replicated animation state name with no wire-format change.

## Tasks

### Task 1: Stagger descriptor surface

Add `StaggerDescriptor` and `stagger: Option<StaggerDescriptor>` to `AiDescriptor` in `crates/foundation/src/data_descriptors/types/combat.rs`, fields per the boundary inventory (all required within the block; the block itself optional). Validation in the shared `AiDescriptor::validate()` with wire-cased paths (`components.ai.stagger.damageThreshold` …). The closed `states` block is untouched — the pain animation name lives in the stagger block. Both script runtimes inherit the field through the shared serde + `validate()` funnel (verify `js/entity.rs` / `lua/entity.rs` need no shim). Regenerate SDK typedefs (`sdk/types/postretro.d.ts`, `.d.luau`) and the committed typedef fixtures.

### Task 2: Damage-taken accumulator

`BrainComponent` gains `stagger_damage_accum: f32` (serde-default 0). `apply_damage` in `crates/entities/src/components/health.rs` — the single chokepoint all damage flows through — additionally adds the payload amount to the target's brain accumulator when the target carries a `BrainComponent` (no-op otherwise; same registry, same call). Its three production callers (player weapon stage in `sim/mod.rs`, the AI melee apply pass, the `applyDamage` reaction in `health/reactions.rs`) need no signature change. The AI tick reads and zeroes the accumulator for each brain entity at the top of its per-entity evaluation, so the value means "damage since this entity's last evaluation" — unit-test all three caller paths and the no-leak property. This is a private brain-side tally, not an event record; the combat-events ledger (`context/research/combat-events.md`) later builds richer facts at the same chokepoint without colliding.

### Task 3: The Stagger latch

Extend `LogicalState` with `Stagger` (`ALL`, `label`, serde all extended; the set stays engine-closed). `BrainComponent` gains `stagger_remaining_ms: Option<f32>` and `stagger_cooldown_remaining_ms: f32` (serde-default). In `run_ai_tick`, after the every-tick death check and before the normal FSM: if not in Death, cooldown elapsed, and the drained accumulator ≥ threshold, force `state = Stagger`, seed the duration timer, arm the re-stagger cooldown, emit `SteeringIntent::Clear`, switch the animation to the stagger block's state (spawn-validated via `validate_brain_animation_states`), and push the `enemyStagger` event. While `Stagger`: decrement the timer, emit no steering, skip the facing block (a new no-write arbitration case alongside Idle/Death), hold `acquired_target`, and let `attack_cooldown` decrements run as they already do unconditionally. On expiry, clear the latch and let the existing FSM path recompute state from the retained target that same tick (the death-recovery precedent). Death checked mid-stagger preempts. Animation resolution for the new state reads the stagger tuning, not `AiStateMap` — extend the tuning carried by `AiTuning` with the resolved stagger block. Cover transitions, timers, precedence, and event emission in the AI test suite.

### Task 4: Reference archetype and verification

Give the reference enemy a stagger block (threshold meaningfully above its per-hit chip damage from a single pistol round, so staggering requires the heavier weapon or accumulation) and map a pain clip in its mesh animations with a `snap` interrupt policy so the flinch cuts hard. Verify on the movement-feel fixture: shoot an approaching enemy — it flinches, halts, resumes; sustained light fire does not chain-stun (cooldown observable). Confirm the client-side pain state on a loopback multiplayer session via the replicated state name.

## Sequencing

**Phase 1 (concurrent):** Task 1 (descriptor crate), Task 2 (entities crate) — independent files.
**Phase 2 (sequential):** Task 3 — consumes Task 1's tuning shape and Task 2's accumulator; lands in the split `ai.rs` layout.
**Phase 3 (sequential):** Task 4 — exercises Task 3 end to end.

Cross-spec: shares the `ai.rs` split with `E10--enemy-multi-attack` Task 1 — whichever spec runs first executes that split; if this one runs first, prepend it as Phase 0 (behavior-preserving, full suite green). Runs after `E10--enemy-facing-slew` lands (both edit the facing block; stagger adds a no-write case to arbitration that spec's tests pin).

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| stagger block | `AiDescriptor::stagger: Option<StaggerDescriptor>` | `"stagger"` | `stagger?: StaggerDescriptor` | `stagger: StaggerDescriptor?` | n/a |
| threshold | `StaggerDescriptor::damage_threshold` | `"damageThreshold"` | `damageThreshold` | `damageThreshold` | n/a |
| duration | `StaggerDescriptor::duration_ms` | `"durationMs"` | `durationMs` | `durationMs` | n/a |
| re-stagger cooldown | `StaggerDescriptor::cooldown_ms` | `"cooldownMs"` | `cooldownMs` | `cooldownMs` | n/a |
| pain animation | `StaggerDescriptor::animation_state` | `"animationState"` | `animationState` | `animationState` | n/a |
| logical state | `LogicalState::Stagger` | `"stagger"` | n/a (engine-internal) | n/a | n/a |
| tick event | `ENEMY_STAGGER_EVENT` | `"enemyStagger"` | `"enemyStagger"` | `"enemyStagger"` | n/a |

## Script syntax examples

```ts
// Proposed design
export const grunt = defineEntityType({
  // ...
  components: {
    health: { max: 60 },
    ai: {
      detectionRange: 16, attackRange: 1.8, attackDamage: 10, attackCooldownMs: 1200,
      leashRange: 50, moveSpeed: 3.2, deathDespawnMs: 1200,
      stagger: { damageThreshold: 15, durationMs: 450, cooldownMs: 2000, animationState: "pain" },
      states: { idle: "idle", alert: "walk", attack: "attack", death: "death" },
    },
  },
});
```

## Open questions

- **Chance-based pain.** Quake rolls pain probabilistically; deterministic threshold is the v1 stand-in. If design wants variance, it needs the engine's seeded-RNG story first — revisit alongside Epic 16 damage work.
- **Stagger while Idle.** Qualifying damage staggers an Idle enemy, but on expiry it returns to Idle if the attacker is outside detection range (no damage-based aggro yet). Accepted v1 oddity; the perception model owns the fix.
