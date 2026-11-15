# area-lights

Brief · resumable · reads: `context/lib/build_pipeline.md` §Compiler pipeline step 12, §Build Cache, §Custom FGD · `context/lib/rendering_pipeline.md` §4 · `context/lib/resource_management.md` §Emissive · research: `research.md` · read at 17c2f1e

## Problem
The owner wants a static light that is a surface. A designer building a big
ceiling skylight or a glowing light-table floor has no way to make it light the
room: emissive maps light nothing, and only `neon_` textures even glow (every
other prefix has zero emissive strength). The workaround is a grid of point
lights, which leaves hot spots on nearby surfaces. Under a 4 × 4 m panel a single
point reads 6.1× too bright at 1 m (`research.md` §1), and every copy adds
lightmap bake cost. When this is done, a designer can draw a brush entity, give
its emitting faces an `_e` map, and set a colour and brightness. Those faces glow
and light the room as one soft-shadowed area light: in the lightmap, in bounce,
and on entities. A script can pulse or recolour it, and the glow moves with the
light.

## Decisions
- **A polygon area light, not template lights.** A new light type whose emitter
  is the set of convex polygons formed by its entity's emitting faces
  (precedent: `research.md` §5). `light_area` is the type's first front end. The
  roadmap "surface lights" entry becomes a texture-keyed front end on this type,
  for screens, signs and lava. It also takes over the texture-level bounce
  multiplier, and must reconcile it with "Albedo is physical"
  (`build_pipeline.md` §Compiler pipeline).
- **Authoring: a brush entity, `light_area`.** Its brushes compile as static
  world brushwork, like `switch` and `func_group`. Its emitting faces are the
  exposed brush sides whose texture has an `_e` map. Emission is one-sided,
  along each face's outward normal. An entity with no emitting face fails the
  build.
- **Brightness is per area.** A larger brush gives proportionally more light, as
  in every area-scaled tool (`research.md` §5). Before the window, far from the
  panel on its axis, a 1 m² panel matches an inverse-squared (`delay 2`) point
  light of the same `light` value; close in, it is dimmer, which is the point.
  The entity carries its own `_color` and `light`, and its light never reads a
  material's emissive strength or a texture prefix.
- **Reach is derived, with an override, and light fades to it.** Reach R is the
  distance from the polygons at which the bound E ≤ L·A/d² falls to 1/256 of
  what a `light 300` point light gives at 1 m. `_falloff_range`, measured the
  same way, overrides it. The term is scaled by the window max(0, 1 − (d/R)⁴)²,
  with d the distance to the nearest polygon (Karis 2013), so light reaches zero
  at R with no visible edge. A hard cut is either visible or reaches hundreds of
  metres (`research.md` §4).
- **Unshadowed term: exact, then windowed.** Lambert's polygon irradiance,
  clipped to the receiver's horizon, with the clipped polygon's irradiance
  vector as the stored direction (`research.md` §1). A receiver on or behind the
  emitter's plane gets zero, so the emitter's own faces take no direct light
  from it.
- **Shadows: integrand-weighted visibility in the existing separable fold.**
  Each shadow sample counts by its share of the unshadowed light, never as a
  plain clear-ray fraction, which is wrong near a large emitter (`research.md`
  §2). The `lightmap_layer` partition record does not change. Area lights take
  the same sample counts as every light: `--soft-shadow-samples` on texel paths,
  the fixed default lattice on probe, patch, scatter, mover and bounce paths.
  - The weighting is the Heitz–Hill–McGuire ratio estimator over solid-angle
    samples (`research.md` §2: equal noise to projected-solid-angle sampling;
    one sampler serves texels and probes).
