# E25--landmark-level-change

Brief · compact · Epic 25 U2 · reads: `context/lib/movement.md` §6 (Teleport) · `context/lib/entity_model.md` §7, §7d · `context/lib/build_pipeline.md` §Custom FGD · `context/lib/boot_sequence.md` §3, §4 · `context/lib/networking.md` §Slot lifecycle, §Session-state ledger · `context/lib/scripting.md` §12 · `plans/done/E18--player-life-states/` · `plans/done/E18--coop-activation-policy/` · hub `drafts/E25--map-transitions/` (D4, D5, D9–D11, AC 3–9, U2 pins, Boundary inventory) · read at 6c862fd (drafted at ab08438)

**Sequence:** after `coop--client-spawn-view-handoff` (done), whose host-side spawn-pose resolver this brief extends; after `E18--coop-activation-policy` (reopened), whose occupancy refs it extends; and after the `streaming_seam_volume` → `stream_split_volume` rename (direct build), so "seam" names only the map transition. Concurrent with U1.

## Problem
Owner-raised, as Epic 25's second unit. A level change today is a hard cut to a player start: `loadLevel` carries a catalog id and nothing else, every pawn spawns at `player_spawn`, and only the seat's carried loadout survives. Nothing lets an author join two maps so a player walks out of one and into the other where they left off. When done, an author places a **seam** — a volume built at the same spot in both maps, with a shared name — and a player who activates it crosses behind a loading screen into the next map at the same place relative to the seam's landmark, facing the same way, moving the same way, with loadout carried. Other players arrive at player starts. This is the seam's author surface and transition contract; U3 later removes the loading screen without changing either.

