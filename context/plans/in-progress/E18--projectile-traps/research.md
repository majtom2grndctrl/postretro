# E18--projectile-traps — research

Read at 035816e3. Findings that inform the brief without deciding it. Cited by symbol; re-verify before relying on any of it.

## Basis: nothing fires today

- No shooter/trap/turret class anywhere. The one FGD (`sdk/TrenchBroom/postretro.fgd`) has `entity_spawner` as its only spawning point class.
- No reaction primitive, system reaction, trigger command, or typed-command op fires a weapon or spawns a projectile. `scripting.md` §12 verb table: mover, light, fog, trigger, teleporter, spawner `fire`, `npcs` `update`/`damage`, player verbs.
- A map-placed weapon descriptor spawns as a world item with no fire input (`DescriptorSpawnPath::MapPlacement`; `is_host_replicated_descriptor_entity`).
- Closest workaround: a stationary enemy (`motion: "hold"`, attack naming a weapon) woken by `npcs({tag}).update({ aggro: true })` or behind a spawner. It aims eye→target (`WeaponAim`, ai `compute.rs`), never fires without a target, is a damageable NPC counted by `npcs()`, and repeats on attack cooldown rather than an authored interval.

## Projectile path coupling

- Both shooters end in `spawn_projectile(registry, owner_pawn, owner_weapon, ProjectileLaunch, predicted_shot_id, source)` (`sim/weapon_stage/commands.rs`). It needs no inventory, brain, aim, or faction. `ProjectileLaunch` (`sim/weapon/mod.rs`) is plain values.
- Enemy driver (`ai/weapon_controller.rs`, `AiWeaponControllers::advance_actor`, `pub(crate)`) holds a private `WeaponComponent` outside the registry and loops `advance_weapon_activation` (`weapon/execution.rs`) → `freeze_weapon_shot` (`weapon/shot.rs`) → hand-built launch → `AiHost::spawn_projectile(registry, actor, actor, …)`, with `WeaponActivationCheckpoint::restore` on spawn failure. Both functions touch only the component; aim is caller-supplied. A per-tick guard (`last_activation_tick`) blocks a second advance in one tick. The AI path skips `resolve_projectile_launch_pose`.
- `pawnless_ammo_weapon_fires_and_cools_but_preserves_the_reload_edge` is not a reusable driver: `run_local_weapon_command_inner` launches projectiles only inside `if let Some(pawn)`.
- Resource bypass: no infinite flag exists. `shot_scale_inputs` returns `ShotResourceCost::None` when effective stats carry no resource; the verdict then accepts and debit is a no-op. A trap-private component built from a descriptor clone with the resource stripped reaches that path. `predicted = true` also skips debit but misuses the flag.
- Projectile weapons have no pellets: `pellet_count` applies to hitscan only.
- `WeaponDescriptor.resolution: ResolutionMode { Hitscan, Projectile }` is per weapon. Precedent filter: `resolve_projectile_attacks` (ai `brain_programs.rs`) requires `Projectile` + `projectile.is_some()` and warns once.
- Tick order (`sim/mod.rs`): triggers/touch → `SimStage::Projectiles` → `SimStage::Ai` (enemy fire) → Steering → `SimStage::Weapons`. New projectiles must reach `projectile_stage::finish_spawn_tick` or they move on their spawn tick. A trigger fire earlier in the tick can latch a request the trap driver consumes later that tick.

## Attribution

- `owner_pawn` is a plain `EntityId` on `spawn_projectile`, `ProjectileComponent`, and `AiHost::spawn_projectile`. Attacker is derived `registry.exists(owner_pawn).then_some(owner_pawn)` in the `projectile_stage::advance` impact closure (direct and zero-amount splash shove) and in `emit_splash_damage`. Passing the live trap id yields `attacker = Some(trap)`; a dead id yields `None` only by accident.
- Other `owner_pawn` readers: `projectile_collision_excludes`; splash `self_damage` / `self_scale`; netcode `weapon_cues::observer_shot_id` (`allocator.stamp(owner_pawn)`).
- With attacker `None`, `apply_damage_with_context` (`entities/components/health.rs`) still sets `time_since_damage_ms = 0`, `damage_bearing = 0`, `damage_source_known = false`; skips `record_attacker_damage`; leaves `last_known_target_pos`. Ledger records the source id with `last_attacker` `None`.
- `credit_source: String`. Enemy projectiles carry the weapon's resolved credit source (`resolve_credit_source`: authored `creditSource`, else canonical name). `ENEMY_ATTACK_SOURCE_ID` is melee-only. Ownerless precedents: `"mover.crush"`, `script.applyDamage`.

