# Projectile Weapon Enhancements — Research

Derivation notes for `index.md`. Grounded seams, lifecycle diagrams, the vantage
× lifecycle cross-product, and the direction exercise. Not consumed by task
agents; the spec inlines what they need.

All identifiers below were read from source during this session.

## What shipped (E16 — Projectile Resolution)

`context/plans/done/E16--projectile-resolution/` (its header still reads
"ready … awaiting orchestration"; the feature is fully in source — the header
is stale, treat code as truth).

A projectile is an ordinary registry entity: `Transform` +
`ProjectileComponent` + presentation components attached at spawn from the
weapon descriptor. Grounded seams:

- **Component.** `crates/entities/src/components/projectile.rs` —
  `ProjectileComponent { direction, speed, radius, remaining_range,
  remaining_lifetime, damage, credit_source, owner_pawn, owner_weapon, spawned,
  predicted_shot_id }`. `ComponentKind::Projectile = 21` (tail),
  `crates/entities/src/registry.rs`. Not replicated.
- **Descriptor.** `crates/foundation/src/data_descriptors/types/combat.rs` —
  `ResolutionMode::{Hitscan, Projectile}`; `WeaponDescriptor.projectile:
  Option<ProjectileDescriptor>`; `ProjectileDescriptor { speed, radius,
  lifetime_ms, visual }`; `ProjectileVisual { body: ProjectileBodyVisual, trail:
  Option<ProjectileTrailVisual> }`; `ProjectileBodyVisual::Sprite { sprite,
  size, opacity, tint }` (serde `#[serde(tag="kind")]` → `{ kind: "sprite" }`) |
  `Model { model }`. Validation: `WeaponDescriptor::validate` →
  `validate_projectile_descriptor` → `validate_projectile_asset_path`; errors are
  field-named `DescriptorError::InvalidShape`.
- **Spawn.** `crates/postretro/src/sim/weapon_stage/commands.rs::spawn_projectile`
  branches on `visual.body`: `Sprite` → attach `SpriteVisual`; `Model` → attach
  `MeshComponent::stateless` (rigid); `visual.trail` → attach
  `BillboardEmitterComponent`. `projectile_model_body_rotation` orients model
  bodies. Fire arm: `crates/postretro/src/weapon/mod.rs::fire_hitscan`
  (returns a launch intent; `ActivationOutcome::Spawned`).
- **Advance stage.** `crates/postretro/src/sim/projectile_stage.rs` — `advance`
  (host/SP, fixed tick), `advance_predicted` (connected client, per frame).
  Called from `sim/mod.rs::simulate_tick_with_presentation_aim` between the
  weapon-fire stage and the death sweep.
- **Render.** The projectile body renders through the **billboard pass**, not a
  bespoke path. `crates/postretro/src/scripting/systems/particle_render.rs::
  ParticleRenderCollector::collect` walks three columns — `ParticleState`
  (particles), `ComponentKind::Projectile` carrying `SpriteVisual` (local
  projectile body, packed at **age 0.0**), and `DescriptorProvenance` with
  `spawn_path == DescriptorSpawnPath::ProjectilePresentation` (remote-observer
  copies, also age 0.0). `pack_sprite_instance` packs the 32-byte
  `SPRITE_INSTANCE_SIZE` layout (`crates/render-cpu/src/fx/smoke.rs`):
  `(pos.xyz, age)`, `(size, rotation, opacity, _pad)`. **No color/tint or
  emissive channel** — `SpriteVisual.tint` is stored but never uploaded.
- **Co-op presentation.** `crates/postretro/src/netcode/{projectile_presentation,
  descriptor_class,remote_materialize}.rs`. The host spawns a presentation-only
  projectile (marked `DescriptorSpawnPath::ProjectilePresentation`); it rides the
  existing Transform + `entity_class` snapshot record; `remote_materialize.rs`
  attaches presentation components (the `SpriteVisual`/`MeshComponent` body +
  `BillboardEmitter` trail) **client-side from the shared descriptor**, no new
  wire field; per-client exclusion stops the firer double-drawing its own shot.

## The billboard pass — what exists for emissive and flipbook

`crates/renderer/src/shaders/billboard.wgsl`, `crates/renderer/src/render/smoke.rs`.

