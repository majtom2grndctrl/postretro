# Weapon Placement

## Goal

Make the first-person weapon's placement authorable instead of a hardcoded engine
constant. A reusable, named weapon-placement descriptor lets authors position each
weapon in view as a continuous transform — centered, down-and-right, a heavy gun
riding lower, a launcher on top — defined once and reused, overridable per weapon
and (designed-for, built later) per instance for dual-wield. Placement is authored
content available on every peer, so it also grounds the later muzzle/fire-origin
work. An unauthored weapon renders exactly as today.

## Scope

### In scope
- A standalone **weapon-placement descriptor**: a continuous placement transform
  (position offset + rotation) in the viewmodel's camera-relative frame, authored
  once via `defineWeaponPlacement` and referenced by handle (its data inlines where
  used — no wire registry).
- A **layered override** resolution, four-tier `mod default < character default <
  per-weapon override < per-instance override`. Resolution is whole-value fallback;
  partial overrides ("same as the default but lower") are authored via object spread,
  not an engine field-merge. v1 **builds** `mod default < per-weapon`; the character and
  per-instance tiers are designed into the resolver signature but absent (`None`)
  in v1 — see Out of scope.
- Replacing the hardcoded `BASE_OFFSET` at the viewmodel render seam with the
  resolved placement; default placement equals today's constant (regression pin).
- Placement carried as **weapon-archetype content**. The host sends each occupied
  wieldable's resolved effective placement in the existing opaque tuning payload; no
  fixed transport wire field is added. The later authoritative fire origin reads the
  same host-owned value.
- SDK types, descriptor validation (finite transform components), hover docs.

### Out of scope
- **Moving the projectile/fire origin.** Projectiles still fire from the camera
  eye. The muzzle composing onto authored placement is the reframed
  `weapon-muzzle-origin` spec (downstream), not this one. That spec owns the sampling
  contract the content decision assumes: the host reproduces the authoritative origin
  from the *steady* (no-sway) placement at tick rate, never a render-rate swayed
  value; render-rate view-feel sway stays cosmetic and excluded from authority. This
  spec keeps the resolved placement available as a standalone pre-sway value at the
  render seam (`placement_offset` / `placement_rot` before the `sway_rot` multiply),
  so the downstream spec reads it without recomputing.
- **The character and per-instance override tiers.** The resolver's precedence
  chain includes a **character default** (home: the player `EntityTypeDescriptor`;
  lands when more than one player type/class exists) and a **per-instance override**
  (home: `WeaponComponent`; lands with dual-wield). v1 passes both as hardcoded `None`
  and builds only the `mod default < per-weapon` tiers — they do no work in v1 because
  the parameters are `None`, and v1 adds no field to either home. This keeps the chain
  from being painted into a corner without building tiers that do nothing yet.
- **Dual-wield end to end.** Beyond the per-instance seam above, the
  second-active-wieldable machinery (two active slots, switch input, per-hand
  rendering) lands with the inventory/wieldable work
  (`context/research/weapon-model.md §8`). This spec builds and proves single-mount
  placement.
