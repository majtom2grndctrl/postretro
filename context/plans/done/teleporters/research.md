# teleporters — research

Read at 41f21c8ae. Findings that informed the brief. Not binding.

## Reference: Dawn of the Machine (Quake, 2026)

MachineGames' 30th-anniversary episode. Its "time travel" is a seamless
teleport: a technique other Quake mods used, prototyped by Andrew Yoder and
pushed hardest in Tom Fritzson's "The Haunted Tower." The player crosses between
near-identical copies of a space with relative position, view and momentum
kept, so the level appears to change around them. Ars Technica: "a
disorienting-but-cool teleportation system that makes it look like the levels
are changing on the fly," with shifts happening "without player input."

Engine analogues: Source `trigger_teleport` with a local destination landmark
(entity offset from the landmark is preserved), Momentum's reorient option
(origin, angles and velocity mapped from landmark-local to destination-local
space). Quake's own `trigger_teleport` sets origin and angle to
`info_teleport_destination` and fires a fixed forward velocity.

"Without player input" is why `arm()` on an occupied teleporter fires: a script
arming a room-sized seamless volume is the time shift.

Sources: slayersclub.bethesda.net/news/quake-dawn-of-the-machine-behind-the-scenes ·
arstechnica.com/gaming/2026/08/new-official-30th-anniversary-quake-mission-pack-adds-new-maps-and-mechanics/ ·
developer.valvesoftware.com/wiki/Trigger_teleport · docs.momentum-mod.org/entity/trigger_teleport/

## Why not a trigger_volume

`trigger_volume` is AABB-only (`resolve_brush_entity_aabb`), host-only
(`TriggerSystem::run_authoritative_tick_with_dispatch`), and its armed state never
replicates (`networking.md`: trigger volumes are shared baked data). A teleporter
evaluated there reaches a connected client only as a reconcile correction one
round trip late. Below `TELEPORT_CORRECTION_MIN_M` that correction smooths as a
glide, and the client's yaw never turns: `InputCommand` carries absolute
`facing_yaw`, so the host cannot rotate a client's view.

Environment volumes are the matching precedent: convex `ConvexLeaf` plane sets
(`postretro-collision` `EnvironmentVolumes`, `resolve_environment`), resolved by
the movement substrate every tick on host and client alike, part of the level
content digest. Mover `carry_yaw` turns only the owning client's view, but it is
app-side, not substrate. The physics tick takes `facing_yaw` as input.
`apply_mover_yaw_carry` writes the camera at the next tick's input seam, and
replay never re-applies it. A teleport needs its yaw on the teleport tick
itself, so it cannot copy that seam unchanged.

## Pose and interpolation seams

- Pawn state: `Transform` + `PlayerMovementComponent` (`velocity`,
  `knockback_velocity`, `push_velocity`, `ground`, `movement_state`).
- No-lerp entity write: registry `set_presentation_transform` (reconcile's
  teleport branch uses it).
- Camera: `frame_timing.hold_state` sets both endpoints. `frame_loop` calls
  `push_state(camera.position)` every tick on both host and client paths. A
  teleport tick must hold instead.
- Observer interpolation (`netcode/src/interpolation.rs`) has no discontinuity
  check. A remote pawn lerps across the map.
- `ServerPresentationPayload` carries PlaySound, Rumble, FlashScreen, Vignette,
  ScreenShake. Host-fired teleport reactions reach a remote activator at host
  latency.
- Reconcile's `CorrectionClass::Teleport` branch stamps the pawn and clears the
  presentation offset. It also clears the unacked command ring after replay, and
  the next snapshot's ack then lands in the cleared span and corrects again. A
  generation cut keeps the ring instead, since stamped rotation keeps replay
  valid. It holds no camera endpoints today, which is why the camera-hold row
  exists.
- Observer extrapolation, displayed speed and heading come from the two adjacent
  samples (`interpolation.rs`). The host seat presents remote pawns through its
  own delay buffer. Both need the generation cut.