- **Probe bakes project over the polygon, not along one direction.** This covers
  direct SH, billboard scatter and the mover probe channel. Unshadowed
  coefficients are analytic; shadowing never applies a per-coefficient ratio
  (`research.md` §3).
  - A static area light bounces through its own surface-radiance patch layer, as
    every static light does. An animated one bounces through the per-hit delta
    bake (id 27), which gains a polygon arm: one solid-angle sample per hit,
    scaled by its weight over the analytic mean weight, so the delta stays
    unbiased. Glow never enters the radiance cache, which keeps the
    no-double-count invariant (`context/lib/index.md` §2).
  - The gi-rtx-lite ruling "Emissive bounce: opt-in per material" stays open.
    The record gains one line: `light_area` faces are excluded from any emissive
    bounce, since their light already bounces.
- **Culls take two spheres.** Shadow-ray and occluder tests that bound the
  emitter get the emitter sphere: the polygons' centroid and circumradius. Reach
  and influence culls get the reach sphere: the same centre, with the reach
  added to the radius. Both contain what they stand for, so every exact-skip
  proof holds and no output byte depends on a cull (`build_pipeline.md`
  §Compiler pipeline step 12, Exact skips). Tight polygon hulls are a follow-on,
  built only if the bake-time row asks for them.
- **Flush emitters are expected.** Buried-light classification and the
  embedded-emitter test do not apply to this type. Every shadow-ray target, the
  receiver floor's centre probe included, lies on an emitting polygon whose
  front half-space holds the receiver, lifted off it by the tracer tolerance; no
  sample is drawn on a polygon the receiver is behind. No shadow ray aims at the
  light's origin, which can sit inside a pillar or on the glass, and none is
  blocked by the face it targets. Another emitting face of the same light may
  shade it, as a hanging panel shades a ceiling panel above it.
- **Glow follows the light.** Each emitting face draws `_e` × its texture
  strength × `_glow` × the light's live colour × its live brightness multiplier,
  on the same frame as its lighting. At rest, the live colour is `_color`, so a
  red light's white pane glows red. Glow is drawn for static lights too.
  - Builds on `plans/ready/emissive-volumes`, which retires the `neon_` prefix:
    texture strength is the author's per-texture value from `materials.toml`
    (1.0 by default), and `_glow` scales it per light. This brief builds after
    that one.
  - `_glow` is an authored multiplier, default 1.0, independent of `light`, so a
    game can pair a dim glow with strong cast light, or the reverse
    (`context/lib/index.md` §2, Defaults for novices, controls for experts).
    Values ≥ 0 are accepted with no upper clamp; a negative value fails the
    build.
  - The light is its faces' one glow controller: an `emissive_volume` skips
    `light_area` faces. Both briefs share one per-face glow table.
- **`_start_inactive` reserves the animated bake.** On an area light, the key
  means the light starts dark, faces and room together, and a script or mod
  reaction can light it. It takes the same animated reservation `_animated 1`
  does. Point lights keep today's rule.
- **Script surface: the existing light family, plus one type name.** Area lights
  appear in `getMapEntities("light")` with brightness and colour animation and
  the script-derived animated reservation. `lightType` reports `"Area"`,
  `falloffModel` reports `"InverseSquared"`, and `position` is the area-weighted
  centroid. A direction channel (`sweep`) on an area light is refused at runtime
  with `InvalidArgument`, as `setLightAnimation` refuses other invalid input.
  Co-op replication of light animation is unchanged.
- **Keys: inert keys warn, contradictory keys fail.** Changing a `light` to
  `light_area` in the editor keeps its keys. So `delay`, `_light_size` and the
  spot keys warn, naming the light and key, and are ignored. `_shadow_type sdf`,
  `direction_curve` and `style` request behaviour this light cannot give, and
  fail the build.
- **v1 refusals.** No area-light specular: the light keeps an inert placeholder
  `SpecLight` record, is never a shadowmask owner, and adds no specular on any
  path. Runtime specular is a recorded door (`research.md` §6), deferred until
  the diffuse light has shipped and been judged. No entity-shadow promotion on
  either path, because promotion shadows from a point, and a point shadow from a
  4 m panel is wrong.
- **IP: build on the first-pass check, stop on a finding.** The first pass
  (`ip-check.md`) found no filing whose claim elements all appear present in
  the design; every claim of the nearest family requires voxel cone tracing. Its
  follow-up list and counsel's reading run beside the build and report to the
  owner. A filing whose elements all appear present stops the build for an
  owner decision. Image quality and bake cost, not patent distance, choose the
  design.
