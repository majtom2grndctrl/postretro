# Billboard Specular Shimmer

## Goal

Give lit billboards a view-dependent, per-texel specular highlight that shifts
across the sprite face as the billboard, player, or light move relative to one
another — the "shimmer" a wet decal, hologram, or glinting crystal sprite
reads as. Today every billboard is lit with one whole-sprite specular value
computed at its center (`N = V`, constant across the quad); this makes a
billboard glint *uniformly* but never *across its surface*. This spec adds a
second billboard lighting model — **specular-shimmer** — that samples a baked
tangent-space normal map and specular mask per fragment, alongside the
isotropic-scatter model that smoke uses. A collection opts into shimmer by
shipping the maps; a collection without them is unchanged.

## Scope

### In scope

- A per-fragment specular path in `billboard.wgsl` for shimmer-classified
  collections: build a camera-facing tangent frame (right, up, `V`) rotated by
  the sprite's spin, perturb the normal by the sampled tangent-space normal
  map, modulate specular strength by the sampled spec mask, and evaluate
  Blinn-Phong per fragment so the highlight varies across the sprite face.
- A per-collection **material classification**: a collection whose baked `.prm`
  carries a `NORMAL` slot is a shimmer material; one without it stays on the
  existing lighting path. The classification is a single per-draw flag the
  renderer sets from the parsed slot mask.
- Extending the billboard group-1 bind group with a specular texture (binding
  3) and a normal texture (binding 4), both `texture_2d_array<f32>`
  (`view_dimension: D2Array`, matching the sprite texture at binding 0), reusing
  the existing filtering sampler at binding 1, and binding the `SPECULAR`/`NORMAL`
  slot array-layer views that the (prerequisite) sprite-PRM load path parses.
- Extending `SpriteDrawParams` with a second `vec4` (`params2`) carrying the
  shimmer flag and the per-collection specular exponent; the existing
  per-collection specular intensity stays in `params.y`.
- Resolving per-collection specular intensity and exponent through the sprite
  draw-contract path — the chokepoint that already resolves lifetime/emissive —
  with the current values as defaults (`6.0`/`0.45` intensity, `4.0` exponent).
  This centralizes intensity and replaces the shader's hardcoded exponent; both
  stages read intensity from `params.y` and exponent from `params2.y`. The
  candidate override fields default to `None` — the map-facing authoring surface
  that would set them is future (Decision 4).
- A dev fixture (a shimmer sprite station under a moving light) and a manual
  GPU check that the highlight travels across the sprite face under relative
  motion, plus `rendering_pipeline.md` documentation of the two-model split.

### Out of scope

- **Baking the sprite `SPECULAR`/`NORMAL` slots.** Produced by the (amended)
  `billboard-sprite-prm-baking` spec; this spec consumes the baked slots. See
  *Prerequisites*.
- **The isotropic-scatter model itself.** Owned by
  `billboard-volumetric-direct-lighting`; this spec is the sibling model it
  reserves the opt-in for. See *Prerequisites* and *Cross-spec coordination*.
- **Per-texel dynamic-light specular.** The dynamic-tier billboard loop stays
  diffuse-only (sharp per-light highlights on billboards read as artifact — the
  existing shader comment). Shimmer's per-fragment specular runs on the static
  chunk-light-list path only, matching where the whole-sprite specular runs
  today.
- **A per-emitter FGD material KVP.** Classification is by baked-slot presence,
  not an author toggle on `billboard_emitter` (see *Design decisions*). No FGD
  change.
- **A per-`billboard_emitter` specular-strength override.** Specular intensity
  and exponent are per-collection (material grain), authored through the draw
  contract where lifetime/emissive already are. A per-instance override is not
  built; it would layer on the same `params2`/draw-contract path later without
  reworking this spec (see *Design decisions*, Decision 4).
- **Spec-mask-only collections.** A `SPECULAR` slot with no `NORMAL` slot is not
  a distinct model here; the spec mask alone cannot move a highlight (Decision
  1). A "static sparkle, no moving glint" look would be a third classification,
  not a tweak to this one — out of scope (see *Design decisions*, Decision 5).
- **Environment/reflection-probe specular on billboards.** Shimmer is direct
  static-light specular only; it does not sample the reflection cubemaps.
- **Billboard self-shadowing or normal-mapped diffuse.** The normal map drives
  the specular term only; the diffuse/SH/scatter terms remain normal-free as
  today.

## Direction

**Problem.** The billboard shader computes its entire lighting term per vertex
at the sprite center with `N = V` and folds it into a single interpolated
`VertexOutput.lighting` (`billboard.wgsl` `vs_main`); the fragment stage does no
lighting. A single center value cannot express a highlight that lives on part of
the sprite face, so no amount of tuning the existing `static_specular` loop
produces a moving glint — the term is spatially constant by construction. The
missing ingredient is a per-fragment surface normal, which requires a normal map
and moving the specular evaluation into the fragment stage.

