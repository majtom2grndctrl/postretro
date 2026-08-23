# Projectile Weapon Enhancements

## Goal

Enrich the shipped projectile visual (`E16--projectile-resolution`) with four
descriptor-authored presentation capabilities: **emissive (full-bright)** sprite
bodies, **flipbook-animated** sprite bodies, an **attached dynamic point light**
that travels with the projectile, and a **parameterized impact-flash light** that
pops — and can expand into a shockwave — at the hit point. The expanding flash is
enabled by a new **generic radius-animation channel** in the light system (the
light-animation curves animate brightness/color/direction today, but not radius,
for no principled reason; this completes them). These turn a flat, scene-lit bolt
into a glowing, pulsing, self-illuminating energy shot that bursts on impact — the
cyberpunk boomer-shooter identity the aesthetic calls for. All are additive,
co-op-correct, and add no wire-format change.

**Prerequisites.** `light-bridge-runtime-light-reclamation` — **shipped**
(`context/plans/done/light-bridge-runtime-light-reclamation`; merged to `main`).
The light bridge now reclaims a despawned runtime light's reserve slot, so the
per-shot travel + impact lights here — the engine's first high-churn runtime
lights, the consumer that prerequisite was built for — no longer cumulatively
exhaust the 256-slot `RUNTIME_DYNAMIC_LIGHT_RESERVE`. Where this spec says a slot
is "reclaimed by the prerequisite," that reclamation is now live engine behavior;
anchor its exact seam (free-list, live-count bound) against the merged
`light_bridge.rs`.

## Scope

### In scope

- **Emissive sprite body.** A per-collection additive HDR emissive strength on
  `ProjectileBodyVisual::Sprite`. The billboard pass adds an unconditional
  self-lit term (`sprite.rgb × emissive`), so a bolt reads full-bright regardless
  of scene lighting and blooms above `BLOOM_THRESHOLD` (§7.8, no bloom change).
  Default 0 = an inert term, byte-identical to current. The term is **not** gated
  by the dev-tools `LightTermMask` — emissive is self-only, categorically outside
  the scene-lighting isolation set (the shipped `lighting--per-term-isolation`
  owner decision that leaves bit 7 permanently unwired).
- **Flipbook sprite body.** A multi-frame sprite-strip body that cycles frames
  over an authored per-frame duration while the projectile travels. Keeps the
  existing **numbered-frames-directory** authoring convention (`textures/<coll>/
  <coll>_00.png`, `_01.png`, …, the `load_sprite_frames` collection-dir path
  emitters already use) and the existing billboard flipbook (`frame_idx` from
  age, uniform frame time); the new work is feeding the body an advancing age and
  registering the collection with a cadence. Applies to the local, predicted, and
  remote-observer bodies.
- **Attached dynamic point light.** An optional `ProjectileLight` on the
  projectile visual. Spawn attaches a `LightComponent` (point, dynamic,
  `follow_transform`) so the light illuminates nearby surfaces and tracks its
  body's render pose each frame — the **raw** tick `Transform` for a sprite body
  (matching the un-interpolated billboard), the **interpolated** pose for a model
  body (matching the mesh) — so the light stays locked to whichever body renders;
  it despawns with the projectile.
  Teaches the light bridge a follow-Transform contract (it is origin-cached
  today). Consumes `RUNTIME_DYNAMIC_LIGHT_RESERVE`; does not cast entity shadows.
- **Light radius-animation channel (engine-floor).** `LightAnimation` gains a
  **radius** channel alongside brightness/color/direction — a sample curve
  evaluated **CPU-side in the light bridge each frame** (mirroring how
  `effective_brightness` is already evaluated every frame), packed into the light's
  `GpuLight` range and its influence sphere; no shader change. Generic to any light
  (a throbbing alarm, a breathing sign); the impact flash is its first consumer.
  While a radius animation is active the light re-packs each frame (like a moving
  light already does), and its influence tracks the current radius so culling stays
  correct as the light grows.
- **Impact-flash light.** An optional `ProjectileImpactLight` on the projectile
  visual — authored parameters (color, intensity, radius, optional peak
  radius, fade duration) so the author decides the effect. On a real contact (not
  travel-bound expiry), spawn a **stationary transient** point light at the hit
  point beside the existing impact particle burst (`spawn_impact_effect_at`); it
  fades over the authored duration and self-despawns (via the existing
  `DeferredEffect` despawn); its reserve slot is reclaimed by the prerequisite.
  With a peak radius it
  **expands** from start to peak over the fade (a shockwave, via the radius
  channel); without one it is a static pop. Presentation only — **not** AoE/splash
  damage (that stays deferred to the later AoE spec, which composes this same
  chokepoint and channel).
- **Co-op parity.** All four visible to remote peers on the host-spawned
  presentation projectile, and to the firing client on its predicted projectile,
  with no doubled light and no new wire message/field.
- **Author surface.** Descriptor validation, TS + Luau SDK typedefs, and the
  dev-mod reference weapons updated in the same pass (primitive-surface contract).

### Out of scope (non-goals)

- **Per-instance emissive or flipbook variation** (e.g. a bolt that dims or
  animates per-instance independent of its collection). Emissive/flipbook are
  per-collection; per-instance variation is additive later (the GPU sprite
  instance has a spare `_pad`).
- **Emissive on model (mesh) projectile bodies.** Billboard sprite bodies only;
  model emissive stays deferred with `emissive-surfaces-bloom`. Emissive is a
  `::Sprite`-variant field, so a model body has none to ignore — its path is
  untouched.
- **Spot / directional / shadow-casting projectile lights.** Both the travel and
  impact lights are point lights that cast no entity shadows; no cone, no
  directional. The travel light authors no animation (it just moves); the impact
  light authors brightness-fade and optional radius-grow curves.
- **GPU-side radius animation.** The new radius channel is evaluated **CPU-side in
  the bridge** (like `effective_brightness`), re-packing the light's range +
  influence each frame while active. A GPU-side radius channel (a descriptor curve
  read in every falloff shader) is not built — it would widen the change across
  forward/billboard/mesh shaders for no benefit at these brief, small-count
  animations.
- **Authoring the radius channel on map/script lights.** This spec adds the radius
  channel to `LightAnimation` and drives it only from the impact flash. Exposing a
  radius curve on the map-light / `setLightAnimation` script surface is a natural
  follow-up but out of scope here (no consumer beyond the impact flash yet).
- **Single-sheet-grid or metadata-JSON flipbook formats.** This spec keeps the
  numbered-frames-directory convention. A single sprite-sheet + grid (Unity/Unreal
  style) or an Aseprite/TexturePacker JSON sidecar (variable per-frame durations,
  named tags) is an engine-wide sprite-pipeline change, not projectile-scoped —
  its own future spec.
- **A dev-tools toggle for emissive.** Emissive is not gated by the
  `LightTermMask` light-term isolation instrument (the shipped decision keeps it
  self-only and bit 7 unwired). A dedicated projectile-glow visibility toggle, if
  ever wanted, is separate work outside that instrument. Emissive's default of 0
  already gives the "off = byte-identical" guarantee without any gate.
- **Emissive tint/color separate from the sprite texture.** The sprite texture
  is the color; emissive is a scalar HDR strength. Additive later.
- **Retuning damage/flight** (speed, radius, lifetime, range) — unchanged from
  E16. These are presentation-only enhancements.
- **New replicated fields.** A goal, verified as an invariant.

## Direction

