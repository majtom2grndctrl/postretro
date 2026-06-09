# Rendering Pipeline

> **Read this when:** implementing or modifying the renderer, level loading, lighting, or any visual pass.
> **Key invariant:** renderer owns all wgpu calls. Other subsystems never touch GPU types. Level loaders produce handles; renderer consumes them.
> **wgpu (context7):** `/gfx-rs/wgpu` for API lookup; `/websites/sotrh_github_io_learn-wgpu` for design rationale.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) §4.1, §4.3

---

## 1. Frame Structure

Each frame runs five stages in fixed order.

| Stage | Work |
|-------|------|
| **Input** | Poll events, update input state |
| **Game logic** | Fixed-timestep update: entity movement, collision, game rules |
| **Audio** | Update listener position, trigger sounds from game events |
| **Render** | Determine visible set, draw visible geometry, dynamic lights, sprites, post-processing |
| **Present** | Swap buffers |

Game logic runs at a fixed timestep decoupled from render rate. Renderer interpolates between the last two game states for smooth visuals at variable framerates. Simulation is deterministic at any refresh rate.

**View vs. sim split.** View angles (yaw, pitch) update at render rate from raw input; player position updates inside the fixed-tick loop and is interpolated between tick states. Evanescent inputs (mouse delta) are consumed at render rate so they are never lost on zero-tick frames. See `input.md §3`.

---

## 2. Visibility and Traversal

Visibility is computed per frame from baked portal geometry — the id Tech 4 approach. Precomputed PVS lengthens compile cycles and fights dynamic geometry; per-frame portal traversal is cheap at modern leaf counts.

Portal traversal is the sole visibility path.

**Portal traversal.** CPU flood-fill. At each portal, clip the portal polygon against the current frustum. A non-empty clip result confirms visibility and narrows the frustum for the next hop. Produces a visible-cell bitmask consumed by the BVH traversal compute pass (§5).

**Fallback paths.** Solid-leaf camera, exterior-camera, and no-portals cases fall back to per-leaf AABB frustum culling against all leaves. See `build_pipeline.md` §Runtime visibility for the compile-side picture.

---

## 3. Level Loading

Loader parses PRL via the `postretro-level-format` crate. Uploads the global vertex/index buffer and BVH arrays to GPU storage buffers. Matches PNG textures by name (checkerboard placeholder for missing albedo, neutral normal for missing normal map). Renderer performs all GPU uploads and returns opaque handles — raw PRL types never cross into renderer code.

---

## 4. Lighting

Three components: **static direct** (baked), **dynamic direct** (runtime), and **indirect** (baked). All evaluated per fragment in the world shader — no deferred stages.

**Lighting architecture map.** The primary split is **bake participation**: baked-tier lights are fixed-position and bake into at least one layer; dynamic-tier lights bake into nothing and are evaluated entirely at runtime under a rationed budget. Within the baked tier, **shadow type** decides only how a light's *direct* shadow resolves. Whether a light is authored static (baked) or dynamic (runtime) is an **authoring choice, not an engine rule**. The engine invariant is narrower than a one-technique-per-light law: a physical light's contribution must not be **double-counted on a given receiver** — overlapping techniques (and overlapping static + dynamic light) must not over-brighten the same fragment. A single light may bake into more than one layer when those layers serve **different receivers** (e.g. static surfaces via the lightmap vs movers via a separate baked layer); what matters is that no receiver sums the same light twice. Every surface reaches indirect through exactly one path (SH, indirect-only). The current static-surface implementation meets the no-double-count invariant by routing each light's direct term through one technique and adding the techniques in the forward (they do not re-weight each other) — an implementation strategy, not a law. One light is shadowed by exactly one source.

```
AUTHOR (TrenchBroom .map) — split on bake participation
    baked tier   — fixed-position lights; shadow type ∈ { static_light_map, sdf }
    dynamic tier — unbaked, runtime-only, rationed (its own light entities)
        │
        ▼
COMPILER (prl-build) — route by tier, then (baked tier) by shadow type
        │
        ├─ INDIRECT  ─ every baked-tier light, both shadow types ─► octahedral probe atlas
        │                       (base atlas + sparse per-light delta; indirect-only)
        │
        ├─ DIRECT · static_light_map shadow type ─► baked into the lightmap (direct + shadow)
        │
        ├─ DIRECT · sdf shadow type ─────────► no baked direct; resolves at runtime
        │                       (perf-gated — reverts to lightmap if the gate fails)
        │
        └─ OCCLUDER FIELD (static geometry, no lights) ─► signed-distance field,
                                baked when sdf lights are present
        │
        ▼
RUNTIME — dynamic tier bakes nothing: evaluated live, shadowed by a rationed
          shadow-map pool (budget-capped; lowest-ranked lights render unshadowed).
          Only the dynamic tier can shadow moving entities.
        │
        ▼
FORWARD COMPOSITION (per fragment) — direct terms disjoint by technique; they add
        total = ambient floor
              + indirect          octahedral base + delta      every surface, one path
              + baked direct       static_light_map shadow type, shadow baked in
              + Σ sdf direct        × each light's runtime SDF visibility
              + Σ dynamic direct    × shadow map (rationed pool)
```