**Prior commitments.**
- The billboard shader already runs a multi-source Blinn-Phong static-specular
  loop over the chunk light list (`billboard.wgsl` `vs_main`, the `use_specular`
  block gated by `LIGHT_TERM_SPECULAR`; helper `blinn_phong`; the exponent is a
  bare `4.0` literal passed to the `blinn_phong` call that accumulates
  `static_specular`, and `spec_int` reads `draw_params.params.y`). Shimmer reuses
  this loop's light-gathering, cone, and attenuation math; it moves the loop to
  the fragment stage, swaps `N = V` for `N'`, modulates strength by the spec
  mask, takes the exponent from `params2.y`, and drops the scatter-map SDF-only
  light filter volumetric added (Decision 6).
- The world/model forward path already samples a per-texel specular mask
  (`spec_texture`, group 1 binding 2, R8) and a tangent-space normal map
  (`t_normal`, group 1 binding 4) in `forward.wgsl`, decoding the normal through
  `sample_normal` (`material_shading.wgsl`). Shimmer reuses that **decode
  arithmetic** (RG `·2−1`, Z-reconstruct, renormalize) but not the sampling
  call: the world path samples through `aniso_sampler` on a 2D texture, while the
  billboard samples through its own `sprite_sampler` + array-aware
  `sample_post_retro` at a `frame_idx` layer. The billboard inlines the decode
  the way it already inlines `blinn_phong`.
- The `.prm` format already carries `SPECULAR`/`NORMAL` slots (`PrmSlots`,
  `crates/level-format/src/prm.rs`); the amended `billboard-sprite-prm-baking`
  spec bakes and parses them for sprites as per-frame array layers, and the
  renderer already holds the parsed `SpriteSheet::specular_view` /
  `normal_view` (D2Array views built by `upload_texture_array_data`). Shimmer
  consumes the parsed slot mask as its classification signal and binds those
  views, so the opt-in reuses data that already exists at load rather than
  adding a parallel declaration.
- Per-collection draw parameters (lifetime, emissive) resolve through
  `SpriteCollectionCandidate` → `resolve_sprite_collection_draw_contract`
  (`crates/postretro/src/startup/lifecycle.rs`); `spec_intensity` is currently a
  field of `SpriteCollectionRegistration` set at each register call site (`6.0`
  for map/projectile collections, `0.45` for weapon-impact), not resolved in
  that contract, and the exponent is not carried at all. Shimmer moves the
  specular intensity and exponent into the same per-collection contract that
  resolves lifetime/emissive — landing the wiring with defaults; the map-facing
  surface that would author per-collection overrides is future (Decision 4).

*Divergence.* Billboard lighting is deliberately hoisted to the vertex stage
today, justified in-shader by every input being constant across the quad.
Shimmer breaks that premise for shimmer collections only: their static-specular
term moves to the fragment stage. This is an intentional, per-material
divergence — the isotropic path keeps its per-vertex constancy and its
justifying comment intact.

**Alternatives rejected.**
- *Per-texel spec mask with `N = V`, no normal map.* Modulating the existing
  center-evaluated highlight by a spec mask makes a static sparkle *pattern* but
  not a *moving* glint: with `N = V` the highlight direction is identical at
  every texel, so the mask brightens fixed texels rather than sweeping a
  highlight across the face as the light moves. The normal map is what makes the
  glint travel; the spec mask is an optional modulator on top. Shimmer requires
  the normal slot and treats the spec slot as optional.
- *An explicit `lighting_model` FGD KVP on `billboard_emitter`.* An author who
  ships a normal map already declared intent; a second toggle can only
  contradict the maps (shimmer flag set with no normal map → nothing to sample;
  normal map present but flag unset → baked data ignored). Classifying by slot
  presence removes the contradiction and the author surface. Recorded under
  *Design decisions*; revisit only if a collection needs shimmer geometry
  authored but disabled.
- *A separate billboard pipeline for shimmer.* Two pipelines double the
  pass-management surface (`SmokePass` owns one pipeline and a per-collection
  bind group). A single shader with a per-draw uniform branch keeps control flow
  uniform (the flag is constant per draw call) and avoids a second pipeline,
  matching how the shader already branches on `light_term_mask`.

## Design decisions

### Decision 1 — Classification is by baked `NORMAL`-slot presence