**Problem.** Shipped projectile bodies are scene-lit billboards (or rigid
models) with no self-illumination, no body animation, no cast light, and no
impact flash — so a plasma bolt darkens in a dark room, never pulses, throws no
light, and lands with only a small particle puff. The cause is that the billboard
pass has no emissive term, the projectile body is packed at frame 0, no gameplay
entity has ever carried a moving light, and the impact chokepoint spawns only a
particle burst.

**Prior commitments.** Consumes without diverging from: the E16 projectile visual
package (body + optional trail, attached at `spawn_projectile` from the
descriptor) and its co-op presentation path (client-materialized visual
components, no wire change); descriptor-owned tuning with no FGD KVPs
(entity_model §4); the primitive-surface contract (SDK + validation move with the
Rust surface, index §2); the additive-HDR emissive + bloom shape established by
`emissive-surfaces-bloom`; the `lighting--per-term-isolation` owner decision that
emissive is self-only and outside the light-term isolation set (so this spec's
billboard emissive is unconditional, not mask-gated); the no-double-count lighting
invariant (index §2 — emissive is a self-only additive term, the dynamic light is
an ordinary additive dynamic-tier light); the billboard vertex storage-buffer
budget ≤ 8 (rendering §7.4); for the impact flash, the single impact chokepoint
(`spawn_impact_effect_at`, `weapon/impact.rs`) that already spawns a self-expiring
particle burst — the flash spawns a transient light beside it, fading via the
existing `LightAnimation` brightness path and self-despawning through the existing
`DeferredEffect` despawn; and, for the radius channel, the per-frame CPU curve-eval PATTERN the bridge
already uses (`effective_brightness` is recomputed every frame — though it only
ranks shadow slots, it does not pack), plus the per-frame re-pack that Task 3
introduces for follow-Transform lights (no light re-packs every frame today;
brightness/color animate GPU-side). The radius channel reuses both, packing
range/influence CPU-side with no shader change. **Two deliberate divergences, argued.** (1) The light bridge is
origin-cached, built for fixed map lights; a moving projectile light cannot be
expressed by writing `component.origin` (the cache wins). This spec teaches the
bridge a follow-Transform contract — the right layer for the first mover-attached
gameplay light, reused by future moving-light effects. (2) `LightAnimation` animates
brightness/color/direction but **not** radius, an asymmetry with no principled
basis; this spec completes it with a generic radius channel. The channel is
engine-floor lighting infrastructure, not projectile-specific — which also sets up
the boundary below.

**Boundary with the deferred AoE / rocket-launcher spec.** An expanding explosion
light looks like AoE, and E16 deferred AoE/splash to "the next Resolution-Modes
spec." The seam is kept clean by scope, not coincidence: the impact-flash light is
**generic impact presentation** (a plain plasma bolt flashes on any hit, no
explosion), spawned off the *same* `spawn_impact_effect_at` chokepoint the particle
burst already uses; the radius channel is **generic light infrastructure** (any
light can pulse its radius). Nothing explosion-*semantic* — splash radius, area
damage, per-target payloads — is built here. The AoE spec later **composes** the
same chokepoint and the same radius channel for its explosion; it does not
re-implement impact presentation, and this spec does not pre-empt its damage model.

**Alternatives rejected.** (a) A glowing *model* body with an `_e.png` emissive
texture instead of a billboard emissive term — model emissive is deferred, models
are heavier, and the billboard bolt is the genre idiom; the billboard term is
cheap and reuses bloom. (b) Attaching the light via a script reaction on spawn —
projectile spawn is engine-floor and not per-projectile script-reachable, and the
follow-Transform plumbing is needed regardless. (c) Body animation via a trail
emitter only — the ask is a pulsing/spinning *body*. (d) A **static-only** impact
flash (no radius growth) — rejected on the project's "build more right faster"
principle: the destination (an expanding shockwave) is clear, the runtime cost is
one more CPU curve eval, the work is patterned after the existing brightness
channel, and it completes a lopsided engine capability — so shipping the static
version and reopening the light system later fragments the build for no runtime
saving. (Full derivation, incl. the leanness-vs-build-sequencing distinction:
`research.md`.)

**Foreclosures / one-way doors.** Low and named. Per-collection emissive
forecloses per-instance emissive until a per-instance channel is added (additive;
spare `_pad` exists). Reusing `SpriteDrawParams.params.w` for emissive spends the
last free slot in that vec4 (a later second per-collection scalar needs a new
uniform field). The `follow_transform` flag defaults false, so the light-bridge
change reverts cleanly. The radius channel is CPU-side (bridge-local): it forecloses
nothing on the shaders and reverts cleanly (a `None` radius curve = today's static
range); the deferred GPU-side variant stays available if a future high-count or
long-lived radius animation ever needs it. The impact flash's transient-light
**despawn** reuses the existing `DeferredEffect` path; its reserve slot is
reclaimed by the prerequisite (`light-bridge-runtime-light-reclamation`). None is a one-way door; undoing the feature is
deleting the descriptor fields, one shader term, two component attaches (travel +
impact light), the bridge flag, the radius channel, and the transient-despawn.

## Acceptance criteria

- [ ] A projectile weapon authored with a sprite body and an emissive strength > 0
  renders full-bright: the bolt shows at (at least) its full texture color in an
  unlit room and, at HDR strength (≥ ~1), produces a bloom halo; with
  `POSTRETRO_BLOOM=0` the halo is gone but the bolt is still full-bright. A sprite
  body with emissive 0 (or unset) is **byte-identical** to the current billboard
  output, and the emissive term is not gated by the dev-tools `LightTermMask`.
  (Note: 'renders full-bright / blooms' is a visual/review gate; 'byte-identical'
  is a draw-param assertion — `params.w == 0`, adding-0.0 is exact — not a
  rendered-pixel compare; 'ungated by `LightTermMask`' is a shader-review/grep
  gate.)
- [ ] A model-body projectile is unaffected by emissive: `emissive` is a field of
  the `ProjectileBodyVisual::Sprite` variant, so a `kind: "model"` body carries
  none — the model path renders exactly as today and logs no error.
- [ ] A projectile weapon authored with a multi-frame sprite body **and a
  per-frame duration** cycles through its frames while travelling (observable:
  the packed instance age advances with flight time and the frame index wraps),
  at the authored cadence, independent of the projectile's travel lifetime/range.
  A body with **no cadence authored** — a single-PNG body OR a multi-frame
  collection dir — packs age `0.0` and shows a static frame: the packed instance
  is byte-identical to current.
- [ ] The flipbook animates identically on the local (SP/host) body, the firing
  client's predicted body, and the remote-observer presentation body — none is
  pinned to frame 0.
- [ ] A projectile weapon authored with a `light` illuminates nearby world
  surfaces and entities as it travels: the lit area moves with the projectile
  (position tracks its body's render pose each frame — raw tick `Transform` for
  a sprite body, interpolated pose for a model body — so the lit area stays on
  the bolt, not a fixed spawn point), and the light disappears the frame the projectile despawns (impact or
  travel-bound) — no light persists after despawn (its reserve slot reclaimed by
  the prerequisite).
- [ ] A projectile light is a dynamic point light that casts no entity shadows,
  consumes one `RUNTIME_DYNAMIC_LIGHT_RESERVE` slot while alive, and its slot is
  reclaimed on despawn by the prerequisite; when more projectile lights are live
  than the reserve holds, the bridge
  warns once and the surplus lights simply do not render (no crash, no corruption
  of the authored/other dynamic lights). (`casts_entity_shadows` is not a
  `LightComponent` field — it is forced `false` in `component_to_map_light`, so
  assert it through the map-light conversion.)