The seams that keep direct and indirect disjoint — tier routing, the position-axis namespace filter, indirect reaching every baked light regardless of shadow type — are pinned by compiler tests. Full producer/consumer inventory and the SDF runtime path (perf-gated, promotes to its own context doc once the gate holds): `context/plans/done/sdf-per-light-shadows/architecture.md`.

**Static direct.** prl-build UV-unwraps world geometry and ray-casts per-texel irradiance and a dominant incoming light direction from static_light_map-typed lights into a directional lightmap atlas. Static shadows are baked as **soft area-light penumbras** (bake-time stratified visibility, summed per light), not hard 1-texel steps. Runtime samples the **irradiance** and animated atlases through a **linear** sampler (the baked penumbra ramp is texel-quantized; hardware bilinear de-blocks it under magnification) while the **direction** atlas stays on a nearest sampler (linear interpolation doesn't commute with octahedral slerp). Bumped-Lambert correction preserves normal-map response to baked static lights.
   - **`Rgba16Float` linear-filterability is a hard runtime requirement** (the irradiance + animated atlas format). Linear filtering of 16-bit-float textures is core WebGPU and mandated on every targeted backend — Vulkan/Metal/DX12 all provide it — so there is no software fallback path: the renderer checks the adapter at init and fails fast with a named renderer message if the flag is absent (rather than a deferred bind-group-creation crash). The only added cost over the prior nearest-only sampling is **one extra sampler binding** in lightmap bind group 4 — no new per-fragment loop.
   - **Irradiance atlas storage.** The baked irradiance atlas is stored BC6H (`Bc6hRgbUfloat`) at rest by default — hardware-decoded and hardware-filterable, ~8× smaller on disk and in VRAM than `Rgba16Float`, no shader change (the fetch already reads `.rgb`). The PRL `irradiance_format` tag selects BC6H vs an uncompressed `Rgba16Float` debug path; the runtime branches texture creation on the tag, both bound `Float { filterable: true }` on the same BGL and linear sampler. `TEXTURE_COMPRESSION_BC` (already required for BC5 normals) covers BC6H; the renderer fail-fasts at init if BC6H format-features are absent. The **animated** lightmap atlas stays `Rgba16Float` (compute-written each frame, not baked). The on-hardware perf-floor numbers (NVIDIA GTX 16-series framerate floor; AMD Radeon Pro 5500M compatibility floor must-run, not framerate-gated) are a **manual** check — GPU perf is verified by running the engine, not in CI.

**Dynamic direct.** Dynamic lights run a per-fragment loop with an influence-volume early-out. Dynamic spot lights support shadow maps (depth texture array, comparison sampler); omnidirectional and sun lights cast no dynamic shadows. Light sources: FGD entities (`light`, `light_spot`, `light_sun`) and gameplay effects. Clustered forward+ binning deferred until profiling shows the flat loop bottlenecks.

**Indirect.** prl-build bakes diffuse irradiance into a DDGI-style octahedral probe atlas over the level's empty space. Runtime walks the 8 neighboring probes, does one hardware-bilinear atlas sample per probe direction, and weights each probe by trilinear factor × validity × optional backface rejection (forward path only) × optional Chebyshev probe visibility. Billboard and fog use the same atlas reads but skip backface rejection; fog also skips Chebyshev visibility. Surviving weights are renormalized, and when no probe survives the indirect term degrades to the ambient floor.

**Probe depth moments.** Each ShVolume probe record carries two baked f16 depth moments alongside the octahedral irradiance data — mean ray distance `E[d]` and mean squared distance `E[d²]` — accumulated over the same 256-ray sphere loop. Sky-miss rays contribute sentinel `4 × length(cell_size)` (4× the full 3D cell diagonal). The Chebyshev runtime interpolant consumes these to weight each probe by visibility and suppress through-wall indirect light leak. Probe record layout: see `build_pipeline.md` §PRL section IDs.

**Animated lights.** Animated lights carry per-light curve data (brightness scalar, RGB color) stored as packed f32 samples in a flat GPU storage buffer. Runtime evaluates Catmull-Rom splines over a `[0, 1)` cycle time with closed-loop wrap — uniform knot spacing, tension 0.5. A shared WGSL helper handles evaluation; it declares no buffers, so both the SH animation path and the animated-lightmap compose pass can bind their own `anim_samples` buffers at different bind-group slots without conflict. The animated-lightmap atlas is sampled by the forward pass only when the cell it belongs to passes the portal-traversed `VisibleCells` bitmask — any future pass that draws animated-lit geometry must share the same visibility gate or skip animated-lit chunks entirely.

**Animated SH delta volumes.** For complex lighting scenes, animated lights also contribute to the irradiance atlas. To avoid dynamic scene recomputation, each animated light's **indirect-only** (bounced) contribution at peak brightness is baked offline as octahedral delta tiles, stored sparsely against the base probe grid (f16, 1.0m probe spacing). Indirect is separate from direct: the animated light's *direct* term lives in `lm_anim` (the animated weight-map bake, occlusion-tested), so the delta carries bounce only — baking direct into both would double-count. The bake clips each light to its portal-reachable region and stores delta probes only where the light actually reaches: the base probe volume is partitioned into **affinity cells** of 4×4×4 base probes (`AFFINITY_FACTOR = 4`), and the section carries a CSR index (`affinity_offsets`/`affinity_lights`) mapping each affinity cell to the lights overlapping it, plus one dense 64-probe octahedral-tile sub-block per (cell, light) entry. At runtime, a pre-frame compute pass (§7.1 step 5) dispatches over the base atlas texels, reverses the near-square tile packing (`probe_index = tile_x + tile_y × atlas_tiles_per_row`) back to the x-fastest base probe, maps that probe to its affinity cell, evaluates animation curves for the current frame time, and adds the composed delta into the base atlas at full weight, writing the total atlas. Forward, billboard, and fog consumers read that composed total atlas through the shared octahedral sampler in group 3. (The former `delta_scale` dev knob was retired with the indirect-only amendment — the delta carries bounce only, so there is no double-count to bisect.) Forward shader includes 10 lighting isolation modes for independently inspecting each lighting contribution. Wire format / bake detail: `crates/level-format/src/delta_sh_volumes.rs`.

**Baked static direct for entities/billboards.** Skinned meshes and billboards additionally sample the baked static-direct SH atlas (`DirectShVolume`, PRL section 35), gated by `has_direct`, via the group-3/group-4 direct atlas binding. World geometry and fog do not use this atlas. See §9 for the full bind-group layout.

**Normal maps.** Perturb the per-fragment normal before direct and indirect evaluation. Tangents baked into the vertex format at compile time.

**Light authoring.** Mappers place light entities in TrenchBroom. Compiler translates FGD properties to a canonical internal format with validation (falloff distance, spotlight direction, intensity bounds). Canonical lights feed both the SH baker and the runtime direct path. See `build_pipeline.md` §Custom FGD.

---

## 5. Cells, BVH, and Draw Leaves

**Cell** = opaque visibility unit. One cell per empty BSP leaf.

World geometry is organized into a global BVH at compile time. Each BVH leaf covers one `(face, material_bucket)` pair. Leaves are sorted by material bucket so each bucket owns a contiguous slot range in the indirect buffer.

**Draw flow.** Portal traversal (§2) produces a visible-cell bitmask → BVH traversal compute walks the tree, tests each leaf AABB and its cell bit, writes or zeros the leaf's indirect buffer slot → opaque pass issues one `multi_draw_indexed_indirect` call per material bucket against its contiguous slot range.

**Global vs. per-region.** One BVH over all static geometry. Global wins on shader simplicity and tree quality. Per-region is the pivot path if a cell-heavy map regresses on frame time — tighter cache behavior at the cost of more bookkeeping and storage buffers. Pivot only when global is measured to fall short. No hardware ray tracing — not in baseline wgpu.

---

## 6. Vertex Format

Custom format for all world geometry. Non-position attributes are quantized where precision loss is imperceptible at the target aesthetic.

| Attribute | Purpose |
|-----------|---------|
| Position | Geometry placement |
| Base UV | Diffuse and normal-map texture sampling |
| Normal | Per-fragment shading normal |
| Tangent | Tangent-space basis for normal-map sampling |
| Lightmap UV | Static direct lighting atlas sampling |

UVs computed from face projection data at compile time; GPU sampler uses repeat addressing. Normals and tangents use octahedral encoding — half the storage of a full float vector at visually-indistinguishable precision. Both generated in prl-build. No per-vertex lighting channel — direct and indirect both accumulate per fragment (§4).

---

## 7. Rendering Stages

### 7.1 Visibility and Culling Prepasses

1. **Portal traversal** (CPU) — §2 flood-fill produces the visible-cell bitmask.
2. **BVH traversal** (compute) — walks the global BVH; tests each leaf AABB against the frustum and the leaf's cell bit; writes or zeros the leaf's indirect buffer slot.
3. **Light list upload** — uploads the active dynamic light array and per-light influence volumes to GPU storage buffers.
4. **Animated lightmap compose** (compute) — composites per-texel animated-light contributions into the atlas using pre-baked weight maps and runtime-evaluated Catmull-Rom curves. The atlas is zero-initialized by wgpu at creation and the compose pass writes every texel the forward pass samples, so no per-frame clear is needed. Culls dispatch tiles against the visible-cell bitmask so invisible rooms' animated lights don't waste GPU cycles. Runs after BVH cull and before the depth prepass. See §4 "Animated lights". **Atlas validity invariant:** the atlas holds valid data only for cells visible this frame. Any future pass that samples the animated lightmap atlas (e.g. reflection probes, alternate cameras) must use the same frame's `VisibleCells`, or skip animated-lit chunks — sampling the atlas for invisible cells yields stale prior-frame contents.
5. **SH compose pass** (compute) — reads the static base octahedral irradiance atlas and per-light delta tile data; evaluates animation curves for each light at the current frame time; accumulates total indirect contributions and writes to the composed atlas. Runs unconditionally (no culling). See §4 "Animated SH delta volumes".
6. **Shadow cone cull** (compute) — for each occupied shadow slot, dispatches BVH traversal gated by that slot's cone frustum only. The visible-cells buffer is all-ones (no camera-PVS AND): an occluder outside the camera PVS can still cast a shadow onto a visible receiver. Each slot writes into its own sub-region of a single shared indirect buffer. Runs after the camera BVH cull compute pass and before the shadow depth render passes. World geometry only; no entity/skinned-mesh shadow culling.

7. **Spot-shadow depth passes** — one render pass per occupied slot; slots with no ranked light are skipped. Each pass draws from its indirect sub-region via `multi_draw_indexed_indirect` per material bucket (same per-bucket contiguous layout as §5). **Fallback:** when no BVH is present (no-BVH maps), the pass falls back to an unconditional draw of all world geometry. Runs before the depth pre-pass so shadow maps are fully written before the forward pass samples them.

### 7.2 Depth Pre-Pass

Runs over the same indirect draw list as the forward pass with the same view-projection transform. Vertex-only: writes the shared depth buffer (eliminates forward-pass overdraw) and nothing else — no fragment stage, no color attachment. (It once wrote a full-res `Rg16Float` lightmap-UV gbuffer MRT for the animated dominant-direction SDF trace; that trace was removed in `sdf-per-light-shadows` Task 1 — the per-light SDF trace keys on light **position**, not lightmap UV — so the MRT was freed.)

Both the depth pre-pass and the forward vertex shader declare `@invariant` on `clip_position`. Without it, some GPUs reassociate the `mat4 × vec4` multiply differently across pipelines, producing Z-fighting dropout when the forward pass tests `Equal`.

### 7.3 World Geometry

One `multi_draw_indexed_indirect` call per material bucket. Depth loaded from the pre-pass buffer (`LoadOp::Load`); depth compare is `Equal`, depth writes disabled — each fragment is shaded exactly once. Per-fragment:

- Sample albedo and normal map; reconstruct world-space normal from TBN and normal-map sample.
- Sample lightmap atlas (irradiance + dominant direction); apply bumped-Lambert correction for normal-map response to static lights.
- Sample octahedral irradiance atlas (8-probe weighted bilinear reads) for indirect lighting.
- Loop over dynamic lights; evaluate direct contribution with influence-volume early-out.
- Output: `albedo × (static_direct + indirect_sh + Σ dynamic_direct)`.

Depth testing and back-face culling are permanent from this pass forward.

### 7.4 Billboard Sprite Pass

Camera-facing quads driven by the particle system. Alpha-blended additive pass; depth write disabled, depth test enabled. Quads are expanded in the vertex shader using the view-space right and up vectors — no geometry shader. Lit by: baked indirect (SH ambient) plus baked static direct (direct SH atlas, `sample_sh_direct`, gated by `has_direct`), multi-source static specular via the chunk light list, and dynamic direct (diffuse only). All four lighting terms are computed **per vertex** in `vs_main` (every input derives from the sprite center and the camera-facing normal `N = V`, so the term is constant across the quad) and interpolated, not re-evaluated per fragment. See §9 for the direct atlas binding.

**Vertex-stage storage-buffer budget.** Because the lighting loops run in `vs_main`, the billboard pipeline's VERTEX stage reads the group-2 light/chunk storage buffers (`lights`, `light_influence`, `spec_lights`, `chunk_offsets`, `chunk_indices` — five) and the group-6 `sprites` instance buffer (one): **six** VERTEX-visible storage buffers. wgpu charges `max_storage_buffers_per_shader_stage` against the BGL *entry* set per stage, not against what the shader reads — so the group-3 SH `anim_descriptors`/`anim_samples`/scripted-light storage entries must stay `FRAGMENT | COMPUTE` (NOT `VERTEX`); `vs_main` never reads them (animated pulses are imperceptible at one-sample-per-sprite). Marking them `VERTEX | FRAGMENT` during the hoist pushed the count to 9, exceeding the downlevel/WebGPU-default ceiling of 8 and crashing `create_pipeline_layout` on real GPUs. The headless `billboard_pipeline_vertex_storage_request_matches_bgl_definitions` test and a debug assert in `Renderer::new` pin the VERTEX-visible storage count at ≤ 8 from the same GPU-free BGL builders the layout is composed from. Batched by sprite-sheet collection — all particles sharing a collection issue one draw call per frame.

Billboard instances come from `BillboardEmitterComponent` particles packed by `ParticleRenderCollector` each frame. The collector walks `ParticleState` entities in the entity registry, buckets them by `SpriteVisual.sprite`, and hands the packed byte slices to `SmokePass::record_draws`. Bind group 6 carries a single shared sprite instance storage buffer sized to the frame's total live sprites; each collection draws from its own region via a `has_dynamic_offset` bind group (per-collection start offsets are padded to the 256-byte storage dynamic-offset alignment, the 32-byte per-instance stride unchanged within a region). The buffer grows on demand when a frame's padded total exceeds capacity. One collection still issues one draw call; there is no per-collection sprite cap.

### 7.5 Fog Volume Composite

Low-resolution raymarched pass over `fog_volume` brush regions. Resolution governed by `fog_pixel_scale` worldspawn property (default 4 — quarter resolution). Per sample: shape membership test (AABB as conservative bound), then optional half-space clip plane; accumulates ambient scatter, dynamic spot beam scatter (with shadow map occlusion for visible shafts and shadow wedges), and dynamic point-light scatter. The raymarch writes raw in-scattering to a low-res `Rgba16Float` **scatter** target. The march start is jittered per pixel and **animated per frame** (golden-ratio walk keyed on `FogParams.frame_index`) so the fixed-step quadrature error stratifies differently each frame instead of baking a static noise pattern — a single frame is grainy but the grain is unbiased frame-to-frame, which the temporal resolve below integrates away.

**Temporal accumulation + reprojection** (`fog_resolve.wgsl`, compute, low-res — isolated from the raymarch so the temporal logic is easy to reason about and toggle). A separate resolve pass reads the raw current scatter + the previous frame's accumulated **history** + the full-res depth buffer, and writes the blended accumulation into a ping-pong target. The composite then reads the **accumulated** buffer, not the raw scatter. Two `Rgba16Float` accumulation textures ping-pong by `frame_index`/`frame_counter` parity; both are recreated on resize (wgpu zero-clears them, so the first post-resize frame's history reads as cleared and the neighborhood clamp below collapses it toward current — no explicit re-init). Per low-res pixel:

- **Reprojection.** Reconstruct the current world position from scene/background depth via current `inv_view_proj`, project with `prev_view_proj` (appended to `FogParams`; the renderer caches last active frame's `view_projection()` and threads it into `FogPass::upload_params`), and sample history at the resulting prior-frame UV. Fog is not a surface, so reprojecting via scene depth is the standard screen-space approximation — exact for distant fog (depth→far: rotation reprojects, translation does not move infinity), approximate for near fog (accepted).
- **Disocclusion / ghosting rejection.** Reproject behind the camera (`w ≤ 0`) or off-screen (UV outside `[0,1]`) → output current (covers the first-frame / uninitialized-history case too).
- **Neighborhood clamp** (the anti-ghosting key, tuned for the pulsing spot). Before blending, clamp the sampled history to the min/max of the current pixel's 3×3 low-res neighborhood. Noise survives the clamp and averages away; a fast legitimate change (the pulse) drags the clamp window with it so stale history is clamped to the new value rather than lagging.
- **EMA blend.** `result = mix(current, clamped_history, ACCUM_ALPHA)` with `ACCUM_ALPHA` (≈0.9) a named constant in `fog_resolve.wgsl` — the single tuning knob: raise for smoother/slower, lower if the pulse smears. The clamp is what lets a high alpha stay responsive.

The accumulated low-res scatter is upsampled to full resolution with a **depth-aware (bilateral) filter** (`fog_composite.wgsl`): each full-res pixel blends the 2×2 bracketing low-res taps by bilinear proximity × depth similarity (linearized view distance, relative sigma), so the blocks dissolve into a smooth gradient without bleeding haze across geometry silhouettes. A plain nearest upscale (the prior behavior) replicated each low-res texel into a `pixel_scale × pixel_scale` block, which read as blocky pixelation when the camera sat inside a volume. Composited over the scene additively.

Temporal smoothing also creates headroom to march coarser (raise `fog.step_size` or `fog_pixel_scale`) for a net perf win, since the per-frame noise a coarser march introduces is what the resolve integrates out — those remain separate live-tuning knobs, not changed by the accumulation architecture.

`FogParams` layout: the temporal `prev_view_proj` (`mat4x4`, 64 bytes) is appended at the END (after the `frame_index`/`_pad2` tail), so the struct is now 176 bytes (`FOG_PARAMS_SIZE`). The composite declares a prefix-only `FogParams` ending at `far_clip` (WGSL allows eliding the tail of a uniform struct), so it is unaffected by the append; the raymarch likewise does not declare the appended field.

**Ambient scatter.** Fog samples irradiance from the same composed octahedral atlas (group 3) used by the forward and billboard passes. The fog pass keeps the stable world-up atlas read as the isotropic baseline, then blends toward a view-derived atlas read when authored `scatter_bias` is above zero. Each read is the same 8-probe loop used by the other samplers: one hardware-bilinear octahedral tap per probe direction, validity-weighted and renormalized. The compiler translates `scatter_bias` to a forward-scatter Henyey-Greenstein `g` value; `g = 0` preserves the flat haze path. `ambient_scatter` scales only the ambient indirect term, so dynamic spot and point-light scatter remain visible when ambient scatter is zero. When no SH volume is present (`has_sh_volume == 0`) the ambient contribution is zero. Per-volume scatter tint and saturation remain available via the `tint` and `saturation` KVPs on fog entities. Fog uses the shared no-depth SH helper with backface rejection disabled; Chebyshev depth visibility stays off for fog.

**Portal-driven volume culling.** Each frame, before dispatching the raymarch, the renderer reduces the per-sample AABB-test loop to only volumes reachable from the camera cell. Per-leaf `u32` bitmasks are baked at compile time into PRL section 31 (`FogCellMasks`); bit `i` set in leaf `L`'s mask means volume `i` overlaps leaf `L`'s bounds (conservative AABB-vs-AABB, no boundary pop). At runtime:

- `VisibleCells::Culled(leaves)` + masks present: OR every *fog-reachable* leaf's mask (portal-traversal reachability — empty leaves included, solid leaves excluded), then unconditionally OR the camera's current leaf's mask, then AND with `all_slots_mask = (1 << canonical_volume_count) - 1`.
- `VisibleCells::Culled(leaves)` + masks absent: legacy-PRL fallback — keep all canonical slots active (`active_mask = all_slots_mask`). Section 30 can ship without section 31, so absence does **not** imply zero volumes.
- `VisibleCells::Culled(leaves)` + empty `fog_reachable` (solid-leaf camera, exterior, or no-portals map): OR produces zero; unconditional camera-leaf OR still runs, then AND with `all_slots_mask` — net result is all canonical slots active. `DrawAll` is never returned for these cases; the empty-world arm is the only source of `DrawAll`, and fog volumes cannot exist in an empty world, so `DrawAll` is unreachable in practice.

The active set is repacked densely into the GPU fog buffer in ascending source-index order; volume indices in the GPU buffer are not stable across frames. `FogParams.active_count = active_mask.count_ones()` controls the WGSL raymarch loop bound. The shader respects `active_count`, so trailing slots past it are stale-but-safe. A separate `live_mask` suppresses density-zero slots inside that loop. When `active_count == 0` the pass is skipped via `FogPass::active()`. Volumes that recently left the reachable set are held active for a brief time-based hysteresis window (framerate-independent) to absorb single-frame portal-narrowing transients. See `context/plans/done/perf-portal-fog-culling/index.md`.

### 7.6 Wireframe Overlay (`dev-tools` only)

Renders world geometry as a line-list overlay using depth from the shared depth buffer (depth test on, depth write on). Runs after the fog composite and before debug lines. Active only when the wireframe toggle is enabled and geometry is loaded. Per-leaf cull status from the BVH traversal pass gates which leaves are drawn, so culled geometry stays invisible in wireframe mode.

### 7.7 Debug Lines (`dev-tools` only)

Immediate-mode line segments uploaded from a CPU buffer each frame. Depth test on, depth write off — lines occlude against opaque geometry but do not occlude each other. Runs after the wireframe overlay. See §12 for the full debug-line renderer contract.

---

## 8. Shader Module Composition

Shared WGSL helpers are appended to consumer shader source via string concatenation at pipeline creation time. No preprocessor, no `#include` directives — consistent with the existing codebase pattern. Binding-agnostic helpers declare no storage buffers; consumers declare the buffers at their preferred `(group, binding)` before the helper source is appended. This lets multiple pipelines share the same helper while binding its inputs at different locations.

---

## 9. Skinned Model Pipeline

Animated meshes (characters, monsters) draw through a separate forward pass from world geometry. World geometry is baked, BVH-organized, and GPU-culled (§5); a skinned model is a runtime entity with a per-frame bone pose. The split keeps each path simple: the world pipeline never carries skinning attributes, the skinned pipeline never touches the indirect-draw machinery.

### CPU model module (no wgpu)

The model module is CPU-only by contract — it never imports wgpu. It produces plain Pod types the renderer uploads. Four concerns:

- **glTF load.** Parses a glTF document into engine structs: one merged skinned mesh (all primitives in one interleaved stream), one `Submesh` per primitive carrying a material key and the index range it occupies in the merged buffer, the skeleton, animation clips, and author-supplied entity tags from the document's top-level glTF `extras`. Material keys are resolved **at load time** by content-hashing the base-color PNG with blake3 — the same recipe the level compiler uses to name `.prm` sidecars (see `build_pipeline.md` §Baked texture mips). Model materials consume only the diffuse slot from that shared cache address; specular and normal use neutral placeholders even when the sidecar is a richer world bundle. An unresolvable material (missing URI, missing file, or embedded image source) degrades to the all-zero sentinel key and renders a silent placeholder. Malformed or unsupported input returns an error; the loader never panics.
- **Skinned vertex.** The interleaved vertex mirrors `WorldVertex`'s encoding so both streams share one decode: position (`f32×3`), base UV (`u16×2`), octahedral normal (`u16×2`), packed tangent (`u16×2`, bitangent sign in the high bit), then the skinning attributes — joint indices (`u8×4`) and weights (`u8×4`, normalized in the vertex shader). A rigid (unskinned) primitive uses the degenerate single-bone case: joint 0 at full weight, which resolves to the instance's world transform.
- **Skeleton + clips.** Joints are stored **parent-before-child** (topological) so pose composition is a single forward sweep. Each joint carries its inverse-bind matrix and its rest-pose local TRS; the rest pose is the fallback for any animation channel a clip omits (a missing channel holds rest, never identity). An animation clip is per-joint translation/rotation/scale keyframe tracks, parallel to the joint array.
- **Pose sampling.** Sampling a clip at a time produces the **bone palette**: one skinning matrix per joint (composed world transform × inverse-bind), in joint order. Interpolation is LINEAR — component lerp for translation/scale, shortest-path slerp for rotation. Time wraps into the clip duration so clips loop. The world sweep relies on the parent-before-child order. Sampling writes into a caller-owned buffer and keeps a reusable scratch, so steady-state frames allocate nothing.

### GPU pass

The skinned-mesh render pass owns all wgpu for skinned models. It uploads a mesh's vertex/index buffers, builds the pipeline (deriving the wgpu vertex layout from the skinned-vertex field widths — the model module stays wgpu-free), and records one instanced `draw_indexed` per model over its CPU-culled visible instances. Skinning runs on the GPU in the vertex shader: each vertex blends its four joint matrices, fetched from the palette, and applies skin → model → view-projection.

**Shared bone-palette storage buffer.** All skinned instances' palettes live in one shared storage buffer. Each instance occupies a contiguous run; a per-instance **base index** selects its run, and the vertex shader addresses a joint as `base_index + joint`. One buffer for the whole frame, one small per-draw scalar — not a buffer or bind group per instance.

The mesh is not in the world depth pre-pass, so it depth-tests `Less` against the world depth *and* writes its own depth (self-occludes correctly), in a dedicated render pass that loads the existing depth attachment writably. Instance culling is the caller's job — a pure leaf-membership test (does the instance's BSP leaf fall in the frame's visible-cell set) mirroring the world path, decided CPU-side before the draw is recorded.

### Bind-group allocation (differs from §10)

The skinned pass owns its **own pipeline layout**, so its group mapping is independent of the world-geometry mapping in §10 — no runtime collision. Its groups:

| Group | Contents |
|-------|---------|
| 0 | Camera uniforms (shared with the forward pass) |
| 1 | Material (the shared material bind group; the full layout is reused so the bind group stays compatible) |
| 2 | **Reserved** — the dynamic-direct lighting bind group goes here when that work settles. Left unallocated; the pipeline layout passes an empty slot so that work *adds* a group rather than renumbering. |
| 3 | Per-instance data: the shared bone-palette storage buffer + a per-instance SSBO (model matrix + palette base index), addressed by `@builtin(instance_index)` — never `first_instance`, which is unreliable on DX12 (gfx-rs/wgpu#2471) |
| 4 | SH atlas superset (`mesh_bind_group`): octahedral indirect atlas + direct static-light atlas (`BIND_SH_DIRECT_ATLAS = 15`) + grid uniform + per-probe depth moments + `DynamicDirectParams` uniform (scale, isolation, `has_direct`; binding 16) |

This differs from §10's world mapping (where group 2 is dynamic lights / influence volumes / per-chunk light lists and groups 3–4 are the irradiance and lightmap atlases). The two layouts coexist because each pipeline declares its own; the shared groups (0 camera, 1 material) carry compatible bind groups.

### Committed vs. provisional

The **vertex attribute set** (the encoding above) and the **shared-palette + base-index scheme** are committed — consumers build against them. What is flat-lit or held open now is a deliberate, consumer-bound choice, not missing work:

- **Lighting.** The fragment samples the SH indirect baseline and the baked static-direct SH atlas (group 4, `mesh_bind_group` superset — depth-aware Chebyshev octahedral irradiance, `reject_backface = false`, Chebyshev probe-occlusion enabled, direct atlas at binding 15, `DynamicDirectParams` at binding 16). Group 2 is reserved for the future dynamic-direct *light loop* — runtime dynamic-tier lights (plan decision D10), distinct from the baked static direct now in group 4. The vertex stage already carries the skinned world-space normal for it.
- **Instancing.** Instances of the same model are batched into a single instanced `draw_indexed`; per-instance data (model matrix + palette base index) lives in a per-instance SSBO addressed by `@builtin(instance_index)`. The per-instance SSBO and argument layout are shaped to drop into `multi_draw_indexed_indirect` without a contract change; this task draws with instanced `draw_indexed` + CPU cull.
- **Depth variant.** A depth-only skinned pipeline (for shadows) would reuse the same palette + base-index scheme with position/joints/weights only. Not built here; the skinning vertex stage is kept separable for it.

---

## 10. Boundary Rule

All wgpu calls live in the renderer module. Map loader, game logic, audio, and input never import wgpu types. Data crosses the boundary as engine-defined types; the renderer translates to GPU operations. Per-subsystem contracts: vertex format §6, cells and BVH §5, lighting §4.

**Device limits.** Renderer requests `max_bind_groups = 8` — the WebGPU spec maximum and the ceiling for any future pass. Allocated bind-group slots:

| Group | Contents |
|-------|---------|
| 0 | Camera uniforms |
| 1 | Material (albedo texture, normal map, per-material uniforms) |
| 2 | Dynamic lights, influence volumes, per-chunk static light lists |
| 3 | Octahedral irradiance atlas (sampled total atlas, grid/tile uniform, animation descriptor + sample buffers, per-probe depth moments; see §4, §8) + direct static-light atlas (`BIND_SH_DIRECT_ATLAS = 15`; billboard samples it; forward and fog carry the binding in the shared group but do not read it) |
| 4 | Lightmap atlas (irradiance + dominant direction textures; nearest + linear samplers) |
| 5 | Spot shadow maps (depth texture array, comparison sampler, light-space matrices) |
| 6 | FX resources (sprite instance storage buffer; fog depth buffer, AABB buffer, scatter target) |

Groups 0, 2, 3, and 5 are shared across the forward, billboard, and fog pipelines — the same bind-group objects are reused, not re-uploaded. When a new pipeline stage consumes a shared BGL, each accessed binding's `visibility` must include that stage (e.g. `FRAGMENT → FRAGMENT | COMPUTE`) — wgpu validates this at pipeline creation, not compile time. One budget slot remains; a pass needing a ninth group must consolidate, not raise the limit.

**Widen visibility minimally.** The converse also bites: wgpu charges the per-stage binding-type limits (`max_storage_buffers_per_shader_stage`, `max_sampled_textures_per_shader_stage`) against the BGL *entry* set per stage, not against what a given shader reads. Adding `VERTEX` (or `COMPUTE`) to a shared entry that the new stage does **not** read still spends a slot in that stage's budget. The renderer does **not** raise `max_storage_buffers_per_shader_stage` above the downlevel/WebGPU default of 8 (broad hardware compat for a modder-friendly retro FPS), so an entry must carry a stage only when a shader in that stage genuinely reads it. The billboard pipeline sits at exactly six VERTEX-visible storage buffers against that ceiling of 8 (see §7.4); the `billboard_pipeline_vertex_storage_request_matches_bgl_definitions` test guards it headlessly.

The mapping above is the world-geometry path. The skinned model pass (§9) owns its own pipeline layout with a **distinct group mapping** — groups 0/1 carry the same camera/material bind groups, but groups 2 and 3 differ. No collision: each pipeline declares its own layout. See §9.

The renderer also requires `max_texture_dimension_2d ≥ 8192` (the lightmap atlas cap; wgpu's default already grants 8192). An adapter pre-check fail-fasts with a named `[Renderer]` error if the limit is below 8192, and a per-atlas runtime guard degrades a loaded lightmap larger than the granted limit to the neutral placeholder rather than panicking.

**Target hardware.** The renderer targets mid-2020 mid-range discrete GPUs — the envelope the lean wgpu pipeline is built toward. **Perf floor** (must hold an acceptable framerate): NVIDIA GTX 16-series (Turing, e.g. GTX 1660 Super). No RT cores at this tier, so SDF shadows sphere-trace in compute (§4) and hardware ray tracing stays a non-goal (§13). **Compatibility floor** (must run, not perf-tuned): AMD Radeon Pro 5500M-class (RDNA1, the 2020 16-inch MacBook Pro discrete GPU) on the Metal backend; a live-tunable quality panel (dev-tools) explores settings on this class. Perf-gated renderer decisions — SDF shadow budgets and the like (§4) — are measured against this envelope; measured per-pass numbers live with the `POSTRETRO_GPU_TIMING` diagnostics (§12), not here.

---

## 11. Camera

### Coordinate System

Right-handed, Y-up. Forward is −Z. Matches glam defaults and wgpu NDC.

### Projection Defaults

| Parameter | Default | Rationale |
|-----------|---------|-----------|
| Horizontal FOV | 100° | Modern boomer shooter default. Configurable 60°–130°. Vertical FOV derived from aspect ratio. |
| Near clip | 0.1 units | Close enough for weapon models without z-fighting |
| Far clip | 4096.0 units | Covers the full coordinate range for large maps |
| Aspect ratio | Derived from window | Updated on window resize |

### View Matrix

Camera position and orientation produce a view matrix each frame, feeding:

- Visibility (§2) — camera position seeds the portal flood-fill
- Frustum culling — view-projection matrix defines the clip volume
- All draw calls — view-projection uniform uploaded once per frame

---

## 12. Diagnostics

### GPU Pass Timing

Set `POSTRETRO_GPU_TIMING=1` to enable per-pass GPU timing. Requires adapter support for `TIMESTAMP_QUERY`; silently disabled if the feature is absent. Passes measured: `cull`, `animated_lm_compose`, `depth_prepass`, `sdf_shadow`, `forward`, `sh_compose`. Results are averaged over a 120-frame window and logged via `log::info!` at the window boundary. Use with `RUST_LOG=info` to see output. SH sampling is not separately timestamp-bracketed because it runs inside the forward fragment shader; measure it as `forward` timing deltas before/after the octahedral migration and with Probe Occlusion on/off.

### Debug-Line Renderer

`dev-tools` only. Immediate-mode API: per-frame CPU buffer of `(start, end, color_rgba)` line segments uploaded to a `LineList` vertex buffer and drawn after the fog composite pass and before egui. Depth test on (matching world render target sample count), depth write off — lines occlude against opaque geometry only. Buffer cleared at the top of the diagnostic emit call each frame, before new segments are pushed — not inside the render path — so it stays bounded even when `render_frame_indirect` early-returns (surface Timeout/Occluded/Outdated). Capped at a fixed segment limit (overflow: log + truncate). First consumer: SH volume diagnostic overlay.

---

## 13. Non-Goals

- **Deferred rendering** — forward lighting with influence-volume early-out keeps per-fragment iteration proportional to nearby lights. Indoor portal-isolated geometry bounds the set further. Deferred adds complexity without benefit.
- **PBR materials** — albedo + normal map is the full material vocabulary. Metallic/roughness is out of scope.
- **Hardware ray tracing** — not in baseline wgpu, and absent at the §10 perf floor (Turing GTX 16-series has no RT cores). Shadow maps cover dynamic shadowing; SH volume covers indirect; SDF shadows sphere-trace in compute.
- **Mesh shaders** — not baseline in wgpu. GPU-driven culling uses compute + `draw_indexed_indirect`.
- **Runtime level compilation** — maps compiled offline by prl-build. Engine is a consumer only.
- **Multiplayer / networking** — single-player engine. Out of project scope.