- No player-pawn collision or separation exists. Only AI agents separate.
- Held and neutral input commands reuse the last real facing (`command_queue.rs`).

## id-51 residency injection

- Builder: `cell_residency_set_bake_cached` → `bake` →
  `build_lead_map` (`cell_residency_bake/lead_map.rs`). Per camera cell, origins
  are the camera cell at lead 0 plus `Neighbors::within_distances` cells. Each
  origin contributes itself plus its sampled visible set, and the first origin
  to reach a cell sets that cell's lead.
- Injection point: arrival cells become extra origins in `build_lead_map`. A
  teleporter is a zero-length route, so every camera cell within max lead of the
  source region gets the arrival cells at its own distance to the region. Lead-0
  origins only at cells the region touches would warm the destination after the
  camera already stands in one, and streaming reads are async. `within_distances`
  gives one cell's pairwise distances with no region form, so distance to a
  region is the minimum over the cells it touches. `build_lead_map` keeps the
  first origin to reach a cell, so arrival origins must merge nearest-first for
  the smallest lead to win. No format change, since the section stores only
  `(cell_id, lead)` sorted per camera cell.
- Runtime reads the set verbatim (lightmap `demand.rs` `entries_for`, SH
  `reach.rs`). Mandatory entries (lead ≤ L) never evict, so the lightmap pool
  grows past its cap to hold them. A band lead would be evictable and would not
  guarantee warmth.
- Cache: hash resolved source and arrival cells into
  `cell_residency_set_cache_key` and bump `CELL_RESIDENCY_SET_STAGE_VERSION`.
  Leave `PORTAL_WALK_EPOCH` unchanged. `build_pipeline.md` says the stage "reads
  no hint." That still holds. A teleporter is movement topology, a zero-length
  route, and id 51 is keyed on movement lead. Authored pins stay runtime-composed
  from id 49.
- Rival weighed in validate-plan: section 55 carries source→arrival cell
  couplings and the cell-demand stage merges them at runtime. That would gate on
  armed state and cover maps without id 51. It was rejected because set pieces
  want the destination warm before arming, and maps without id 51 have no usable
  portals.
- Seamless arrival cells come from the source region's image under the landmark
  transform. A room-sized volume lands the pawn in whichever copy cell its offset
  maps to, not only the target point's cell.
- Risk: the compiler's all-resident floor and pool-gate checks
  (`pipeline/lightmap_pool_gate.rs`) may trip on maps with many teleporters.

## Traveller cue lane

`MovementEvents` (`physics/src/movement/mod.rs`) carries `landed`, `jumped`,
state edges and `swim_fluid`. `audio.md` §4: those edges come from the client's
forward prediction, never from reconcile replay. Movement sounds resolve from
the local descriptor. A descriptor-wide teleport cue would also fire on seamless
teleports, which breaks the roof, so cues are per-teleporter KVPs baked into the
section the client already holds. Observers derive the sound from the pawn's
replicated teleport generation, as reload edges derive from replicated slots
(`audio.md` §4). A host observer cue was weighed and dropped: `WeaponCue` is weapon-typed with a required shot id, its
entity anchor resolves to the observer's delayed pose at the source, and mover
map-entity sounds stay host-only, so "a map entity's sound" was no warrant.

## Commitments touched

- `networking.md` §Current contract: trigger armed state is host-only.
  Teleporter armed state diverges, because client prediction reads it.
- `networking.md` digest rule: "a deterministic input to client prediction
  belongs in the hash."
- `movement.md` §7 non-goal: no per-tick script-authored movement. The
  teleporter is declarative map data, and script only arms and disarms it.
- `scripting.md` §12 `progress`: counts only entities placed in the map. Closets
  released by a spawner never re-arm through it. Out of scope here.

## Oversized files on the path

