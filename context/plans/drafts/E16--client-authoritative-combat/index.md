# E16 - Client-Authoritative Combat

> **Status:** draft.
>
> **Epic:** 16 - Combat.
>
> **Milestone:** Resolution Modes.
>
> **Prerequisite for `E16 — Ammo Resource`:** that spec consumes the FIRE path,
> the per-pawn weapon map, and the named reload-delivery seam this spec builds.
>
> **Supersedes** the earlier `E16 — Host Combat Command Application`
> (host-authoritative-fire) draft: same milestone, reshaped authority model.

## Goal

Make co-op (PvE) combat **client-authoritative HIT, host-authoritative WORLD** —
consistently, even for hitscan — to lay the foundation for hitscan shotgun-pellet
spreads and, soon, projectiles. The client detects its own hits against the world
as it renders it and declares them; the host validates cheaply and applies the
damage; enemy health and death stay host-owned and replicated, never predicted.

This rides the engine's existing prediction + reconciliation + interpolation
netcode with **no server rewind**. PvP is a non-goal, so a trust-with-cheap-
validation model is sound: fire-rate and ammo — how often and how many shots — are
the damage-integrity surface and stay host-authoritative, while per-shot geometry
is trusted from the client and sanity-checked.

## Scope

### In scope

- **The FIRE / HIT split.** FIRE (cooldown + ammo) is host-authoritative weapon
  state, client-predicted and reconciled on the existing command timeline. HIT
  (damage) is client-authoritative detection: the client casts its own ray against
  its rendered world and declares the result; the host validates and applies.
- **Client-hittable remote enemies.** The local hit query targets host-materialized
  remote enemies (which carry no `Health`) at their rendered (interpolated) pose,
  WITHOUT making them damageable client-side.
- **A read-only client fire path.** The connected client, which today runs no
  weapon code, gates fire on client-side weapon state and runs the local hit query
  at the rendered pose.
- **A standalone hit-declaration message** referencing a `shot_id`, carrying 0..N
  hit records `{target NetworkId, point, zone}`, arriving on the fire tick or a
  later tick (projectile-ready).
- **Host fire application per pawn:** validate fire-legitimacy, advance cooldown,
  consume ammo, mint/record the authorized `shot_id` — no ray cast. Reload rides
  the same per-pawn seam.
- **Host hit-declaration ingest, four-check validation, and damage application**
  crediting the declaring client's pawn.
- **Owner-private reconcile facts:** per-shot accept/reject and the firing pawn's
  own weapon cooldown, scoped to the owning client the way movement-authority
  metadata is.
- **`reload` on the command frame**, delivered to the mapped pawn's weapon through
  a named seam the ammo spec fills.
- **A host pawn -> active-weapon map** and **both `NetworkId <-> EntityId` reverse
  maps** (client `EntityId -> NetworkId` to build the declaration; host
  `NetworkId -> EntityId` to resolve the target).
- **One atomic wire-contract bump** of BOTH version constants.

### Out of scope (non-goals)

- **Server rewind, favor-the-shooter, lag-compensation history window.** Made
  unnecessary by client-side HIT detection + reconciliation; contradicts this
  engine's prediction-not-rewind netcode and the `index.md` §4 non-goal.
- **PvP, anti-cheat, deterministic lockstep.** Co-op PvE only; the trust model
  depends on cooperating players.
- **Dynamic-occluder line-of-sight validation.** The host validates LOS against
  static world geometry only, never against the live enemy pose or other dynamic
  occluders.
- **The ammo resource, magazine/reserve mutation semantics, and what reload
  transfers.** The ammo spec owns those; this spec delivers the fire and reload
  intents and the seams, not the mutations.
- **Projectiles as a resolution mode.** The message and validation are shaped to
  admit a later-arriving, multi-record declaration, but projectile flight,
  spawning, and replication are a separate spec.

## Acceptance criteria

- [ ] With a resourceless weapon, a connected client firing at a host-replicated
  enemy resolves a hit LOCALLY against that enemy at the pose the client renders
  (the interpolated pose), not the host's present pose. The query returns a hit for
  a drawn remote enemy that has no `Health`.
- [ ] Single-player and listen-host hit behavior is unchanged — the same local hit
  query serves all three roles, and a Health-bearing target still resolves and is
  damaged exactly as before.
- [ ] The client's fire path produces a locally-resolved hit only when its own
  weapon state permits (off cooldown, fire-mode satisfied); a held trigger on a
  cooling weapon produces no local hit.
