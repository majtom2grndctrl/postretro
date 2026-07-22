# Enemy Stagger

## Goal

Quake-style pain interrupts, authored on the behavior state graph (`E10--behavior-state-graph`): enough accumulated damage flags an enemy for stagger; an authored interrupt carries it into a flinch state — pain animation, movement halted, facing frozen — for a commitment window, then it resumes. Opt-in per archetype: an archetype without a `stagger` tuning block and a flinch state/interrupt never staggers. Enemies only in v1.

## Scope

### In scope

- `stagger` tuning sub-block on `components.behavior`: damage threshold, re-stagger cooldown. Absent block = no stagger, behavior identical to today. Duration is not engine tuning — it's an authored commitment-window guard (`@brain.timeInStateMs`) on the flinch state itself.
- A damage-taken accumulator on `BrainComponent`, written at the `apply_damage` chokepoint, consumed by the AI tick before graph evaluation. Captures every damage source (player weapon stage, enemy melee, the `applyDamage` script reaction) between consecutive AI evaluations.
- Trigger rule: accumulated damage since the last AI evaluation ≥ threshold, and the re-stagger cooldown elapsed → the engine writes `@state.staggered` (`registry.entity_state_mut`) as a one-tick pulse, then clears it the following tick. Checked every tick (not think-strided), after the death check — death always wins. This is the only engine-authored write; everything downstream is graph content.
- An authored `interrupts` entry reading `@state.staggered` carries any state into a flinch state — no forced `LogicalState`, no engine-side latch. Interrupts evaluate before state-local transitions (behavior-state-graph invariant), so a pulse on the tick it fires always wins.
- An authored flinch state: pain animation, `hold` motion (destination cleared, agent decelerates to a halt), `onEnter: "enemyStagger"`, and a commitment-window transition back to a resume state gated on `@brain.timeInStateMs` — the archetype author picks resume target and window length.
- Stun-lock guard: after a pulse fires, no re-pulse until the re-stagger cooldown elapses (engine-owned, since the threshold check that produces the pulse is engine-owned). While on cooldown or already in the flinch state, further qualifying damage still accumulates but produces no pulse — no clip restart, no timer extension. Keeps remote clients exact, since only state-name changes replicate.
- Reference enemy's authored graph gains a `stagger` tuning block, a flinch state, and the interrupt wiring.

### Out of scope

