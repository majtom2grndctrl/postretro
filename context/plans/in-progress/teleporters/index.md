# teleporters

Brief · resumable · reads: `context/lib/movement.md` §4, §6 · `context/lib/entity_model.md` §4, §5, §7 (Environment Volumes) · `context/lib/networking.md` §Current contract, §Host input command queue, §Presentation events vs. replicated state · `context/lib/scripting.md` §12 · `context/lib/audio.md` §4 · `context/lib/build_pipeline.md` §Compiler pipeline, §PRL section IDs · read at 41f21c8ae

## Problem
The owner requested a capability: teleporters for modders. The engine has none,
because nothing at runtime relocates a pawn: no predicted pose chokepoint, no
view-turning path other than mover `carry_yaw`, and no destination vocabulary.
When this is done, a mapper places a teleporter and a destination. A player
crossing in arrives either at the destination facing its way (classic), or
carried relative to a landmark pair with offset, view and momentum kept, so
near-identical room copies read as the world changing around them (seamless,
after Quake's *Dawn of the Machine*). A script turns any teleporter on or off
from gameplay: a button, a closet opening, the closet's monsters dying. Arming
an occupied seamless room shifts everyone in it with no input from them. In
co-op a walk-in is instant for the traveller. A scripted shift reaches a remote
traveller one network delay late, with position, view, motion and sound agreeing.
Observers see a clean cut and hear it. The destination's lighting is already
resident on arrival.

## Decisions
- **One predicted relocation chokepoint.** One movement-substrate path relocates a
  pawn: position, every velocity channel, a yaw delta, movement-state reset, and a
  presentation cut. Teleport is its first consumer.
- **Movement-substrate volume, not a trigger_volume.** The physics movement step
  teleports the pawn, so clients predict it as they predict environment volumes
  and mover carry.
- **This brief is the regional-field amendment** that `entity_model.md` §4
  requires beyond gravity, push and fluid. Teleporter regions are script-armable,
  not static. Promotion updates the "static volumes" wording in `entity_model.md`,
  `movement.md` and `networking.md`, and the id-51 stage's inputs and cache key in
  `build_pipeline.md`.
- **Players only.** Spawners already deliver "monsters teleport in," and NPC
  traversal needs navmesh links. Projectiles never teleport, as in Quake; moving
  one in flight would need a per-entity discontinuity marker nothing else uses,
  so a seamless shift leaves shots behind. A pawn dead at tick start never
  teleports, barred as vault entry is (`movement.md`).
- **Map vocabulary.** Brush class `trigger_teleport`, whose convex brushes may
  be several per entity (union). Point class `teleport_destination` (`name`,
  `angles`). `mode` is `classic` or `seamless`, explicit rather than inferred
  from `landmark`, so TrenchBroom offers the choice and a mistyped landmark
  fails loudly instead of degrading to classic. Seamless names a `landmark`, a
  `teleport_destination` in the source space, so a two-way seamless pair shares
  its points. The compiler rejects an unknown or duplicate name, seamless without
  `landmark`, classic with one, `landmark` equal to `target`, and an unknown
  `mode`. Only the yaw of `angles` counts; a pitch or roll warns at compile.
- **Firing point.** After the pawn's movement step, a teleporter fires when the
  body point environment gravity and push resolve at (`entity_model.md` §7) lies
  inside an armed region. Capsule overlap alone never fires. The tick ends at the
  arrival pose. Where regions overlap, the smallest summed brush volume wins, then
  the earliest map entity. A pawn teleports at most once per tick.
- **One rotation rule.** Both modes rotate the view yaw and every velocity
  channel (base, knockback, push) by one world-up yaw delta, on the teleport tick
  itself. Speeds and pitch are unchanged, and roll is never touched. Classic
  keeps momentum, unlike Quake's fixed exit speed.
  - Classic: the body point lands on the destination origin, and the delta is
    destination yaw minus the facing yaw.
  - Seamless: the delta is target yaw minus landmark yaw, and the body's offset
    from the landmark, vertical included, is rotated onto the target.
- **Arrival state.**
  - A vault aborts. Slide and dash continue with the rotated velocity.
  - Ground becomes airborne, and no mover carry follows the pawn.
  - An arrival capsule blocked by world geometry or a mover at its pose this
    tick refuses the teleport with no event and no sound, and warns once per
    teleporter.
- **Arrival latch.** Arriving latches the pawn against every armed teleporter
  containing its body point. A latched teleporter drops out before the winner
  pick until the body point leaves its region. Arming a disarmed teleporter
  clears every latch against it, so occupants fire. Arming an armed one changes
  nothing.
- **Armed changes take effect on the next tick.** A script `arm` or `disarm`
  applies on the host only, at the start of its next movement tick, before the
  firing test, in listed order. Clients take armed state from replication alone
  and never clear a latch on an armed change they observe; latches reach them in
  the pawn record. Armed state replicates because prediction reads it, which
  diverges from trigger volumes (`networking.md` §Current contract). A joiner
  predicts from `enabled_on_spawn` until a teleporter's armed state arrives, and
  a mispredict resolves as any other.
- **Level content digest.** Teleporter geometry, pairing, mode, and destination
  and landmark poses join it, under "a deterministic input to client prediction
  belongs in the hash." The cue is presentation, and armed state replicates, so
  neither is hashed.
- **Teleports reconcile, view and input included.** Each pawn's replicated
  movement state carries the teleport record in the Boundary inventory. Each
  input command carries the accumulated teleport yaw the client held when it
  sampled the command's facing.
  - Host and client replay apply one rule to every command: facing, and the
    movement intent relative to it, rotate by the pawn's current accumulated yaw
    minus the stamp. A command the host synthesizes from an earlier one keeps
    that command's stamp with its facing. A command sampled before the client's
    first record for its pawn carries zero, since its facing has never been
    turned. Host and client store the accumulated yaw already wrapped, by one
    shared function.
  - A client whose post-replay generation or accumulated yaw differs from its
    prediction takes the teleport correction class, whatever the distance, and
    its camera holds across the cut. View yaw moves by the accumulated-yaw
    difference.
  - The first record a client receives for a new pawn is adopted silently: no
    sound, cut or view turn. Later records, re-baselines included, are not
    first records.
- **Observers cut and hear from the generation.** An observer, the host seat
  included, cuts when a remote pawn's teleport generation changes between
  samples. It never interpolates, extrapolates, or derives displayed speed or
  heading across the change. On the change it plays the last teleporter's
  `arrival_sound` once, at the arrival pose. No cue message is added.
- **Traveller sound is predicted and per-teleporter.** `arrival_sound` is baked
  into the teleporter record. It plays on the traveller's machine from forward
  prediction, never from reconcile replay, as landing and jumping do
  (`audio.md` §4). A cut whose post-replay generation is ahead of the
  prediction plays the authoritative last teleporter's cue once for the teleport
  the client missed, such as an unpredicted scripted shift. A same-generation
  authoritative replacement retains the speculative cue without another playback,
  even if the teleporter or its cue differs. Several unseen teleports coalesce to
  the last cue. It defaults to none, which keeps seamless silent.
- **Host-fired gameplay event.** The authoritative teleport fires the teleporter's
  `teleport` event once per pawn per teleport, never from prediction. It
  publishes `activators` and a `teleporter` token (`arm`/`disarm`), and no
  `occupancy`. A mapper may name a reaction in `on_teleport`, as `on_fire` works
  on triggers.
- **Destination residency rides id 51.** A teleporter is a zero-length route
  from its source region to its arrival cells: the destination cell for classic,
  the cells the source region maps onto for seamless. Every camera cell within
  the bake's maximum lead of the region carries the arrival cells and their
  visible sets at its distance to the region, lead 0 where the region touches.
  The bake ignores armed state, so a set piece is warm before it arms.
- **Non-goals.**
  - Telefrag: pawns do not collide with each other, so arrivals overlap
    harmlessly, and a kill on arrival is a gameplay policy no one has asked for.
  - Warm SH on maps without id 51. They have no portal data to route through;
    their lightmaps stay all-resident and only SH arrives cold.
  - A predicted arrival screen flash. Quake teleports have none. A mod can bind
    `flashScreen` to the event, and a later key can add one without breaking
    maps.
  - Arrival effects observers see beyond the sound, such as particles. A later
    key adds them; today a mod reaches observers only through the event's
    host-latency presentation reactions.
  - Rendering through a teleporter: seamless works by room copies, so nothing
    needs to see through.
  - Spawner kills counting toward `progress`, unowned with no draft yet.
    Closets built from map-placed monsters re-arm through `progress` today.
  - Teleporters in the id-46 cell-coupling relation. No consumer reads it for
    network relevance or AI perception yet, and the first one adds teleporter
    couplings.

### Scripting surface
Map keys a mapper writes:
```
{ "classname" "trigger_teleport" "_tags" "vault_gate" "mode" "classic" "target" "vault_in"
  "enabled_on_spawn" "0" "arrival_sound" "sfx/teleport" }
{ "classname" "teleport_destination" "name" "vault_in" "angles" "0 90 0" }
{ "classname" "trigger_teleport" "_tags" "era_shift" "mode" "seamless" "target" "era_b" "landmark" "era_a"
  "enabled_on_spawn" "0" }
{ "classname" "teleport_destination" "name" "era_a" "angles" "0 0 0" }
{ "classname" "teleport_destination" "name" "era_b" "angles" "0 90 0" }
```
Brush faces are omitted. `era_a` sits in the source room and `era_b` at the same
spot in its copy.

A level data script:
```ts
// Proposed design.
import { defineReaction, getMapEntities } from "postretro";
import type { TeleportEventParams } from "postretro";

export function setupLevel() {
  const gate = getMapEntities("teleporter", { tag: "vault_gate" });   // members, map order
  const shift = getMapEntities("teleporter", { tag: "era_shift" });
  const button = getMapEntities("trigger", { tag: "vault_button" });
  const plate = getMapEntities("trigger", { tag: "closet_plate" });

  const open = defineReaction("gate.open", { sequence: gate.flatMap((t) => t.arm()) });
  const shut = defineReaction("gate.shut", { sequence: gate.flatMap((t) => t.disarm()) });
  const cleared = defineReaction("closet.cleared", { progress: { tag: "closet", at: 1.0, fire: "gate.open" } });
  const timeShift = defineReaction("era.shift", { sequence: shift.flatMap((t) => t.arm()) });
  const oneWay = defineReaction("gate.used", (on: TeleportEventParams) => on.teleporter.disarm());

  return {
    reactions: [open, shut, cleared, timeShift, oneWay],
    triggerEvents: [...button.flatMap((t) => t.on("enter", [open])), ...plate.flatMap((t) => t.on("enter", [shut]))],
    teleportEvents: gate.flatMap((t) => t.on("teleport", [oneWay])),
  };
}
```

## Acceptance
Pin ids refer to `research.md` §Ordering pins.

### Automated
Placement:
- [ ] Classic: the body point lands on the destination origin. The view yaw equals the destination yaw and pitch is unchanged. Every velocity channel is rotated by the delta with its speed unchanged.
- [ ] Seamless at landmark deltas 0°, 90°, 180° and −90°: offset (vertical included), every velocity channel, and yaw are rotated onto the target, with speeds unchanged.
- [ ] A two-way seamless pair sharing its two points returns a pawn to its starting offset, yaw and velocity, one teleport per crossing, after it steps out of the arrival region and back in (P1).
- [ ] A pawn that teleports off a turning platform gains none of the platform's rotation after arrival, in its view or its motion (P2).

Firing:
- [ ] A body point inside after the movement step fires. A capsule overlapping the region with its body point outside does not. A teleporter built from several brushes fires anywhere in their union.
- [ ] A disarmed teleporter never fires. `enabled_on_spawn` 0 starts disarmed.
- [ ] `arm()` while a body point is inside fires on the next tick. `disarm()` issued on the tick a body point crosses in does not stop that tick's teleport, and stops any on the next (P23).
- [ ] On overlapping regions, exactly one teleport fires per pawn per tick: the region with the smaller summed brush volume, then the earlier map entity.
- [ ] Arrival inside an armed teleporter latches. There is no fire until exit, and exit and re-entry fires. A pawn arriving in a teleporter armed that same tick is latched against it (P23).
- [ ] Arriving where two armed teleporters overlap latches both, and neither fires until the body point leaves it; a third, unlatched one still wins the pick (P28).
- [ ] Arming both directions of a two-way seamless pair in one call while a remote client stands latched in one: the host fires once, and the client shifts once through prediction or adoption, never ping-ponging, cutting back, or playing a stray sound (P29).
- [ ] Disarming then arming a latched teleporter, in one tick or across ticks, clears its latches and fires it for occupants on the next tick. `arm()` on an armed one does not. Arm then disarm of a disarmed one in one tick leaves it disarmed and fires nothing (P3).
- [ ] Two players entering one classic teleporter on the same tick both arrive, and the event fires twice. Arming an occupied seamless teleporter with two players inside shifts both on the next tick, and the event fires once for each (P4).
- [ ] An arrival blocked by world geometry or a mover is refused: no move, no event, no sound, one warning. A mover moving into the arrival capsule on the teleport tick refuses it, and one moving out of it does not, on host and client alike (P5).
- [ ] A vault in progress aborts. A slide or dash survives with the rotated velocity. The pawn is airborne after arrival.
- [ ] An enemy or projectile inside an armed teleporter is not moved, and no event fires. A player dead at tick start, including one zeroed by a deferred health write that lands before movement, does not teleport. A player killed later in the tick it crosses in still teleports and fires the event (P25).
- [ ] On the teleport tick, a trigger volume at the arrival point fires its enter edge and one the pawn left at the source fires its exit edge (P24).

Event and script:
- [ ] The scripting-surface map and script run as a `content/dev` level, with a Luau twin that installs the same level manifest and produces byte-identical teleporter wire. The gate is disarmed after the plate, armed after the button, and armed again when the closet's monsters die. Using it disarms it. Arming `era_shift` shifts a player standing in it.
- [ ] One-way gate: a second player entering one tick after the first never teleports on the host, whether it runs one or several catch-up ticks that frame (P6).
- [ ] An arm or disarm from a frame-end source (a progress or crossing reaction) takes effect on the first movement tick after that frame's drain (P7).
- [ ] The `teleport` event and an `on_teleport` reaction fire once per authoritative teleport and never from client prediction alone. The event's activators resolve to the teleported player, and it publishes no occupancy.

Sound:
- [ ] A predicting client starts `arrival_sound` on its teleport tick and does not play it again when reconcile replays that tick.
- [ ] Every non-traveller machine, the host seat included, hears `arrival_sound` once per observed generation change, at the arrival point, even on frames where it still renders the traveller at the source. Several teleports between one observer's samples play one sound, the last teleporter's (P8).
- [ ] When the host refuses a teleport the client predicted (a disarm it had not seen, a blocked arrival, a pawn already dead), the client's sound has played once, the cut back plays none, and no observer hears it (P9).
- [ ] An adoption whose post-replay generation is ahead of the prediction plays the authoritative last teleporter's cue once. A one-tick skew of the same crossing never sounds twice. A same-generation authoritative replacement retains the speculative cue without another playback, even if the teleporter or cue differs. Several unseen teleports coalesce to the last cue (P10, P32).
- [ ] A teleporter with no `arrival_sound` plays nothing on any machine.

Compiler and format:
- [ ] Each malformed reference or value named in Decisions fails the compile with a message naming the entity. A `teleport_destination` with pitch or roll in `angles` compiles with a warning and uses only its yaw.
- [ ] Every camera cell within maximum lead of the source region holds the arrival cells and their visible sets at its distance to the region, lead 0 where the region touches, for classic and seamless alike. Moving a destination or landmark, reshaping the brush, or changing the mode in a warm build re-bakes id 51 (P11).
- [ ] Arrival cells hold the smallest lead any origin gives them, never a larger one set by a farther origin reached first. A teleporter with `enabled_on_spawn` 0 adds the same id-51 entries as an armed one (P12).
- [ ] The level content digest changes with teleporter geometry, pairing, mode, or destination and landmark poses, and does not change with `arrival_sound` or `enabled_on_spawn`.
- [ ] A malformed teleporter section fails the level load. An absent one means no teleporters.

Presentation and network:
- [ ] On the teleport tick, the rendered view yaw is already rotated at every interpolation alpha, and a shot fired on that tick leaves the arrival eye along the rotated view, on the host seat and a predicting client (P13).
- [ ] On the teleport tick, at any interpolation alpha, neither the camera nor the pawn mesh renders between source and destination. This holds when the teleport is the first, a middle or the last of several catch-up ticks in one frame, and on zero-tick frames after it (P14).
- [ ] When a correction takes the teleport class, the camera never renders between the predicted and corrected pose, at any interpolation alpha on that frame or the tick after it (P15).
- [ ] A correction still settling when a predicted seamless teleport fires never pulls the eye off the arrival pose in the source room's frame (P16).
- [ ] At 100 ms round trip on a clean link, the predicting client and the host teleport on the same client tick, no correction exceeds the ordinary band, and the client's yaw matches the host. Under the harness's lossy profile, corrections stay within the ordinary band and yaw still matches.
- [ ] At 100 ms round trip, a script arming an occupied seamless teleporter at a 90° landmark delta reaches a remote client as one teleport-class cut. Its yaw rotates, its sound plays once, its in-flight inputs move it along its new facing on host and client both, and the next snapshot needs no further correction (P17).
- [ ] When the host refuses a predicted entry, the client cuts back with its yaw restored. This cuts at a 1 m teleport distance too (P18).
- [ ] Two predicted teleports inside one round trip, the second refused, produce exactly one cut, with yaw ending at the host's. Both agreed: no cut. Two unpredicted teleports in one snapshot leave the client's yaw at the host's. Client and host teleporting on the same tick through different teleporters cut (P19).
- [ ] Under the lossy profile, with the client's input lost across its teleport tick, the host's held, neutral, and late real commands after the teleport face and move in the post-teleport frame, and the remote avatar never turns back to its old facing (P20).
- [ ] Client predicts a classic entry one tick before the host takes it; or the host refuses a prediction; or the two take different teleporters on one tick. In each, in-flight commands run in the pawn's actual frame on host and replay, the avatar never shows the predicted facing, and no correction follows the cut (P27).
- [ ] A command sampled on a later tick of a multi-tick frame, or sent on a zero-tick frame, faces and moves on host and replay exactly as in forward prediction (P31).
- [ ] A classic teleport the host takes on a held command turns the client's view, on adoption, by the turn that command missed. A seamless one takes no extra turn (P35).
- [ ] An observer and the host seat never render a remote pawn between source and destination, including across two teleports between consecutive samples and a generation wrap. A starved observer holds the arrival pose. Displayed speed and heading never come from a pair straddling the cut, and an observer's first sample of a pawn neither cuts nor plays sound (P21).
- [ ] A joiner whose teleporter was re-armed by script before it joined predicts from `enabled_on_spawn` until the armed state arrives, and converges with one cut once it does. A joiner whose pawn teleported before its first record adopts that record with no sound, cut, or view turn. Its commands sampled before that record carry a zero stamp, and those after carry the record's accumulated yaw (P26, P30, P33).
- [ ] A teleport arriving in a periodic re-baseline of the client's own pawn plays its sound and turns its view; only the first record for a new pawn is silent (P34).
- [ ] An input command stamped before a level change or a restart of the same level is never rotated by a teleport in the new session (P22).
- [ ] An `arm()` or `disarm()` member step run on a client changes nothing there; the client's armed state follows replication only.

### Manual
- [ ] Visual on Mac/Metal: a seamless swap between distant room copies shows no pop and no lighting change.
- [ ] Co-op playtest at about 100 ms: walk-ins are instant for the traveller, its sound lands on the jump, and the observer sees a clean cut. A scripted shift lands for the remote traveller with view, motion and sound agreeing.
- [ ] Playtest: the closet loop (disarm on reveal, re-arm on clear) plays end to end.
- [ ] Runtime diagnostics: lightmap pool peak and SH working set beside a teleporter, against the same map with teleporters removed. Measure and report.

## Path
- Versioning: the pawn teleport record joins the replicated player movement state beside the swim re-entry latch, which bumps `SNAPSHOT_VERSION` (20 at read) and `WIRE_VERSION`. The input stamp joins the input command, which bumps `WIRE_VERSION` only (`networking.md` §Version gates). One bump of each covers the whole brief.
- First slice: a host-seat classic teleport through the substrate, ending the tick at the arrival pose with camera hold, presentation write, and a same-tick yaw delta. Mover carry applies yaw at the next input seam (`apply_mover_yaw_carry`), one tick late for a teleport. The delta must also reach the pawn rotation write and the aims captured before the tick (`research.md`). Then seamless, netcode, sound, then bake.
- Seams:
  - Substrate: `physics/src/movement/mod.rs`, `mover_carry.rs`, `environment.rs`, and `MovementEvents` for the predicted sound edge.
  - Region index: `collision` `ConvexLeaf` and `EnvironmentVolumes`.
  - Parse shape: `level-compiler` `trigger_volumes.rs` and `environment_volumes`.
  - Residency: `cell_residency_bake/lead_map.rs` `build_lead_map`, with region distance as the minimum of `Neighbors::within_distances` over touched cells, merged nearest-first. Hash the teleporter section into the stage cache key and bump its stage version. The section format does not change.
  - Reconcile: the `CorrectionClass::Teleport` branch, which holds no camera today and clears the unacked ring after replay. A generation cut keeps the ring, since stamped rotation keeps replay valid.
  - Input stamp: host `command_queue.rs` (real, held and neutral commands) and client replay in `prediction.rs`, read at the same point as the facing.
  - Armed mutation: member steps apply on every machine (`scripting.md` §12), so teleporter `arm`/`disarm` needs a host-only gate the trigger path lacks.
  - Observer: `netcode/src/interpolation.rs` and host-side remote-pawn presentation.
  - Handle and event plumbing: trigger `on` binding in `sdk/lib/map_entities.ts`.
  - Armed-state binding: only movers bind a replicated record to the client's locally installed entity today (`client.rs`, `bindings.rs`); teleporters need the same.
  - First record: key silent adoption on the prediction identity change, not on every own-pawn record.
- Rival: extending `trigger_volume` with a teleport command. It is cheaper, but host-only and AABB-only, and it cannot turn a client's view (`research.md`).
- Files past ~800 lines (`main.rs`, `parse.rs`, `frame_loop/mod.rs`, `interpolation.rs`) take call-site edits only. Split first if a change grows past that.

## Open questions
- After the bake slice, how much mandatory lighting do arrival origins add — **owner, after measurement**. Report per-camera-cell mandatory lightmap blocks and SH clusters, and the runtime pool peak, with and without teleporters on the `content/dev` teleporter level and on a stress case with eight teleporters inside one lead radius. Mandatory entries never evict, so this is the 2 GB VRAM exposure. The owner decides whether arrival origins beyond the touching cells move to the evictable band. The all-resident pool gate is not in play: it checks only levels without id 51, which get no arrival origins.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Teleporter brush | new teleporter component | `COMPONENT_KIND_TELEPORTER` = next free id (24 at read) | `"teleporter"` member kind | `"teleporter"` | `trigger_teleport` |
| Destination point | compile-time only | n/a | n/a | n/a | `teleport_destination` |
| Mode | enum `Classic` / `Seamless` | u8 0 / 1 | n/a | n/a | `mode` = `classic` / `seamless` |
| Target, landmark | resolved to records at compile | n/a | n/a | n/a | `target`, `landmark` (destination `name`) |
| Armed state | `armed` | bool | n/a | n/a | `enabled_on_spawn` (0/1, default 1) |
| Arrival sound | baked sound key | none; observers derive it from the pawn record | n/a | n/a | `arrival_sound` (key under `sounds/`, no extension) |
| Event | teleport edge | `"teleport"` | `t.on("teleport", …)`, `TeleportEventParams`, manifest `teleportEvents` | same | `on_teleport` |
| Token | teleporter target | `"@teleporter"` | `on.teleporter` (`arm`, `disarm`) | same | n/a |
| Pawn teleport record | generation, accumulated yaw, last teleporter, arrival latch, in the replicated player movement state | u8 wrapping; f32 radians wrapped to (−π, π]; u16 index, `0xFFFF` none; latch as a u16 count then that many u16 teleporter indices in ascending order, uncapped | n/a | n/a | n/a |
| Input stamp | accumulated teleport yaw at sampling | f32 radians, same representation as the record | n/a | n/a | n/a |
| Protocol | `WIRE_VERSION` bump (31 at read); `SNAPSHOT_VERSION` bump (20 at read) | handshake | n/a | n/a | n/a |

## Wire format
New PRL section `Teleporters`, id 55 (54 is reserved for ReflectionProbes). It
is present when the map has at least one `trigger_teleport`. It mirrors id 52
`EnvironmentVolumes` for endianness, internal version header, record count
placement, convex-leaf plane encoding, and length-prefixed strings. Internal
version 1. Per record, in map entity order:
- source entity
- mode u8
- enabled_on_spawn u8
- the region's convex leaves, as in id 52
- destination origin and yaw (f32 radians)
- landmark origin and yaw. Classic writes zeros, and the loader ignores them.
- `arrival_sound` string, where empty means none
- `on_teleport` string, where empty means none

A malformed section is a load error. Absent means no teleporters.
