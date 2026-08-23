# Weapon Muzzle Origin

> **STATUS (superseded shape — do not implement as written).** This draft is now
> **downstream of a weapon-placement spec** and blocked on it. Design decisions
> that changed after this was written:
> - **Placement first.** The load-bearing capability is authorable weapon
>   placement (centered / down-right / on-top / dual-wield), owned by a reusable
>   **weapon-placement descriptor**. That is a separate spec; this one composes
>   onto it.
> - **Muzzle is model-local, not camera-relative.** The `muzzleOffset` primitive
>   below is authored in *camera/eye* space, which bakes placement in and breaks
>   under variable placement. The muzzle point is intrinsic model geometry (like a
>   hit zone) → author it in **model-local** space so it composes through *any*
>   placement unchanged. Fire origin = `eye ∘ placement ∘ muzzle_local`.
> - **Three origins, one authority.** The shot origin differs by vantage: the
>   shooter's first-person muzzle, the observer's third-person-avatar muzzle
>   (presentation only), and the authoritative origin — which **honors what the
>   shooter sees** (reproduced host-side from replicated placement, steady/no
>   view-feel; sway is cosmetic and excluded from authority).
>
> Everything below re-anchored cleanly to merged `main` (all fire-path / netcode
> anchors confirmed) *except* the coordinate-space and placement-composition
> shape. Revisit after the placement spec lands.

## Goal

Give weapon authors control over where a projectile originates. A per-weapon
muzzle offset moves the authoritative projectile spawn from the camera eye to the
barrel, aimed so the shot still converges on the crosshair at range. The visible
projectile then emerges from the gun instead of the screen center. Omitting the
offset reproduces today's eye-origin behavior exactly.

## Scope

### In scope
- A per-weapon `muzzleOffset` on the weapon descriptor: a weapon-local,
  camera-relative `[x, y, z]` vector. Threaded to the weapon component and
  effective stats. SDK types, validation, hover docs.
- A shared deterministic resolver that composes the muzzle world origin from the
  tick-rate aim pose (eye + yaw/pitch basis) and the offset — no view-feel.
- Projectile fire: spawn at the muzzle, aim at the camera-ray convergence point
  (nearest world/entity hit, else the far point at `range`), across the local
  fire path, the remote authoritative origin, and the remote presentation origin.
- An author-time tool that reads a `"muzzle"` socket from a weapon's viewmodel
  glTF and prints the camera-relative offset for pasting into `muzzleOffset`.
- Backward compatibility: absent/zero offset ⇒ eye origin and aim direction,
  identical to current behavior. Hitscan resolution is unchanged.

### Out of scope
- **Positioned muzzle flash / tracer for hitscan (and any engine-owned muzzle
  VFX).** Muzzle FX today is a mod reaction bound to the `"activate"` event;
  `muzzle_fx_visible` is a boolean predicted-shot gate with no position
  (`weapon/mod.rs:91`). There is no engine-side positioned flash to re-anchor.
  Emitting FX at the muzzle needs a new spawn-at-muzzle presentation seam — a
  separate feature. See Open Questions.
- **Runtime resolution of the glTF `"muzzle"` socket into the authoritative
  offset.** The viewmodel is client-local; the host lacks remote viewmodels, so a
  live socket read cannot feed the deterministic origin (research.md). The socket
  is an author-time source only.
- **Build-time bake** of the socket into the descriptor. Possible later; not v1.
- Moving the **hitscan** origin off the eye. Crosshair-perfect hitscan is kept.
- Per-weapon selection of convergence vs parallel-ray aiming. v1 is
  converge-to-crosshair for every projectile weapon.
- Augment/mount-point machinery from `research/weapon-model.md` §3. `muzzleOffset`
  is a flat precursor, not the slotted mount system.

## Direction

**Problem.** Projectiles are hard-coded to spawn at the camera eye
(`ProjectileLaunch.origin = aim_origin`, `weapon/mod.rs:585`; `remote_fire_origin`,
`commands.rs:274`). A weapon's visible barrel is therefore cosmetic — shots do not
come from it. Authors have no control over the origin.

