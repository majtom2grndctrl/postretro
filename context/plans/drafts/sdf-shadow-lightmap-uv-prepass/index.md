# SDF Shadow Lightmap-UV Pre-pass

> v2 follow-up to `sdf-static-occluder-shadows` (done). Fixes the §SAMPLING APPROXIMATION the v1 spec explicitly deferred: the half-res SDF shadow pass currently samples the per-texel dominant direction textures using **screen-space UV**, which has no correspondence to the surface's lightmap UV. The direction read is uncorrelated with geometry, producing splotchy shadows that don't follow surfaces. Verified empirically via `SdfShadowMode::Visualize`.

## Goal

Reconstruct the visible surface's **lightmap UV** at every shaded pixel, so the SDF shadow pass can sample the static-lightmap baked dominant direction and the animated-baked per-frame dominant direction with **per-texel correctness** instead of screen-space noise. This restores the v1 SDF feature to its intended quality without changing its trace, penumbra, or aggregation math.

The chosen mechanism: a new **lightmap-UV render target on the depth pre-pass** (MRT slot), full resolution, `Rg16Unorm`. The pre-pass already runs the vertex stage that has `lightmap_uv_packed` in flight; adding one fragment that writes the interpolated UV is nearly free per pixel and benefits from early-Z (the written UV is the visible-surface UV). The half-res shadow pass samples this target at half-res pixel centers and uses the lightmap UV to index `static_lm_direction` and `animated_lm_direction`.

**Load-bearing decision — write on the pre-pass, not the forward pass.** Forward fragments are already expensive (SH lookups, lightmap fetches, normal-mapped shading, dynamic-light loop). Adding bandwidth there compounds the cost class that originally killed inline SDF (`done/sdf-static-occluder-shadows/research.md` — "Why SDF was removed"). Pre-pass fragments currently do **zero** shading work — one Rg16Unorm write is the minimum incremental cost and benefits from early-Z. Forward stays single-color-target.

This is a **quality fix on a shipped pipeline**, not a new lighting capability. Scope is deliberately narrow: one MRT slot, one binding, one shader-side substitution.

### Key constraint — do not re-create `f50314d`

`f50314d` (May 2026) reverted the fog quality bundle because a full-res direction pass + bilateral downsample dropped the 2020 MBP below 60fps with vsync. The structurally similar **Option E** below — full-res shadow pass + bilateral downsample — is rejected for the same reason. The chosen path keeps the shadow trace at half-res; only the UV-sampled gbuffer is full-res, and reading it is one `textureLoad` per half-res shader invocation.

## Options considered

Five candidates surveyed. The spec picks B and names the others so future readers don't re-propose them.

| | Option | Verdict | Reason |
|---|---|---|---|
| A | SH L1 dominant direction (reuse band-1 textures) | **Rejected (correctness)** | Zero new memory. But SH L1 encodes hemispheric irradiance direction; lightmap dominant direction is luminance-weighted shadow-caster direction — different quantities (`done/sdf-static-occluder-shadows/research.md:5–46`). Substituting SH L1 breaks the v1 trace's correctness contract. Grid spacing is 1m (`sh_bake.rs:23`) — still too coarse for per-texel direction. Named as last-resort perf-gate retreat only (see *Resolved decisions*). |
| B | Lightmap-UV gbuffer on pre-pass MRT | **Chosen** | Exact per-texel direction. ~8 MB at 1080p Rg16Unorm. Pre-pass write benefits from early-Z; forward stays single-target. |
| C | Chart-LUT GPU lookup (per-pixel chart search) | Rejected | High implementation risk; per-pixel search work in the shadow pass. |
| D | Lightmap-space SDF trace (re-bake per-chart 2D SDFs) | Rejected (scope) | Epic-sized. Would discard the proven brick + coarse-fallback structure the research says to keep. |
| E | Pre-pass + full-res direction pass + bilateral downsample | **Hard rejected** | Structurally identical to the fog quality bundle reverted in `f50314d`. The research's most explicit perf foot-gun. |

## Scope

### In scope

