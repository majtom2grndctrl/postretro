# Research — Impact / Death Lifecycle

Grounding anchors from three session agents (death lifecycle, enemy AI, timer/state substrate). File:line drift with the tree — re-confirm before editing.

## Damage chokepoint
- `crates/entities/src/components/health.rs:319-340` — `apply_damage_with_context`, the single HP-decrement site. `updated.current = (updated.current - payload.amount).max(0.0)` at `:329`. Pre-health and unfloored post (`current - amount`, may be negative) both in scope at `:329`; unfloored value is discarded today.
- `payload.amount` is zone-multiplier-scaled at the fire site (`crates/postretro/src/scripting/sim/mod.rs:783-795`), not at the chokepoint. `HealthComponent.last_attacker` set at `health.rs:333`.
- All producers route through it: weapon fire `sim/mod.rs:803`; `applyDamage` reaction `crates/.../reactions/health/reactions.rs:74` (damage-only, rejects negatives `:57-63`).

## Auto-death machinery (to make inert)
- `sweep_deaths` — `crates/postretro/src/scripting/systems/health.rs:89-185`. Dead predicate `health.current <= 0.0 || !is_finite` (`:103`). Plain non-player branch does immediate `registry.despawn(id)` at `:170-181` (the fused branch to unfuse). Brain branch (`:143-166`) latches + counts kill, does NOT despawn (already detect-only). `death_handled` latch at `crates/entities/src/components/health.rs:239`.
- Called via `run_death_sweep` (`sim/mod.rs:816-833`) at `sim/mod.rs:311`, game-logic stage, after AI tick (`:270`) and weapon fire (`:298`).
- AI-tick 0-HP handler: `crates/postretro/src/scripting/systems/ai.rs:601-633` forces `LogicalState::Death`, clears target; despawn timer `death_despawn_remaining_ms` seeded `:622`, despawn in pass 4 `:907-914`. **Resurrect recovery exists** (`ai.rs:648-651`): HP restored above 0 while in Death → recover to Idle, clear countdown. NOTE: it keys off `brain.state == Death`, NOT `death_handled` (in-code comment `ai.rs:645-647`). Once Task 2 removes the 0-HP→Death transition, nothing puts a brain into Death from HP, so this recovery goes vestigial — the load-bearing kill re-arm is resetting `death_handled` (below).
- `alive_players` occupancy predicate keys off `current > 0.0` at `sim/mod.rs:180-191` (reconcile: latched-0-HP-but-present player).
- No downstream changes needed: scoring `reaction_dispatch.rs:59 on_entity_killed` (fed by the kill report, decoupled from despawn); HUD reads `player.health` slot; replication (`netcode/replication.rs:245-257`) prunes by despawn, not HP.

## Despawn + timers
- `registry.despawn(id)` — `crates/entities/src/registry.rs:778-809`, synchronous, clears columns/tags, bumps generation.
- Deferred idiom to copy: `BrainComponent.death_despawn_remaining_ms: Option<f32>` (`crates/entities/src/components/brain.rs:150`), ms countdown decremented by `dt_ms` in `ai.rs:622-633`; timer is authoritative (not anim-completion). Same idiom: `attack_cooldown_remaining_ms`, `WeaponComponent.cooldown_remaining_ms`. No shared scheduler — each is bespoke on a closed component.
- System-command drain is instant, not scheduled: `SystemCommandQueue` `crates/entities/src/reactions/system_commands.rs:75-113` (`take()` returns whole Vec, no fire time). A general `afterMs` needs a new sim-time pending queue drained in the game-logic stage.
- Accumulator+crossing proves frame ordering (authoritative tick → write → settled crossing): `crates/postretro/src/scripting/systems/slot_accumulators.rs:98-151,220-295`.

## Health write-back (setHealth)
- No script health-write path exists; `applyDamage` is damage-only. `player.health` slot is engine-owned readonly, projected *from* the component (`netcode/state_slots.rs:545-561`) — writing the slot does not write HP.
- Storage write exists internally: `HealthComponent.current` public, written via `registry.set_component`; `refresh_from_descriptor` clamps on hot-reload (`health.rs:275-277`). `setHealth` = new absolute chokepoint mirroring `apply_damage`; resurrect must reset `death_handled = false`. `death_handled` is set only by `sweep_deaths` (`health.rs:129`, `:154`) and is NEVER cleared today — resetting it in `setHealth` is what re-arms kill detection (`apply_damage`/`sweep_deaths` skip a latched target). This is the real re-arm, distinct from the FSM-state recovery above.

