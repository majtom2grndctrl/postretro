# Forward Dynamic-Light Visibility Cull

> **Status:** ready (promoted after review-draft-spec + review-implementability). Sibling of
> `context/plans/drafts/perf-shadow-caster-culling/` — both attack
> large-map dynamic-light cost, on different frame stages (this = the forward shading loop; that =
> the shadow world re-raster). Independent; no build-order dependency between them.
> **Related:** `context/lib/rendering_pipeline.md` §2 (portal visibility), §4 (dynamic direct),
> §7.1 step 3 (light list upload), §7.3 (world light loop), §10 (per-stage binding budgets) ·
> `context/lib/build_pipeline.md` §PRL section IDs (Cells id 38, ChunkLightList id 23) ·
> `context/plans/done/perf-dynamic-light-pvs-cull/` (the own-cell-gate history this must not
> repeat) · `context/plans/drafts/perf-shadow-caster-culling/` (sibling; see "Relationship" below).

## Problem

The forward world pass evaluates **every dynamic light in the level** for **every drawn fragment**,
with no per-frame visibility cull. The engine computes a portal-visible cell set every frame
(`determine_visible_cells`, `crates/visibility/src/visibility.rs:471`), but the light loop never
sees it:

- Light selection runs **once at level load**: `filter_dynamic_lights`
  (`crates/renderer/src/render/renderer_lighting.rs:93–108`) keeps every `is_dynamic` light with no
  spatial test, from both assembly paths (`renderer_resources.rs:149–153` on level install,
  `renderer_full_init.rs:50–53` at init). `full.light_count = level_lights.len()`
  (`renderer_resources.rs:168`) — the map-wide dynamic count, uncapped.
- The forward fragment loop (`crates/renderer/src/shaders/forward.wgsl:1093–1219`) iterates
  `uniforms.light_count`. Its only cull is the **per-fragment** influence-sphere early-out
  (`forward.wgsl:1095–1103`) — which still costs one `light_influence[i]` storage fetch, a dot, and
  a branch per light per fragment, even for a light three rooms away that cannot touch anything on
  screen.

So the shading loop scales with the *map-wide* dynamic-light count. On `stress-warren-lit`
(~157 dynamic lights; see the sibling shadow spec), a camera in one portal-isolated room still pays
157 loop iterations × every drawn fragment, when typically a handful of lights can reach the drawn
scene. Influences are packed once at load (`renderer_resources.rs:239–250`); a light with no
influence record degrades to an uncullable sentinel (`uncullable_light_influence`,
`renderer_lighting.rs:44–49`) — radius `f32::MAX`, never early-outed.

The shadow path already does a per-frame reachability cull
(`shadow_candidate_reaches_visible_cell`, `renderer_lighting.rs:73–89`, driven from
`renderer_light_slots.rs:79–127`) — but deliberately against the **wide** portal-reachable set, and
its result feeds only slot ranking, never the forward loop. The forward loop has no equivalent.

## Goal

Each frame, build the set of dynamic lights whose influence sphere can reach any **drawn** cell,
and make the forward world loop iterate only that set — with zero change to any drawn pixel, zero
change to the light/influence buffer layouts and their index-space contracts, and no effect on
shadow-slot eligibility. On a portal-dense map the forward pass cost then tracks the lights near
the visible scene, not the map total.

## Methodology

**Chosen: CPU per-frame light-set cull against the portal-drawn cell set, consumed via a
shader-side index list (indirection — no buffer rebuild).** This is the portal-engine member of
the light-culling family: the same role id Tech 4's per-area light interaction lists play —
visibility structure the engine already computes bounds the light set — done per frame on the CPU
because portal traversal is already a per-frame CPU pass (`rendering_pipeline.md` §2). The
per-fragment influence early-out stays as the fine-grained second stage; the new cull is the coarse
first stage it was missing.

Rejected alternatives, and why:

