# E25--landmark-level-change

Brief · compact · Epic 25 U2 · reads: `context/lib/movement.md` §6 (Teleport) · `context/lib/entity_model.md` §7, §7d · `context/lib/build_pipeline.md` §Custom FGD · `context/lib/boot_sequence.md` §3, §4 · `context/lib/networking.md` §Slot lifecycle, §Session-state ledger · `context/lib/scripting.md` §12 · hub `drafts/E25--map-transitions/` (D4, D5, D9–D11, AC 3–9, U2 pins, Boundary inventory) · read at ab08438

## Problem
Owner-raised, as Epic 25's second unit. A level change today is a hard cut to a player start: `loadLevel` carries a catalog id and nothing else, every pawn spawns at `player_spawn`, and only the seat's carried loadout survives. Nothing lets an author join two maps so a player walks out of one and into the other where they left off. When done, an author places a **seam** — a volume built at the same spot in both maps, with a shared name — and a player who activates it crosses behind a loading screen into the next map at the same place relative to the seam's landmark, facing the same way, moving the same way, with loadout carried. Other players arrive at player starts. This is the seam's author surface and transition contract; U3 later removes the loading screen without changing either.

## Decisions
- **A seam is its own entity, built on trigger activation.** Brush entity `level_seam`, compiled to its own PRL section plus an id-29 companion, with its own component, script kind and event source, as `trigger_teleport` and `ghost_link` are. Activation reuses the trigger stage's touch and use edges and its activation gate, so a pawn that cannot activate a trigger cannot activate a seam (E18's `touch` and `use` capabilities included, once built). Not `switch`-style sugar over section 44: that record has no room for a target, a name, a mode or a landmark, and a desugared seam would publish trigger events, not seam events. Engine mechanism, authored placement.
- **FGD keys** (one-way door; the Boundary inventory pins spellings): `name` (required; the seam name both maps share), `target` (catalog id of the next map; required), `activation` (`touch` default, `use`), `mode` (`classic` default, `seamless`; until U3, `seamless` runs classic with no warning), `landmark` (optional `teleport_destination` name), `enabled_on_spawn` (default 1), `on_arrive` (reaction name), `_tags`. Defaults mirror `trigger_volume` and `trigger_teleport` so a minimal seam is name plus target (D9). A duplicate seam name in one map, an unresolved landmark, or a missing name or target fails the build; an unknown catalog id warns once at activation and does nothing, as `loadLevel` does.
- **Landmark.** An authored landmark reuses `teleport_destination` (origin and yaw), as `ghost_link` does. Absent, it is the seam volume's bounds centre at yaw 0 (D9).
- **One inside test: capsule against the brush union.** Every use — activation, arrival, latch, and U3's eyes and occupancy (D11) — tests the upright player capsule against the union of the seam's convex brushes. Not the trigger's union AABB: a bent hallway, the seam U3 exists for, would count the rooms beside its bend as inside, and requiring a box would forbid it.
- **Activation queues a classic level change carrying the seam.** It enqueues the catalog load through the ordinary request queue with the seam's name attached, so the existing coalescing, cancellation and failure rules apply. Host and single player only. A disarmed seam never activates. The destination seam's armed state does not affect arrival.
- **Arrival is read before unload tears anything down.** An arrival harvest runs at the top of `unload_level`, ahead of the net reset, as suspend already orders its own harvest. The carried-state harvest stays where it is, so carried loadout keeps today's rule. At every unload, for each seat it writes the seat's **arrival**: when the load carries a seam and the pawn is inside that seam, the body offset from the landmark (vertical included), view yaw and pitch relative to the landmark, every velocity channel and the movement-state payload, all in the landmark's frame; otherwise none, including for a seat with no bound pawn. Suspend clears every arrival. View comes from the host camera, or from the slot's latest command for a client. Carried state is unchanged (hub I3).
- **The first spawn consumes the arrival; nothing else reads it.** A seat's next pawn spawn in the next level takes its arrival, mapped through the destination seam's landmark, instead of a player start. A respawn never reads it, and the next unload clears an unconsumed one. The seat's arrival joins the session-state ledger (`networking.md` §Session-state ledger, rewritten at promotion).
- **One arrival rule, shared with teleports** (hub D4, I4). The apply half of the teleport chokepoint — channel and boost rotation, vault abort to `Normal`, arrival ground settle, blocked-arrival fit, latch against every armed teleporter containing the body point — is factored out of `resolve`, and teleports and seam spawns both call it. The mapping is `SeamlessMapping` with the source landmark and the destination landmark. Pitch passes through unchanged, as on a teleport. A blocked arrival or a destination with no seam of that name places the pawn at a player start with one warning; the blocked test runs at spawn.
- **Host view and Settling follow the arrival.** On an arriving host or single player, the presented pose is the arrival eye, yaw and pitch, so Settling settles the view the player reveals into.
- **Co-op: the entry rule spawns at the arrival.** A client pawn still spawns at participation entry (`networking.md` §Slot lifecycle); its spawn takes the seat's arrival instead of `assign_joiner_placement`'s spawn when one exists and fits. The relevel message is unchanged: the host places every pawn. `networking.md` §Slot lifecycle (entry rule) is rewritten at promotion.
- **Arrival latches; arming keeps latches** (hub D11, latch table). Every pawn spawn latches the pawn against every seam its capsule overlaps, before any trigger tick sees it, so a two-way seam and a player start inside a seam never fire straight away. A latch clears when the capsule stops overlapping. Arming keeps latches, unlike a teleporter's arm. The latch is host-side state, off the wire: seams run only in the host's trigger stage.
- **The arrival event.** The destination seam fires `arrive` once per arriving pawn, host-only, inside the first fixed tick after the pawn spawns, as teleport events dispatch inside their tick. Its params are opaque `activators` and `seam` tokens. U3 adds `waiting` and `ready` to the same source.
- **Life state crosses as a level change.** A pawn inside the seam crosses landmark-relative whatever its life state; the seam changes where it arrives, never its life or health, which follow the level-change rule (E18 O7).
- **`streaming_seam_volume` keeps its name.** It marks an SH warm-up portal inside one map; `level_seam` joins two maps. Docs that name either say which is which.
- **Non-goals.**
  - A seam argument on `loadLevel`. A scripted transition arms a disarmed touch seam, which fires for its standing occupants on the next trigger tick.
  - Prefetch, the blind check, `waiting` / `ready` and their compiler warnings: hub U3.
  - Co-op client view at reveal. A client's camera yaw and pitch at reveal come from its first baseline, which `coop--client-spawn-view-handoff` owns (Open questions).

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

