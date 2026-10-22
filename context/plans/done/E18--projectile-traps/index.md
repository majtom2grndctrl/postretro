# E18--projectile-traps

Brief · compact · Epic 18 · reads: `context/lib/entity_model.md` §Components (Weapon vocabulary, Weapon activations), `context/lib/scripting.md` §12, `context/lib/networking.md` §Presentation events vs. replicated state, `context/lib/build_pipeline.md` §Custom FGD · read at 035816e3 · Acceptance runs long: the contract crosses Rust, TS, Luau, FGD, and the level file.

## Problem
Owner request, a capability. Mappers cannot build Quake-style projectile traps: no map-placed entity can fire a weapon, and no reaction or trigger command spawns a projectile. The nearest workaround, a stationary enemy, aims at a target instead of along a facing, takes damage, counts as an NPC, and repeats on attack cooldown. When done, a mapper places a trap that names a projectile weapon and fires along its map facing. It fires once per command, repeats at an authored interval, or starts at level load. A trigger drives it with no script, or a level script drives it, including a timed wave. The same script-free trigger command reaches every brainless map entity that can be fired, so spawners join traps there. In co-op every player sees and hears its shots, and it fires only on the host.

## Decisions
- **Entity.** FGD point class `projectile_trap`; map member kind `"trap"`; an engine-closed component, like `Spawner` (`entity_model.md` §Components). KVPs: `weapon`, `angles`/`mangle`/`angle`, `interval_ms` (default 1000), `phase_ms` (default 0), `start_on_spawn` (default false), `_tags`. No body: no model, collision, or health.
- **Weapon.** `weapon` names the one canonical `WeaponDescriptor` (§Weapon vocabulary), resolved once per level install. Projectile-only by owner ruling. A trap is inert with one load warning when the name is unknown, resolution is hitscan, or the primary action charges: a trap has no held trigger to release.
- **Fire.** `fire` runs one execution of the weapon's primary action through the shared activation and shot machinery, so authored shot/wait sequences, recovery, and captured shot tuning behave as for any wielder. A trap has no resource limits: it pays no ammo, heat, or cell, and never reloads, dry-fires, or overheats. This diverges from §Weapon vocabulary's no-branch-on-wielder rule by owner ruling. A `fire` that meets a running execution or recovery is dropped, not queued (§Weapon activations).
- **Timing.** A request issued during a tick before the trap stage acts on that tick: trigger-bound and teleport-bound steps. Every other request latches and acts on the next authoritative tick: player-event-bound steps, named and `levelLoad` dispatch, post-`wait` steps, and `start_on_spawn`, which is the install's first request. On a tick, latched requests apply first, then that tick's own, each in issue order, all before the due check. A `start` anchors on the tick it applies.
- **Repeat.** `start` schedules executions at `phase` after its tick, then every `interval`, anchored so the cadence never drifts. Interval rounds up to whole ticks by `wait`'s rule, so a sub-tick interval becomes one tick. Phase rounds up the same way without the one-tick floor, so phase 0 fires on the start tick. A due repeat fires when the shared weapon gate is open on its tick and is dropped otherwise; the next stays due on schedule. `start` on a running trap is a no-op. `stop` cancels future repeats and leaves an in-flight execution to finish. `interval` ≤ 0, a negative `phase`, or a non-finite value makes the trap inert with one warning (KVP), or rejects the reaction at install (script literal).
- **Aim.** Fires from the entity origin, which is its muzzle since it has no body, along the view-frame forward of its map angles (`entity_model.md` §4 Facing convention).
- **Attribution.** Trap shots carry no attacker, direct or splash. Credit source is the weapon's resolved credit source, the same as for any wielder. A damaged enemy registers damage from an unknown source and never turns toward the trap.
- **No-script wiring.** Trigger `command` gains `fire`. On Enter, a trigger's command applies to every member carrying its `target_tag` whose kind accepts the command and that carries no brain; a member with a brain is skipped even when its kind accepts. Movers accept `start`/`stop`/`reverse`/`go_to_path_node`; traps accept `fire`/`start`/`stop`; spawners accept `fire`, keeping their own meaning of one batch per firing. A non-empty tag that some member carries but no member accepts logs one warning at level install; an empty or unmatched tag stays silent, as today. The FGD default stays `start`. Addressing stays tags-only (`context/research/co-op-triggers-trap-pools.md` §Risks and non-goals); the direct mover command is the precedent for script-free wiring (`done/E17--trigger-command-surface`).
- **Script handle.** `start` options apply to that run only; a `start` without options uses the authored interval and phase. TS and Luau both ship.
- **Placement.** Engine mechanism: component, fire driver, trigger command. Authored policy: which weapon, when, how often. All trap behavior is host-authoritative: no trap request acts on a client, including a step before the first `wait`. Trap state never crosses the wire.
- **Co-op presentation.** Trap shots reach clients through the host projectile presentation mirror with the weapon's descriptor class, and carry an observer shot id so fire and impact cues reach every player.
- **Lifecycle.** Started state, schedules, and latched requests belong to one level install; a reinstall or level change clears them. A mod hot reload leaves started traps, latched requests, and each trap's install-time weapon in place, as it leaves mover motion; parked post-`wait` steps drop with the scheduler's other parked work. Replaced descriptors reach traps at the next level install, the spawner's install-time precedent.
- **Fixtures.** `trap-volley.map` has no `data_script` and proves trigger wiring alone. `trap-wave.map` (TS) and `trap-wave-luau.map` (Luau) run the Scripting surface example; a map names one `data_script`, so each runtime gets its own map (`teleporters-luau.map` precedent).
- **Non-goals.**
  - Tracking aim: by owner ruling it belongs to a future non-walking enemy.
  - Runtime-adjustable timing: the owner asked for script-parameter waves like the `arena-lights.ts` wave, whose timing is author-time constants. No primitive reads a store slot.
  - A no-script stop on trigger exit: the direct path acts on Enter only. Repeat-while-occupied uses a script `on("exit")` or a second trigger.
  - Traps as trap-pool members: pools arm triggers, and a disarmed trigger issues no command, so pools already gate traps (`done/E18--trap-pools-seeded-arming`).

