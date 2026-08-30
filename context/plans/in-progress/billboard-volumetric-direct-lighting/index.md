# Billboard Volumetric Direct Lighting

## Goal

Make particle billboards respond to direct lights as smoke, not camera-facing
flat surfaces. A red direct light must keep smoke red as the camera moves
around it, while static-light occlusion and animated brightness still apply.

## Scope

### In scope

- A camera-independent, normal-free direct-scatter field for all particle
  billboards.
- Static `static_light_map` direct transport and animated baked direct deltas.
- Isotropic dynamic point, spot, and directional direct response for billboards.
- Existing probe-depth visibility, light reach, animation scales, and optional
  PRL degradation behavior.
- A fixture and manual GPU check covering both sides of a red light/emitter.

### Out of scope

- Changing world, mover, or mesh lighting.
- Changing indirect-SH ambient behavior, fog scattering, or material lighting.
- Billboard self-shadowing, dynamic shadow-map receipt, or static SDF direct
  policy.
- New emitter KVPs, scripting primitives, or sprite-instance fields.

## Design decisions

1. **Normal-free scatter is the billboard direct-lighting model.** For every
   particle billboard, the renderer samples a normal-free RGB direct-scatter
   field at the sprite centre. It replaces the billboard's baked direct-SH
   surface response for `static_light_map` lights and the old surface-like
   static-specular term; it does not choose another fake surface normal. This
   scatter path is the model for every billboard — there is no per-material split
   — with the legacy-fallback branch (Design decision 6) the only other path.
2. **Bake before cosine convolution.** The base field sums each reaching
   static light's `incident_radiance_at_point` radiance (its `.0` tuple element)
   multiplied by the same `soft_visibility` used by direct SH. The animated field stores the matching
   per-light peak delta. Both retain light color, falloff, cone, reach culling,
   and baked occlusion without a receiver normal.
3. **Animated scatter compose mirrors the direct-SH compose.** Before the
   billboard draw, a compute pass writes `base + Σ(active animation scale ×
   delta)` to a 3D `Rgba16Float` composed texture. It is not an independent
   pass: it binds the same live shared animation descriptor and sample buffers,
   the same group-0 `time` and `light_term_mask`, and dispatches at the same
   site and after the same per-frame descriptor flush as `direct_sh_compose`
   (the animated-direct "Pass B"). Its dispatch predicate mirrors
   `direct_compose_should_dispatch`: an initial copy-through base-seeds the
   composed texture before the first billboard draw, a change in the
   `BAKED_DIRECT_ANIMATED` light-term-mask bit re-dispatches, and one settle
   frame follows active→inactive. The dispatch gate keys off each descriptor's
   active flag, never its per-frame evaluated scale, so a curve crossing 0 while
   active still re-dispatches and settles to base. The re-dispatch predicate
   mirrors `direct_compose_should_dispatch`'s whole-mask compare against its own
   `last_composed_mask` (the `BAKED_DIRECT_ANIMATED` bit is one input, not the
   whole rule) so a dev-tools toggle of any direct bit recomposes scatter in
   lock-step with the direct atlas. When `BAKED_DIRECT_ANIMATED`
   is clear the compose zeroes Σ, matching Pass B, so billboard scatter and mesh
   animated-direct never describe different light scales in the same frame. It
   does not reuse direct-SH tiles.
4. **Dynamic direct has no Lambert cosine.** The billboard loop keeps its
   existing influence, falloff, and spot-cone gates, then adds color times
   attenuation without `dot(N, L)`. Its current unshadowed policy remains.
