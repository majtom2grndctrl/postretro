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
  (precedent: `research.md` §5). `light_area` is the type's first front end. At
  promotion, the roadmap "surface lights" entry is reworded as a texture-keyed
  front end on this type, for screens, signs and lava. It also takes over the
  texture-level bounce multiplier, and must reconcile it with "Albedo is
  physical" (`build_pipeline.md` §Compiler pipeline).
- **Authoring: a brush entity, `light_area`.** Its brushes compile as static world
  brushwork, like `switch` and `func_group`. Its emitting faces are the exposed
  brush sides whose texture has an `_e` map. Emission is one-sided, along each
  face's outward normal. An entity with no emitting face fails the build.
- **Brightness is per area.** A larger brush gives proportionally more light, as
  in every area-scaled tool (`research.md` §5). Far from the panel, on its axis,
  a 1 m² panel matches an inverse-squared (`delay 2`) point light of the same
  `light` value; close in, it is dimmer, which is the point. The entity carries
  its own `_color` and `light`, and its light never reads a material's emissive
  strength or a texture prefix.
- **Reach is derived, with an override, and light fades to it.** Reach R is the
  distance from the polygons at which the bound E ≤ L·A/d² falls to 1/256 of
  what a `light 300` point light gives at 1 m. `_falloff_range`, measured the
  same way, overrides it. The term is scaled by the window max(0, 1 − (d/R)⁴)²,
  with d the distance to the nearest polygon (Karis 2013), so light reaches zero at R with no visible edge
  and nothing beyond R is lit. A hard cut is either visible or reaches hundreds of
  metres (`research.md` §4).
- **Unshadowed term: exact, then windowed.** Lambert's polygon irradiance, clipped to the
  receiver's horizon, with the clipped polygon's irradiance vector as the stored
  direction (`research.md` §1). A receiver on or behind the emitter's plane gets
  zero, so the emitter's own faces take no direct light from it.
- **Shadows: integrand-weighted visibility in the existing separable fold.** Each
  shadow sample counts by its share of the unshadowed light, never as a plain
  clear-ray fraction, which is wrong near a large emitter (`research.md` §2). The
  `lightmap_layer` partition record does not change. Area lights take the global
  escalated sample count (`--soft-shadow-samples`), with no per-type count
  (`research.md` §2).
- **Probe bakes project over the polygon, not along one direction.** This covers
  direct SH, billboard scatter and the mover probe channel. Unshadowed
  coefficients are analytic; shadowing never applies a per-coefficient ratio
  (`research.md` §3).
  - Bounce comes from the light's own surface-radiance patch layer, as for every
    light. Its glow never enters the radiance cache, which keeps the
    no-double-count invariant (`context/lib/index.md` §2).
  - The gi-rtx-lite ruling "Emissive bounce: opt-in per material" stays open.
    The record gains one line: `light_area` faces are excluded from any emissive
    bounce, since their light already bounces.
- **Culls take two spheres.** Shadow-ray and occluder tests that bound the
  emitter get the emitter sphere: the polygons' centroid and circumradius. Reach
  and influence culls get the reach sphere: the same centre, with the reach added
  to the radius. Both contain what they stand for, so every exact-skip proof
  holds and no output byte depends on a cull (`build_pipeline.md` §Compiler
  pipeline step 12, Exact skips). Tight polygon hulls are a follow-on, built only
  if the bake-time row asks for them.
- **Flush emitters are expected.** Buried-light classification and the
  embedded-emitter test do not apply to this type. Shadow rays are never blocked
  by the emitter's own faces.
- **Glow follows the light.** Each emitting face draws `_e` × `_glow` × the
  light's live colour × its live brightness multiplier, on the same frame as its
  lighting. At rest, the live colour is `_color`, so a red light's white pane
  glows red. Glow is drawn for static lights too.
  - `_glow` is an authored number, independent of `light`, so a game can pair a
    dim glow with strong cast light, or the reverse (`context/lib/index.md` §2,
    Defaults for novices, controls for experts). Values ≥ 0 are accepted with no
    upper clamp; a negative value fails the build.
  - On a `light_area` face, `_glow` replaces the texture prefix's emissive
    strength, so a `neon_` texture there does not glow twice. This is the first
    per-surface glow scalar: it overrides "no per-surface runtime scalar in v1"
    (`resource_management.md` §Emissive) for area-light faces only.
- **`_start_inactive` reserves the animated bake.** On an area light, the key
  means the light starts dark, faces and room together, and a script or mod
  reaction can light it. It takes the same animated reservation `_animated 1`
  does. Point lights keep today's rule.
- **Script surface: the existing light family, plus one type name.** Area lights
  appear in `getMapEntities("light")` with brightness and colour animation and
  the script-derived animated reservation. `lightType` reports `"Area"`,
  `falloffModel` reports `"InverseSquared"`, and `position` is the area-weighted
  centroid. A direction channel (`sweep`) on an area light is refused at runtime
  with `InvalidArgument`, as `setLightAnimation` refuses other invalid input. Co-op
  replication of light animation is unchanged.