**Placement.** Three axes decide where each piece lives. (1) engine-floor vs
mod-data: the muzzle *value* is per-weapon taste → mod/descriptor content, seeded
to zero; the *composition and convergence* are one right answer → engine floor,
one shared function. (2) deterministic-sim vs client-presentation: the
authoritative origin runs in the game-logic tick and is re-derived on the host
for remote shooters, so it composes against the tick-rate aim pose with no
view-feel; the VFX muzzle (out of scope here) is client-presentation. (3)
replicated-content vs client-local-mesh: the offset must be replicated content,
not read from the client-local viewmodel mesh — the host has no remote viewmodels
(research.md). These placements are the load-bearing decisions; a reviewer should
check them first.

**Prior commitments.** `research/weapon-model.md` §3 (Amendment 2026-07) names a
`MountPoint` union with `muzzle` and "one modifier system, not two." This spec
authors a flat `muzzleOffset` on the weapon block rather than a slotted mount.
Divergence is intentional and argued: v1 has no augment/roll machinery, weapon
stats are a flat block on `defineEntity`, and `muzzleOffset` is exactly the
effective muzzle a future mount/augment layer would resolve into — additive, not a
competing system. The netcode contract that `AuthorizedShot.fire_origin` measures
projectile travel and gates LOS (`netcode/mod.rs:2001`, `:2063`) is preserved by
keeping the spawned origin and `fire_origin` coincident at the muzzle. E21 (bone
sockets) and the sibling draft `weapon-mount-frame-solver` deliberately foreclose
per-attachment offset/rotation *in data* — "art fixes placement in the prop, not
in data" — for mesh **render placement**. `muzzleOffset` is not that case: it is a
**simulation projectile-spawn origin**, replicated content the deterministic tick
must read on every peer, which a mesh transform cannot supply (the host has no
remote viewmodel). The surface resonance (muzzle + offset + socket) is
coincidental; the domains do not overlap.

**Alternatives rejected.** (1) Keep the authoritative origin at the eye and make
the barrel purely a presentation offset (visually lerp the projectile from barrel
to true path). Rejected: the owner chose muzzle-authoritative; a cosmetic lerp
adds sim/visual divergence that the networked contact validation would have to
paper over, and it is more code than moving the origin. (2) Read the muzzle live
from the viewmodel socket at fire time. Rejected: the viewmodel is client-local,
so the host would compute a different origin for remote shooters, breaking
determinism and hit validation.

## Acceptance criteria

- [ ] A weapon descriptor with `muzzleOffset` omitted spawns its projectile at the
      camera eye with the aim direction — byte-identical launch to current behavior
      (regression pin).
- [ ] A weapon descriptor with a nonzero `muzzleOffset` spawns its projectile with
      `Transform.position` at the composed muzzle world point, not the eye.
- [ ] The projectile's launch direction points from the muzzle toward the point the
      camera aim ray hits (nearest world-or-entity hit within `range`); on a clean
      miss it points from the muzzle toward `eye + aim_dir * range`. At long range
      the flight path passes through the crosshair target.
- [ ] The muzzle world origin computed on the firing peer and the origin the host
      derives for that same shot (same eye, aim direction, offset) are equal within
      float tolerance.
- [ ] For a projectile shot, the spawned projectile `Transform.position` equals the
      `AuthorizedShot.fire_origin` used for host validation. A projectile hit
      declaration whose contact is within `range * HIT_RANGE_TOLERANCE` of the
      muzzle origin still validates.