A collection is a shimmer material iff its loaded `.prm` slot mask contains
`NORMAL`. The renderer reads the parsed mask (from the prerequisite PRM-load
path), sets a per-collection shimmer flag, and packs it into
`SpriteDrawParams.params2.x`. No FGD KVP, no descriptor field, no second source
of truth: providing the normal map *is* the opt-in. A collection with a
`SPECULAR` slot but no `NORMAL` slot is **not** shimmer — the spec mask alone
cannot move a highlight (see *Alternatives rejected*); its spec slot is ignored
by the billboard path until a normal slot accompanies it. This keeps the
discriminator single-valued. The shimmer flag and the binding-3/4 real-vs-
placeholder view selection derive from the **same**
`header.slot_mask.contains(PrmSlots::NORMAL)` read, so the flag can never
disagree with which view is bound.

### Decision 2 — Shimmer specular is per-fragment; isotropic stays per-vertex

The shader keeps its current per-vertex lighting for non-shimmer collections
**output-identical at the default specular values** — verified by AC 3, not a
byte-for-byte source claim: the vertex shader gains a `params2.x`-gated skip
around its static-specular loop, so its control flow changes; only the rendered
result is unchanged at defaults. For
shimmer collections, the static specular term is computed in the fragment stage
instead, where the normal map varies per texel, and is **added to** the
interpolated `VertexOutput.lighting`; the vertex path **skips** its
static-specular loop for shimmer (`params2.x != 0`) so the term is never
double-counted. The ambient floor, SH indirect/direct, and dynamic diffuse
terms remain per-vertex for both models — only the static specular term moves,
because it is the only term the normal map changes.

### Decision 3 — Tangent frame is the camera-facing basis, rotated by sprite spin

Billboards have no authored tangents, so the world path's mesh-derived TBN
(`reconstruct_tbn_normal`, which needs `mesh_n` + `world_tangent` +
`bitangent_sign`) does not apply. The tangent frame is instead `(right, up, V)`,
recomputed in the fragment stage from uniforms it can read: `right`/`up` from
`camera_right_up(uniforms.view_proj)` (group 0, fragment-visible) and
`V = normalize(uniforms.camera_position - in.world_position)` where
`in.world_position` is the sprite **center** (`vs_main` sets
`out.world_position = sprite_pos`, constant across the quad — it must stay the
center for this reconstruction). `right`/`up` are then rotated by the sprite's
`rotation` (threaded through `VertexOutput`) so the glint pattern spins with the
sprite. The `NORMAL` slot is BC5 (`Bc5RgUnorm`), decoded the way `sample_normal`
(`material_shading.wgsl`) does; its RG channels decode to a tangent-space normal
(Z reconstructed) that maps to world space through the rebuilt `(right, up, V)`
frame — the exact decode/transform is in Task 1 — renormalized, since BC5
quantisation plus filtering leaves the sample slightly off unit length. The basis is not
strictly orthonormal for off-center sprites (`right`/`up` are screen-constant
while `V` is per-sprite); this is accepted — billboards are small and the world
path's orthonormalization does not apply. Light direction `L` is taken from the
sprite center per light (`spec_lights[i].pos − in.world_position`); per-texel `L`
variation is negligible and the shimmer comes from `N'`, not `L`.

### Decision 4 — Specular strength is per-collection (wiring lands; authoring surface future)

This spec lands the per-collection draw-contract wiring for specular intensity
and exponent — resolved alongside lifetime/emissive, with conflict rejection and
defaults (`6.0`/`0.45` intensity, `4.0` exponent). No map/FGD surface populates
the candidate override fields yet, so map collections resolve to the defaults;
that authoring surface is future. A per-instance override on `billboard_emitter`
is likewise not built. Both layer on the same `params2` / draw-contract path
later without reworking this spec's contracts.

### Decision 5 — Spec-mask-only collections are not a model

Decision 1 ignores a `SPECULAR` slot that arrives without a `NORMAL` slot on the
billboard path. A "static sparkle mask, no moving glint" look, if later wanted,
is a distinct third classification (spec-without-normal → mask-modulated
center-evaluated highlight), not a tweak to shimmer. Not built here.

### Decision 6 — Shimmer specular runs over all static lights, not the scatter-map SDF subset

Volumetric's vertex loop skips non-SDF static lights in a scatter map
(`has_scatter != 0 && !spec_light_is_sdf(sl)` → `continue`): their diffuse
transport moved into the normal-free scatter volume, so re-adding it would
double-count. Shimmer's fragment loop keeps that filter **off** — specular is a
view-dependent lobe the isotropic scatter volume cannot carry, so a non-SDF
light's glint is a distinct term, additive over its baked diffuse, not a
double-count (the engine invariant forbids double-counting one contribution, not
layering different lobes). The chunk light list physically retains every static
record — `spec_lights` packs a per-record SDF flag
(`crates/lighting/src/spec_buffer.rs`), shared with forward specular and SDF
K-selection — so the fragment loop reaches non-SDF lights by omitting the
continue, no repack. Without this, a shimmer sprite in a scatter-baked map would
glint only under SDF lights, silently defeating the AC 1 demo.