- **Wire: AlphaLights (id 18) v5 adds one type value.** No polygon payload ships
  until a runtime consumer reads it. A later specular feature takes its own
  bump.
- **Non-goals.**
  - Two-sided emitters: none of the motivating content needs one.
  - Texture-keyed emission and the texture-level bounce multiplier: the reworded
    roadmap entry owns them.
  - Emissive bounce from ordinary emissive faces, and glow outside `light_area`
    faces: `plans/ready/emissive-volumes` owns it.
  - Texture-coloured emission. The cast light's colour is `_color`, so one
    texture can serve lights of different colours.

### Scripting surface
One new name: `"Area"` in `LightKind`, in both `postretro.d.ts` and
`postretro.d.luau`. An area light is otherwise a light member:

```ts
import { defineReaction, getMapEntities } from "postretro";

export function setupLevel(_ctx: unknown) {
  const panels = getMapEntities("light", { tag: "skylight" });
  return {
    reactions: [
      defineReaction("levelLoad", {
        sequence: panels.flatMap((p) => p.pulse({ min: 0.3, max: 1.0, periodMs: 900 })),
      }),
    ],
  };
}
// p.component.lightType === "Area"; p.position is the emitting faces' centroid.
// p.sweep(...) on an area light fails with InvalidArgument.
```

## Acceptance
### Automated
Polygon term (oracle values in `research.md` §1):
- [ ] Every receiver in the §1 table matches to 1e-5 relative: on-axis heights
  against the closed-form centred square, tilts including exactly 0 at 120°
  (which fails without the horizon clip), off-axis and edge-line receivers with
  no NaN, and the grazing receiver finite.
- [ ] Coplanar, behind-the-emitter and emitter-face receivers read 0. A receiver
  1 mm under the centre reads within 1e-4 of π.
- [ ] Small-emitter limit: the ratio to L·A·cosθ_e·cosθ_r/d² is within 1e-3,
  1e-5 and 1e-7 at s = 0.1, 0.01 and 0.001, with the window off. On the axis, 4
  m from a 1 m² `light 300` panel, the resolved reach is 16 m within 1e-4
  relative and the lit value is 0.972 of an inverse-squared point light of the
  same `light` value, within 0.1%; at 1 m it is lower.
- [ ] f32 corners about 1 km from the origin agree with f64 corners.

Shadows and probes:
- [ ] Unoccluded and fully occluded texels under a flush `light_area` panel
  store exactly 1 and 0.
- [ ] An occluder over the near half of a large emitter and one over the far
  half store different visibility, ordered by integrand weight. An unweighted
  fraction ties them.
- [ ] Unoccluded probe coefficients match the analytic L0 (solid angle) and L1
  (irradiance vector).
- [ ] 4 × 4 m panel, probe 1 m below: the side-facing reconstruction is within
  5% of 0.781, and the down-facing one within 0.05 of 0. The single-direction
  projection gives 0.230 and 0.154.
- [ ] For a light wrapping all four sides of a pillar and for a flush ceiling
  panel, a texel facing an emitting face with nothing between stores visibility
  exactly 1, and behind a post that blocks every ring probe's sample but not the
  whole face it stores above 0 (research.md O19).
- [ ] A texel north of a light wrapping all four sides of a pillar, with nothing
  between, stores visibility exactly 1 and matches the unoccluded polygon term
  within 1% (research.md O20).
- [ ] Billboard scatter and the mover probe channel each project a 4 × 4 m panel
  over the polygon: at a probe 1 m below, the side-facing value is within 5% of
  0.781.

Placement, culls and keys:
- [ ] In one entity holding a frame brush and an `_e` glass brush, only the
  glass's exposed faces emit. The entity's brushes block movement and cast
  shadows like worldspawn.