## Trigger wiring

- Script path: `on_fire`/`on_exit` name reactions resolved by `TriggerBindingTable::bind_event` against `data_registry.reactions` (script-filled only). Spawner precedent: `wrapSpawnerEntity(...).fire()` → `spawnFromSpawner` → `bind_command` → `BoundTriggerCommand::Spawn` → `trigger_commands.rs`; also `register_sequenced_spawner_primitives`, the `partition.rs` list, `is_trigger_consequential_primitive`, and `data_descriptors/{js,lua}/reactions.rs` validation. Host gate: `SpawnContext::can_materialize_runtime_spawns` via `set_runtime_spawn_authority`.
- No-script path: `TriggerSystem::run_authoritative_tick_with_dispatch` on Enter queries `KinematicMover` by `trigger.target_tag` and calls `apply_mover_command_to_known_movers`, outside the binding table. `content/dev/maps/kinematic-platform.map` uses it with no `data_script`.
- `TriggerVolumeComponent.command` is a `MoverCommand`. Compiler `resolve_trigger_volume` maps `command` strings to u8 0–3; `level-format/src/trigger_volumes.rs` `from_bytes` rejects `> 3`; `trigger_volume_bridge.rs` maps u8 → `MoverCommand` with an `unreachable!` arm. `prl_loader.rs` logs per-command counts.
- Non-goal record (`context/research/co-op-triggers-trap-pools.md` §Risks and non-goals): "tags over Quake-style `targetname`/`target` pairing … no general logic-gate entity graphs". The rule governs the addressing form, not script-free authoring; `build_pipeline.md` §Custom FGD endorses KVP authoring for non-script mappers.
- Pre-wait member steps apply on every machine unless the primitive gates host-only (teleporter `arm`/`disarm` precedent); post-wait steps run host-only.

## Co-op

- Trigger volumes are not replicated (`networking.md`: "armed state never crosses the wire"). Changing the runtime command type touches no wire code.
- `COMPONENT_KIND_SPAWNER` is a reserved discriminant only; `ComponentPayload` covers Teleporter, Transform, PlayerMovement, MeshAnimation, KinematicMover, Light, Emitter, Fog. Guards: `component_kind_discriminant` and `discriminant_mapping_matches_enum_layout` (exhaustive, gapless). An appended unreplicated kind needs no `WIRE_VERSION` bump; a mid-enum insert renumbers and does.
- Mirror: `mirror_gameplay_projectile_with_descriptor_class` (`pub(crate)`, `PresentationFlight::FollowGameplay`). `mirror_local_gameplay_projectile` reads class from `DescriptorProvenance` on `owner_weapon`; a trap has none, so it returns `None`. Enemy class: `ResolvedProjectileAttack::canonical_weapon_name`, pushed as `EnemyProjectilePresentationSpawn` through `host_spawn_projectile_presentations` (`postretro/main.rs`).
- Cues: `WeaponCue` carries `shot_id`, kind, sounds, alias, anchor; no class. `publish_observer_weapon_cues` sends fire cues only for `activate` emissions with `shot_id` and `Emitter::Entity`. Client `materialize_observer_weapon_cue` falls back to origin contacts when the anchor entity is unknown, so an unreplicated trap is safe.
- `networking.md` §Version gates states PRLC / `WIRE_VERSION` 33; code has PRLB / 32 (`crates/net/src/handshake.rs`). Likely a decided-not-built entry; not this brief's to reconcile.

## Wave timing precedent