## Decisions
- **A seam is its own entity, built on trigger activation.** Brush entity `level_seam`, compiled to its own PRL section plus an id-29 companion, with its own script kind and event source, as `trigger_teleport` is. Not `switch`-style sugar over section 44: that record has no room for a target, a mode or a landmark, and a desugared seam would publish trigger events, not seam events. Engine mechanism, authored placement.
- **FGD keys** (one-way door; the Boundary inventory pins spellings): `name` (required; the seam name both maps share), `target` (catalog id of the next map; required), `activation` (`touch` default, `use`), `mode` (`classic` default, `seamless`; until U3, `seamless` runs classic with no warning), `landmark` (optional `teleport_destination` name), `enabled_on_spawn` (default 1), `on_arrive` (reaction name), `_tags`. Defaults mirror `trigger_volume` and `trigger_teleport` so a minimal seam is name plus target (D9). A duplicate seam name in one map, an unresolved landmark, or a missing name or target fails the build; a seam in a map with no `player_spawn` warns. At activation the seam checks its target against the catalog: an unknown id warns once, requests nothing, and leaves the level running.
- **Landmark.** An authored landmark reuses `teleport_destination` (origin and yaw), as `ghost_link` does. Absent, it is the seam volume's bounds centre at yaw 0 (D9).
- **One inside test: capsule against the brush union.** Every use — activation, arrival, latch, the occupancy refs, and U3's eyes (D11) — tests the upright player capsule against the union of the seam's convex brushes. Not the trigger's union AABB: a bent hallway, the seam U3 exists for, would count the rooms beside its bend as inside, and requiring a box would forbid it.
- **Activation requests a classic level change carrying the seam.** The existing load coalescing, cancellation and failure rules apply, with one exception: when seams with different targets or names activate on one frame, the first in map order wins, as hub D7 orders seams for U3. Host and single player only. A disarmed seam never activates. The destination seam's armed state affects neither arrival nor its `arrive` event.
- **Arrival is read before unload tears anything down.** At every unload, before its first teardown step, an arrival harvest writes each seat's **arrival**: when the load carries a seam and the pawn is inside that seam, the body offset from the landmark (vertical included), view yaw and pitch relative to the landmark, every velocity channel and the movement-state payload, all in the landmark's frame; otherwise none, including for a seat with no bound pawn. View comes from the host camera; for a client, from the slot's latest resolved command in the pawn's teleport frame, or the pawn's own facing when none has resolved. An arrival belongs to the load that carried the seam: suspend, and a load that fails or is cancelled, clear every arrival. Carried state is unchanged (hub I3).
- **The first spawn consumes the arrival; nothing else reads it.** A seat's next pawn spawn in the next level takes its arrival, mapped through the destination seam's landmark, instead of a player start, as the class of the start the seat would otherwise take. A respawn never reads it, and the next unload clears an unconsumed one. The seat's arrival joins the session-state ledger (`networking.md` §Session-state ledger, rewritten at promotion).
- **One arrival rule, shared with teleports** (hub D4, I4). Teleports and seam spawns apply the same arrival: channel and boost rotation, vault abort to `Normal`, arrival ground settle, blocked-arrival fit, and latch against every armed teleporter containing the body point. The mapping is the teleport's seamless mapping from the source landmark to the destination landmark. Pitch passes through unchanged, as on a teleport. A blocked arrival or a destination with no seam of that name places the pawn at a player start with one warning. The blocked test runs when the pawn spawns: at install for the host's pawn; for a client's, when its pose resolves and again at promotion, which falls back to the seat's player start and resends the hint.
- **Host view and Settling follow the arrival.** On an arriving host or single player, the presented pose is the arrival eye, yaw and pitch, so Settling settles the view the player reveals into.
- **Co-op: the client spawns at the resolver's pose.** A client pawn still spawns at participation entry (`networking.md` §Slot lifecycle, entry rule, rewritten at promotion). The host's one spawn-pose resolver (`coop--client-spawn-view-handoff`) resolves to the seat's mapped arrival when it has one and it fits, else the seat's player start, so the client's hint, its Settling view and its spawned pawn agree on eye, yaw and pitch. A seat resolving to an arrival reserves no player start. The arrival is a single-use source, so a re-promotion or rejoin after the first spawn resolves to a player start. E25 adds nothing to the relevel message (catalog id and level life): the host places every pawn.
- **Arrival latches; arming keeps latches** (hub D11, latch table). Every pawn spawn and every respawn latches the pawn against every seam its capsule overlaps, before any trigger tick sees it, so a two-way seam and a player start inside a seam never fire straight away. A teleport is not a spawn and latches no seam. A latch clears when the capsule stops overlapping. Arming keeps latches, unlike a teleporter's arm. The latch is host-side state, off the wire: seams run only in the host's trigger stage.
- **The arrival event.** The destination seam fires `arrive` once per arriving pawn, host-only, inside the first fixed tick after the pawn spawns, as teleport events dispatch inside their tick. Its params are opaque `activators` and `seam` tokens. U3 adds `waiting` and `ready` to the same source.
- **Who may activate: as a teleporter.** A seam moves the whole party, so its default matches the teleporter's: a pawn at zero health that declares no life states cannot activate it. A pawn that declares life states follows E18's `touch` and `use` capabilities, so whether a downed player can trigger the exit is the author's, per state.
- **Life state crosses as a level change.** A pawn inside the seam crosses landmark-relative whatever its life state; the seam changes where it arrives, never its life or health, which follow the level-change rule (E18 O7).
- **A seam is an occupancy volume.** `occupiedCount(tag)` and `occupants(tag)` (`E18--coop-activation-policy`, reopened: its refs never shipped, and this brief follows it) count seams carrying the tag by the seam's own inside test, armed or not, through the effective-occupant rule triggers use. A wait-for-everyone policy arms a disarmed seam from a crossing on those refs, and reads exactly the test the engine moves players by; a covering `trigger_volume` would count a bent seam's whole box. No new SDK name beyond that brief's.
- **Non-goals.**
  - A seam argument on `loadLevel`. A scripted transition arms a disarmed touch seam, which fires for its standing occupants on the next trigger tick. It cannot send back players latched in the seam they arrived by; the argument is cheap to add later.
  - Moving `trigger_volume` to the brush-union test. Triggers keep their union AABB, documented as a known limitation; changing every shipped trigger is its own work.
  - Anything but player pawns crossing. NPCs, items and projectiles inside the seam stay behind (hub Out of scope).
  - Prefetch, the blind check, `waiting` / `ready` and their compiler warnings: hub U3.