- [ ] With `_falloff_range` set shorter than the derived reach, a receiver past
  it, measured from the panel, is unlit, one at half of it reads 0.879 of the
  unwindowed term, and the light's falloffRange, reach sphere and layer key use
  it (research.md O22). Without the key, doubling `light` lengthens reach by √2,
  and along the axis the lit value falls continuously to 0 at reach.
- [ ] On the axis of a 1 m² `light 300` panel at half its reach, the lightmap
  texel, the animated weight map at curve 1, the surface-radiance patch, the
  direct-SH probe's L0, billboard scatter and the mover probe channel each read
  0.879 of their unwindowed term within 1e-3, and each reads 0 at reach.
- [ ] A wall within reach of a panel's corner, but beyond reach of its centroid,
  is lit at the windowed polygon term within 1e-5 relative in the lightmap, its
  nearest direct-SH probe and its surface-radiance patch (research.md O21).
- [ ] An area light on all four sides of a pillar, with its centroid inside the
  pillar, is neither dropped as buried nor flagged embedded. A point light
  buried in solid is still dropped.
- [ ] A wall texel at a flush emitter's edge stores visibility 1 and matches the
  unoccluded polygon term within 1%.
- [ ] Every shadow-ray target the polygon lattice yields, and the receiver
  floor's centre probe target, after any lift, lies inside the light's emitter
  sphere, for a 4 × 4 m ceiling panel, a 4 × 0.1 m strip and a light wrapping
  all four sides of a pillar.
- [ ] Under a flush ceiling panel and beside a light wrapping all four sides of
  a pillar, with nothing between, the direct-SH probe's L0, the surface-radiance
  patch, billboard scatter and the mover probe channel each match their
  unoccluded polygon term within 1%.
- [ ] A door mover between a light wrapping all four sides of a pillar and the
  floor it lights, and one under a flush ceiling panel, is gated against that
  light: the mover light gate counts texels and probes it shades above zero.
- [ ] Every bake output is byte-identical with exact skips on and off on an
  area-light map, and the shaft cull still clears tiles there: its cleared-tile
  count is above zero.
- [ ] Moving one polygon vertex re-keys the lightmap layer, the surface-radiance
  layer and the direct-SH memos. An unrelated light's cache entries do not move.
- [ ] A 4 × 4 m panel built from four 2 × 2 m brushes lights every receiver
  within 1e-5 relative of the same panel as one brush, and resolves the same
  reach (research.md O6).
- [ ] Two overlapping coplanar `_e` brushes emit their union once, and an `_e`
  face half buried in a worldspawn wall emits from its exposed half only: the
  light's polygon area equals the visible glass area (research.md O7).
- [ ] A sliver emitting piece is dropped, a texel whose samples all lie on its
  horizon stores 0, and no bake output holds a NaN. An entity left with only
  sliver pieces fails as having no `_e` face (research.md O8).
- [ ] Moving a wall that lies within reach of a panel's corner, but beyond reach
  of its centroid, re-keys that light's lightmap layer, and the warm rebuild
  matches a cold one byte for byte (research.md O10).
- [ ] An area light authored between two point lights keeps its entity-order
  position in `getMapEntities("light")` and in every light namespace
  (research.md O11).
- [ ] An `_e` side the BSP splits glows on every piece and enters the light's
  polygon list once. A frame face sharing an edge with the glass carries no glow
  id (research.md O12).
- [ ] On one thread, a point light's probe set is the same whether or not an
  area light's probe set was built first at the same count (research.md O9).
- [ ] An entity with no `_e` face, `_shadow_type sdf`, `direction_curve` or
  `style` fails the build, naming the light and key. `delay`, `_light_size` and
  each spot key warn, naming the light and key, and leave every bake byte as if
  absent.
- [ ] Editing only `delay`, `_light_size` or a spot key on an area light re-keys
  no bake cache entry: a warm rebuild reads every lightmap, surface-radiance and
  SH entry from the cache.
- [ ] Promotion selects no area light on either path, though the animated area
  light is in section 45's direct-delta set and an animated spot beside it is
  selected. The placeholder `SpecLight` is never an id-42 owner, even on a face
  it alone lights. World faces beside the panel take no specular from it, nor do
  billboards with billboard scatter withheld.