**Carry and consumption**
- [ ] Carried state crosses a seam exactly as it crosses `loadLevel` today.
- [ ] A seat's arrival is consumed by its first spawn. A later respawn, and the level after a non-seam change, place it at player starts (pin P2).
- [ ] A pawn outside the seam at the harvest arrives at a player start, including a pawn that activated the seam and left it before the load drained (pin P1).

**Activation**
- [ ] Touch and use activation each start the transition. Use without overlap does not; a disarmed seam does not. A pawn activates a seam exactly when it would activate a `trigger_volume` of the same activation and shape. A seam activation and a script `loadLevel` on one frame load exactly one level, and only a seam load writes arrivals (pin P3).
- [ ] An unknown target catalog id warns once and leaves the level running.
- [ ] A pawn whose capsule overlaps the seam's union AABB but none of its brushes is outside: it neither activates the seam nor arrives landmark-relative.
- [ ] Arming a disarmed touch seam with a player standing inside starts the transition on the next trigger tick.

**Latch and events**
- [ ] The latch follows the hub D11 table on every row, including an arrival whose capsule overlaps the seam with its body point outside, and a capsule that leaves while the body point is already out (pin P4).
- [ ] The destination seam fires `arrive` once per arriving pawn, on the host only, inside the first fixed tick after the pawn spawns. A brush `on_arrive` and a script `seamEvents` binding both run.
- [ ] The scripting-surface example compiles in TypeScript and Luau and, on the dev pair, disarms the destination seam on arrival.