## Enemy AI FSM
- Closed 4-state FSM: `LogicalState { Idle, Alert, Attack, Death }` `crates/entities/src/components/brain.rs:29-40`, "engine-closed, scripts cannot add states" `brain.rs:24-26`. Per-instance `BrainComponent` `brain.rs:131-180`.
- Pure swappable transition core `evaluate_transition(...)` `ai.rs:248-318`; tick `run_ai_tick_with_navigation` `ai.rs:504`. Distance-driven edges only.
- Damage→AI is near-absent: only 0-HP→Death. `ai.rs` never reads `last_attacker`. No flinch/stagger/hurt/downed anywhere (grep-confirmed).
- Modder surface today: `AiDescriptor` 8 scalars + `AiStateNames` closed anim map (`crates/foundation/src/data_descriptors/types/combat.rs:230-261`); one bool `enemies({tag}).update({aggro})` → `reactions/enemy_state.rs`; tag `setAnimationState` `reactions/animation.rs:14`.
- Animation seam reusable: `switch_animation_state`/`restart_animation_clip` `ai.rs:864-902`; `state_elapsed` clip-complete query pre-built, unused, annotated "future AI state-selection layer is the named consumer" `mesh_anim.rs:393-407`.
- Prior design intent: `context/research/enemy-aggro-model.md` (v0-floor aggro; stagger/downed axis not yet on paper). M10 decision `ai.rs:11-13`.

## Per-entity state substrate (keystone)
- Stores are GLOBAL only: `SlotTable = HashMap<String, SlotRecord>` `crates/entities/src/slot_table.rs:181-189`, one table on `ScriptCtx` (`ctx.rs:34`), keyed by dotted name, no `EntityId` in key. `defineStore` produces global dotted namespace (`store_bridge.rs:175`).
- Component vocabulary engine-closed: `ComponentKind` 16-variant enum `crates/entities/src/registry.rs:96-146`; dense per-kind columns `:559`. Modder-defined components are a hard non-goal (`index.md §4`).
- Closest existing per-entity string-keyed number map: `HealthComponent.zone_multipliers: HashMap<String, f32>` `health.rs:245` (descriptor-seeded, not a script write target). Health pattern = descriptor→component→runtime-field template.
- IR binding seam to reuse: typed command buffer + pluggable `BindingScope` (`scripting.md §11`; `StoreScope` at `crates/scripting-core/src/ir/scopes.rs:208-278`, resolves inputs/outputs by name). A new `EntityScope` binds `state(name)` as an IR leaf and `setState`/`setHealth` as IR outputs, reusing bind-once/eval-per-tick.

## Replication (per-entity state = non-goal)
- Slot-based only: `ReplicationScope { None, SharedGlobal, OwnerPrivatePlayer }` `slot_table.rs:51-77`; schema walks `SlotTable` by dotted name (`netcode/state_slots.rs:59-95`); wire ids per-slot, never per-entity.
- `OwnerPrivatePlayer` is player/client-keyed via hardcoded name→component match (`state_slots.rs:545-561`); mod `ownerPrivate` rejected (`store_bridge.rs:498-520`, "no per-player authoring namespace").
- Entity snapshot ships only Transform/PlayerMovement/KinematicMover/Mesh-anim, not Health/arbitrary fields (`netcode/replication.rs collect_payloads`). Per-entity state replicates through neither channel → net-new, high cost, deferred.

## Writes
- Store writes absolute, last-writer-wins, ship values not ops (`store_bridge.rs:43-51`, `slot_table.rs:106-112`, `state_slots.rs:811-839`). Additive exists only engine-side via `accumulate` IR (`slot_accumulators.rs:124-130`). `slot.add(delta)` lowers to self-referential IR `Add{read(slot), delta}` → same slot — no new evaluator; read-modify-write races LWW.