- [ ] An animated area light's weight map at curve 1 equals its static bake.
- [ ] On a map with `_bounce_fill 0`, an animated flush ceiling panel's SH
  bounce delta at curve 1, averaged over the probes in its reach, matches the
  same light baked static within 2%.
- [ ] At one unoccluded bounce hit 1 m from a 4 × 4 m panel, the polygon arm's
  one-sample visibility, averaged over every lattice index and 64 seeds, is 1
  within 1e-2, though single values are neither 0 nor 1. With the panel's far
  half occluded, it averages the dense-quadrature ratio of shadowed to
  unshadowed term within 1e-2 (research.md O23).
- [ ] A 4 × 4 m animated panel with `_falloff_range 3` holds a bounce-delta
  entry for every affinity cell within 3 m of a polygon, including cells beyond
  3.5 m of its centroid (research.md O24).
- [ ] Raising `--soft-shadow-samples` on an area-light map re-bakes its lightmap
  layers and the weight-map stage, and a warm rebuild still reads its
  surface-radiance, direct-SH, scatter, mover-channel and bounce-delta entries
  from the cache.
- [ ] AlphaLights v5 round-trips area and non-area records. A v4 file and an
  unknown type value are rejected.
- [ ] On a map with no area light, every bake section is byte-identical to the
  build before this change.
- [ ] A sub-second area-light fixture joins `GATE_FIXTURES`, profiled first, and
  its two lightmap bakes match byte for byte.

Script surface:
- [ ] The Scripting surface example runs as a `content/dev` script on panels
  with no `_animated` key. Each tagged panel gains an animated-bake reservation,
  its faces' glow entries name that reserved slot, `lightType` reads `"Area"`,
  and `p.position` is the area-weighted centroid. The TypeScript and Luau type
  checks both accept `"Area"`.
- [ ] `sweep` on an area light returns `InvalidArgument` and leaves the light's
  animation unchanged. `sweep` on a spot light still works.
- [ ] A tagged `sweep` over a spot, an area light and a second spot, in that
  order, changes no light's animation on the host or a client. The same sweep
  over the two spots alone applies to both (research.md O14).
- [ ] An area light carrying `delay 0`, `_light_size` and a spot key has
  identical bake-time and runtime snapshots: `lightType` `"Area"`,
  `falloffModel` `"InverseSquared"`, `falloffRange` the resolved reach,
  `position` the centroid (research.md O18).

Glow and light identity (orderings in `research.md` §Pin table):
- [ ] A static area light's `_e` faces glow at `_e` × texture strength ×
  `_glow` × `_color`, so a white pane on a red light glows red, and a texture
  whose `materials.toml` strength is 4 glows four times as bright under the
  same light. The same textures on a worldspawn face glow at their texture
  strength alone, in the same draw. An `emissive_volume` enclosing the light's
  faces leaves them out of its membership. With `_glow` at 0, 0.5 and 8 the emissive term scales
  by that factor and every bake section stays byte-identical. A negative `_glow`
  fails the build.
- [ ] A `colorShift` moves the glow's colour and the room's colour together.
- [ ] With `_falloff_range` set, doubling an area light's `light` doubles its
  lightmap and probe output and leaves its faces' emissive term unchanged
  (research.md O2).
- [ ] Two otherwise equal area lights with the same diffuse map and
  all-glowing `_e` maps, one on a texture with `materials.toml` strength 4 and a
  white `_e` map and one
  on a plain texture with a red `_e` map, produce byte-identical lightmap, SH and
  scatter sections.
- [ ] Changing only `_glow` re-keys no bake cache entry: a warm rebuild reads
  every lightmap, surface-radiance and SH entry from the cache.
- [ ] A script fade from 1 to 0.25 on an animated area light settles with its
  faces' emissive multiplier at 0.25, equal to the room's settled scale, on the
  settle frame and after (research.md O1).
- [ ] A one-shot colour animation from white to amber on an animated area light
  settles with its faces' glow colour amber, equal to the room's settled colour,
  on the settle frame and after (research.md O17).