- **Flipbook already works — for particles.** `vs_main` computes
  `frame_idx = floor(age / (lifetime / frame_count)) % frame_count` and per-frame
  UV `u = (frame_idx + corner.u) / frame_count` over a horizontal sprite strip.
  `SpriteDrawParams` (group 1 binding 2, per-collection uniform) carries
  `params.x = frame_count`, `params.y = spec_intensity`, `params.z = lifetime`,
  `params.w = pad` (**one free f32**). Frames come from
  `stitch_frames_to_strip` at `SmokePass::register_collection(device, queue,
  collection, frames, spec_intensity, lifetime)`; the frame source is
  `crates/render-cpu/src/fx/smoke.rs::load_sprite_frames` — a `.png` reference is
  one frame; a reference **without** `.png` is a sequential collection dir
  (`name_00.png`, `name_01.png`, …) via `load_collection_frames`.
  - **Why it is inert for projectile bodies:** the collector packs the projectile
    body at **age 0.0** (`particle_render.rs`, both the `Projectile` and the
    `ProjectilePresentation` arms), so `frame_idx` pins to 0 regardless of
    `frame_count`. Flipbook needs (a) an advancing age fed for the body and
    (b) a multi-frame collection registered with a loop cadence.
  - **File-format decision (owner-confirmed): keep the numbered-frames directory.**
    Cross-engine survey of how flipbooks are supplied: (1) *single texture + uniform
    grid* — Unreal SubUV (`SubImages Horizontal × Vertical`), Unity Texture Sheet
    Animation, Godot particle process material; the dominant VFX-flipbook convention.
    (2) *single image + metadata sidecar* — Aseprite / TexturePacker JSON with
    per-frame rects, **variable** per-frame `duration`, and named tags; the only
    camp with non-uniform timing. (3) *individual numbered frames* — Doom/Quake/
    GZDoom sprite lumps and film image sequences. Postretro today is a hybrid:
    authoring is camp 3 (numbered PNGs in `textures/<coll>/<coll>_NN.png`), runtime
    is camp 1 (stitched to a horizontal strip, uniform frame time). The owner chose
    to keep camp 3 for this spec — already built, matches emitters, zero renderer
    change. Moving to a single sprite-sheet + grid (the more universal modern
    expectation) or the Aseprite JSON (variable durations, tags) is an **engine-wide
    sprite-pipeline** change touching every collection, not projectile-scoped — a
    separate future spec. Uniform per-frame duration (chosen cadence) matches the
    Unreal/Unity default; variable per-frame timing only comes with camp 2.
- **No emissive term.** `fs_main` returns `sprite_sample.rgb * lighting *
  opacity` (lighting = ambient floor + SH indirect + SH direct + static specular
  + dynamic diffuse, all per-vertex). A billboard in a dark room goes dark —
  the opposite of a self-lit bolt. The fix is an **unconditional additive**
  self-lit term. Do **not** gate it on `LightTermMask` bit 7: the shipped
  `lighting--per-term-isolation` owner decision (2026-08-10, mirrored in the
  `LightTermMask` doc comment) keeps bit 7 **permanently** unwired because
  emissive lights only itself and is categorically outside the scene-lighting
  isolation set — "not deferred work." The default runtime mask
  `LightTermMask::ALL = 0x7F` has bit 7 clear (guarded by a test asserting
  `ALL.bits() == 0x7F`), so gating emissive on bit 7 would make it **off by
  default** in shipping builds. Emissive's default strength of 0 already makes the
  term inert, giving the "off = byte-identical" guarantee with no gate.
- **Bloom is ready.** §7.8: the bloom compositor extracts HDR luminance above
  `BLOOM_THRESHOLD` and haloes it; `POSTRETRO_BLOOM=0` disables it for the manual
  no-bloom check. An HDR emissive term (strength ≥ ~1) blooms with no extra work.
- **Emissive precedent.** `context/plans/done/emissive-surfaces-bloom/` shipped
  texture-driven `{name}_e.png` **additive HDR** emissive on world +
  kinematic-brush surfaces (`forward.wgsl`, `kinematic_brush.wgsl`). It
  explicitly deferred model/mesh and per-entity/sprite emissive, and "emissive as
  a light source" (no light injection). This spec's billboard emissive follows
  that additive-HDR shape; the dynamic-light capability is the separate,
  genuinely-different "throws light on the scene" feature.
