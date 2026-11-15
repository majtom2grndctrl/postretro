# area-lights

Brief · resumable · reads: `context/lib/build_pipeline.md` §Compiler pipeline step 12, §Build Cache, §Custom FGD · `context/lib/rendering_pipeline.md` §4 · `context/lib/resource_management.md` §Emissive · research: `research.md` · read at 17c2f1e

## Problem
The owner wants a static light that is a surface. A designer building a big
ceiling skylight or a glowing light-table floor has no way to make it light the
room: emissive maps light nothing, and only `neon_` textures even glow (every
other prefix has zero emissive strength). The workaround is a grid of point
lights, which leaves hot spots on nearby surfaces. Under a 4 × 4 m panel a single
point reads 6.1× too bright at 1 m (`research.md` §1). The grid also loses
specular past the four shadowmask owners per face, and every copy adds lightmap
bake cost. When this is done, a designer can draw a brush entity, give its
emitting faces an `_e` map, and set a colour and brightness. Those faces glow and
light the room as one soft-shadowed area light: in the lightmap, in bounce, and
on entities. A script can pulse or recolour it, and the glow moves with the
light.

## Decisions
- **A polygon area light, not template lights.** A new light type whose emitter
  is the set of convex polygons formed by its entity's emitting faces. This
  diverges from the roadmap "surface lights" entry (copies of a light templated
  across every face of a named texture). It also diverges from the gi-rtx-lite
  ruling "Emissive bounce: opt-in per material", because opt-in moves from the
  material to the placement. Both records are updated with this brief. Precedent:
  id q3map 1999, Source 2 `light_rect`, Godot 4.7 (`research.md` §5).
- **Authoring: a brush entity, `light_area`.** Its brushes compile as static
  world brushwork, like `switch` and `func_group`.
  - Emitting faces are the exposed brush sides whose texture has an `_e` map,
    taken before BSP splitting. An entity with no such face is a compile error.
  - Emission is one-sided, along each face's outward normal.
- **Brightness is per area.** On the emitter's axis, a 1 m² panel matches a
  `light` point light of the same `light` value at the same distance. A larger
  brush gives proportionally more light, as in every area-scaled tool
  (`research.md` §5).
  - The entity carries its own `_color` and `light`.
  - It never reads `Material::emissive_strength` or a texture prefix, since
    `neon_` is being retired separately.
- **Reach is derived, with an override.** Reach is the distance from the
  polygon at which the bound E ≤ L·A/d² falls below a fixed cutoff. Nothing
  beyond it is lit. `_falloff_range`, measured from the panel, overrides it.
  `delay` (falloff model) does not apply. Reach is measured from the polygon,
  never from its centre (`research.md` §4).
- **Unshadowed term: exact.** Lambert's polygon irradiance is clipped to the
  receiver's horizon. The stored direction is the clipped polygon's irradiance
  vector. Both are computed in f64, with the robust edge form (`research.md` §1).
  A receiver on or behind the emitter's plane gets zero, so the emitter's own
  faces take no direct light from it.
- **Shadows: ratio estimator in the existing separable fold.** Stored visibility
  is Σ wV / Σ w over samples spread across the polygons by solid angle
  (Heitz–Hill–McGuire 2018). The `lightmap_layer` partition record does not
  change. Fully lit and fully shadowed texels stay exactly 1 and 0. An unweighted
  clear-ray fraction is wrong near a large emitter, which is the case this light
  exists for (`research.md` §2).
- **Probe bakes project over the polygon, not along one direction.**
  - This covers direct SH (ids 35, 41, 45), billboard scatter (ids 47, 48) and
    the mover probe channel.
  - The unshadowed coefficients are analytic. Shadowing is a control-variate
    term with all-visible and none-visible fast paths. There is never a
    per-coefficient ratio (`research.md` §3).
  - The bounce comes from the light's own surface-radiance patch layer, as for
    every light. Its glow is never also added to the radiance cache, which keeps
    the no-double-count invariant (`context/lib/index.md` §2).
- **Culls stay sphere-shaped in this brief.** Every reach, shaft and influence
  cull receives a sphere that contains the polygons plus their reach. That keeps
  every exact-skip proof true, so no output byte depends on a cull
  (`build_pipeline.md` §Compiler pipeline step 12, Exact skips). Tight polygon
  hulls are a follow-on, built only if the bake-time row asks for them.
- **Flush emitters are expected.**
  - Buried-light classification and the embedded-emitter test do not apply to
    this type.
  - Shadow rays must not be blocked by the emitter's own faces.
- **Glow follows the light.**
  - Each emitting face draws `_e` × `_glow` × the light's live brightness and
    colour multipliers, on the same frame as its lighting.
  - `_glow` is an authored number, independent of `light`, so a game can pair
    a dim glow with strong cast light, or the reverse.
    - Default 4.0, today's `neon_` look, above the bloom threshold.
    - Any value ≥ 0 is accepted, with no upper clamp; 0 is a light with
      unlit faces.
    - A negative value is a compile error.
    - No preset names and no prefix lookup (`context/lib/index.md` §2,
      Defaults for novices, controls for experts).
  - The id → runtime table follows the `animated_block` precedent.
  - Glow is drawn even when the light is static.
