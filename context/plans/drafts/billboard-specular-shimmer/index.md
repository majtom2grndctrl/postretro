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
  3) and a normal texture (binding 4), reusing the existing filtering sampler
  at binding 1, and binding the `SPECULAR`/`NORMAL` slot views that the
  (prerequisite) sprite-PRM load path parses.
- Extending `SpriteDrawParams` with a material field carrying the shimmer flag
  and the per-collection specular parameters (intensity, exponent).
- Making the per-collection specular intensity (today hardcoded `0.3`/`0.45`)
  and specular exponent author-controllable per collection through the sprite
  draw-contract path, with the current values as defaults.
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
- **Environment/reflection-probe specular on billboards.** Shimmer is direct
  static-light specular only; it does not sample the reflection cubemaps.
- **Billboard self-shadowing or normal-mapped diffuse.** The normal map drives
  the specular term only; the diffuse/SH/scatter terms remain normal-free as
  today.

## Direction

**Problem.** The billboard shader computes its entire lighting term per vertex
at the sprite center with `N = V` and folds it into a single interpolated
`VertexOutput.lighting` (`billboard.wgsl` `vs_main`); the fragment stage does no
lighting. A single center value cannot express a highlight that lives on part
of the sprite face, so no amount of tuning the existing `static_specular` loop
produces a moving glint — the term is spatially constant by construction. The
missing ingredient is a per-fragment surface normal, which requires a normal map
and moving the specular evaluation into the fragment stage.

**Prior commitments.**
- The billboard shader already runs a multi-source Blinn-Phong static-specular
  loop over the chunk light list (`billboard.wgsl` `vs_main`, the `use_specular`
  block; helper `blinn_phong`, `spec_exp = 4.0`, scalar `spec_int` from
  `draw_params.params.y`). Shimmer reuses this loop's light gathering, cone, and
  attenuation math verbatim; it changes only where the loop runs (fragment, not
  vertex) and what normal it uses (`N'` from the map, not `N = V`).
- The world/model forward path already samples a per-texel specular mask
  (`spec_texture`, group 1 binding 2, R8) and a tangent-space normal map
  (`t_normal`, group 1 binding 4) in `forward.wgsl`. Shimmer mirrors that
  sampling convention on the billboard bind group rather than inventing one.
- The `.prm` format already carries `SPECULAR`/`NORMAL` slots (`PrmSlots`,
  `crates/level-format/src/prm.rs`); the amended `billboard-sprite-prm-baking`
  spec bakes and parses them for sprites. Shimmer consumes the parsed slot mask
  as its classification signal, so the opt-in reuses data that already exists at
  load rather than adding a parallel declaration.
- Per-collection draw parameters (lifetime, emissive) resolve through
  `SpriteCollectionCandidate` → `resolve_sprite_collection_draw_contract`
  (`crates/postretro/src/startup/lifecycle.rs`); `spec_intensity` is currently
  hardcoded at the `register_smoke_collection` call site, not in that contract.
  Shimmer moves the specular parameters into the same per-collection contract so
  they are authorable where lifetime/emissive already are.

*Divergence.* Billboard lighting is deliberately hoisted to the vertex stage
today, justified in-shader by every input being constant across the quad.
Shimmer breaks that premise for shimmer collections only: their specular moves
to the fragment stage. This is an intentional, per-material divergence — the
isotropic path keeps its per-vertex constancy and its justifying comment intact.

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
path), sets a per-collection shimmer flag, and packs it into `SpriteDrawParams`.
No FGD KVP, no descriptor field, no second source of truth: providing the normal
map *is* the opt-in. A collection with a `SPECULAR` slot but no `NORMAL` slot is
**not** shimmer — the spec mask alone cannot move a highlight (see *Alternatives
rejected*); its spec slot is ignored by the billboard path until a normal slot
accompanies it. This keeps the discriminator single-valued.

### Decision 2 — Shimmer specular is per-fragment; isotropic stays per-vertex

The shader keeps its current per-vertex lighting for non-shimmer collections
unchanged (byte-for-byte control flow; the isotropic path and its constancy
comment survive). For shimmer collections, the static specular term is computed
in the fragment stage instead, where the normal map varies per texel. The
ambient floor, SH indirect/direct, and dynamic diffuse terms remain per-vertex
for both models — only the static specular term moves, because it is the only
term the normal map changes.