### Scripting surface
```ts
import { defineReaction, getMapEntities, wait } from "postretro";

const STEP_MS = 250;   // gap between neighbouring traps
const PASSES = 3;
const GAP_MS = 1500;   // pause between passes

export function setupLevel() {
  const traps = getMapEntities("trap", { tag: "wave" })
    .sort((a, b) => a.position.x - b.position.x);

  // Sweep: PASSES passes, each firing the traps in order. fire() runs one execution.
  const pass = traps.flatMap((t, i) => [...(i ? wait(STEP_MS) : []), ...t.fire()]);
  const sweep = defineReaction("wave.sweep", {
    sequence: Array.from({ length: PASSES }, (_, s) => [...(s ? wait(GAP_MS) : []), ...pass]).flat(),
  });

  // Looping wave: every trap repeats on one period; phase staggers them.
  const periodMs = traps.length * STEP_MS + GAP_MS;
  const loopOn = defineReaction("wave.loopOn", {
    sequence: traps.flatMap((t, i) => t.start({ intervalMs: periodMs, phaseMs: i * STEP_MS })),
  });
  const loopOff = defineReaction("wave.loopOff", { sequence: traps.flatMap((t) => t.stop()) });

  return {
    reactions: [sweep, loopOn, loopOff],
    triggerEvents: [
      ...getMapEntities("trigger", { tag: "sweep_plate" }).map((p) => p.on("enter", [sweep])),
      ...getMapEntities("trigger", { tag: "loop_zone" }).flatMap((z) => [
        z.on("enter", [loopOn]),
        z.on("exit", [loopOff]),
      ]),
    ],
  };
}
```
`fire()`, `start(opts?: { intervalMs?: number; phaseMs?: number })`, and `stop()` each return `SequenceStep[]`, like other member handles.

## Acceptance
Pins `P#` are in `research.md` §Ordering pins.

