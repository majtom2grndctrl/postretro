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

Visibility is computed per frame from baked portal geometry — the id Tech 4 approach. Precomputed visibility sets lengthen compile cycles and fight dynamic geometry; per-frame portal traversal is cheap at modern cell counts.

Portal traversal normally computes visibility. Solid-cell, exterior-camera, and no-portals cases use the defined AABB-culling fallback.

**Portal traversal.** CPU flood-fill. At each portal, clip the portal polygon against the current frustum. A non-empty clip result confirms visibility and narrows the frustum for the next hop. Produces a visible-cell bitmask consumed by the BVH traversal compute pass (§5).

**Fallback paths.** Solid-cell camera, exterior-camera, and no-portals cases fall back to per-cell AABB frustum culling against all cells. See `build_pipeline.md` §Runtime visibility for the compile-side picture.

---

## 3. Level Loading

Loader parses PRL via the `postretro-level-format` crate. Uploads the global vertex/index buffer and BVH arrays to GPU storage buffers. World materials resolve through PRL cache keys to baked `.prm` sidecars; missing or invalid sidecars use placeholders. UI and sprite textures load PNGs through their separate runtime paths. Renderer performs all GPU uploads and returns opaque handles — raw PRL types never cross into renderer code.

Kinematic brush movers load from PRL `KinematicGeometry` as a renderer-owned dynamic geometry path. Their vertices/indices are uploaded separately from the static world BVH/indirect path; game logic supplies per-frame mover instances and interpolated transforms. Movers bind the same world-material bundle as static geometry, so albedo, normal maps, texture filtering, and material shininess follow the world contract. Their runtime light loop stays diffuse-only for dynamic lights. View-dependent specular is limited to promoted static-light records, where it follows the promotion crossfade; non-promoted static lights remain baked-direct only for movers.

---

## 4. Lighting

Three components: **static direct** (baked), **dynamic direct** (runtime), and **indirect** (baked). All evaluated per fragment in the world shader — no deferred stages.

