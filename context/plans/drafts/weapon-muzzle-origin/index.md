# Weapon Muzzle Origin

## Goal

Give weapon authors control over where a projectile originates. A per-weapon
model-local muzzle point moves the authoritative projectile spawn from the camera
eye to the barrel, composed through the weapon's authored placement and aimed so
the shot still converges on the crosshair at range. The visible projectile then
emerges from the gun instead of the screen center. Omitting the muzzle point
reproduces today's eye-origin behavior exactly.

## Scope

### In scope
- A per-weapon `muzzleOffset` on the weapon descriptor: a **model-local** `[x, y, z]`
  point in the viewmodel's own coordinate frame — intrinsic model geometry, like a
  hit zone. Threaded to the weapon component and effective stats, and replicated in the
  opaque tuning payload beside placement (host value, no client-local fallback). SDK
  types, validation, hover docs.
- A shared deterministic resolver that composes the muzzle world origin as
  `eye ∘ placement ∘ muzzle_local` from the tick-rate aim pose and the resolved
  **steady** `WeaponPlacementDescriptor` (no view-feel).
- Projectile fire: spawn at the muzzle, aim at the camera-ray convergence point
  (nearest world/entity hit, else the far point at `range`), across the local fire
  paths, the remote authoritative origin, and the remote presentation origin.
- An author-time read that reports a weapon viewmodel's rigid `"muzzle"` socket as a
  model-local `[x, y, z]` for pasting into `muzzleOffset`, homed in the landed mount
  tooling.
- Backward compatibility: absent `muzzleOffset` ⇒ eye origin and aim direction,
  byte-identical to current behavior. Hitscan resolution is unchanged.

### Out of scope
- **Positioned muzzle flash / tracer for hitscan (and any engine-owned muzzle
  VFX).** Muzzle FX today is a mod reaction bound to the `"activate"` event;
  `muzzle_fx_visible` is a boolean predicted-shot gate with no position
  (`weapon/mod.rs`). There is no engine-side positioned flash to re-anchor. Emitting
  FX at the muzzle needs a new spawn-at-muzzle presentation seam — a separate
  feature. See Decisions.
- **A third-person avatar-socket muzzle for observers.** An observer watching a
  remote shooter sees the weapon posed by the avatar hand socket, which reads no
  placement — a foreclosed divergence (`weapon-placement`; `networking.md`
  §Weapon placement is content). v1 spawns the remote presentation projectile at the
  authoritative FP-composed muzzle world point, so presentation and validation share
  one origin. A presentation muzzle anchored to the avatar's own weapon socket — the
  observer's vantage — is a follow-up; it needs muzzle data on the avatar model,
  which does not exist. See Decisions.
- **Runtime resolution of the glTF `"muzzle"` socket into the authoritative offset.**
  The viewmodel is client-local; the host lacks remote viewmodels, so a live socket
  read cannot feed the deterministic origin (`research.md`). The socket is an
  author-time source only.
- **Build-time bake** of the socket into the descriptor. Possible later; not v1.
- Moving the **hitscan** origin off the eye. Crosshair-perfect hitscan is kept.
- Per-weapon selection of convergence vs parallel-ray aiming. v1 is
  converge-to-crosshair for every projectile weapon.
- Augment/mount-point machinery from `research/weapon-model.md` §3. `muzzleOffset`
  is a flat precursor, not the slotted mount system.

## Direction

**Problem.** Projectiles spawn at the camera eye
(`ProjectileLaunch.origin = aim_origin`, `resolve_client_fire`; `remote_fire_origin`,
`commands.rs`). The landed `weapon-placement` spec moved the viewmodel to an authored
position but deliberately kept the fire origin at the eye — a weapon's visible barrel
is still cosmetic, and authors have no control over the origin.

**Placement.** Three axes decide where each piece lives. (1) engine-floor vs
mod-data: the muzzle *point* is per-weapon model geometry → mod/descriptor content,
absent by default; the *composition and convergence* are one right answer → engine
floor, one shared function. (2) deterministic-sim vs client-presentation: the
authoritative origin runs in the game-logic tick and is re-derived on the host for
remote shooters, so it composes against the tick-rate aim pose and the steady
placement with no view-feel; the VFX muzzle (out of scope) is client-presentation.
(3) model-local vs camera-relative: the muzzle is authored in **model-local** space
so it composes through *any* placement unchanged (`eye ∘ placement ∘ muzzle_local`).
A camera-relative offset would bake the placement in and break whenever the placement
changes. These placements are the load-bearing decisions; a reviewer should check
them first.

