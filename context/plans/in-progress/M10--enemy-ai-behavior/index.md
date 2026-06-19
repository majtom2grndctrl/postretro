# M10 — Enemy AI Behavior

> **Wave:** plan 2 of 2 in the M10 closing wave (one `/orchestrate` session). Build order: **`M10--pathfinding-path-following` → this plan**. Consumes that plan's runtime steering API (set agent destination, read arrived/blocked). This is the milestone payoff — the behavioral convergence that closes M10's north star.

## Goal

A small engine-owned state machine (idle → alert → attack → death) that drives a skinned-mesh enemy: it navigates toward the player via the steering API (plan 1), attacks by applying damage to the player in range, and selects its animation state per logical state. The enemy archetype and its tuning are authored as SDK descriptor data; the transitions and per-tick logic are engine-owned Rust. A foundation to refine, not a stub.

## Architectural decision: engine-owned FSM, declarative tuning

Scripts declare; Rust executes (`scripting.md` §1). There is no per-tick script callback and no live VM at tick time (§11), and there is no per-entity behavior tick for non-player entities today. So the FSM is **engine-owned Rust**, exactly as movement states are (`movement.md` §2: native states, declarative tuning). "Authored in the SDK as a reference behavior" means the enemy archetype, its tuning thresholds, and its logical-state→animation-state mapping are declared as descriptor data; the engine evaluates a **closed** transition set sized to idle/alert/attack/death and runs the behavior each tick. This stays inside "scripts declare, Rust executes" and keeps AI shallow. A future migration of AI *policy* onto the typed command buffer (M14) is out of scope — tuning is plain descriptor data here.

## Scope

### In scope