### Scripting surface
```typescript
import { defineReaction, getMapEntities } from "postretro";
import type { SeamEventParams } from "postretro";

export function setupLevel() {
  const airlock = getMapEntities("seam", { tag: "airlock" });
  const cycle = defineReaction("airlock.cycle", { sequence: airlock.flatMap((s) => s.arm()) });
  const sealed = defineReaction("airlock.sealed", (on: SeamEventParams) => on.seam.disarm());
  return {
    reactions: [cycle, sealed],
    triggerEvents: getMapEntities("trigger", { tag: "airlock_button" }).map((t) => t.on("enter", [cycle])),
    seamEvents: airlock.map((s) => s.on("arrive", [sealed])),
  };
}
```
Luau mirrors it as teleporters do: `s:arm()`, `s:disarm()`, `s:on("arrive", { reaction })`, `seamEvents` under `setupLevel`. Both ship.

## Acceptance
### Automated
**Arrival**
- [ ] On a fixture pair, a player crossing a classic seam arrives with the same offset from the landmark (vertical included), the same view yaw and pitch relative to it, and every velocity channel rotated by the landmark yaw delta. A sliding or dashing player continues on the rotated boost; a vaulting one arrives in `Normal`. The arrival equals a seamless teleport between the same poses.
- [ ] Facing and velocity are read before unload: a host and a co-op client crossing while turned and moving arrive with their own facing and velocity, not a reset or default.
- [ ] A seam with no landmark uses its bounds centre at yaw 0. An authored landmark's yaw rotates the arrival.
- [ ] A destination with no seam of the crossing name, and an arrival blocked at spawn, each place the pawn at a player start with one warning.
- [ ] An arriving player overlapping an armed teleporter in the next map is latched against it and does not teleport.
- [ ] An arriving host settles and reveals at the arrival eye, yaw and pitch, not the first player start.
- [ ] A pawn at zero health inside the seam at the harvest arrives landmark-relative, with health as a level change restores it.
- [ ] A destination seam with `enabled_on_spawn` 0 places every crossing pawn landmark-relative and latches it, exactly as an armed one does.
- [ ] A co-op client crossing a seam while its resolved commands still carry a pre-teleport stamp arrives facing as its host-side pawn faced (pin P8).

**Carry and consumption**
- [ ] Carried state crosses a seam exactly as it crosses `loadLevel` today.
- [ ] A seat's arrival is consumed by its first spawn. A later respawn, and the level after a non-seam change, place it at player starts with no warning, the later level holding a seam of the crossing's name (pin P2).
- [ ] A pawn outside the seam at the harvest arrives at a player start, including a pawn that activated the seam and left it before the load drained (pin P1).
- [ ] A seam load that fails, or that a queued unload cancels, leaves no arrival: the next level loaded places every seat at a player start with no warning (pin P12).
- [ ] Suspend before a seam's load drains, during its load, and while a crossing client still loads the destination each leave no arrival: after resume every seat spawns at a player start (pin P6).

**Activation**
- [ ] Touch and use activation each start the transition. Use without overlap does not; a disarmed seam does not. A seam activation and a script `loadLevel` on one frame load exactly one level, and only a seam load writes arrivals (pin P3).
- [ ] An unknown target catalog id warns once at activation, requests no load, and leaves the level running.
- [ ] Two seams with different targets activating on one frame load the first in map order's target; the other seam's occupants arrive at player starts (pin P11).
- [ ] A pawn whose capsule overlaps the seam's union AABB but none of its brushes is outside: it neither activates the seam nor arrives landmark-relative.
- [ ] Arming a disarmed touch seam with a player standing inside starts the transition on the next trigger tick.
- [ ] A pawn at zero health that declares no life states neither touches nor uses a seam into firing, as it cannot teleport. A declared pawn follows its state's `touch` and `use` capabilities.
- [ ] `occupants(tag)` counts a player in a bent seam's brushes and not one beside the bend inside its bounding box, armed or not, and not an undeclared pawn at zero health inside it. A disarmed seam armed from `onStateCrossing(occupants(tag), …)` when every player is inside moves every player landmark-relative.