- **Script surface is the existing light family.** Area lights appear in
  `getMapEntities("light")` and take brightness and colour animation, including
  the compile-time animated-bake reservation. A direction channel is refused at
  compile time. Co-op replication of light animation is unchanged.
- **v1 refusals.**
  - No area-light specular. The light keeps a placeholder `SpecLight` record and
    is never a shadowmask owner.
  - No entity-shadow promotion on the base or the animated path.
  - `_shadow_type sdf`, `_light_size`, `direction_curve`, `style` and spot keys
    are compile errors.
- **Wire: the AlphaLights section (id 18) bumps to v5.**
  - A new type value, and the polygon list after the records.
  - The polygon list is enough to rebuild a later specular term (LTC or
    representative point, `research.md` §6) without another bump.
  - LightInfluence stays a bounding sphere.
- **Non-goals.**
  - Runtime specular, promotion, SDF shadows, two-sided emitters, tight cull
    hulls.
  - Template `_surface` lights: this brief replaces that roadmap entry.
  - Retiring `neon_`, which is separate work.
  - Texture-coloured emission: the light's colour is the entity's `_color`,
    not the `_e` texels.

### Scripting surface
No new names. An area light is a light member:

```ts
const panels = getMapEntities("light", { tag: "skylight" });
defineReaction("levelLoad", {
  sequence: panels.flatMap((p) => p.pulse({ min: 0.3, max: 1.0, periodMs: 900 })),
});
// p.position is the area-weighted centroid of the emitting faces.
// p.sweep(...) on an area light fails the level bake by name.
```

## Acceptance
### Automated
Polygon term (oracle values in `research.md` §1):
- [ ] A unit square, L = 1, facing down, with receivers facing up at h = 0.1,
  0.5, 1, 2, 5 and 10, matches the closed-form centred-square value to 1e-5
  relative.
- [ ] Tilted receivers at h = 1 match the table, including exactly 0 at 120°.
  That case fails without the horizon clip.
- [ ] Off-axis and edge-line receivers match the table, with no NaN.
- [ ] A grazing receiver (0.1 mm below the plane) is finite and matches the
  table.
- [ ] Coplanar, behind-the-emitter and emitter-face receivers read 0. A receiver
  1 mm under the centre reads within 1e-4 of π.
- [ ] Small-emitter limit: the ratio to L·A·cosθ_e·cosθ_r/d² is within 1e-3,
  1e-5 and 1e-7 at s = 0.1, 0.01 and 0.001. On the axis, a 1 m² panel equals the
  point term of the same `light` value.
- [ ] f32 corners about 1 km from the origin agree with f64 corners.

Shadows and probes:
- [ ] Unoccluded and fully occluded texels store exactly 1 and 0.
- [ ] An occluder over the near half of a large emitter and one over the far
  half store different visibility, ordered by integrand weight. An unweighted
  fraction ties them.
- [ ] Unoccluded probe coefficients match the analytic L0 (solid angle) and L1
  (irradiance vector).
- [ ] 4 × 4 m panel, probe 1 m below: the side-facing reconstruction is within 5%
  of 0.781, and the down-facing one within 0.05 of 0. The single-direction
  projection gives 0.230 and 0.154.

Placement, identity and refusals:
- [ ] In one entity holding a frame brush and an `_e` glass brush, only the
  glass's exposed faces emit. The entity's brushes block movement and cast
  shadows like worldspawn.
- [ ] With `_falloff_range` set, a receiver just past it, measured from the
  panel, is unlit, and one just inside is lit. Without the key, reach follows
  brightness: doubling `light` lengthens it.
- [ ] An area light on a ceiling is neither dropped as buried nor flagged
  embedded. A point light buried in solid is still dropped.
- [ ] A wall texel at the emitter's edge is lit, not black.
- [ ] Every bake output is byte-identical with exact skips on and off on an
  area-light map.
- [ ] Moving one polygon vertex re-keys the lightmap layer, the surface-radiance
  layer and the direct-SH memos. An unrelated light's cache entries do not move.
- [ ] Each of these fails the build, naming the light: an entity with no `_e`
  face, `_shadow_type sdf`, `direction_curve`, and a script `sweep`.
- [ ] Promotion selects no area light on either path. The placeholder
  `SpecLight` is never an id-42 owner.
- [ ] An animated area light's weight map at curve 1 equals its static bake.
- [ ] AlphaLights v5 round-trips area and non-area records. A v4 file is
  rejected by version.
- [ ] The Scripting surface example runs as a `content/dev` script: the pulse
  reaches the light and its glow.
- [ ] A static area light's `_e` faces glow on a texture without the `neon_`
  prefix, at `_e` × `_glow`. With `_glow` at 0, 0.5 and 8, the face's emissive
  term scales by that factor, and the room's light does not change. A negative
  `_glow` fails the build. A `_start_inactive` light's faces start dark.

### Manual
- [ ] Hallway map, recorded in `plan.md`: cold and warm bake time for one 4 × 4 m
  skylight, against the same patch lit by a 4 × 4 grid of point lights.
