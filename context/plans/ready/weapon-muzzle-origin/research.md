# Weapon Muzzle Origin — Research

Investigation notes. Not the spec. Decisions live in `index.md`.

## The question

Do projectiles fire from the barrel of the wielded weapon, or from the camera?
Answer from source: **hard-coded to the camera eye**. There is no
barrel/muzzle-origin concept anywhere in the simulation.

- Local fire builds `ProjectileLaunch { origin: aim_origin, direction: aim_direction, .. }`
  in the `ResolutionMode::Projectile` arm of `resolve_client_fire`
  (`crates/postretro/src/weapon/mod.rs`, launch struct defined ~`:255`, built ~`:585`).
  `aim_origin`/`aim_direction` come from `Camera::aim_ray()`, which returns
  `(self.position, direction)` — literally the camera eye
  (`crates/postretro/src/camera.rs:144`).
- Remote authoritative origin = `transform.position + Vec3::Y * movement.capsule.eye_height`
  (`remote_fire_origin`, `crates/postretro/src/sim/weapon_stage/commands.rs:274`), used
  for `AuthorizedShot.fire_origin` and the presentation launch origin (`commands.rs:177`).
- `spawn_projectile` consumes `launch.origin` as the projectile `Transform.position`
  (`commands.rs:473`).

## Why "read the visible barrel" does not work

The first-person weapon (viewmodel) is a **screen-space overlay**, not a world
object:

- Its transform is `view_matrix.inverse() * viewmodel_camera_space_transform(..)`
  (`crates/postretro/src/main.rs:1387`), where the camera-space transform is a
  fixed `BASE_OFFSET = (0.32, -0.28, -0.62)` plus view-feel bob/sway/roll
  (`viewmodel_camera_space_transform`, `main.rs:1364`).
- It renders in a **dedicated pass with its own tight projection**:
  `viewmodel_projection(aspect)` using `VIEWMODEL_HFOV_RADIANS` /
  `VIEWMODEL_NEAR_CLIP` / `VIEWMODEL_FAR_CLIP`
  (`crates/renderer/src/render/renderer_frame.rs:14`;
  `write_viewmodel_view_projection`, `mesh_pass.rs:1794`).

Because the viewmodel uses a different projection than the world camera, its
rendered barrel tip does **not** correspond to a single world-space point under
the world camera. A projectile spawned at the socket's world position will not
line up pixel-perfectly with the drawn muzzle. In practice this is invisible —
the projectile leaves the barrel immediately and converges toward the crosshair
— but "the shot exactly touches the drawn muzzle" is not achievable and is not
a goal.

## Why the authoritative origin must be replicated content, not a live mesh read

The viewmodel is a **client-local presentation asset**. It is resolved only on
the local follow path — `local_viewmodel_asset` → `viewmodel_asset_for_archetype`
reads the weapon archetype's `viewmodel` and is called from the render seam for
the local pawn only (`main.rs:1339`, `:3760`). The host never loads a remote
player's viewmodel geometry.

The authoritative fire origin runs in the deterministic game-logic tick and is
re-derived on the host for remote shooters (`remote_fire_origin`). View-feel
bob/sway is computed at render rate, client-local (`vf_eye_offset`, `vf_roll` in
`main.rs`), and never reaches the tick. Therefore:

- The muzzle **value** must be replicated weapon content (descriptor →
  component), available to the tick on every peer without loading any mesh. It is
  authored **model-local** (the viewmodel mesh's own frame), like a hit zone.
- The authoritative origin composes that model-local point through the resolved
  **placement** and the **tick-rate aim pose**: `eye ∘ placement ∘ muzzle_local`,
  with **no** view-feel — deterministic and reproducible on the host.
- A glTF `"muzzle"` socket read at runtime cannot feed the authoritative origin:
  the host lacks remote viewmodels, so the value would differ per peer. The
  socket is an **author-time** source for the model-local point, never a runtime
  input.

The engine already parses `{"socket": "name"}` from glTF node `extras`
(`crates/model/src/gltf_extras.rs`) into `LoadedModel.sockets: HashMap<String, SocketBinding>`
where a rigid socket carries its composed rest transform in mesh-node local space
(`crates/model/src/gltf_loader.rs`). The rigid `"muzzle"` socket's translation
(`SocketBinding::RigidRest(Mat4).w_axis.truncate()`) is the model-local point the
author read reports (Task 4).

## Composition: model-local muzzle through placement

`weapon-placement` (done) resolves the viewmodel placement `mod default < per-weapon`
via `resolve_weapon_placement` (`main.rs`, pure — borrowed descriptors, no I/O) and
exposes the resolved **steady** value as `placement_offset` / `placement_rot` at the
render seam (`viewmodel_camera_space_transform`, before the `sway_rot` multiply). The
host replicates the effective placement in the opaque tuning payload
(`tuning_payload_for_pawn`, `netcode/mod.rs`).

- **Authoritative origin (deterministic, all peers):** `eye ∘ placement ∘ muzzle_local`,
  reading the steady placement — no view-feel. Both the placement and `muzzle_offset`
  are **host-authoritative**: a connected client reads them from the replicated tuning
  payload (the render seam's client branch already reads placement via
  `placement_for_slot` / `placement_for_archetype`), while the host resolves placement
  from its own descriptors (`resolve_weapon_placement`) for its own pawn and for remote
  shooters. One shared helper composes both, so every peer agrees.
- **Presentation muzzle (client-local VFX, out of scope):** the same `muzzle_local`
  composed through the live swaying viewmodel transform, so a muzzle-flash sticks to
  the visibly-bobbing barrel.

Because the muzzle is model-local, it composes through *any* placement unchanged: a
placement edit moves the muzzle with the barrel, and dual-wield can share one authored
value. A camera-relative offset would bake the placement in.

Placement is **not** carried on `WeaponComponent`/`EffectiveStats` (the per-instance
tier is `None`); the fire paths source it as the render seam does — the replicated payload
accessor on a client, local resolve on the host. `muzzle_offset` rides the tuning payload
beside placement, so the connected-client fire path reads it from the same payload
accessor (`muzzle_for_slot`) — atomic with placement, avoiding the split-seam a
component-sync would open. The component keeps `muzzle_offset` (from the descriptor) for
the host/single-player authoritative and remote paths, read via `effective()`.

## `fire_origin` is load-bearing for anti-corruption validation

The host validates a projectile hit declaration against `fire_origin`, not the
present eye:

- `valid_projectile_contact_point` accepts the declared contact only when
  `shot.fire_origin.distance(point) <= shot.range * HIT_RANGE_TOLERANCE`
  (`crates/postretro/src/netcode/mod.rs:2001`).
- Projectile LOS uses `fire_origin`, deliberately skipping present-eye world LOS
  (`mod.rs:2063`; test `projectile_declaration_uses_fire_origin_and_skips_late_world_los`,
  `mod.rs:3999`).

Consequence: moving `fire_origin` to the muzzle stays valid **only if** the
client's projectile also spawns at the muzzle. Divergent origins would shift the
validation sphere off the real trajectory. This is the "spawn origin ==
fire_origin" invariant.

## Convergence to the crosshair

Chosen fire semantics (owner decision): spawn at the muzzle, aim at the point
the camera ray hits, so the shot leaves the visible barrel yet still strikes the
crosshair at range. Honest close-range parallax (desirable for arcs).

The convergence target is a ray from the **eye** along the aim direction. The
reuse is `cast_ray(collision_world, Point, Vector, range) -> Option<RayHit>`
(`crate::collision::cast_ray`, used by `resolve_client_hitscan` at
`weapon/mod.rs:746`; hit point = `origin + direction * time_of_impact`), plus the
nearest-entity hit the hitscan path already resolves (`nearest_entity_hit`), so
convergence can target an enemy body, not only world geometry. Miss → far point
`eye + aim_dir * range`.

This raycast is needed only on the **firing peer** — the two local paths that spawn
the real gameplay projectile, both already holding `collision_world` /
`hit_zone_store` / `anim_time`: the client-prediction `resolve_client_fire` and the
host/single-player authoritative `run_local_weapon_command` → `fire_hitscan`
projectile arm. The host's remote-shooter path (`run_remote_weapon_commands`) needs
`fire_origin` = muzzle for validation and presentation, but does **not** re-simulate
trajectory: projectile hit authority is the client's declared contact validated by
distance (`netcode/mod.rs`). So the remote path needs the muzzle origin, not the
convergence raycast.

## Where muzzle FX lives today

`muzzle_fx_visible` is a **predicted-shot boolean** that rolls back on host
reject (`weapon/mod.rs:91`, `:186`); it carries no position. It gates the
`"activate"` script event (`main.rs:6601`). Positioned muzzle flash / tracer is
whatever a mod binds to `activate` — there is **no engine-side positioned
muzzle-flash render** to re-anchor. Delivering "hitscan muzzle flash from the
barrel" therefore requires a new spawn-at-muzzle presentation seam, not a
re-point of an existing effect. That is why it is out of scope here (see
`index.md`).

## Prior design intent

`context/research/weapon-model.md` §3 (Amendment 2026-07) already names a
`MountPoint` union with `muzzle` and an augment `mount: { point, mesh }`, under
"one modifier system, not two." A per-weapon `muzzleOffset` is the thin
precursor of that muzzle mount point: the effective muzzle an augment system
would later feed. Weapon stats today are a flat block on `defineEntity`
(no `defineWeapon`, no `initialStats` wrapper), so a flat `muzzleOffset` sits
beside `damage`/`range`.