`postretro/src/main.rs`, `level-compiler/src/parse.rs`, `frame_loop/mod.rs` and
`netcode/src/interpolation.rs` are past ~800 lines. New code lands in new
modules, such as a compiler `teleporters.rs` beside `trigger_volumes.rs` and a
physics `movement/teleport.rs` beside `mover_carry.rs`. Those files take only
call-site edits.

## Choices made in review

- **Input stamp, not scheduled arming.** Round 1 replaced a per-input stamp with
  scheduled arming, which applied a script change on a host tick chosen to lead
  every client's prediction. Round 2 found that it does not hold in this netcode:
  - Client and host ticks are separate numbers, and the host cursor freezes and
    trims, so a host tick has no stable client-tick mapping.
  - The host measures only uplink buffer depth, not each client's lead.
  - Pending changes could reorder when the lead shrank.

  The stamp is keyed to what the client knew when it sampled each command, so
  freezes and trims do not disturb it. Round 3 changed the stamp from a
  generation number to the accumulated teleport yaw itself. A generation cannot
  name a frame once prediction is ahead or diverged: a refused teleport and a
  later host teleport can share one number, and an ahead stamp is not "older".
  A yaw stamp rotates by current minus stamp in every direction and needs no
  per-generation history. Held and neutral commands reuse an earlier facing, so
  they keep that command's stamp. Paired with replay applying the same
  rotation, and with the unacked ring kept across a generation cut, an
  unpredicted scripted shift reaches a remote traveller as one cut. Position,
  view, motion and sound agree, so it reads as instant to that player.
- **Accumulated teleport yaw in the pawn record.** A bare last-teleporter index
  cannot recover a classic delta the client did not predict (it depends on
  facing), two deltas in one snapshot, or two different teleporters on one tick.
  The client corrects view yaw by the difference in accumulated yaw.
- **Relocate after the movement step.** The tick ends at the arrival pose, so a
  classic destination lands exactly where authored. The same-tick yaw must reach
  every reader. The host camera and local fire read after movement through the
  post-movement closure. The host seat's pose aim (captured before the tick),
  remote aims, and the pawn rotation write do not, so the delta is published to
  the registry for them.
- **Pawn-only teleport record**, not a generic per-entity discontinuity marker.
  Its named adopters do not need it: `NetworkId` is never recycled, so slot reuse
  is a new network entity, and the E17 rider snap is client-local and adds no
  snapshot fields. `coop--client-spawn-view-handoff` writes only an unarmed
  stand-in camera, so it is not a second consumer of the relocation chokepoint.
- **Digest** keeps the cue and `enabled_on_spawn` out. Presentation and
  host-sent state stay out of the hash (`networking.md`), and a sound-key
  mismatch would hold a slot below participating.
- **Dead pawns** never teleport. A corpse would fire `on_teleport` and trip a
  one-way gate. Liveness is read before movement and deaths sweep after weapons,
  so a pawn killed on its crossing tick still teleports.
- **No arrival flash.** Quake teleports have no screen flash, and sound alone
  carries the cue. Dropping it removes a KVP, wire bytes, and a limiter path.