- `content/dev/scripts/arena-lights.ts`: author-time constants (`lightSpacingMs`, `pulseDurationMs`, `cyclePauseMs`) → shared `periodMs` → per-light `phase: (i * spacing) / period`. Timing is literal at install; nothing adjusts it at runtime.
- `wait(ms)` is sequential, literal, rejects ≤ 0 / NaN at install. Reactions have no declared parameters; author-time parameterization is a plain JS factory (`scripting.md` §12). `MAX_PENDING_REACTION_INSTANCES` = 256. Each activator gets its own parked tail, so a wait-staggered wave runs once per player who fires its trigger.

## Fixtures

- Maps: `content/dev/maps/*.map`, worldspawn `data_script "../scripts/X.ts"`; `prl-build` compiles the script through `scripts-build`. Precedents: `spawner-test.map` + `spawner-test.ts` (script-wired), `kinematic-platform.map` (no script), `trap-pools.map`.

## Ordering pins

Acceptance rows cite these by id.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Two `fire` requests reach one idle trap on one tick | any mix: two Enter edges of one trigger, two triggers, a trigger command plus a trigger-bound `fire()` step, two co-op activators | one execution (Fire drop rule; per-tick activation guard) |
| P2 | Duplicate `fire` on a trap vs a spawner | two `fire()` steps with no `wait` between them, or two activators entering a `multiple` trigger with zero re-arm on one tick, on a tag holding a trap and a spawner | trap: one execution. Spawner: two batches (spawner steps are not deduplicated) |
| P3 | `fire` and phase-0 `start` on one tick | either order | one execution; the schedule anchors on that tick and the next repeat is due one interval later |
| P4 | `fire` on a started trap between repeats | fire at tick k, between due ticks | fires if the gate is open; the schedule does not move; a due repeat that meets that execution's recovery is dropped, and the one after fires on schedule |
| P5 | `stop` and `start` on one tick | stop then start(opts); start then stop | stop→start: running, re-anchored on that tick with the new values. start→stop: no execution, not running |
| P6 | phase ≥ interval | — | first execution at `phase`, then one every `interval` |
| P7 | A trigger-bound reaction's `fire()` before its first `wait` | in-tick bound command | fires on the Enter tick, the same tick as a no-script `fire` command, and dedupes with it under P1 |
| P8 | Co-op: two players enter the sweep plate (a `multiple` trigger with zero re-arm, so both Enters fire) | same tick, or d ticks apart with d shorter than execution plus recovery | same tick: one sweep's shots. d apart: the second activator's parked tail adds no shot to any trap still in execution or recovery |
| P9 | Mod hot reload while a trap is started, or with a trap request latched | reload lands mid-schedule or between latch and the next tick | the started schedule and the latched request survive (trap state belongs to the level install); a parked post-`wait` step is dropped, as every parked tail is on hot reload |
| P10 | Level reinstall with a latched request or a parked tail that targets a trap | request issued before teardown, consumed after install | no shot in the new install |
| P11 | Connected client installs a `start_on_spawn` trap, or runs a trap step before the first `wait` of a `levelLoad`, named, or player-event reaction | client install or client-side dispatch | no request, no latch, no gameplay projectile, ever |
| P12 | Trigger command on a tag shared by kinds that do not all accept it | `fire` on mover + trap; `reverse` on mover + trap; `fire` on trap + spawner | only accepting members act; no warning, because one member accepts |
| P13 | Commands on an inert trap | any source | no shot, no schedule |
| P14 | Requests issued after the trap stage of a tick or outside any tick: player-event-bound steps, named or `levelLoad` dispatch, `wait` landings, `start_on_spawn` | after the trap stage, frame-end drain, or install | latched; act on the next authoritative tick, which is their start tick. A wait-staggered sweep's first post-`wait` gap, after a trigger-bound first step, is longer than later gaps by one tick plus any ticks that ran after the Enter tick in its frame; fixture tests step one tick per frame |
| P15 | Several `wait` landings drain in one multi-tick frame | catch-up frame | all act on the one next tick |
| P16 | interval exactly equals execution plus recovery | repeat due on the tick recovery reaches zero | the shared weapon gate decides (float recovery); tests stay clear of the boundary |
| P17 | A latched request and a trigger-bound request reach one trap on one tick | request latched by the previous frame's drain, then the Enter tick's trigger command | the latched request applies first: latched start then trigger stop leaves the trap stopped with no execution; latched stop then trigger start re-anchors on that tick |
| P18 | A started trap across a frame that runs several authoritative ticks | catch-up frame; repeats due on more than one of its ticks, or a trigger-bound start on a later tick of the frame | each tick runs its own due check: every due repeat fires on its own tick, a mid-frame start anchors on its own tick, and shot ticks match a run of one tick per frame. Post-`wait` steps still follow P15 |
| P19 | `fire` on a tick a repeat is due | fire latched or in-tick on that tick | one execution; the next repeat stays on its original tick |
| P20 | `start_on_spawn` with a `levelLoad` `stop` or `start(opts)` | both latched for the first authoritative tick | `start_on_spawn` is the install's first request: a `levelLoad` `stop` leaves the trap stopped with no execution; a `levelLoad` `start(opts)` is a no-op on the running trap |
| P21 | Teleport-bound trap step | issued during movement, before the trap stage | acts on the teleport's tick |
| P22 | Trigger Exit-bound `stop` on a started trap | Exit edge on the tick a repeat is due | the stop acts on the Exit tick, before the due check: no execution that tick or after; an in-flight execution finishes |
| P23 | `fire` and `stop` reach one trap on one tick | either order; in-tick, latched, or one of each | one execution; the trap ends stopped. `stop` cancels repeats, never a `fire` |
| P24 | A trigger's no-script `command` and its bound reaction's pre-`wait` trap step on one Enter | direct command issued first (`trigger_system.rs` runs it before the edge's bound dispatch) | issue order: `start` then `stop()` leaves the trap stopped; `stop` then `start()` leaves it running |

## Review pins

- Host-only member steps: `scripting.md` §12 Role check names only teleporter `arm`/`disarm`; the spawner's client gate lives in code (`SpawnContext::can_materialize_runtime_spawns`). The trap gate follows the spawner and is a candidate for that doc row at promotion.
- Live descriptor refresh is deferred because `refresh_from_descriptor` (`entities/components/weapon.rs`) cancels activation and reinstalls the resource from the descriptor it is given (`weapon_resource.rs`). A trap refresh would have to re-strip, or every later shot dry-fires. The enemy controller the Path mirrors does refresh live (`AiWeaponControllers::sync`), so the trap driver must not copy that step.
- Existing dev maps carry placeholder triggers: `target_tag "unused"` in `a11y-strobe-test.map` and `trigger-event-spike-trap`, an empty tag in `closet-reveal.map`. The install warning stays silent for empty and unmatched tags so these stay quiet.
- `--headless` (`postretro/observability/driver.rs`) runs no trigger stage and no landing drain; fixture rows use the compiled-level harness (`startup/teleporters_surface_tests.rs`), which steps one tick per frame.
- `wait` converts with `ticks = max(1, ceil(ms × 1000 / 16_667))` (`ms_to_ticks`, `reaction_scheduler.rs`). The two wave forms differ: a looping wave fires trap i at its rounded phase after the start tick, while a sweep whose first step is trigger-bound fires trap i, for i ≥ 1, one tick after its summed `wait` ticks (P14). Both runtimes share these offsets; the two wave forms do not.
- In-tick binding sites (`command_binding.rs`): trigger and teleport steps run before the trap stage (`sim/mod.rs` movement stage hosts teleport dispatch); player-event fires run after the sim tick (`frame_loop/mod.rs`), so their trap steps latch.
- No data-script hot reload exists; a level script runs once at install. The live reload is the mod hot reload (`staged_manifest_lifecycle.rs`), which clears parked `wait` tails and rebuilds bindings, and leaves mover motion alone.
- Splash requires a clear static-world sightline and derives its attacker separately from direct hits (`sim/splash.rs`).
- Pause does not stop authoritative ticks (UI-captured frames still tick, `frame_loop/mod.rs`); no pause row is owed.