- **AI brain component** — engine-internal: current logical state, per-instance timers (attack cooldown, think stride), and resolved tuning. Materialized on spawn from the descriptor (the `data_archetype.rs` precedent).
- **`components.ai` descriptor block** — authored on the entity archetype: detection range, attack range, attack damage, attack cooldown, move speed, leash (lose-target) range, optional death-despawn delay, and the mapping of logical states (idle / alert / attack / death) to the archetype's declared animation-state names. Rust descriptor struct + parser in a **new module**, wired into `EntityTypeDescriptor` as a new field. `EntityTypeDescriptor` derives no `Default` and is built with all-fields-named struct literals, so adding the `ai` field is a compile error at every construction site — production paths, all test fixtures, and the Luau parse twin; the production parse twins (`conv.rs` `FromJs`/`FromLua`) and the production construction sites use explicit all-fields literals (not `..Default::default()`), so deriving `Default` does NOT reduce the blast radius at production sites — each must gain the `ai` field by hand regardless; `Default` only eases test fixtures if they are rewritten to `..Default::default()`. The parser/materializer additions are localized. QuickJS/Luau parse twins: same validation, same abort discipline (parity).
- **AI FSM tick system** — engine-owned, new module + a thin `run_ai_tick` wrapper in `main.rs`, hooked **after** `run_movement_tick` (player movement + camera follow) and **before** `run_weapon_fire_tick`; if the pathfinding plan's `run_agent_tick` is present in the same window, `run_ai_tick` precedes it. Follows the `run_movement_tick` / `run_weapon_fire_tick` precedent: a `fn(&mut self)` that borrows `self.script_ctx.registry`. Per enemy: resolve the player pawn + position; evaluate transitions (idle↔alert by detection/leash range, alert→attack by attack range, any→death at zero HP); drive the steering API (set the agent destination to the player while chasing, clear it when idle); on an attack tick (in range + cooldown elapsed) apply damage to the player via the Rust damage chokepoint and emit an attack event; select the animation state per logical state via the engine switch path; if `switch_animation_state` returns `UnknownState` or `NotAnimated`, warn-once and keep the prior animation state — never abort the tick. The attack-in-range/cooldown check and zero-HP death check run every tick regardless of stride; only detection/leash target-acquisition is strided. Distant / off-screen enemies evaluate detection/leash on a think stride (shallow time-slicing for waves), aligned with the animation resample precedent.
- **Death handling + kill reporting** — an enemy with a brain plays its death clip before despawning: at zero HP it enters the Death state, the engine death sweep defers despawn for brain-bearing entities (mirroring the player's death latch), the death clip plays, the kill is counted exactly once at the latch (reusing `HealthComponent.death_handled` or a dedicated brain death flag — one authoritative latch; the FSM does NOT separately report the kill), and the entity despawns after the configured `deathDespawnMs` (timer authoritative; clip playback best-effort). Non-brain non-player entities keep immediate despawn.
- **Reference enemy archetype + SDK surface** — a reference enemy authored in `sdk/behaviors/reference/entities.{ts,luau}`: `health` (with a hitbox + zone multipliers so the shipped weapon hitscan kills it), `mesh` (idle / locomotion / attack / death animation states) driven by a 4-clip rigged model sourced in-wave (the existing single-clip test model is insufficient — Task 4), and the new `ai` block. Map-placeable by `canonicalName`. Regenerated SDK typedefs (TS + Luau) with the new `ai` descriptor types and drift-guard pass. A placement on a dev map so the north star runs end-to-end.
- **Typed enemy sound events** — the enemy emits alert / attack / death events through the existing event system (weapon precedent); audible playback lands with M12 (Sound Foundation). No audio code here.

### Out of scope

- Pathfinding / steering internals (plan 1 owns them).
- Multiple enemy archetypes — one reference enemy.
- Patrol graphs; line-of-sight occlusion (detection is distance-only — LOS deferred); squad/group coordination; cover usage; flanking / strafing / retreat. The FSM is idle/alert/attack/death only.
- Projectile / ranged attacks and attack variety — the attack is an instantaneous in-range damage hit (contact/melee shape), mirroring the weapon's damage emission. Projectiles deferred.
- Multi-target threat selection — the single player pawn is the only target.
- Audio playback (events emitted only); M12.
- Map-overridable AI tuning — descriptor-owned, never FGD KVPs (`entity_model.md` §4).
- Behavior-IR / command-buffer authoring of AI policy (M14).
- Player-death consequences beyond the existing one-shot `playerDied` event (respawn, game-over flow).

## Acceptance criteria

- [ ] Parse-time abort: numeric/range fields (ranges, finiteness, non-negative `attackDamage`) are validated at parse and abort the ai descriptor on violation, twinned across both runtimes (parity — identical outcome on QuickJS and Luau); a runnable test verifies this at parse time on a bare descriptor value with no entity materialized. Spawn-time unmapped-state: a logical-state mapping naming an undeclared or unresolved animation-state name is validated at spawn — an unmapped state warns-once and that logical state simply does not switch animation (the FSM keeps the prior animation state); a separate runnable test verifies this with a materialized entity (the health/mesh descriptor precedent).
- [ ] An idle enemy whose player crosses the detection range transitions to alert and sets its agent destination to the player; when the player leaves the leash range it returns to idle and clears the destination; assert both transitions via the `path_state` read (the observable surface — has-path / arrived / blocked / distance) rather than an internal `set_destination` call count (runnable unit test on the transition evaluator with a stubbed player position + steering surface).
- [ ] An enemy with the player inside attack range and its cooldown elapsed applies its configured damage to the player exactly once per cooldown period, via the Rust damage chokepoint; below range or during cooldown it deals no damage (runnable unit test asserting player HP deltas over ticks).
- [ ] Each logical state switches the enemy to the mapped animation state via the engine switch path: idle→idle, alert→locomotion, attack→attack, death→death; assert the FSM's SELECTED target state string per logical state (the name passed to `switch_animation_state`), not clip resolution — the test does not require a real multi-clip model (runnable unit test asserting the requested state name per FSM state).
- [ ] A killed enemy enters Death and is despawned by the AI tick after `deathDespawnMs` (the timer is authoritative; the death clip plays best-effort, and an unresolved clip still despawns on the timer); the kill is counted exactly once at the death latch; a non-brain non-player entity at zero HP still despawns immediately in the sweep (runnable unit tests on the death-sweep coordination).
- [ ] Distant enemies evaluate target-acquisition (detection/leash) on a reduced think stride; near enemies run detection/leash every tick; the attack-in-range/cooldown check and the zero-HP death check run every tick regardless of stride (runnable unit test asserting that a stride-gated detection gap does not suppress an in-stride attack or death transition).
- [ ] SDK typedefs regenerate to include the `ai` descriptor types: assert the committed typedefs CONTAIN the `AiDescriptor` type (positive content assertion), and the drift-detection test passes (runnable: `gen-script-types` + the existing drift test).
- [ ] End-to-end on a dev map: a map-placed reference enemy spawns, walks toward the player without clipping playing its locomotion clip, switches to its attack clip and damages the player in range, takes hitscan weapon damage (zone multipliers applied), and plays its death clip then despawns at zero HP; a small wave of enemies pathing to the player does not stack into one body (separation from plan 1) (review-gate: manual play-through — the M10 north star).

## Tasks

### Task 1: AI brain component + `components.ai` descriptor

New engine-internal brain component (new `ComponentKind`): logical state, attack-cooldown timer, think-stride counter, resolved tuning. A new `ComponentKind` variant must extend every compiler-enforced surface: the `ComponentKind` enum + its repr discriminant, the `COUNT`/`VARIANTS` array, the serde-tagged `ComponentValue` enum, `ComponentValue::kind()`, and an `impl Component` block — all in `scripting/registry.rs`. Note: plan 1 also adds an Agent `ComponentKind` variant in this same `registry.rs` in this wave; expect an adjacent edit (compiler-checked, but a likely merge point). New descriptor struct + parser module for the `ai` block (detection/attack/leash ranges, attack damage, attack-cooldown ms, move speed, death-despawn ms, logical-state→animation-state map). Wire into `EntityTypeDescriptor` with a new field; see Scope for the construction-site blast radius — deriving `Default` (or a builder) on `EntityTypeDescriptor` only eases test fixtures if they are rewritten to `..Default::default()`; production parse twins (`conv.rs` `FromJs`/`FromLua`) use explicit all-fields literals and must gain the `ai` field by hand regardless. Materialize the brain — and plan 1's agent component by calling `attach_agent(registry, entity, &agent_params, move_speed)` (plan 1's public free fn; move speed from the `ai` descriptor; capsule seeded from the passed `NavAgentParams`) — at the data-archetype attach site in `data_archetype.rs`. This plan ships no agent type of its own; it consumes plan 1's `attach_agent`. Read `agent_params()` into a local `Option<NavAgentParams>` BEFORE borrowing the registry (avoids borrowing `self.nav_graph` while `self.script_ctx.registry` is `borrow_mut`'d from the same `self`); pass that `Option<NavAgentParams>` through the new attach/dispatch parameter. Threading this new `Option<NavAgentParams>` arg changes the signatures of `attach_descriptor_components`, `apply_data_archetype_dispatch`, and the `spawn_from_player_starts` sibling, plus their in-file test call sites in `data_archetype.rs`; give the new arg a `None` default at the player-start and existing-test call sites so they stay green. Note: reading `nav_graph` from the always-on attach path depends on plan 1's Task 1 having de-gated `nav_graph` to all builds; until then the field is dev-tools-only and unreadable from the always-on path. If a map has no navmesh, the capsule falls back to an engine default and the agent simply cannot path. Validation at parse (abort on violation, twinned across both runtimes): all range fields must be finite and positive; `attackDamage` must be non-negative and finite (a negative value would heal the player via `apply_damage`'s subtraction); the four logical-state keys (`idle` / `alert` / `attack` / `death`) are a closed set — an unrecognized key is a parse error. Logical-state→animation-state name mapping is cross-component (the ai block cannot see the mesh block at its own parse), so it is validated at spawn: an unmapped or undeclared state name warns-once and that logical state does not switch animation (the FSM keeps the prior animation). Validation is twinned: identical outcome on QuickJS and Luau.