5. **No double count, promotion included.** When scatter data is usable, a
   billboard does not sample `DirectShVolumeSection`, and its `static_light_map`
   entries do not add the old surface-like static-specular term — scatter
   replaces both, with no receiver normal in play. Billboards evaluate promoted
   static-light records in their runtime dynamic-direct loop today
   (`billboard.wgsl` iterates `total_light_count`, which appends promoted static
   records after the dynamic tier), relying on `direct_sh_compose` subtracting
   the baked share of the direct-SH atlas they read at binding 15. Scatter
   replaces that subtracted read with the full, **unsubtracted** static
   contribution, so the scatter path must not also add the promoted runtime term
   for those lights. On the scatter path the billboard runtime loop therefore
   iterates only the genuine dynamic tier — records `[0, light_count)`, the
   existing dynamic-tier count already in the shared uniform
   (`FrameUniforms.light_count`, offset 80..84; `total_light_count = light_count
   + promoted static records`), excluding the appended promoted static records —
   so a promoted static light
   reaches smoke only through its baked scatter value, never twice. The scatter
   compose consequently applies **no** promotion subtraction (unlike
   `direct_sh_compose`); that is correct only because the scatter loop drops the
   promoted records rather than because billboards are unaffected by promotion.
   The static-specular loop is not deleted from the shader: on both paths it
   retains static SDF records through `spec_lights`. Only its surface-like
   `static_light_map` work is legacy-fallback-only (Design decision 6), which
   reproduces current behavior when scatter data is absent. It is not a
   reservation for any other spec. Meshes and movers keep their
   direct-SH/promotion behavior unchanged.
6. **Safe legacy fallback.** Both new sections are optional. Missing,
   malformed, mutually incompatible, policy-oversized, or
   device-limit-incompatible scatter data selects the exact current billboard
   direct path — the `has_scatter` uniform mode (Task 4) is zero,
   the shader samples the direct-SH atlas and iterates `total_light_count` as
   today. Section 48 (animated scatter) exists only for maps that carry
   section 45 (animated baked lights): if section 45 is present, section 48 must
   validate against section 45's affinity and descriptor map, and a missing or
   invalid section 48 disables the scatter feature as a whole rather than
   silently omitting animated direct light. A map with only static scatter
   (section 47, no section 45) needs no section 48. A map whose scatter entries
   come only from animated `static_light_map` sources emits section 47 with zero
   RGB and id-34 validity solely as the anchor for its valid section-48 deltas;
   this is the only all-zero-base exception.

## Acceptance criteria

- [ ] `[manual GPU]` In a room lit only by a static red `light_spot`, smoke at
      a fixed position remains red while the camera crosses from one side of
      the emitter to the space between emitter and light. Its brightness may
      change only with particle position, authored animation, or light range —
      not camera direction.
- [ ] `[manual GPU]` The same red-light test remains dark behind an occluding
      wall and respects the spot cone. It does not gain unshadowed red through
      walls.
- [ ] `[unit]` A baked direct-scatter probe equals the sum of normal-free
      visible light radiance; an occluded source contributes zero. The test
      covers point, spot, and directional lights.
- [ ] `[unit]` Animated scatter uses the same descriptor index, active state,
      brightness, and color scale as section 45 (animated direct SH). At zero
      scale it is base-only; at unit scale it adds exactly the baked peak delta
      (the exact-delta arithmetic is checked by a CPU mirror of the compose
      `base + Σ scale×delta`, since the GPU output is not directly unit-testable).
      With the `BAKED_DIRECT_ANIMATED` mask bit clear, the composed scatter is
      base-only (pin-table row P1).
- [ ] `[unit]` Section 47/48 round-trip and loader validation reject bad
      version, dimensions, CSR shape, descriptor mapping, or payload length
      (section 48 payload length = `affinity_lights.len() × 64 × f16×4`).
      Invalid optional data falls back to legacy billboard direct lighting
      without failing the level load. An animated-only `static_light_map`
      source emits and loads a zero-RGB section-47 anchor plus valid section 48;
      static-only no-contribution maps still omit section 47. Section 48 has an
      independent 64 MiB maximum encoded size: compiler overage withholds 47/48
      together before dense bake/cache materialization, loader overage soft-drops
      scatter before decode, and valid empty P7 pairs remain accepted.
- [ ] `[shader-source gate + unit slot-ordering]` On the scatter path
      (`has_scatter != 0`), the `billboard.wgsl` source omits camera-facing
      `NdotL` and surface-like `static_light_map` specular work, retains static
      SDF records through `spec_lights`, and bounds the runtime dynamic-direct
      loop with `light_count` (not `total_light_count`), while the dynamic path
      keeps range/cone rejection with no Lambert cosine — verified as an
      `include_str!` source gate over `billboard.wgsl` (per the `shader_tests`
      idiom). A CPU slot-ordering test confirms promoted static records append
      after `light_count`, so the bound excludes them. Only the
      `static_light_map` surface-like specular work is legacy-only. (The
      pixel-level "counted exactly once" truth is manual GPU — AC 1.)