- [ ] Hallway map, recorded in `plan.md`: penumbra RMS noise against a 256-ray
  reference at 32 and 64 escalated rays, 1–3 m from the panel. The chosen count
  is recorded.
- [ ] Owner A/B: skylight room against the template grid. No hot spots or wall
  ripple, and no visible edge at the reach cutoff.
- [ ] A scripted flicker: the glow and the room light move together frame for
  frame.
- [ ] Entities walking under the skylight are lit like the walls beside them.

## Path
- **Chokepoints.** Add the polygon arm at these, by symbol:
  - Lightmap and shadowmask: `lightmap_bake::{light_contribution_and_direction,
    emitter_radius, hard_ray_target, area_sample_target, probe_sample_direction,
    emitter_lattice_spin, SoftProbes::new}`, `texel_footprint::EmitterKey`.
  - Probes: `sh_bake::light_eval::{incident_radiance_at_point,
    light_contribution_lambert, bounce_contribution}`,
    `sh_bake/probe.rs::bake_probe_direct_rgb_posed`.
  - Mover probe channel: `mover_probe_channel/{direct,visibility,posed_scene}`.
  - Billboard scatter: `billboard_direct_scatter_bake`.
- **Containing sphere.** One reach-sphere accessor, read by every
  `falloff_range` cull reader (`research.md` §Grounding lists them). It is
  cheaper and safer than editing each one by hand.
- **Lattice snap.** The polygon lattice depends on the light and the receiver.
  Key it apart from the count-keyed point/spot snap in `SoftProbes::new`.
- **Self-occlusion.** Lift sample points off the plane (q3map, qrad and
  ericw-tools precedent), or exclude the emitter's faces from the hit test.
  First check how `segment_clear` treats an endpoint lying on a triangle.
- **Cache digest.** Add the polygon to `surface_radiance/layer.rs::LightDigest::of`
  by hand. The lightmap key already hashes the whole `MapLight`.
- **Promotion.** Animated promotion
  (`animated_baked_shadow_candidates_with_direct_delta`) needs an explicit type
  refusal. `is_promotable_base_light` already has one.
- **Glow table.** It must be a uniform, not a texture: the forward pass is at
  16/16 textures.
- **Strongest rival:** texture-keyed polygon lights, which use the roadmap's
  mark-a-texture-once authoring with this brief's maths. They win for many small
  emitters (screens, signs). The entity wins for the Problem's set pieces: one
  light per panel, a script handle, and its own colour and brightness.
- **Template point lights** lose on hot spots and per-copy bake cost
  (`research.md` §1). They keep up to four specular owners per face, which v1
  area lights give up.
- **First slice.** Polygon term plus ratio-estimator visibility in the lightmap
  only, on a test map: the riskiest maths, behind the analytic and visibility
  rows. Then probes, glow, wire and refusals.
- **Cross-check.** Godot 4.7's `lm_area_lights_inc.glsl` (MIT) is a cheap
  cross-check of the clipped polygon term. Its centre-aimed shadows are not this
  design.

## Open questions
- Patent search on polygon area-light baking and on ratio-estimator visibility,
  charted per `context/research/gi-rtx-lite/ip-check.md`. Prior art only so far
  (`research.md` §5). — owner: owner / counsel — **blocks build**
- Reach cutoff constant: no visible edge in the owner A/B. — **delegated**
- Escalated sample count for area lights, from the noise row. — **delegated**
- Self-occlusion mechanism: lift or exclude. — **delegated**

## Boundary inventory

| Name | Rust | Wire | FGD KVP | Docs |
|---|---|---|---|---|
| Area light classname | adapter-recognised brush entity | n/a | `light_area` (`@SolidClass`) | `docs/level_design.md` lights |
| Light type | `LightType::Area` | `AlphaLightType` value 3 | n/a | n/a |
| Brightness, colour, override reach | `MapLight` intensity, color, falloff_range | existing record fields | `light`, `_color`, `_falloff_range` | same |
| Animation keys | existing | existing | `brightness_curve`, `color_curve`, `period_ms`, `_curve_phase`, `_animated`, `_start_inactive`, `_bake_only`, `_tags` | same |
| Glow strength | per-entity f32 | glow table (not AlphaLights) | `_glow(float)`, default 4.0 | same |

## Wire format
AlphaLights (id 18) v5, little-endian throughout, mirroring v4:
- Version u32, then light count u32, then fixed 73-byte records with the v4
  field order unchanged.
- An area record:
  - `light_type` = 3
  - `origin` = area-weighted centroid
  - `falloff_range` = resolved reach
  - `falloff_model` = 2
  - cone fields zero
  - `shadow_type` = 0
- After the records: area count u32, then, per area light in record order:
  - record index u32
  - polygon count u32
  - per polygon: vertex count u32 (≥ 3), then vertices as f32 × 3 in emitting
    winding (counter-clockwise seen from the lit side)
- An empty list is a zero area count.
- A loader rejects a non-3 record with an entry, a type-3 record without one,
  and any version other than 5.