- **Tiled forward / Forward+ (Harada 2012).** Screen-space tile binning of lights via a compute
  pass over the depth buffer. Wins when many lights *are* on screen and overlap little. Here the
  problem is off-screen lights, which the portal set already identifies for free; Forward+ adds a
  compute binning pass, per-tile light lists (a storage buffer the forward fragment stage has no
  budget for — see Decisions), and depth-bounds plumbing. Wrong cost profile for a lean pipeline.
- **Clustered forward shading (Olsson/Billeter/Assarsson 2012; Persson 2013; Doom-2016-style).**
  3D view-space froxel grid with per-cluster light lists. Strictly more machinery than Forward+ for
  the same mismatch: it builds a *new* spatial subdivision when the engine already has one (cells)
  computed and culled per frame. `rendering_pipeline.md` §4 already defers this explicitly:
  "Clustered forward+ binning deferred until profiling shows the flat loop bottlenecks." This spec
  is the cheaper step that must come first; if a profiled map later shows many lights *inside* the
  drawn set, clustering is the follow-on, and this spec's per-frame set becomes its input.
- **Per-cell baked dynamic-light lists (the `ChunkLightList` pattern,
  `crates/level-compiler/src/chunk_light_list_bake.rs:85–86`).** The bake precedent exists for
  static specular lights, but baking is wrong for the dynamic tier: scripted lights change
  brightness/aim at runtime, and a baked cell→light table still needs the per-frame visible-cell
  join at runtime anyway — the join *is* the work this spec does, without a new PRL section.
- **GPU compute light binning.** Moves the cull to a compute pass reading the visible-cell bitmask.
  Adds a pipeline, a writable light-index buffer, and an indirect count for a job that is ~10⁴
  sphere-vs-AABB tests per frame on the CPU (see Decisions) — measured against the CPU portal walk
  it rides behind, noise. Not worth a pass.

Within the chosen methodology, two integration shapes were compared (research §A/§B):

- **(A) Per-frame compaction/rebuild** of the lights + influence buffers to visible lights only.
  **Rejected** — the buffer index space is a load-bearing, multi-writer contract:
  `upload_bridge_lights` asserts and rewrites the **full** `level_lights`-length buffer every
  animated frame (`renderer_lighting.rs:361–389`); the shadow pool patches per-light slot bytes
  keyed on level index (`renderer_light_slots.rs:208–248`); the promoted-static tail is appended
  **at offset `light_count`** and read by `shadowmask_union_subtraction`
  (`forward.wgsl:730–758`); shadowmask metadata sits after `total_light_count` in the influence
  buffer (`renderer_light_slots.rs:260–290`); and the scripted-light descriptor buffer is
  index-parallel to `level_lights` (`scripted_light_descriptors[i]`, `forward.wgsl:1113`).
  Compaction breaks or forces per-frame rewrites of all of it, plus billboard/mesh consumers.
- **(B) Keep the full buffers; add a per-frame visible-light index list + count the forward loop
  iterates.** **Committed.** Every byte contract above is untouched; the shader adds one uniform
  fetch of the index per visible light (amortized: 4 indices per 16-byte uniform row); the CPU
  uploads ≤ 4 KiB per frame only when the list changes. GPU-side, indirection costs one extra
  scalar load per *visible* light versus (A)'s zero — dwarfed by removing N−V full iterations —
  while (A) costs a full lights+influence re-upload and re-pack every frame on top of its contract
  breakage.

## Scope

### In scope

- A pure CPU predicate producing the visible-forward-light index list from the drawn cell AABBs
  (in `postretro-lighting`, per the §4 ownership boundary: wgpu-free light-reachability math).
- Plumbing the drawn-cell AABBs from the app's visibility result into the renderer per frame.
- A small fixed-capacity **uniform** index-list buffer on group 2 plus a `visible_light_count`
  field in the shared group-0 `Uniforms` (fills the existing `124..128` pad; stride stays 128).
- The forward world loop iterating via the index list, with an identity-sentinel fallback that
  reproduces today's full iteration bit-for-bit (overflow, fallback visibility paths, and a dev
  A/B toggle all take it).
- Doc touch-ups: `rendering_pipeline.md` §7.1 step 3 and §7.3 light-loop bullet.