- [ ] `[unit]` After extracting the renderer seam, existing direct-SH compose
      behavior is unchanged, and the billboard vertex-stage storage-buffer budget
      is unchanged (section-48 CSR buffers are COMPUTE-visible only, never
      VERTEX). With binding 17 added VERTEX-only, the **forward** pipeline's
      fragment sampled-texture count stays `== 16` (binding 17 adds nothing to
      the shared fragment budget) and the billboard vertex-stage sampled-texture
      count stays within the downlevel/WebGPU default of 16 — asserted via a new
      `vertex_sampled_textures` budget helper alongside the existing
      `vertex_storage_buffers` / forward fragment-count guards.
- [ ] `[manual GPU]` An animated red baked light pulses billboard scatter with
      the wall/other dynamic receivers, has no camera-side pop, and adds a
      measurable but bounded compose cost to `POSTRETRO_GPU_TIMING`.

## Tasks

### Task 1: Extract the renderer direct-resource seam

Split the direct-atlas/compose ownership out of `sh_volume.rs`
(`crates/renderer/src/render/`, ~2,257 lines) before extending it. Keep the
shared SH bind-group construction and existing direct-SH resources
behavior-preserving: current bindings, the `direct_compose_should_dispatch`
predicate and its load/active/zero-transition states, mesh binding 16, and the
billboard vertex storage-budget guard must keep their contracts. If the shared
group-3 BGL builder (`sh_bind_group_layout_entries`) moves, re-point or
re-export it so the budget guards that count it — `pipeline_layout.rs` and
`tests/pipeline_budget_tests.rs` — still resolve. The extracted seam becomes the
owner for the new billboard scatter texture and compose pass.

### Task 2: Add optional direct-scatter PRL sections and loader validation

Register `BillboardDirectScatterVolume` (ID 47) and
`AnimatedBillboardDirectScatterDeltaVolumes` (ID 48) in
`postretro-level-format` — 46 is the shipped `CellVisibility` section, so 47/48
are the next free IDs. Section 47 carries a versioned base grid mirroring the
section-34 origin, cell size, dimensions, and x-fastest probe order; each probe
is one `Rgba16Float` value (RGB = normal-free static direct scatter, A = the
per-probe validity mirrored from section 34, binary 0/1). Section 48 is
versioned and **reuses section 45's `animation_descriptor_indices` table and CSR
(`affinity_offsets` /
`affinity_lights`, keyed by `AnimatedBakedLights`)** but defines its own dense
payload: no `valid_probe_masks` / `cell_levels` and no coarsening — a fixed
`4×4×4 = 64`-probe block per CSR entry, each probe one `Rgba16Float` (RGB =
delta, A = reserved zero), x-fastest within the block. Section 47 may carry
zero RGB at every valid probe only as section 48's animated-`static_light_map`
anchor; a static-only map with no contribution omits it.

Thread both through `postretro-level-loader::LevelWorld`. Validate section 47
against the base SH grid. Validate section 48 against section 45's affinity
layout and descriptor map, and its payload length against
`affinity_lights.len() × 64 × f16×4`. Treat either parse/validation failure, or
a missing section 48 for a map carrying section 45, as absent scatter data and
select the legacy billboard path; do not reject the map. Do **not** copy the
adjacent section-45 (`AnimatedDirectShDeltaVolumes`) loader validation, which
hard-fails (returns `Err`) on mismatch — 47/48 validation returns absent
(the scatter data is dropped), never `Err`.

### Task 3: Bake base and animated normal-free scatter