**Latch and events**
- [ ] The latch follows the hub D11 table on every row, including an arrival whose capsule overlaps the seam with its body point outside, and a capsule that leaves while the body point is already out (pin P4).
- [ ] The destination seam fires `arrive` once per arriving pawn, on the host only, inside the first fixed tick after the pawn spawns, and fires none for a pawn of the same load placed at a player start, a blocked arrival included. A brush `on_arrive` and a script `seamEvents` binding both run.
- [ ] A teleporter whose destination lies inside a touch seam fires that seam on the teleport's tick (pin P9).
- [ ] A pawn respawned onto a player start inside a touch seam is latched and does not fire until its capsule leaves and re-enters (pin P14).
- [ ] A disarmed destination seam fires `arrive` for each arriving pawn, as an armed one does.
- [ ] A seam whose target is its own map reloads it once, and each occupant spawns where it stood, latched (pin P10).
- [ ] The scripting-surface example compiles in TypeScript and Luau and, on the dev pair, disarms the destination seam on arrival.

**Co-op**
- [ ] A client pawn inside the seam arrives landmark-relative at participation entry; one outside arrives at its assigned player start.
- [ ] A crossing client settles, reveals and arms at its arrival eye, yaw and pitch, and its seat holds no player start. A client whose arrival is blocked resolves to a player start, and its hint names that start. A client whose arrival fit at resolve but is blocked at promotion, by a mover or the host's pawn, spawns at its player start with one warning and a resent hint (pin P13).
- [ ] A crossing client with no resolved command at the harvest arrives with its pawn's own facing.
- [ ] A host level change before a crossing client spawns clears that client's arrival; it arrives at a player start (pin P5).
- [ ] A crossing client demoted or reconnected after its arrival spawn re-enters at a player start, and its hint names that start; one that reconnects before any spawn in the destination spawns at its arrival (pin P7).

**Compiler and surface**
- [ ] A duplicate seam name in one map, an unresolved landmark, and a missing name or target each fail the build with the entity named. A seam in a map with no `player_spawn` warns. Blank optional keys take their defaults.
- [ ] An arrival spawns as the class of the destination start its seat would otherwise take.
- [ ] The FGD defaults equal the compiler's, by test.
- [ ] A seam with `mode` `seamless` crosses exactly as a classic one and logs no warning.

### Manual
- [ ] On the dev-mod seam pair, a player crosses into the next map and back, turned and moving, and keeps facing and momentum.
- [ ] A co-op session crosses the pair with one player inside the seam and one outside.