### Task 2: AI FSM tick system

Engine-owned FSM tick (new module + `App::run_ai_tick` wrapper in `main.rs`), following the `run_movement_tick` / `run_weapon_fire_tick` precedent (`fn(&mut self)` borrowing `self.script_ctx.registry`). Insert the `run_ai_tick` call after the camera-follow block and before `run_weapon_fire_tick`; if the pathfinding plan's `run_agent_tick` is present, `run_ai_tick` precedes it. Per enemy on its think tick: locate the player pawn by iterating with `iter_with_kind(ComponentKind::PlayerMovement)` and reading its `Transform` for position (the `run_movement_tick` precedent — NOT `pawn_with_health`, which returns `(EntityId, HealthComponent)` and is the damage-target id only); evaluate transitions; drive the steering API (`set_destination` / `clear_destination` free fns over the registry) and read arrival/blocked via plan 1's `AgentPathState` struct returned by `path_state` — that struct's `blocked` and `arrived` flags drive FSM reads; on attack apply damage to the player via the Rust chokepoint (`apply_damage`) + emit the attack event; request the mapped animation state via the engine switch path (`switch_animation_state`) — if `switch_animation_state` returns `UnknownState` or `NotAnimated`, warn-once and keep the prior animation state, never abort the tick. The transition logic should be a pure function over (player position, agent position, tuning, current state) returning the next state + steering intent, so AC2's unit test can drive it without the `App`. The attack-in-range/cooldown check and zero-HP death check run every tick regardless of stride; only detection/leash target-acquisition is strided. Think-stride time-slicing by player distance on detection/leash. Depends on plan 1's steering API and Task 1.

### Task 3: Death coordination + kill reporting