Add a focused compiler module rather than extending `direct_sh_bake.rs`
(`crates/level-compiler/src/`, ~2,274 lines). Reuse the direct baker's
static-light filtering and affinity reach decomposition, plus
`incident_radiance_at_point` (`sh_bake.rs`, the pre-cosine normal-free RGB
radiance — its `.0` tuple element) and `soft_visibility` (`lightmap_bake.rs`),
both `pub(crate)` helpers reachable from a new `level-compiler` module. Do
**not** call `bake_probe_direct_rgb` (`sh_bake.rs`) — it applies
`apply_cosine_lobe_rgb` and SH projection, which would reintroduce the receiver
normal this bake exists to avoid; reimplement the inner light loop to sum the
pre-cosine `incident_radiance_at_point(...).0 × soft_visibility` directly. Emit
one base RGB value per valid probe for static-light-map sources and one dense
64-probe peak RGB delta per animated baked light/cell (peak = the delta at the
animation's maximum brightness state, matching section 45's unit-scale delta).
The base and delta must share the existing grid and
section-45 `AnimatedBakedLights` indexing. Version/cache the new bake outputs
independently; changing their radiance or payload computation invalidates only
their cache stages. When section 45 leaves animated `static_light_map` scatter
entries but no non-animated static-light-map source exists, emit the matching
id-34-validity section-47 base with zero RGB so section 48 has a usable anchor.
Do not emit that zero base for a static-only no-contribution map. Append both
optional sections during PRL assembly. Before reading the animated-scatter cache
or materializing dense blocks, predict id 48's exact encoded size from finalized
id 45. The independent 64 MiB cap is not a quality reducer: overage withholds
ids 47/48 together and keeps id 45 unchanged. Packing repeats this pair guard.

### Task 4: Compose and consume scatter in the billboard pass

Add renderer-owned upload resources for the base 3D scatter texture, an optional
composed texture, and the section-48 CSR buffers. Binding 17 resolves like the
direct-SH atlas at binding 15: it samples the composed texture when animated
scatter (section 48) is present, the base scatter texture when only static
scatter (section 47) exists, and the 1×1×1 dummy when scatter is unavailable.
Append the sampled texture at
group 3 binding 17, **`VERTEX`-visible only** — billboards light per vertex, so
the scatter read replaces the per-vertex `sample_sh_direct` read, and the
forward and fog pipelines share this group-3 BGL. Forward sits at exactly 16
fragment sampled textures (the downlevel/WebGPU floor); a `FRAGMENT`-visible
entry here would push it to 17 and panic `create_pipeline_layout`, so binding 17
must NOT carry `FRAGMENT`. Do not change existing bindings. Add a
`vertex_sampled_textures` budget helper (mirroring the existing
`vertex_storage_buffers`) and assert with it that the forward fragment
sampled-texture count stays 16 and the billboard vertex sampled-texture count
stays within the downlevel default (AC 7). The section-48 CSR storage buffers
stay `COMPUTE`-visible only — never `VERTEX` (the load-bearing property shared
with `anim_descriptors` / `anim_samples`, which protects the billboard vertex
storage budget), so the vertex-stage storage-buffer count is unchanged. Add a
single `has_scatter` mode word to the shared
128-byte `FrameUniforms` ABI (the 4-way contract mirrored by the Rust writer,
`forward.wgsl`, `billboard.wgsl`, and `wireframe.wgsl`) in its one free slot at
offset 112..116 (the retired `_dynamic_direct_pad`); `UNIFORM_SIZE` stays 128.
Zero means unavailable/dummy/legacy, one means immutable section-47 base, and
two means section-48 composed texture. Both real modes stay nonzero so existing
boolean availability semantics remain valid. A static-base mode samples only
when `BAKED_DIRECT_STATIC` is set. A composed mode samples when either baked
direct bit is set because compose independently gates its base and deltas.
Update the `frame_uniforms.rs` tests that assert the 112..116 (and 104..128)
tail is zero. Add no new dynamic-tier count: the existing
`FrameUniforms.light_count` (offset 80..84) already is it (`total_light_count =
light_count + promoted static records`). The renderer sets `has_scatter` zero
when section 47 is absent or invalid, or when section 45 is present but section
48 is absent, invalid, or incompatible (a valid static-only map — section 47, no
section 45 — sets static-base mode and samples the base texture); in the
`has_scatter == 0` case it binds binding 17 to a dummy 1×1×1 `Rgba16Float`
texture so the bind-group layout never varies with map content.
Before allocating or binding section-48 buffers, require each dense-delta,
CSR-offset, CSR-light, and descriptor-index buffer to fit both
`max_storage_buffer_binding_size` and `max_buffer_size`. Any failure selects the
same whole-scatter dummy/legacy mode; it is not a renderer initialization error.