### Automated
**Fire and aim**
- [ ] One `fire` spawns a projectile at the trap origin whose direction matches `angles`, `mangle`, and `angle` (including `-1` up and `-2` down), with the descriptor's speed and damage.
- [ ] A multi-shot sequence produces its authored shot count and spacing from one `fire`.
- [ ] `fire` during an execution or its recovery adds no shots; `fire` after recovery fires. Two `fire`s on one tick give one execution, whatever mix of triggers, trigger edges, and script steps issues them (P1).
- [ ] Two `fire` steps on one trap with no `wait` between them give one execution, while two on one spawner still spawn two batches. Two players entering a zero-re-arm trigger on one tick fire a tagged trap once and a tagged spawner twice (P2).
- [ ] A weapon carrying ammo, heat, or cell keeps firing from a trap past the shot where its magazine, heat threshold, or cell would stop a wielder, and never dry-fires, overheats, or reloads.
- [ ] An unknown, hitscan, or charged-primary weapon leaves the trap inert with exactly one warning; a valid projectile weapon arms. An inert trap fires on no trigger command, script step, or `start_on_spawn` (P13).

**Timing and repeat**
- [ ] A trigger-bound or teleport-bound command acts on its own tick, including a trap step before the first `wait` of a trigger-bound reaction; with a no-script `fire` on the same trap that tick, the two give one execution (P7, P21). A player-event-bound, `levelLoad`, named, or post-`wait` request acts on the next authoritative tick, and post-`wait` steps landing on several ticks of one frame all act on that one next tick (P14, P15).
- [ ] `start_on_spawn` true starts the trap on the first authoritative tick after install, so its first execution lands `phase` after that tick; false stays silent until driven. A `levelLoad` `stop` leaves a `start_on_spawn` trap stopped with no execution, and a `levelLoad` `start(opts)` is a no-op on it (P20).
- [ ] A request latched before a tick applies ahead of that tick's trigger-bound requests: a post-`wait` `start` followed by a trigger `stop` on the next tick fires nothing, and a post-`wait` `stop` followed by a trigger `start` re-anchors on that tick (P17).
- [ ] Over N intervals a started trap fires exactly N executions. The first lands `phase` after the start tick; `phase` 0 fires on the start tick. A phase longer than the interval delays only the first execution (P6). Interval and phase round up to whole ticks; a sub-tick interval repeats every tick.
- [ ] `start` on a running trap changes neither rate nor phase. `stop` halts repeats; an in-flight sequence completes. `stop` then `start(opts)` on one tick re-anchors with the new values; `start` then `stop` on one tick fires nothing (P5). After a run started with script options is stopped, a `start` without options runs at the authored interval and phase. A `start` naming only `phaseMs` runs at the authored interval, and one naming only `intervalMs` runs at the authored phase, in both runtimes.
- [ ] `fire` and a phase-0 `start` on one tick, in either order, give one execution. The next repeat is due one interval after that tick (P3).
- [ ] `fire` and `stop` on one tick, in either order, give one execution and leave the trap stopped (P23).
- [ ] On one Enter, a trigger's no-script `command` acts before its bound reaction's trap steps: `start` then a bound `stop()` leaves the trap stopped (P24).
- [ ] A `fire` between repeats on a started trap does not move the schedule. A repeat that meets that execution's recovery is dropped, and the next one fires on its original tick (P4). A `fire` on the tick a repeat is due gives one execution, and the next repeat stays on its original tick (P19).
- [ ] An interval shorter than one execution plus recovery drops the overlapping repeats and keeps the schedule anchored. Tests stay clear of the exact-equality boundary, which the shared weapon gate decides (P16).
- [ ] A frame that runs several authoritative ticks fires each repeat due within it on its own tick, and a trigger-bound `start` on a later tick of that frame anchors on that tick; shot ticks match a run of one tick per frame (P18).
- [ ] A trap with no `interval_ms`, `phase_ms`, or `start_on_spawn` stays silent until started, then fires on its start tick and every 1000 ms after, rounded up to ticks.
- [ ] A weapon whose primary action is `hold` gives one execution per `fire`, and a started trap with it repeats at the authored interval, not at execution plus recovery.
- [ ] A non-positive interval, a negative phase, or a non-finite value: the KVP gives an inert trap and one warning, and the script literal rejects the reaction at install.
- [ ] A level reinstall clears started state. No schedule, latched request, or parked step from the old install fires a trap in the new one (P10).
- [ ] A mod hot reload leaves a started trap firing on its schedule with its install-time weapon, still without resource limits, and keeps a trap request already latched; a sweep step still parked in a `wait` is dropped with the scheduler's other parked work (P9). The next level install picks up the replaced descriptor.