- [ ] A light carrying a `LightAnimation` **radius** curve animates its lit area:
  the bridge evaluates the curve each frame and the light's effective falloff
  range **and** its influence-volume cull radius both track the current value (so
  a growing light is never wrongly culled as it expands, and a shrinking one stops
  lighting beyond its current reach). A light with no radius curve is unchanged
  (static range, byte-identical to today). A finite (`play_count`-bounded) radius
  curve settles its final value into the packed range at completion (the
  `check_play_count_completion` radius branch).
- [ ] A projectile weapon authored with an `impactLight` spawns a stationary
  point light at the **hit point** on a real contact — with the authored color,
  intensity, and radius — that fades over the authored duration and
  self-despawns; its reserve slot is reclaimed by the prerequisite (no light
  lingers). When a **peak
  radius** is authored the lit area **expands** from start to peak over the fade
  (a shockwave); without one it is a static pop. A projectile that reaches its
  travel bound without hitting spawns **no** impact flash; a projectile with no
  `impactLight` authored behaves exactly as today (particle burst only).
- [ ] Co-op: a projectile fired by any peer shows its emissive, flipbook, moving
  light, and impact flash (expanding included) to other peers (on the host-spawned
  presentation projectile) and to the firing client (on its predicted projectile);
  the firing client sees exactly one moving light for its own shot (no doubled
  presentation copy). No new wire message or field is added and no version constant
  is bumped. (Component/enrollment-level assertions; 'sees'/rendered is a visual
  gate. 'No new wire message/field' is a review/grep gate; the version-constant
  check is a runtime assertion on the constant.)
- [ ] Author surface: an out-of-range emissive strength, an invalid flipbook
  cadence, an out-of-range/invalid travel `light`, or an invalid `impactLight`
  (bad color/intensity/radius/peak-radius/fade) is rejected at descriptor
  validation with a field-named error; the TS and Luau typedefs expose the emissive
  field, the flipbook cadence, and the `light` and `impactLight` unions (incl. the
  optional peak radius); the dev mod's reference plasma bolt is emissive +
  flipbook-animated + casts a travel light + a modest impact pop, and the reference
  rocket casts a travel light + a larger **expanding** impact shockwave.
- [ ] Tests: emissive-off and single-frame regressions are byte-identical to
  current output; a projectile light tracks its body's render pose (raw `Transform`
  for a sprite body, interpolated pose for a model body) and is removed on despawn
  (slot reclaimed by the prerequisite); the
  reserve-exhaustion path degrades gracefully; a radius curve drives
  both the packed range and the influence cull radius each frame (and a no-radius
  light is unchanged); an impact flash spawns on contact (not on expiry), expands
  when a peak radius is authored, and self-despawns after its fade (slot reclaimed
  by the prerequisite); a connected-client projectile's emissive/flipbook/travel-light/impact-flash
  are present on the predicted and remote paths; no new `unsafe` (grep gate); no
  authority version constant changed (both are grep/review gates, not runtime
  behavior tests).

## Tasks

### Task 1: Emissive sprite body — thin slice (descriptor → collection → billboard → bloom)