**Composes onto placement.** `weapon-placement` (done) authors the first-person
viewmodel's position as a reusable `WeaponPlacementDescriptor`, resolves it
`mod default < per-weapon` via `resolve_weapon_placement`, and replicates the
host-resolved effective placement in the opaque tuning payload
(`networking.md` §Weapon placement is content). It exposes the resolved value as a
**steady, pre-sway** `placement_offset` / `placement_rot` at the render seam
(`viewmodel_camera_space_transform`, before the `sway_rot` multiply) precisely so
this spec can read it. The muzzle origin reads that same host-authoritative steady
placement — a connected client from the replicated tuning payload, the host from its
own resolve — so both peers agree; render-rate view-feel sway stays cosmetic and
excluded from authority. `muzzleOffset` rides the same tuning payload, so the whole
composed origin is host-sourced on every peer.

**Prior commitments.** This spec composes onto `weapon-placement` — the muzzle point
is intrinsic model geometry that rides *through* the authored placement, not a
second placement system. `research/weapon-model.md` §3 (Amendment 2026-07) names a
`MountPoint` union with `muzzle` and "one modifier system, not two." A flat
`muzzleOffset` on the weapon block is exactly the effective muzzle a future
mount/augment layer would resolve into — additive, not competing. The netcode
contract that `AuthorizedShot.fire_origin` measures projectile travel and gates the
declared-contact distance check (`netcode/mod.rs`) is preserved by keeping the
spawned origin and `fire_origin` coincident at the muzzle. `E21` (bone sockets) and
`weapon-mount-frame-solver` (done) foreclose per-attachment offset *in data* — "art
fixes placement in the prop or the socket, not in data" — for the third-person mesh
mount. `muzzleOffset` is not that case: it is a **simulation projectile-spawn
origin**, replicated content the deterministic tick reads on every peer, which a mesh
transform cannot supply (the host has no remote viewmodel). The surface resonance
(muzzle + socket) is coincidental; the domains do not overlap.

**Alternatives rejected.** (1) Author the muzzle **camera-relative** rather than
model-local. Rejected: a camera-relative offset bakes the placement in, so every
placement edit silently invalidates the muzzle and dual-wield cannot share one
authored value. Model-local composes through any placement unchanged. (2) Keep the
authoritative origin at the eye and make the barrel a presentation-only offset
(visually lerp the projectile from barrel to true path). Rejected: the owner chose
muzzle-authoritative; a cosmetic lerp adds sim/visual divergence the contact
validation would have to paper over, and is more code than moving the origin.
(3) Read the muzzle live from the viewmodel socket at fire time. Rejected: the
viewmodel is client-local, so the host would compute a different origin for remote
shooters, breaking determinism and hit validation.

## Acceptance criteria

- [ ] **AC 1.** A weapon descriptor with `muzzleOffset` omitted spawns its projectile
      at the camera eye with the aim direction — byte-identical launch to current behavior
      (regression pin).
- [ ] **AC 2.** A weapon descriptor with a `muzzleOffset` spawns its projectile with
      `Transform.position` at the composed muzzle world point
      (`eye ∘ placement ∘ muzzle_local`), not the eye.
- [ ] **AC 3.** The same `muzzleOffset` under two different resolved placements (a
      per-weapon placement vs the mod default, or two per-weapon placements) yields two
      different muzzle world points — the muzzle composes through the authored
      placement, not a fixed camera offset.
- [ ] **AC 4.** An authored placement rotation rotates the muzzle point: a canted
      placement moves the composed muzzle along the rotated barrel, not along world axes.
- [ ] **AC 5.** The projectile's launch direction points from the muzzle toward the
      point the camera aim ray hits (nearest world-or-entity hit within `range`); on a
      clean miss it points from the muzzle toward `eye + aim_dir * range`. At long range
      the flight path passes through the crosshair target.
- [ ] **AC 6.** The shared `muzzle_world_origin` helper is the single composition on
      every peer: given equal (eye, aim direction, resolved placement, `muzzle_local`) it
      returns the same point within float tolerance. (Equal *eye* is the premise — the
      client's render-rate eye carries the reconcile `presentation_offset` the host's
      reconstructed eye lacks, a pre-existing eye-origin divergence the muzzle work carries
      unchanged, not a term this spec closes; see Invariant 1 and Temporal pin P7.)
- [ ] **AC 6a.** Cross-peer: a connected client predicts its shot from the
      **host-replicated** placement and `muzzle_offset` (both from the tuning payload), so
      the client and host compose the **same `placement ∘ muzzle_local` sub-expression** for
      that shot — equal given equal eye/aim (the P7 reconcile-eye gap applies unchanged) —
      including after a host live-edit of placement or `muzzle_offset`, once replicated. A
      client whose local `data_registry` holds a different placement must ignore it. Runnable
      via the factored payload-term helper (Task 1) plus the pure `muzzle_world_origin`
      equality; the full frame-ordering scenarios P1–P3 and P8 are App-integration /
      review-gated, not unit tests. (Temporal pins P1–P3, P8.)
