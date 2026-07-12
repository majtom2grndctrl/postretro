# Research notes — Enemy Stagger

Grounding verified against source 2026-07-07. Line numbers from that tree; treat as hints, not contracts.

## The death latch is the template

- Death is layered **outside** the pure FSM: `evaluate_transition` is HP-blind and holds `Death` terminal (`ai.rs:279-284`); the tick checks HP every tick (`ai.rs:551-554`), forces the state, clears steering via `Hold`, and runs a seeded-once countdown `death_despawn_remaining_ms: None → Some(ms)` decremented per tick (`ai.rs:564-583`; field `brain.rs:141-149`), with recovery back to `Idle` if HP restores (`ai.rs:598-601`). Stagger mirrors: forced entry, `Option<f32>` timer, FSM recomputes on exit.
- Timer-authoritative over clip length is the established idiom: "the entity despawns after `death_despawn_ms` regardless of whether the death clip ever resolved" (`ai.rs:453-461`). Stagger duration is likewise a descriptor ms value, not clip-driven.

## Why not clip-completion-driven return

- `state_elapsed(anim, table, anim_time) -> StateElapsed { complete, .. }` exists but is dead code (`mesh_anim.rs:387-429`, `#[cfg_attr(not(test), allow(dead_code))]`) and needs `ModelClipTable` clip durations, which live render-side — the fixed-tick AI has no access today. Promoting it is real plumbing; the ms timer needs none.

## Damage is fire-and-forget — the accumulator seam

- `apply_damage(registry, id, &DamagePayload)` — `crates/entities/src/components/health.rs:121-130`; `DamagePayload { amount }` amount-only (`foundation_pods.rs:8-10`). Exactly three production callers: player weapon stage (`sim/mod.rs:237`), AI melee apply (`ai.rs:757-765`), `applyDamage` reaction (`health/reactions.rs:70`).
- Nothing records who-hit-whom; `WeaponImpact` is consumed and dropped in `run_weapon_fire_tick` (`sim/mod.rs:198-241`). Hence the brain-side accumulator written inside the chokepoint.
- Tick order (`sim/mod.rs:56-96`): AI (:65) runs **before** weapon fire (:79), so player damage lands post-AI and staggers on the next tick — one tick of latency, deterministic. Consume-and-clear per brain entity inside the AI tick is the only clear point that never drops damage.
- `context/research/combat-events.md` plans per-impact facts (`combat.damage`, `targetHpBefore`, zone) instrumented at this same chokepoint + the death sweep — "build on that seam, do not race it." The accumulator is deliberately private/minimal so the ledger can land later without collision.

## FSM extension points

- `LogicalState { Idle, Alert, Attack, Death }` engine-closed, `ALL`/`label`/serde snake_case — `brain.rs:27-60`. `AiStateNames` is `deny_unknown_fields` with four required keys (`combat.rs:151-158`) — putting the pain animation name in the stagger block avoids breaking every existing descriptor.
- Facing arbitration only writes in `Alert | Attack` (`ai.rs:726-751`); Idle/Death are the no-write precedent Stagger joins. `E10--enemy-facing-slew` (ready) rate-limits those writes and pins arbitration behavior with tests — sequencing after it avoids churn.
- Animation interrupt machinery exists: `switch_animation_state` with per-state `InterruptPolicy` (`"smooth"`/`"snap"`) handles mid-fade cuts (`mesh.rs:320-433`, policy `mesh.rs:28-36`); `restart_animation_clip` (`mesh.rs:469-505`) exists but is deliberately unused here (see wire note).
- Spawn validation of animation names: `validate_brain_animation_states` (`brain.rs:212-244`) — extended, not duplicated.

## Replication

- Wire carries `Transform` + animation state **name** only (`WireMeshAnimationState { current_state }`, `net/src/wire.rs:181-183`); Brain/Health never replicate. A new logical state costs nothing on the wire when expressed as a state name.
- In-state clip restarts produce **no wire delta** — remote clients would clamp on the first playthrough. This is why re-stagger-while-staggered is specified as a no-op rather than a restart: it makes host and client exactly consistent for free.

## Behavior-IR reality check

- Shipped IR (M14) is a pure expression language with one real adopter (dash tuning via `NumberOrIr`, `movement.rs:161-176`); the named-output write path has zero consumers; no combat `BindingScope` exists — that is Epic 16's `CombatScope` (`roadmap.md:220`, combat-events.md §7). Descriptor scalars now, additive `NumberOrIr` upgrade later (dash shows literal→IR is wire-compatible via `#[serde(untagged)]`).

## Prior art

- Stagger consciously deferred at `M10--skeletal-hit-zones` ("Locational effects (gibs, stagger, hit reactions)" out of scope, index.md:22) and never picked up; no other combat-sense stagger/pain mention in `context/`. Roadmap kin: Epic 16 knockback ("on-hit impulse payload field") and status effects — stagger stays impulse-free to avoid claiming that surface.

## File sizes

`ai.rs` 839 + `ai_tests.rs` 2130 → split shared with `E10--enemy-multi-attack` Task 1. `health.rs` 350, `brain.rs` 471, `combat.rs` 227 — all comfortable.