- [ ] `reload` round-trips the command frame and survives the command queue
  (sanitize / hold / neutral / catch-up). The standalone hit-declaration message
  round-trips its `shot_id` and its 0..N hit records. The owner-private per-shot
  accept/reject fact round-trips to the owning client only. The client can name any
  targeted remote enemy on the wire, and the host can resolve any declared target
  back to a live entity.
- [ ] Both version constants are bumped in one change — the message-vocabulary
  constant and the byte-layout constant — and BOTH handshake gates assert the new
  values; a peer built before this change is refused at the handshake.
- [ ] A remote client's fire advances THAT pawn's own weapon cooldown and consumes
  its ammo host-side with no ray cast, and records an authorized shot for that pawn.
  Two owned pawns firing on the same tick each affect only their own weapon — no
  cross-talk.
- [ ] A remote client's reload reaches its own pawn's mapped weapon through the
  named delivery seam, and no other pawn's weapon.
- [ ] Every owned remote pawn is mapped to its active weapon at slot accept and
  unmapped at slot close. A pawn whose descriptor declares no weapon maps to none,
  never fires host-side, and is logged once (not an error).
- [ ] The host applies damage from a validated declaration crediting the declaring
  client's pawn as attacker, reusing the existing zone-multiplier, credit-source,
  contributor-ledger, and death-sweep path. A pitched or zone-tagged hit credits and
  scales correctly.
- [ ] **`shot_id` binding (security).** A fire the host REJECTED (cooling weapon or
  empty magazine) yields NO accepted hit and NO damage, even when the client sends a
  geometrically plausible declaration — because the declaration's `shot_id` matches
  no host-authorized open shot. One authorized fire accepts at most one declaration.