**Damage**
- [ ] A trap projectile damages a player and an enemy. The enemy records no attacker and does not turn toward the trap. The ledger records the weapon's credit source with no attacker, and no player gets kill credit.
- [ ] A splash weapon damages every damageable volume in radius with a clear static-world sightline, including both a player and an enemy, and neither records an attacker or turns the enemy toward the trap; a volume behind world geometry takes none (regression guard).

**No-script wiring**
- [ ] On one Enter, every trap carrying the trigger's tag fires on that tick.
- [ ] A tag shared by a mover and a trap: `start` starts both; `stop` stops both.
- [ ] On a tag shared by a mover and a trap, `fire` fires the trap and leaves the mover still, and `reverse` reverses the mover and leaves the trap silent. Neither logs a warning. On a tag shared by a trap and a spawner, one `fire` fires the trap and spawns one batch (P12).
- [ ] A `fire` command on a spawner tag spawns one batch per spawner on the host, the same as the spawner's script `fire`.
- [ ] A member carrying a brain is skipped by every trigger command even when its kind accepts the command: a test attaches a brain to a tagged trap and to a tagged mover, and neither acts. An enemy carrying the tag through a spawner's `spawned_tags` is untouched.
- [ ] A `fire` command on a tag carried only by movers logs exactly one warning at install. A trigger with an empty or unmatched `target_tag` logs no command warning. Existing `start` triggers on brainless mover tags behave as before (regression guard).
- [ ] A `multiple` trigger with `rearm_ms` driving a trap tag refires the traps only after re-arm (regression guard).
- [ ] A compiled map whose trigger carries `fire` round-trips through the level file; an out-of-range command value fails to load.

**Co-op**
- [ ] A connected client spawns no gameplay projectile from any trap, including a `start_on_spawn` trap and a step before the first `wait` (P11).
- [ ] Every trap shot gets a host presentation mirror carrying the weapon's descriptor class, and its fire and impact cues carry a shot id that reaches every participant. Traps that fire on one tick carry distinct shot ids.
- [ ] Two players entering the sweep plate, a `multiple` trigger with zero re-arm, on one tick produce one sweep's shots, not two. A second player entering while the traps are still in execution or recovery adds no shot to those traps (P8).
- [ ] A host snapshot of a level with a started trap carries no trap state, and the wire and snapshot versions are unchanged (a diff gate on the two constants).

**Surface**
- [ ] The `trap-wave` fixtures, run as compiled levels through the authoritative tick and the frame-end drains with a player entering `sweep_plate`, fire their traps in sweep order on the shot ticks their constants imply under the timing rule (P14), in both TS and Luau. The recorded shot ticks match between runtimes.
- [ ] In each `trap-wave` fixture, entering `loop_zone` fires trap i first at its rounded phase after the Enter tick, then once per rounded period. Leaving it on a tick a repeat is due fires nothing on that tick or after (P22).
- [ ] The `trap-volley` fixture (no script), run as a compiled level through the authoritative tick with a player entering each trigger, fires every tagged trap on the Enter tick.
- [ ] SDK typedefs, drift snapshots, and TS/Luau parity fixtures cover the `"trap"` kind and its handle. Test filters matched tests.

### Manual
- [ ] All three fixture maps play: shots are readable, the `trap-volley` triggers land their traps together, and the sweep and looping wave read as waves.
- [ ] Co-op with one client on `trap-volley`: the client sees trap projectiles, hears fire and impact at the right place, and takes damage.
- [ ] TrenchBroom shows `projectile_trap` with a facing arrow and offers `fire` on `trigger_volume` and `switch`.