Coordinate enemy death with the death sweep (`scripting/systems/health.rs`). Today the sweep despawns any non-`PlayerMovement` entity at zero HP immediately; add a branch: detect a brain-bearing entity via `has_component_kind(id, ComponentKind::<brain-variant>)` in the sweep's pass 2 (mirroring the existing `PlayerMovement` check). A brain-bearing entity at zero HP sets a death latch — reuse `HealthComponent.death_handled` or a dedicated brain death flag; one authoritative latch — and reports the kill **once** at the latch transition (so `DeathReport`/progress count it). The FSM does NOT separately report the kill; a brain death is counted exactly once, never twice. The brain-bearing entity is **not** despawned by the sweep. The AI tick owns the despawn: on entering Death it switches to the death animation, and after `deathDespawnMs` (the timer is authoritative — clip playback is best-effort, since an unresolved death clip yields `UnknownState` and plays nothing) it despawns the entity. Despawn runs as a two-pass collect-then-despawn inside `run_ai_tick` (the `sweep_deaths` / particle-sim precedent), never mid-iteration, to respect entity_model §3's ID-invalidation rule. Depends on Task 2 (the Death state) and Task 1 (the brain).

### Task 4: Reference enemy archetype + SDK + map placement

Author the reference enemy in `sdk/behaviors/reference/entities.{ts,luau}` (health + mesh animation states + the `ai` block), with Luau parity. Regenerate SDK typedefs (`gen-script-types`) to include the `ai` descriptor types; register `AiDescriptor` (and the `EntityTypeDescriptor.components.ai` shape) in the typedef generator's descriptor arms in `typedef.rs` where `MeshDescriptor` / `HealthDescriptor` are special-cased — a type the generator does not emit passes the drift test vacuously. Regenerate and commit the updated typedefs; keep the drift test green. The archetype is map-placeable via the data-archetype `canonicalName` dispatch (`find_descriptor` / `apply_data_archetype_dispatch` in `data_archetype.rs`) — `prop_mesh` is the precedent for a stateless-mesh component, not the routing precedent. Placeability rides on the enemy carrying `health` and `mesh` components: `is_directly_map_placeable` already checks for health/mesh/movement/light/emitter — no extension of `is_directly_map_placeable` is needed for an enemy archetype (it would only be needed for a hypothetical ai-only archetype, which is not in scope). The existing `decraniated` test model carries a single clip (`mixamo.com`), so this task **sources a rigged enemy character with distinct idle / walk / attack / death clips** (a permissively-licensed Mixamo-or-similar character, owner-approved) under `content/dev/models/`, its PNG textures riding the existing `.prm` pipeline; the `mesh` block's animation states map to its four clip names. Place a reference enemy on a dev map so the north-star play-through runs; the end-to-end AC depends on this 4-clip asset. Depends on Tasks 1–3.

### Task 5: EXP store + on-kill reward reaction — DEFERRED (not built in this wave)

Deferred by owner decision during implementation. The design (a mod-declared `progress.exp` store via `defineStore`, plus a mod-authored reaction bound to a typed `enemyKilled` event that increments it via a new additive `addState(slot, amount)` one-instruction command buffer) is recorded here for a future follow-up. A key constraint surfaced during Task 3: named events are **payload-less** (`fire_named_event` dispatches reactions by name only), so a per-enemy reward amount cannot ride the event — a future build must either use a fixed reward constant in the reaction, stage the reward in a store slot for an IR-valued reaction (the deferred `setState`-IR write path, `scripting.md` §11), or have the engine apply the reward at the kill latch. The `expReward` descriptor field and the `enemyKilled` event emission that were folded into Tasks 1/3 for this feature are **removed** in a cleanup pass, since they have no consumer once this task is deferred.

## Sequencing

**Phase 1 (sequential):** Task 1 — the brain component + descriptor the system reads.
**Phase 2 (sequential):** Task 2 — the FSM tick consumes Task 1 and plan 1's steering API.
**Phase 3 (sequential):** Task 3 — death coordination consumes the Death state (Task 2) and the brain (Task 1).
**Phase 4 (sequential):** Task 4 — the reference archetype + SDK + placement is the integrated outcome consuming Tasks 1–3.
**Phase 5:** Task 5 (EXP store + on-kill reward) — DEFERRED, not built in this wave.

## Rough sketch