- [ ] When the convergence point is at or behind the muzzle (a surface within the
      barrel's forward reach), the launch keeps the aim direction and still spawns
      at the muzzle — no degenerate zero-length direction, and the origin stays
      host-reproducible.
- [ ] Aiming straight up and straight down composes the offset through the pitched
      aim basis (the muzzle tracks the barrel's pitch, not a fixed world offset).
- [ ] `muzzleOffset` rejects non-finite components at descriptor validation with a
      field-named error, consistent with other weapon-field validation.
- [ ] `sdk/types/postretro.d.ts` and `.d.luau` declare `muzzleOffset` with hover
      docs (units, axis convention, default); the SDK drift test passes.
- [ ] The author tool prints a weapon viewmodel's `"muzzle"` socket as a
      camera-relative `[x, y, z]` offset, and errors clearly when the socket is
      absent.

## Tasks

### Task 1: Muzzle offset content + deterministic resolver + local converge (thin slice)

Add `muzzle_offset: [f32; 3]` to `WeaponDescriptor`
(`crates/foundation/src/data_descriptors/types/combat.rs`), `#[serde(default)]`
defaulting to `[0.0, 0.0, 0.0]`; under the struct's existing
`#[serde(rename_all = "camelCase")]` it wires as `"muzzleOffset"`. Validate each
component finite in `WeaponDescriptor::validate` with a `components.weapon.muzzleOffset`
field-named `DescriptorError::InvalidShape`, matching the sibling numeric checks.
Carry it onto `WeaponComponent` as `muzzle_offset: glam::Vec3`
(`crates/entities/src/components/weapon.rs`): set it in `from_descriptor_with_canonical`,
update it in `refresh_from_descriptor` (authored tuning, so refresh overwrites —
it is not live instance state), and surface it on `EffectiveStats` and
`effective()`. The vector is weapon-local, camera-relative: `+X` right, `+Y` up,
`−Z` forward (the `Camera::aim_ray`/`viewmodel_camera_space_transform` sign
convention). Add a shared engine-floor helper — `muzzle_world_origin(eye: Vec3,
aim_direction: Vec3, offset: Vec3) -> Vec3` — that builds a right-handed basis
from the aim direction and world up (`forward = aim_direction`,
`right = normalize(forward × Vec3::Y)`, `up = right × forward`) and returns
`eye + right * offset.x + up * offset.y + forward * (−offset.z)`. Taking the aim
direction (not yaw/pitch) means both the local and remote paths feed the helper
the same vector they already hold, so the origin is computed one way on every peer
(Invariant 1); the basis never degenerates because `Camera::aim_ray` clamps pitch
short of vertical (`camera.rs:130`). Place it where both the local weapon tick and
the remote command path can call it (e.g. a `crate::weapon` free function). In the
`ResolutionMode::Projectile` arm of `resolve_client_fire`
(`crates/postretro/src/weapon/mod.rs`, ~`:581`), replace `origin: aim_origin,
direction: aim_direction` with: compute `muzzle = muzzle_world_origin(aim_origin,
aim_direction, effective.muzzle_offset)`; find the convergence point by the
nearest-hit logic the hitscan path already resolves (`cast_ray` against
`collision_world` plus `nearest_entity_hit` over `hit_zone_store`/`anim_time`,
both already in this tick), taking the nearer hit's point or `aim_origin +
aim_direction * range` on a miss; set `direction = (convergence - muzzle)`
normalized. The launch is **always** at the muzzle — there is no eye fallback, so
the origin is reproducible host-side from the same inputs (Invariant 2); guard
only the degenerate normalize (Invariant 4): when `(convergence - muzzle)` is
below an epsilon length or points opposite `aim_direction` (convergence at or
behind the muzzle, i.e. a surface inside the barrel's reach), keep
`direction = aim_direction` while still spawning at the muzzle. When
`muzzle_offset` is zero the muzzle equals the eye and the direction collapses to
`aim_direction`, so the launch is byte-identical to today (Invariant 3).
`remaining_range` stays `range`. Cover this task with unit tests for: omitted
offset ⇒ eye-origin launch (AC 1); nonzero offset ⇒ muzzle origin (AC 2);
convergence direction aims at the hit point and at the far point on miss (AC 3);
degenerate-convergence guard keeps aim direction and muzzle origin (AC 6);
pitched-aim composition up and down (AC 7); non-finite rejection (AC 8).

### Task 2: Remote authoritative + presentation muzzle origin

In the remote weapon-command path
(`crates/postretro/src/sim/weapon_stage/commands.rs`), route the remote
authoritative projectile origin and the presentation launch origin through the
muzzle. The remote path already reconstructs the shot's `yaw`/`pitch` and the
pawn eye (`transform.position + Vec3::Y * movement.capsule.eye_height`,
`remote_fire_origin` at `:274`; direction rebuilt from yaw/pitch at `:160`). For a
projectile shot, compute the muzzle with the same shared `muzzle_world_origin(eye,
aim_direction, weapon.muzzle_offset)` helper from Task 1, passing the direction
this path already reconstructs — the weapon component is spawned host-side for the
remote player, so `muzzle_offset` is available without any viewmodel mesh. Set `AuthorizedShot.fire_origin` (`netcode` mod, field at
`mod.rs:498`) to that muzzle point and set `RemoteProjectilePresentationLaunch.origin`
(`commands.rs:177`) to the same point, so the presentation visual and the
validation sphere share the muzzle (Invariant 2). The remote path does not
re-simulate trajectory — projectile hit authority is the client's declared
contact validated by distance (`valid_projectile_contact_point`, `mod.rs:2001`) —
so it needs no convergence raycast; the presentation `direction` stays the
reconstructed aim direction. Hitscan shots keep `fire_origin: Vec3::ZERO` and the
present-eye path unchanged. Add a test asserting the remote muzzle origin equals
the firing peer's for identical (eye, yaw, pitch, offset) inputs (AC 4), and that
a projectile contact within `range * HIT_RANGE_TOLERANCE` of the muzzle validates
(AC 5).

### Task 3: SDK types and hover docs

Regenerate the SDK type surfaces so `muzzleOffset` appears on the weapon block in
`sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` with hover
documentation: a weapon-local camera-relative `[x, y, z]` offset in metres
(`+X` right, `+Y` up, `−Z` forward), default `[0, 0, 0]` meaning the camera eye,
moving the projectile spawn to the barrel while the shot still converges on the
crosshair. The descriptor field's doc comment is the generator source
(`gen-script-types`, `cargo run -p postretro --bin gen-script-types`); the debug
build also emits these files at startup. Confirm the committed `.d.ts`/`.d.luau`
match the registry so the drift test in `cargo test` passes (AC 9). This task
consumes the descriptor field from Task 1.

### Task 4: Author tool — muzzle socket → offset

Provide an author-time command that reads a weapon viewmodel glTF and prints its
`"muzzle"` socket as a camera-relative `[x, y, z]` offset ready to paste into
`muzzleOffset`. Extend the existing `crates/model/examples/socket_dump.rs`
pattern (it already `load_model`s and reads `model.sockets: HashMap<String,
SocketBinding>`, `gltf_loader.rs:101`): for a rigid socket, take its composed
rest transform's translation in mesh-node local space; compose it with the
viewmodel's fixed camera-space placement (`BASE_OFFSET` and the base rotation
from `viewmodel_camera_space_transform`, view-feel zeroed) so the printed vector
is in the same camera-relative space the sim composes against. Error clearly when
the model has no `"muzzle"` socket or it is skinned rather than rigid (AC 10).
This is tooling only — no runtime code reads the socket. Keep `BASE_OFFSET` and
the placement math sourced from one place if it is factored out of `main.rs`;
otherwise document the coupling in the tool. **Coordination:** the sibling draft
`weapon-mount-frame-solver` (same owner) promotes `socket_dump.rs` into a shared
`crates/model/src/mount.rs` and a `solve-weapon-mount` xtask. Do not fork a second
socket-reading tool: if that draft lands first, add this muzzle read to its
tooling home; if this lands first, keep the read in one place the mount work can
absorb. The owner sequences the two — see Open Questions.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice through content → resolver → local
fire → tests. Falsifies the boundary assumptions: offset composition basis,
convergence math, point-blank guard, and backward compatibility. Everything else
builds on the `muzzle_offset` field and the shared resolver it introduces.

**Phase 2 (concurrent):** Task 2, Task 3, Task 4 — independent. Task 2 reuses the
Task 1 resolver on the remote path; Task 3 regenerates SDK types from the Task 1
descriptor doc comment; Task 4 is standalone tooling. None share files.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| `muzzleOffset` | `WeaponDescriptor.muzzle_offset: [f32; 3]` (default `[0.0; 3]`); `WeaponComponent.muzzle_offset: glam::Vec3` | `"muzzleOffset"` | `muzzleOffset?: [number, number, number]` | `muzzleOffset` | n/a |

Axis convention (every surface): weapon-local, camera-relative, metres. `+X`
right, `+Y` up, `−Z` forward. Default `[0, 0, 0]` = camera eye.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| 1. Authoritative muzzle origin is computed by one shared function on every peer, so the firing peer and the host agree for identical (eye, aim_direction, offset). | Task 1 (`muzzle_world_origin`, basis built from the aim direction both paths already hold) | Threatened if the remote path (Task 2) builds the aim basis differently, or feeds a direction that differs from the firing peer's. | AC 4 |
| 2. The spawned projectile origin and `AuthorizedShot.fire_origin` are the same muzzle point, unconditionally (no firing-peer-only fallback that the host cannot reproduce). | Task 1 (local spawn always at muzzle), Task 2 (remote `fire_origin` + presentation origin) | Threatened if one path applies the offset and another does not, or if a peer conditionally moves the origin off the muzzle; a divergence shifts the `valid_projectile_contact_point` validation sphere off the real trajectory (`netcode/mod.rs:2001`). | AC 5 |
| 3. Omitting `muzzleOffset` reproduces the current eye-origin, aim-direction launch exactly. | Task 1 (default `[0,0,0]`; zero offset ⇒ muzzle == eye and convergence collapses to aim) | Threatened if the default is nonzero, or the convergence path perturbs direction when offset is zero. | AC 1 |
| 4. The launch direction is never a degenerate normalize. | Task 1 (guard on `(convergence − muzzle)`: epsilon length or opposite `aim_direction` ⇒ keep aim direction, still spawn at muzzle) | Threatened when the convergence point is at or behind the muzzle (a surface within the barrel's reach). Guarding by keeping aim direction, not by moving the origin, so Invariant 2 stays intact. | AC 6 |

## Script syntax examples

```ts
// Proposed design — a projectile weapon whose shots leave the barrel.
// muzzleOffset is weapon-local, camera-relative metres: +X right, +Y up, -Z forward.
defineEntity({
  name: "plasma_rifle",
  components: {
    weapon: {
      damage: 24,
      range: 2000,
      fireRateMs: 180,
      fireMode: "semi",
      resolution: "projectile",
      muzzleOffset: [0.3, -0.24, -1.1], // barrel tip: right, down, ~1.1m forward
      projectile: {
        speed: 90,
        radius: 0.15,
        lifetimeMs: 4000,
        visual: { body: { kind: "sprite", sprite: "sprites/plasma.png", emissive: 1.0 } },
      },
    },
  },
})

// Omit muzzleOffset (or set [0, 0, 0]) to fire from the camera eye, unchanged.
```

## Open questions

- **Muzzle VFX seam (natural follow-up).** Making a muzzle flash / tracer emit
  from the barrel — for hitscan *and* projectile — needs a spawn-at-muzzle
  presentation path, since muzzle FX today is a mod reaction on `"activate"` with
  no position. The client-local swaying muzzle (the same `muzzleOffset` composed
  through `viewmodel_world_transform`) is the anchor. Decide whether that is an
  engine-owned effect or a new primitive that hands the muzzle world point to a
  mod-authored emitter/light. Out of scope here; the projectile-origin work
  already makes the projectile *body* leave the barrel.
- **Author ergonomics: tool vs build-time bake.** Task 4 ships a print-and-paste
  tool. A build-time bake (mod/level build reads the viewmodel glTF and writes
  `muzzleOffset` into the descriptor) would make it automatic but adds a build
  dependency on viewmodel assets. Worth it once several projectile weapons exist;
  not v1.
- **Convergence range accounting.** The projectile keeps `remaining_range = range`
  measured from the muzzle, while the convergence raycast measures `range` from
  the eye. The muzzle-to-eye distance (~1 m) makes these differ by that much.
  Left as-is for v1 (negligible against typical ranges); revisit if a
  short-range projectile weapon exposes it.
- **Point-blank occlusion.** v1 always spawns at the muzzle. If the muzzle sits
  past a near surface (barrel jammed into a wall, aiming into it), the projectile
  originates inside/beyond that surface and may not contact it. A deterministic
  fix (a short eye→muzzle occlusion ray that clamps the origin, run identically on
  both peers) needs collision access in the host remote path; deferred so the
  fallback cannot silently break Invariant 2. Acceptable v1 edge for a barrel-
  origin weapon.
- **Author-tool home and sequencing vs `weapon-mount-frame-solver`.** Both drafts
  touch `socket_dump.rs`. Decide the order and the shared tooling home
  (`crates/model/src/mount.rs` / `solve-weapon-mount`) before either promotes, so
  the muzzle read lands there rather than as a parallel tool. Owner decision.