Build the whole emissive spine end-to-end for single-player, the narrowest path
crossing every seam the sprite-body capabilities share (descriptor field →
validation → SDK → level-load collection harvest → per-collection draw-param →
billboard shader term → bloom). **Descriptor:** add an emissive strength field to
`ProjectileBodyVisual::Sprite` in
`crates/foundation/src/data_descriptors/types/combat.rs` (an f32, default 0.0 =
current behavior, HDR-capable; a full-bright bolt authors ~2–4). Harden
`validate_projectile_descriptor` to require it finite and ≥ 0, as a field-named
`DescriptorError::InvalidShape` mirroring the existing sprite-field checks. Add no
FGD KVP. **Registration:** the emissive strength is a property of the sprite
*collection*. Thread it from the projectile-sprite harvest
(`projectile_presentation_assets` in
`crates/postretro/src/scripting/builtins/data_archetype.rs`, threaded through
`crates/postretro/src/startup/lifecycle.rs`, which today yields
`{ collection, lifetime }`) into
`SmokePass::register_collection`
(`crates/renderer/src/render/smoke.rs`) (threaded through the
`renderer.register_smoke_collection(collection, frames, spec_intensity,
lifetime)` wrapper and `ProjectileSpriteCollection`, which both gain the
emissive field) and pack it into the free
`SpriteDrawParams.params.w` slot via `build_draw_params` (currently
`(frame_count, spec_intensity, lifetime, pad)`), keeping the 16-byte size.
Because emissive is per-collection (keyed by sprite name), all live instances of
that projectile's sprite share it; note in a comment that a particle emitter
sharing the same sprite name would inherit it (projectiles register their own
sprite, so this is not a collision in practice). **Shader:** in
`crates/renderer/src/shaders/billboard.wgsl`, read `params.w` as the emissive
strength and add an additive self-lit term to the fragment output —
`sprite_sample.rgb × emissive` added on top of the existing
`sprite_sample.rgb × lighting`, so a full-bright bolt shows its texture color
even when `lighting` is near zero and, at HDR strength, exceeds `BLOOM_THRESHOLD`
and haloes through the existing bloom compositor (no bloom-pass change). The term
is **unconditional** — do **not** gate it on `LightTermMask` (emissive is
self-only, categorically outside the scene-lighting isolation set; the shipped
`lighting--per-term-isolation` owner decision keeps bit 7 permanently unwired, and
the default mask `LightTermMask::ALL = 0x7F` has bit 7 clear, so gating there would
make the feature off by default). The default strength 0 gives the "off =
byte-identical" guarantee with no gate. Compute the emissive contribution in
`fs_main` from the `draw_params` strength (or carry it interpolated from
`vs_main` — implementer's choice), keeping the ≤ 8 VERTEX storage-buffer budget:
emissive rides the group-1 `draw_params` **uniform**, adding no storage buffer.
**SDK +
reference:** add the emissive field to the TS and Luau `ProjectileSpriteBodyVisual`
typedefs (`sdk/types/postretro.d.ts`, `.d.luau`; generator under
`crates/postretro/src/scripting/typedef/`), and make the dev-mod reference plasma
bolt emissive. **Co-op parity is automatic and needs no co-op-specific work
here:** the remote-observer presentation body and the firing client's predicted
body render through the same `ParticleRenderCollector` and the same registered
collection, so they read the same `params.w` emissive — the enhancement is a
property of the sprite collection, not the entity. AC: emissive > 0 renders
full-bright and blooms; emissive 0 is byte-identical to current and ungated by the
light-term mask; a model body with an emissive field set is unaffected.

### Task 2: Flipbook sprite body — advancing age + multi-frame cadence

Make a sprite body cycle frames while travelling, on the local, predicted, and
remote-observer bodies. The billboard flipbook already exists (`billboard.wgsl`
`frame_idx = floor(age / (lifetime / frame_count)) % frame_count`) and the
multi-frame source already exists (`load_sprite_frames` treats a `.png` reference
as one frame and a reference **without** `.png` as a numbered collection dir); the
missing pieces are an advancing age for the body and a loop cadence. **Descriptor:**
add a per-frame duration field (ms, finite > 0 when present) to
`ProjectileBodyVisual::Sprite`; `None`/absent = static (single frame, current
behavior). Validate it in `validate_projectile_descriptor` (field-named error).
**Age:** add an elapsed-flight age (seconds) to `ProjectileComponent`
(`crates/entities/src/components/projectile.rs`) advanced in
`crates/postretro/src/sim/projectile_stage.rs` (`advance` for host/SP; the client
path `advance_predicted` reuses the same advance body, so it advances there too),
and pack it **only when the descriptor authored a flipbook cadence**
(`frameDurationMs`); when no cadence is
authored, pack `0.0` as today (byte-identical, static frame 0 — regardless of
frame_count, so a single-PNG body AND a multi-frame collection-dir body with no
cadence both stay static). Carry a flipbook-active flag (or the `Option`
cadence) on `ProjectileComponent`, set at `spawn_projectile` from
`body.frameDurationMs.is_some()`, so both the advance and the collector's pack —
in `crates/postretro/src/scripting/systems/particle_render.rs::collect`, for
**both** the `ComponentKind::Projectile` arm and the `ProjectilePresentation`
arm — know whether to use the elapsed age or `0.0`. Apply the same gate to the
presentation arm (it packs `0.0` unless the descriptor — resolved via
`entity_class` — has a cadence). The
presentation projectile carries **no spawn stamp today**
(`projectile_presentation.rs::attach_projectile_visual_components` /
`materialize_armed_remote_projectile` attach only Transform + provenance + visual
components; the host-side `PresentationFlight` side table is not
registry-readable by the collector). So this task **adds** a spawn-time (or
elapsed-age) registry component to the presentation entity at materialization —
readable by `particle_render.rs::collect`'s presentation arm — and threads a `now`
(the fixed-tick `script_time`) into that arm. Pin the clock to the **same
fixed-tick basis and spawn-tick offset** the local `ProjectileComponent` age uses,
so the frame index matches across local/predicted/presentation (AC 4's
'identically'). This is required new state, not a confirm-only step. No
replicated field. **Registration:** register the projectile sprite
collection with the frames from the collection-dir source; **only when a
cadence is authored**, set the per-collection draw-param `lifetime` (the
flipbook loop period) to `per_frame_ms/1000 × frame_count` (so
`frame_duration = lifetime / frame_count` equals the authored per-frame
duration and the loop wraps via `% frame_count`) — with no cadence, leave
today's dead `1.0` (age is packed `0.0`, so the value is inert). This
repurposes the currently-dead projectile `lifetime` draw-param. The seam is the
same one Task 1 threads: `projectile_presentation_assets` →
`ProjectileSpriteCollection` (extend it with the cadence) → the `lifecycle.rs`
loop → `register_smoke_collection`; `frame_count` comes from
`load_sprite_frames`/`stitch_frames_to_strip` in that loop. **SDK +
reference:** typedef the cadence field (TS + Luau) and make the reference plasma
bolt a multi-frame animated sprite. AC: a multi-frame body animates at the
authored cadence on local, predicted, and remote bodies, independent of travel
lifetime/range; a single-frame or cadence-less body is byte-identical to current.

### Task 3: Attached dynamic point light — spawn attach + follow-Transform bridge + co-op

Give a projectile an optional dynamic point light that travels with it, on the
SP/host, predicted-client, and remote-observer paths. **Descriptor:** add
`light: Option<ProjectileLight>` to `ProjectileVisual`
(`crates/foundation/src/data_descriptors/types/combat.rs`), where
`ProjectileLight` carries color (`[f32;3]`), intensity (f32), falloff range (f32),
and falloff model (reuse the existing falloff discriminant; default inverse-square)
— a point light only, no cone. Validate in `validate_projectile_descriptor`:
intensity finite ≥ 0, range finite > 0, color finite, field-named errors. Add no
FGD KVP. **Component attach:** in
`crates/postretro/src/sim/weapon_stage/commands.rs::spawn_projectile`, when
`visual.light` is present attach a `LightComponent`
(`crates/entities/src/components/light.rs`) at the muzzle: `light_type = Point`,
`is_dynamic = true`, the authored color/intensity/falloff, and a new
`follow_transform` flag set true (see below). **Follow-Transform bridge contract:**
add a `follow_transform: bool` to `LightComponent` (internal routing, `#[serde(
default)]` = false, omitted from world-query snapshots like `animated_slot`), and
teach `crates/postretro/src/scripting/systems/light_bridge.rs` that a tracked
runtime light with `follow_transform` resolves its position and influence center
from the pose that MATCHES ITS BODY's render path: a **sprite** body from the
**raw** `Transform` (the value `pack_sprite_instance` packs), a **model** body
from `interpolated_transform(id, alpha)` (the value `mesh_render` packs at the
frame alpha) — the interpolated read is correct ONLY for a model body; a sprite
body must use the raw `Transform` or the light trails the un-interpolated
billboard. The bridge selects raw-vs-interpolated per light by probing the
projectile entity's body component — `has_component_kind(id, SpriteVisual)` →
raw `Transform`, `has_component_kind(id, MeshComponent)` →
`interpolated_transform` (new component-kind probes on a bridge that today
knows only lights). `interpolated_transform(id, alpha)` already exists on `EntityRegistry`
(which `update` already holds `&mut` to), and `frame_result.alpha` is already in
scope at the `main.rs` bridge call site, so the only new wiring is adding an
`alpha` param to `update` and passing it. The genuinely-new work: the per-id loop
reads only `LightComponent` today (not the `Transform`), so add (a) a body-matched
pose read per follow light plus a cached-pose comparison that sets `self.dirty` on
movement, and (b) two overrides in the pack loop — the packed **position** (today
`cached_origins_f64[idx]` via `component_to_map_light`) AND the influence **center**
(today `cached_influences[idx]`, seeded once at enrollment via
`component_to_influence(&LightComponent)` (it reads `.origin` internally)) must
both come from the body-matched
pose, not the caches. Non-follow lights are unchanged (they still read
`component.origin`).
**Cleanup:** the light despawns with the projectile — the projectile entity's
removal drops its `LightComponent`, and the bridge's existing tombstone path (a
tracked id whose component read fails forces one zeroing upload) zeroes its GPU
slot; the reserve slot itself is reclaimed by the prerequisite
(`light-bridge-runtime-light-reclamation`) — the shipped free-list
`LightBridge.free_slots` + per-slot `MapLightShape.reclaimed`, with the
live-count bound `entity_ids.len() - authored_light_count - free_slots.len()`.
Confirm the follow-Transform light is
enrolled so the zeroing fires. Projectile
lights inherit the runtime-light default `casts_entity_shadows = false`
(`component_to_map_light`), so they never enter the shadow pool. **Co-op:** on the
connected client's predicted projectile, attach the same `LightComponent` locally
(mirrors Task 1/2 predicted presentation). On the remote-observer presentation
projectile, attach a `LightComponent{follow_transform}` **client-side from the
shared descriptor** in
`crates/postretro/src/netcode/projectile_presentation.rs::attach_projectile_visual_components`
(which `remote_materialize.rs::materialize_armed_remote_projectile` delegates to
— the same site that already materializes the `SpriteVisual`/`MeshComponent`
body) so it tracks the replicated/interpolated Transform — adding no wire
field. The host's existing per-client presentation suppression stops the firer
seeing a doubled light for its own shot. **Client enrollment (new plumbing):**
`absorb_dynamic_lights` runs only in the host/SP tick block today (`main.rs`);
the connected-client branch skips it. Add a client-path `absorb_dynamic_lights`
call — in the connected-client tick branch or the post-loop client fire/
materialization path — that runs BEFORE the frame's `light_bridge.update`, so
the client's predicted, presentation, and impact-flash `LightComponent`s enroll
and render. Without it none of the client-side lights appear. **SDK + reference:** typedef the `light`
union (TS + Luau) and make **both** reference weapons cast a travel light — the
plasma bolt a small blue light, the rocket a warm exhaust light. AC: the lit area
moves with the projectile and vanishes on despawn; the light casts no entity
shadows; reserve exhaustion degrades gracefully; remote peers and the firing
client each see one moving light; no wire change.