- **Keys: inert keys warn, contradictory keys fail.** Changing a `light` to
  `light_area` in the editor keeps its keys. So `delay`, `_light_size` and the
  spot keys warn, naming the light and key, and are ignored. `_shadow_type sdf`,
  `direction_curve` and `style` request behaviour this light cannot give, and fail
  the build.
- **v1 refusals.** No area-light specular: the light keeps an inert placeholder
  `SpecLight` record, is never a shadowmask owner, and adds no specular on any
  path. Runtime specular is a recorded door (`research.md` §6), deferred until
  the diffuse light has shipped and been judged. No entity-shadow promotion on
  either path, because promotion shadows from a point, and a point shadow from a
  4 m panel is wrong.
- **Wire: AlphaLights (id 18) v5 adds one type value.** No polygon payload ships
  until a runtime consumer reads it. A later specular feature takes its own bump.
- **Non-goals.**
  - Two-sided emitters: none of the motivating content needs one.
  - Texture-keyed emission and the texture-level bounce multiplier: the reworded
    roadmap entry owns them.
  - Emissive bounce from ordinary emissive faces, and any change to prefix glow
    outside `light_area` faces.
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
  1e-5 and 1e-7 at s = 0.1, 0.01 and 0.001, with the window off. On the axis,
  4 m from a 1 m² `light 300` panel (reach 16 m), the lit value is 0.972 of an
  inverse-squared point light of the same `light` value, within 1%; at 1 m it
  is lower.
- [ ] f32 corners about 1 km from the origin agree with f64 corners.

Shadows and probes:
- [ ] Unoccluded and fully occluded texels under a flush `light_area` panel
  store exactly 1 and 0.
- [ ] An occluder over the near half of a large emitter and one over the far
  half store different visibility, ordered by integrand weight. An unweighted
  fraction ties them.
- [ ] Unoccluded probe coefficients match the analytic L0 (solid angle) and L1
  (irradiance vector).
- [ ] 4 × 4 m panel, probe 1 m below: the side-facing reconstruction is within 5%
  of 0.781, and the down-facing one within 0.05 of 0. The single-direction
  projection gives 0.230 and 0.154.
- [ ] Billboard scatter and the mover probe channel each project a 4 × 4 m panel
  over the polygon: at a probe 1 m below, the side-facing value is within 5% of
  0.781.

Placement, culls and keys:
- [ ] In one entity holding a frame brush and an `_e` glass brush, only the
  glass's exposed faces emit. The entity's brushes block movement and cast
  shadows like worldspawn.
- [ ] With `_falloff_range` set, a receiver just past it, measured from the
  panel, is unlit, and one just inside is lit. Without the key, doubling `light`
  lengthens reach by √2. Along the axis the lit value falls continuously to 0 at
  reach, and at half reach it is 0.879 of the unwindowed term.
- [ ] An area light on all four sides of a pillar, with its centroid inside the
  pillar, is neither dropped as buried nor flagged embedded. A point light
  buried in solid is still dropped.
- [ ] A wall texel at a flush emitter's edge stores visibility 1 and matches the
  unoccluded polygon term within 1%.
- [ ] Every shadow-ray target the polygon lattice yields, after any lift, lies
  inside the light's emitter sphere, for a 4 × 4 m ceiling panel, a 4 × 0.1 m
  strip and a light wrapping all four sides of a pillar.
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
  selected. The placeholder `SpecLight` is never an id-42 owner. World faces
  beside the panel take no specular from it, nor do billboards with billboard
  scatter withheld.
- [ ] An animated area light's weight map at curve 1 equals its static bake.
- [ ] AlphaLights v5 round-trips area and non-area records. A v4 file and an
  unknown type value are rejected.
- [ ] A sub-second area-light fixture joins `GATE_FIXTURES`, profiled first, and
  its two lightmap bakes match byte for byte.

Script surface:
- [ ] The Scripting surface example runs as a `content/dev` script on panels
  with no `_animated` key. Each tagged panel gains an animated-bake reservation,
  its faces' glow entries name that reserved slot, `lightType` reads `"Area"`, and
  `p.position` is the area-weighted centroid. The TypeScript and Luau type
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
- [ ] A static area light's `_e` faces glow at `_e` × `_glow` × `_color` on a
  plain or `neon_` texture, so a white pane on a red light glows red. The same
  textures on a worldspawn face keep their prefix glow, and a plain one stays
  dark, in the same draw. With `_glow` at 0, 0.5 and 8 the emissive term scales
  by that factor and every bake section stays byte-identical. A negative `_glow`
  fails the build.
- [ ] A `colorShift` moves the glow's colour and the room's colour together.
- [ ] Doubling an area light's `light` doubles its lightmap and probe output and
  leaves its faces' emissive term unchanged (research.md O2).
- [ ] Two otherwise equal area lights with the same diffuse map, one on a
  `neon_` texture with a white `_e` map and one on a plain texture with a red
  `_e` map, produce byte-identical lightmap, SH and scatter sections.
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
  at `_e` × `_glow` × `_color` while the animated light pulses (research.md
  O16).