- **The third-person weapon mount.** Placement is **first-person only**. The
  avatar's weapon mounts at a bare hand socket (`hand_r`) with no per-attachment
  offset — a deliberate, doubly-foreclosed design (`plans/done/E21--bone-sockets-attachments`
  and the `weapon-mount-frame-solver` draft: "art fixes placement in the prop or
  the socket joint, not in data"). This spec does not add offset data to the TP
  mount; observers keep seeing the weapon posed by the socket, unchanged. FP
  placement and the TP mount legitimately diverge — which is consistent with the
  vantage split (shooter's view is authored FP placement; observers' view is the
  avatar socket).
- **View-feel (sway/bob/tilt).** That is the render-only integrator overlay owned
  by movement/`player-descriptor-composition`. Placement is the base it rides on;
  the two compose but are separate axes.
- **Player-preference placement** (a per-player FOV-like setting). Placement here
  is authored weapon/mod content, not a runtime user option.
- **Scale** as a placement axis. Per-weapon viewmodel size is a modeling concern —
  a chunkier gun is modeled chunkier ("art fixes it in the prop," as for the TP
  mount). Position and rotation earn a place in data because they are per-game
  screen presentation (the same gun sits centered in one game, down-right in
  another); per-weapon scale is not. A *global* viewmodel scale (every gun bigger on
  screen) would be a separate view-setting axis, not per-weapon placement. Placement
  is position + rotation only.

## Direction

**Problem.** The first-person viewmodel is pinned to a hardcoded constant
(`BASE_OFFSET` in `viewmodel_camera_space_transform`, `main.rs:1371`). Every weapon
in every game sits identically; none of centered / down-right / low-heavy / on-top
/ dual is expressible. Observation: designing weapon fire origin surfaced that the
engine has no concept of where a weapon sits, and the placements the project wants
(Doom-Eternal on-top launcher, Killzone wide miniguns, a pistol higher than a
heavy MG) all need a continuous, per-weapon, reusable placement.

**Placement.** Three axes. (1) engine-floor vs mod-data: *where* a weapon sits is
per-game/per-weapon taste → mod-data (a descriptor, seeded to today's default);
the *composition* into the view transform is engine floor. (2) content vs
client-local: placement is authored content on the weapon archetype (replicated as
host-resolved tuning), not a client-local render tweak — required so the host can reproduce
the shooter's authoritative fire origin (the later muzzle spec). The third-person
mount does *not* read placement (it is socket-posed; Invariants, #4); the
content-ness rests on authority alone. (3) placement vs
view-feel: placement is the base position (content); view-feel sway is the
render-only overlay on top (owned elsewhere). These placements are the load-bearing
decisions.

**Prior commitments.** The DRY reference + layered fallback reuses *shipped*
idioms, not the unbuilt `player-descriptor-composition` `base`+`layers` merge
(that draft is direction, not contract): `defineWeaponPlacement` follows the
**pure identity/type helper** pattern of `defineMapCatalog` / `defineTriggerPool`
(each `return`s its argument unchanged — typed data that inlines where referenced,
no wire change, no registration; `scripting.md §2` Map catalog), so the handle *is*
the reference and no name-string lookup or Rust-side merge exists (sparse override is
JS object spread — `{ ...base, positionFromCenter }`). The `mod default <
per-weapon` precedence mirrors the shipped `switching: SwitchingDescriptor`
mod-global rule that per-weapon `blockDuringReload` already overrides
(`combat.rs` weapon block; `ModManifestResult.switching`). Placement
is a **sibling axis** to `player-descriptor-composition`'s reserved *viewFeel*
overlay (which is render-only feel, not base placement), deliberately not a
character-only field — authoring only at the character level would foreclose
per-weapon placement, the corner to avoid. The third-person mount's
**offset-in-data is doubly foreclosed** (`plans/done/E21--bone-sockets-attachments`
and the `weapon-mount-frame-solver` draft); this spec honors that by keeping
placement FP-only. `research/weapon-model.md` frames weapons as a flat block on
`defineEntity` and marks dual-wield/second-active-slot unbuilt (§8), so the
per-instance tier is designed-for but not built here. The `weapon-muzzle-origin`
draft is reframed as downstream: it composes the model-local muzzle onto this
spec's authored placement.

**Alternatives rejected.** (1) A discrete placement enum (centered / down-right /
…). Rejected: a pistol and a heavy MG differ by continuous amounts on every axis;
an enum cannot express "ride 4cm lower," and named presets are better modeled as
resolving *to* the continuous transform. (2) Placement as a client-local render
setting (like FOV). Rejected: the host must reproduce the shooter's authoritative
fire origin from placement (the downstream muzzle spec), so it must be shared
content, not a client-local tweak. (3) Folding placement into
`player-descriptor-composition` as a character-level field. Rejected: it forecloses
per-weapon overrides — the explicit requirement. (4) **Bake held-placement into the
viewmodel model** — "art fixes placement in the prop," the exact answer E21 and
`weapon-mount-frame-solver` apply to the *third-person* mount. Rejected for the
*first-person* side: a model baked to one placement stops being portable across
games (a different game wants the same gun centered vs down-right), and per-instance
dual-wield and DRY reuse cannot live in a shared mesh. The FP-data / TP-model split
is deliberate — the FP viewmodel is a per-game screen-space presentation, so its
placement is authored data; the TP prop is posed by the avatar skeleton, so its
placement is fixed in the prop.

## Acceptance criteria

- [ ] A weapon with no placement at any tier (no `defaultWeaponPlacement`, no
      per-weapon `placement`) composes its viewmodel transform to exactly today's
      `BASE_OFFSET` composition — byte-identical, asserted as transform-equality with
      no mod default present.
- [ ] A weapon with an authored placement renders its viewmodel at that position and
      rotation; two weapons with different placements resolve to different viewmodel
      transforms (asserted at the transform level, or observed across a weapon switch —
      v1 shows one viewmodel at a time).
- [ ] Placement is a continuous transform: two placements differing only in
      `positionFromCenter.up` render the viewmodel at correspondingly different
      heights (a pistol higher than a heavy MG), with no discrete snapping.
- [ ] The built tiers resolve `mod default < per-weapon` as whole-value fallback: a
      weapon with `placement` uses it; a weapon without uses the mod default; with
      neither present it falls back to `BASE_OFFSET`. (Sparse override is authored in
      script via object spread, not an engine merge; the character and per-instance
      tiers are designed into the resolver signature but absent — `None` — in v1.)
- [ ] Two weapons referencing the same placement handle resolve to identical
      placements (DRY: one authored source, one resolved result).
- [ ] The resolved placement is present and identical on host and on every client
      for a given weapon archetype, with no fixed transport wire field. The host sends
      the effective placement on initial participation and replaces it after live
      per-weapon or mod-default edits. Connected clients never fall back to local
      placement content. (Verified by tuning-payload resolution, codec, and reload
      change-detection tests plus a review/grep gate that no fixed wire type changes.)
- [ ] A shooter's authored FP placement does **not** move its third-person avatar
      mount: observers see the weapon posed by the avatar hand socket exactly as
      today, whatever the shooter's FP placement (placement is FP-only). (Review gate:
      the third-person mount function never takes the placement param; the test asserts
      the `hand_r` transform is independent of placement, proving no wiring.)
- [ ] Invalid placement is rejected with a field-named error, not silently degraded to
      `BASE_OFFSET`: a non-finite `positionFromCenter` or `rotation` component at
      descriptor `validate()`, and a non-object placement value (a function/symbol the
      VM→JSON bridge coerces to null) at the runtime pre-check.
- [ ] `sdk/types/postretro.d.ts` and `.d.luau` declare the placement descriptor and
      the weapon-block reference with hover docs (units, axis convention, default);
      the SDK drift test passes. (The drift test pins the generated name and shape —
      runnable; the hover-doc prose is a review gate, asserted by no test.)

## Script syntax examples

```ts
// Proposed design. defineWeaponPlacement is a pure SDK identity/type helper
// (like defineMapCatalog / defineTriggerPool): no FFI, no registration — it returns
// a typed WeaponPlacementDescriptor. Reference it by handle; data inlines where used.
// Sparse override is object spread. Layered fallback = the shipped switching
// "mod default < per-weapon override" precedence.
//
// positionFromCenter: metres from screen center (the crosshair / eye) —
//   right / up / forward (forward = toward the aim).
// rotation: degrees — yaw / pitch / roll. Internally forward maps to -Z and
//   rotation converts to a quaternion.

// Define once, reference by handle (DRY):
const downRight = defineWeaponPlacement({
  positionFromCenter: { right: 0.32, up: -0.28, forward: 0.62 },  // == today's BASE_OFFSET
  rotation:           { yaw: 0, pitch: 0, roll: 0 },
})

const heavyLow = defineWeaponPlacement({
  positionFromCenter: { right: 0.30, up: -0.40, forward: 0.70 },  // rides lower, a touch further out
})

defineEntity({
  name: "pistol",
  components: {
    weapon: {
      damage: 12, range: 1000, fireRateMs: 180, fireMode: "semi", resolution: "hitscan",
      placement: downRight,                      // by handle — data inlines here
      viewmodel: "models/pistol/view.gltf",
    },
  },
})

defineEntity({
  name: "heavy_mg",
  components: {
    weapon: {
      damage: 9, range: 1400, fireRateMs: 70, fireMode: "auto", resolution: "hitscan",
      // sparse override via spread — one level deep to reach `up`:
      placement: { ...heavyLow, positionFromCenter: { ...heavyLow.positionFromCenter, up: -0.44 } },
      viewmodel: "models/heavy_mg/view.gltf",
    },
  },
})

// Mod-global default — applies to every weapon that omits `placement`.
// Mirrors the mod-global `switching` rule that per-weapon fields override.
defineMod({
  // ...
  defaultWeaponPlacement: downRight,
})
```

## Rough sketch

- **Type:** `WeaponPlacementDescriptor { offset, rotation }` in
  `crates/foundation/src/data_descriptors/types/combat.rs` beside `WeaponDescriptor`,
  `#[serde(rename_all = "camelCase")]`, with labeled sub-structs
  `PlacementOffset { right, up, forward }` (metres) and
  `PlacementRotation { yaw, pitch, roll }` (degrees) — each field `f32`,
  `#[serde(default)]` (omitted ⇒ 0) — and a `validate()` rejecting non-finite
  components. The internal field stays `offset`; it carries
  `#[serde(rename = "positionFromCenter")]`, so the author-facing key reads
  `positionFromCenter` (metres from screen center). External labels map to internal
  camera space: `right → +X`,
  `up → +Y`, `forward → −Z`; rotation degrees → `Quat::from_rotation_{y,x,z}`
  (radians), the order `viewmodel_camera_space_transform` already uses. The semantic
  labels are the SDK "simplify the constraint" divergence (`scripting.md §9`) from
  the `[f32;3]` vectors used for colors/velocities — warranted because placement is
  hand-authored and convention-heavy.
- **Weapon reference:** `placement?: WeaponPlacementDescriptor` on the weapon block
  — a handle's data inlined (or an inline literal, or a JS-spread override). It flows
  through the existing `serde_json::from_value(weapon).validate()` path in
  `js/entity.rs` / `lua/entity.rs`, but — like the optional `viewmodel` field — needs
  a small optional-object **pre-check** in both runtimes that **rejects** a malformed
  value: the generic VM→JSON bridge maps an unsupported value (a function or symbol) to
  `null`, which serde reads as `None`, silently rendering at `BASE_OFFSET` rather than
  erroring. Call `optional_object_field_js(weapon, "placement", …, /* reject_malformed */
  true)` (and its lua twin), the rejecting pattern `validate_optional_weapon_model_paths_js`
  uses — not the top-level `projectile` pre-check, which passes `false` and silently
  drops. No name-string reference, no registry lookup, no Rust-side merge — DRY
  (define-once) and sparse override (`{ ...base, positionFromCenter }`) are pure
  authoring-side JS.
- **Helper + default:** `defineWeaponPlacement(desc)` is a **pure SDK identity/type
  helper** (the `defineMapCatalog` / `defineTriggerPool` precedent — no FFI, no
  registration), giving the typed named-const handle. The mod-global default is
  `defaultWeaponPlacement?: WeaponPlacementDescriptor` on the manifest — inlined
  data, modeled on `ModManifestResult.switching`. There is **no** `weaponPlacements`
  registry table; immutable content inlines where referenced.
- **Resolution:** whole-value fallback, precedence `mod default < character <
  per-weapon < per-instance` — the first present tier wins the whole placement;
  absent everything ⇒ `BASE_OFFSET`. Not a field-level merge (partial inheritance is
  authored via object spread). v1 **builds** `mod default < per-weapon` and designs
  in the other two tiers as absent-in-v1 `Option` parameters — passed hardcoded `None`
  in v1, so they do no work yet (the reason is the `None`, not any coincidence of one
  player type). Their homes: the **character** default on the player
  `EntityTypeDescriptor` (lands with more than one player type/class); the
  **per-instance** override on `WeaponComponent` (lands with dual-wield), following
  the per-instance-override precedent of `block_during_reload` (an `Option<bool>` on
  `WeaponComponent` that `unwrap_or`s the mod-global default).
- **Render seam:** at the `collect_viewmodel` call (`main.rs`), both the
  local-inventory path (`local_viewmodel_asset`, host/singleplayer) and the
  Client-only archetype path (`local_active_weapon_archetype` →
  `viewmodel_asset_for_archetype`) currently return only the model `&str`, resolving
  the descriptor internally and discarding it — so the placement is *not* in scope at
  the call site. Extend **both** helpers to also return the resolved
  `Option<WeaponPlacementDescriptor>` from the *same* archetype lookup (host/SP keys on
  the weapon entity's `DescriptorProvenance::canonical_name`; the client keys on the
  replicated archetype string), so model and placement always come from one archetype
  resolution — never two out-of-band reads that could disagree. Their two unit tests
  update for the new return shape. The resolved placement threads through
  `viewmodel_world_transform` into `viewmodel_camera_space_transform`, where
  `BASE_OFFSET` lives — both signatures gain the placement param. It is re-resolved
  from the live `data_registry` borrow every frame, exactly as the model asset already
  is, so a mod/level re-drain is picked up on the next frame (no equip-time cache to go
  stale). **Composition:** placement is the base and view-feel sway rides on top —
  `Mat4::from_scale_rotation_translation(ONE, sway_rot * placement_rot,
  placement_offset + bob_offset)`, with `placement_offset` **not** rotated by sway and
  rotation about the camera origin (as today, so an authored rotation pans the gun in
  view about the eye). Absent placement passes identity rotation and
  `placement_offset = BASE_OFFSET`, folding through the exact same arithmetic as
  today's `from_srt(ONE, sway_rot, BASE_OFFSET + bob_offset)` — byte-identical, since
  an identity-quat left-multiply and the unchanged offset addition are bit-exact.

## Tasks

### Task 1: Placement descriptor + FP render seam (thin slice)

Add `WeaponPlacementDescriptor { offset: PlacementOffset, rotation: PlacementRotation }`
to `crates/foundation/src/data_descriptors/types/combat.rs` beside `WeaponDescriptor`,
`#[serde(rename_all = "camelCase")]`, with labeled sub-structs
`PlacementOffset { right, up, forward }` (metres) and
`PlacementRotation { yaw, pitch, roll }` (degrees), each field `f32`
`#[serde(default)]` (omitted ⇒ 0). The `offset` field keeps its internal name but
carries `#[serde(rename = "positionFromCenter")]` so the author-facing key reads
`positionFromCenter`. `validate()` rejects non-finite components with a
`components.weapon.placement.*` field-named `DescriptorError::InvalidShape`
(mirror the finite checks already in the file). Add an **inline-only**
`placement: Option<WeaponPlacementDescriptor>` field to `WeaponDescriptor`
(`#[serde(default)]`), and call its `validate()` from `WeaponDescriptor::validate`.
It parses through the existing `serde_json::from_value(weapon).validate()` path in
`crates/scripting-core/src/data_descriptors/js/entity.rs` and `lua/entity.rs`, but add
an optional-object **pre-check** in both runtimes (behavioral twins) that **rejects** a
malformed value — call `optional_object_field_js(weapon, "placement",
"components.weapon.placement", /* reject_malformed */ true)` (and its lua twin),
returning `DescriptorError::InvalidShape`. The rejecting precedent is
`validate_optional_weapon_model_paths_js` (the `viewmodel` path, which `Err`s on a
non-string): the generic VM→JSON bridge coerces an unsupported value (function/symbol)
to `null`, which serde reads as `None` and would silently render at `BASE_OFFSET`. Do
**not** mirror the top-level `projectile` pre-check — it passes `reject_malformed =
false` and silently drops; placement must reject (AC 8). Register `WeaponPlacementDescriptor` and its
labeled sub-structs `PlacementOffset` / `PlacementRotation` (nested sub-structs are
registered by name, as `ProjectileDescriptor` does) plus the weapon field for SDK
generation in `crates/postretro/src/scripting/primitives/mod.rs` (`register_type`
beside the `WeaponDescriptor` registration), authoring `.field(...)` doc strings that
state units (metres / degrees), the axis convention (`right/up/forward → +X/+Y/−Z`),
and the default (`BASE_OFFSET`, zero rotation) — the hover-doc content AC 9 requires
(the drift test pins the generated name and shape, not doc prose, so the docs must be
authored deliberately). Regenerate `sdk/types/postretro.d.ts` / `.d.luau` so the drift
test passes.

At the viewmodel render seam (`crates/postretro/src/main.rs`, the `collect_viewmodel`
call site), extend both `local_viewmodel_asset` (host/singleplayer) and
`viewmodel_asset_for_archetype` (the Client path, via `local_active_weapon_archetype`)
to also return the resolved `Option<WeaponPlacementDescriptor>` from the *same*
archetype lookup that yields the model — so model and placement never disagree — and
update their two unit tests for the new return shape. (The two `collect_viewmodel`-site
`.map` closures keep their distinct entity seeds — host `weapon.to_raw()`, client
`local_pawn.to_raw()` — and each widens to carry the placement. This `Option` seam is
provisional: Task 2 replaces the `None ⇒ BASE_OFFSET` fallback with
`resolve_weapon_placement`.) Thread the resolved placement through
`viewmodel_world_transform` into `viewmodel_camera_space_transform`, where the
`BASE_OFFSET` constant lives (both signatures gain the param — their two direct unit
tests, `viewmodel_transform_applies_view_feel_offsets_without_world_camera_rotation` and
`viewmodel_world_transform_keeps_shared_shader_positions_in_world_space`, update for the
new signature); re-resolve it from the
live `data_registry` borrow every frame, as the model asset already is. Compose it as
`from_scale_rotation_translation(ONE, sway_rot * placement_rot, placement_offset +
bob_offset)` — placement is the base, view-feel sway rides on top, the offset is not
rotated by sway, and rotation is about the camera origin. Map the `positionFromCenter`
labels to camera space: `right → +X`, `up → +Y`, `forward → −Z` (so today's
`BASE_OFFSET (0.32, -0.28, -0.62)` is `{ right: 0.32, up: -0.28, forward: 0.62 }`);
`placement_rot = Quat::from_rotation_y(yaw) * from_rotation_x(pitch) *
from_rotation_z(roll)` (degrees → radians), the euler order
`viewmodel_camera_space_transform` already uses for its sway quat. An absent placement
passes identity rotation and `placement_offset = BASE_OFFSET`, folding through the
exact same arithmetic as today so the transform is **byte-identical** (Invariant 1) —
assert transform-equality, not a render golden.

`WeaponDescriptor` remains outside the mod compatibility digest because its small
host-resolvable values use the existing opaque tuning payload. Each occupied wieldable
row carries the effective placement after `mod default < per-weapon` resolution. The
payload is sent on initial participation and rebuilt every host poll, so live descriptor
or default changes use the existing send-if-changed path. Connected-client rendering
uses that value without a local fallback. This changes the engine-owned payload epoch,
not the fixed transport wire layout (Invariant 2).

Tests: absent placement ⇒ `BASE_OFFSET` composition byte-identical, run with **no**
`defaultWeaponPlacement` present (AC 1); an inline placement moves the viewmodel, and
two different placements yield two different transforms, asserted at the transform
level (v1 renders one viewmodel at a time, not two on screen) (AC 2); a vertical-only
difference shifts height continuously (AC 3); a mistyped placement value is rejected by
the pre-check, and a non-finite component is rejected by `validate()` (AC 8); an
authored placement leaves the third-person `hand_r` attachment transform unchanged
(AC 7). Ordering per Ordering pins P1–P3, P7.

### Task 2: `defineWeaponPlacement` helper, mod default, layered fallback

Build the reuse and default layer on top of Task 1's per-weapon field. Add
`defineWeaponPlacement(desc)` as a **pure SDK identity/type helper** in `sdk/lib`
(the `defineMapCatalog` / `defineTriggerPool` precedent — each `return`s its argument,
no FFI, no registration; returns a typed `WeaponPlacementDescriptor`), giving authors
the named-const handle `const x = defineWeaponPlacement({...})` for define-once reuse
and IDE types. Its data inlines wherever a weapon references it, so there is **no**
`weaponPlacements` registry table and no wire sharing — DRY and sparse override
(`{ ...x, positionFromCenter }`) are pure authoring-side JS. Add a mod-global
`defaultWeaponPlacement?: WeaponPlacementDescriptor` to `ModManifestResult`
(`crates/scripting-core/src/runtime/types.rs`), parsed in both `drain_*_js` /
`drain_*_lua` manifest parsers (behavioral twins, alongside the switching parse). Add a
matching `default_weapon_placement` field to `DataRegistry`
(`crates/entities/src/data_registry.rs`) with a setter, and drain the manifest value
into it inside `drain_manifest_registrations` (`crates/postretro/src/session/mod.rs`),
in the *same* `data_registry.borrow_mut()` as `entities` (`upsert_entity_type`) and
`maps` (`replace_maps`) — that shared borrow is the whole-snapshot commit, so the mod
default is atomic with `entities` (no weapon descriptor ever live while its default is
uncommitted) and reachable at the render seam's `data_registry` borrow. This is *not*
the `switching` commit path: `switching` lands on the session `self.switching` field
(via `staged_switching`), which is neither in `DataRegistry` nor reachable at the seam
— `switching` is cited only as the *precedence* precedent (a mod-global that per-weapon
fields override), not the storage home. Adding the field to `ModManifestResult` also
requires extending the `register_type("ModManifest")` block and the `_shape_anchor`
field-parity list in `crates/postretro/src/scripting/primitives/manifest.rs` (the guard
asserting the registered `ModManifest` mirrors `ModManifestResult`), or its test fails.
Implement
`resolve_weapon_placement(mod_default, character: Option<…>, weapon: Option<…>, instance: Option<…>)
-> WeaponPlacementDescriptor` as **whole-value fallback** with precedence
`mod default < character < per-weapon < instance` — the first present tier wins the
entire placement; absent everything ⇒ a default descriptor
`{ right: 0.32, up: -0.28, forward: 0.62 }`, zero rotation, whose label→camera
conversion composes to the `BASE_OFFSET` transform. This is a *different* path from
Task 1's `BASE_OFFSET`-const passthrough — it yields the same bits (the sign-flip of
`0.62_f32` and the identity rotation are bit-exact), not the same arithmetic — so after
Task 2 rewires the seam, AC 1 rests on that conversion bit-exactness. It is not a
field-level merge; partial inheritance is authored via object
spread. v1 builds the `mod default < per-weapon` tiers and passes `character` and
`instance` as hardcoded `None` — the two future tiers are `resolve_weapon_placement`
parameters only; v1 adds **no** field to either `EntityTypeDescriptor` or
`WeaponComponent`. Their eventual homes: the character default on the player
`EntityTypeDescriptor` (lands with multiple player types), the per-instance override on
`WeaponComponent` (lands with dual-wield, following the `block_during_reload`
per-instance-override precedent). The
render seam (Task 1) now reads the resolved effective placement
(weapon `placement` else mod default else `BASE_OFFSET`) instead of the raw
per-weapon field. Register the manifest surface for SDK generation and regenerate
the typedefs. Tests: the mod default applies to a weapon that omits `placement`
(AC 4); a present per-weapon placement overrides the mod default whole-value (AC 4);
two weapons referencing the same handle resolve identically (AC 5); an archetype's
effective placement enters its host-resolved wieldable tuning row, connected-client
rendering reads that row without local fallback, and per-weapon or mod-default edits
change the payload sent by the existing live-retune path (AC 6); and — because Task 2
rewires the seam
to always read `resolve_weapon_placement` — re-assert AC 1 through the resolver path:
`resolve_weapon_placement(None, None, None, None)` composes to the `BASE_OFFSET`
transform, byte-identical (Task 1's AC 1 test covered the now-replaced const-passthrough
path). Ordering per Ordering pins P4–P6.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice through descriptor → parse → SDK →
render seam, integrated and exercised end to end. Falsifies the boundary
assumptions: the two-helper co-resolution of model and placement, the placement×sway
composition order, the camera-relative frame and euler→quat order, and the
default-equals-`BASE_OFFSET` byte-identical regression. Everything else builds on the
`WeaponPlacementDescriptor` type and the render seam it establishes.

**Phase 2 (sequential):** Task 2 — consumes Task 1's descriptor, field, and render
seam; adds the `defineWeaponPlacement` identity helper, the mod-global default, and
the whole-value layered fallback the seam then reads. Sequential (not concurrent)
because it edits the same render-seam resolution Task 1 introduced.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD |
|---|---|---|---|---|---|
| `WeaponPlacementDescriptor` | `{ offset: {right,up,forward: f32}, rotation: {yaw,pitch,roll: f32} }` (`offset` serde-renamed) | `"positionFromCenter"` + `"rotation"`, labeled keys | `defineWeaponPlacement({ positionFromCenter, rotation })` (pure identity helper) | same | n/a |
| `placement` (weapon field) | `WeaponDescriptor.placement: Option<WeaponPlacementDescriptor>` | `"placement"` | `placement?: WeaponPlacementDescriptor` (a handle, inline literal, or `{ ...handle, … }` spread) | same | n/a |
| mod default | `ModManifestResult` default-placement (`Option<WeaponPlacementDescriptor>`) | `"defaultWeaponPlacement"` | `defaultWeaponPlacement?: WeaponPlacementDescriptor` on `defineMod` | same | n/a |

Placement frame (every surface): `positionFromCenter` (internal `offset`) in metres
from screen center — `right` / `up` / `forward` (forward = toward the aim);
`rotation` in degrees — `yaw` / `pitch` / `roll`. External labels map to internal
camera space (`right → +X`, `up → +Y`, `forward → −Z`; rotation → quaternion).
Default (absent everything) = today's `BASE_OFFSET`, i.e.
`{ right: 0.32, up: -0.28, forward: 0.62 }`, zero rotation.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| 1. A weapon with no placement at any tier (no mod default, no per-weapon) composes at exactly today's `BASE_OFFSET`. | Task 1 (`BASE_OFFSET`-const passthrough), preserved by Task 2 (the resolver's absent-everything default converts to the `BASE_OFFSET` transform) | Threatened if the default differs, the resolver's absent-everything default drifts from `BASE_OFFSET`, resolution perturbs the transform when nothing is authored, or a present `defaultWeaponPlacement` masks the AC 1 regression pin. | AC 1 |
| 2. Placement adds no fixed transport wire field. Host-resolved effective placement rides each occupied wieldable's opaque tuning row; connected clients never resolve it locally. | Task 1 (per-weapon field and render seam), Task 2 (mod default and resolver), parity repair (tuning payload) | Threatened if placement leaves tuning, live tuning change detection stops rebuilding, or a connected-client fallback reads local descriptor/view-feel state. | AC 6 |
| 3. Resolution is whole-value fallback, precedence `mod default < character < per-weapon < per-instance`: the first present tier wins the entire placement; absent everything ⇒ `BASE_OFFSET`. Partial inheritance is authored via object spread, not an engine merge. v1 builds `mod default < per-weapon`; character and per-instance are absent (`None`) parameters. | Task 2 (`resolve_weapon_placement`) | Threatened if the order is wrong, a present per-weapon placement fails to override the mod default, or the two future tiers are foreclosed rather than left as no-op parameters. | AC 4 |
| 4. Placement is first-person only; the third-person avatar mount is unchanged. | Task 1 (render seam touches only the viewmodel transform) | Threatened if any placement value is threaded into the `hand_r` attachment (reopens the E21 offset-in-data foreclosure). | AC 7 |