### Out of scope

- **Any change to shadow-slot eligibility.** The wide reachable-set gate
  (`shadow_candidate_reaches_visible_cell`) and the orientation-invariance invariant that protects
  it stay untouched — see "Relationship" below.
- **Billboard and skinned-mesh light loops.** They iterate the total count
  (`billboard.wgsl:328` reads `uniforms.total_light_count`; `skinned_mesh.wgsl:443` reads
  `mesh_light_params.light_count`, whose field name says "light_count" but carries the *total*
  count, written from `total_light_count` at `renderer_render_frame.rs:384` — same loop, two
  symbols) and keep doing so: billboard lighting is per-vertex over few sprites, mesh fragments
  cover little screen.
  Because the buffers are untouched, leaving them on full iteration is safe by construction. A
  follow-on may extend the index list to them if profiling demands.
- **Clustered forward+ / per-tile binning.** Deferred as before (`rendering_pipeline.md` §13,
  §4); this spec is its prerequisite-sized step, not its replacement.
- **Culling promoted-static records or the static specular loop.** The promoted-static forward tail
  is already bounded by the shadow-pool promotion budget: `total_light_count − light_count =
  promoted_static_records.len() ≤ MAX_PROMOTED_SPOT (8) + MAX_PROMOTED_CUBE (2) = 10`
  (`renderer_light_slots.rs:851`, `:797/:813–824`; `renderer_types.rs:371–372`), independent of
  map-wide static count — so it does not share this spec's growth problem and needs no cull. The
  static specular loop is already chunk-list culled. The promoted-static population was assessed
  separately: `context/plans/drafts/perf-promoted-static-light-load/` (bounded per-fragment; the
  only growth is a CPU-side selection scan, outside this spec's mechanism).
- **Influence-volume tightening** (spot-cone-shaped influences, animated-aim-aware volumes). The
  cull consumes the same load-time `LightInfluence` spheres the per-fragment early-out uses.
- **New GPU timing infrastructure.** The forward pass is already bracketed
  (`TIMING_PAIR_FORWARD`, `pipeline_layout.rs:135`); the perf AC uses it as-is.

## Correctness argument (the crux)

Two points this cull must get right, both settled by prior engine history:

1. **Cull by influence-reaches-a-drawn-cell, NOT by the light's own cell being visible.** A
   dynamic light whose own cell fell out of the PVS still illuminates visible geometry its
   influence sphere reaches. The engine shipped exactly that bug once — an own-cell-PVS gate on
   shadow eligibility — and deliberately removed it: the comment at
   `renderer_light_slots.rs:65–77` records that the prior gate "dropped a light whose cell left
   the shrinking PVS on pitch-down even though it still lit and shadowed geometry in view", and
   `light_reaches_visible_cell`'s doc (`crates/lighting/src/lib.rs:106–138`) plus the regression
   test `light_with_off_pvs_leaf_but_reachable_receiver_is_eligible` (`lib.rs:751`) pin the
   replacement. This spec's predicate is the same shape: influence sphere vs cell AABBs, never an
   own-cell membership test.

2. **The forward cull may be TIGHTER than the shadow gate — and why that is safe.** The shadow
   gate tests against the **wide** portal-reachable set (empty `face_count == 0` cells included,
   `renderer_light_slots.rs:16–24`), because an off-view *caster* can cast into view, and because
   the merged shadow rework locked shadow-slot eligibility **orientation-invariant** (a slot
   vanishing on pitch is a visible shadow pop — see the sibling shadow spec). Forward
   *contribution* is different: it only matters for fragments actually **drawn**. Every drawn
   fragment belongs to a drawn cell — portal traversal emits the drawable `VisibleCells` set and
   the camera cull writes only those cells' BVH leaves (`rendering_pipeline.md` §5, §7.1 step 2);
   a cell's faces lie within its bounds (Cells id 38, `build_pipeline.md`). So if a light's
   influence sphere intersects **no** drawn cell's AABB, no drawn fragment can lie inside that
   sphere, and the per-fragment early-out (`forward.wgsl:1095–1103`) would have `continue`d on
   every drawn fragment anyway. The cull removes only zero-contribution iterations: the AABB-lifted
   form of the exact test the shader already runs, hence **bit-identical output**. And
   orientation-dependence of the drawn set is harmless here precisely because the culled quantity
   is per-drawn-fragment — there is no persistent per-light state (no slot, no hysteresis) to pop.

The **drawn set is the drawable `VisibleCells`** from `determine_visible_cells`
(`visibility.rs:471`): the portal path's `is_drawable && portal_visible` filter
(`visibility.rs:370–378`), or the per-cell AABB frustum fallback (`visible_cells_frustum_all`,
`visibility.rs:281–290`) on the solid-cell / exterior / no-portals paths — on every non-`DrawAll`
path it is exactly the cell set the frame draws from. `DrawAll` (empty world) maps to the
empty-slice sentinel → no cull, matching the `light_reaches_visible_cell` sentinel contract
(`lib.rs:144–147`). It is **not** `fog_reachable` — that wider set (empty cells included,
`visibility.rs:92–105`) exists to bound *influence sources* for fog and shadow eligibility and
must keep feeding those paths unchanged.

## Acceptance criteria

- [ ] `cargo build -p postretro` and `cargo test -p postretro-renderer -p postretro-lighting
  -p postretro-render-cpu` pass clean, no warnings. (`render-cpu` holds the `frame_uniforms.rs`
  byte tests updated by the offset-124 AC below.)
- [ ] **Correctness (the keep-test):** a `postretro-lighting` unit test constructs a light whose
  own cell is NOT in the drawn set but whose influence sphere reaches a drawn cell's AABB, and
  asserts its index **is kept** in the visible list (the forward twin of
  `light_with_off_pvs_leaf_but_reachable_receiver_is_eligible`, `lib.rs:751`). Companion tests: a
  light reaching no drawn cell is culled; empty drawn-AABB slice (DrawAll sentinel) keeps all;
  an uncullable-influence light (radius `f32::MAX`) is always kept. The overflow→identity-sentinel
  behavior lives renderer-side (Task 3 owns the capacity and the sentinel substitution, not the
  `postretro-lighting` predicate), so it is pinned by a separate `postretro-renderer` test: a
  visible count exceeding `MAX_VISIBLE_LIGHT_INDICES` yields the identity sentinel (full iteration),
  never truncation.
- [ ] **No pixel change:** with the dev A/B toggle, culled vs identity renders are
  indistinguishable on `campaign-test.prl` and `stress-warren-lit.prl`, including under
  `LightingIsolation::DynamicOnly` (mode 8) where any dropped light is maximally visible. Manual.
- [ ] **Shadow path untouched:** `POSTRETRO_SHADOW_DEBUG=1` slot counts (`spot=<used>/96 …`) are
  identical with the cull on and off — the visible-forward set must not feed
  `update_dynamic_light_slots` or the reachable-set inputs. The shadow-ranking
  orientation-invariance tests pass unchanged.
- [ ] **Loop-bound pinning:** `count_split_shader_consumers_use_expected_loop_bounds`
  (`crates/renderer/src/render/tests/shader_tests.rs:152–175`) is updated to pin the new
  index-list loop in `forward.wgsl` AND still pins billboard (`uniforms.total_light_count`) and
  mesh (`mesh_light_params.light_count`) full iteration byte-unchanged. The shadowmask-tail test
  (`shader_tests.rs:177`) passes unchanged — the promoted-tail helper must not be touched.
- [ ] **Perf:** on `stress-warren-lit.prl`, from a vantage where most of the warren is
  portal-occluded, the `forward` line of `POSTRETRO_GPU_TIMING=1` drops materially versus the
  toggle-off baseline, and the logged per-frame visible-light count is a small fraction of the
  ~157 total. Manual, via `RUST_LOG=info POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run
  content/dev/maps/stress-warren-lit.prl`. The visible-count log MUST be emitted at a level this
  command surfaces (`log::info!`, or under the `POSTRETRO_GPU_TIMING` gate itself) — not a bare
  `log::debug!` that this command would swallow.
- [ ] `build_uniform_data` seeds the new `visible_light_count` field with the identity sentinel,
  and the `frame_uniforms.rs` byte tests are updated for offset 124..128 (several currently assert
  `data[124..128]` — or a range containing it — is all-zero).
- [ ] A frame that skips the per-frame cull write (`render_world = false`: the frontend path at
  `main.rs:3356` passes `DrawAll` + empty slices) renders with full iteration — no stale index
  list is ever consumed (the sentinel seed guarantees it).
- [ ] **Docs:** `rendering_pipeline.md` §7.1 step 3 gains the per-frame visible-index list, and the
  §7.3 dynamic-loop bullet reads "Loop over the frame's portal-visible dynamic lights" (Task 4).

## Tasks

### Task 1 — CPU visible-light predicate (`postretro-lighting`)

In `crates/lighting/src/lib.rs`, add a pure function alongside `light_reaches_visible_cell`
(`:139–155`), e.g.:

```rust
// Proposed design
/// Indices (into the influence slice = `level_lights` order) of dynamic lights
/// whose influence sphere reaches any drawn-cell AABB. Empty `drawn_cell_aabbs`
/// = DrawAll sentinel → identity (every index). Reuses the closest-point
/// sphere-vs-AABB test; an influence with radius ≥ the uncullable sentinel is
/// always kept, matching the missing-influence degradation contract.
pub fn visible_forward_light_indices(
    influences: &[(glam::Vec3, f32)],       // (center, radius) per level light
    drawn_cell_aabbs: &[(glam::Vec3, glam::Vec3)],
    out: &mut Vec<u32>,
)
```

Same sphere-vs-AABB `closest = origin.clamp(min, max)` test, early-out on first hit, O(lights ×
drawn cells) with `any` short-circuit — at warren scale (~157 lights × ~10²-cells drawn) about
10³–10⁴ clamp/dot ops per frame, i.e. the same order as the shadow gate the frame already runs
(`renderer_light_slots.rs:87–95`). No spatial index needed; do not add one. Caller-owned `out`
keeps steady-state frames allocation-free. **Input contract (load-bearing):** the `influences`
slice is exactly the dynamic `[0, light_count)` range (`full.level_light_influences`, length =
`light_count = level_lights.len()`), so every emitted index is `< light_count` — the forward loop
indexes only dynamic records, never a promoted-static tail entry at `[light_count,
total_light_count)`. Add a `debug_assert!` that every produced index is `< influences.len()`. Unit
tests per the ACs (keep-test, cull-test, sentinel, uncullable, plus determinism of index order —
ascending).

### Task 2 — Plumb drawn-cell AABBs from the app

In `crates/postretro/src/main.rs`, next to the `reachable_cell_aabbs` build (`:2230–2239`), build
`drawn_cell_aabbs` from the **drawable** `visible_cells` instead of `fog_reachable`:
`VisibleCells::Culled(cells)` → map cell ids through `world.cells` bounds; `VisibleCells::DrawAll`
or no level → empty (sentinel). Pass it as a new parameter to `render_frame_indirect`
(`crates/renderer/src/render/renderer_render_frame.rs:15–28`); the frontend call site
(`main.rs:3356`) passes `&[]`. Keep the two comment blocks at `main.rs:2199–2229` intact — they
document why the *shadow* inputs use the wider set; add the mirror-image comment on the new build
(this one is deliberately the narrower drawable set, per the tighter-cull argument above).

### Task 3 — Renderer index-list resource + per-frame upload

**Why a uniform buffer, not storage (hard constraint):** the forward pipeline layout's
FRAGMENT-visible storage-buffer entry set already sits at the downlevel/WebGPU ceiling of 8 —
five in group 2 (`lighting_bind_group_layout_entries`,
`crates/renderer/src/render/pipeline_layout.rs:279–312`) plus three in group 3
(`sh_bind_group_layout_entries`, `crates/renderer/src/render/sh_volume.rs:710`) — and §10
refuses to raise `max_storage_buffers_per_shader_stage`. A uniform index list costs a
uniform-buffer slot instead (forward is far under that per-stage limit).

- Add a group-2 **binding 6** uniform entry to `lighting_bind_group_layout_entries`
  (`pipeline_layout.rs:279`, array grows 6 → 7), visibility **FRAGMENT only** (§10: widen
  minimally — billboard/fog never read it; the billboard VERTEX storage budget test
  `billboard_pipeline_vertex_storage_buffer_count` at `pipeline_layout.rs:373` is unaffected
  because the entry is neither storage nor VERTEX). Update **both** bind-group creation sites
  against this BGL: `build_lighting_bind_group` (`renderer_init_resources.rs:281–367`) and the
  level-install rebuild (`renderer_resources.rs:288`). A missed site fails bind-group creation
  loudly.
- Create the buffer once at init: fixed capacity `MAX_VISIBLE_LIGHT_INDICES` (1024 indices packed
  as 256 × `vec4<u32>` rows = 4 KiB; uniform address space requires the 16-byte stride), usage
  `UNIFORM | COPY_DST`.
- Add `pub const VISIBLE_LIGHT_COUNT_OFFSET: u64 = 124;` to
  `crates/render-cpu/src/frame_uniforms.rs` (the documented free pad,
  `build_uniform_data` `:268–269`) and seed the field with the identity sentinel (e.g.
  `0xFFFF_FFFF`) in `build_uniform_data`; update every group-0 tail-zero assertion (`data[124..128]`
  or any range covering it). These are confined to `frame_uniforms.rs` (the `sdf_shadow.rs` and
  shadow-pass `124..128` slots are a different uniform layout, not group 0) — but grep to confirm no
  other group-0 byte test asserts the tail is zero.
- Per frame, inside `render_frame_indirect`'s `render_world` block (adjacent to
  `update_dynamic_light_slots`, `renderer_render_frame.rs:83–109`): run Task 1's predicate over
  `full.level_light_influences` (the CPU mirror, `renderer_resources.rs:250`) and the new
  `drawn_cell_aabbs`; if the count exceeds capacity or the cull is toggled off, patch the identity
  sentinel to `VISIBLE_LIGHT_COUNT_OFFSET`; else patch the real visible count and upload the packed
  index rows. **The 4-byte count patch is unconditional every cull frame** — only the ≤4 KiB index-
  row upload is dedup-skippable when the list is unchanged (mirror the `last_lights_upload` compare
  pattern, `renderer_light_slots.rs:243–247`). This is not optional: `update_per_frame_uniforms`
  rewrites the full 128-byte group-0 uniform every frame (called earlier, `main.rs:2338`; writer at
  `renderer_frame.rs:148–164`), re-seeding offset 124 back to the sentinel — so skipping the count
  patch on an unchanged steady-state frame would silently revert to full iteration exactly when the
  cull should win most. The patch itself follows the existing `TOTAL_LIGHT_COUNT_OFFSET` precedent
  (`renderer_light_slots.rs:291–295`).