- [ ] A map with no area light carries no glow entries and draws prefix glow
  unchanged. Emitting faces past the glow-table cap fail the build, naming the
  count and the cap, and the loader rejects an over-cap table (research.md O5).

### Manual
- [ ] Hallway map, recorded in `plan.md`: cold and warm bake time for one 4 × 4 m
  skylight, against the same patch lit by a 4 × 4 grid of point lights.
- [ ] Hallway map, recorded in `plan.md`: penumbra RMS noise against a 256-ray
  reference at 32 and 64 escalated rays, 1–3 m from the panel. At the global
  count of 32 the penumbra looks no noisier than at 64 in the owner's A/B; a
  shortfall is answered by sample placement, not a per-type count.
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
  - Billboard scatter: `billboard_direct_scatter_bake`.
- **Two spheres.** The emitter sphere replaces `light_size` in
  `lightmap_layer/shaft_cull.rs::ShaftCull::new` and the mover occluder tests
  (`posed_scene`, `visibility`). One reach-sphere accessor serves the
  `falloff_range` cull readers (`research.md` §Grounding), including the layer
  key's geometry slice (O10).
- **Visibility weighting.** Two equal options (`research.md` §2): Heitz–Hill–
  McGuire ratio estimator over solid-angle samples (Ureña for rectangles, an Arvo
  fan otherwise), or projected-solid-angle sampling (Peters 2021) with plain
  counting. Pick after the patent search.
- **Probe shadowing.** A control-variate term on the 4-ray set, with all-visible
  and none-visible fast paths. The scalar ratio taken from L0 is the cheapest
  fallback (`research.md` §3).
- **Numerics.** f64 with the atan2 edge form; no fitted approximation offline.
- **Emitting sides.** Take sides before BSP splitting, and stamp glow ids on every
  derived render face (O12).
- **Lattice snap.** The polygon lattice depends on the light and the receiver.
  Key it apart from the count-keyed point/spot snap in `SoftProbes::new`.
- **Self-occlusion.** Lift each shadow-ray target along its face normal by the
  tracer tolerance τ. The segment then never crosses the emitter's plane, and
  the lift stays inside the emitter sphere's slack (`research.md` §4). Larger
  prior-art lifts break that containment.
- **Cache digest.** Add the polygon to `surface_radiance/layer.rs::LightDigest::of`
  by hand. Keep `_glow` out of every bake key.
- **Promotion.** Animated promotion
  (`animated_baked_shadow_candidates_with_direct_delta`) needs an explicit type
  refusal. `is_promotable_base_light` already has one.
- **Glow table.** Follow the `animated_block` id → uniform-table precedent; the
  forward pass has no texture binding to spare (`research.md` §6). The
  glow reads the compose descriptor and the frame's time uniform in-shader (O13).
- **Strongest rival:** texture-keyed polygon lights, which use the roadmap's
  mark-a-texture-once authoring with this brief's maths. They win for many small
  emitters (screens, signs). The entity wins for the Problem's set pieces: one
  light per panel, a script handle, and its own colour and brightness.
- **First slice.** Polygon term plus weighted visibility in the lightmap only, on
  a test map: the riskiest maths, behind the analytic and visibility rows. Then
  probes, glow, wire and keys.
- **Cross-check.** Godot 4.7's `lm_area_lights_inc.glsl` (MIT) checks the clipped
  polygon term cheaply. Its centre-aimed shadows are not this design.

## Open questions
- Patent search on polygon area-light baking and on the visibility-weighting
  options, charted per `context/research/gi-rtx-lite/ip-check.md`. Prior art only
  so far (`research.md` §5). — owner: owner / counsel — **blocks build**
- Visibility weighting option, after the patent search. — **delegated**

## Boundary inventory

| Name | Rust | Wire | TS / Luau | FGD KVP | Docs |
|---|---|---|---|---|---|
| Area light classname | adapter-recognised brush entity | n/a | n/a | `light_area` (`@SolidClass`) | `docs/level_design.md` lights |
| Light type | `LightType::Area` | `AlphaLightType` value 3 | `LightKind` `"Area"`, both files ship | n/a | `docs/scripting-reference.md` light handles |
| Falloff reported | n/a | `falloff_model` 2 | `falloffModel` `"InverseSquared"` | n/a | same |
| Brightness, colour, override reach | `MapLight` intensity, color, falloff_range | existing record fields | existing | `light`, `_color`, `_falloff_range` | same |
| Animation keys | existing | existing | existing | `brightness_curve`, `color_curve`, `period_ms`, `_curve_phase`, `_animated`, `_start_inactive`, `_bake_only`, `_tags` | same |
| Glow strength | per-entity f32 | glow table (not AlphaLights) | n/a | `_glow(float)`, default 4.0 | same |

## Wire format
AlphaLights (id 18) v5, little-endian throughout, mirroring v4:
- Version u32, then light count u32, then fixed 73-byte records with the v4 field
  order unchanged. No trailing payload.
- An area record has `light_type` = 3, `origin` = area-weighted centroid,
  `falloff_range` = resolved reach, `falloff_model` = 2, zero cone fields and
  `shadow_type` = 0.
- A loader rejects any version other than 5 and any unknown type value.