## Prerequisites

This spec consumes three upstream pieces:

1. **`prm-array-layers` (merged)** migrated the billboard sprite texture path
   from a stitched strip to per-frame `texture_2d_array` layers, extended the
   PRM format with a file-header `layer_count`, and routed the per-fragment
   `frame_idx` (flat-interpolated on `VertexOutput` at location 4) that shimmer
   samples the spec/normal maps by. Shimmer inherits an already-D2Array billboard
   shader and bind layout; its own `VertexOutput` growth (the sprite `rotation`
   for the tangent frame) takes a new location (5) so it does not collide with
   `frame_idx` at location 4. Shimmer samples the spec/normal maps per fragment
   at `layer = frame_idx`, not from a strip.
2. **`billboard-sprite-prm-baking` (merged)** bakes optional per-collection
   `SPECULAR`/`NORMAL` slots into the sprite `.prm` (as array-layer slots atop
   the `prm-array-layers` format) and parses/uploads them at runtime, exposing
   the loaded slot mask and the slot texture views (`SpriteSheet::specular_view`
   / `normal_view`, D2Array) to the billboard pass. Shimmer consumes: the slot
   mask (classification) and the two views (binding).
3. **`billboard-volumetric-direct-lighting` (merged)** added the normal-free
   isotropic-scatter path (PRL sections 47/48, `has_scatter` mode + `direct_scale`
   in `FrameUniforms`, scatter volume at group 3 binding 17) and **preserved** —
   did not delete — the static-specular loop this spec relocates, now carrying a
   per-light `has_scatter && !spec_light_is_sdf` filter (Decision 6). Shimmer owns
   that loop's per-fragment form.

All three prerequisites are merged (`context/plans/done/` for 1–2; volumetric
landed on the current branch). The billboard shader carries the preserved
static-specular loop this spec relocates and the scatter machinery it composes
with, so the spec is authored against present source — see *Cross-spec
coordination* for the shared surface.

## Cross-spec coordination

Volumetric landed first and preserved the `vs_main` static-specular loop rather
than deleting it; this spec relocates that loop's shimmer-flagged form to the
fragment stage. The shared surface is that loop and its `params.y` / `params2`
read sites. Volumetric left it gated by `use_specular` + `has_chunk_grid` +
`spec_int > 0.0` + the cell-in-bounds check, plus a per-light
`has_scatter && !spec_light_is_sdf` continue. Shimmer's fragment form keeps the
four grid-safety guards and drops the per-light continue (Decision 6): the vertex
loop stays scatter-aware for non-shimmer collections, while the fragment loop
evaluates every static light.

## Acceptance criteria

- [ ] `[manual GPU]` On a shimmer collection (normal map present) lit by one
      static point light, the specular highlight occupies a sub-region of the
      sprite face and **moves across the face** as the light orbits the sprite
      (or the camera orbits a fixed light) — not a whole-sprite brightness
      change. On the same scene, a non-shimmer collection (no normal map) shows
      the current whole-sprite behavior, unchanged. (Pin table P3, P4, P5.)
- [ ] `[unit]` A GPU-free classification helper maps a `.prm` slot mask
      containing `NORMAL` to a set shimmer flag (`params2.x`) and a mask without
      `NORMAL` (diffuse-only, or diffuse+specular) to a clear flag. `[review]`
      The flag and the bound normal view derive from the same `NORMAL`-presence
      read (one predicate, checked by inspection). (Pin table P8.)
- [ ] `[unit]` The billboard group-1 bind group layout exposes a
      `texture_2d_array<f32>` specular texture at binding 3 and a
      `texture_2d_array<f32>` normal texture at binding 4 (`view_dimension:
      D2Array`, FRAGMENT), and the two shared 1×1 single-layer D2Array
      placeholders carry the documented CPU texel bytes (spec `.r = 1.0`, normal
      `rg = 0.5`). `[manual GPU]` A non-shimmer collection binds the placeholders
      at 3/4 and renders identically to pre-spec output (pixel-equality against a
      captured pre-change frame on a smoke fixture; the capture harness is
      GPU-gated, not a CI unit test). (Pin table P5, P6, P7.)