### Task 4: Light radius-animation channel (engine-floor)

Add a **radius** channel to `LightAnimation` so a light can animate its falloff
range over time, generic to any light — the impact flash (Task 5) is its first
consumer. **Descriptor field:** add `radius: Option<Vec<f32>>` to `LightAnimation`
(`crates/entities/src/components/light.rs`), a sample curve alongside the existing
`brightness`/`color`/`direction`, `#[serde(default)]`, snake_case storage / camelCase
wire like the siblings. **CPU evaluation in the bridge:** in
`crates/postretro/src/scripting/systems/light_bridge.rs`, evaluate the radius curve
each frame reusing the `effective_brightness` eval PATTERN
(`sample_brightness_at`/`_at_open`; note `effective_brightness` only ranks shadow
slots — it does not pack, so the packing below is new). Packing is NOT just "call
`component_to_influence`": in the pack loop, override the `MapLight.falloff_range`
with the current radius **before** `pack_light`, AND override the pushed influence
radius with the current radius (not the `cached_influences[idx]` clone). Both must
track the animated value so the influence cull radius grows with the lit radius
and a growing light is never wrongly culled as it expands. Because packing is
dirty-gated, a light with an **active** radius curve must mark the bridge dirty
each frame while the radius curve is active (`animation.radius.is_some()` and
not yet settled) — the impact flash is stationary (`follow_transform: false`),
so it does NOT get Task 3's movement-driven dirty; the active-radius dirty
trigger is its own mechanism (no light re-packs every frame today;
brightness/color animate GPU-side), so the range/influence re-pack while it
animates; when the curve is `None` nothing
changes (static range, byte-identical). **No shader change:** the falloff shaders
read `GpuLight` range as today; only the packed value moves. Extend the
`play_count`-completion path (`check_play_count_completion`, which today settles
brightness→intensity, color, and cone_direction but has **no radius branch**) so
a finite radius curve settles its final value into `falloff_range`. For the
impact flash the settled intensity is 0 (final brightness 0), so the settled
light is invisible; still pin the settle-frame influence radius (= final/peak)
for the culling test. AC: a light with a radius curve grows /
shrinks its packed range and influence radius in lockstep each frame; a light with
no radius curve is byte-identical to today.

### Task 5: Impact-flash light — parameterized transient light (static or expanding)