- [ ] An area light with `_start_inactive 1` and no `_animated` key gains an
  animated reservation and starts with dark faces and an unlit room. The same
  light under a `levelLoad` pulse starts lit, faces and room together
  (research.md O3).
- [ ] A `_start_inactive 1` area light whose only `levelLoad` step is a `sweep`
  starts with dark faces and an unlit room (research.md O15).
- [ ] A static point light with `_start_inactive 1` and no `_animated` key,
  curve or script step gains no animated reservation and bakes lit into the
  static lightmap.
- [ ] On a map with an animated point light ahead of three animated area lights,
  one of them `_bake_only`, each area light's faces resolve to that light's own
  compose descriptor slot (research.md O4).
- [ ] A static area light authored ahead of an animated one keeps a steady glow
  at `_e` × texture strength × `_glow` × `_color` while the animated light pulses (research.md
  O16).
- [ ] A map with no area light carries no area-light glow entries and draws
  texture-strength glow unchanged. Emitting faces past the glow-table cap fail the build, naming the
  count and the cap, and the loader rejects an over-cap table (research.md O5).

### Manual
- [ ] Hallway map, recorded in `plan.md`: cold and warm bake time for one 4 × 4
  m skylight, against the same patch lit by a 4 × 4 grid of point lights.
- [ ] Hallway map, recorded in `plan.md`: penumbra RMS noise against a 256-ray
  reference at 32 and 64 escalated rays, 1–3 m from the panel. At the global
  count of 32 the penumbra looks no noisier than at 64 in the owner's A/B.
- [ ] Owner A/B: skylight room against the template grid. No hot spots or wall
  ripple, and no visible edge at the reach cutoff.
- [ ] A scripted flicker: the glow and the room light move together frame for
  frame (research.md O13).
- [ ] Entities walking under the skylight are lit like the walls beside them.
- [ ] The roadmap "surface lights" entry reads as a texture-keyed front end on
  the area-light type that owns the texture-level bounce multiplier, and the
  gi-rtx-lite "Emissive bounce" record carries the line excluding `light_area`
  faces.

## Path
- **Chokepoints.** Add the polygon arm at these, by symbol:
  - Lightmap and shadowmask: `lightmap_bake::{light_contribution_and_direction,
    emitter_radius, hard_ray_target, area_sample_target, probe_sample_direction,
    emitter_lattice_spin, SoftProbes::new}`, `texel_footprint::EmitterKey`.
  - Probes: `sh_bake::light_eval::{incident_radiance_at_point,
    light_contribution_lambert, bounce_contribution}`,
    `sh_bake/probe.rs::bake_probe_direct_rgb_posed`.
  - Mover probe channel: `mover_probe_channel/{direct,visibility,posed_scene}`.
  - Animated bounce: `lightmap_bake::soft_visibility_one_sample`,
    `sh_bake/bounce.rs::{bounce_sample_index, sample_radiance_rgb}`,
    `delta_sh_bake.rs`, and the range guard `light_eval::light_reaches_point`.
    The one-sample visibility is w·V·Ω / ∫w dω, each sample uniform in solid
    angle given its seed (O23).
  - Mover light gate: `mover_light_gate/{probes,texels}`, which aim at
    `hard_ray_target` today.
  - Billboard scatter: `billboard_direct_scatter_bake`.
- **Two spheres.** The emitter sphere replaces `light_size` in
  `lightmap_layer/shaft_cull.rs::ShaftCull::new` and the mover occluder tests
  (`posed_scene`, `visibility`). One reach-sphere accessor serves the
  `falloff_range` cull readers (`research.md` §Grounding), including the layer
  key's geometry slice (O10), `affinity_grid` and `delta_sh_bake::light_aabb`
  (O24).
- **Solid-angle sampler.** Ureña et al. 2013 for rectangles, an Arvo 1995 fan
  for other convex polygons, picking triangles by solid angle. Each texel sample
  carries the weight cosθ_r at the texel centre. Polygons whose plane the
  receiver is behind are skipped; a sample below the texel's horizon carries
  weight 0 (O20).