- A new **full-resolution lightmap-UV render target** (`Rg16Unorm`), allocated alongside the depth attachment and resized in lockstep with it on surface resize.
- A depth pre-pass shader gaining a **fragment stage** that interpolates `lightmap_uv_packed` (`@location(4)`, already present on the shared vertex layout but currently unused by the pre-pass) and writes the unpacked UV to the new target.
- A pre-pass pipeline gaining one color attachment with `LoadOp::Clear` to `(0.0, 0.0)` — see *Sentinel and consumer behavior*.
- An **SDF shadow pass binding** for the new target on its own pipeline layout (group 1, the pass's own group; **does not** consume group 7 — the last free top-level slot).
- A shader-side substitution at `sdf_shadow.wgsl`'s SAMPLING APPROXIMATION block: replace the screen-UV direction read with `textureLoad` of the lightmap-UV target at the half-res pixel's full-res sample point, then use that UV to index `static_lm_direction` and `animated_lm_direction`.
- The animated-lightmap-direction sampling path (Task 2b of v1) is covered by the **same** substitution — both direction textures are atlas-keyed on lightmap UV, both read through the same screen-UV bug today.
- Sentinel-and-degradation behavior for pixels with no lightmap UV (sky / discarded / pre-existing legacy maps with missing lightmap data) — see *Sentinel and consumer behavior*.

### Out of scope (non-goals)

- **Growing the pre-pass into a general gbuffer.** Pre-pass is locked to {depth, lightmap UV} — see *Resolved decisions*. Future features that want a slot require a new draft-plan.
- **Temporal accumulation of the lightmap-UV gbuffer.** Single-frame, written each frame, no reprojection.
- **MSAA.** The pre-pass is not MSAA'd today; this spec does not change that. If MSAA lands later, the new target must resolve to a single-sample view before the shadow pass reads it — out of scope.
- **A bilateral upsample on the UV gbuffer itself.** UV is sampled at half-res pixel centers via nearest-equivalent `textureLoad` — the shadow factor's own bilateral upsample (already in forward, Task 5 of v1) is unchanged.
- **Transparent surfaces taking SDF static-occluder shadows.** Transparency does not participate in the depth pre-pass today and continues not to. (The current pipeline already does not shadow transparents from static occluders.)
- **Changing the SDF trace, penumbra estimate, aggregation, or channel layout.** Only the direction-sampling source changes.
- **Fixing rendering_pipeline.md §7.5 drift** or any other doc drift outside this feature's footprint.

## Sentinel and consumer behavior

A pixel may have no usable lightmap UV in three cases: depth = clear sentinel (sky / no geometry), the visible surface has no lightmap UV (e.g. dynamic geometry that participates in the pre-pass in the future — none today), or the legacy degradation path (pre-Task 2a PRL with no animated-lightmap directions).

**Clear value.** The pre-pass clears the lightmap-UV target to `(0.0, 0.0)`. `(0.0, 0.0)` is the sentinel: `chart_raster.rs:20` sets `CHART_PADDING_TEXELS = 2`, so true `(0,0)` is unreachable by any real chart texel. See *Resolved decisions — MRT format* for the format choice rationale.

Verify on `campaign-test.prl` that chart-edge precision is acceptable; if visible artifacts appear, fall back to `Rg16Float` with a `(-1.0, -1.0)` negative sentinel.

**Shadow-pass consumer behavior.** Already covered by the existing path: the half-res shadow shader skips the trace when `reconstruct_world` returns `.w == 0.0` (the existing `recon.w > 0.5` check at `sdf_shadow.wgsl:248`) (depth at clear sentinel). For pixels where depth is valid but the UV reads `(0.0, 0.0)` — should not happen given pre-pass writes every fragment that wrote depth — confirmed: today's `depth_prepass.wgsl` has no fragment stage, no `discard`, and no alpha-test, and the new fragment stage added by this spec adds none either, so depth-write ↔ color-write is 1:1 — but defensively — the shadow pass returns `FULLY_LIT` for that pixel (same path as `sdf_meta.present == 0`). No new visual artifact: `FULLY_LIT` means "no SDF factor applied," which is what v1 already does for sky / no-atlas cases.

**Pre-pass + forward consistency.** The pre-pass and forward share a vertex layout and use `@invariant` on clip-space position to guarantee `depth_compare: Equal` matches bit-for-bit. Confirmed correct on Metal (`[[position, invariant]]`) and Vulkan (SPIR-V `Invariant` decoration); v1 already ships `@invariant` for `depth_compare: Equal`. The lightmap-UV write follows the same vertex path, so the UV written at pre-pass equals the UV interpolated at forward for the same fragment. This holds because the spec targets 1× sample rate with no MSAA; `@invariant` covers fragment identity, and at identical viewport + 1× rate, the interpolator produces the same per-fragment UV. If MSAA is added later, this assumption must be re-examined.

## Acceptance criteria

Automated (test- or tooling-gated):

- [ ] The depth pre-pass pipeline declares one color target of format `Rg16Unorm`; the renderer allocates a matching texture at surface size and recreates it on resize. Asserted via the pipeline descriptor and a resize-path unit test. [T1, T2]
- [ ] The pre-pass shader exports a fragment stage that writes the unpacked lightmap UV at `@location(0)`. Asserted via source-string check that `@location(0)` appears on the pre-pass fragment return (matches the existing test style in `render/sdf_shadow.rs` tests). [T1]
- [ ] The SDF shadow pass's pipeline layout adds the lightmap-UV target binding at `@group(1) @binding(6)` inside the pass-owned bind group (group 1), not in a globally-shared group. Asserted on the BGL. [T3]
- [ ] `sdf_shadow.wgsl` no longer references screen-UV sampling of `static_lm_direction` / `animated_lm_direction`; both are indexed via a UV value sourced from the new lightmap-UV target. Asserted by source-string check (matches the existing test style in `render/sdf_shadow.rs` tests). [T3]
- [ ] The pre-pass color attachment uses `LoadOp::Clear` with `r = 0.0` and `g = 0.0`. Asserted via render-pass descriptor inspection in a unit test. [T2]
- [ ] On a map with the SDF atlas loaded, GPU timing for the depth pre-pass increases by no more than a small fraction of frame time on the 2020 MBP target (budget: ≤0.3 ms *delta* over the current pre-pass time at 1080p; if it exceeds, the feature is not shippable and the perf-gate retreats in *Resolved decisions* apply). Measured via `POSTRETRO_GPU_TIMING=1` (measured on the 2020 MBP target adapter, which supports `TIMESTAMP_QUERY`; absence of measurement on other adapters does not trigger the retreats — only measured failure on the target adapter does). [T1, T2]

Manual / visual (observed by a human running the engine — not machine-verified):

- [ ] In `SdfShadowMode::Visualize` on `content/dev/maps/campaign-test.prl`, the static-aggregate shadow factor follows wall and ceiling geometry rather than splotching independently of screen position. Concretely: a wall-floor crease shows a continuous shadow band along the geometry edge, not isolated patches near the screen-top region.
- [ ] In `SdfShadowMode::On`, static-occluder shadows are recognizably attached to occluder geometry — an off-screen static occluder casts a shadow that lands where the occluder's projected silhouette predicts (one of v1's stated Quake-impossible wins, currently broken by the screen-UV bug).
- [ ] The animated-baked sweep in the arena now casts a shadow that both **tracks the brightness phase** (already in v1) **and lands on the correct surfaces** (the v2 fix). Both effects visible simultaneously.
- [ ] With no PRL loaded (no geometry), `SdfShadowMode::Visualize` shows uniformly fully-lit output (the sentinel + `FULLY_LIT` fallback path is exercised everywhere).
- [ ] No visible regression in non-SDF passes (forward shading, fog composite, smoke) — the only consumers of the pre-pass depth attachment are unchanged, and forward gains no new binding.