## Path
Non-binding.
- **Fire driver.** Mirror the enemy loop in `AiWeaponControllers::advance_actor` (`advance_weapon_activation` → `freeze_weapon_shot` → hand-built `ProjectileLaunch` → spawn, with `WeaponActivationCheckpoint::restore` on failure) for a trap-private `WeaponComponent` built by `from_descriptor_with_canonical` from a descriptor clone with its resource stripped, so `shot_scale_inputs` yields `ShotResourceCost::None`. Do not copy the controller's live refresh in `sync`. Advance every trap every tick so recovery drains. Run after `projectile_stage::advance` and feed spawned ids to `projectile_stage::finish_spawn_tick`.
- **No attacker.** Split credited attacker from owner on the projectile so the impact closure and `emit_splash_damage` stop deriving attacker from `registry.exists(owner_pawn)`. The trap stays the owner, which feeds `observer_shot_id` and `projectile_collision_excludes`.
- **Trigger command.** `TriggerVolumeComponent.command` becomes a trigger-level command over brainless kinds rather than a `MoverCommand`. Touch `resolve_trigger_volume`, `level-format` `from_bytes`, `trigger_volume_bridge.rs`, the `prl_loader.rs` command counts, and the Enter branch of `TriggerSystem::run_authoritative_tick_with_dispatch`. Spawner `fire` can reuse `spawn_from_spawner_member` (`sim/spawner.rs`), which the trigger tick already reaches.
- **Spawner precedent** for everything else. `builtins/entity_spawner.rs` + `builtins/mod.rs` registration; `resolve_spawners_for_level` for install-time descriptor resolution; `wrapSpawnerEntity`/`MapEntityKind`/`MAP_KIND_COMPONENTS`; the Luau prelude and `light_membership.rs` kind lists; `bind_command` → `BoundTriggerCommand` at the trigger, teleport, and player-event sites; `partition.rs`; `is_trigger_consequential_primitive`; `register_sequenced_spawner_primitives`; `data_descriptors/{js,lua}/reactions.rs`; typedef templates and fixtures. Host gate: `SpawnContext::can_materialize_runtime_spawns`.
- **Presentation.** Feed an enemy-shaped spawn list (`EnemyProjectilePresentationSpawn`) through `host_spawn_projectile_presentations`. The class is the resolved canonical weapon name.
- **Fixture harness.** The `--headless` observability driver runs no trigger stage and no landing drain. Prove fixtures through the compiled-level harness the teleporter surface tests use (`startup/teleporters_surface_tests.rs`). In co-op the looping wave stops when the first player leaves `loop_zone`, which is why the manual co-op pass uses `trap-volley`.
- **Rival shape.** Map `start`/`stop` onto the weapon's own `hold` activation, so cadence is execution plus recovery and phase is a scheduler `wait` before `start`. It needs no trap scheduler, but moves placement timing (Quake's `wait`) into weapon descriptors, so one weapon cannot repeat at different rates in different traps.
- **First slice.** A `start_on_spawn` trap in a fixture map with no trigger and no script, through the compiled-level harness: assert shots, attacker `None`, and a mirror per shot. This falsifies the driver, attribution, and presentation assumptions before any authoring surface exists.
- **Large files.** `trigger_system.rs`, `sim/mod.rs`, `projectile_stage.rs`, and `postretro/main.rs` exceed 800 lines. Land trap logic in new modules so these files take only call-site hooks; no pre-split.

## Open questions
- Whether `TRIGGER_VOLUMES_VERSION` advances for the new command value — **delegated**: the executor decides per `build_pipeline.md` §PRL section IDs and reports it.
- Command-to-kind dispatch shape inside the trigger tick — **delegated**.

## Boundary inventory
| Name | Rust | Level file / wire | JS / TS | Luau | FGD |
|---|---|---|---|---|---|
| Trap entity | new `ComponentKind`, appended last | reserved discriminant, never replicated | kind `"trap"`; component key `"projectile_trap"` | kind `"trap"` | class `projectile_trap` |
| Weapon reference | canonical descriptor name | — | — | — | `weapon` |
| Repeat interval | milliseconds, rounded up to ticks (minimum one) | — | `intervalMs` | `intervalMs` | `interval_ms` |
| Repeat phase | milliseconds, rounded up to ticks (zero allowed) | — | `phaseMs` | `phaseMs` | `phase_ms` |
| Auto start | bool | — | — | — | `start_on_spawn` |
| Handle primitives | reaction primitive names | — | `fireTrap`, `startTrap`, `stopTrap` | same | — |
| Trigger fire command | trigger command variant | section 44 `command` = 4 | — | — | `command` choice `fire` |

## Wire format
PRL section 44 (trigger volumes): the `command` u8 gains value 4 = `fire`. Existing values unchanged: 0 `start`, 1 `stop`, 2 `reverse`, 3 `go_to_path_node`. Field order, widths, and entry layout are unchanged; the decoder accepts 0–4 and rejects anything larger. No network payload changes: `WIRE_VERSION` and `SNAPSHOT_VERSION` hold.