- [ ] `[unit]` `SpriteDrawParams` round-trips the widened uniform:
      `SPRITE_DRAW_PARAMS_SIZE` grows from 16 to 32 bytes; `params2.x`
      (shimmer flag) and `params2.y` (spec exponent) pack and unpack at their
      documented offsets; the existing `params` vec4
      (frame_count/spec_intensity/lifetime/emissive) is unchanged in layout
      (`draw_params_layout` still holds for its bytes). Specular intensity
      remains at `params.y` and is not duplicated into `params2`. A
      `build_draw_params` call with the default exponent packs `params2.y = 4.0`
      (not a zero-filled default). (Pin table P14.)
- [ ] `[unit]` The WGSL shader validates (naga) and the `SpriteInstance` stride
      test (`billboard_wgsl_sprite_instance_stride_matches_cpu`) still passes;
      the shimmer branch adds no per-particle storage-buffer field (new data
      rides `SpriteDrawParams` and `VertexOutput`). The billboard pipeline's
      fragment stage now reads the group-2 chunk-light storage buffers; those
      entries are already `VERTEX | FRAGMENT`-visible on the shared lighting BGL,
      so no visibility change or budget slot is spent — assert this with a
      fragment-stage storage-count check mirroring the existing
      `billboard_pipeline_vertex_storage_request_matches_bgl_definitions` test
      (only a vertex counter exists in `pipeline_layout.rs`; a `pub(crate)`
      fragment counter must be added there).
- [ ] `[unit]` The per-collection specular intensity and exponent resolve
      through the draw-contract path: a directly-constructed candidate that
      supplies them resolves to the declared values; one supplying neither
      resolves to the defaults (intensity `6.0`, exponent `4.0`); the
      weapon-impact registration keeps its `0.45`. Conflicting candidate values
      are rejected order-independently (the lifetime/emissive message shape);
      zero candidates → the defaults. `build_draw_params` packs intensity →
      `params.y` and exponent → `params2.y`. `[review]` Both shader stages read
      the exponent from `params2.y` (one read site each; naga-checked), so an
      authored value is honored, not silently dropped. (Pin table P1, P2, P11,
      P12, P13.)
- [ ] `[unit]` The shader validates (naga). `[review]` The fragment shimmer path
      replicates the vertex loop's grid-safety guards (`use_specular`,
      `has_chunk_grid != 0`, `spec_int > 0.0`, cell-in-bounds) before indexing
      the chunk buffers — and omits its per-light scatter/SDF continue (Decision
      6), so the loop evaluates every static record. `[manual GPU]` A shimmer
      collection placed outside the chunk grid, or on a level with
      `has_chunk_grid == 0`, renders zero static specular with no GPU fault. (Pin
      table P9.)
- [ ] `[manual GPU]` A spinning shimmer sprite's glint pattern rotates with the
      sprite (the tangent frame tracks `rotation`, threaded flat at location 5),
      and the sprite viewed at distance does not shimmer-crawl (the normal/spec
      maps are mipped by the prerequisite bake and selected at distance). (Pin
      table P10.)

## Tasks

### Task 1: Thin slice — end-to-end per-fragment shimmer for one classified collection

Prove the boundary from parsed slot mask → bind group → fragment shader → screen
on a single shimmer collection before fanning out.

**Bind group.** Extend `sprite_sheet_bind_group_layout_entries` in
`crates/renderer/src/render/smoke.rs` (currently `[_; 3]`) with a specular
texture at binding 3 and a normal texture at binding 4 — both
`texture_2d_array<f32>`, `view_dimension: D2Array`, FRAGMENT visibility, sampled
through the existing filtering sampler at binding 1. Bind the parsed
`SpriteSheet::specular_view` / `normal_view`, falling back to a placeholder when
a slot is absent. `SmokePass` owns **two** shared 1×1 single-layer D2Array
placeholder views, created in `SmokePass::new` (the way it owns the sampler and
layouts): a spec placeholder with `.r = 1.0` (an absent spec slot leaves the
highlight unmodulated) and a normal placeholder with `rg = 0.5` (flat `N' = V`).
Both group-1 bind-group build sites (baked branch and PNG-fallback branch) bind
at 3/4.

**Uniform.** Widen `SpriteDrawParams` with a second `vec4<f32>` `params2` =
`(shimmer_flag, spec_exponent, reserved, reserved)`; `SPRITE_DRAW_PARAMS_SIZE`
grows 16 → 32. Specular intensity stays in `params.y` — not duplicated into
`params2`. `build_draw_params` **must write** both `params2.x` (the flag) and
`params2.y` (the exponent, `4.0` this task; Task 2 re-parameterizes it): a
zero-filled `params2.y` yields `pow(NdH, 0) = 1` and breaks non-shimmer
specular, which only the GPU-gated AC 3 would catch. Derive the flag from
`slot_mask.contains(PrmSlots::NORMAL)` via a GPU-free helper so classification is
unit-testable without a device (Decision 1).