- Damage the player: `scripting::components::health::apply_damage(registry, player_id, &DamagePayload { amount: attackDamage })` — the same chokepoint the weapon uses (`scripting.md` §10.5). The payload carries no zone field; zone multipliers are a weapon-hitscan-site concern and a contact attack has no hit zone.
- Animation: `scripting::components::mesh::switch_animation_state(registry, id, state_name)` — already the engine switch path the `setAnimationState` reaction wraps; the AI calls it directly (it is `pub(crate)`). The runtime never decides transitions; the FSM does.
- Death: `sweep_deaths` latches + reports the kill once for a brain-bearing zero-HP entity and skips despawn; `run_ai_tick` despawns it (two-pass collect-then-despawn) after `deathDespawnMs`.
- Steering: call plan 1's `set_destination(&mut EntityRegistry, agent, Vec3)` / `clear_destination` / `path_state` free fns — the agent is engine-internal, reached by id, never `worldQuery`.
- Descriptor: model the `ai` parse module on the `health` / `mesh` descriptor blocks in `data_descriptors.rs`; add `EntityTypeDescriptor.ai: Option<AiDescriptor>` and a `ComponentValue` brain variant.
- Player lookup: the player POSITION comes from `iter_with_kind(ComponentKind::PlayerMovement)` + its `Transform` (the `run_movement_tick` precedent). `pawn_with_health` returns `(EntityId, HealthComponent)` and is the damage-target id only — do not use it for position.
- Asset: the reference enemy needs a skinned glTF with idle / locomotion / attack / death clips. If the slice's test model lacks four named clips, supplying a suitable model (or mapping available clips) is part of Task 4 — see Open questions.

## Boundary inventory

The `ai` descriptor block crosses Rust ↔ JS/TS ↔ Luau ↔ wire. No FGD KVPs (tuning is descriptor-owned). Casing pinned once here:

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| component block | `EntityTypeDescriptor.ai` | `"ai"` | `components.ai` | `components.ai` |
| detection range | `detection_range` | `"detectionRange"` | `detectionRange` | `detectionRange` |
| attack range | `attack_range` | `"attackRange"` | `attackRange` | `attackRange` |
| leash range | `leash_range` | `"leashRange"` | `leashRange` | `leashRange` |
| attack damage | `attack_damage` | `"attackDamage"` | `attackDamage` | `attackDamage` |
| attack cooldown (ms) | `attack_cooldown_ms` | `"attackCooldownMs"` | `attackCooldownMs` | `attackCooldownMs` |
| move speed | `move_speed` | `"moveSpeed"` | `moveSpeed` | `moveSpeed` |
| death despawn (ms) | `death_despawn_ms` | `"deathDespawnMs"` | `deathDespawnMs` | `deathDespawnMs` |
| state→clip map | `states` | `"states"` | `states` | `states` |

`states` maps the four logical states to animation-state names, e.g. `{ idle, alert, attack, death }` → declared `mesh` state names. The four keys (`idle` / `alert` / `attack` / `death`) are a closed set; an unrecognized key in `states` is a parse error, aborted and twinned. The component / descriptor block name `ai` is a working name — settling it before Task 1 is preferred, since the name is hardcoded across Rust, the JS parser, the Luau parser, and generated typedefs; a rename touches all four layers. Resolvable during review against the engine-closed component vocabulary (`entity_model.md` §1).

## Script syntax examples

```ts
// sdk/behaviors/reference/entities.ts  — // Proposed design
defineEntity({
  canonicalName: "enemy_grunt",
  components: {
    health: { max: 40, hitbox: { halfExtents: [0.4, 0.9, 0.4] }, zoneMultipliers: { head: 2.0 } },
    mesh: {
      model: "models/grunt/scene.gltf",
      defaultState: "idle",
      animations: {
        idle:   { clip: "Idle",  loop: true },
        walk:   { clip: "Walk",  loop: true,  crossfadeMs: 120 },
        attack: { clip: "Attack", loop: false, crossfadeMs: 80 },
        die:    { clip: "Death", loop: false, crossfadeMs: 80 },
      },
    },
    ai: {
      detectionRange: 18, leashRange: 26, attackRange: 2.2,
      attackDamage: 8, attackCooldownMs: 1200, moveSpeed: 3.5,
      deathDespawnMs: 1500,
      states: { idle: "idle", alert: "walk", attack: "attack", death: "die" },
    },
  },
});
```

## Open questions

- **Reference model + clips (resolved).** The existing `decraniated` model has a single clip, so Task 4 sources a 4-clip rigged enemy (idle / walk / attack / death) in-wave (owner-approved asset + license). The end-to-end AC requires it — no single-clip degrade path.
- **Attack damage timing within the clip.** The attack applies damage on cooldown while in range (a simple gate), not synced to an animation-clip impact frame. Frame-synced attacks (an "impact" keyframe event) are a future refinement, not this plan.
- **Blocked-path behavior.** When plan 1 reports *blocked* (no path to the player), the enemy holds in alert facing the player rather than walking into geometry. Richer recovery (repath to a nearby reachable point) defers.