The compose compute pass mirrors `direct_sh_compose` (template:
`direct_sh_compose.rs` and `direct_compose_should_dispatch`) — Task 4's agent
should follow that source, since the full contract is: bind the same live shared
animation descriptor and sample buffers and group-0 `time` / `light_term_mask`;
dispatch at the same site, after the same per-frame descriptor flush, and before
the billboard draw in the same encoder; predicate = copy-through base-seed on
level load (before the first billboard draw), re-dispatch on a whole-mask change
against its own `last_composed_mask`, active-flag gate (not evaluated scale), one
settle frame after active→inactive; zero Σ when `BAKED_DIRECT_ANIMATED` is clear.
Register it as a `POSTRETRO_GPU_TIMING` pass alongside
`animated_direct_sh_compose`. It never applies static-promotion subtraction —
correct only because the scatter runtime loop drops promoted records (below).

In `billboard.wgsl`, when `has_scatter != 0`, depth-aware trilinear
interpolation reads the scatter texture at the sprite centre (weighting probes
by the A-channel validity as the SH read does), routes static-light-map direct
through that result instead of its surface-like static-specular term and the
direct-SH surface response, retains static SDF records through `spec_lights`,
and bounds the runtime dynamic-direct loop to `[0, light_count)` (the existing
dynamic-tier count) so promoted static records are not double-counted. The
static-specular loop stays in the shader: static SDF records reach it on both
paths, while its `static_light_map` surface-like work runs only when
`has_scatter == 0` (the legacy-fallback branch). This spec reserves nothing
there for another spec. Make runtime dynamic direct isotropic by removing only
the Lambert cosine after existing influence/range/cone work. Preserve the
legacy shader branch (direct-SH atlas read, `total_light_count` loop bound) when
`has_scatter == 0`.

### Task 5: Regression fixture and durable documentation

Extend `content/dev/maps/spawner-test.map` with a red-light smoke station that
has a clear camera path from the light-facing side to the space between the
light and emitter, plus a nearby occluded comparison. Exercise the existing
animated alarm fixture too. Add the unit tests the Pin table rows describe.
Document billboard's direct-scatter policy, legacy fallback, promotion handling,
and timing check in `context/lib/rendering_pipeline.md` §7.4, and update adjacent
billboard-lighting references that still describe all direct terms as `N = V` —
including §9's "For meshes and billboards, direct SH subtraction handles the
handoff", which must now note the scatter path drops promoted records instead of
subtracting. Also scope §4's billboard statements that now hold only on the
legacy branch — the "billboards sample the composed direct-SH atlas at binding
15, gated by `has_direct`" line, the "billboards receive the same pulse/color
term" via id 45 line, and the receiver-table Billboard row (direct-SH base | id
45 composed delta) — to the legacy path, since on the scatter path billboards
read neither binding 15 nor id 45. Add the new scatter-compose pass to §12's
"Passes measured" list.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — renderer seam extraction and format/loader contract do not overlap.

**Phase 2 (sequential):** Task 3 — emits the sections defined by Task 2.

**Phase 3 (sequential):** Task 4 — consumes Task 1's seam, Task 2's loader output, and Task 3's payloads.

**Phase 4 (sequential):** Task 5 — validates the integrated renderer path.

## Wire format

Both new sections are little-endian and optional.

| Section | ID | Header | Payload | Empty encoding |
|---|---:|---|---|---|
| `BillboardDirectScatterVolume` | 47 | `u8 version`, `f32×3 grid_origin`, `f32×3 cell_size`, `u32×3 grid_dimensions` | `f16×4` per x-fastest probe (`x*y*z` probes); RGB = normal-free static direct scatter, A = section-34 per-probe validity (binary 0/1) | Omitted when no static-light-map source contributes, except a map with animated-only `static_light_map` scatter entries emits a zero-RGB base as that companion's required grid/validity anchor. Never emit a zero base for a static-only no-contribution map. |
| `AnimatedBillboardDirectScatterDeltaVolumes` | 48 | `u8 version`, `u8 affinity_factor`, `u32×3 affinity_dims`, `u32 animated_light_count`, `u32×animated_light_count animation_descriptor_indices` | CSR offsets, CSR `AnimatedBakedLights` indices, then a dense `64 × f16×4` block per CSR entry (x-fastest within the 4×4×4 block); RGB = delta, A = reserved zero. No `valid_probe_masks` / `cell_levels`. Payload length = `affinity_lights.len() × 64 × f16×4`. Maximum encoded section size is 64 MiB by compiler/loader policy; this adds no wire field or version change. | offsets of zero for every cell, no indices or payload; the valid empty pair remains emitted for base seeding. |

