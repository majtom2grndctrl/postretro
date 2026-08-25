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

- The muzzle offset **value** must be replicated weapon content (descriptor →
  component), available to the tick on every peer without loading any mesh.
- The authoritative origin composes that offset against the **tick-rate aim
  pose** (eye + yaw/pitch basis), with **no** view-feel — deterministic and
  reproducible on the host.
- A glTF `"muzzle"` socket read at runtime cannot feed the authoritative origin:
  the host lacks remote viewmodels, so the value would differ per peer. The
  socket is an **author-time** source for the offset, never a runtime input.

The engine already parses `{"socket": "name"}` from glTF node `extras`
(`crates/model/src/gltf_extras.rs:86`) into `LoadedModel.sockets: HashMap<String, SocketBinding>`
where a rigid socket carries its composed rest transform in mesh-node local space
(`crates/model/src/gltf_loader.rs:70`, `:101`). `crates/model/examples/socket_dump.rs`
already loads a model and reads `model.sockets`. That is the reuse path for the
author tool.

## Single source of truth: one offset, two composition paths

- **Authoritative origin (deterministic, all peers):** eye + `aim_basis(yaw,pitch) * muzzleOffset`.
  No view-feel.
- **Presentation muzzle (client-local VFX):** the same `muzzleOffset` composed
  through the live swaying `viewmodel_world_transform`, so a muzzle-flash sticks
  to the visibly-bobbing barrel.

Same authored vector; the sim uses the steady pose, presentation uses the
swaying pose. The glTF socket only produces the vector.

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

This raycast is needed only on the **firing peer** (client, or listen-host firing
its own weapon) — the path that spawns the real gameplay projectile
(`resolve_client_fire`, which already has `collision_world`, `hit_zone_store`,
`anim_time`). The host's remote-shooter path needs `fire_origin` = muzzle for
validation and presentation, but does **not** re-simulate trajectory: projectile
hit authority is the client's declared contact validated by distance
(`mod.rs:2001`). So the remote path needs the muzzle origin, not the convergence
raycast.

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