## Ordering pins

The invariants state *what* holds; these rows pin the *mechanics* — each is
concrete enough to write a test from. Task 1's tests cover P1–P3, P7; Task 2's cover
P4–P6.

| # | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Host/SP resolution key | Weapon equipped; `Host` or no `net_endpoint`; the seam runs `local_viewmodel_asset` only | Placement resolves from the local weapon entity's descriptor (`DescriptorProvenance::canonical_name`) — the same lookup that yields the model; no archetype-string read; absent ⇒ `BASE_OFFSET`; transform byte-identical to today. |
| P2 | Model/placement coherence | Either path resolves both the model and the placement in one archetype lookup | Model and placement always name the same archetype; never model = weapon A / placement = weapon B. |
| P3 | Batched weapon switches | Two switches in one tick, then render (render follows game logic) | The seam samples the final active wieldable; the last switch's weapon *and* its placement render; no intermediate placement is drawn. |
| P4 | Mod default present, weapon omits `placement` | `defaultWeaponPlacement` set; weapon has no per-weapon `placement` | Resolves to the mod default, not `BASE_OFFSET`. (The AC 1 byte-identical pin therefore runs with **no** mod default.) |
| P5 | Default changes under a live weapon | Mod/level re-drain d_A → d_B; the same unauthored weapon stays equipped | The next frame renders d_B (re-resolved from the live `data_registry`), not a cached d_A. |
| P6 | Manifest commit atomicity | `defaultWeaponPlacement` and `entities` arrive from one manifest | Both drain in the same `drain_manifest_registrations` borrow (one whole-snapshot commit); no weapon descriptor is live while its mod default is uncommitted. |
| P7 | Boot / pre-equip | Frame 1, during load, or no weapon equipped | `collect_viewmodel` is not called (both paths `None`); no placement resolves and nothing is drawn — Invariant 1 applies only once a weapon exists. |

The downstream authority-sampling pins (host reproduces the steady placement at tick
rate; render-rate sway excluded) belong to `weapon-muzzle-origin` — see §Out of scope,
fire origin.