- Dev A/B toggle: an env-gated flag (e.g. `POSTRETRO_FORWARD_LIGHT_CULL=0`) forcing the identity
  sentinel, read once at init. Log the per-frame visible count at a level the perf-AC command
  (`RUST_LOG=info POSTRETRO_GPU_TIMING=1 …`) actually surfaces — a rate-limited `log::info!`, or
  fold it into the `POSTRETRO_GPU_TIMING` emit itself. A bare `log::debug!` is insufficient: the
  perf AC requires the count observable under that exact command.

### Task 4 — Shader loop indirection

In `crates/renderer/src/shaders/forward.wgsl`:

- Rename the group-0 `Uniforms` tail pad `_dyn_pad1` (`:60`) to `visible_light_count`. Mirror the
  rename in `billboard.wgsl` (`_dyn_pad1`, `:37`) and `wireframe.wgsl` — where the 124..128 slot is
  named `_dyn_pad3` (`:48`; wireframe's `_dyn_pad1` is a different offset) — as inert fields. The
  three-way 128-byte contract (`pipeline_layout.rs:142–165`) keeps its stride; no offset moves.
- Declare the group-2 binding-6 uniform index array (256 × `vec4<u32>`).
- Change the world loop head (`:1093–1094`): iterate `k` over the effective bound — the identity
  sentinel selects `light_count` and `i = k`; otherwise the bound is
  `min(visible_light_count, light_count)` and `i` is fetched from the index list
  (`row = k >> 2`, lane `k & 3`). The `min` guards only the *count* against a corrupt
  over-capacity value; the index *values* are already guaranteed `< light_count` by Task 1's
  dynamic-only input contract, so no promoted-static tail entry is ever shaded in the dynamic loop.
  Keep the `use_dynamic` isolation gate (`select(0u, …)`
  pattern, `:1093`) and keep the per-fragment influence early-out — it is still the fine cull for
  the surviving lights. Every body read (`light_influence[i]`, `lights[i]`,
  `scripted_light_descriptors[i]`, slot fields) keeps indexing the untouched full buffers.
- Do **not** touch `shadowmask_union_subtraction` (`:730–758`), the static-specular chunk loop,
  or any other shader. Billboard (`billboard.wgsl:328`) and mesh (`skinned_mesh.wgsl:443`) loops
  stay byte-identical.
- Update the pinning test `count_split_shader_consumers_use_expected_loop_bounds`
  (`shader_tests.rs:152–175`) to pin the new forward loop form (and still forbid
  `uniforms.total_light_count, use_dynamic` in forward); leave the shadowmask-tail test
  (`:177–209`) untouched.
- Doc update, same change: `context/lib/rendering_pipeline.md` §7.1 step 3 ("Light list upload" —
  add the per-frame visible-index list) and the §7.3 dynamic-loop bullet ("Loop over dynamic
  lights" → "Loop over the frame's portal-visible dynamic lights"); one clause each, no new
  section.

## Sequencing

Task 1 → Task 2 → Task 3 → Task 4. Tasks 1–2 are independently landable (pure fn + an unused
parameter); Task 3 lands the buffer/uniform plumbing behind the identity sentinel (no behavior
change until Task 4); Task 4 flips the loop and carries the A/B verification.

## Decisions

- **Indirection (B) over compaction (A).** The lights/influence buffers are a multi-writer,
  index-keyed contract (bridge full-buffer rewrite `renderer_lighting.rs:361–389`; slot patches
  `renderer_light_slots.rs:227–248`; promoted tail based at `light_count` `forward.wgsl:741–745`;
  shadowmask metadata after `total_light_count` `:758`; descriptor buffer index-parallel
  `:1113`). (A) breaks all of it for a marginal GPU win; (B) costs one uniform fetch per visible
  light. Committed: (B).
- **Uniform index list, fixed cap, identity sentinel.** Forced by the FRAGMENT storage-buffer
  ceiling (8/8 used; §10 forbids raising it). 1024 indices covers any plausible dynamic-light
  count several times over (`stress-warren-lit` ≈ 157); overflow degrades to today's exact
  behavior, never to truncation. Correctness never depends on the cap.
- **Cull against drawable `VisibleCells`, not `fog_reachable`.** Contribution is
  per-drawn-fragment, so the tight set is exactly right (see Correctness argument); the wide set
  keeps serving fog and shadow eligibility untouched.
- **CPU, O(lights × drawn cells), no spatial index.** Same cost class as the shadow gate that
  already runs per frame; a combined-AABB broad phase or cell-indexed structure would be
  optimizing noise. Revisit only if a profiled map shows this loop in a CPU trace.
- **Forward world loop only.** Billboard/mesh full iteration is cheap at their fragment/vertex
  counts and stays trivially correct because the buffers don't change. Extending the index list to
  them is a mechanical follow-on if ever measured to matter.
- **Same influence volumes as the early-out.** The cull is the AABB-lifted form of the existing
  per-fragment test, so it inherits its conservatism exactly — including for scripted lights
  (descriptors animate brightness/color/aim, never position; influences are load-time static,
  `renderer_light_slots.rs:250–252`). No new geometric assumptions.

## Risks

- **Fragment outside its cell AABB.** The argument requires a drawn fragment to lie within its
  cell's bounds. Cells partition space and a cell's faces lie in its bounds (id 38); residual
  boundary/interpolation error is at floating-point epsilon against a closed-AABB, closest-point
  test. If a seam artifact ever surfaces, an epsilon pad on the sphere radius in Task 1 is the
  one-line mitigation; do not preemptively add it.
- **BGL change misses a bind-group site.** Adding binding 6 requires both creation sites
  (`renderer_init_resources.rs:281–367`, `renderer_resources.rs:288`) — a miss fails loudly at
  bind-group creation, not silently.
- **Uniform-struct mirror drift.** The rename touches three WGSL mirrors of one 128-byte layout;
  stride and every offset are unchanged, and pipeline creation validates stride. The updated
  `frame_uniforms.rs` byte tests pin the new field.
- **Stale list / skipped patch.** Any path that writes uniforms without running the cull
  (frontend `render_world = false`, boot) renders full-iteration via the sentinel seed in
  `build_uniform_data` — fail-open to today's behavior, never fail-dark.
- **Perceived shadow regression via confusion with the shadow gate.** This spec changes zero
  shadow inputs; the shadow-debug AC pins it. Reviewers should check the wide-set comments at
  `main.rs:2219–2229` and `renderer_light_slots.rs:16–24` remain untouched.

## Relationship to `perf-shadow-caster-culling`

Sibling large-map dynamic-light perf specs attacking **different frame costs**: this spec cuts the
forward **shading loop** (per-drawn-fragment iteration over map-wide `light_count`); that spec cuts
the **shadow world re-raster + per-slot cone cull** on the up-to-96+36 dynamic slots. They are
independent — no shared code paths change, no build-order dependency; land in either order.

Two deliberate asymmetries, both load-bearing:

- **Tight vs wide.** The shadow spec is *barred* from tightening its eligibility gate
  (orientation-invariance invariant); this spec is *entitled* to a tighter, orientation-dependent
  cull because forward contribution exists only on drawn fragments (see Correctness argument).
  Neither spec's cull may be reused for the other's purpose.
- **Measurement.** The shadow spec's Task 1 (shadow timestamp brackets) is the shared measurement
  substrate for dynamic-light perf work on these maps; this spec needs nothing from it because the
  forward pass is already bracketed (`TIMING_PAIR_FORWARD`) — reference, don't duplicate.

Context, not a task: the zero-engineering escape hatch for any light that doesn't need to be
dynamic is to author it **static** — it bakes into the lightmap (and SH), leaving both the forward
loop and the shadow pool entirely (`rendering_pipeline.md` §4: dynamic vs static is an authoring
choice). Map authors should reach for that before either spec's machinery is assumed necessary.

## Related work

- **`context/plans/done/perf-dynamic-light-pvs-cull/`** — shipped the shadow-eligibility origin-cell
  gate, later replaced by the influence-vs-reachable test after the pitch-down bug. This spec's
  predicate is that lesson applied to the forward loop, against the tight set.
- **`context/plans/drafts/perf-anti-penumbra-pvs/`** — shrinks the drawable PVS at compile time.
  Complementary and multiplicative: a smaller drawn cell set directly shrinks this cull's input
  and output.
- **`crates/level-compiler/src/chunk_light_list_bake.rs`** — the baked per-cell light-list
  precedent (static specular). Left as-is; see Methodology for why baking doesn't fit the dynamic
  tier.
- **`context/plans/drafts/cell-light-binning/`** — design investigation that considered folding this
  cull into a shared cell→light index. Conclusion: fork **(b)** — keep this spec standalone. Its
  `visible_forward_light_indices` predicate *is* the shared bin's forward gather for one cell set, so
  shipping it pre-builds the exact primitive the binning would later reuse (a refactor, not a rewrite).
  The binning itself is a streaming-era build, not a dependency of this cull.