**Shader.** In `billboard.wgsl`, declare `spec_texture` (g1 b3) and
`normal_texture` (g1 b4) as `texture_2d_array<f32>`. Thread the sprite `rotation`
through `VertexOutput` at `@location(5) @interpolate(flat)` (location 4 is
`frame_idx`; `rotation` is per-sprite constant). Keep
`out.world_position = sprite_pos` (the center) — the fragment reconstructs `V`
and per-light `L` from it.

- **Vertex path.** Add `&& params2.x == 0.0` to the static-specular loop guard
  (`if use_specular && chunk_grid.has_chunk_grid != 0u && spec_int > 0.0 &&
  params2.x == 0.0`), and pass `params2.y` as the `blinn_phong` exponent in place
  of the `4.0` literal (default 4.0 → output-identical). A shimmer collection's
  vertex `lighting` thus excludes `static_specular`; ambient floor + SH + dynamic
  diffuse still fold at the vertex.
- **Fragment path.** When `params2.x != 0.0`, compute static specular in the
  fragment: rebuild `right`/`up` from `camera_right_up(uniforms.view_proj)` and
  `V = normalize(uniforms.camera_position − in.world_position)`, rotate
  `right`/`up` by `in.rotation`, sample the BC5 normal at `layer = in.frame_idx`
  through `sample_post_retro` + `sprite_sampler`, decode tangent-space
  `(nx, ny) = rg·2−1`, `nz = sqrt(max(0, 1 − nx² − ny²))`, and transform to world
  space `N' = normalize(nx·right + ny·up + nz·V)`. Inline this decode in
  `billboard.wgsl` — do **not** concatenate `material_shading.wgsl` (it redefines
  `blinn_phong`, a naga collision, and its `sample_normal` is the
  2D/`aniso_sampler` variant). Sample the `.r` spec mask at the same layer to
  modulate strength — the mask is the optional modulator (Decision 1); a fixture
  `_spec` frame exercises its sub-1.0 effect visually (manual, not unit-gated).
  Run the chunk-light-list Blinn-Phong loop with `N'`,
  `spec_int = max(params.y, 0.0)`, exponent `params2.y`. Replicate the vertex
  loop's **grid-safety** guards — `LIGHT_TERM_SPECULAR` (`use_specular`),
  `has_chunk_grid != 0`, `spec_int > 0.0`, and the
  `all(cell >= 0) && all(cell < dims)` bounds check before indexing
  `chunk_offsets`/`chunk_indices`. Do **not** replicate the vertex loop's
  per-light `has_scatter != 0 && !spec_light_is_sdf(sl)` continue: shimmer
  specular evaluates every static record in the chunk list, not the SDF-only
  subset a scatter map's vertex loop keeps (Decision 6). The loop otherwise
  differs from the vertex form only in the normal (`N'` vs `N = V`). Add the
  fragment term to `in.lighting`.

The group-2 chunk-light storage buffers are already `VERTEX | FRAGMENT`-visible
on the shared lighting BGL, so the fragment becoming a reader needs no visibility
change (verify per AC 5).

Register one hand-built shimmer collection (normal map present) in a dev path;
confirm manually that a moving light sweeps a highlight across the face. This
task is the widest, single-collection slice; author-facing parameters and the
fixture come after.

### Task 2: Material classification and per-collection specular parameters

Route the specular parameters through the per-collection draw-contract path so
they are resolved data, not hardcoded constants.

Extend `SpriteCollectionCandidate` and `resolve_sprite_collection_draw_contract`
(`crates/postretro/src/startup/lifecycle.rs`) to resolve `spec_intensity`
(default `6.0`) and `spec_exponent` (default `4.0`) alongside lifetime/emissive —
same `get_or_insert` + `to_bits` conflict rejection and `map_or` defaulting. The
candidate's spec fields are `Option<f32>` defaulting to `None`; nothing populates
them from map data in this spec (the map-facing authoring surface is future,
Decision 4), so map collections resolve to the defaults and the conflict/override
tests construct candidates directly. Widening the return type breaks the existing
destructure sites (the sole call site and the resolve unit tests) — update them.

The weapon-impact collection is registered outside the resolve loop
(`weapon::impact_sprite_collection()`); it keeps its `0.45` intensity as its
`SpriteCollectionRegistration.spec_intensity` field (with default `4.0`
exponent) — engine-registered, not routed through resolve.

Widen `SpriteCollectionRegistration` with `spec_exponent`, thread the resolved
intensity/exponent through `register_smoke_collection` /
`SmokePass::register_collection`, and update `build_draw_params` to pack
intensity → `params.y`, exponent → `params2.y`, flag → `params2.x`. **Both** call
sites (baked + PNG-fallback) pass the resolved values; the PNG branch (slot mask
`DIFFUSE` only) passes `shimmer_flag = 0` with the resolved intensity/exponent,
never zeros. Update the layout tests for the re-parameterized exponent (the size
constant itself changes in Task 1).