- [ ] **AC 7.** For a projectile shot, the spawned projectile `Transform.position` equals
      the `AuthorizedShot.fire_origin` used for host validation. A projectile hit
      declaration whose contact is within `range * HIT_RANGE_TOLERANCE` of the muzzle
      origin still validates.
- [ ] **AC 8.** When the convergence point is at or behind the muzzle (a surface within
      the barrel's forward reach), the launch keeps the aim direction and still spawns at
      the muzzle — no degenerate zero-length direction, and the origin stays
      host-reproducible.
- [ ] **AC 9.** Aiming straight up and straight down composes the offset through the
      pitched aim basis (the muzzle tracks the barrel's pitch, not a fixed world offset).
- [ ] **AC 10.** `muzzleOffset` rejects non-finite components at descriptor validation
      with a field-named error, consistent with other weapon-field validation.
- [ ] **AC 11.** `sdk/types/postretro.d.ts` and `.d.luau` declare `muzzleOffset` with
      hover docs (model-local frame, units, default); the SDK drift test passes.
- [ ] **AC 12.** The author read prints a weapon viewmodel's rigid `"muzzle"` socket as a
      model-local `[x, y, z]`, and errors clearly when the socket is absent or is a
      skinned joint rather than a rigid rest transform.

## Tasks

### Task 1: Muzzle content + deterministic resolver + local converge (thin slice)

**Descriptor + component.** Add `muzzle_offset: Option<[f32; 3]>` to `WeaponDescriptor`
(`crates/foundation/src/data_descriptors/types/combat.rs`), `#[serde(default)]` so an
omitted field is `None`; under the struct's existing `#[serde(rename_all = "camelCase")]`
it wires as `"muzzleOffset"`. In `WeaponDescriptor::validate` (beside the
`placement.validate()` call), reject any non-finite component with a
`components.weapon.muzzleOffset` field-named `DescriptorError::InvalidShape`, matching
the sibling numeric checks. Carry it onto `WeaponComponent` as
`muzzle_offset: Option<glam::Vec3>` (`crates/entities/src/components/weapon.rs`): set
it in `from_descriptor_with_canonical`, overwrite it in `refresh_from_descriptor`, and
surface it on `EffectiveStats` and `effective()`. The vector is **model-local** — the
viewmodel mesh's own frame, the same frame the rigid `"muzzle"` socket's rest
translation lives in (Task 4).

**Replicate `muzzle_offset` like placement — through the same payload accessor.**
`muzzle_offset` is authority-critical (the host composes `fire_origin` from it for remote
shooters, the client predicts from it), so a connected client must read the *host's*
value, not its own descriptor — the same contract placement already follows
(`networking.md` §Weapon placement is content: "clients read only the host value, no
local fallback"). Add `muzzle_offset` per wieldable row to `WieldableTuningPayload`
(`crates/postretro/src/netcode/tuning_payload.rs`, where `placement` already rides) with
accessors `muzzle_for_slot` / `muzzle_for_archetype` mirroring `placement_for_slot` /
`placement_for_archetype`, and write it host-side where `placement` is written
(`tuning_payload_for_pawn`, `netcode/mod.rs`). Critically, the connected-client fire path
reads `muzzle_offset` from the **payload accessor**, exactly as it reads placement — **not**
from the component via `apply_net_wieldable_tuning`. Placement refreshes at the `Tuning`
Control seam (`install_tuning_payload`) while the component-sync seam
(`apply_net_wieldable_tuning`) only runs on a frame that applies a local-pawn snapshot;
reading muzzle from the component would let a client compose the host's *new* placement
against a *stale* component muzzle on a Control-only frame (Temporal pin P8). Reading both
terms from the one payload keeps them atomic. The added field changes the opaque payload's
serialized shape, so bump `TUNING_PAYLOAD_EPOCH` (5 → 6) and re-bless the committed golden
fixture (`payload_json_matches_committed_fixture`, `tuning_payload.rs`, via
`POSTRETRO_BLESS_COMPATIBILITY_FIXTURES=1`). It is still not a fixed transport wire field
and does not change the mod-compatibility digest — the same category as placement. The
component keeps its `muzzle_offset` for the host/single-player authoritative and remote
paths, which read the host's own descriptor value directly.

**Shared composition seam.** Factor the placement→camera-space derivation currently
inline in `viewmodel_camera_space_transform` (`crates/postretro/src/main.rs`, the
`placement_offset = Vec3::new(right, up, -forward)` and the yaw/pitch/roll quat) into a
shared method on `WeaponPlacementDescriptor`, e.g. `camera_space(&self) -> (Vec3, Quat)`,
and call it from both the render seam and the muzzle helper so the **steady placement
sub-expression** is composed identically on both paths (the render seam then multiplies
in `sway_rot` and adds `bob_offset`, which the muzzle deliberately excludes). Add a
shared engine-floor helper — `muzzle_world_origin(eye: Vec3, aim_direction: Vec3,
placement: &WeaponPlacementDescriptor, muzzle_local: Vec3) -> Vec3` — that takes
`(p_off, p_rot) = placement.camera_space()`, composes the muzzle into camera space as
`cam = p_rot * muzzle_local + p_off` (so the muzzle rides authored placement yaw, pitch,
and **roll**), builds a right-handed basis from the aim direction and world up
(`forward = aim_direction`, `right = normalize(forward × Vec3::Y)`, `up = right × forward`),
and returns `eye + right * cam.x + up * cam.y + forward * (−cam.z)` (camera space is `−Z`
forward, matching the render seam's `−forward`). Taking the aim direction means both the
local and remote paths feed the helper the vector they already hold, so the origin is
computed one way on every peer (Invariant 1). Guard the basis inside the helper: when
`forward × Vec3::Y` is below an epsilon length (a near-vertical aim), fall back to an
alternate right axis (e.g. from world `Z`) so the helper never NaNs — the local
`Camera::aim_ray` clamps pitch short of vertical, but the remote path builds direction
from the wire `aim_pitch`, which the clamp does not govern. Place the helper as a
`crate::weapon` free function callable by the local tick and the remote command path.

**Compose at both local fire paths.** Two local paths build a gameplay `ProjectileLaunch`
at the eye origin today — the client-prediction `resolve_client_fire`
(`crates/postretro/src/weapon/mod.rs`, projectile arm) and the host/single-player
authoritative `run_local_weapon_command` → `tick_resolved_component` → `fire_hitscan`
projectile arm (same file). Both currently emit `origin: aim_origin`, so both must compose
the muzzle or the shot leaves the eye in single-player (the authoritative path) or on a
predicting client. Factor the compose + converge + guard into one shared `crate::weapon`
helper both arms call: given `Some(muzzle_local)`, the resolved steady placement, the aim
pose, `range`, and the tick's `collision_world` / `registry` / `hit_zone_store` /
`anim_time`, it computes `muzzle = muzzle_world_origin(aim_origin, aim_direction, &placement,
muzzle_local)`, finds the convergence point by casting the camera aim ray **from
`aim_origin` along `aim_direction` to `range`** via the existing `resolve_nearest_hit`
(its `cast_ray` against `collision_world` plus `nearest_entity_hit` over
`hit_zone_store`/`anim_time`), takes the nearer hit's point or `aim_origin + aim_direction
* range` on a miss, and returns `(origin = muzzle, direction = (convergence − muzzle)
normalized)`. The convergence ray originates at the eye, not the muzzle, so the shot still
converges on the crosshair. The launch is **always** at the muzzle — no eye fallback once
a muzzle is authored, so the origin is reproducible host-side from the same inputs
(Invariant 2); guard only the degenerate normalize (Invariant 4): when `(convergence −
muzzle)` is below an epsilon length or points opposite `aim_direction` (convergence at or
behind the muzzle, a surface inside the barrel's reach), keep `direction = aim_direction`
while still spawning at the muzzle. When `muzzle_offset` is `None`, both arms keep
`origin: aim_origin, direction: aim_direction` unchanged — the current code path,
byte-identical to today (Invariant 3).

**Source placement and muzzle per peer.** Neither fire arm holds these today, and
`fire_hitscan` takes scalar params (not `EffectiveStats`), so each path passes the
`muzzle_offset` and the resolved `WeaponPlacementDescriptor` it sourced into the composition
(into `resolve_client_fire` beside `aim_origin`/`aim_direction`, and through
`tick_resolved_component` into `fire_hitscan`). Each path sources them the way the viewmodel
render seam sources placement for that peer (`main.rs`, the `collect_viewmodel` site):

- **Connected client** — the client-prediction `resolve_client_fire`, whose sole production
  caller is the App fire path (`main.rs`, `run_client_fire_path_post_loop_inner`), which holds
  the tuning payload and descriptors as the render seam does. It reads **both** placement
  (`tuning.placement_for_slot(active_slot)` / `placement_for_archetype`) **and** `muzzle_offset`
  (`muzzle_for_slot` / `muzzle_for_archetype`) from the one payload — never from local content,
  never from the component. Because that App path has no unit harness, factor the per-slot
  term selection (payload placement + `muzzle_offset` for the active slot/archetype) into a
  small pure helper and unit-test it directly: given a payload and a mismatched local
  component/`data_registry`, it returns the payload terms and ignores the local ones (AC 6a).
- **Host / single-player** — the authoritative `run_local_weapon_command` →
  `tick_resolved_component` → `fire_hitscan`, reached via `simulate_tick_with_presentation_aim`
  (which holds `descriptors: &[EntityTypeDescriptor]` but not `default_weapon_placement`). It
  resolves placement with `resolve_weapon_placement(default_weapon_placement.as_ref(), None,
  weapon_authored_placement, None)` from local descriptors — looking the authored placement up
  by the fired weapon's canonical name in `descriptors` — and reads `muzzle_offset` from the
  component (`effective()`). Thread **both** `descriptors` and `default_weapon_placement` into
  `run_local_weapon_command` (both reachable at its `simulate_tick_with_presentation_aim` call
  site) and through `tick_resolved_component` into `fire_hitscan`; resolve for the pre-switch
  active weapon `run_local_weapon_command` captures at entry (the P6 instance). Name the blast
  radius: the `simulate_tick*` signatures and their test call sites, **and** the second
  production caller `simulate_client_wieldable_tick` (the connected client's equip-only tick,
  which suppresses fire via `can_fire: false`, so it never reaches the `Accepted` arm) — it
  passes the new params as `None`/default. Muzzle composition sits in the
  `WeaponFireAuthorization::Accepted` arm, so
  the client equip tick never composes a component-sourced muzzle; the client's real fire
  prediction is the payload-sourced path above. The `fire_hitscan`/`tick_resolved_component`
  signature change also touches their `tick`/`tick_resolved` test helpers — a compiler-caught
  update. No tuning handle is needed on this path, since the host is the authoritative source.

Resolve placement and read `muzzle_offset` for the **same wieldable instance the fire reads**
— the pre-switch active weapon captured at `run_local_weapon_command` entry (and the
`active_slot` the client keys its payload read on) — never the incoming weapon of a same-tick
switch. `remaining_range` stays `range`. Cover this task with unit tests for: omitted offset ⇒
eye-origin launch (AC 1); a muzzle ⇒ composed muzzle origin (AC 2); the same muzzle under two
placements yields two origins (AC 3); a placement rotation (including roll) rotates the muzzle
(AC 4); convergence direction aims at the hit point and at the far point on miss (AC 5);
degenerate-convergence guard keeps aim direction and muzzle origin (AC 8); pitched-aim
composition up and down, plus a direct `muzzle_world_origin` unit test at a near-vertical
`aim_direction ≈ Vec3::Y` (bypassing the camera pitch clamp) that exercises the basis
guard (AC 9); non-finite rejection (AC 10); and the client predicting from the replicated
payload placement and `muzzle_offset`,
not local content, including after a host live-edit and on a Control-only frame (AC 6a,
Temporal pins P1–P3, P8).

### Task 2: Remote authoritative + presentation muzzle origin

In the remote weapon-command path
(`crates/postretro/src/sim/weapon_stage/commands.rs`), route the remote authoritative
projectile origin and the presentation launch origin through the muzzle. The remote
path already reconstructs the shot's `yaw`/`pitch` and the pawn eye
(`transform.position + Vec3::Y * movement.capsule.eye_height`, `remote_fire_origin`;
direction rebuilt from yaw/pitch). It does **not** currently have the descriptor list
or the mod default in scope, so thread `descriptors: &[EntityTypeDescriptor]` and
`default_weapon_placement` into `run_remote_weapon_commands` (the same inputs
`tuning_payload_for_pawn` already uses to resolve placement host-side). For a
projectile shot, resolve the remote shooter's steady placement with
`resolve_weapon_placement(default_weapon_placement.as_ref(), None, authored, None)`
(keyed by the weapon's canonical archetype, as `tuning_payload_for_pawn` keys it — the
host is authoritative for the remote shooter, so it resolves from local descriptors, not
a payload), read `muzzle_offset` from the host-spawned `WeaponComponent`, and — when it
is `Some` — compute the muzzle **once** with the shared `muzzle_world_origin(eye,
aim_direction, &placement, muzzle_local)` helper from Task 1, passing the direction this
path reconstructs. Assign that single muzzle point to **both** `AuthorizedShot.fire_origin`
(`netcode/mod.rs`) and `RemoteProjectilePresentationLaunch.origin` (defined in
`crates/postretro/src/sim/mod.rs`, constructed in `commands.rs`), so the presentation
visual and the validation sphere share one muzzle point unconditionally (Invariant 2) —
today the two are computed independently (both recompute the eye), so a single shared
computation is what keeps them coincident. When `muzzle_offset` is `None`, keep the
current eye origin for both. The remote path does not re-simulate trajectory — projectile
hit authority is the client's declared contact validated by distance
(`valid_projectile_contact_point`, `netcode/mod.rs`) — so it needs no convergence
raycast; the presentation `direction` stays the reconstructed aim direction, **not** the
shooter's convergence direction — an accepted presentation edge (the observed trajectory
will not terminate at the host-authoritative impact; Temporal pin P5). Hitscan shots keep
`fire_origin: Vec3::ZERO` and the present-eye path unchanged.
Add a test asserting the remote muzzle origin equals the firing peer's for identical (eye,
aim direction, resolved placement, `muzzle_offset`) inputs (AC 6), and that a projectile
contact within `range * HIT_RANGE_TOLERANCE` of the muzzle validates (AC 7).

### Task 3: SDK types and hover docs

Regenerate the SDK type surfaces so `muzzleOffset` appears on the weapon block in
`sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` with hover documentation:
a **model-local** `[x, y, z]` point in metres, in the viewmodel's own coordinate frame,
default absent meaning the camera eye, moving the projectile spawn to the barrel while
the shot still converges on the crosshair — extract the value from the viewmodel's
`"muzzle"` socket with the Task 4 read. The generator source is the `.field(...)`
registration string in `crates/postretro/src/scripting/primitives/mod.rs` (not the Rust
struct's doc comment); register the field beside the existing `placement?` registration
there, then regenerate via the `gen-script-types` binary (`src/bin/gen_script_types.rs`;
`cargo run -p postretro --bin gen-script-types`). Confirm the committed
`.d.ts`/`.d.luau` match the registry so the drift test
`committed_sdk_types_match_current_registry` passes (AC 11). This task consumes the
descriptor field from Task 1.

### Task 4: Author read — muzzle socket → model-local offset

Provide an author-time read that reports a weapon viewmodel glTF's rigid `"muzzle"`
socket as a model-local `[x, y, z]` ready to paste into `muzzleOffset`. The muzzle read
is a rigid-socket translation — `LoadedModel.sockets["muzzle"] = SocketBinding::RigidRest(Mat4)`,
its `.w_axis.truncate()` in mesh-node local space (`gltf_loader.rs`) — which is
**not** what the landed mount tooling does: `mount.rs` resolves *skinned* sockets to a
world joint frame and detects the muzzle geometrically, and rejects a `RigidRest`
socket as non-skinned. Add the rigid model-local read to the landed mount home — the
`solve-weapon-mount` xtask (`crates/xtask/src/main.rs`) and `crates/model/src/mount.rs`
(`docs/weapon-mounts.md`) — as a distinct read path, not a forked tool. Error clearly
when the model has no `"muzzle"` socket, or when it is a `SkinnedJoint` rather than a
rigid rest transform (AC 12). This is tooling only — no runtime code reads the socket.

The printed value is authored in the same frame the sim composes against, because the
socket's rest translation and the mesh vertices share one frame: the engine loads models
at their authored glTF scale — no fit-to-size, unit conversion, or auto-rescale
(`resource_management.md` §7) — and the muzzle helper composes placement at scale
`Vec3::ONE`, so a socket at mesh-node-local `(x, y, z)` is the model-local point a vertex
at `(x, y, z)` occupies. This is model-local authoring fidelity, **not** a promise that
the world muzzle lands on the drawn barrel pixel: the viewmodel renders with its own tight
projection, so the world-space muzzle does not pixel-align with the drawn barrel tip
(`research.md`, "Why 'read the visible barrel' does not work") — expected and not a goal.
The rigid socket carries no Blender up-axis correction (that is the skinned-mount concern);
it is read raw, as the vertices are.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice through content → replication → resolver →
local fire → tests. Falsifies the boundary assumptions: model-local composition through
placement, the cross-peer placement/`muzzle_offset` source, convergence math, point-blank
guard, and backward compatibility. Everything else builds on the `muzzle_offset` field,
its tuning-payload replication, the shared `camera_space` seam and `muzzle_world_origin`
helper, and the `default_weapon_placement` threading Task 1 adds to the sim tick.

**Phase 2 (concurrent):** Task 2, Task 3, Task 4 — independent of each other, no shared
files. Task 2 reuses the Task 1 helper and inherits Task 1's `default_weapon_placement`
threading into `simulate_tick_with_presentation_aim` (it adds only the
`run_remote_weapon_commands` leg); Task 3 regenerates SDK types from the Task 1 field
registration; Task 4 is standalone tooling.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| `muzzleOffset` | `WeaponDescriptor.muzzle_offset: Option<[f32; 3]>` (`#[serde(default)]`); `WeaponComponent.muzzle_offset: Option<glam::Vec3>` | `"muzzleOffset"` | `muzzleOffset?: [number, number, number]` | `muzzleOffset` | n/a |

Frame (every surface): **model-local** metres — the viewmodel mesh's own coordinate
frame, the frame the `"muzzle"` socket's rest translation lives in. Absent ⇒ camera
eye (today's launch). The engine composes it as `eye ∘ placement ∘ muzzle_local`; the
author reads it from the socket (Task 4).

Replication: `muzzle_offset` rides the existing opaque `WieldableTuningPayload` (beside
`placement`), host-written per occupied wieldable — no fixed transport wire field, no
mod-digest change. A connected client's fire path reads both `muzzle_offset` and placement
from the one payload accessor (`muzzle_for_slot` / `placement_for_slot`), never from local
content or a component sync, so the two refresh atomically (Temporal pin P8). The component
`muzzle_offset` serves the host/single-player authoritative and remote paths.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| 1. Authoritative muzzle origin is computed by one shared function on every peer, so the firing peer and the host agree given equal (eye, aim_direction, resolved placement, muzzle_local). The muzzle work does not close the pre-existing eye divergence (client render-rate reconcile offset vs host reconstructed eye) — it carries it unchanged. | Task 1 (`muzzle_world_origin` + `placement.camera_space`; client and host both source the *same* replicated placement/`muzzle_offset`) | Threatened if a peer sources a different placement (client resolving local `data_registry` instead of the replicated payload) or a different `muzzle_offset` (client reading a snapshot-stale component instead of the payload accessor — pin P8). Aim-direction fidelity (client `aim_ray` vs host yaw/pitch reconstruction) is the inherited netcode contract the hitscan `fire_origin` already trusts, not newly introduced here. | AC 6, AC 6a |
| 2. The spawned projectile origin and `AuthorizedShot.fire_origin` are the same muzzle point, unconditionally (no firing-peer-only fallback the host cannot reproduce). | Task 1 (local spawn always at muzzle when authored), Task 2 (remote `fire_origin` + presentation origin) | Threatened if one path composes the muzzle and another does not, or if a peer conditionally moves the origin off the muzzle; a divergence shifts the `valid_projectile_contact_point` validation sphere off the real trajectory (`netcode/mod.rs`). | AC 7 |
| 3. Omitting `muzzleOffset` reproduces the current eye-origin, aim-direction launch exactly. | Task 1 (`None` ⇒ the unchanged eye code path; no placement composition runs) | Threatened if the `None` branch perturbs origin or direction, or if a default muzzle is synthesized when the field is absent. | AC 1 |
| 4. The launch direction is never a degenerate normalize. | Task 1 (guard on `(convergence − muzzle)`: epsilon length or opposite `aim_direction` ⇒ keep aim direction, still spawn at muzzle) | Threatened when the convergence point is at or behind the muzzle (a surface within the barrel's reach). Guarding by keeping aim direction, not by moving the origin, so Invariant 2 stays intact. | AC 8 |

## Temporal pins

The invariants state *what* holds; these rows pin the cross-peer mechanics — each
concrete enough to write a test from. The test tasks reference the rows.

| # | Scenario | Ordering | Expected outcome | Extends |
|---|---|---|---|---|
| P1 | Client predicts from the replicated payload, not local content | Join with a weapon whose per-weapon placement ≠ mod default. Host writes `P_host` / `M_host` into the tuning payload; client fires. | Client-predicted muzzle composes `P_host ∘ M_host` (from `placement_for_slot` / `muzzle_for_slot`), the same sub-expression as the host `fire_origin` (eye held equal, P7). Stubbing the client's local `data_registry` / component to different values must not change the predicted origin. | AC 6a |
| P2 | Host live-edits placement, steady state | Host edits placement `P_old→P_new`; client installs the new `Tuning`; then client fires. | Client and host compose the same `P_new ∘ muzzle` (eye held equal); no permanent stale-placement divergence. | AC 6a |
| P3 | Host live-edits `muzzle_offset` | Host edits `muzzleOffset M_old→M_new` (host `refresh_from_descriptor` + payload rewrite); client installs the new `Tuning`; client fires. | Client reads `M_new` from the payload accessor (`muzzle_for_slot`), so client and host compose the same `placement ∘ M_new`. (The client reads muzzle from the payload, not a component sync — see P8.) | AC 6a |
| P4 | Shot crosses the replication latency window | Host edits placement/muzzle at tick T; client fires at T+k before installing the new `Tuning`; host processes with the new values. | Origins differ by ≤ Δplacement+Δmuzzle (~1 m). The declared contact still validates **while** `Δplacement+Δmuzzle ≤ range * (HIT_RANGE_TOLERANCE − 1)` — a short-range projectile weapon crossing the window can drop a max-range shot (a bounded, self-correcting reject, not a desync). The accepted predicted projectile keeps flying from its predicted origin (no re-anchor on accept; `apply_verdict` despawns only on reject) — no pop, no ghost. Accepted bounded window. | AC 7 |
| P5 | Observer sees a laterally-offset remote muzzle | Remote shooter fires a projectile with a side muzzle at a close target; host spawns the presentation launch at the muzzle along the reconstructed aim direction. | The presentation projectile leaves the muzzle but flies along aim direction, not the convergence direction, so it does not visually terminate at the host-authoritative impact. Accepted v1 presentation edge — origin shared, direction not. | Observer Decision |
| P6 | Weapon switch + fire in one tick | Input carries `select_slot=incoming` and `fire`; `run_local_weapon_command` captures active=outgoing. | The shot composes the **outgoing** weapon's `muzzle_offset` through the **outgoing** weapon's resolved placement — never outgoing-muzzle × incoming-placement. If the lower blocks the fire, no shot. | AC 2 |
| P7 | Reconcile offset live in the client eye | Client mid-reconcile (nonzero `presentation_offset`) fires; host reconstructs the eye without it. | Client origin and host `fire_origin` differ by the reconcile offset even with equal placement/`muzzle_offset` — the pre-existing eye gap, unchanged by the muzzle work; AC 6's tolerance is scoped to equal eye inputs. | AC 6 |
| P8 | `Tuning` Control lands on a frame with no local-pawn snapshot | Host live-edits `P_old,M_old → P_new,M_new`. Client frame F: the `Tuning` Control installs the new payload (placement → `P_new`, muzzle → `M_new`) but no local-pawn snapshot applies in F. Client fires in F. | Both terms read from the one payload, so the fire composes `P_new ∘ M_new` — never `P_new ∘ M_old`. This is why the client reads `muzzle_offset` from the payload accessor, not from a component the snapshot-gated `apply_net_wieldable_tuning` seam would refresh a frame later. | AC 6a |

## Script syntax examples

```ts
// Proposed design — a projectile weapon whose shots leave the barrel.
// muzzleOffset is model-local metres in the viewmodel's own frame — read it from the
// viewmodel's "muzzle" socket (Task 4). It composes through the weapon's placement.
defineEntity({
  name: "plasma_rifle",
  components: {
    weapon: {
      damage: 24,
      range: 2000,
      fireRateMs: 180,
      fireMode: "semi",
      resolution: "projectile",
      placement: downRight,               // authored viewmodel placement (weapon-placement)
      muzzleOffset: [0.0, 0.02, 0.58],    // barrel tip in the viewmodel's own frame
      viewmodel: "models/plasma/view.gltf",
      projectile: {
        speed: 90,
        radius: 0.15,
        lifetimeMs: 4000,
        visual: { body: { kind: "sprite", sprite: "sprites/plasma.png", emissive: 1.0 } },
      },
    },
  },
})

// Omit muzzleOffset to fire from the camera eye, unchanged.
```

## Decisions

No open design questions remain. Dispositions recorded so implementation does not
re-litigate them:

- **Observer third-person muzzle — deferred.** The authoritative origin honors what
  the *shooter* sees: the first-person muzzle, composed through authored placement and
  reproduced host-side. An *observer* watching a remote shooter sees the weapon posed
  by the avatar hand socket, which reads no placement (foreclosed). v1 spawns the
  remote presentation projectile at the authoritative FP-composed muzzle world point —
  presentation and validation share one *origin* (Invariant 2), the shot leaves the
  right neighborhood, and no new mechanism is needed. The observed *direction* is the
  reconstructed aim, not the shooter's convergence direction, so the presented
  trajectory does not visually terminate at the host-authoritative impact (Temporal
  pin P5) — an accepted, gameplay-irrelevant edge. A presentation muzzle anchored to
  the avatar's own weapon socket is a follow-up; it needs muzzle data on the avatar
  model, which does not exist today.
- **Live edits and the replication window — decided v1.** Placement and `muzzle_offset`
  are host-authoritative content, replicated in the same tuning-payload row; a connected
  client reads both from the one payload accessor and never from local content, so a live
  host edit reaches every peer atomically (Temporal pins P1–P3, P8). A shot fired in the
  brief window before a client installs a replacement `Tuning` composes an origin up to
  ~1 m off the host's; it still validates **while** `Δplacement + Δmuzzle ≤
  range * (HIT_RANGE_TOLERANCE − 1)` (Temporal pin P4) — a short-range projectile weapon
  crossing the window can drop a single max-range shot, a bounded self-correcting reject.
  The accepted predicted projectile is not re-anchored; it flies from its predicted origin.
  Acceptable and bounded, the same magnitude argument as convergence-range accounting.
- **Muzzle VFX / flash — decided shape, deferred to a follow-up spec.** The engine owns
  the muzzle *world point* (a fact); mods own the flash's *look* (a reaction / emitter /
  light anchored there); never a baked engine effect. Building it is a separate
  follow-up. The projectile *body* already leaves the barrel from this origin work.
- **Author read — kept, homed in the mount tooling.** Model-local authoring makes the
  extraction straightforward (the rigid `"muzzle"` socket's rest translation), and the
  landed `solve-weapon-mount` xtask / `mount.rs` is the socket-tooling home — so the read
  lives there as a new rigid-socket path (`mount.rs` today rejects `RigidRest`), not a
  forked tool. No socket→offset composition and no build-time bake in v1.
- **Convergence range accounting — decided v1.** Keep `remaining_range = range`
  measured from the muzzle; the ~1 m eye-vs-muzzle difference is negligible against
  typical ranges. Revisit only if a short-range projectile weapon exposes it.
- **Point-blank occlusion — decided v1.** Always spawn at the muzzle; the deterministic
  eye→muzzle occlusion guard (needs host-path collision access) is deferred. Acceptable
  barrel-origin edge, documented and tested; it must not break Invariant 2
  (origin == `fire_origin`).
