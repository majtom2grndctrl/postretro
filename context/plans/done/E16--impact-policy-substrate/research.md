# Research — Impact Policy Substrate

Grounding anchors, split out of the original combined impact/death session research (three session agents: death lifecycle, enemy AI, timer/state substrate). This file carries the substrate anchors; the death-machinery anchors (sweep, latch, kill report, resurrect, FSM) live in `E16--impact-death-lifecycle/research.md`. File:line drift with the tree — re-confirm before editing.

## Damage chokepoint
- `crates/entities/src/components/health.rs:319-340` — `apply_damage_with_context`, the single HP-decrement site. `updated.current = (updated.current - payload.amount).max(0.0)` at `:329`. Pre-health and unfloored post (`current - amount`, may be negative) both in scope at `:329`; unfloored value is discarded today.
- `payload.amount` is zone-multiplier-scaled at the fire site (`crates/postretro/src/sim/mod.rs:783-795`), not at the chokepoint. The damager identity is `DamageContext.attacker` (`health.rs:61`), the `context` param in scope at the chokepoint (copied to `record.attacker` at `health.rs:333`, but only when the ledger records a hit — the param itself is always in scope). There is no `HealthComponent.last_attacker` field; `last_attacker` exists only on `ContributorLedgerEntry`/`ContributorLedgerOverflow` (`health.rs:85`, `:121`).
- All producers route through it: weapon fire `crates/postretro/src/sim/mod.rs:803` (in-tick); enemy melee `crates/postretro/src/scripting/systems/ai.rs:835` (in-tick); `applyDamage` reaction `crates/postretro/src/health/reactions.rs:74` (app-drain; damage-only, rejects negatives `:57-63`).

## Despawn + timers
- `registry.despawn(id)` — `crates/entities/src/registry.rs:778-809`, synchronous, clears columns/tags, bumps generation.
- Deferred idiom to copy: `BrainComponent.death_despawn_remaining_ms: Option<f32>` (`crates/entities/src/components/brain.rs:150`), ms countdown decremented by `dt_ms` in `ai.rs:622-633`; timer is authoritative (not anim-completion). Same idiom: `attack_cooldown_remaining_ms`, `WeaponComponent.cooldown_remaining_ms`. No shared scheduler — each is bespoke on a closed component. (The brain field itself goes vestigial under the death spec; the new deferred-effect component copies the *idiom*, not the field.)
- System-command drain is instant, not scheduled: `SystemCommandQueue` `crates/entities/src/reactions/system_commands.rs:75-113` (`take()` returns whole Vec, no fire time). A general `afterMs` needs a new sim-time pending queue drained in the game-logic stage.
- Accumulator+crossing proves frame ordering (authoritative tick → write → settled crossing): `crates/postretro/src/scripting/systems/slot_accumulators.rs:98-151,220-295`.

## Health write-back (setHealth)
- No script health-write path exists; `applyDamage` is damage-only. `player.health` slot is engine-owned readonly, projected *from* the component (`crates/postretro/src/netcode/state_slots.rs:545-561`) — writing the slot does not write HP.
- Storage write exists internally: `HealthComponent.current` public, written via `registry.set_component`; `refresh_from_descriptor` clamps on hot-reload (`crates/entities/src/components/health.rs:275-277`). `setHealth` = new absolute chokepoint mirroring `apply_damage`. (Its kill-detection re-arm — clearing `death_handled` — is the death spec's concern; anchors there.)

## AI-tick seam (inert early-out, playAnim)
- AI tick: `run_ai_tick_with_navigation` `crates/postretro/src/scripting/systems/ai.rs:504`, called from the game-logic stage at `crates/postretro/src/sim/mod.rs:270-279`. The effects task's inert early-out (skip steering/attack/animation-request when the inert flag is set) lands inside this tick's per-entity walk.
- Animation seam reusable: `switch_animation_state`/`restart_animation_clip` `ai.rs:864-902` (id-targeted — the `playAnim` route, vs. the tag-resolving `setAnimationState` reaction `crates/postretro/src/scripting/reactions/animation.rs:14`); `state_elapsed` clip-complete query pre-built, unused, annotated "future AI state-selection layer is the named consumer" `crates/postretro/src/scripting/systems/mesh_anim.rs:393-407`.

## Per-entity state substrate (keystone)
- Stores are GLOBAL only: `SlotTable = HashMap<String, SlotRecord>` `crates/entities/src/slot_table.rs:181-189`, one table on `ScriptCtx` (`ctx.rs:34`), keyed by dotted name, no `EntityId` in key. `defineStore` produces global dotted namespace (`crates/scripting-core/src/store_bridge.rs:175`).
- Component vocabulary engine-closed: `ComponentKind` 16-variant enum `crates/entities/src/registry.rs:96-146`; dense per-kind columns `:559`. Modder-defined components are a hard non-goal (`index.md §4`).
- Closest existing per-entity string-keyed number map: `HealthComponent.zone_multipliers: HashMap<String, f32>` `health.rs:245` (descriptor-seeded, not a script write target). Health pattern = descriptor→component→runtime-field template.
- IR binding seam to reuse: typed command buffer + pluggable `BindingScope` (`scripting.md §11`; `StoreScope` at `crates/scripting-core/src/ir/scopes.rs:208-278`, resolves inputs/outputs by name; total-zero unset read at `scopes.rs:256-257`; dispatch scope layering over `StoreScope` at `scopes.rs:172-206`). A new `EntityScope` binds `state(name)` as an IR leaf and `setState`/`setHealth` as IR outputs, reusing bind-once/eval-per-tick.

## Replication (per-entity state = non-goal)
- Slot-based only: `ReplicationScope { None, SharedGlobal, OwnerPrivatePlayer }` `crates/entities/src/slot_table.rs:51-77`; schema walks `SlotTable` by dotted name (`crates/postretro/src/netcode/state_slots.rs:59-95`); wire ids per-slot, never per-entity.
- `OwnerPrivatePlayer` is player/client-keyed via hardcoded name→component match (`state_slots.rs:545-561`); mod `ownerPrivate` rejected (`crates/scripting-core/src/store_bridge.rs:498-520`, "no per-player authoring namespace").
- Entity snapshot ships only Transform/PlayerMovement/KinematicMover/Mesh-anim, not Health/arbitrary fields (`crates/postretro/src/netcode/replication.rs collect_payloads`). Per-entity state replicates through neither channel → net-new, high cost, deferred.

## Writes
- Store writes absolute, last-writer-wins, ship values not ops (`crates/scripting-core/src/store_bridge.rs:43-51`, `crates/entities/src/slot_table.rs:106-112`, `crates/postretro/src/netcode/state_slots.rs:811-839`). Additive exists only engine-side via `accumulate` IR (`slot_accumulators.rs:124-130`). `slot.add(delta)` lowers to self-referential IR `Add{read(slot), delta}` → same slot — no new evaluator; read-modify-write races LWW.