The shimmer flag is set from the loaded slot mask in the register path (Decision
1), not from the draw contract — the contract carries strength, slot presence
carries classification.

### Task 3: Dev fixture and durable documentation

Add a shimmer sprite station to a dev map with a clear relative-motion path: a
static point (or spot) light and a shimmer emitter positioned so orbiting the
camera sweeps the highlight across the sprite face, plus a non-shimmer smoke
emitter nearby for the unchanged-behavior comparison the AC names. Author the
shimmer collection's `_normal` (and optional `_spec`) companion frames so the
prerequisite bake produces the slots. Document the two billboard lighting models
(isotropic-scatter default vs specular-shimmer opt-in), the slot-presence
classification, the camera-facing tangent frame, the per-fragment-vs-per-vertex
split, and the single read-site rule (intensity `params.y`, exponent
`params2.y`, both honored by both models) in
`context/lib/rendering_pipeline.md` §7.4, and update any adjacent
billboard-lighting reference that describes billboard specular as a single
center-evaluated `N = V` term to scope that description to the isotropic model.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the tangent-frame,
fragment-branch, array-texture, and bind-group boundary assumptions on one
collection before any author-facing surface is built.

**Phase 2 (sequential):** Task 2 — consumes Task 1's `SpriteDrawParams.params2`
layout and register-path signature; turns the slice's hardcoded specular
constants into per-collection draw-contract data.

**Phase 3 (sequential):** Task 3 — validates the integrated path end to end and
captures the durable model split; consumes the classification and parameters
from Tasks 1–2 and the baked slots from the prerequisite.

## Boundary inventory

| Name | Rust | Wire / serde | WGSL | FGD KVP |
|---|---|---|---|---|
| Shimmer classification | per-collection flag from loaded `PrmSlots::NORMAL` | n/a (derived from `.prm` slot mask) | `draw_params.params2.x` | n/a |
| Sprite specular map | `SPECULAR` slot D2Array view bound at group 1 binding 3 | `.prm` `slot_mask` bit 1 (`R8Unorm`, linear) | `spec_texture` (g1 b3, `texture_2d_array`), sampled `.r` at `layer = frame_idx` | n/a |
| Sprite normal map | `NORMAL` slot D2Array view bound at group 1 binding 4 | `.prm` `slot_mask` bit 2 (`Bc5RgUnorm`, linear) | `normal_texture` (g1 b4, `texture_2d_array`), RG-decoded + Z-reconstructed at `layer = frame_idx` | n/a |
| Specular intensity | `SpriteCollectionCandidate` → draw contract → `register_collection` | n/a | `draw_params.params.y` (both stages) | n/a |
| Specular exponent | `SpriteCollectionCandidate` → draw contract → `register_collection` | n/a | `draw_params.params2.y` (both stages) | n/a |

## Wire format

No new binary surface. Sprite `SPECULAR`/`NORMAL` slots ride the existing `.prm`
format (produced/parsed by the prerequisite). `SpriteDrawParams` is a
renderer-internal uniform, not a wire type; growing it from one `vec4` to two
(`SPRITE_DRAW_PARAMS_SIZE` 16 → 32) is a CPU/GPU layout change with a matching
layout test, not a serialized-format change.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Non-shimmer billboards render identically (output-identical at defaults) to pre-spec output | Task 1 (fragment branch gated by `params2.x`; vertex exponent from `params2.y` defaults to 4.0; placeholder views at b3/b4) | Task 1 — the `params2.x == 0` path must not alter the per-vertex lighting result at defaults; Task 2 — both register branches (baked + PNG) must pass the resolved intensity/exponent, never zeros | AC 1, AC 3, AC 6 |
| Static specular is evaluated exactly once — per fragment (shimmer) or per vertex (isotropic), never both | Task 1 (vertex loop gains `&& params2.x == 0.0`; fragment runs only when `params2.x != 0`) | Task 1 — the vertex path must skip the static-specular loop for shimmer collections so it is not double-added after the fragment computes it | AC 1 (P3, P4) |
| Specular intensity and exponent each have one read site, shared by both stages | Task 1 (intensity `params.y`, exponent `params2.y`; intensity not duplicated into `params2`) | Task 2 — `build_draw_params` writes each value to exactly one slot; neither stage reads a stale second copy | AC 4, AC 6 |
| Classification has one source: `NORMAL`-slot presence | Task 1 (flag and bound normal view both from `slot_mask.contains(NORMAL)`), Decision 1 | Task 2 — the draw contract carries specular *strength* only; it must not carry or override the classification flag | AC 2 |
| Fragment shimmer specular never reads chunk buffers out of bounds | Task 1 (full guard stack replicated in the fragment) | Task 1 — the `has_chunk_grid`/cell-in-bounds guards must precede any `chunk_offsets`/`chunk_indices` index | AC 7 |
| `SpriteInstance` GPU layout and per-particle budget unchanged | Task 1 (new data rides `SpriteDrawParams` uniform + `VertexOutput`, not the instance buffer) | Task 1 — passing `rotation`/center inputs to the fragment uses `VertexOutput`, not new `SpriteInstance` fields | AC 5 |