- **Explicit `mode`.** Inferring it from `landmark` presence (Source's shape)
  would let a mistyped landmark degrade silently to classic.

## Ordering pins

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Two-way seamless pair sharing two points: cross, step out of the arrival region, re-enter | Arrival latches; exit clears the latch | Pawn returns to its starting offset, yaw and velocity, one teleport per crossing |
| P2 | Pawn standing on a carry-yaw turntable teleports | Carry ground captured at the input seam before movement; arrival sets ground airborne | No turntable yaw after arrival, at tick rate or as a render residual |
| P3 | One tick's commands hold [disarm, arm] on an armed teleporter, or [arm, disarm] on a disarmed one | Applied in listed order at the next tick start | First: latches cleared, occupants fire. Second: stays disarmed, no fire |
| P4 | Arm an occupied seamless teleporter with two pawns inside | Arm applies at tick start; both body points tested after movement | Both shift; the event fires once per pawn |
| P5 | Mover moves into, or out of, the arrival capsule on the teleport tick | Mover stage precedes player movement on host; client predicts movers before movement | Into: refused. Out of: accepted. Same on host and client |
| P6 | One-way gate; second pawn enters one tick after the first; host running 1 or 3 catch-up ticks | `on_teleport` disarm applies at the next tick start | Second pawn never teleports on the host |
| P7 | Arm or disarm from a frame-end source (progress, crossing) | Applied at the frame-end drain, after the tick loop | Takes effect on the first movement tick after that drain |
| P8 | Observer or host seat sees a remote teleport while its delayed render still shows the source; several teleports between samples; first sample of a pawn | Sound derived on the generation change, anchored at the arrival pose | One sound per observed change, last teleporter's, at the arrival point; none on first sample |
| P9 | Client predicts a teleport the host refuses (unseen disarm, blocked arrival, pawn already dead) | Sound from forward prediction; cut back through reconcile | Sound played once; the cut back plays none; no observer hears it, since the generation never advanced |
| P10 | Teleport the client did not predict, such as a scripted shift | Teleport first appears through reconcile | Sound plays once on adoption, never again on later replays |
| P11 | Camera cell at portal distance d ≤ max lead from the source region | Arrival cells merged as origins at lead d | Arrival cells and their visible sets present at lead d; lead 0 in touching cells |
| P12 | Two origins reach one arrival cell at different distances; or the teleporter is disarmed | First origin to reach a cell sets its lead; merge nearest-first; bake reads geometry, not armed state | The smaller lead wins; disarmed adds the same entries |
| P13 | Teleport tick on the host seat and a predicting client | Yaw delta applied in-tick and published before the post-movement aim and camera endpoints | Rendered yaw rotated at every alpha; a shot on that tick leaves the arrival eye along the rotated view |
| P14 | Teleport on the first, a middle or the last of N catch-up ticks, then a zero-tick frame | Snapshot at tick start; camera push once per tick; the hold replaces the push on the teleport tick | Neither camera nor pawn mesh renders between source and destination |
| P15 | Snapshot apply lands a teleport-class correction, then 0 or more ticks that frame | Reconcile runs before the tick loop; the next push would make the pre-correction eye `previous` | Camera never renders between the predicted and corrected eye |
| P16 | Correction offset still decaying when a predicted seamless teleport fires | Offset is a world-space vector added to the eye at tick rate | Offset cleared or rotated with the delta; the eye never drifts in the source frame |
| P17 | Script arms an occupied seamless teleporter at 100 ms round trip, 90° landmark delta, client holding forward | Arm applies on the host's next tick; the client adopts through reconcile, keeping its unacked ring; host and replay rotate stamped commands alike | One cut; yaw rotated; sound once; in-flight inputs move along the new facing on both; no second correction |
| P18 | Host refuses a predicted entry; teleport distance 1 m | Generation mismatch after replay | Teleport-class cut back; yaw restored |
| P19 | Two predicted teleports in one round trip, second refused; two unpredicted teleports in one snapshot; client and host through different teleporters on one tick | Compare post-replay generation and accumulated yaw with the prediction | One cut where they differ, none where they agree; yaw ends at the host's |
| P20 | Remote client's input lost across its teleport tick; host holds, synthesizes neutral, then late real commands arrive | Held and neutral commands reuse the last real facing and keep its stamp; real ones carry their sampled stamp | All rotate by current minus stamp, face and move in the post-teleport frame; the avatar never turns back |
| P21 | Observer receives two teleports between snapshots, or a generation wrap; starves just after one; first sample of a pawn | One snapshot per frame after the tick loop; extrapolation from adjacent samples | Cuts once, never lerps or extrapolates across; speed and heading never from the straddling pair; first sample neither cuts nor sounds |
| P22 | Command stamped before a level change, or before a restart of the same level where teleporter indices match | Commands from an old participation epoch are dropped; the first commands of the new session carry a zero stamp | Never rotated by a teleport in the new session |
| P23 | `arm()` or `disarm()` issued during tick k; pawn crosses in on tick k; pawn arrives in a teleporter armed at the start of its tick | Changes apply at the next tick start, before the firing test | Tick k's teleport happens regardless; the change governs k+1; the arriving pawn is latched |
| P24 | Trigger volumes at the arrival point and at the source on the teleport tick | Trigger tick runs after player movement | Arrival trigger enters and source trigger exits on the teleport tick |
| P25 | Player killed on the tick its body point crosses in | A deferred health write lands before movement; liveness is read before movement; the death sweep runs after weapons | A deferred kill bars the teleport; a kill later in the tick still teleports and fires the event |
| P26 | Player joins a level whose teleporter was re-armed or disarmed by script | Join baselines send the joiner's pawn first; other entities follow under the snapshot budget | Joiner predicts from `enabled_on_spawn` until armed state arrives; a mispredict resolves as one cut |
| P27 | Client predicts a classic entry one tick before the host; or the host refuses a prediction; or host and client take different teleporters on one tick | Stamps are behind, ahead of, or diverged from the pawn's accumulated yaw | Every command rotates by current minus stamp on host and replay; the avatar never shows the predicted facing; no correction follows the cut |
| P28 | Arrival point lies inside two armed teleporters, and a third armed one also contains it after a step | Arrival latches every armed teleporter containing the body point; latched ones drop out before the winner pick | Neither latched one fires until exit; the unlatched one can win |
| P29 | One call arms both directions of a two-way seamless pair while a remote client stands latched in one | Arm applies on the host only; clients never clear latches from an observed armed change | Host fires once; the client shifts once by prediction or adoption, with no ping-pong, cut back, or stray sound |
| P30 | Joiner's pawn teleported before the joiner's first record | First record for the client's own pawn | Adopted silently: no sound, cut, or view turn; commands not rotated by that teleport |
| P31 | Command sampled on a later tick of a multi-tick frame, or sent on a zero-tick frame | Stamp read at the same point as the facing | Faces and moves on host and replay exactly as in forward prediction |
| P32 | One-tick skew between client and host on a classic crossing; different teleporters on one tick; a teleport appearing only in replay | Sound plays from forward prediction, and once more on adoption whose post-replay generation is ahead; same-generation replacements retain the speculative cue | Skewed crossings never sound twice; replacements add no cue even when their sound differs; ahead adoptions play the authoritative last cue once and several unseen teleports coalesce |
| P33 | Commands sent before the client's first record for its pawn, on join, level change or restart | No accumulated yaw known; facing never turned | Stamp zero; rotated by the full accumulated yaw; a stamp cached from the previous level never applies |
| P34 | A teleport arrives in a periodic re-baseline of the client's own pawn | Every own-pawn record reports the armed local pawn; only the prediction identity change marks a first record | Sound and view turn as any adoption; silence only for a new pawn's first record |
| P35 | Host takes a classic teleport on a held command | Held command reuses a facing sampled before the turn the client made after it | Client view turns on adoption by the turn the held command missed; seamless takes no extra turn |

## Implementation notes from review

- The accumulated yaw must be stored already wrapped, by one shared function on
  host and client. Wrapping only at encode lets a one-ulp difference read as a
  different yaw and cut spuriously. Facing and camera yaw are unbounded, so
  rotating by any difference is otherwise harmless.
- `armed_local_pawn` is set on every own-pawn record, baseline or delta. Only the
  prediction identity change marks the first record.
- Only movers bind a replicated record to the client's locally installed entity.
  Any other record spawns a fresh entity, so teleporter armed state needs the
  mover-style binding.
- Host rotation of a command's facing must come before both its movement input
  and the aim capture.
- Neutral synthesis needs a stamp stored beside `latest_facing_yaw`; held
  commands already carry theirs in the last resolved command.
