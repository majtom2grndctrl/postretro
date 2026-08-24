# Weapon Placement — Research

Design analysis. Not the spec. Decisions live in `index.md`.

## The problem this solves

The first-person weapon (viewmodel) sits at a **hardcoded constant**:
`BASE_OFFSET = Vec3::new(0.32, -0.28, -0.62)` inside
`viewmodel_camera_space_transform` (`crates/postretro/src/main.rs:1371`), composed
into world space by `viewmodel_world_transform` (`:1387`) and rendered by
`collect_viewmodel` (`main.rs:~3783`) through a dedicated `viewmodel_projection`
(`crates/renderer/src/render/renderer_frame.rs:14`). Every weapon in every game
sits in the same spot. Authors cannot express a centered retro look, a
down-and-right placement, a heavy weapon that rides lower, a grenade launcher on
top (Doom Eternal), or wide dual miniguns (Killzone 3).

## The decomposition that keeps concerns separate

Two things were conflated in the earlier `weapon-muzzle-origin` draft and must be
split:

- **How a weapon is held** — its placement in view: centered / down-right /
  on-top / dual, and continuous per-weapon differences (a pistol sits higher than
  a heavy MG). This is **per-game / per-weapon taste** → script/data. It must not
  live in the model, or the model stops being portable across games.
- **Where the barrel is** — the muzzle point in the weapon's own coordinates.
  Intrinsic model geometry, portable across games (like a hit zone). Owned by the
  model. (That is the downstream `weapon-muzzle-origin` spec's concern, not this
  one.)

This spec owns the first: authorable placement. The muzzle composes onto it later
(`world_muzzle = eye ∘ placement ∘ muzzle_local`).

## Placement is a continuous transform, not an enum

A pistol vs a heavy machine gun differ by arbitrary amounts on every axis
(vertical, forward, lateral, tilt). So the primitive is a continuous placement
transform — a labeled `positionFromCenter { right, up, forward }` (metres, internal
field `offset`) plus
`rotation { yaw, pitch, roll }` (degrees) — authored in the viewmodel's
camera-relative frame that `viewmodel_camera_space_transform` already uses. (Scale
is out of scope; per-weapon size is a modeling concern.) Named presets ("downRight", "centered") are convenience that
resolve *to* a transform on top of the primitive — never the primitive itself.

## DRY: a reusable weapon-placement descriptor with layered override

Authors should define a placement once and reuse it. `defineWeaponPlacement` is a
**pure SDK identity/type helper** (the `defineMapCatalog` / `defineTriggerPool`
precedent: each `return`s its argument, no FFI, no registration) returning a typed
`WeaponPlacementDescriptor`. Reference it
by handle (`const x = defineWeaponPlacement({...})`; `placement: x`); its immutable
data inlines where used, so there is no wire registry and no name-string lookup. The
tiers resolve as **whole-value fallback**:

```
mod / character default  <  per-weapon  <  per-instance (dual-wield)
```

- A house default at the mod (or later, player-descriptor) level covers weapons
  that omit `placement`.
- A per-weapon placement overrides the default whole-value. Partial inheritance —
  "same as the default but lower" — is authored in JS via object spread
  (`{ ...downRight, positionFromCenter: { ...downRight.positionFromCenter, up: -0.44 } }`),
  not an engine field-merge.
- A dual-wielded pistol overrides per-instance (left vs right) — future tier.

Precedence mirrors the shipped `switching` mod-global rule that per-weapon
`blockDuringReload` overrides; it is **not** the unbuilt `player-descriptor-composition`
`base`+`layers` field-merge. Placement is a first-class axis (its own descriptor,
cited-not-owned by that draft), deliberately not a character-only field — authoring
it only at the character level would foreclose per-weapon placement, the corner to
avoid.

**Placement ≠ viewFeel.** `player-descriptor-composition` reserves a "wieldable
viewFeel overlay" — but viewFeel is the render-only *sway/bob/tilt* integrator.
Placement is the *base position* the sway rides on. Different concerns; placement
is a sibling axis, not the viewFeel overlay.

## The render seam

Placement threads in at one place: the `collect_viewmodel` call site
(`main.rs:~3783`). That site already holds `descriptors` (the data registry
borrow), the local pawn, the resolved weapon entity, and — on the client fallback
path — the active weapon archetype (`local_viewmodel_asset` /
`viewmodel_asset_for_archetype` / `local_active_weapon_archetype`,
`main.rs:1339`, `:3760`). So it can resolve the weapon's placement transform and
pass it into `viewmodel_world_transform` in place of the constant `BASE_OFFSET`.
The default placement equals today's `BASE_OFFSET`, so an unauthored weapon is
unchanged (regression pin).

## The three origins and the netcode contract

(Pinned here because it governs *why placement is content*, though the fire-origin
consumer is the downstream muzzle spec.)

The shot origin differs by vantage, and that is correct:

- **Shooter's FP muzzle** — `eye ∘ FP-placement(+view-feel sway) ∘ muzzle_local`.
  Client-local; what the shooter sees.
- **Observer's TP muzzle** — the third-person avatar's weapon mount (the existing
  hand-socket attachment) ∘ `muzzle_local`. Presentation only; what other players
  see the projectile leave.
- **Authoritative origin (hits)** — `eye ∘ FP-placement(steady, no sway) ∘
  muzzle_local`, reproduced host-side. This is how "hits honor what the shooter
  sees" holds without extra wire: FP placement is replicated **content**, the eye
  and aim already replicate, and render-rate view-feel sway is cosmetic and
  excluded from authority. The shooter's own client declares the contact; the host
  validates it by distance (the existing projectile model — favor-the-shooter in
  co-op PvE, `netcode/mod.rs` `valid_projectile_contact_point`).

Consequence for *this* spec: **FP placement must be authored content available on
every peer**, not a client-local render tweak. It rides the weapon archetype
descriptor, which is shared by mod-parity admission (entity types are engine-global
via the mod manifest; admission gates on mod id — `scripting.md §Mod init`,
`networking.md §Admission and content parity`). So placement reaches host and every
client with no new wire. Shipped E21 (`plans/done/E21--coop-avatar-weapon-presentation`)
already confirms both halves: `active_weapon_archetype` replicates as an identity
string (not the descriptor), and a client resolves the descriptor locally by
`canonical_name` — so a remote player's weapon (TP mount) already renders from the
replicated archetype, and a new presentation field on that archetype rides the same
mod-parity content with no wire change.

## Scope boundary with the muzzle spec

This spec ships placement as **presentation** — the viewmodel sits where authored,
and the third-person mount is reconciled. Projectiles still fire from the eye until
`weapon-muzzle-origin` (reframed as downstream) composes the muzzle onto the
authored placement. Placement being content (not just render) is what lets that
later spec reproduce the authoritative origin — so this spec establishes the
content, even though it does not itself move the fire origin.

## Dual-wield

Placement is modeled per-wieldable-instance from the start (the precedence chain's
last tier), so dual-wield is additive. The first spec builds and proves
single-weapon placement; the per-instance override and the second-active-slot
machinery land with the inventory/wieldable work the weapon-model research marks
unbuilt (`context/research/weapon-model.md §8`). Design-for-it, build single first.