## Pin table

Ordering/state scenarios the spec pins; the acceptance criteria that consume
them (AC 1–4, AC 6–8) write from these rows rather than restating them.

| # | Scenario | Ordering / stage | Expected outcome |
|---|---|---|---|
| P1 | Non-shimmer collection authors `spec_intensity ≠ 6.0` | Resolve (load) → pack → vertex read | Isotropic vertex path reads the authored value from `params.y`; no stale copy. |
| P2 | Candidate supplies `spec_exponent ≠ 4.0` (non-shimmer) | Resolve → pack → vertex read | Resolved into `params2.y`; the vertex path reads its exponent from `params2.y` (not the old `4.0` literal), so an authored value is honored, not dropped. |
| P3 | Shimmer collection (NORMAL present), chunk grid built | Vertex stage | Vertex loop skipped (`params2.x != 0`); `out.lighting` excludes static_specular. |
| P4 | Shimmer collection | Fragment stage | Fragment computes static_specular with `N'` over every static light (scatter/SDF continue off, Decision 6) and adds to `in.lighting`; term counted exactly once (vs P3). |
| P5 | Non-shimmer collection | Vertex + fragment | Rendered result output-identical to pre-spec; fragment shimmer branch not taken. Golden pixel-equality (AC 3). |
| P6 | PNG-fallback collection (diffuse-only smoke) | Register branch (PNG) | `build_draw_params` writes 32 bytes; `shimmer_flag = 0`, resolved intensity/exponent packed (not zeros); bind group binds placeholder at b3/b4. No binding-size mismatch. |
| P7 | Diffuse-only collection registered at level load | `register_collection` (before any draw) | Shared 1×1 D2Array placeholder views already exist (created in `SmokePass::new`); bind group builds successfully. |
| P8 | NORMAL-slot presence | Register | Shimmer flag (`params2.x`) and binding-3/4 real-vs-placeholder selection derive from the same `slot_mask.contains(NORMAL)` read; flag=1 ⇒ real normal view bound, never placeholder. |
| P9 | Shimmer sprite outside chunk grid, or `has_chunk_grid == 0` | Fragment stage | Fragment replicates the grid-safety guards + cell-in-bounds; static_specular = 0, no OOB read, no black sprite. |
| P10 | Spinning shimmer sprite | Vertex (emit rotation) → fragment (rebuild frame) | `rotation` at `@location(5) @interpolate(flat)`; `out.world_position` stays sprite center; fragment rebuilds `(right,up,V)` from `view_proj` + `camera_position` + `in.world_position`, rotated by `rotation`. Glint rotates with sprite (AC 8). |
| P11 | Two candidates for one collection, differing `spec_intensity`, either order | Resolve (load) | Rejected regardless of candidate order, with the lifetime/emissive conflict-message shape. |
| P12 | Zero candidates supply spec params (N=0) | Resolve (load) | Defaults `spec_intensity = 6.0`, `spec_exponent = 4.0`. |
| P13 | weapon-impact collection | Register (outside resolve loop) | Keeps `0.45` intensity via its `SpriteCollectionRegistration` field + default `4.0` exponent; not routed through the map-candidate resolve path. |
| P14 | Non-shimmer collection at the **default** exponent (vertex loop now reads `params2.y`, was the `4.0` literal) | Pack (`build_draw_params`, zero-init buffer) → vertex read | `build_draw_params` — the single packer — writes `params2.y = 4.0` unconditionally. An omitted write leaves the zero-filled `0.0` → `pow(NdH, 0) = 1` → whole-sprite specular blowout; AC 4's default-packing assertion catches the omission in a unit test. Asymmetry: `params2.x` zero-filling to `0.0` is benign (correct for non-shimmer); only `params2.y` is load-bearing. |

## Open questions

None. The prior open questions (a per-`billboard_emitter` strength override; a
spec-mask-only "static sparkle" model) are decided as out of scope in *Design
decisions* (Decision 4, Decision 5); both layer on later without reworking this
spec's contracts.