**Co-op**
- [ ] A client pawn inside the seam arrives landmark-relative at participation entry; one outside arrives at its assigned player start.
- [ ] A host level change before a crossing client spawns clears that client's arrival; it arrives at a player start (pin P5).

**Compiler and surface**
- [ ] A duplicate seam name in one map, an unresolved landmark, and a missing name or target each fail the build with the entity named. Blank optional keys take their defaults.
- [ ] The FGD defaults equal the compiler's, by test.

### Manual
- [ ] On the dev-mod seam pair, a player crosses into the next map and back, turned and moving, and keeps facing and momentum.
- [ ] A co-op session crosses the pair with one player inside the seam and one outside.

## Path
- Shape: a `trigger_teleport`-style entity — section, companion, component, `getMapEntities("seam")` kind, arm and disarm verbs, a seam event source and binding table. Rival: `switch`-style sugar over section 44 plus a side table keyed by trigger index; cheaper to compile, but triggers have no companion and would publish trigger events.
- Seams: `TeleporterParser` and `region_companion` (compile), `TeleportersSection` (layout precedent), `spawn_loaded_teleporters` / `bind_loaded_teleporters` (install), `TeleporterBindingTable` (events), `trigger_system`'s edge and gate code (activation), `physics::movement::teleport::resolve` (factor), `unload_level`, `SeatTable::harvest_bound_pawns`, `spawn_from_player_starts_with_carried_loadout`, `spawn_net_slot_pawn_with_carried_loadout`, the participation-entry handler in `net_poll_and_apply`, `place_camera_at_presented_pose`, `LevelRequest` / `LevelSource`.
- Inside test: capsule against each convex brush by its planes inflated by the radius, about the segment; it over-includes near edges by at most the radius, the safe direction for activation and the conservative one for U3's eyes. Teleporter regions already store convex brush planes.
- Client facing at harvest: `HostCommandQueues` keeps the latest facing yaw and aim pitch per client but exposes no facing accessor; the facing turns through `facing_in_teleport_frame`.
- The movement state payload carries `elapsed_ms`; a dash or slide resumes, not restarts.
- First slice: factor the apply half out of `resolve` with teleport tests green, then a host-only seam on a fixture pair with the harvest moved. That falsifies the riskiest assumptions: the chokepoint splits cleanly, and nothing between the old and new harvest point depended on the old order.
- `startup/lifecycle.rs`, `trigger_system.rs` and `level-compiler/src/parse.rs` are past ~800 lines: split first, behavior-preserving, own commits.
- Docs: `docs/level_design.md` gains a seam section (it has none for triggers either), `docs/scripting-reference.md` the handle, `docs/modding.md` the index; `build_pipeline.md` corrects "each brush" for `trigger_volume` at promotion.
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

## Open questions
- Co-op client view at reveal. Today a client reveals at the host's first-spawn yaw, and arming moves position only, so a client arriving landmark-relative plays from the right place facing the wrong way. `coop--client-spawn-view-handoff` (draft) fixes it with a host-sent spawn pose; the seat's arrival is that pose. Lean: build U2 after it and send the arrival eye, yaw and pitch as the hint, which also needs pitch added to its pose. Otherwise U2 takes only that brief's "arming adopts the pawn's replicated facing" and pitch starts level. — owner — **blocks build**
- Event key spellings beyond the Boundary inventory's — **delegated**.
- Where the seam request rides between the trigger stage and the request queue (in-tick enqueue or a frame-end residual), within pin P3 — **delegated**.