- **Vertex storage-buffer budget.** `vs_main` already reads six VERTEX-visible
  storage buffers against the ceiling of 8 (§7.4). Emissive and flipbook ride the
  group-1 **uniform** `draw_params` (not a storage buffer) and the per-instance
  `age` (already packed), so neither adds a VERTEX storage buffer. The dynamic
  light adds nothing to the billboard pipeline.
- **Level-load registration.** `crates/postretro/src/startup/lifecycle.rs`
  (~line 936) harvests every emitter `sprite`, every projectile body/trail via
  `projectile_presentation_assets(&data_registry.entities).1` (returns
  `{ collection, lifetime }` per projectile sprite), and the impact collection,
  then calls `renderer.register_smoke_collection(collection, frames,
  spec_intensity, lifetime)` + `particle_render.register_sprite(collection)`.
  Projectile bodies today register with `spec_intensity = 0.3` and the
  projectile's `lifetime`; because the body packs age 0.0, that `lifetime` is
  currently dead. Flipbook repurposes the per-collection `lifetime` draw-param as
  the animation loop period.

## The light bridge — why a moving projectile light is real plumbing

`crates/postretro/src/scripting/systems/light_bridge.rs`.

The bridge exists to sync `LightComponent` entities → GPU light buffer. It is
built around **fixed-position map lights** that change only when a script mutates
the component:

- `absorb_dynamic_lights` scans `ComponentKind::Light` each tick, and for any
  untracked id **captures its `LightComponent.origin` once** into
  `cached_origins_f64` and derives an influence sphere once
  (`component_to_influence`), bounded by
  `RUNTIME_DYNAMIC_LIGHT_RESERVE` (`postretro_renderer`).
- `update` packs the GPU light from `component_to_map_light(component,
  cached_origins_f64[idx], …)` — **the cached spawn origin, not the entity
  `Transform`, and not even `component.origin`.** Dirty detection is
  `snapshot.component != current`; a repack still reads the cache.

Consequence, verified: attaching a `LightComponent` to a moving projectile and
updating its `Transform` (or even its `component.origin`) each tick does **not**
move the packed light — the cache wins. A moving gameplay light is a case the
bridge cannot express today. Projectiles are the **first** mover-attached
gameplay light (rendering §4 names "gameplay effects" as a light source, but no
existing one moves).

**Decision (placement).** Teach the bridge a follow-Transform contract rather
than have the projectile stage poke `component.origin` (which the cache would
ignore anyway). A `LightComponent.follow_transform` flag (internal, snapshot-
omitted, default false = current behavior): when set, the bridge resolves the
light's position and influence center from the entity's live `Transform` each
frame and treats Transform movement as a dirty edit. This is the small, cohesive
generalization other moving-light features (muzzle flashes, tracers, thrown
ordnance) reuse; it reverts cleanly (flag defaults false). Projectile lights are
point lights; `component_to_map_light` already forces `casts_entity_shadows =
false` for runtime lights, so projectile lights do not enter the shadow pool
(desirable — many moving shadow casters would be a perf sink).

## Lifecycle — projectile dynamic light (attach → follow → cleanup), all vantages

```mermaid
sequenceDiagram
    participant Fire as Weapon fire (tick N)
    participant Stage as Projectile advance (N+1..)
    participant Bridge as Light bridge (per frame)
    participant GPU as GPU light buffer
    Fire->>Stage: spawn_projectile attaches LightComponent{Point, follow_transform}
    Note over Fire: SP/host: gameplay projectile · client: predicted projectile · host: presentation copy per peer
    Bridge->>Bridge: absorb_dynamic_lights picks up the new Light id (consumes RESERVE)
    loop each frame while in flight
        Stage->>Stage: cur = prev + dir*speed*dt (moves Transform)
        Bridge->>Bridge: follow_transform → position/influence from body-matched pose (raw Transform for sprite, interpolated for model); mark dirty if moved
        Bridge->>GPU: repack moving light
    end
    Stage->>Stage: impact or travel-bound → despawn projectile (LightComponent gone)
    Bridge->>GPU: tracked light disappeared → one tombstone upload (zeroed slot); free-list reclaims the slot
```