- [ ] **World-LOS, not live pose.** A declaration whose point sits behind a static
  wall (a wall nearer than the declared point along the attacker's eye ray) is
  rejected. A legitimate shot at a moving enemy is NOT rejected — validation runs
  against static world geometry only, never against the enemy's live host-present
  pose.
- [ ] Accepted hit records are clamped to the weapon's effective pellet count; a
  declaration carrying more records than that applies damage for at most that many.
- [ ] **Double-application invariant.** The client structurally cannot mutate enemy
  health locally: remote enemies carry no client-side `Health`, so the local hit
  query targets them for hitmarker/FX only and no client damage path can reach them.
  Enemy health and death are host-authoritative and replicated, never predicted; the
  client has no enemy-HP rollback.
- [ ] The firing client predicts local cooldown, muzzle FX, and a hitmarker on fire,
  and reconciles cooldown against the owner-private authoritative fact and the
  hitmarker against the per-shot accept/reject. A rejected fire rolls back the
  client's local FX, cooldown, and hitmarker. The movement reconciliation replay
  path stays weapon-free — no weapon tick runs inside it.
- [ ] The deterministic test harness exercises: a client resolving a hit against a
  remote enemy at the rendered pose; a declaration round-tripping and applying damage
  host-side; a rejected fire yielding no accepted hit despite a plausible declaration;
  pellet-count clamping; a pitched/zone hit crediting correctly; a through-wall
  declaration rejected while a moving-enemy legitimate shot is not false-rejected; two
  pawns firing independently; both version constants bumped with both gates asserting.
- [ ] No new `unsafe` (review/grep gate).

## Tasks

### Task 1: Client-hittable remote enemies

Give the shared nearest-entity hit query a hittable / hit-zone iteration basis
instead of gating on `Health`. Today it iterates `iter_with_kind(ComponentKind::Health)`
and, per entity, prefers the zone-bearing skeletal capsules and otherwise falls back
to the authored `hitbox` AABB; an entity with no `Health` is never even considered.
Host-materialized remote enemies carry a mesh and a hit-zone store entry (uploaded to
the client `HitZoneStore` at level load) but NO `Health` and NO `hitbox` component, so
they are currently unhittable by the local query. Widen the iteration basis so an
entity that has a zone-bearing model in the store (its derived reach bound is a valid
coarse fallback when no `hitbox` is present) is a candidate, in addition to today's
Health-bearing entities. Do NOT make these entities damageable: they still have no
`Health`, so the damage chokepoint no-ops on them by construction. Preserve the
existing zero-HP skip for Health-bearing entities. The query stays a pure read used by
single-player, listen-host, and client alike. AC: single-player and host hit behavior
is unchanged (the query is shared), and a drawn remote enemy with no `Health` returns a
hit.

### Task 2: Client fire path + client-side weapon state + local hit query

The connected client runs no weapon code today — after `client_predict_movement_tick`
it `continue`s past `simulate_tick`, because AI / weapons / death are host-authoritative
and arrive via snapshots. Add a read-only client fire path in that same frame branch,
as a small helper the frame loop calls (mirroring `client_predict_movement_tick`; do not
grow the frame loop inline). The client's own pawn has no local `Weapon` component today
(only host pawns and the host's `active_wieldable` do), so first establish the minimal
client-side weapon state this path needs: at least the cooldown timer and fire-mode
gating for the client's own weapon, seeded from the client pawn's descriptor at
materialization. Each fire tick: build the aim ray from the client camera
(`aim_ray()` gives origin + full-pitch direction); gate on the client-side weapon state
(off cooldown, fire-mode satisfied — magazine gating arrives with the ammo spec); and,
when the gate passes, run the shared local hit query (Task 1) against the client
collision world and the client `HitZoneStore` at the hoisted animation clock, so the ray
tests the SAME rendered (interpolated) pose the player sees. Produce a locally-resolved
hit (0..N target/point/zone records for the pellet-ready shape; one record for a single
hitscan ray). This task produces the local resolution; Task 3 gives it a wire, Task 7
predicts and reconciles it. AC: the client resolves a hit against a remote enemy at the
rendered pose, and only fires when its own weapon state permits.

### Task 3: Wire changes — one atomic two-constant bump

Land every new wire surface in one contract change. (1) Add a `reload: bool` field to
the command frame — `SimCommand` (engine) and the wire `InputCommand` — threaded through
the wire<->engine conversion and defaulted in the neutral command; `reload` is sampled in
`main.rs` as a LEVEL (held) bit from the existing reload action, mirroring `fire_button`,
and needs no new sanitize rule (a `bool` has no invalid state). (2) Add the NEW standalone
hit-declaration message as an appended variant of the client->server message enum
(reliable Input channel): `shot_id` plus a length-prefixed list of 0..N hit records, each
`{target: NetworkId, point: [f32;3], zone: <optional tag>}`, host-clamped on ingest to the
weapon's effective pellet count. It is standalone (not folded into `InputCommand`) so it
can arrive on a later tick than the fire. (3) Add the two owner-private server->client
facts the firing client reconciles against, each by its kind: the firing pawn's own weapon
COOLDOWN as an `OwnerPrivatePlayer` state slot with a per-pawn projection mirroring the
health projection — the same state-slot carrier the ammo spec extends with magazine/reserve
(player weapon cooldown is not replicated today, only enemy attack cooldown is) — and the
per-shot ACCEPT/REJECT verdict as a reconciliation ack scoped to the owning client the way
movement-authority metadata (`last_processed_client_tick`) is scoped per record, keyed by
`shot_id`. (4) Add BOTH reverse maps: the client `EntityId -> NetworkId` (to name
a locally-hit remote enemy on the wire) and the host `NetworkId -> EntityId` (to resolve a
declared target to a live entity), each maintained beside its existing forward map and
kept in lockstep on spawn/despawn. Thread the new command field through the wire-convert
and sanitize paths, and default it in `neutral_sim_command`. Bump the message-VOCABULARY
constant (a new message family — mirrors the current "adds the state-slot message family"
bump) AND the byte-LAYOUT constant (added fields change bitcode layout), and note in one
line which axis each answers. Update BOTH drift-guard sides (net crate and engine) and
assert BOTH handshake gates. AC: reload, the declaration, the cooldown slot, the
per-shot ack, and both reverse maps round-trip; both constants bump; both gates assert; a
pre-change peer is refused.

### Task 4: Host fire application per pawn

Generalize the host's per-pawn command resolve so each owned remote pawn's fire intent
reaches its own weapon. Today the host keeps only `.movement` per pawn and drops
`fire_button`; widen the per-pawn resolve to carry the whole resolved command
(`fire_button`, `reload`) alongside `movement`, then in the sim's weapon stage apply fire
PER PAWN — mirroring how movement iterates the resolved remote pawns then appends the
listen host's own pawn. For each owned pawn: resolve its weapon via the pawn -> weapon map
(Task 5), run the weapon-state half of the resolved weapon tick — validate fire-legitimacy
(off cooldown; has ammo once the ammo spec lands), advance cooldown, consume ammo, and
mint and record an authorized `shot_id` derived from `(pawn, fire-tick)` so both sides
compute it identically — but perform **no ray cast** (the client owns HIT). Record the
authorized shot per pawn as a still-open shot the hit-ingest path (Task 6) matches and
retires. A resolved owned pawn that maps to no weapon logs once via a per-pawn de-dup latch
and never fires. Fold in the **reload-delivery seam** here: route each pawn's resolved
`reload` to that pawn's mapped weapon as a single named call site (name it clearly — the
ammo spec fills the body BY NAME, not by task number), keeping it one seam so the ammo spec
adds no plumbing. The listen host's own pawn keeps its camera-driven fire path, appended
last. AC: a remote pawn's fire advances only its own cooldown/ammo and records an
authorized shot, with no ray cast; two pawns are independent; reload reaches the correct
weapon.

### Task 5: Pawn -> active-weapon map (host)

Add a host-owned `pawn -> active-weapon` map, mirroring `MovementOwners`, as a field on
the same struct that owns `MovementOwners`, constructed alongside it at every construction
site (the owning struct's initializer, the accept/close paths, and the test fixtures).
`spawn_net_slot_pawn` today spawns the pawn's sibling `defaultWeapon` instance and DISCARDS
its `EntityId`; return it instead (as `Option<EntityId>`). The accept intermediary
(`on_slot_accepted`, which calls `spawn_net_slot_pawn`) currently returns `(pawn, net_id)`
up to the `owners.set(pawn, client_id)` site; widen BOTH `spawn_net_slot_pawn`'s return AND
that accept-path tuple to carry the `Option<EntityId>` weapon id, so it reaches the
`owners.set` site where the new map is written in the same accept path. A pawn with no
weapon records no entry. Clear the map entry on slot close beside the movement-owner
removal. This map is needed for fire-legitimacy validation, credit source, cooldown, and
hit-declaration attacker resolution. Changing `spawn_net_slot_pawn`'s return type breaks its
test callers (in the descriptor tests and the predict/reconcile harness fixtures) — update
them. AC: every owned pawn maps to its weapon at accept and unmaps at close; a weaponless
pawn maps to none.

### Task 6: Host hit-declaration ingest, validate, and apply

Ingest the standalone hit-declaration, run the four-check validation IN THIS EXACT ORDER,
then apply damage. **Check 1 (first, load-bearing): `shot_id` binds to a host-authorized
fire.** Reject any declaration whose `shot_id` has no matching still-open authorized shot
recorded for that pawn on the FIRE path (Task 4), and retire the shot on accept so one
authorized fire accepts at most one declaration (and at most `pellet_count` hit records).
This is the security spine: without it a client whose predicted fire the host rejected
(cooling / empty) could collect unbounded free damage — HIT-accept depends on
FIRE-authorization, a one-way dependency. **Check 2:** each record's target resolves via the
host `NetworkId -> EntityId` reverse map (Task 3) and is alive (`Health.current > 0`); a
declaration naming a just-despawned enemy simply misses the lookup and drops (NetworkId is
never recycled). **Check 3: world-geometry line-of-sight ONLY** — cast a ray against STATIC
world geometry from the attacker eye (pawn transform + `standing_eye_height`) toward the
declared point, and reject if a wall is nearer than the declared point; do NOT re-check LOS
against the live enemy pose (the client aimed at the interpolated past pose; the host is in
the present, so a live-pose recheck would false-reject legitimate shots on moving enemies).
Dynamic-occluder LOS is intentionally not validated. **Check 4:** generous range tolerance
(attacker->point distance vs the weapon's effective range times a tolerance factor).
Fire-legitimacy (cooldown / ammo) is NOT re-checked here — it lives on the FIRE path. Then
apply damage: refactor `apply_weapon_impact_damage` to take the ATTACKER as a parameter (it
currently hard-codes the attacker via `local_movement_pawn`; the weapon is already a
parameter) so the declaring client's pawn is the attacker, and reuse its zone-multiplier,
credit-source, contributor-ledger, and death-sweep path unchanged. Emit the owner-private
per-shot accept/reject fact (Task 3) back to the declaring client. AC: a validated
declaration applies damage crediting the declaring pawn; the `shot_id`-binding, world-LOS,
pellet-clamp, and double-application invariants hold.

### Task 7: Client predict + reject-rollback

Predict the local shot when the client fires: advance the client-side weapon cooldown, play
muzzle FX, and show a hitmarker for a locally-resolved hit — a local weapon tick keyed to
the sent command, NEVER run inside movement's `replay` (which must stay weapon-free; its
registry-blind signature is the guard). Reconcile: the firing pawn's own weapon cooldown is
not replicated today (only enemy attack cooldown is), so consume the owner-private
authoritative cooldown slot (Task 3) to reconcile the predicted cooldown, and consume the
owner-private per-shot accept/reject to confirm or retract the hitmarker. On a rejected fire
(host refused: cooling / empty), roll back the client's local FX, cooldown, and hitmarker.
State explicitly: enemy health is NEVER predicted — there is no enemy-HP rollback, only
local FX / cooldown / hitmarker rollback; enemy HP and death arrive by replication. Ammo
later layers magazine/reserve as additional reconciled owner-private facts via its own
projection; this spec adds only the resourceless cooldown fact and the per-shot ack. AC: the
client predicts and reconciles cooldown and hitmarker, rolls back on reject, and never
predicts enemy HP; replay stays weapon-free.

### Task 8: Tests

Reuse the `command_queue.rs` / `wire_convert.rs` / `predict_reconcile_harness.rs` patterns.
Cover: the client resolves a hit against a remote enemy at the rendered (interpolated) pose;
a hit-declaration round-trips and applies damage host-side; a fire the host rejected
(cooling / empty) yields NO accepted hit even with a geometrically plausible declaration (the
`shot_id`-binding security test); a declaration with more records than `pellet_count` applies
damage for at most that many; a pitched / zone-tagged hit credits and scales correctly;
world-LOS rejects a through-wall declaration while a moving-enemy legitimate shot is NOT
false-rejected; two pawns fire independently against their own weapons; both version
constants are bumped and both handshake gates assert (a pre-change peer is refused). AC: the
harness exercises each listed behavior deterministically.

## Sequencing

**Phase 1 (concurrent):** Task 1 (hittable basis), Task 2 (client fire path), Task 5
(pawn -> weapon map) — wire-independent, disjoint seams.
**Phase 2 (sequential):** Task 3 — the wire/version contract, once the message and fact
shapes from Phases 1/2 are pinned.
**Phase 3 (concurrent):** Task 4 (host fire application) and Task 6 (hit ingest + validate +
apply) — both consume Task 3's wire and Task 5's map; Task 6 matches the authorized shots
Task 4 records, so land Task 4's shot-authorization record shape before Task 6 asserts
against it (sequence 4 then 6 if they share the shot-record file).
**Phase 4 (sequential):** Task 7 — client predict/reconcile against Tasks 3/4/6 outputs.
**Phase 5 (sequential):** Task 8 — verifies the surface.

## Rough sketch

Grounded seams (current source):

- `weapon/mod.rs`: `tick_resolved:133` splits the weapon-state half (`:152-166`) from the
  cast half (`:167-177`); `WeaponFireCommand:47`, `WeaponImpact:61`, `ActivationOutcome:27`.
  The FIRE path runs the weapon-state half only.
- `scripting/systems/hit_zones.rs`: `nearest_entity_hit:540` iterates
  `iter_with_kind(ComponentKind::Health):563`; `coarse_fallback_hit:717` accepts
  `hitbox: Option<&Hitbox>` and resolves from the derived reach bound when `None` — the hook
  for Task 1's Health-free basis.
- `netcode/remote_materialize.rs:38-51`: remote enemies get mesh only, no
  `Health`/`Weapon`; `startup/lifecycle.rs:1100-1104` uploads their models to the client
  `HitZoneStore`; `client.rs::sample_into_registry:1189` -> `set_presentation_transform:1213`
  writes the interpolated pose into the readable `Transform` — the rendered pose the client
  hit query tests.
- `sim/mod.rs`: `SimCommand { movement, fire_button }:28` gains `reload`; the weapon stage
  (`run_weapon_fire_tick:224` -> `apply_weapon_impact_damage:250`) becomes per-pawn;
  `apply_weapon_impact_damage` hard-codes the attacker via `local_movement_pawn:277` — Task 6
  parameterizes it (the weapon is already a param).
- `netcode/command_queue.rs`: `host_resolve_movement_inputs:355` keeps only `.movement` —
  widen to the whole command; `MovementOwners:48` is the map to mirror for the pawn->weapon
  map; `neutral_sim_command:376` gains `reload: false`.
- `netcode/lifecycle.rs`: `on_slot_accepted:148` calls `spawn_net_slot_pawn:184` and returns
  `(pawn, net_id)`; widen to carry the weapon id to `owners.set` (`netcode/mod.rs:1168`).
  `scripting/builtins/net_descriptor.rs:37` spawns and discards the sibling weapon id.
- `crates/net/src/wire.rs`: `ClientMessage:931` (append the hit-declaration variant);
  `InputCommand:836` (add `reload`); `WireMovementInput.facing_yaw:815` stays yaw-only — no
  pitch is added (the client casts locally; the declared point crosses instead).
  `crates/net/src/transport.rs`: `PROTOCOL_ID:46`, `WIRE_VERSION:52` — bump both.
- `netcode/prediction.rs`: `PredictedTick.command:147` already retains the full command;
  `replay:426` stays weapon-free. `netcode/reconcile.rs`: the prune-through-ack + merge loop
  is the model (production entry `reconcile_local_pawn_with_mover_history`; `reconcile_local_pawn`
  is `#[cfg(test)]`).
- `camera.rs::aim_ray:139` (origin + full-pitch direction, client-local);
  `player_movement.rs::standing_eye_height:203` (host attacker eye).

Proposed shapes (`// Proposed design`, remove after implementation):

```rust
// Proposed design — engine side.
struct AuthorizedShot { shot_id: ShotId, pawn: EntityId, fire_tick: u32 }   // still-open until a hit-declaration retires it
// ShotId derived from (pawn NetworkId, fire client_tick) — both sides compute it identically.

// Proposed design — wire side (bitcode Encode/Decode), appended to ClientMessage.
struct HitDeclaration { shot_id: u64, records: Vec<HitRecord> }             // length-prefixed; empty list = a shot that hit nothing
struct HitRecord { target: u32 /* NetworkId */, point: [f32; 3], zone: Option<String> }
```

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
| --- | --- | --- | --- | --- | --- |
| Reload intent | `SimCommand::reload: bool` | `InputCommand.reload` (bitcode `bool`); part of the byte-layout bump | n/a | n/a | n/a |
| Aim / pitch | client camera `aim_ray()` (full pitch) | **does NOT cross the wire** — the client casts locally; the declared `point` carries the spatial payload | n/a | n/a | n/a |
| Hit declaration | new engine `HitDeclaration` + `HitRecord` | appended `ClientMessage` variant; `shot_id: u64`, length-prefixed `Vec<HitRecord>` | n/a | n/a | n/a |
| Hit record fields | `target: EntityId` (resolved), `point: Vec3`, `zone: Option<String>` | `target: u32` (NetworkId), `point: [f32;3]`, `zone: Option<String>` | n/a | n/a | n/a |
| shot_id | `ShotId` from `(pawn NetworkId, fire client_tick)` | `u64` on the declaration; recomputed host-side, not trusted blindly | n/a | n/a | n/a |
| Per-shot ack | owner-private accept/reject fact | server->client, owner-scoped like movement-authority metadata; part of the vocabulary bump | n/a | n/a | n/a |
| Owner weapon cooldown | firing pawn's `cooldown_remaining_ms` | owner-private reconcile fact (server->client), owner-scoped | n/a | n/a | n/a |
| Client `EntityId -> NetworkId` | new reverse map beside `ClientReplication.map` | n/a (engine-side; net crate never sees `EntityId`) | n/a | n/a | n/a |
| Host `NetworkId -> EntityId` | new reverse map beside `NetworkIdAllocator` | n/a (engine-side) | n/a | n/a | n/a |
| Pawn -> weapon map | new host map beside `MovementOwners` | n/a (engine-side) | n/a | n/a | n/a |

## Wire format

The codec is **bitcode** (owns endianness and bit-packing; wire types do no manual byte
layout). Two new binary surfaces land here, both gated on the two-constant handshake.

**Hit-declaration message (client -> server, reliable Input channel).** Appended as a NEW
variant of the tagged client->server message enum (`ClientMessage`), preserving the
discriminant order of existing variants — that is why the app-protocol (vocabulary) constant
bumps, mirroring its current "adds the state-slot message family" note. Layout: `shot_id: u64`
first, then a bitcode length-prefixed list of hit records; each record is
`{ target: u32 (NetworkId), point: [f32; 3], zone: Option<String> }` in that field order. An
**empty record list** is valid and encodes a shot that hit nothing (the host retires the
authorized shot, applies no damage). The host clamps the decoded record count to the weapon's
effective pellet count on ingest. This message mirrors the existing `ClientMessage` variant
family (`Input`, `Ack`, `BaselineRefresh`, ...) — a tagged, self-describing, append-only enum.

**Owner-private per-shot ack and cooldown fact (server -> client).** Owner-scoped, delivered
only to the owning client, mirroring how movement-authority metadata (`local_player`,
`last_processed_client_tick`) is scoped per record and validated only on the owner's records.
Its addition to the server->client vocabulary is part of the same vocabulary bump; its byte
layout is part of the layout bump.

**`reload` field** on `InputCommand` is a single `bool` appended to the existing layout — a
byte-layout change, so the layout constant bumps.

**Two-constant bump, one per axis** (both drift-guard sides updated, both gates asserted):

| Constant | Axis | Why it bumps here |
| --- | --- | --- |
| `PROTOCOL_ID` | message VOCABULARY | new hit-declaration message family + owner-private per-shot ack |
| `WIRE_VERSION` (6 -> 7) | byte LAYOUT | `reload` field + the new messages' own bitcode layout |

## Design decisions & rationale

- **FIRE / HIT split.** FIRE (cooldown + ammo) is the damage-integrity surface — how often
  and how many shots — and stays host-authoritative, client-predicted and reconciled on the
  existing command timeline. HIT (geometry) is trusted from the client, which casts against
  the world it actually renders, and the host sanity-checks it. This is what makes hitscan
  consistent with future pellet spreads and projectiles: all three are "declare resolved
  hits against the rendered world," differing only in ray count and arrival timing.
- **`shot_id` binds HIT-accept to FIRE-authorization (one-way).** The host authorizes shots
  on the FIRE path and accepts at most one declaration per authorized shot. Because
  fire-rate and ammo are the integrity surface, damage MUST check against an authorized fire;
  a rejected fire (cooling / empty) can produce a plausible declaration but no authorized
  shot, so it applies no damage. This is checked FIRST and is the security spine.
- **World-LOS only, never live-enemy-pose LOS.** The client aims at the interpolated (past)
  enemy pose; the host is in the present. Re-checking LOS against the live pose would
  false-reject legitimate shots on moving enemies — silently rebuilding the staleness miss
  this design removes, the same reason server rewind is ruled out. The host validates against
  static world geometry only; dynamic occluders are not validated.
- **Enemy health is never client-predicted.** Remote enemies carry no client-side `Health`,
  so the client structurally cannot mutate enemy HP — it only shows a hitmarker. Enemy HP and
  death are host-authoritative and replicated. This makes double application impossible by
  construction, not by a runtime guard, and means there is no enemy-HP rollback path.
- **No pitch on the wire.** The superseded draft added `pitch` because the HOST cast the ray.
  Here the CLIENT casts locally with its full camera aim and sends the resolved `point`, so
  the host needs no pitch — it validates from the eye toward the declared point. The aim/pitch
  boundary row records this deletion explicitly.
- **One spec, reload included.** `reload` rides this spec's single wire bump (a held LEVEL bit
  sampled like `fire_button`); the ammo spec consumes `SimCommand.reload` and fills the named
  reload seam, owning no wire change. Rising-edge detection (a held key reloading once) is the
  ammo spec's job at consumption, not an input-layer pulse.
- **Two-constant bump, one per axis.** New message vocabulary bumps the app-protocol constant;
  added fields / new message layout bump the wire-version constant. Both drift guards updated,
  both handshake gates asserted, so a pre-change peer is refused at the handshake.
- **Owner-private reconciliation facts map to two carriers by kind, following the movement
  pattern this spec mirrors.** Continuous weapon STATE the client reconciles — the firing
  pawn's own cooldown, and (in the ammo spec) magazine/reserve — is an `OwnerPrivatePlayer`
  state slot with a per-pawn projection, exactly as `player.health`; this is the carrier the
  ammo spec extends. The per-shot ACCEPT/REJECT verdict is a reconciliation ack, so it rides
  owner-scoped authority metadata alongside movement's `last_processed_client_tick`, keyed by
  `shot_id`. Cooldown is to the replicated `Transform` as the shot verdict is to
  `last_processed_client_tick`. The verdict's exact encoding (per-shot list vs. a resolved
  high-water mark plus a reject set) is routine implementation latitude.