### Decision 3 — Tangent frame is the camera-facing basis, rotated by sprite spin

Billboards have no authored tangents, so the world path's mesh-derived TBN
(`reconstruct_tbn_normal`, which needs `mesh_n` + `world_tangent` +
`bitangent_sign`) does not apply. The tangent frame is instead `(right, up, V)`
from the existing `camera_right_up` basis and `V = normalize(camera_position -
sprite_pos)`, with `right`/`up` rotated by the sprite's `rotation` so the glint
pattern spins with the sprite. The `NORMAL` slot is BC5 (`Bc5RgUnorm`), so the
tangent-space sample is decoded the way `sample_normal`
(`material_shading.wgsl`) does — `(nx, ny) = rg·2 − 1`, `nz = sqrt(max(0, 1 −
nx² − ny²))` — and mapped to `N' = normalize(nx·right + ny·up + nz·V)`
(renormalized, since BC5 quantisation plus filtering leaves the sample slightly
off unit length). Light direction `L` is still taken from the sprite center
(billboards are small; per-texel `L` variation is negligible and the shimmer
comes from `N'`, not `L`).

## Prerequisites

This spec consumes three upstream pieces that must land before its review begins:

1. **`prm-array-layers` (lands first, foundational)** migrates the billboard
   sprite texture path from a stitched strip to per-frame `texture_2d_array`
   layers, extends the PRM format with a file-header `layer_count`, and routes
   the per-fragment `frame_idx` (flat-interpolated on `VertexOutput`) that
   shimmer samples the spec/normal maps by. Shimmer inherits an already-D2Array
   billboard shader and bind layout; its own `VertexOutput` growth (the sprite
   `rotation` for the tangent frame) **coordinates with** the `frame_idx` field
   that spec adds — the two must not collide in the struct. Shimmer samples the
   spec/normal maps per fragment at `layer = frame_idx`, not from a strip.
2. **`billboard-sprite-prm-baking` (amended)** bakes optional per-collection
   `SPECULAR`/`NORMAL` slots into the sprite `.prm` (as array-layer slots atop
   the `prm-array-layers` format) and parses/uploads them at runtime, exposing
   the loaded slot mask and the slot texture views to the billboard pass.
   Shimmer consumes: the slot mask (classification) and the two texture views
   (binding).
3. **`billboard-volumetric-direct-lighting` (amended)** scopes its normal-free
   isotropic-scatter path to non-shimmer (default) billboards and preserves —
   does not delete — the static-specular path for shimmer-flagged materials.
   Shimmer owns that preserved path's per-fragment form.

If any of the three is not yet merged when this spec is picked up, that is a
sequencing block, not a scope change here.

## Acceptance criteria

- [ ] `[manual GPU]` On a shimmer collection (normal map present) lit by one
      static point light, the specular highlight occupies a sub-region of the
      sprite face and **moves across the face** as the light orbits the sprite
      (or the camera orbits a fixed light) — not a whole-sprite brightness
      change. On the same scene, a non-shimmer collection (no normal map) shows
      the current whole-sprite behavior, unchanged.
- [ ] `[unit]` A collection whose loaded `.prm` slot mask contains `NORMAL` is
      classified shimmer and its `SpriteDrawParams` material field carries the
      shimmer flag set; a collection without `NORMAL` (diffuse-only, or
      diffuse+specular) is classified non-shimmer with the flag clear.
- [ ] `[unit]` The billboard group-1 bind group layout exposes a specular
      texture at binding 3 and a normal texture at binding 4; a non-shimmer
      collection binds the shared 1×1 placeholder views at 3/4 and renders
      identically to before this spec (a golden or pixel-equality check against
      the pre-change billboard output on a smoke fixture).
- [ ] `[unit]` `SpriteDrawParams` round-trips the material field: the shimmer
      flag, specular intensity, and specular exponent pack and unpack at their
      documented offsets, and the existing `params` vec4
      (frame_count/spec_intensity/lifetime/emissive) is unchanged in layout
      (`draw_params_layout` still holds for its bytes).