## Path
- Shape: a `trigger_teleport`-style entity — section, companion, component, `getMapEntities("seam")` kind, arm and disarm verbs, a seam event source and binding table. Activation reuses the trigger stage's touch and use edges and its activation gate.
- Rival A, the strongest: a passive seam marker (name, region, landmark) with a `trigger_volume` calling `loadLevel(id, { seam })`. Better if U2 were the end; it loses because U3 needs the engine to own activation and the latch, and the owner kept U3 (hub, "Stop after U1 and U2", rejected 2026-10-28).
- Rival B: a target-map key on `trigger_teleport`. Teleporters fire per pawn on the body point, predicted, latches on the wire; a seam moves the party, host-only, at a frame boundary.
- Rival C, compile layout only: `switch`-style sugar over section 44 plus a side table keyed by trigger index; cheaper to compile, but triggers have no companion and would publish trigger events.
- Seams: `TeleporterParser` and `region_companion` (compile), `TeleportersSection` (layout precedent), `spawn_loaded_teleporters` / `bind_loaded_teleporters` (install), `TeleporterBindingTable` (events), `trigger_system`'s edge and gate code (activation), `physics::movement::teleport::resolve` (factor), `unload_level`, `SeatTable::harvest_bound_pawns`, `spawn_from_player_starts_with_carried_loadout`, `spawn_net_slot_pawn_with_carried_loadout`, the participation-entry handler in `net_poll_and_apply`, `place_camera_at_presented_pose`, `LevelRequest` / `LevelSource`.
- Inside test: capsule against each convex brush by its planes inflated by the radius, about the segment; it over-includes near edges by at most the radius, the safe direction for activation and the conservative one for U3's eyes. Teleporter regions already store convex brush planes.
- Client facing at harvest: `HostCommandQueues` keeps the latest facing yaw and aim pitch per client but exposes no facing accessor; the facing turns through `facing_in_teleport_frame`.
- The movement state payload carries `elapsed_ms`; a dash or slide resumes, not restarts.
- Occupancy: the reserved per-tag slots the trigger stage writes from its effective-occupant set each host tick; seams join that set through their own test.
- Request: activation enqueues the catalog load through the ordinary request queue (`LevelRequest` / `LevelSource`) with the seam's name attached. A later `Load` drops queued ones at enqueue (last wins), and the trigger stage walks volumes in `EntityId` order, not map order, so first-in-map-order needs its own pick.
- Harvest: at the top of `unload_level`, ahead of `clear_net_level_parity`, as suspend already orders its own harvest; the carried-state harvest stays where it is. U1's beside commit also calls `unload_level`.
- Arrival rule: the apply half of the teleport chokepoint factors out of `resolve` and both callers use it, with `SeamlessMapping` built from the two landmarks. Promotion keeps a recorded unbacked pose today (`resolve_for_promotion`); the re-test is new.
- Arrival code already shared: `physics::movement::teleport::settle_arrival` holds the ground settle, record bump and teleporter latch rebuild, and respawn calls it. The fit test, boost rotation, vault abort and velocity rotation remain inline in `resolve`; those are what to factor.
- Client arrival through the resolver: `UnbackedSpawn` carries only `SpawnPose` (eye, yaw, pitch), `entity_class` and `single_use`. Velocity channels, movement state and the seam latch need their own path into the participation-entry spawn beside it.
- Activation gate: the trigger stage lets an undeclared zero-HP pawn touch and use (`PlayerCapabilities::permits_touch` / `permits_use` default to permit); the seam adds the teleporter's zero-HP bar on top.
- First slice: factor the rest of the apply half out of `resolve` with teleport tests green, then a host-only seam on a fixture pair with the harvest moved. That falsifies the riskiest assumptions: the chokepoint splits cleanly, and nothing between the old and new harvest point depended on the old order.
- `startup/lifecycle.rs`, `trigger_system.rs` and `level-compiler/src/parse.rs` are past ~800 lines: split first, behavior-preserving, own commits.
- Docs: `docs/level_design.md` gains a seam section (it has none for triggers either), `docs/scripting-reference.md` the handle, `docs/modding.md` the index; `build_pipeline.md` corrects "each brush" for `trigger_volume` at promotion, recording its union-AABB inside test as a known limitation.
- Research: `research.md`.

## Wire format
New PRL section `Seams`, next free id, packed only when the map has a seam; mirrors `TeleportersSection` (id 55): little-endian, `u32` version then `u32` record count, strings `u32`-length-prefixed UTF-8, an empty string meaning absent. Per record, in order: `source_entity` (`u32`, id-29 companion index), `name`, `target`, `activation` (`u8`: 0 touch, 1 use), `mode` (`u8`: 0 classic, 1 seamless), `enabled_on_spawn` (`u8`), brushes (`u32` count, each a `u32` plane count then planes as four `f32`), `landmark_origin` (three `f32`), `landmark_yaw` (`f32`), `on_arrive`. The landmark is always resolved at compile time, defaulted or authored. A seamless record carries nothing extra until U3.

## Boundary inventory
| Concept | FGD | PRL | Rust | TypeScript | Luau |
|---|---|---|---|---|---|
| Seam | `level_seam` brush | `Seams` record | seam component | `getMapEntities("seam")`, `SeamHandle` | same, `SeamHandle` |
| Landmark | `landmark` key → `teleport_destination` | `landmark_origin`, `landmark_yaw` | — | — | — |
| Arm / disarm | `enabled_on_spawn` | `enabled_on_spawn` | `armSeam` / `disarmSeam` primitives | `s.arm()`, `s.disarm()`, `on.seam` | `s:arm()`, `s:disarm()` |
| Arrival event | `on_arrive` | `on_arrive` | seam binding table | `s.on("arrive", …)` under `seamEvents`; `SeamEventParams` | `s:on("arrive", …)` |
| Occupancy | `_tags` | tags | per-tag occupancy slots | `occupiedCount(tag)`, `occupants(tag)` (`E18--coop-activation-policy`) | same |

## Open questions
- Event key spellings beyond the Boundary inventory's — **delegated**.
- Where the seam request rides between the trigger stage and the request queue (in-tick enqueue or a frame-end residual), within pin P3 — **delegated**.