## Boundary inventory

| Name | Rust | Wire | WGSL | FGD KVP |
|---|---|---|---|---|
| Base scatter | `BillboardDirectScatterVolumeSection` | section 47 | sampled 3D texture | n/a |
| Animated scatter | `AnimatedBillboardDirectScatterDeltaVolumesSection` | section 48 | compose CSR buffers | n/a |
| Scatter texture | renderer-owned resource | n/a | group 3 binding 17 (`VERTEX`-only) | n/a |
| Scatter resource mode | shared `FrameUniforms` `has_scatter` (offset 112..116): 0 unavailable, 1 static base, 2 composed animated | n/a | uniform, 4-way ABI; nonzero remains the availability check | n/a |
| Dynamic-tier bound | existing `FrameUniforms.light_count` (offset 80..84) | n/a | scatter-path runtime-loop bound | n/a |
| Animation key | `AnimatedBakedLights` index | section-48 descriptor/CSR index | compose descriptor index | existing light animation |

## Pin table

Concrete orderings the renderer must honor. Sections named by their final IDs
(47 base, 48 animated). **Testability:** P1, P3, P5, P6, P7, P9, P10, P11 are
writable headless as CPU-predicate or `include_str!` shader-source gates (P3/P11
mirror the existing `direct_compose_should_dispatch` tests; P9 = the loop-bound
source gate plus the `light_count`-vs-`total_light_count` slot-ordering test).
P2 (copy-through before the first billboard draw) and P4 (one flushed descriptor
shared by both composes) need a dispatch-order / flush-count seam Task 4 must add
to be headless-testable. P8's "counted once" pixel truth is manual GPU (AC 1);
its headless surrogate is P9.

| # | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Sections 47+48 valid; red animated light active | Frame T: `BAKED_DIRECT_ANIMATED` set; T+1: cleared | T+1 mesh animated-direct → base **and** billboard scatter → base in the same frame; never disagree by one delta. |
| P2 | Sections 47+48 valid; no animation ever activated | First rendered frame after level load | Composed scatter texture is copy-through base-seeded before the first billboard draw; billboard reads base, never uninitialized/prior-level data. |
| P3 | Animated light active, curve ramps through scale 0 | Frame where evaluated scale == 0, active flag still set | Scatter compose still dispatches; composed texture == base (Σ = 0), not the prior frame's base+delta. |
| P4 | Descriptor mutated by scripting bridge on tick N | Bridge write → per-frame descriptor flush → scatter compose + direct compose in one encoder | Both composes read the identical flushed descriptor/sample/`time`; scatter binds the live shared buffers, owns no private copy. |
| P5 | Sim rate ≠ render rate | Several render frames per descriptor update | Every render frame both composes read the same `time` and descriptor sample; scatter never reads a different sample than the direct compose. |
| P6 | Section 45 present, section 48 absent (or present-but-invalid/incompatible/oversized/device-incompatible), mid-session reload | Reload swaps to this map | Scatter disabled whole; `has_scatter` zero; binding 17 → dummy 1×1×1; billboard samples legacy direct-SH path and `total_light_count`; no dangling texture/CSR; bind-group layout unchanged. Id 45 remains independently available. |
| P7 | Section 48 present, empty CSR (zero animated lights) | Level load → every frame | Compose seeds and writes base (Σ over N=0 = base); billboard scatter == base; no skipped-dispatch stale window. |
| P8 | Promoted static light near a mesh, also reaching smoke | Frame with light promoted at weight `w` | Mesh: `(1−w)` baked-SH-subtracted + `w` runtime term. Billboard scatter path: full static scatter, runtime loop skips the promoted record → light counted once, no `w`-scaled double add. |
| P9 | Sections 47+48 valid; promotion fires this frame | `light_count` snapshotted in the per-frame uniform flush → `lights` buffer rebuilt (dynamic prefix patched in place, N promoted records appended) → billboard scatter draw | Scatter loop bound `[0, light_count)` still equals the dynamic-prefix record count; the promoted tail `[light_count, total_light_count)` is never read; no dropped dynamic light, no read into a promoted record. Promotion cannot move the boundary. |
| P10 | Section 48 present, then an in-session state change tries to invalidate scatter without a reload | Any in-session frame | `has_scatter` mode, binding 17 (base/composed/dummy), and the loop-bound choice are all load-fixed and never mutated per-frame; the only present→absent transition is a reload (P6). No frame exists where `has_scatter` is nonzero but binding 17 is the dummy, or the loop is bounded to `light_count` while sampling the legacy atlas. |
| P11 | Valid 47-only map and valid 47+48 map; dev-tools clears `BAKED_DIRECT_STATIC` while animated remains set | Frame T mask change → composed passes evaluated | The 47-only static-base mode is dark; it cannot expose immutable base through the animated bit. The 47+48 composed mode re-dispatches on the whole-mask change, drops static in lock-step with direct SH, and may still expose animated deltas. No billboard shows a term the mask cleared. |