- **Probe shadowing.** A control-variate term on the 4-ray set, with all-visible
  and none-visible fast paths. The scalar ratio taken from L0 is the cheapest
  fallback (`research.md` §3).
- **Numerics.** f64 with the atan2 edge form; no fitted approximation offline.
- **Emitting sides.** Take sides before BSP splitting, and stamp glow ids on
  every derived render face (O12).
- **Lattice snap.** The polygon lattice depends on the light and the receiver.
  Key it apart from the count-keyed point/spot snap in `SoftProbes::new`.
- **Self-occlusion.** Lift each shadow-ray target along its face normal by the
  tracer tolerance τ. The segment then never crosses the emitter's plane, and
  the lift stays inside the emitter sphere's slack (`research.md` §4). The
  receiver floor's centre probe traces the lattice sample it would otherwise
  replace, not `hard_ray_target`'s origin (O19).
- **Cache digest.** Add the polygon to
  `surface_radiance/layer.rs::LightDigest::of` by hand. Keep `_glow` out of
  every bake key.
- **Promotion.** Animated promotion
  (`animated_baked_shadow_candidates_with_direct_delta`) needs an explicit type
  refusal. `is_promotable_base_light` already has one.
- **Glow table.** Share the per-face glow id and table that
  `plans/ready/emissive-volumes` builds, rather than a second mechanism; the
  forward pass has no texture binding to spare (`research.md` §6). An area
  light's entry still evaluates its compose descriptor and the frame's time
  uniform in-shader, so glow and room stay on one frame (O13); volume entries
  are packed on the CPU. The entry layout is the executor's call.
- **Strongest rival:** texture-keyed polygon lights, which use the roadmap's
  mark-a-texture-once authoring with this brief's maths. They win for many small
  emitters (screens, signs). The entity wins for the Problem's set pieces: one
  light per panel, a script handle, and its own colour and brightness.
- **First slice.** Polygon term plus weighted visibility in the lightmap only,
  on a test map: the riskiest maths, behind the analytic and visibility rows.
  Then probes, glow, wire and keys.
- **Cross-check.** Godot 4.7's `lm_area_lights_inc.glsl` (MIT) checks the
  clipped polygon term cheaply. Its centre-aimed shadows are not this design.

## Open questions
- Patent follow-ups and counsel's reading of `ip-check.md`. Runs beside the
  build; a filing whose elements all appear present stops it (Decisions, IP).
  — owner: owner / counsel — **delegated**

## Boundary inventory

| Name | Rust | Wire | TS / Luau | FGD KVP | Docs |
|---|---|---|---|---|---|
| Area light classname | adapter-recognised brush entity | n/a | n/a | `light_area` (`@SolidClass`) | `docs/level_design.md` lights |
| Light type | `LightType::Area` | `AlphaLightType` value 3 | `LightKind` `"Area"`, both files ship | n/a | `docs/scripting-reference.md` light handles |
| Falloff reported | n/a | `falloff_model` 2 | `falloffModel` `"InverseSquared"` | n/a | same |
| Brightness, colour, override reach | `MapLight` intensity, color, falloff_range | existing record fields | existing | `light`, `_color`, `_falloff_range` | same |
| Animation keys | existing | existing | existing | `brightness_curve`, `color_curve`, `period_ms`, `_curve_phase`, `_animated`, `_start_inactive`, `_bake_only`, `_tags` | same |
| Glow strength | per-entity f32 | glow table (not AlphaLights) | n/a | `_glow(float)`, default 1.0 | same |

## Wire format
AlphaLights (id 18) v5, little-endian throughout, mirroring v4:
- Version u32, then light count u32, then fixed 73-byte records with the v4 field
  order unchanged. No trailing payload.
- An area record has `light_type` = 3, `origin` = area-weighted centroid,
  `falloff_range` = resolved reach, `falloff_model` = 2, zero cone fields and
  `shadow_type` = 0.
- A loader rejects any version other than 5 and any unknown type value.