- [ ] `[unit]` The WGSL shader validates (naga) and the `SpriteInstance` stride
      test (`billboard_wgsl_sprite_instance_stride_matches_cpu`) still passes;
      the shimmer branch adds no per-particle storage-buffer field.
- [ ] `[unit]` The per-collection specular intensity and exponent flow from the
      sprite draw-contract path: a collection that declares them uses the
      declared values; a collection that declares neither uses the defaults
      (intensity `0.3`, exponent `4.0`), and the weapon-impact collection keeps
      its `0.45` intensity. A conflict between two candidate sources for the
      same collection is rejected the way lifetime/emissive conflicts already
      are.
- [ ] `[manual GPU]` A spinning shimmer sprite's glint pattern rotates with the
      sprite (the tangent frame tracks `rotation`), and the sprite viewed at
      distance does not shimmer-crawl (the normal/spec maps are mipped by the
      prerequisite bake and selected at distance).

## Tasks

### Task 1: Thin slice — end-to-end per-fragment shimmer for one classified collection

Prove the boundary from parsed slot mask → bind group → fragment shader → screen
on a single shimmer collection before fanning out. Extend the billboard group-1
bind group layout in `crates/renderer/src/render/smoke.rs`
(`sprite_sheet_bind_group_layout_entries`) with a specular texture at binding 3
and a normal texture at binding 4, both `texture_2d<f32>` sampled through the
existing filtering sampler at binding 1; bind the `SPECULAR`/`NORMAL` slot views
the prerequisite PRM-load path parsed, falling back to a shared 1×1 placeholder
view when a slot is absent. Add a material field to `SpriteDrawParams` (a second
`vec4<f32>`, `params2`) carrying `x = shimmer_flag` (0/1), `y = spec_intensity`,
`z = spec_exponent`, `w = reserved`; set the flag from whether the loaded slot
mask contains `NORMAL`. In `billboard.wgsl`, declare `spec_texture` (group 1
binding 3) and `normal_texture` (group 1 binding 4); when `params2.x != 0`,
compute the static-specular term in the **fragment** stage instead of the vertex
stage — build the tangent frame `(right, up, V)` (recomputed in the fragment
from the same basis the vertex uses, rotated by the sprite `rotation` passed
through `VertexOutput`), sample the BC5 normal and decode/reconstruct `N'` the
way `sample_normal` (`material_shading.wgsl`) does (RG `·2−1`, `nz = sqrt(1 −
nx² − ny²)`, renormalize; the billboard copies this decode the way it already
copies `blinn_phong`, since it samples through its own `sample_post_retro` +
`sprite_sampler`, not the world `aniso_sampler`), sample the `.r` spec mask, and
run the existing chunk-light-list Blinn-Phong loop per fragment with `N'` and
`params2.y`/`params2.z`; when `params2.x == 0`, keep the current per-vertex path
untouched. Pass the sprite `rotation` and any center-derived
loop inputs the fragment now needs through `VertexOutput`. Register one
hand-built shimmer collection (normal map present) in a dev path and confirm
manually that a moving light sweeps a highlight across the face. This task is
the widest, single-collection slice; author-facing parameters and the fixture
come after.

### Task 2: Material classification and per-collection specular parameters

Route the shimmer flag and specular parameters through the per-collection
draw-contract path so they are data, not a hardcoded slice constant. Extend
`SpriteCollectionCandidate` and `resolve_sprite_collection_draw_contract`
(`crates/postretro/src/startup/lifecycle.rs`) to resolve a per-collection
`spec_intensity` (default `0.3`) and `spec_exponent` (default `4.0`) alongside
lifetime/emissive, rejecting conflicting candidate values with the same
conflict-message shape the function already uses for lifetime and emissive; keep
the weapon-impact collection's `0.45` intensity by supplying it as that
collection's candidate value rather than a call-site literal. Thread the
resolved intensity/exponent into `register_smoke_collection` /
`SmokePass::register_collection` in place of the current hardcoded
`spec_intensity` argument, and pack them into `SpriteDrawParams.params2` at the
offsets Task 1 defined. The shimmer flag itself is set from the loaded slot mask
inside the register path (Decision 1), not from the draw contract — the contract
carries the specular *strength*, slot presence carries the *classification*.
Update the `draw_params` pack helper (`build_draw_params` in `render/smoke.rs`)
and its layout tests to cover the widened uniform.