## Cross-spec coordination

This spec makes normal-free scatter the billboard direct model for every
billboard on the scatter path; the old surface-like `static_light_map` work in
the vertex-stage static-specular loop is legacy-only, while static SDF records
remain on that loop through `spec_lights` on both paths. No reservation exists
for another spec. The downstream `billboard-specular-shimmer` spec adds a
second, opt-in billboard lighting model (per-fragment specular from a baked
normal map) and owns the per-material split between the two — the classification
flag, the fragment-stage specular path, and the re-scoping of scatter to the
non-shimmer default. That partition is entirely shimmer's to build. Shimmer
depends on this spec only for the established default (scatter for all
billboards); it builds its own per-fragment specular path rather than inheriting
one.

**`billboard.wgsl` / `VertexOutput` merge coordination with `prm-array-layers`.**
The foundational `prm-array-layers` spec restructures `billboard.wgsl` — the
sprite binding becomes `texture_2d_array`, the strip UV math is replaced by
per-layer sampling, and `VertexOutput` gains a flat-interpolated `frame_idx`.
This spec's Task 4 also edits `billboard.wgsl` (the scatter compose/read) and may
grow `VertexOutput`. The two are orthogonal in intent — sprite sampling vs. the
lighting term — but touch the same shader and struct, so this spec's edits rebase
onto the array-migrated shader; it does not reintroduce strip UV math or collide
with the `frame_idx` field. No dependency, only a merge point.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Billboards are normal-free on the scatter path: smoke direct color is independent of camera direction. | Task 3 bakes normal-free transport; Task 4 samples it without `N = V`. | Legacy fallback must remain explicit; on the scatter path, no static-light-map specular or direct-SH double path. | AC 1, 6, 8 |
| Baked occlusion and cone reach remain authoritative. | Task 3 reuses direct-bake radiance, visibility, and reach logic. | Task 4 may only interpolate baked values; it cannot replace them with `spec_lights`. | AC 2, 3 |
| Animated scatter and animated direct SH describe the same light scale. | Task 2 pins the shared descriptor map; Task 3 emits matching deltas; Task 4 compose shares live buffers, `time`, and the `BAKED_DIRECT_ANIMATED` mask gate. | Task 4 compose uses the shared active/color/brightness values and the mask gate, and treats mismatch as absent. | AC 4, 5, P1 |
| No receiver double-counts a physical direct term, promotion included. | Task 4 replaces the billboard's static-light-map direct SH with unsubtracted scatter and bounds the runtime loop to `[0, light_count)` (the existing dynamic-tier count). | On the scatter path a promoted static light reaches smoke only through scatter, never also through a `w`-scaled runtime term; the scatter compose applies no promotion subtraction because the loop drops promoted records. The dynamic prefix `[0, light_count)` is immovable by promotion (P9). Static promotion behavior for meshes/movers is unchanged; SDF and dynamic policies stay disjoint. | AC 6, P8, P9 |

## Open questions

None. The first manual GPU pass may tune authored light intensity only; it does
not change the normal-free transport contract or add an author-facing scale.