Three points the review settled (updated for merged `main`). **(1) Reserve-slot
reclamation is SHIPPED** (`light-bridge-runtime-light-reclamation`, PR #401, now in
`context/plans/done/`). The `update` tombstone branch still zeroes the GPU slot; the
merged bridge additionally pushes the freed runtime slot to `LightBridge.free_slots`
(guarded by a per-slot `MapLightShape.reclaimed`), and `absorb_dynamic_lights` reuses
it via `free_slots.pop()` and bounds new lights on `entity_ids.len() -
authored_light_count - free_slots.len()` — a **live** count, not cumulative. So the
projectile lights' churn no longer exhausts the 256 reserve. This spec consumes that
shipped behavior; it does not implement reclamation. **(2) The follow-Transform light
reads the pose that MATCHES ITS BODY's render path** — a **sprite** body from the
**raw** `Transform` (the un-interpolated value `pack_sprite_instance` packs), a
**model** body from `interpolated_transform(id, alpha)` (the value `mesh_render`
packs at the frame alpha). "Always interpolated" was wrong for sprite bodies (the
reference plasma bolt): a smooth light on a stuttering billboard trails by up to
(1−alpha)·speed·tick_dt. The accessor is not missing — `interpolated_transform` is on
`EntityRegistry` (which `update` already holds), and `frame_result.alpha` is in scope
at the `main.rs` bridge call site; the only new wiring is an `alpha` param on
`update`. The genuinely-new work is the per-id pose read (the loop reads only
`LightComponent` today) + a dirty-on-move comparison, overriding BOTH the packed
position (`cached_origins_f64` via `component_to_map_light`) and the influence center
(`cached_influences[idx]`, seeded once at enrollment via `component_to_influence`)
from the body-matched pose. **(3) `absorb_dynamic_lights` is host-only** (`main.rs`
host/SP tick block); the connected-client branch skips it, so client-side predicted /
presentation / impact lights need a new client-path `absorb` call before the frame's
`update` or they never enroll.

## The impact flash — reuses the impact chokepoint

`crates/postretro/src/weapon/impact.rs::spawn_impact_effect_at(registry, point,
normal)` is the single impact chokepoint: on a real contact the projectile stage
calls it before despawning the projectile. It spawns a 9-particle burst
(`ParticleState` + `SpriteVisual`, `IMPACT_LIFETIME = 0.18s`) that the particle sim
self-expires; its own doc comment says "future data-defined impact descriptors
replace the body of this function." That is exactly where the flash lands: spawn a
stationary `LightComponent` at `point` beside the burst.

The flash is **stationary** (`follow_transform = false` — the bridge's default
origin-driven path already handles a fixed light, no follow-Transform needed). Its
fade reuses the existing `LightAnimation` brightness path — a one-shot
`play_count = Some(1)`, `brightness = [1.0, 0.0]` over the authored `fade` ms; the
light bridge already ramps a `play_count`-bounded animation and settles it. When a
**peak radius** is authored, the same one-shot animation also carries a
`radius = [start, peak]` curve (the new channel below), so the lit area **expands**
over the fade — a shockwave. **Despawn** is the one genuinely net-new bit:
`LightAnimation` completion writes final radiance back and clears the animation but
does **not** despawn the entity, so a faded flash would linger at brightness 0
holding a `RUNTIME_DYNAMIC_LIGHT_RESERVE` slot forever. So the flash schedules a
`DeferredEffectKind::Despawn` at `fade` ms (the `DeferredEffect` queue is
per-entity, awaiting a later tick — the chosen mechanism, not a new sweep). This is
presentation, not AoE: the flash is a light, no overlap query, no splash damage —
the AoE *damage* family stays deferred to its own spec, which composes this same
chokepoint and the radius channel below rather than re-implementing them.

## The radius-animation channel — completes a lopsided capability

`LightAnimation` (`crates/entities/src/components/light.rs`) animates
`brightness`/`color`/`direction` but not radius/falloff-range — an asymmetry with
no principled basis. The growing shockwave needs radius over time, so this spec
completes the channel. **Design: CPU-side in the bridge, no shader change.** The
bridge already recomputes `effective_brightness` **every frame** (it's time-varying
and drives shadow-slot ranking), using the CPU Catmull-Rom mirrors
`sample_brightness_at`/`sample_brightness_at_open`. The radius channel reuses that
per-frame eval **pattern** — but note `effective_brightness` only ranks shadow
slots, it does not pack, so the packing is new. And packing is NOT just "call
`component_to_influence`": in the pack loop, override the `MapLight.falloff_range`
with the current radius **before** `pack_light`, AND override the pushed influence
radius with the current radius (not the `cached_influences[idx]` clone, which is
captured once at absorb). Both must track the animated value. The falloff shaders
read `GpuLight` range unchanged; only the packed number moves, so **no
`forward`/`billboard`/`mesh` shader edit**. Because packing is dirty-gated, a light
with an *active* radius curve marks the bridge dirty each frame — reusing the
per-frame re-pack **Task 3 introduces** for follow-Transform lights (no light
re-packs every frame today; brightness/color animate GPU-side), so no new cost
structure. Overriding the influence radius each frame keeps culling exact as the
light grows (no conservative max-radius needed). The GPU-side alternative (a radius curve in the animation
descriptor, evaluated in every falloff shader) is deliberately not taken: it widens
the change across three shaders and the descriptor layout for brief, small-count
animations. This channel is generic engine-floor lighting — any light can pulse its
radius — and the impact flash is merely its first consumer.