### Task 3: Dev fixture and durable documentation

Add a shimmer sprite station to a dev map with a clear relative-motion path: a
static point (or spot) light and a shimmer emitter positioned so orbiting the
camera sweeps the highlight across the sprite face, plus a non-shimmer smoke
emitter nearby for the unchanged-behavior comparison the AC names. Author the
shimmer collection's `_normal` (and optional `_spec`) companion frames so the
prerequisite bake produces the slots. Document the two billboard lighting models
(isotropic-scatter default vs specular-shimmer opt-in), the slot-presence
classification, the camera-facing tangent frame, and the per-fragment-vs-
per-vertex split in `context/lib/rendering_pipeline.md` §7.4, and update any
adjacent billboard-lighting reference that describes billboard specular as a
single center-evaluated `N = V` term to scope that description to the isotropic
model.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the tangent-frame,
fragment-branch, and bind-group boundary assumptions on one collection before
any author-facing surface is built.

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
| Sprite specular map | `SPECULAR` slot view bound at group 1 binding 3 | `.prm` `slot_mask` bit 1 (`R8Unorm`, linear) | `spec_texture` (g1 b3), sampled `.r` | n/a |
| Sprite normal map | `NORMAL` slot view bound at group 1 binding 4 | `.prm` `slot_mask` bit 2 (`Bc5RgUnorm`, linear) | `normal_texture` (g1 b4), RG-decoded + Z-reconstructed | n/a |
| Specular intensity | `SpriteCollectionCandidate` → draw contract → `register_collection` | n/a | `draw_params.params2.y` | n/a |
| Specular exponent | `SpriteCollectionCandidate` → draw contract → `register_collection` | n/a | `draw_params.params2.z` | n/a |

## Wire format

No new binary surface. Sprite `SPECULAR`/`NORMAL` slots ride the existing `.prm`
format (produced/parsed by the prerequisite). `SpriteDrawParams` is a
renderer-internal uniform, not a wire type; growing it from one `vec4` to two is
a CPU/GPU layout change with a matching layout test, not a serialized-format
change.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Non-shimmer billboards render identically to pre-spec output | Task 1 (fragment branch gated by `params2.x`; placeholder views at b3/b4) | Task 1 — the `params2.x == 0` path must not alter the per-vertex lighting term or its inputs; Task 2 must not change the default specular values a smoke collection resolves | AC 1, AC 3 |
| A billboard's static specular is evaluated exactly once per fragment (shimmer) or once per vertex (isotropic), never both | Task 1 (mutually exclusive branch on `params2.x`) | Task 1 — the vertex path must skip the static-specular loop for shimmer collections so it is not double-added after the fragment computes it | AC 1, AC 5 |
| Classification has one source: `NORMAL`-slot presence | Task 1 (flag set from loaded slot mask), Decision 1 | Task 2 — the draw contract carries specular *strength* only; it must not carry or override the classification flag | AC 2, AC 6 |
| `SpriteInstance` GPU layout and per-particle budget unchanged | Task 1 (new data rides `SpriteDrawParams`, a per-draw uniform, not the instance buffer) | Task 1 — passing `rotation`/center inputs to the fragment uses `VertexOutput`, not new `SpriteInstance` fields | AC 5 |

## Open questions

- **Where specular strength is authored.** Task 2 routes intensity/exponent
  through the descriptor-backed draw contract (where lifetime/emissive live).
  Whether shimmer collections also warrant a per-`billboard_emitter` override is
  deferred — the draw-contract path covers per-collection authoring, which is
  the material's natural grain; a per-instance override is a later addition if a
  use case appears. Not a blocker.
- **Spec-mask-only collections.** Decision 1 ignores a `SPECULAR` slot with no
  `NORMAL` slot on the billboard path. If a "static sparkle mask, no moving
  glint" look is later wanted, it would be a third classification, not a tweak
  to this one. Recorded, not built.