**Lighting architecture map.** The primary split is **bake participation**: baked-tier lights are fixed-position and bake into at least one layer; dynamic-tier lights bake into nothing and are evaluated entirely at runtime under a rationed budget. Within the baked tier, **shadow type** decides only how a light's *direct* shadow resolves. Whether a light is authored static (baked) or dynamic (runtime) is an **authoring choice, not an engine rule**. The engine invariant is narrower than a one-technique-per-light law: a physical light's contribution must not be **double-counted on a given receiver** — overlapping techniques (and overlapping static + dynamic light) must not over-brighten the same fragment. A single light may bake into more than one layer when those layers serve **different receivers** (e.g. static surfaces via the lightmap vs movers via a separate baked layer); what matters is that no receiver sums the same light twice. Every surface reaches indirect through exactly one path (SH, indirect-only). The current static-surface implementation meets the no-double-count invariant by routing each light's direct term through one technique and adding the techniques in the forward (they do not re-weight each other) — an implementation strategy, not a law. One light is shadowed by exactly one source **per receiver** — different receiver classes may resolve the same light's shadow through different techniques (promoted static lights: baked shadow on world surfaces, pool map on entities), but no receiver stacks two shadow techniques for one light. Two depth maps compared under one technique — a promoted slot's entity depth and the cached world depth, each a Nearest compare per tap — count as one source.

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
          Entity shadow receipt is pool-driven: dynamic-tier lights always;
          compiler-selected static lights via promotion (see "Promoted static
          lights" below).
        │
        ▼
FORWARD COMPOSITION (per fragment) — direct terms disjoint by technique; they add
        total = ambient floor
              + indirect          octahedral base + delta      every surface, one path
              + baked direct       static_light_map shadow type, shadow baked in
              + Σ sdf direct        × each light's runtime SDF visibility
              + Σ dynamic direct    × shadow map (rationed pool)
```

Compiler tests pin the seams that keep direct and indirect disjoint: tier routing, the position-axis namespace filter, and indirect reachability for every baked light regardless of shadow type. The SDF runtime path remains performance-gated.

**Ownership boundary.** Wgpu-free light packing, light spec packing, influence packing, and light-reachability CPU math may live in `postretro-lighting`. GPU pools/resources, uploads, bind groups, and wgpu-facing layout construction remain renderer-owned.

**Static direct.** prl-build UV-unwraps world geometry and ray-casts per-texel irradiance and a dominant incoming light direction from static_light_map-typed lights into a directional lightmap atlas. Static shadows are baked as **soft area-light penumbras** (bake-time stratified visibility, summed per light), not hard 1-texel steps. Runtime samples the **irradiance** and animated atlases through a **linear** sampler (the baked penumbra ramp is texel-quantized; hardware bilinear de-blocks it under magnification) while the **direction** atlas stays on a nearest sampler (linear interpolation doesn't commute with octahedral slerp). Bumped-Lambert correction preserves normal-map response to baked static lights.
   - **`Rgba16Float` linear-filterability is a hard runtime requirement** (the irradiance + animated atlas format). Linear filtering of 16-bit-float textures is core WebGPU and mandated on every targeted backend — Vulkan/Metal/DX12 all provide it — so there is no software fallback path: the renderer checks the adapter at init and fails fast with a named renderer message if the flag is absent (rather than a deferred bind-group-creation crash). The only added cost over the prior nearest-only sampling is **one extra sampler binding** in lightmap bind group 4 — no new per-fragment loop.
   - **Irradiance atlas storage.** The baked irradiance atlas is stored BC6H (`Bc6hRgbUfloat`) at rest by default — hardware-decoded and hardware-filterable, ~8× smaller on disk and in VRAM than `Rgba16Float`, no shader change (the fetch already reads `.rgb`). The PRL `irradiance_format` tag selects BC6H vs an uncompressed `Rgba16Float` debug path; the runtime branches texture creation on the tag, both bound `Float { filterable: true }` on the same BGL and linear sampler. `TEXTURE_COMPRESSION_BC` (already required for BC5 normals) covers BC6H; the renderer fail-fasts at init if BC6H format-features are absent. The **animated** lightmap atlas stays `Rgba16Float` (compute-written each frame, not baked). The on-hardware perf-floor numbers (NVIDIA GTX 16-series framerate floor; AMD Radeon Pro 5500M compatibility floor must-run, not framerate-gated) are a **manual** check — GPU perf is verified by running the engine, not in CI.

**Dynamic direct.** Dynamic lights run a diffuse-only per-fragment loop with an influence-volume early-out; no surface, including movers, receives dynamic-light specular. Dynamic spot lights cast shadow maps (depth texture 2D-array pool, comparison sampler). Dynamic point lights cast cube-array shadows (6-face depth pool). Uncached pool slots render cone-culled static world geometry plus entity occluders; cached slots copy cached world depth into the live pool before adding current entity depth. Sun/directional lights cast no dynamic shadows. Light sources: FGD entities (`light`, `light_spot`, `light_sun`) and gameplay effects. Clustered forward+ binning deferred until profiling shows the flat loop bottlenecks. A renderer-owned dynamic world-depth cache reuses static world depth when a dynamic light's source identity and full projection are unchanged: three 1024² spot layers and four atomic 6-face 512² cube units cover the campaign's scripted room-local peaks. Stable winners retain their cache layer across transient pool-slot moves; overflow deterministically falls back to the ordinary pool without evicting a warm winner. On a cold key the cache clears and draws static world depth. Every cached frame copies that depth into the assigned live pool layer before an entity pass loads it and adds current entity occluders; a warm key skips the world cull and raster pass. World, skinned, and kinematic receivers sample the resulting minimum-depth pool through the existing PCF path, equivalent to two nearest depth comparisons per tap. Cache textures are copy sources with no fragment bindings, keeping the complete forward inventory at 16 sampled textures with cube support (15 without). Each full-resolution cache is allocated only for its light type; absent types need no placeholder textures. Planning uses fixed-size storage, and level reload invalidates all keys.

**Indirect.** prl-build bakes diffuse irradiance into a DDGI-style octahedral probe atlas array over the level's empty space. SH atlases are `texture_2d_array`: whole octahedral probe tiles stay on one layer, layer-major in PRL, and shaders derive the array layer from the probe's x-fastest linear index. Runtime walks the 8 neighboring probes, does one hardware-bilinear atlas sample per probe direction, and weights each probe by trilinear factor × validity × optional backface rejection (forward path only) × optional Chebyshev probe visibility. Billboard and fog use the same atlas reads but skip backface rejection; fog also skips Chebyshev visibility. Surviving weights are renormalized, and when no probe survives the indirect term degrades to the ambient floor.

**Probe depth moments.** Each ShVolume probe record carries two baked f16 depth moments alongside the octahedral irradiance data — mean ray distance `E[d]` and mean squared distance `E[d²]` — accumulated over the same 256-ray sphere loop. Sky-miss rays contribute sentinel `4 × length(cell_size)` (4× the full 3D cell diagonal). The Chebyshev runtime interpolant consumes these to weight each probe by visibility and suppress through-wall indirect light leak. Probe record layout: see `build_pipeline.md` §PRL section IDs.

**Animated lights.** Animated lights carry per-light curve data (brightness scalar, RGB color) stored as packed f32 samples in a flat GPU storage buffer. Runtime evaluates Catmull-Rom splines over a `[0, 1)` cycle time with closed-loop wrap — uniform knot spacing, tension 0.5. Script sample slots preserve runtime-present authored map-light order (`_bake_only` omitted) so baked lights can drive reserved compose slots without entering the dynamic-direct buffer. Runtime-spawned dynamic lights draw from renderer-owned reserved capacity; a despawned runtime light's slot is reclaimed, so the reserve bounds peak-concurrent live lights, not cumulative spawns — a high-churn light source must despawn to free capacity. Forward light, influence, descriptor, and brightness arrays share one compact dynamic order and count, bounded by the peak-concurrent runtime light count for the level. A shared WGSL helper handles evaluation; it declares no buffers, so both the SH animation path and the animated-lightmap compose pass can bind their own `anim_samples` buffers at different bind-group slots without conflict. The animated-lightmap atlas is sampled by the forward pass only when the cell it belongs to passes the portal-traversed `VisibleCells` bitmask — any future pass that draws animated-lit geometry must share the same visibility gate or skip animated-lit chunks entirely.

Runtime dynamic lights may attach to a **moving** gameplay entity (e.g. a projectile) and track that body's render pose each frame — the raw tick `Transform` for a sprite body, the interpolated pose for a model body — rather than a fixed spawn origin. The renderer refreshes authored dynamic shadow candidates and their influences from the same current GPU upload before ranking and projection construction. Moving a ranked light or changing its range updates all shadow matrices and makes its world-depth cache cold; source identity still retains a warm layer across pool-slot moves when the projection is unchanged. Runtime-spawned lights outside the authored candidate list continue to render unshadowed. `LightAnimation` also has an engine-internal **radius** channel, CPU-evaluated in the light bridge and packed into the light's range plus influence volume; it is not yet authored by map/script light surfaces. Client-materialized runtime lights enroll through the client-side `absorb_dynamic_lights` path.

**Animated SH delta volumes.** For complex lighting scenes, animated lights also contribute to the irradiance atlas. To avoid dynamic scene recomputation, each animated light's **indirect-only** (bounced) unit-radiance transport is baked offline as octahedral delta tiles, stored sparsely against the base probe grid (f16, 1.0m probe spacing). Indirect is separate from direct: the animated light's *direct* term lives in `lm_anim` (the animated weight-map bake, occlusion-tested), so the delta carries bounce only — baking direct into both would double-count. The bake clips each light to its portal-reachable region and stores delta probes only where the light actually reaches: the base probe volume is partitioned into **affinity cells** of 4×4×4 base probes (`AFFINITY_FACTOR = 4`), and the section carries a CSR index (`affinity_offsets`/`affinity_lights`) mapping each affinity cell to the lights overlapping it. Each cell has a coarsening level: L0 stores every valid tile, L1 stores its valid corners, and L2 stores one synthesized brick-mean tile. Payload tiles are in kept-rank order. At runtime, compose reconstructs dropped-valid probes strictly from the same brick into the existing dense composed atlas: L1 trilinearly blends its kept corners; L2 reads the brick mean. For each CSR entry, coarsened compose loads the kept lattice once per brick into workgroup memory, so reads scale with the kept payload rather than refetching corners per output texel. It then evaluates animation curves for the current frame time, applies the descriptor's authored intensity and either its base color or sampled color curve exactly once, and adds the composed delta into the base atlas. Forward, billboard, and fog consumers read that dense composed total atlas through the shared octahedral sampler in group 3. (The former `delta_scale` dev knob was retired with the indirect-only amendment — the delta carries bounce only, so there is no double-count to bisect.)

**Per-term lighting mask (dev-tools).** `LightTermMask` is the single per-frame diagnostic instrument for world, mesh, mover, billboard, and fog consumers. Bits 0–6 independently select ambient floor, static/animated indirect, static/animated baked direct, dynamic direct, and specular; bit 7 remains reserved for the intentionally unwired emissive category. The renderer snapshots this mask before the diagnostics UI runs and every consumer reads that snapshot, so a toggle lands atomically on the next frame. Ambient, world lightmap, dynamic, and specular terms are gated in their consumer shaders. Indirect SH, direct-SH, and billboard-scatter compose consume their corresponding mask bits; direct-SH promotion subtraction occurs only while dynamic direct is enabled. Billboard scatter never subtracts promotion. The all-on default is unchanged. Wire format / bake detail: `crates/level-format/src/delta_sh_volumes.rs`.

**Baked direct for dynamic receivers.** Kinematic movers and skinned meshes sample the composed direct-SH atlas at binding 15, gated by `has_direct`. `DirectShVolume` (PRL section 35) supplies its static base; world geometry and fog bind but do not sample this atlas. The atlas is directional (L2 SH sampled with the fragment normal) but cannot encode cast self-shadowing — probes know nothing of the receiver's own geometry. Crisp entity shadowing under a selected static light comes from pool promotion.

**Animated direct SH for dynamic receivers.** A script-animated baked light adds its static-occlusion, unit-radiance direct transport through `AnimatedDirectShDeltaVolumes` (PRL section 45) during the pre-frame direct-atlas compose. Brightness and RGB curves scale that delta once through the shared animation descriptor, so kinematic movers and skinned meshes receive the same pulse/color term without receiver-specific wiring or a dynamic-light entry. Direction curves use the light's rest direction for this baked path; author a live cone sweep across moving receivers as `light_dynamic_spot` instead.

**Billboard direct scatter.** Valid `BillboardDirectScatterVolume` (PRL section 47) replaces a billboard's directional direct-SH sample with normal-free RGB scatter. The billboard vertex shader samples group 3 binding 17 only. `FrameUniforms.has_scatter` at byte 112 is a load-fixed mode: zero selects dummy/legacy, one selects an immutable static base, and two selects a composed animated texture; both real modes remain nonzero for availability checks. A static base is visible only under `BAKED_DIRECT_STATIC`. A composed texture is visible when either baked-direct bit is set because its compose pass independently removes the static base and animated deltas. Animated maps compose section 48's dense deltas using the section-45 descriptor mapping, shared animation time, and the direct-term mask before the billboard draw. Maps whose scatter comes only from animated `static_light_map` entries use a zero-RGB section-47 base as section 48's grid/validity anchor. The scatter path has neither direct SH nor static-light-map specular; static SDF lights retain their existing spec-light path because section 47 does not bake them. Its runtime direct loop stops at `light_count`, dropping promoted-static records instead of subtracting a direct-SH share. Missing, invalid, policy-oversized, or device-limit-incompatible scatter selects the exact legacy path: binding 17 is dummy, binding 15 supplies direct SH, static specular remains active, and the runtime loop uses `total_light_count` including the promoted tail. Section 48's dense delta, CSR-offset, CSR-light, and descriptor-index buffers must each fit both the active device's storage-binding and single-buffer limits; one failure disables scatter whole for that level.

| Direct receiver | Static baked | Animated baked | Dynamic tier |
|---|---|---|---|
| Static world | lightmap | animated lightmap | — |
| Kinematic mover | direct-SH base | id 45 composed delta | runtime direct loop |
| Skinned mesh | direct-SH base | id 45 composed delta | runtime direct loop |
| Billboard | id 47 direct scatter (legacy: direct-SH base) | id 48 composed scatter delta (legacy: id 45 direct-SH delta) | dynamic prefix only (legacy: total light count) |

**World specular shadowmask.** Compiler-selected non-SDF static world specular is multiplied by its baked `ShadowmaskAtlas` channel; absent, rejected, or dropped shadowmask data is fully lit, and this world-only signal remains independent of pool-shadow promotion and its crossfade.

**Promoted static lights (entity shadows).** Every occlusion fact is computed once, by the source that knows it best, and never re-derived: the bake owns static-onto-static and static-onto-probe occlusion; the runtime owns only the facts that involve a dynamic body — entity onto world, entity onto entity, and world onto entity at near-tier resolution — and a promoted slot exists to supply those three. Compiler-selected static lights (heuristic selection, no per-light KVP; dim, short-falloff, directional, SDF, and decorative wall/ceiling fixtures excluded) promote into the shadow pool at runtime when a shadow-relevant receiver intersects their influence and the light is portal-reachable. Relevance includes skinned meshes and active movers; a mover remains active while present, including when docked or camera-PVS-culled. Both receiver kinds share the existing ranker and fixed promotion budget: 8 spot slots and 2 cube slots. Promotion is budget-capped and crossfaded by a weight `w`: a mover or mesh receives the light as `(1 − w) × baked direct SH + w × runtime term × pool shadow map` — the SH atlas is the far LOD (occlusion-tested, directional light/dark space), the pool slot the near tier (true self-shadowing), and no receiver sums the light twice. Per-light baked direct SH delta tiles make the subtraction possible. Billboard scatter deliberately drops promoted records rather than applying this direct-SH subtraction/handoff. Mover specular is part of the promoted runtime term, so it fades with `w`; baked direct SH remains diffuse-only. World receivers keep their direct term in the lightmap; promotion reaches them only as the shadowmask union subtraction — the reconstructed direct term attenuated by baked visibility times (1 − entity visibility), weighted by `w` and removed from the accumulated static direct term — never as an appended runtime light record. The subtrahend is bounded by what the lightmap holds for that light and is exactly zero where no entity occludes, by construction rather than by threshold. Fog excludes promoted slots. A promoted slot holds entity-occluder depth only; the static world is never rendered into it. The light's static world depth is rendered once per assignment into a promoted-depth cache (static lights never move) and sampled by entity receivers, which combine it with the slot per tap, so movers and skinned meshes keep near-tier static shadows while world receivers never compare against the world.

**Pool-shadow receiver bias.** The pool-shadow receiver classes that sample a depth map containing their own geometry — world surfaces on the forward dynamic loop, kinematic movers, and skinned characters — need an offset, because a raw depth-compare produces self-shadow acne (texel-grid striping). The shared pool sampling path offsets the receiver position along its **geometric** normal (never the bump/shading normal, which would wobble shadow boundaries) by the shadow-texel world footprint before the compare, scaled per receiver class — world and movers aggressive, skinned conservative. Normal-offset is chosen over more caster-side depth bias because it scales structurally with texel footprint and does not degrade the already-tuned skinned contact shadows. Skinned self-shadow bias is **authorable per model** through a mesh-component scalar whose default preserves current appearance: quantized self-shadow reads as on-brand on flat pixel-art characters but objectionable on rounded ones, so no single engine constant serves both. The shadowmask union path offsets by zero: a promoted slot never contains the world, so a world fragment cannot self-compare there, and runtime static→static shadowing is exactly zero (the double-count invariant) without a threshold.

**Normal maps.** Perturb the per-fragment normal before direct and indirect evaluation. Tangents baked into the vertex format at compile time.

**Light authoring.** Mappers place light entities in TrenchBroom. Compiler translates FGD properties to a canonical internal format with validation (falloff distance, spotlight direction, intensity bounds). Canonical lights feed both the SH baker and the runtime direct path. See `build_pipeline.md` §Custom FGD.

---

## 5. Cells, BVH, and Draw Leaves

**Cell** = opaque visibility unit. Cells are serialized runtime records derived from the compiler BSP output. BSP itself is compile-only scaffolding and is not loaded by the renderer.

World geometry is organized into a global BVH at compile time. Each BVH leaf covers one `(face, material_bucket)` pair. Leaves are sorted by material bucket so each bucket owns a contiguous slot range in the indirect buffer.

**Draw flow.** Portal traversal (§2) produces a visible-cell bitmask → the camera cull (§7.1) writes or zeros each leaf's indirect buffer slot, via either the candidate path (gathers only the visible cells' leaves from the baked `CellDrawIndex` CSR) or the tree-walk fallback (walks the whole BVH, testing each leaf's AABB and cell bit) → opaque pass issues one `multi_draw_indexed_indirect` call per material bucket against its contiguous slot range. `CellDrawIndex` is required for non-empty BVH maps; missing or invalid required indexes fail load.

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
2. **Camera cull** (compute) — writes or zeros each leaf's global indirect slot via one of two paths; both share the global per-leaf slot layout, so the draw path (`bucket_ranges`, §7.3) is byte-for-byte identical regardless of which ran.
   - **Candidate cull** (`candidate_cull.wgsl`) — the fast path. Eligible iff a valid baked `CellDrawIndex` (build_pipeline.md, id 37) is loaded, this frame's visibility is `VisibleCells::Culled`, AND its provenance is `VisibilityPath::PrlPortal`. Non-empty BVH maps require the index at load time; absence or validation failure is a load error, not a runtime fallback. The CPU expands the visible cells' owned BVH-leaf spans from the CSR into a flat candidate-leaf list (deduping visible cell ids first, so a repeated cell never double-writes a slot), clears the camera indirect and cull-status ranges to zero, then dispatches one invocation per candidate leaf. Each invocation frustum-tests its leaf and writes that leaf's existing global slot (submit) or leaves it cleared (frustum reject). Non-candidate leaves stay cleared — so cull cost scales with *visible* geometry, not the whole tree. An out-of-range visible cell id falls back to the tree walk for that frame.
   - **Tree walk** (`bvh_cull.wgsl`) — the runtime fallback. Walks the whole global BVH in one invocation; tests each leaf AABB against the frustum and the leaf's cell bit; writes or zeros the leaf's slot. Selected for `DrawAll`, non-portal `Culled` fallbacks (solid-cell / exterior / no-portals), and the out-of-range visible-cell case above. Shadow cone cull (step 6) always uses the tree walk.
3. **Light list upload** — uploads the active dynamic light array and per-light influence volumes to GPU storage buffers.
4. **Animated lightmap compose** (compute) — composites per-texel animated-light contributions into the atlas using pre-baked weight maps and runtime-evaluated Catmull-Rom curves. The atlas is zero-initialized by wgpu at creation and the compose pass writes every texel the forward pass samples, so no per-frame clear is needed. Culls dispatch tiles against the visible-cell bitmask so invisible rooms' animated lights don't waste GPU cycles. Runs after BVH cull and before the depth prepass. See §4 "Animated lights". **Atlas validity invariant:** the atlas holds valid data only for cells visible this frame. Any future pass that samples the animated lightmap atlas (e.g. reflection probes, alternate cameras) must use the same frame's `VisibleCells`, or skip animated-lit chunks — sampling the atlas for invisible cells yields stale prior-frame contents.
5. **SH and billboard-scatter compose passes** (compute) — the indirect SH pass reads the static base octahedral irradiance atlas and per-light animated delta tile data; evaluates animation curves for each light at the current frame time; accumulates the mask-selected contributions and writes to the dense composed indirect atlas. The static-indirect bit selects the base and the animated-indirect bit gates every delta accumulation path. Dropped-valid probes reconstruct strictly within their 4×4×4 brick; for each CSR entry, all coarsened compose passes load each brick's kept lattice once into workgroup memory, while L0 probes keep direct reads. It dispatches only when its composed atlas would change — on level-load copy-through, while any animated indirect light is active, once when activity returns to zero, and whenever the current mask differs from the mask that produced the atlas — and writes the full affinity grid when it does (no per-cell culling). The direct SH pass is the static-entity-shadow sibling: whenever usable base direct SH is present, it writes a `Rgba16Float` composed direct atlas with static and animated contributions selected by their mask bits; promotion subtraction is enabled only with dynamic direct. It composes even without selected direct deltas, copying the base when baked direct is enabled and writing zero when it is disabled, so the baked-direct-static mask can isolate base direct SH. It dispatches on level-load copy-through, while any weight is nonzero, once when weights return to all-zero, and whenever the current mask differs from the mask that produced the atlas; maps without usable direct SH allocate no composed direct atlas and keep the no-direct binding behavior. The animated billboard-scatter sibling copies section 47 then accumulates section 48's dense deltas with the shared descriptors, time, and mask; static-only maps bind section 47 directly. All compose passes run before the depth prepass and before their consumers sample the results. See §4 "Animated SH delta volumes" and "Promoted static lights".
6. **Shadow cone cull** (compute) — for each occupied spot-shadow slot that needs a world-depth update, dispatches BVH traversal gated by that slot's cone frustum only. The visible-cells buffer is all-ones: an occluder outside the camera's portal-visible set can still cast a shadow onto a visible receiver. Each slot writes into its own sub-region of a single shared indirect buffer. A second instance of the same cull pipeline serves the cube pool: one sub-region per `(cube slot, face)` layer, gated by that face's 90° perspective frustum (dispatched inside step 8). Runs after the camera cull compute pass and before the shadow depth render passes. This cull serves static world geometry only. Skinned and rigid-instance occluders are outside the world BVH and CPU cone-culled per slot in steps 7–8. Warm promoted-static and dynamic-cache slots skip these sub-region dispatches because their static world depth is already cached.

7. **Spot-shadow depth passes** — one live-pool render pass per occupied dynamic slot; slots with no ranked light are skipped. An uncached slot clears to the far plane, then draws static world from its indirect sub-region via `multi_draw_indexed_indirect` per material bucket (same per-bucket contiguous layout as §5) and live entity occluders. A cold dynamic-cache key clears and fills its cache layer with static world. Each cached frame copies that layer to the live pool before the entity pass loads it and adds current occluders; a warm key skips the world cull and cache raster pass. **Fallback:** when no BVH is present (no-BVH maps), a required world pass draws all world geometry. Skinned dynamic casters include portal-visible meshes plus explicitly authored shadow-only meshes; the latter stay eligible outside camera PVS. Broader off-PVS mesh retention for promoted-static relevance does not enter dynamic slots. Rigid mover occluders are position-only depth draws, CPU cone-culled from every present mover's world AABB per slot; camera-PVS culling limits their beauty pass only. Movers are the first caller of this generic rigid-occluder path. Promoted static spot slots use a dedicated promoted-depth cache sized to `MAX_PROMOTED_SPOT` 1024² layers: on assignment/reassignment the world pass renders once into the cache layer, which entity receivers sample directly; every frame the live pool layer clears to the far plane and every retained statically relevant skinned caster and rigid entity occluder draws into it, so the slot holds entity depth only. Movers never enter the static-depth cache. Runs before the depth pre-pass so shadow maps are fully written before the forward pass samples them.

8. **Point cube-array depth passes** — one live-pool render pass per occupied cube-slot face (up to `CUBE_COUNT` concurrent point lights, 6 faces each). An uncached face clears to the far plane, draws static world geometry indirect from its own cull sub-region (a second shadow-cull instance sized `CUBE_COUNT × CUBE_FACES`, each region gated by that face's 90° frustum — the cube counterpart of step 6), then draws skinned and rigid mover occluders CPU-culled against the same frustum when the slot's light casts entity shadows. A cold dynamic-cache key clears and fills its six cache faces with static world; each face is copied to the corresponding live-pool face before its entity pass loads depth and adds current occluders. A warm key skips all six world culls and cache raster passes, retaining the copies and entity draws. Skinned caster filtering matches spot slots: dynamic faces accept portal-visible plus explicitly shadow-only meshes; promoted-static faces accept the broader retained static-relevance set. Mover caster collection is independent of camera PVS. Promoted static point slots use a promoted-depth cache sized to `MAX_PROMOTED_CUBE × 6` 512² face layers: cache fill replaces the dynamic world draw on assignment, and the cached faces are sampled by entity receivers; each occupied promoted face clears to the far plane every frame and draws entity occluders only. Rigid movers never enter the static cache. Slots with no ranked point light are skipped. Face layers are arranged for the forward sampler's y-flipped cube lookup (`CUBE_FACE_DIRS` in `cube_shadow.rs`, `sample_point_shadow` in `shadow_sample.wgsl`; pinned texel-exact by `cube_face_layers_round_trip_hardware_sampling`). Requires adapter support for `CUBE_ARRAY_TEXTURES`; absent that, point shadows are cleanly disabled without affecting the spot path.

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

Kinematic brush movers draw through a dedicated dynamic world-geometry pass after opaque world geometry. They use the same albedo/normal/specular material path and normal-map filtering as world geometry, but do not enter the static BVH or static indirect draw buffers. Their diffuse response includes the dynamic tier; their specular lobe evaluates only promoted static-light records and fades as promotion ends.

### 7.4 Billboard Sprite Pass

Camera-facing quads driven by the particle system. Alpha-blended additive pass; depth write disabled, depth test enabled. Quads are expanded in the vertex shader using the view-space right and up vectors — no geometry shader. Every billboard receives baked indirect and dynamic direct. With valid direct scatter it also samples normal-free baked direct RGB at group 3 binding 17, omits direct SH and isotropic vertex static-light-map specular, retains static SDF spec-light handling, and excludes promoted-static records from the runtime loop. Specular-shimmer billboards remain the exception: they evaluate every static chunk-light record per fragment. This binding is VERTEX-only. `has_scatter` in `FrameUniforms` byte 112 distinguishes static-base and composed-animated resources while preserving zero as dummy/legacy. Missing or invalid scatter uses the exact legacy vertex path: binding 15 `sample_sh_direct`, static specular via the chunk light list, and the promoted tail.

**Billboard lighting models.** A collection with no baked `NORMAL` slot uses the default **isotropic-scatter** model. This includes a collection with only a `SPECULAR` slot: a mask alone cannot produce a travelling glint. Its direct/indirect/dynamic terms and static specular remain center-evaluated in `vs_main`, interpolated across the quad, and use the camera-facing `N = V` convention. Thus the old whole-sprite glint remains uniform. A collection with a baked `NORMAL` slot is a **specular-shimmer** material; the parsed slot mask is the sole classifier, not an FGD or emitter flag. Its optional `SPECULAR` slot is a per-texel mask (a missing mask binds white), and its normal/specular frame arrays are bound at group 1 bindings 4/3 respectively. The normal slot does not affect diffuse, indirect SH, direct scatter, or dynamic direct: those terms remain normal-free and dynamic lights remain diffuse-only.

For shimmer, `vs_main` omits only the static chunk-light-list specular loop. `fs_main` rebuilds the camera-facing tangent frame from the view-space right/up vectors and `V = normalize(camera_position - sprite_center)`, rotates right/up by the sprite rotation, decodes the BC5 tangent-space normal for the flat `frame_idx` array layer, and transforms it through `(right, up, V)`. It then evaluates the static-light Blinn-Phong lobe per fragment and adds it to the interpolated vertex lighting; this is what lets relative camera/light motion sweep the highlight across the sprite. The sprite center intentionally remains the position for `V` and light directions. Isotropic static specular is therefore vertex-only and `N = V`; shimmer static specular is fragment-only and normal-mapped, so neither model double-counts it.

`SpriteDrawParams.params.y` is the sole specular-intensity input, and `params2.y` is the sole exponent input. The isotropic vertex loop and shimmer fragment loop each read those same fields; no hardcoded exponent or second draw-parameter source is allowed. Resolved exponents must be finite and strictly positive; draw-contract resolution rejects invalid overrides, and renderer registration rejects invalid direct callers before uniform packing.

**Vertex-stage storage-buffer budget.** Isotropic static specular and both models' dynamic direct run in `vs_main`, so the billboard pipeline's VERTEX stage reads the group-2 light/chunk storage buffers (`lights`, `light_influence`, `spec_lights`, `chunk_offsets`, `chunk_indices` — five) and the group-6 `sprites` instance buffer (one): **six** VERTEX-visible storage buffers. wgpu charges `max_storage_buffers_per_shader_stage` against the BGL *entry* set per stage, not against what the shader reads — so the group-3 SH `anim_descriptors`/`anim_samples`/scripted-light storage entries must stay `FRAGMENT | COMPUTE` (NOT `VERTEX`); `vs_main` never reads them (animated pulses are imperceptible at one-sample-per-sprite). Marking them `VERTEX | FRAGMENT` during the hoist pushed the count to 9, exceeding the downlevel/WebGPU-default ceiling of 8 and crashing `create_pipeline_layout` on real GPUs. The headless `billboard_pipeline_vertex_storage_request_matches_bgl_definitions` test and a debug assert in `Renderer::new` pin the VERTEX-visible storage count at ≤ 8 from the same GPU-free BGL builders the layout is composed from. Batched by sprite collection — all particles sharing a collection issue one draw call per frame.

**Fragment-stage storage-buffer budget.** Shimmer's per-texel static-specular loop reads the five group-2 light/chunk storage buffers. The shared group-3 animation/scripted-light entries add three more FRAGMENT-visible storage buffers, even though the billboard shader does not read them. This consumes the downlevel/WebGPU-default limit of **eight**; fragment visibility must not grow. The layout-derived debug assert in `Renderer::new` and headless `billboard_pipeline_fragment_storage_request_matches_bgl_definitions` test pin the per-group inventory at 5 + 3 and the total at 8. Consolidate bindings rather than raising the device limit.

Billboard instances come from `BillboardEmitterComponent` particles packed by `ParticleRenderCollector` each frame. The collector walks `ParticleState` entities in the entity registry, buckets them by `SpriteVisual.sprite`, and hands the packed byte slices to `SmokePass::record_draws`. Bind group 6 carries a single shared sprite instance storage buffer sized to the frame's total live sprites; each collection draws from its own region via a `has_dynamic_offset` bind group (per-collection start offsets are padded to the 256-byte storage dynamic-offset alignment, the 32-byte per-instance stride unchanged within a region). The buffer grows on demand when a frame's padded total exceeds capacity. One collection still issues one draw call; there is no per-collection sprite cap.

**Manual GPU check.** Compile `spawner-test.map`, then run it with `POSTRETRO_GPU_TIMING=1`. From the light-facing side, walk between the red static spotlight and the paired smoke emitters, then strafe left-to-right while keeping both in view. `smoke_puff` has baked NORMAL/SPECULAR slots: its highlight should sweep across the sprite face under the relative motion. The neighboring `smoke_puff_isotropic` control has no normal slot and should retain the uniform center-evaluated glint. The diffuse-only smoke behind the nearby wall checks baked direct-scatter visibility: it should remain dark while occluded and fall off outside the spot cone. It is not a shimmer-occlusion probe; shimmer evaluates every static chunk-light record and the billboard pipeline has no static-light shadowmask binding for that lobe. Trigger the closet plate to exercise the existing animated alarm transition, moving the camera while it runs; the smoke must not pop. After a 120-frame timing window, confirm `billboard_direct_scatter_compose` is present only while needed and has bounded cost relative to the frame's other compose passes. This is a manual GPU acceptance check, not a CI assertion.

Projectile sprite collections may provide a per-collection additive HDR **emissive** term (`sprite.rgb × emissive`, in `SpriteDrawParams.params.w`) so a billboard reads full-bright and blooms. It is deliberately **unconditional**, not gated by `LightTermMask` (emissive is self-only, outside the light-term set; see §per-term-mask, bit 7 stays unwired). The existing per-frame **flipbook** (`frame_idx` from packed age) also applies to projectile sprite bodies when an authored cadence enables its advancing age; no cadence remains byte-identical and static. The decoded-PNG fallback represents a collection as a renderer-owned `texture_2d_array`: one frame per layer. Map-emitter `_NN` collections prefer a content-addressed layered `.prm` sidecar: its D2-array upload carries every layer's full mip chain and uses a linear mip sampler with a per-sidecar LOD clamp. A sidecar cache miss, parse failure, or directory-frame-count mismatch falls through to the single-mip decoded-PNG path. `frame_idx` crosses the vertex/fragment boundary as a flat integer varying and selects that layer during shader sampling; no horizontal stitched strip or atlas-coordinate remap remains. A sidecar's optional baked specular/normal arrays use the same flat layer; normal presence selects the shimmer material path described above.

### 7.5 Fog Volume Composite

Low-resolution raymarched pass over `fog_volume` brush regions. Resolution governed by `fog_pixel_scale` worldspawn property (default 4 — quarter resolution). Per sample: shape membership test (AABB as conservative bound), then optional half-space clip plane; accumulates ambient scatter, dynamic spot beam scatter (with shadow map occlusion for visible shafts and shadow wedges), and dynamic point-light scatter. The raymarch reads the dynamic-direct bit from the shared group-0 per-frame mask snapshot: when clear, both dynamic scatter loop bounds are zero, so fog and world dynamic direct change together. Ambient scatter continues to follow the mask-selected composed indirect atlas. The raymarch writes in-scattering to a low-res `Rgba16Float` **scatter** target. The march start is jittered from the output pixel and is stationary across frames, which dissolves constant-step shells without shimmer.

**Composite.** `fog_composite.wgsl` samples the current scatter target with nearest filtering and additively blends it over the scene. Its encoded-space triangular-PDF dither suppresses 8-bit swapchain banding. There is no temporal history, reprojection, or resolve pass.

`FogParams` is 112 bytes (`FOG_PARAMS_SIZE`), matching `postretro_render_cpu::fog_volume::FogParams` and the raymarch WGSL mirror. It ends with the `frame_index`/`_pad2` tail; no prior-frame projection field is present.

**Ambient scatter.** Fog samples irradiance from the same composed octahedral atlas (group 3) used by the forward and billboard passes. The fog pass keeps the stable world-up atlas read as the isotropic baseline, then blends toward a view-derived atlas read when authored `scatter_bias` is above zero. Each read is the same 8-probe loop used by the other samplers: one hardware-bilinear octahedral tap per probe direction, validity-weighted and renormalized. The compiler translates `scatter_bias` to a forward-scatter Henyey-Greenstein `g` value; `g = 0` preserves the flat haze path. `ambient_scatter` scales only the ambient indirect term, so dynamic spot and point-light scatter remain visible when ambient scatter is zero. When no SH volume is present (`has_sh_volume == 0`) the ambient contribution is zero. Per-volume scatter tint and saturation remain available via the `tint` and `saturation` KVPs on fog entities. Fog uses the shared no-depth SH helper with backface rejection disabled; Chebyshev depth visibility stays off for fog.

**Portal-driven volume culling.** Each frame, before dispatching the raymarch, the renderer reduces the per-sample AABB-test loop to only volumes reachable from the camera cell. Per-cell `u32` bitmasks are baked at compile time into PRL section 31 (`FogCellMasks`); bit `i` set in cell `C`'s mask means volume `i` overlaps cell `C`'s bounds (conservative AABB-vs-AABB, no boundary pop). At runtime:

- `VisibleCells::Culled(cells)` + masks present: OR every *fog-reachable* cell's mask (portal-traversal reachability — empty cells included, solid cells excluded), then unconditionally OR the camera's current cell's mask, then AND with `all_slots_mask = (1 << canonical_volume_count) - 1`.
- `VisibleCells::Culled(cells)` + masks absent: stale/corrupt modern PRLs fail load before this point; valid modern maps with no fog volumes keep all canonical slots inactive.
- `VisibleCells::Culled(cells)` + empty `fog_reachable` (solid-cell camera, exterior, or no-portals map): portal isolation does not apply, so the renderer returns `all_slots_mask` directly. Camera-cell union is skipped on this path. `DrawAll` is never returned for these cases; the empty-world arm is the only source of `DrawAll`, and fog volumes cannot exist in an empty world, so `DrawAll` is unreachable in practice.

The active set is repacked densely into the GPU fog buffer in ascending source-index order; volume indices in the GPU buffer are not stable across frames. `FogParams.active_count = active_mask.count_ones()` controls the WGSL raymarch loop bound. The shader respects `active_count`, so trailing slots past it are stale-but-safe. A separate `live_mask` suppresses density-zero slots inside that loop. When `active_count == 0` the pass is skipped via `FogPass::active()`. Volumes that recently left the reachable set are held active for a brief time-based hysteresis window (framerate-independent) to absorb single-frame portal-narrowing transients.

### 7.6 Wireframe Overlay (`dev-tools` only)

Renders world geometry as a line-list overlay after the fog composite and before debug lines. The Diagnostics Spatial tab owns the full selector; `Alt+Shift+Backslash` remains a fast toggle between Off and the cull-status mode.

Modes:

- **Off** — no triangle wireframe pass.
- **Cull-status triangles (all leaves, x-ray)** — draws all loaded world triangles from every BVH leaf, renders always-on-top (`depth_compare = Always`, depth writes off), and tints by the GPU BVH traversal pass's per-leaf cull status: cyan = not submitted by the GPU cull pass (including leaves outside the CPU-visible set and descendants of skipped subtrees), red = leaf explicitly marked frustum-culled, green = rendered by the GPU indirect path. This is a culling diagnostic, not a visible-surface mesh view.
- **CPU-visible triangles (depth-tested)** — draws only BVH leaves whose `cell_id` is in the current frame's drawable `VisibleCells` set (`DrawAll` draws every leaf), uses a flat color with no cull-status tinting, and depth-tests against the shared scene depth (`LessEqual`, depth writes off). This shows geometry submitted by the CPU visibility path; it does not mean final GPU BVH/frustum survivors. Current cull status is GPU-resident, and this mode does not add GPU readback.

### 7.7 Debug Lines (`dev-tools` only)

Immediate-mode line segments uploaded from a CPU buffer each frame. Depth-tested lines test against opaque scene depth with depth writes off, so they occlude against world geometry but do not occlude each other. Explicit overlay/x-ray lines use the always-on-top debug-line path. Runs after the wireframe overlay. See §12 for the full debug-line renderer contract.

Spatial diagnostics use this pass for CPU-authored structural overlays:

- **BVH leaf AABBs** come from the renderer-owned CPU copy of compiled `BvhLeaf` records loaded from the PRL BVH section. They default to stable cell-id coloring, have a local deterministic budget (`max_boxes`, `stride`, optional visible-cells-only filter), and do not read back GPU cull status. Depth-tested is the default; x-ray is an explicit mode.
- **Cell bounds** come from decoded `LevelWorld.cells`. Solid cells are skipped. Drawable visible cells are colored from the current frame's drawable `VisibleCells::Culled` set; `VisibleCells::DrawAll` uses a distinct fallback color so it does not look like a successful portal walk.
- **Portal edges** come from decoded `LevelWorld.portals` polygon edges. They use the same depth-tested/x-ray selector as the other Spatial context overlays.

Spatial visible-cell coloring is derived from the drawable `VisibleCells` result that feeds world rendering, not from fog/light reachability masks. The wider `fog_reachable` / light-reachable sets include empty cells for volume and dynamic-light isolation and must not drive first-pass Spatial visibility colors.

### 7.8 HDR scene color, bloom, and screen-space resolve

The renderer owns a single-sample, surface-sized linear `Rgba16Float`
`scene_color` target. Every gameplay scene pass and gameplay UI pass writes
there; the fullscreen resolve is the sole swapchain writer for the gameplay
path. It samples `scene_color`, applies the near-neutral soft-knee tonemap,
then the flash, vignette, and shake effects, and writes the sRGB swapchain.
The target stores raw linear values; sampling it does not decode sRGB, and the
swapchain store performs the one display encode. This preserves in-range
content closely while compressing HDR overshoot rather than hard-clipping it.

The renderer-owned bloom compositor runs after fog and before capture,
wireframe/debug/viewmodel overlays, and gameplay UI. It extracts HDR luminance
above `BLOOM_THRESHOLD`, filters a five-level downsample/blur/upsample chain,
then additively composites the result back into `scene_color`. As a result,
emissive texels and other HDR-bright scene content make a soft screen-space
halo without changing any lighting buffer or causing overlays/UI to bloom. Set
`POSTRETRO_BLOOM=0` to disable the pass for the manual no-bloom emissive check.

**Mod bloom profile.** A mod may set a static bloom profile in its manifest.
The profile chooses a half, quarter, or eighth-resolution base chain and may
use texel-addressed pixelated upsample/composite reads. Omission uses the
half-resolution smooth profile. Downsample and blur stay linear in every mode.
The renderer owns profile state and resource changes; it persists across level
changes, resize, and full-renderer recreation. Player overrides and
per-material bloom tiers are separate features.

The boot-splash path is separate from the gameplay resolve: it writes directly
to the swapchain `view`, never touching `scene_color`, the UI pass,
`UiReadSnapshot`, or the screen-effects compose. Startup records black/logo
splash timing only after the renderer reports that command submission reached a
successful present path.

**Renderer boot/full phase split.** Renderer init is two phases so first pixels reach the window before the heavy pipelines build. The **boot phase** (`Renderer::new`) creates the instance, surface, adapter, device, queue, surface config, and the renderer-owned boot splash pass; device creation requests the full feature/limit set because wgpu features can't be added after the device exists. The **full phase** builds the steady-state renderer — world buffers, lighting/shadow resources, screen effects, mesh/UI/fog passes, debug lines. `is_boot_ready` gates splash painting; `is_full_ready` gates Frontend, Loading completion, Running, the UI pass, and scene rendering. Full init is idempotent/restartable across surface recreation, so a suspend→resume that recreated the surface reruns it without re-running deferred session init. See `boot_sequence.md` §1.

**Boot splash pass.** A renderer-owned pass (`render/splash_pass.rs`) that clears the swapchain (`LoadOp::Clear` black) and, when a logo is installed, draws it as one aspect-preserving textured quad sized by pure GPU-free math. It owns its pipeline, bind group layout, sampler, uploaded logo texture, and uniform — no shared world/UI resources. The app-facing renderer API stays small: install decoded splash pixels, render a black/logo frame, receive a `PresentHandle` after successful submission, clear the logo. Transient or skipped acquire paths return no handle, so startup timing does not advance. The app decodes the PNG on the boot thread (CPU-only, no wgpu) and hands pixels to the renderer, which owns all GPU work. Independent of the UI system: no `UiPass`, `UiImageRegistry`, `UiReadSnapshot`, glyphon, taffy, or UI JSON.

The resolve applies a near-neutral soft-knee tonemap before the existing flash (over-blend toward a tint color, weighted by `flash.a`), vignette (edge darken/tint, strength-scaled radial blend), and shake (pure UV offset applied before the sample). All three are packed CPU-side from the frame's `UiReadSnapshot` into a per-frame `EffectUniform` (binding 2 of group 0). The former byte-identity resolve contract is superseded: in-range content remains a visual-parity/manual-GPU gate. The resolve sampler is NEAREST / pixel-aligned. See `crates/renderer/src/render/screen_effects.rs` and `crates/renderer/src/shaders/screen_effects.wgsl`.

**Frame capture.** Headless capture runs the same soft-knee
tonemap into a capture-only `Rgba8UnormSrgb` target after the bloom composite,
then reads it back. PNG bytes therefore stay deterministic RGBA8 while capture
includes scene bloom and excludes transient screen effects. Renderer owns the
readback (per the boundary rule).

Capture is VM-free and single-instant: no script VM, no trigger firing, no game
tick — one authored frame. Byte-stable across runs on one adapter; adapter
rounding rules out cross-adapter goldens, so regressions compare same-adapter
frames, not committed reference PNGs.

Dynamic receivers (kinematic movers and skinned prop meshes) draw in capture at
their authored rest pose through the same renderer draw seams the windowed frame
uses — no capture-only draw path. Billboards are excluded: their sprites are
emitter-produced and need a particle sim tick to exist, which a single load-only
instant does not run. The harness reaches an animated light's fired appearance
by seeding an authored forced-active compose descriptor with a chosen radiance;
it never runs the script VM or an event loop, because a single frozen instant
cannot reproduce a finite curve's elapsed-since-fire state.

Capture `force_active` radiance channels must be finite and in `0..=64`. This
capture-only HDR budget leaves headroom in the half-float atlases; it does not
change scripting intensity limits. Capture fails if an authored prop mesh has
no model handle, its model cannot load, or a forced light resolves outside the
installed compose descriptor count, including when SH installation degrades to
dummy resources.

---

## 8. Shader Module Composition

Shared WGSL helpers are appended to consumer shader source via string concatenation at pipeline creation time. No preprocessor, no `#include` directives — consistent with the existing codebase pattern. Binding-agnostic helpers declare no storage buffers; consumers declare the buffers at their preferred `(group, binding)` before the helper source is appended. This lets multiple pipelines share the same helper while binding its inputs at different locations.

---

## 9. Skinned Model Pipeline

Animated meshes (characters, monsters) draw through a separate forward pass from world geometry. World geometry is baked, BVH-organized, and GPU-culled (§5); a skinned model is a runtime entity with a per-frame bone pose. The split keeps each path simple: the world pipeline never carries skinning attributes, the skinned pipeline never touches the indirect-draw machinery.

### CPU model crate (no wgpu)

`postretro-model` is CPU-only by contract — it never imports wgpu. It produces plain Pod types the renderer uploads. Five concerns:

- **glTF load.** Parses a glTF document into engine structs: one merged skinned mesh (all primitives in one interleaved stream), one `Submesh` per primitive carrying a material key and the index range it occupies in the merged buffer, the skeleton, animation clips, and author-supplied entity tags from the document's top-level glTF `extras`. Material keys are resolved **at load time** by content-hashing the base-color PNG with blake3 — the same recipe the level compiler uses to name `.prm` sidecars (see `build_pipeline.md` §Baked texture mips). Model materials consume only the diffuse slot from that shared cache address; specular and normal use neutral placeholders even when the sidecar is a richer world bundle. An unresolvable material (missing URI, missing file, or embedded image source) degrades to the all-zero sentinel key and renders a silent placeholder. Malformed or unsupported required model structure returns an error. Malformed optional authored data, including animation channels, may warn and degrade; skipped animation channels hold rest pose. The loader never panics.
- **Pose-mask authoring.** Per-joint glTF metadata assigns joints to convention-named pose masks; one joint may belong to several masks. Spine joints may also carry positive bend weights for body-specific aim falloff. Loader remaps memberships with the skeleton's topological order and derives the model's ordered pose-modifier stack. Unknown or malformed optional metadata warns and is ignored. A disconnected or branching spine mask drops its bend modifier rather than failing the model load.
- **Skinned vertex.** The interleaved vertex mirrors `WorldVertex`'s encoding so both streams share one decode: position (`f32×3`), base UV (`u16×2`), octahedral normal (`u16×2`), packed tangent (`u16×2`, bitangent sign in the high bit), then the skinning attributes — joint indices (`u8×4`) and weights (`u8×4`, normalized in the vertex shader). A rigid (unskinned) primitive uses the degenerate single-bone case: joint 0 at full weight, which resolves to the instance's world transform.
- **Skeleton + clips.** Joints are stored **parent-before-child** (topological) so pose composition is a single forward sweep. Each joint carries its inverse-bind matrix and its rest-pose local TRS; the rest pose is the fallback for any animation channel a clip omits (a missing channel holds rest, never identity). An animation clip is per-joint translation/rotation/scale keyframe tracks, parallel to the joint array. All of a document's clips load and are addressable by authored name; each track records its authored interpolation mode (LINEAR or STEP — CUBICSPLINE degrades to LINEAR at load with a warning).
- **Pose sampling.** Sampling a clip at a time produces the **bone palette**: one skinning matrix per joint (composed world transform × inverse-bind), in joint order. Interpolation follows the track's authored mode (LINEAR — component lerp for translation/scale, shortest-path slerp for rotation; STEP holds the lower key). Looping is per-state policy: a looping clip wraps time into its duration, a one-shot clip clamps and holds its final keyframe. Crossfades blend two local-pose sources — a clip or a captured static TRS snapshot — per joint. Ordered pose modifiers then mutate only their masked local joints before the single hierarchy and palette composition; overlapping modifiers compose in list order. An empty stack or absent pose inputs uses the unmodified sampling path. The world sweep relies on the parent-before-child order. Sampling writes into a caller-owned buffer and keeps a reusable scratch, so steady-state frames allocate nothing. All skeletal-animation timing derives from one accumulated game-layer clock (`frame_dt × time_scale`) — the slow-motion/pause seam; it respects the dev-tools time freeze.

**Pose consumers — shared sampling, separate authority.** Fixed-tick game logic authors transient pose inputs for the same frame's draw. The renderer samples visible instances; a model with an active pose stack forces per-frame palette resampling so changing inputs never leave a stale time-sliced presentation pose. Precise hit-zone models also force visible palette resampling. Authoritative hit zones share the animation clock and clip or crossfade sample parameters, but intentionally sample the unmodified world pose. Presentation modifiers affect only the rendered palette and never change authoritative hit-zone geometry. Hit zones own their **own** CPU model copy (skeleton + clips), separate from the renderer's: per-model-type data is O(model types), not O(instances), so duplication is negligible and the boundary stays clean. The crossfade snapshot store is renderer-owned, but smooth-interrupt capture instructions are render-free. Game-side hit zones reconstruct exact snapshot captures when possible. When the precise capsule pose is non-authoritative — an unreconstructable snapshot (a chained smooth interrupt needing renderer-only stored data), or a degenerate static-identity zone model — the entity degrades to a coarse fallback: its authored AABB when present, else the model's derived reach bound (a conservative superset of every posed capsule). So a drawn zone-bearing entity is never unshootable, and no wrong fallback-clip capsule is posed. A genuine posed miss on a real skeleton stays an authoritative miss. The game-side consumer is shipped as `scripting_systems::hit_zones`, which samples poses for hitscan evaluation on the same game clock.

### GPU pass

The skinned-mesh render pass owns all wgpu for skinned models. It uploads a mesh's vertex/index buffers, builds the pipeline (deriving the wgpu vertex layout from the skinned-vertex field widths — the `postretro-model` crate stays wgpu-free), and records one instanced `draw_indexed` per model over its CPU-culled visible instances. Skinning runs on the GPU in the vertex shader: each vertex blends its four joint matrices, fetched from the palette, and applies skin → model → view-projection.

**Shared bone-palette storage buffer.** All skinned instances' palettes live in one shared storage buffer. Each instance occupies a contiguous run; a per-instance **base index** selects its run, and the vertex shader addresses a joint as `base_index + joint`. One buffer for the whole frame, one small per-draw scalar — not a buffer or bind group per instance.

The mesh is not in the world depth pre-pass, so it depth-tests `Less` against the world depth *and* writes its own depth (self-occludes correctly), in a dedicated render pass that loads the existing depth attachment writably. Instance culling is the caller's job — a pure cell-membership test (does the instance's located cell fall in the frame's visible-cell set) mirroring the world path, decided CPU-side before the draw is recorded.

**First-person viewmodels.** The CPU planner partitions `MeshInstanceInput` values into world and viewmodel plans while packing both into the same palette and instance storage buffers. Shadow-depth passes receive only the world plan. The viewmodel plan draws in a final mesh pass with a dedicated tight view-projection (roughly 70° FOV, 0.01–2 m clip range) and clears the shared depth attachment first, so nearby world geometry cannot clip the local weapon. Game-side render assembly composes view-feel bob and tilt in camera space, converts the result to a world-space instance transform, and supplies the matching render-camera view. Shared mesh shading therefore receives real world positions; renderer owns only projection choice and does not read view-feel state.

### Bind-group allocation (differs from §10)

The skinned pass owns its **own pipeline layout**, so its group mapping is independent of the world-geometry mapping in §10 — no runtime collision. Its groups:

| Group | Contents |
|-------|---------|
| 0 | Camera uniforms (shared with the forward pass) |
| 1 | Material (the shared material bind group; the full layout is reused so the bind group stays compatible) |
| 2 | **Runtime direct lighting + shadow receipt** — mesh-specific layout over the same underlying GPU buffers forward's group-2/group-5 shadow resources use, omitting forward's SDF-factor and scene-depth entries the mesh must not sample. b0 runtime light records (dynamic tier first, promoted static records appended); b1 influence volumes; b2 scripted-animation descriptors; b3 anim samples; b4 params uniform (total light count / time / `LightTermMask` / ambient floor / dynamic-tier light count); b5 spot shadow depth 2D-array; b6 comparison sampler; b7 light-space matrices uniform; b8 conditional cube-array depth; b9 promoted-depth cache spot layers; b10 conditional promoted-depth cache cube faces. |
| 3 | Per-instance data: the shared bone-palette storage buffer + a per-instance SSBO (model matrix + palette base index), addressed by `@builtin(instance_index)` — never `first_instance`, which is unreliable on DX12 (gfx-rs/wgpu#2471) |
| 4 | SH atlas superset (`mesh_bind_group`): octahedral indirect atlas + direct static-light atlas (`BIND_SH_DIRECT_ATLAS = 15`) + grid uniform + per-probe depth moments + `DynamicDirectParams` uniform (scale, padding, `has_direct`; binding 16) |

This differs from §10's world mapping (where group 2 is dynamic lights / influence volumes / per-chunk light lists and groups 3–4 are the irradiance and lightmap atlases). The two layouts coexist because each pipeline declares its own; the shared groups (0 camera, 1 material) carry compatible bind groups.

### Committed vs. provisional

The **vertex attribute set** (the encoding above) and the **shared-palette + base-index scheme** are committed — consumers build against them. What is flat-lit or held open now is a deliberate, consumer-bound choice, not missing work:

- **Lighting.** The fragment samples the SH indirect baseline and the baked static-direct SH atlas (group 4, `mesh_bind_group` superset — depth-aware Chebyshev octahedral irradiance, `reject_backface = false`, Chebyshev probe-occlusion enabled, direct atlas at binding 15, `DynamicDirectParams` at binding 16). Group 2 is allocated and live: `accumulate_dynamic_direct` evaluates runtime dynamic-tier light records plus promoted static-light records, with spot + point shadow-map attenuation and diffuse-only Lambert against the interpolated normal. World receivers do not enter this loop: their static direct term remains in the lightmap, and promoted lights reach them through the shadowmask union subtraction instead. Meshes and movers use direct-SH subtraction for promotion handoff; billboard direct scatter drops promoted records instead (§4 "Promoted static lights").
- **Instancing.** Instances of the same model are batched into a single instanced `draw_indexed`; per-instance data (model matrix + palette base index) lives in a per-instance SSBO addressed by `@builtin(instance_index)`. The per-instance SSBO and argument layout are shaped to drop into `multi_draw_indexed_indirect` without a contract change; this task draws with instanced `draw_indexed` + CPU cull.
- **Depth variant.** A depth-only skinned pipeline exists (`skinned_depth.wgsl`): reuses the `skin_matrix` kernel (position/joints/weights only) and projects via a per-render light-space matrix supplied in group 0. One pipeline serves both spot slots and cube faces — the target view and matrix are supplied per render pass. Used for entity occluders in both the spot-shadow and point cube-array passes (§7.1 steps 7–8).

---

## 10. Boundary Rule

All wgpu calls live in the renderer module. Map loader, game logic, audio, and input never import wgpu types. Data crosses the boundary as engine-defined types; the renderer translates to GPU operations. Per-subsystem contracts: vertex format §6, cells and BVH §5, lighting §4.

**Device limits.** Renderer requests `max_bind_groups = 8` — the WebGPU spec maximum and the ceiling for any future pass. Allocated bind-group slots:

| Group | Contents |
|-------|---------|
| 0 | Camera uniforms |
| 1 | Material (albedo texture, normal map, per-material uniforms) |
| 2 | Dynamic lights, influence volumes, per-chunk static light lists |
| 3 | Octahedral irradiance atlas array (sampled total `texture_2d_array`, grid/tile/layer uniform, animation descriptor + sample buffers, per-probe depth moments; see §4, §8) + direct static-light atlas array (`BIND_SH_DIRECT_ATLAS = 15`; billboards use it only on their legacy fallback) + billboard direct-scatter 3D texture (binding 17, VERTEX-only) |
| 4 | Lightmap atlas (irradiance + dominant direction textures; nearest + linear samplers) |
| 5 | Shadow resources: binding 0 = `spot_shadow_depth` (depth 2D-array, spot pool); binding 1 = comparison sampler (shared by spot and cube paths); binding 2 = `light_space_matrices` uniform (spot slots); binding 3 = SDF shadow factor (half-res `Rgba8Unorm`); binding 4 = full-res scene depth; binding 5 = `point_shadow_cube` (`texture_depth_cube_array`, point-light cube shadows) |

Groups 0, 2, 3, and 5 are shared across the forward, billboard, and fog pipelines — the same bind-group objects are reused, not re-uploaded. When a new pipeline stage consumes a shared BGL, each accessed binding's `visibility` must include that stage (e.g. `FRAGMENT → FRAGMENT | COMPUTE`) — wgpu validates this at pipeline creation, not compile time. Two budget slots remain; a pass needing a ninth group must consolidate, not raise the limit.

**Widen visibility minimally.** The converse also bites: wgpu charges the per-stage binding-type limits (`max_storage_buffers_per_shader_stage`, `max_sampled_textures_per_shader_stage`) against the BGL *entry* set per stage, not against what a given shader reads. Adding `VERTEX` (or `COMPUTE`) to a shared entry that the new stage does **not** read still spends a slot in that stage's budget. The renderer does **not** raise `max_storage_buffers_per_shader_stage` above the downlevel/WebGPU default of 8 (broad hardware compat for a modder-friendly retro FPS), so an entry must carry a stage only when a shader in that stage genuinely reads it. The billboard pipeline sits at exactly six VERTEX-visible storage buffers against that ceiling of 8 (see §7.4); the `billboard_pipeline_vertex_storage_request_matches_bgl_definitions` test guards it headlessly.

The mapping above is the world-geometry path. The skinned model pass (§9) owns its own pipeline layout with a **distinct group mapping** — groups 0/1 carry the same camera/material bind groups, but groups 2 and 3 differ. No collision: each pipeline declares its own layout. See §9.

The renderer also requires `max_texture_dimension_2d ≥ 8192` (per-layer lightmap and SH atlas cap; wgpu's default already grants 8192) and `max_texture_array_layers ≥ 256` (lightmap and SH array-atlas layer cap; wgpu's default grants 256). Lightmaps and SH irradiance/direct atlases are `texture_2d_array` resources; PRL caps each layer to the 2D floor and spills overflow into array layers. An adapter pre-check fail-fasts with a named `[Renderer]` error if either limit is below its floor. Per-atlas runtime guards degrade oversized lightmaps to the neutral placeholder and disable oversized SH volumes cleanly, rather than panicking during texture creation or upload.

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

Set `POSTRETRO_GPU_TIMING=1` to enable per-pass GPU timing; for a normal dev launch use `RUST_LOG=info POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run`. With dev-tools enabled, use `RUST_LOG=info POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run --features dev-tools --`. Cargo flags before `--` go to the engine `cargo run`; args after it go to postretro. Requires adapter support for both `TIMESTAMP_QUERY` (pass-descriptor timestamps) and `TIMESTAMP_QUERY_INSIDE_ENCODERS` (multi-pass/copy brackets); silently disabled if either feature is absent. Passes measured: `cull`, `animated_lm_compose`, `depth_prepass`, `sdf_shadow`, `forward`, `sh_compose`, `direct_sh_compose`, `animated_direct_sh_compose`, `promoted_depth_cache_upper`, `dynamic_spot_depth_upper`, `dynamic_cube_depth_upper`, `smoke`, `bloom`, `billboard_direct_scatter_compose`. The two dynamic spans are intentionally upper bounds over interleaved cache, entity-pool, and promoted work; their 120-frame cache log reports exact skipped world passes and cull dispatches, which is the primary warm-cache savings proof. Results are averaged over a 120-frame window and logged via `log::info!` at the window boundary. SH sampling is not separately timestamp-bracketed because it runs inside the forward fragment shader; measure it as `forward` timing deltas before/after the octahedral migration and with Probe Occlusion on/off.

### Debug-Line Renderer

`dev-tools` only. Immediate-mode API: per-frame CPU buffer of `(start, end, color_rgba)` line segments uploaded to a `LineList` vertex buffer and drawn after the fog composite pass and before egui. Depth-tested lines match the world render target sample count, test against opaque scene depth, and keep depth writes off. Overlay/x-ray lines are a separate always-on-top stream. Buffer cleared at the top of the diagnostic emit call each frame, before new segments are pushed — not inside the render path — so it stays bounded even when `render_frame_indirect` early-returns (surface Timeout/Occluded/Outdated). Capped at a fixed segment limit (overflow: log + truncate). Consumers include SH volume diagnostics, nav/path overlays, remote-entity markers, and Spatial BVH/cell/portal overlays.

---

## 13. Non-Goals

- **Deferred rendering** — forward lighting with influence-volume early-out keeps per-fragment iteration proportional to nearby lights. Indoor portal-isolated geometry bounds the set further. Deferred adds complexity without benefit.
- **PBR materials** — albedo + normal map is the full material vocabulary. Metallic/roughness is out of scope.
- **Hardware ray tracing** — not in baseline wgpu, and absent at the §10 perf floor (Turing GTX 16-series has no RT cores). Shadow maps cover dynamic shadowing; SH volume covers indirect; SDF shadows sphere-trace in compute.
- **Mesh shaders** — not baseline in wgpu. GPU-driven culling uses compute + `draw_indexed_indirect`.
- **Runtime level compilation** — maps compiled offline by prl-build. Engine is a consumer only.
- **General-purpose multiplayer** — deterministic lockstep / rollback, competitive PvP, matchmaking, anti-cheat, peer-to-peer topologies, and full server-rewind lag compensation are out of scope. Authoritative client-server co-op is in scope.