## Vantage × lifecycle cross-product

Same visual state observed from three vantages. "Free" claims carry warrants.

| Vantage | Emissive | Flipbook | Travel light | Impact flash |
|---|---|---|---|---|
| **SP / listen-host** (local gameplay projectile) | draw-param on the registered collection → billboard term | body gets advancing age from the advance stage → animates | `LightComponent` attached at spawn; bridge follows the body-matched pose (raw Transform for sprite, interpolated for model) | impact branch spawns the flash at the hit point (`origin = point`; static or expanding via the radius channel) |
| **Connected client — own shot** (predicted local projectile) | same registered collection, same draw-param → **same** as SP (warrant: identical `register_smoke_collection` path, keyed by collection name) | predicted advance feeds age the same way (warrant: `advance_predicted` reuses the advance body per E16) → animates | client attaches the `LightComponent` locally; the host suppresses this client's **presentation** copy, so no doubled light | the predicted projectile's own impact spawns the flash locally (a local effect, like a muzzle flash) |
| **Remote peer** (host-spawned presentation projectile) | **same** collection draw-param (warrant: the presentation body renders through the same `ParticleRenderCollector`/registered collection) → **free** | **not free** — the presentation arm also packs **age 0.0** (`particle_render.rs`); needs the same age feed → Task 2 covers both projectile columns | `projectile_presentation.rs::attach_projectile_visual_components` (which `remote_materialize.rs` delegates to) attaches a `LightComponent{follow_transform}` client-side from the shared descriptor (like the body/trail); it tracks the body-matched pose and needs the new client-side `absorb_dynamic_lights` call to enroll at all (absorb is host-only today) | the presentation projectile spawns a flash at its **despawn point** (`projectile_presentation.rs::advance`, threading the config via `PresentationFlight`) (approximate — deterministic straight-line, not the exact resolved hit); in scope, an accepted presentation-only asymmetry (per the shipped remote-observer aim precedent) |

## Orderings that actually occur

Captured as spec rows (`index.md` Orderings). Key ones:

- Body flipbook with `frame_count == 1` (single-PNG sprite) or no cadence
  authored → static frame 0 (byte-identical to current); the shader already
  clamps `frame_count` to `max(_, 1)` and `frame_duration` to `1e-6`.
- Projectile whose whole flight is shorter than one frame duration → shows only
  frame 0 (acceptable).
- Light entity despawns on the impact tick → the same-frame bridge pass sees the
  component gone and tombstones (zeroes) the GPU slot; the reserve entry is
  reclaimed by the prerequisite spec, not this path.
- N *concurrently-live* projectile lights, N > `RUNTIME_DYNAMIC_LIGHT_RESERVE` → the
  bridge warns once and later lights don't render (graceful; not a crash).
  *Cumulative* churn exhaustion is the prerequisite's problem, fixed by reclamation.
- Impact flash spawns **only** on a real contact, never on travel-bound expiry; it
  fades (and expands, if a peak radius is authored). The `play_count` settle fires
  ~1 tick BEFORE the `DeferredEffect` despawn (despawn counts from the next tick);
  the settled light is intensity-0 (invisible), then the despawn removes it and the
  slot is reclaimed (prerequisite) — no visible zero-brightness linger.
- A radius curve grows the packed range and the influence cull radius in lockstep
  each frame; a `None` radius curve leaves both at the static `falloff_range`.
- Emissive default (strength 0), flipbook default (single frame), no travel light,
  no impact light, and no radius curve must all be byte-identical to current output
  (regression rows).

## Direction exercise (Q1–Q6, solo)

