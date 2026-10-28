# Landmark level change — research

Read at `ab08438`. Session-verified facts behind the brief, cited by symbol. Hub: `drafts/E25--map-transitions/` (U2).

## 1. Teleport arrival today

- **Chokepoint.** `physics::movement::teleport::resolve` (private to the movement module) runs as the last step of `tick_with_immersion_and_armed`, the shared host / prediction / replay entry. In order: prune latches the body point left; skip when `vault_barred`; pick a winner; compute arrival and yaw delta (classic sets view yaw to the destination's; seamless is relative through `SeamlessMapping`); refuse a blocked arrival (`capsule_fits_combined`, one warning per teleporter through `warn_refused`); rotate `Dash`/`Sliding` boost; abort `Vault` to `Normal` with a `vault_ended` edge; rotate `velocity`, `knockback_velocity`, `push_velocity`; `settle_arrival_ground`; bump the teleport record and rebuild latches; emit `TeleportEvent`.
- **Not separable today.** `settle_arrival_ground`, `capsule_fits_combined`, `SeamlessMapping` and `wrap_teleport_yaw` stand alone. The rotation, vault abort and record update are inline, tied to the current map's teleporter index (`warn_refused`, `last_teleporter`, latch rebuild against `static_world.teleporters.regions`).
- **Callers own the view.** Host movement and client replay write `Transform.rotation` from facing plus the delta and cut presentation (`set_presentation_transform`, `hold_local_arrival`); the sim tick turns cached aims; `apply_local_camera_turn` turns the host camera. `movement.md` §6 overstates the chokepoint ("a presentation cut").
- **Pitch never changes.** `SeamlessMapping` is yaw-only; destination pitch is dropped at compile time.
- **Latches.** Per pawn, on `PlayerMovementComponent.teleport.arrival_latches`, sorted teleporter indices, wire-mirrored. Arming a disarmed teleporter clears latches against it on the host only (`apply_pending_teleporter_mutations`).
- **Events and script surface.** `TeleporterBindingTable::execute` runs inside the authoritative tick, host and single player only, after movement and before Triggers; presentation work drains at frame end. Script: `getMapEntities("teleporter")`, `arm()` / `disarm()` / `on("teleport", fire)` under `setupLevel`'s `teleportEvents`; params are opaque `activators` and `teleporter` tokens; brush key `on_teleport`.
- **Compile and PRL.** `trigger_teleport` and `ghost_link` each own a section (55, 56) and write an id-29 companion through `region_companion`; `teleport_destination` is a yaw-only named point both reuse.

## 2. Trigger volumes

- **Compile.** `resolve_trigger_volume` builds one engine-space AABB per entity, the union of its brushes (`build_pipeline.md` says "each brush"; stale). Section 44 is a fixed layout; triggers write no id-29 companion. `switch` desugars into the same record, so it has no channel for extra keys.
- **Inside test.** `capsule_overlaps_aabb` against the pawn's upright capsule — a capsule test, not a body point. Only canonical player pawns take part.
- **Activation.** Touch fires on an entry edge, or on the first evaluation after arming while overlapping (`touch_reactivation_pending`). Use fires on a fresh press while overlapping, first fixed tick of the frame only; remote presses come from the command. No runtime use reach. Gate: `armed && !(once && latched) && rearm elapsed`.
- **Tick placement.** Movers → host movement → teleport dispatch → Triggers → touchables → projectiles → AI → traps. Host and single player only.
- **Lifecycle primitives.** `loadLevel`, `restartLevel`, `returnToFrontend` ride the residual steps and drain at frame end. `loadLevel(id)` carries only `{ map }`; an extra arg is silently ignored.

## 3. Level change

- **`unload_level` order** (`startup/lifecycle_net.rs`): CPU timer; **net reset** (`clear_net_level_parity` → `reset_level_scoped_host_state` replaces `command_queues`); client reset; audio; renderer release; streaming clear; `level = None`; surface-lifetime clear (weapon activations cancelled, triggers, collision, teleporter bindings); nav, environment; session clears; **`harvest_bound_pawns`**; **registry despawn**; `clear_pawn_bindings_for_level_unload`; UI; **camera reset** to the origin, yaw and pitch 0.
- **Harvest placement.** At the top of `unload_level` the registry and pawn bindings are intact. Suspend already harvests before the net reset (`suspended`). Only production caller of unload: `drain_level_requests`.
- **Carried state.** `CarriedState` holds health, reserve, wieldables, magazines and active slot; cleared only by `release_seat`. No per-seat record is consumed once by a first spawn today.
- **Placement.** Host pawn spawns at install stage 11 (`spawn_from_player_starts_with_carried_loadout`, first spawn in `first_player_spawn_order`). A client pawn spawns at participation entry, after the host's reveal, through `assign_joiner_placement` and `spawn_net_slot_pawn_with_carried_loadout`. No engine respawn exists; mods bind `playerDied`.
- **Facing sources.** Host: `App.camera` yaw and pitch. Client on the host: `HostCommandQueues` keeps `latest_facing_yaw` and `latest_aim_pitch` per client (no facing accessor); the facing converts through `facing_in_teleport_frame`. The queues reset at the start of unload.
- **Camera at install.** Seeded from the first player spawn's angles, then the presented pose. On a client this is the host's first spawn; arming sets position only (`coop--client-spawn-view-handoff` documents the gap).
- **Relevel.** `ServerControlMessage::Relevel(String)` carries the catalog id only; the client follows it and the host places every pawn.

## 4. Naming

`streaming_seam_volume` is a keyless convex brush marking a best-effort SH warm-up portal; no gameplay or visibility effect. "Seam" otherwise names code boundaries and the teleporter's `seamless` mode. The map-transition seam exists only in Epic 25 plans.

## 5. Dev content

`content/dev/scripts/frontend-menu.ts` `defineMapCatalog` holds entries `{ id, path, name, tags, loadingTree }`. `*.prl` is gitignored except force-committed fixtures under `content/dev/maps/test-fixtures/` (README documents regeneration).

## 6. Ordering pins

| Pin | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Activator leaves before the load drains | activate on tick T → capsule leaves the seam on a later tick of the same frame → redraw boundary drains the load → harvest | The harvest's inside test decides: that pawn arrives at a player start. Arrival is never read at activation |
| P2 | Arrival consumed, then a respawn and a later change | seam crossing → first spawn takes the arrival → death → mod respawn → `loadLevel` to a third map | The respawn and the third map place at player starts |
| P3 | Seam activation and script `loadLevel` on one frame | seam activates in the trigger stage → a reaction's `loadLevel` drains at frame end → next boundary drains the queue | One load. Arrivals are written only when the load that drains is the seam's |
| P4 | Latch clears on capsule exit | touch seam latched → body point leaves, capsule still overlaps → capsule leaves → re-enter | No fire while the capsule overlaps; re-entry fires (hub pin) |
| P5 | Level change before a co-op client arrives | seam crossing A→B → client still loading B → host changes B→C | The client arrives in C at a player start; its B arrival is cleared at B's unload (hub pin) |