### Task ↔ AC cross-check

| Task | Covering ACs |
|---|---|
| T1 | pipeline declares Rg16Unorm MRT; shader exports fragment; GPU timing budget |
| T2 | resize allocates new target; clear color = (0,0,_,_); GPU timing budget |
| T3 | shadow-pass binding in group 1; shader-side substitution; visual ACs (incl. sentinel path) |

## Tasks

### Task 1: Lightmap-UV gbuffer plumbing (pre-pass)

`crates/postretro/src/shaders/depth_prepass.wgsl` gains a fragment stage. The vertex stage adds `@location(4) lightmap_uv_packed: vec2<u32>` to its `VertexInput` (it is already in the bound vertex buffer at offset 28; the pre-pass currently declares attributes at offsets 0, 12, 20, 24 (locations 0–3); the new lightmap-UV attribute is added at offset 28, location 4, format `Uint16x2`). Unpack in the vertex stage (mirroring `forward.wgsl:273`), pass `vec2<f32>` as a vertex output, and have the fragment passthrough to `@location(0)`.

Pipeline-side: `depth_prepass_pipeline` (`render/mod.rs:1635`) gains a `fragment: Some(...)` block declaring one color target of format `Rg16Unorm`, no blend. The pipeline layout is unchanged — fragment stage reads from no resources. Depth-stencil state is unchanged — still `depth_compare: Less` with `depth_write_enabled: true` (do not mirror forward's `Equal`).

The unpacked-UV computation mirrors `forward.wgsl:273`. The interpolated value at the fragment is the correct lightmap UV at the visible surface — the `@invariant` clip-Z and `depth_compare: Equal` guarantee from §7.2 of `rendering_pipeline.md` keeps the visible-surface fragment fixed.

### Task 2: Renderer resource ownership + resize

A new `lightmap_uv_view: wgpu::TextureView` field on `Renderer`, allocated by a helper analogous to `create_depth_texture` (`render/mod.rs:446`). Cleared each frame via `LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 })` — only `r`/`g` are read by the `Rg16Unorm` target; `b`/`a` are ignored. Resized in the surface-resize path alongside the depth view (`render/mod.rs:2568–2570`). `SdfShadowPass::resize` gains a `&lightmap_uv_view` parameter; the call site at `render/mod.rs:2574–2575` is updated in the same pass.

The pre-pass render-pass descriptor (`render/mod.rs:3284`) gains a `color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &lightmap_uv_view, ... })]` entry.

### Task 3: SDF shadow pass consumption

`SdfShadowPass` gains a `lightmap_uv_tex` field on its pass-owned bind group, layout entry, and BGL — group 1 in `sdf_shadow.wgsl`, where the depth texture already lives (binding 1). Group 1 in `sdf_shadow.wgsl` is the SDF shadow pass's own BGL — all six existing bindings (lines 71–81: params, depth, static/animated direction textures, sh_depth_moments, shadow_factor write target) are pass-private, not shared. Bind `lightmap_uv_tex` at `@group(1) @binding(6)`. The pass's `update_targets` / resize path threads the new view alongside the existing depth view.

Shader-side, `sdf_shadow.wgsl` replaces the SAMPLING APPROXIMATION block (the `let uv = ...` through `let animated_dir = ...` span inside the `if (recon.w > 0.5)` body); the `trace_shadow` calls that follow are unchanged:

```wgsl
// Replace screen-UV sampling with pre-pass-written UV.
let lm_dims = textureDimensions(lightmap_uv_tex);
let scale_x = f32(lm_dims.x) / f32(params.half_res_size_x);
let scale_y = f32(lm_dims.y) / f32(params.half_res_size_y);
let full_x = i32(min((f32(half_xy.x) + 0.5) * scale_x, f32(lm_dims.x) - 1.0));
let full_y = i32(min((f32(half_xy.y) + 0.5) * scale_y, f32(lm_dims.y) - 1.0));
let lm_uv = textureLoad(lightmap_uv_tex, vec2<i32>(full_x, full_y), 0).rg;
if (lm_uv.x == 0.0 && lm_uv.y == 0.0) {
    // Pre-pass sentinel (Rg16Unorm cleared to (0,0)) — no fragment wrote this pixel. Bail to fully lit.
    textureStore(shadow_factor, vec2<i32>(i32(half_xy.x), i32(half_xy.y)), vec4<f32>(FULLY_LIT, FULLY_LIT, 1.0, 1.0));
    return;
}
let static_dims = textureDimensions(static_lm_direction, 0);
let static_coord = vec2<i32>(i32(lm_uv.x * f32(static_dims.x)), i32(lm_uv.y * f32(static_dims.y)));
// ...same for animated_lm_direction
```

Recomputing `scale_x` / `scale_y` here mirrors the existing block in `reconstruct_world` — they are local to that function and not visible at `cs_main` scope. The lightmap-UV target shares full-res dimensions with depth, so the ratios are identical. The trace, penumbra, and write paths are untouched. The "SAMPLING APPROXIMATION" header comment is removed and replaced with a one-line note pointing at the pre-pass MRT.

### Sequencing

Strictly sequential — Tasks 1–3 form a single dependency chain (shader → renderer wiring → consumer). Splitting them would leave the pre-pass writing a target nobody reads or the shadow pass binding a non-existent texture; not worth the parallelism.

## Wire format

No PRL changes. This is a pure runtime-pipeline addition: no baked data, no section, no version bump.

## Rough sketch

- **One MRT, full-res, Rg16Unorm.** ~8 MB at 1080p; ~500 MB/s bandwidth at 60 fps. Well within the 2020 MBP budget — pre-pass writes hit early-Z so the per-fragment cost is effectively one ROP write. `Rg16Unorm` is a round-trip for the `u16/65535` packing in `forward.wgsl:273–275`; uniform ~1.5e-5 precision across [0,1].
- **Sentinel = (0.0, 0.0).** True `(0,0)` is unreachable by any real chart texel — see *Resolved decisions — MRT format*. The shadow-pass consumer reads the sentinel and bails to `FULLY_LIT`.
- **Same fix covers both direction textures.** Static-lightmap and animated-baked atlases are both keyed on lightmap UV — one UV reconstruction substitutes both indexings.
- **No new top-level bind group.** The new binding lives inside the SDF shadow pass's own group 1 layout. Group 7 (last free) stays free.
- **No forward changes.** Forward neither writes nor reads the new target. The cost lives entirely in the pre-pass (write) and the half-res shadow pass (one `textureLoad`).

## Resolved decisions (formerly Open questions)

All five open questions from the initial draft are resolved. No unresolved questions remain.

**MRT format — `Rg16Unorm`, not `Rg16Float`.** Lightmap UVs are born as `u16/65535` (`forward.wgsl:273–275`). `Rg16Unorm` is a literal round-trip with uniform ~1.5e-5 precision. `Rg16Float` loses mantissa precision near 1.0 — exactly where edge-packed charts sit. `Rgba16Float` doubles memory for no gain. `Rg11B10` loses precision below 0.0005 — too tight. Sentinel is `(0.0, 0.0)` (negative sentinel is impossible with unorm; `(0,0)` is safe because `CHART_PADDING_TEXELS = 2`, `chart_raster.rs:20`). Verify on `campaign-test.prl` that chart-edge precision is acceptable; fall back to `Rg16Float` with `(-1.0, -1.0)` sentinel if artifacts appear.

**Option A (SH L1) — analyzed and rejected on correctness grounds.** SH L1 encodes hemispheric irradiance direction; lightmap dominant direction is luminance-weighted shadow-caster direction (`done/sdf-static-occluder-shadows/research.md:5–46`). Different quantities — substituting SH L1 breaks the v1 trace's correctness contract. Additionally, `sh_bake.rs:23` sets `DEFAULT_PROBE_SPACING = 1.0` m — still too coarse for per-texel direction even if the quantity were right. Option A remains named only as a last-resort perf-gate retreat that accepts a quality compromise (see below).

**Pre-pass-as-mini-gbuffer policy — locked to {depth, lightmap UV}.** Per `feedback_hardcoded_but_seamed`: one named chokepoint, not a speculative framework. Per `project_m10_velocity_shift`: don't pre-build what isn't needed. Per `feedback_plan_threshold_for_renderer_features`: a future MRT consumer is a renderer feature requiring its own draft-plan. This spec does not establish a "slot budget." Name the chokepoint during implementation (something like `create_depth_prepass_attachments()`); future features that want a slot require a new draft-plan to justify it.

**Half-res sampling — single tap, resolved.** The shadow pass uses one `textureLoad` per half-res invocation. Forward's depth-aware bilateral upsample at `forward.wgsl:486` (`upsample_shadow_factor`) already rejects bad half-res taps at depth discontinuities — dilating to depth-closest-of-2×2 duplicates that rejection. Atlas-edge mis-samples (coplanar charts at atlas boundary) read ~4 cm of bake noise (`lightmap_bake.rs:21` ≈ 4 cm/texel) — bounded, within the trace's penumbra tolerance. Verify via `SdfShadowMode::Visualize` instrumentation that no chart-edge fringe is visible; if it is, revisit.

**Animated-lightmap-direction sampling — same bug, same fix, confirmed.** The animated direction atlas is keyed on the same lightmap UV as the static atlas (both indexed by `lightmap_uv`). The Task 3 substitution fixes both reads with the same UV value. No sibling spec needed.

**Perf-gate retreats.** If the `≤0.3 ms` pre-pass budget AC fails on the 2020 MBP, retreats in priority order: (b) shrink the lightmap-UV target to half-res, written by a per-half-res sample-the-nearest-depth-tap pass between depth pre-pass and shadow pass — adds one tiny pass, lets the shadow read be 1:1; (c) compute-shader UV write keyed on depth + face-id buffer — skips fragment-stage MRT bandwidth entirely, but needs a screen-to-face mapping not present today, so costlier to design than (b); (a) drop to Option A (SH L1), last resort, accepts a correctness/quality compromise (see above). None of these are currently designed.