1. **Problem (cause).** Shipped projectile bodies are lit billboards (or rigid
   models) with no self-illumination, no body animation, no cast light, and no
   impact flash. Four identity boomer-shooter looks are unreachable: a full-bright
   plasma bolt (a billboard *darkens* in a dark room — inverted from a glowing
   bolt), an animated/pulsing bolt (body pinned to frame 0), a projectile that
   throws moving light on nearby walls, and a bright pop of light where it lands
   (the impact chokepoint spawns only a particle puff). Observed against a
   cyberpunk aesthetic that explicitly wants "billboard sprite volumetrics that
   react to light" + emissive/bloom.
2. **Right level.** Authoring: descriptor-owned tuning on the weapon (no FGD
   KVPs), mirroring the shipped projectile visual and the primitive-surface
   contract. Rendering: the billboard shader owns the emissive term and the
   flipbook already lives there; the light system (engine-floor bridge) owns the
   dynamic light. The one non-obvious placement is the **moving light** — placed
   in the light bridge as a follow-Transform contract, not as per-stage origin
   pokes (the bridge caches origin and would ignore them). Recorded because a
   drafter who mis-placed it would not know: the bridge's cache-origin design is
   the reason the naive placement fails.
3. **Foreclosures.** Emissive as a **per-collection** scalar (draw-param.w)
   forecloses per-instance emissive variation (e.g. a bolt that dims with age)
   until a per-instance channel is added — additive later; the instance layout
   has a spare `_pad`. Reusing `draw_params.params.w` spends the last free slot
   in that vec4; a second per-collection scalar later needs a new uniform field.
   The radius channel is CPU-side (bridge-local), so it forecloses nothing on the
   shaders and reverts cleanly (`None` curve = today's static range); the GPU-side
   variant stays available if a future high-count/long-lived radius animation ever
   needs it. Keeping the numbered-frames flipbook convention forecloses nothing (a
   sprite-sheet-grid / metadata-JSON format stays an independent engine-wide
   option). All named, all cheap to reverse.
4. **Prior commitments touched.** No-double-count lighting invariant — emissive
   is a self-only additive term (unconditional, ungated by the light-term mask per
   the `lighting--per-term-isolation` owner decision), the travel and impact lights
   are ordinary additive dynamic-tier lights; none re-weights another term.
   Descriptor-owned tuning + primitive-surface contract (SDK + validation move in
   the same pass). The impact flash reuses the single impact chokepoint
   (`spawn_impact_effect_at`) and the existing `LightAnimation` fade path. Vertex
   storage-buffer ≤ 8 (unaffected). `RUNTIME_DYNAMIC_LIGHT_RESERVE` (both light
   kinds consume it). Co-op no-wire-change invariant (all four are descriptor
   content / client-materialized / local effects).
5. **One-way door?** No. Each capability is an additive descriptor field + a
   removable shader term / component attach. Bit 7 stays unwired. The
   follow-Transform flag defaults false. The impact flash's transient-despawn is
   self-contained.
6. **Strongest alternative.** (a) Model-with-`_e.png` for the glow instead of a
   billboard emissive term — rejected: model emissive is deferred, models are
   heavier, and the billboard bolt is the genre idiom; the billboard term is
   cheap and reuses the bloom pipeline. (b) Author the lights as a script reaction
   on projectile spawn/impact — rejected: projectile spawn/impact are engine-floor
   (`fire_hitscan` → `spawn_projectile`; the stage's impact branch), not
   per-projectile script-reachable, and the follow-Transform plumbing is needed
   regardless; descriptor-declared + attached-at-spawn mirrors the body/trail.
   (c) Animate via a trail emitter only, no body flipbook — rejected: a
   pulsing/spinning bolt body is the ask. (d) Ship a **static-only** impact flash
   and defer the growing shockwave — rejected under "build more right faster": the
   destination (an expanding shockwave) is clear, the radius channel is patterned
   after the existing brightness channel, it's cheap at runtime (one CPU curve eval,
   no shader change), and it completes a lopsided engine capability — so deferring
   would fragment the build for no runtime saving. The distinction that flipped this:
   *leanness* is a runtime property (tiny binary, effects used sparingly on screen),
   not a build-sequencing rule; using it to defer a clear-destination, runtime-cheap
   feature was a category error. Leanness still argues for the CPU-side channel over
   the wider GPU-side one, and against building things with no consumer (per-instance
   emissive, script-authored radius curves) — which stay deferred.