Give a projectile an optional light flash where it lands, authored as parameters so
the author decides the effect (the dev demo flashes a bigger radius that expands and
fades). Builds on Task 3's descriptor-light validation and `LightComponent`-
materialization patterns and Task 4's radius channel. **Descriptor:** add
`impact_light: Option<ProjectileImpactLight>` to `ProjectileVisual`
(`crates/foundation/src/data_descriptors/types/combat.rs`), where
`ProjectileImpactLight` carries color (`[f32;3]`), intensity (f32), a base
**radius** (f32, its own falloff range — typically larger than the travel light), an
**optional peak radius** (f32; when present the flash expands start→peak over the
fade — a shockwave; when absent it is a static pop), and a fade duration (ms, > 0).
Validate in `validate_projectile_descriptor`: intensity finite ≥ 0, radius
finite > 0, peakRadius (if present) finite ≥ radius, fade finite > 0, color
finite; field-named errors. Independent of the travel `light` — a weapon may author
either, both, or neither. Add no FGD KVP. **Spawn at contact:** in the projectile stage's impact
branches (`crates/postretro/src/sim/projectile_stage.rs`) — **both** `advance`
(host/SP) and `advance_predicted` (client) call `spawn_impact_effect_at` on a
real contact before despawning the projectile, **only** on a real contact,
never on travel-bound expiry — spawn a **stationary** light entity at the hit
`point`. CRITICAL: a non-follow bridge light reads its
position from `LightComponent.origin`, NOT the entity `Transform` — so set
`LightComponent.origin = point` (a bare `Transform` at `point` would render the
flash at the world origin). The component is `light_type = Point`,
`is_dynamic = true`, authored color/intensity, `radius` as the base
`falloff_range`, **`follow_transform = false`** (it does not move, so it needs no
interpolation) carrying a one-shot `LightAnimation { period_ms = fade,
play_count = Some(1), brightness = [1.0, 0.0], radius: peak.map(|p| vec![start, p]) }`
(the omitted `LightAnimation` fields — `phase`, `start_active`, `color`,
`direction` — are `None`) — brightness fades
and, when a peak is authored, the Task 4 radius channel expands the lit area over
the same fade. **Config carriage:** `ProjectileComponent` (verified) carries only
flight state and the impact branch must never re-resolve `owner_weapon` (it may be
gone), so carry the resolved `ProjectileImpactLight` config **on
`ProjectileComponent`** (not replicated), written at `spawn_projectile` where
`launch.descriptor.visual` is in scope; the impact branch reads it from there.
Remove the entity after the fade with the existing
`DeferredEffect` despawn (schedule a `DeferredEffectKind::Despawn` at `fade` ms).
Read `visual.impact_light` (and Task 3's `visual.light`) for the
`ProjectileComponent` config **before** the `match launch.descriptor.visual.body`
moves the body (or via a borrow). Use the ready-made
`impact_effects::despawn(registry, id, Some(fade_ms))` helper for the timed
despawn.
Ordering (grounded): `tick_deferred_effects` runs at the TOP of the tick, so a
despawn scheduled during the impact branch starts counting next tick and fires
~1 tick AFTER the `play_count` settle — the settle wins. Because the settled
brightness is 0 the settled light is **invisible**, so the ≤1-tick pre-despawn
window is not a visible defect; its reserve slot is reclaimed (prerequisite) once
the despawn fires. Do not claim the despawn beats the settle. The impact light
is stationary, so it does **not** need Task 3's follow-Transform path. It casts no
entity shadows (runtime-light default). **Co-op:** the flash is a brief presentation
effect, so spawn it wherever a projectile resolves — the host's gameplay projectile
and the firing client's predicted projectile each spawn it at their own contact
point (a local effect, like a muzzle flash; no replication). For remote peers, the
flash spawns at the presentation projectile's despawn point — the site is
`projectile_presentation.rs::advance` (where the presentation entity despawns),
which today carries only kinematics in `PresentationFlight` (no
descriptor/impact-light); thread the `impact_light` config into
`PresentationFlight` (or re-resolve it via `descriptor_class`) so that site can
spawn the presentation flash. The presentation despawn
(`projectile_presentation.rs::advance`) fires on shot-retire OR straight-line
completion and does **not** carry the authority's contact-vs-expiry outcome
(`PresentationFlight` has no such signal), so the remote flash is **not** gated
on contact — it spawns on every remote shot resolution. This is the accepted
presentation approximation — also true of the hit location itself (the
presentation projectile is deterministic straight-line) — presentation only, no
new wire field; do not hunt for a contact signal that isn't replicated. **SDK + reference:**
typedef the `impactLight` union (TS + Luau, incl. the optional peak radius) and
author both reference weapons to flash on impact — the plasma bolt a modest static
blue-white pop; the rocket a larger **expanding** warm shockwave (peak radius > start,
the demo's grow-and-fade). AC: an `impactLight` spawns a stationary flash at the hit
point on contact, expands when a peak radius is authored, fades, and self-despawns
(slot reclaimed by the prerequisite); no flash on travel-bound expiry; no `impactLight` authored = today's
behavior; no wire change.

### Task 6: Tests + reference-weapon finalization

Extend the weapon-stage / projectile-stage and netcode harnesses to cover the
capabilities deterministically, and finalize the dev-mod reference weapons.
**Emissive:** assert emissive 0 packs a draw-param byte-identical to today
(regression) and emissive > 0 packs the authored strength into `params.w`; assert
the emissive term is not gated by the light-term mask (the default mask has bit 7
clear, so a gated term would be off by default). **Flipbook:** assert a
**no-cadence** body (single-PNG **and** a multi-frame collection dir) packs age
`0.0` — byte-identical at the packed-instance level; assert a **cadence**
body's packed age advances across ticks (frame index advances) on the local,
predicted, and presentation collector arms — the presentation arm is the
regression that used to pin age 0.0. **Travel light:** assert a projectile with a `light` attaches a
`follow_transform` `LightComponent` at spawn; that the light bridge packs its
position from its body's render pose (step the projectile, assert the packed light
position moved to match the body — raw `Transform` for a sprite body, interpolated
pose for a model body) rather than the spawn origin;
that despawn tombstones (zeroes) the slot with the reserve entry reclaimed by the
prerequisite (no leaked light); that exceeding `RUNTIME_DYNAMIC_LIGHT_RESERVE` warns once and
drops surplus lights without disturbing the others; that projectile lights carry
`casts_entity_shadows = false`. **Radius channel:** assert the bridge evaluates a
`LightAnimation.radius` curve each frame and packs the current value into **both**
the `GpuLight` range and the influence radius (step time across the curve, assert
both move together); assert a `None` radius curve leaves range/influence
byte-identical to today; assert the `play_count` finite curve settles to its final
value. **Impact light:** assert a contact spawns a stationary flash light at the hit
point (and a travel-bound expiry spawns none); that with a peak radius the installed
`radius` curve is `[start, peak]` and the packed range grows over the fade, and
without a peak the range stays static; that it self-despawns after the fade via the
`DeferredEffect` despawn (slot reclaimed by the prerequisite, no lingering light); and that a projectile
with no `impactLight` spawns none. **Ordering pins:** assert the impact light's
packed `GpuLight` position equals the hit `point` (not the world origin — the
`origin = point` requirement); assert a model-body travel light's packed position
equals the interpolated pose at a fractional render alpha (not the raw tick
pose); assert a **sprite**-body travel light packs from the raw `Transform`
(matching the billboard), not the interpolated pose; assert that on a
connected-client path the predicted/presentation/impact `LightComponent`s
enroll (the client-side `absorb` fires) and render; assert the flipbook frame index matches across the local, predicted, and
presentation arms after K ticks (same clock basis + spawn-tick offset); assert a
sub-frame `fadeMs` (below one tick) yields a one-frame pop with no visible
expansion; assert the `check_play_count_completion` radius write-back sets
`falloff_range` to the final/peak value. **Co-op:** a connected-client projectile's
emissive/flipbook/travel-light/impact-flash (expanding included) are present on the
predicted and presentation paths and the firer sees no doubled light; assert no
authority version constant changed and no new `unsafe` (grep gate). Finalize the
reference weapons (plasma bolt: emissive + flipbook + travel light + static impact
pop; rocket: model + trail + travel light + expanding impact shockwave) and update
the weapon-authoring reference docs to cover the new fields (the dev-mod
reference weapons live in the dev mod's weapon descriptors — name the concrete
path when editing; Tasks 2, 3, and 5 each touch the same reference plasma
bolt). AC: the harness
exercises each listed behavior deterministically; references are authored and placed
to fire on the dev map.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice. Falsifies the shared boundary the
sprite-body capabilities reuse (descriptor field → `projectile_presentation_assets`
harvest → `register_collection` draw-param → `billboard.wgsl` term → bloom) with
the simplest capability, before flipbook and light fan out.
**Phase 2 (concurrent):** Task 2 (flipbook) and Task 3 (travel light) — disjoint
seams on top of Task 1. Task 2 touches `particle_render.rs`, `projectile_stage.rs`,
`ProjectileComponent`, and the collection cadence; Task 3 touches `light_bridge.rs`,
`spawn_projectile`, `remote_materialize.rs`, and `LightComponent`. Both add
disjoint fields to `combat.rs` and the SDK typedefs (additive; coordinate the
shared-file edits). Neither touches the other's stage code. Task 2 and Task 3
also both edit the **same dev-mod reference weapon descriptor** — coordinate
that file alongside `combat.rs` and the SDK typedefs.
**Phase 3 (sequential):** Task 4 (radius-animation channel) — extends the light
bridge Task 3 just touched (both edit `light_bridge.rs` + `LightComponent`), so it
follows Task 3 rather than racing it on those files. Engine-floor; its first
consumer is Task 5.
**Phase 4 (sequential):** Task 5 (impact light) — consumes Task 3's
`LightComponent` materialization + co-op presentation path and Task 4's radius
channel; adds the impact-branch spawn and the `DeferredEffect` despawn.
**Phase 5 (sequential):** Task 6 — tests + reference finalization, once behavior
lands.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Emissive strength | `ProjectileBodyVisual::Sprite` emissive f32 (default 0) → `SpriteDrawParams.params.w` | descriptor JSON camelCase (content, shared — not replicated) | typedef field on `ProjectileSpriteBodyVisual` | typedef field | n/a |
| Flipbook cadence | `ProjectileBodyVisual::Sprite` per-frame-duration ms `Option<f32>` → per-collection draw-param `lifetime = ms/1000 × frame_count` | descriptor JSON camelCase | typedef field | typedef field | n/a |
| Projectile body age | `ProjectileComponent` elapsed-flight age f32 (engine-internal) | not replicated (presentation body derives its own elapsed time) | n/a | n/a | n/a |
| Projectile (travel) light | `ProjectileVisual.light: Option<ProjectileLight>` → `LightComponent{Point, is_dynamic, follow_transform}` at spawn | descriptor JSON camelCase; light **materialized client-side** from the shared descriptor, no new wire field | typedef union `light?` | typedef union | n/a |
| Impact-flash light | `ProjectileVisual.impact_light: Option<ProjectileImpactLight>` (color, intensity, radius, optional peakRadius, fade ms) → stationary `LightComponent{Point, follow_transform:false}` + one-shot fade `LightAnimation{ brightness:[1,0], radius:[radius,peak]? }` at the hit point | descriptor JSON camelCase; local presentation effect (host + predicted client) + presentation-projectile flash for peers, no new wire field | typedef union `impactLight?` | typedef union | n/a |
| Impact-light config carriage | resolved `ProjectileImpactLight` stored on `ProjectileComponent` at `spawn_projectile`; read by the impact branch | not replicated (host/predicted local; presentation flash threads it via `PresentationFlight`) | n/a | n/a | n/a |
| Presentation spawn stamp | spawn-time / elapsed-age registry component on the presentation entity, added at materialization; read by the collector with a threaded `now` | not replicated (each observer derives its own elapsed) | n/a | n/a | n/a |
| Light radius-animation channel | `LightAnimation.radius: Option<Vec<f32>>` (sample curve; CPU-evaluated in the bridge → `GpuLight` range + influence radius) | descriptor JSON camelCase like `brightness`/`color`; not replicated (attached to a client-materialized light) | n/a (not exposed to script/SDK this spec) | n/a | n/a |
| Follow-Transform flag | `LightComponent.follow_transform: bool` (`#[serde(default)]`, snapshot-omitted) | internal — not authored, not a world-query field | n/a | n/a | n/a |

Naming: the travel light's falloff range is `falloffRange` (attenuation); the
impact light's is `radius`/`peakRadius` (flash size) — deliberately different
words for the same underlying falloff-range concept.

## Wire format

No new binary or replicated surface. Emissive and flipbook cadence are descriptor
content (shared by both peers, like the existing projectile visual, not
replicated). The travel light rides the presentation entity's existing Transform +
`entity_class` snapshot record and is materialized client-side from the shared
descriptor (as the body/trail already are). The impact flash is a local
presentation effect (spawned at each observer's own contact / the presentation
projectile's despawn point), also materialized client-side from the descriptor.
The `follow_transform` flag, the body age, the `LightAnimation` radius curve, and
the impact light's fade/despawn are engine-internal, never serialized. No version
constant changes on the authority path.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| **Emissive-off is byte-identical** to current billboard output (default strength 0, additive term contributes zero) | Task 1 | a non-zero default; folding emissive into the existing `× lighting` multiply instead of an additive term | AC 1; Task 6 emissive-0 regression |
| **No-cadence body is byte-identical**: no cadence authored (single-PNG or multi-frame dir) packs age `0.0` → static frame 0, packed instance byte-identical to current; age advances only when a cadence is authored | Task 2 | packing a non-zero age for a no-cadence body; a multi-frame dir animating without an authored cadence | AC 3; Task 6 no-cadence regression |
| **Flipbook animates on all three bodies** (local, predicted, presentation) — none pinned to age 0.0 | Task 2 | the presentation arm keeping its hard-coded 0.0; the predicted path not advancing age | AC 4; Task 6 presentation-age assertion |
| **Travel light tracks its body's render pose** (position + influence from the body-matched pose each frame — raw `Transform` for a sprite body, interpolated pose for a model body — not the cached spawn origin) | Task 3 (follow-Transform bridge contract) | the bridge reading `cached_origins_f64` / `component.origin`; a stale influence center; reading the interpolated pose for a sprite body (or raw for a model body) so light and bolt diverge; a stale cached origin/influence | AC 5; Task 6 moving-light assertion |
| **Travel light despawns with its projectile** — no leaked light after impact/expiry; reserve slot reclaimed by the prerequisite | Task 3 (enroll for the bridge tombstone path) | a follow light not enrolled, so no tombstone fires; a presentation light outliving its shot | AC 5; Task 6 despawn-tombstone assertion |
| **Radius curve drives range AND influence in lockstep** — a growing light's cull radius tracks its lit radius; `None` = static range unchanged | Task 4 (CPU per-frame eval → `pack_light` range + the `cached_influences[idx]` push (both overridden per frame from the animated radius)) | packing the animated range but leaving the influence at the static `falloff_range` (a growing light culled before it reaches) | AC 7; Task 6 radius-channel assertion |
| **Impact flash spawns only on contact, expands (if peak), fades, and self-despawns** — no light lingers, its reserve slot is reclaimed by the prerequisite; a travel-bound expiry spawns none | Task 5 (impact-branch spawn + one-shot brightness/radius curves + `DeferredEffect` despawn) | spawning on expiry; the fade settle preceding the despawn by ~1 tick (accepted: the settled light is intensity-0/invisible; the slot is reclaimed on despawn) | AC 8; Task 6 impact-flash spawn/expand/despawn assertion |
| **Projectile lights cast no entity shadows** and consume `RUNTIME_DYNAMIC_LIGHT_RESERVE`, with reclamation of despawned slots (prerequisite) so churn does not cumulatively exhaust; degrading gracefully on genuine concurrent exhaustion | Task 3 (travel), Task 5 (impact) — runtime-light `casts_entity_shadows = false`; reserve bound | a projectile light entering the shadow pool; reserve overflow crashing or corrupting other lights | AC 6; Task 6 reserve-exhaustion + no-shadow assertions |
| **No wire-format change** (emissive/flipbook are descriptor content; lights are client-materialized; age/flag/fade/radius are internal) | Tasks 1–5 (descriptor + client materialization), Task 6 (assert) | a new replicated field/message; a version-constant bump; serializing the flag, age, or curves | AC 9; Task 6 no-constant-changed assertion |
| **Billboard VERTEX storage-buffer budget ≤ 8** preserved | Task 1 (emissive via the group-1 `draw_params` uniform), Task 2 (age via the existing instance channel) | adding a VERTEX-visible storage buffer to the billboard pipeline | rendering §7.4 guard test (`billboard_pipeline_vertex_storage_request_matches_bgl_definitions`) |

## Orderings

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Sprite body with `frame_count == 1` or no cadence authored | no animation state | Static frame 0; byte-identical to current (`frame_count` clamps to `max(_,1)`, `frame_duration` to `1e-6`). |
| Projectile whose entire flight is shorter than one frame duration | age never reaches `frame_duration` | Shows only frame 0; acceptable. |
| Flipbook age exceeds one loop period | age ≥ `lifetime` | Wraps via `% frame_count`; loops continuously while travelling. |
| Projectile with a travel light despawns on the impact tick | despawn precedes the next bridge pack | Same-frame bridge sees the `LightComponent` gone → one tombstone upload zeroes the slot; no light persists; reserve slot reclaimed (prerequisite). |
| N simultaneous projectile lights (travel + impact), N > `RUNTIME_DYNAMIC_LIGHT_RESERVE` | absorb order | Bridge warns once; surplus lights do not render; authored + other dynamic lights unaffected; no crash. |
| Slot freed in frame F, reused later | free pushes to `free_slots` in `update` (end of frame F); that same `update` emits the zeroed `GpuLight` for the still-in-`entity_ids` slot; `absorb` pops `free_slots` only in a later tick | Zero-before-reuse holds structurally: a reclaimed slot is always GPU-zeroed the frame it frees and cannot be re-popped until a later frame's `absorb`, so a new light never inherits stale slot data. |
| Impact flash spawns the same tick the travel light despawns | tick N: impact branch spawns flash + despawns projectile(+travel light) → `absorb` (same tick) enrolls the flash → post-loop `update` frees the travel-light slot | The flash does NOT reuse the travel light's slot this tick (it is not in `free_slots` until `update`, after `absorb`); the travel-light slot is zeroed that `update` and reusable only from tick N+1. |
| Connected client fires a light projectile | predicted (local) + presentation (host, per peer) | Firer sees one moving light (its predicted copy; host suppresses the firer's presentation copy); other peers see the presentation light. No doubled light. |
| Connected client fires a projectile carrying a `light`/`impactLight` | client tick branch `continue`s without `absorb`; only `update` runs per frame | Requires the new client-side `absorb` call (Task 3); without it the predicted/presentation/impact lights never enroll → never render on the client. |
| Projectile reaches its travel bound (no contact) | expiry precedes despawn | Travel light removed with the projectile; **no impact flash spawned** (flash is contact-only). |
| Projectile hits a target/wall | contact → `spawn_impact_effect_at` + impact-light spawn → projectile despawn | Impact flash spawns at the hit point, fades over its duration (and expands start→peak if a peak radius is authored), then self-despawns (slot reclaimed by prerequisite); the travel light dies with the projectile the same tick. |
| Impact flash with a peak radius, over the fade | each frame while animating the bridge re-packs range + influence | Range grows start→peak; the influence cull radius grows with it, so the expanding light is never culled before it reaches; brightness fades to 0 over the same window. |
| Impact flash's fade completes | fade elapses → `DeferredEffect` despawn fires | The `play_count` settle fires ~1 tick BEFORE the `DeferredEffect` despawn (despawn counts from the next tick); the settled light is intensity-0 (invisible), then the despawn removes it and the slot is reclaimed (prerequisite). No visible zero-brightness linger. |
| Emissive default (0) / flipbook default (single frame) / no light / no impactLight | steady state | Byte-identical to current output (regression rows). |
| Level unload with lit/animated projectiles and live impact flashes | registry teardown | Projectiles and all their lights (travel + impact) cleared with the registry; the bridge clears its tracking (`LightBridge::clear`); no dangling light next level. |
| Impact light spawned with a `Transform` but default `LightComponent.origin` | non-follow bridge reads `component.origin`, ignores the `Transform` | Renders at the world origin — WRONG. Requires `origin = point`. |
| Model-body (rocket) travel light between ticks (render alpha 0.5) | mesh renders interpolated; light packed once per frame | Light position = the interpolated pose, locked to the rendered rocket — not the raw tick pose; a **sprite**-body light packs from the **raw** `Transform` (matching the un-interpolated billboard) — the pose source matches the body kind, so light and bolt never diverge. |
| Tick that spawns a remote presentation projectile + light | `absorb_dynamic_lights` runs before `host_spawn_projectile_presentations`; body collected this frame, light enrolled next | Accepted as an untested approximation (like the presentation-flash location): the presentation body may show for one frame without its travel light (the host enrolls it tick N+1). |
| Flipbook frame index after K ticks, all three bodies | local/predicted age from `ProjectileComponent` (fixed tick); presentation age from `now − spawn_time` | Identical frame index — pinned by using the same fixed-tick clock basis and spawn-tick offset (Task 2). |
| `impactLight.fadeMs` below one tick | settle fires the tick after spawn; radius curve never sampled mid-way | One-frame pop at base radius; expansion not observable. Accepted (validation still requires fade > 0). |

## Rough sketch

Grounded seams, the light lifecycle diagram, the vantage × lifecycle
cross-product, and the direction derivation live in `research.md`. Key entry
points: `ProjectileBodyVisual::Sprite` / `ProjectileVisual` /
`validate_projectile_descriptor` (`foundation/.../combat.rs`);
`SmokePass::register_collection` / `build_draw_params` (`renderer/src/render/
smoke.rs`); `billboard.wgsl` `fs_main`/`vs_main`; `ParticleRenderCollector::
collect` / `pack_sprite_instance` (`particle_render.rs`);
`projectile_stage.rs` `advance`/`advance_predicted` (and its impact branch, which
calls `spawn_impact_effect_at`); `spawn_projectile` (`weapon_stage/commands.rs`);
`spawn_impact_effect_at` (`weapon/impact.rs`); `LightBridge` /
`absorb_dynamic_lights` / `component_to_map_light` / `component_to_influence`
(`light_bridge.rs`); `LightComponent` / `LightAnimation` (`entities/.../light.rs`; the new `radius`
channel mirrors `brightness`); the bridge's per-frame `effective_brightness` eval
and `sample_brightness_at`/`_at_open` CPU Catmull-Rom mirrors (the pattern the
radius eval follows); `pack_light` range + the `cached_influences[idx]` push
(the radius channel's two pack targets, both overridden per frame);
`DeferredEffectKind::Despawn` (the chosen
transient-despawn); `remote_materialize.rs`. Reserve constant:
`RUNTIME_DYNAMIC_LIGHT_RESERVE` (`postretro_renderer`). Bloom: rendering §7.8,
`BLOOM_THRESHOLD`, `POSTRETRO_BLOOM=0`.

## Script syntax example

```ts
// Proposed — a glowing, animated, light-casting plasma bolt (dev-mod reference).
const plasmaRifle = defineEntity({
  components: {
    weapon: {
      damage: 25,
      range: 128,
      fireRateMs: 180,
      fireMode: "auto",
      resolution: "projectile",
      projectile: {
        speed: 80,
        radius: 0.2,
        lifetimeMs: 4000,
        visual: {
          body: {
            kind: "sprite",
            sprite: "plasma_bolt",           // bare collection name → textures/plasma_bolt/plasma_bolt_NN.png (60-frame flipbook, placed)
            size: 0.4,
            emissive: 3.0,                    // HDR full-bright; blooms above threshold
            frameDurationMs: 60,              // per-frame hold; loops while travelling
          },
          light: {                            // moving dynamic point light (travels with the bolt)
            color: [0.3, 0.7, 1.0],
            intensity: 2.5,
            falloffRange: 6.0,
          },
          impactLight: {                       // stationary flash at the hit point
            color: [0.6, 0.85, 1.0],
            intensity: 4.0,
            radius: 10.0,                       // bigger than the travel light — a static pop
            fadeMs: 180,                        // fades then self-despawns
            // (no peakRadius → static, non-expanding)
          },
          // optional trail unchanged from E16
        },
      },
      creditSource: "plasma",
    },
  },
});

// The reference rocket authors a warm travel light + a larger EXPANDING impact shockwave:
//   light:       { color: [1.0, 0.7, 0.3], intensity: 3.0, falloffRange: 8.0 },
//   impactLight:  { color: [1.0, 0.6, 0.2], intensity: 6.0,
//                   radius: 4.0, peakRadius: 18.0,   // expands 4→18m over the fade (shockwave)
//                   fadeMs: 300 },
```
(Exact field names — `emissive`, `frameDurationMs`, the `light` and `impactLight`
shapes incl. the optional `peakRadius` — are the tasks' to pin against the existing
descriptor style; the constraint is that they are descriptor-owned tuning on the
projectile visual, never FGD KVPs.)

Resolved decisions (owner-confirmed this session): **emissive is an HDR scalar**
(default 0); **flipbook keeps the numbered-frames-directory convention** with a
**per-frame duration (ms)** cadence (single sprite-sheet + grid, and metadata-JSON
formats, are an engine-wide follow-up, out of scope here); **both reference weapons
cast a travel light**; **the impact light is parameterized** (author decides);
**the growing-shockwave impact flash is in scope**, built on a new generic
radius-animation channel in `LightAnimation` (evaluated CPU-side in the bridge) —
the "build more right faster" call: clear destination, patterned work, cheap at
runtime, and it completes a lopsided engine capability; **the remote-peer impact
flash stays in scope** (spawned at the presentation projectile's despawn point, an
accepted presentation-only approximation consistent with the shipped remote-observer
aim asymmetry); **the transient despawn reuses `DeferredEffect`** rather than a new
per-tick system.

Resolution notes:

- **Formerly-open items, now resolved:** the presentation spawn stamp is added as
  a new registry component at materialization (Task 2); the settle-vs-despawn
  window is accepted as an invisible ≤1-tick linger (the settled light is
  intensity-0), with the reserve slot reclaimed by the prerequisite (Task 5,
  Orderings).
- **Dependency (satisfied):** `light-bridge-runtime-light-reclamation`
  (reserve-slot reclamation) is **shipped** on `main` (`context/plans/done/`);
  projectile lights build on top of it.