- Player stagger / view punch / hit feedback (view-feel surface, separate spec).
- Knockback impulse and damage types — Epic 16 "Damage & Defenses" kin; this spec must not claim `DamagePayload` fields.
- Chance-based pain rolls (Quake's random pain) — needs a seeded deterministic RNG story; threshold-only in v1.
- IR-authored stagger tuning — the accumulator threshold and cooldown ship as plain descriptor scalars (the guard that reads `@state.staggered` is already IR; the scalars that produce the pulse are not), upgradeable additively to `NumberOrIr` per the dash precedent once the combat `BindingScope` (Epic 16's `CombatScope`) lands.
- Damage-based aggro (getting shot from beyond detection range does not force acquisition — perception-model territory; on resume the enemy re-enters normal evaluation and goes wherever the graph computes).
- Interrupting attack windups — no windups exist; the animation interrupt is the whole effect.
- Combat-events emission (`onImpact`/`onDamage` facts) — the accumulator is deliberately minimal and private to the brain; the events schema stays with `context/research/combat-events.md`.
- Splitting `scripting/systems/ai.rs` — owned by `E10--behavior-state-graph` Task 1. This spec lands after that split.

## Acceptance criteria

- [ ] A `components.behavior.stagger` block parses and validates identically in QuickJS and Luau; rejections carry pathed errors in both: non-finite or ≤ 0 threshold, < 0 cooldown. SDK typedef drift tests pass with `stagger` in both committed fixtures.
- [ ] Archetypes without a `stagger` block behave bit-for-bit as today: the full existing AI test suite passes unchanged.
- [ ] A single hit ≥ threshold on an enemy in any state pulses `@state.staggered` and the authored interrupt fires that same tick: the flinch state's animation becomes current, the agent's destination is cleared and it decelerates to a halt, and facing stays fixed for the commitment window.
- [ ] Two sub-threshold hits landing between consecutive AI evaluations whose sum meets the threshold also trigger a pulse; a single sub-threshold hit never does, and the accumulator does not leak across evaluations (a hit followed by a quiet tick, then another sub-threshold hit, does not trigger).
- [ ] All three damage paths trigger it: player hitscan, another entity's melee, and the `applyDamage` script reaction.
- [ ] The commitment-window guard fires on the first tick `@brain.timeInStateMs` elapses and never before; the authored resume transition re-enters normal evaluation within that tick — a retained in-range target is chased again.
- [ ] A lethal hit's death interrupt wins over a simultaneous stagger pulse (interrupt declaration order); an enemy dying while in the flinch state transitions to death immediately.
- [ ] While in the flinch state and during the re-stagger cooldown, qualifying damage produces no pulse, no clip restart, and no timer change; after the cooldown, a qualifying hit pulses again.
- [ ] Attack cooldown state keeps ticking through the flinch state (an enemy staggered mid-cooldown does not get a free reset in either direction).
- [ ] The `enemyStagger` event (the flinch state's `onEnter`) appears in the tick's AI events exactly once per stagger entry.
- [ ] Sim determinism tests stay green; on a connected client, a staggered host enemy shows the pain state via the replicated animation state name with no wire-format change.

## Tasks

### Task 1: Stagger tuning block

Add a `stagger: Option<StaggerTuning>` field to `BehaviorGraphDescriptor` in `postretro-foundation` (`data_descriptors/types/`), fields per the boundary inventory (both required within the block; the block itself optional). Validation alongside the rest of `BehaviorGraphDescriptor`'s structural checks, wire-cased paths (`components.behavior.stagger.damageThreshold` …). Both script runtimes inherit the field through the shared `behavior` parsing funnel established by `E10--behavior-state-graph` Task 2 (verify `js/entity.rs` / `lua/entity.rs` need no shim). Regenerate SDK typedefs (`sdk/types/postretro.d.ts`, `.d.luau`) and the committed typedef fixtures.

### Task 2: Damage-taken accumulator and stagger pulse

`BrainComponent` gains `stagger_damage_accum: f32` and `stagger_cooldown_remaining_ms: f32` (serde-default 0) as engine substates, alongside the ones `E10--behavior-state-graph` Task 4 already carries forward. `apply_damage` in `crates/entities/src/components/health.rs` — the single chokepoint all damage flows through — additionally adds the payload amount to the target's brain accumulator when the target carries a `BrainComponent` (no-op otherwise; same registry, same call). Its three production callers (player weapon stage in `sim/mod.rs`, the AI melee apply pass, the `applyDamage` reaction in `health/reactions.rs`) need no signature change. Before BrainScope `refresh` each per-entity AI evaluation: drain the accumulator; if the archetype has a `stagger` block, cooldown has elapsed, and the drained amount ≥ threshold, write `@state.staggered` true via `registry.entity_state_mut`, arm the cooldown; otherwise write it false, so the flag is a one-tick pulse the authored interrupt observes and the engine clears without graph-side help. Unit-test all three caller paths, the no-leak property, the pulse-then-clear shape, and the cooldown gate.

### Task 3: Reference archetype and verification

Give the reference enemy a `stagger` block (threshold meaningfully above its per-hit chip damage from a single pistol round, so staggering requires the heavier weapon or accumulation), a flinch state (pain animation, `hold` motion, `onEnter: "enemyStagger"`, commitment-window transition back to `chase`), and the `@state.staggered` interrupt. Verify on the movement-feel fixture: shoot an approaching enemy — it flinches, halts, resumes; sustained light fire does not chain-stun (cooldown observable). Confirm the client-side pain state on a loopback multiplayer session via the replicated state name.

## Sequencing

**Depends on:** `E10--behavior-state-graph` — needs `components.behavior`, `interrupts`, the `@state.*` guard input, the flinch state's `onEnter` event plumbing, and the split `ai.rs` layout in place first.

**Phase 1 (concurrent):** Task 1 (foundation descriptor), Task 2 (entities crate) — independent files.
**Phase 2 (sequential):** Task 3 — exercises Task 1 and Task 2 end to end on the reference graph.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| stagger tuning block | `BehaviorGraphDescriptor::stagger: Option<StaggerTuning>` | `"stagger"` | `stagger?: StaggerTuning` | `stagger: StaggerTuning?` | n/a |
| threshold | `StaggerTuning::damage_threshold` | `"damageThreshold"` | `damageThreshold` | `damageThreshold` | n/a |
| re-stagger cooldown | `StaggerTuning::cooldown_ms` | `"cooldownMs"` | `cooldownMs` | `cooldownMs` | n/a |
| per-entity stagger flag | `ENTITY_STATE_INPUT_PREFIX` leaf, written via `registry.entity_state_mut` | — | `"@state.staggered"` | same | n/a |
| tick event | `ENEMY_STAGGER_EVENT` | `"enemyStagger"` | `"enemyStagger"` | `"enemyStagger"` | n/a |

## Script syntax examples

```ts
// Proposed design
import { defineEntity, runtime } from "postretro";

const dist = runtime.read("@brain.targetDistance");

export const grunt = defineEntity({
  canonicalName: "grunt",
  components: {
    health: { max: 60 },
    mesh: { /* model, animation states */ },
    behavior: {
      initial: "idle",
      attack: { damage: 10, range: 1.8, cooldownMs: 1200 },
      stagger: { damageThreshold: 15, cooldownMs: 2000 },
      interrupts: [
        { to: "flinch", when: runtime.ge(runtime.read("@state.staggered"), 1) },
      ],
      states: {
        idle: {
          animation: "idle", motion: "hold",
          transitions: [{ to: "chase", when: runtime.le(dist, 16) }],
        },
        chase: {
          animation: "walk", motion: "chaseTarget",
          transitions: [{ to: "attack", when: runtime.le(dist, 2) }],
        },
        attack: {
          animation: "attack", motion: "chaseTarget", action: "attack",
          transitions: [{ to: "chase", when: runtime.gt(dist, 2) }],
        },
        flinch: {
          animation: "pain", motion: "hold", onEnter: "enemyStagger",
          // Commitment window: cannot exit for 450 ms, then resume.
          transitions: [{
            to: "chase",
            when: runtime.ge(runtime.read("@brain.timeInStateMs"), 450),
          }],
        },
      },
    },
  },
});
```

## Open questions

- **Chance-based pain.** Quake rolls pain probabilistically; deterministic threshold is the v1 stand-in. If design wants variance, it needs the engine's seeded-RNG story first — revisit alongside Epic 16 damage work.
- **Stagger while Idle.** Qualifying damage staggers an Idle enemy too (interrupts fire from any state), but the authored resume transition may land somewhere that isn't the attacker's direction if the attacker is outside detection range (no damage-based aggro yet). Accepted v1 oddity; the perception model owns the fix.
