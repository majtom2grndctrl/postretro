# DX12 / NVIDIA: No Surfaces Rendered

> **Status:** draft — diagnosis-first plan; refine after Phase 0 before implementing fixes.
> **Related:** [rendering_pipeline.md §5, §7](../../../lib/rendering_pipeline.md) · [compute_cull.rs](../../../../postretro/src/compute_cull.rs) · [render/mod.rs](../../../../postretro/src/render/mod.rs)
> **Reported symptom:** On Windows 11 + NVIDIA GTX 1660, the engine runs to the main loop and presents the gray clear color, but no world surfaces appear. macOS (Metal) renders correctly from the same build.

---

## Goal

Make the engine render the world correctly on the wgpu DX12 backend on consumer NVIDIA hardware (Turing+). Treat this as a portability bug in the GPU-driven culling / draw-submission path, not a content or shader-correctness bug — the clear color paints, which rules out surface / swapchain issues.

---

## Diagnosis — hypotheses

The clear color paints but no geometry is drawn. Candidate failure modes, ranked by likelihood after code review:

1. **`Features::MULTI_DRAW_INDIRECT` is not enabled.** [render/mod.rs:473](../../../../postretro/src/render/mod.rs) requests only `FLOAT32_FILTERABLE`. But [compute_cull.rs:421](../../../../postretro/src/compute_cull.rs) calls `multi_draw_indexed_indirect` whenever the downlevel probe at `render/mod.rs:456` (`DownlevelFlags::INDIRECT_EXECUTION`) is set. Those are two different capabilities: the downlevel flag gates singular `draw_indirect` / `draw_indexed_indirect`; `multi_draw_indexed_indirect` requires the opt-in `Features::MULTI_DRAW_INDIRECT` feature. Calling the multi variant without the feature is a wgpu validation error. On DX12 the draw is skipped, producing exactly this symptom. This alone may be the full root cause.
2. **Camera lands in the wrong cell on first frame.** [portal_vis.rs:141](../../../../postretro/src/portal_vis.rs) already guards `camera_leaf >= leaf_count` by returning an empty visible set, which means bitmask all zeros and every leaf culled. The concern here is not an out-of-range index (the BSP walk in [prl.rs:193-206](../../../../postretro/src/prl.rs) always returns a valid stored leaf), but a wrong-side-of-a-splitter result that places the camera in a solid leaf or one with no portal connectivity. Backend-dependent FP rounding in the plane-sign test could change which side of a boundary the spawn point lands on.
3. **DX12 cull-mode / winding-order mismatch.** If the render pipeline relies on wgpu defaults for `FrontFace` / `cull_mode` and the defaults happen to cull the world on DX12, the result is exactly this: clear color, no surfaces. Cheap to verify by temporarily setting `cull_mode: None`.
4. **Depth attachment defaults reject every fragment.** If the depth buffer is cleared to a value that fails the depth test against typical fragment depths (e.g. cleared to `0.0` with `CompareFunction::Less`), every fragment is rejected. One log line of the depth descriptor resolves it.
5. **Compute → indirect synchronization.** wgpu inserts compute→indirect barriers within a submission in current versions, so this is less likely than earlier searches suggested (historical wgpu issues #503, #2680, #2810 are resolved). Keep as a diagnostic if the above four come up empty.
6. **`PowerPreference::default()` resolves to `LowPower`.** On a hybrid system this can pick the iGPU. Inert on a desktop GTX 1660 box, but cheap to rule out.
7. **SRGB swapchain format mismatch.** Would produce dark frames, not empty ones. Low probability; noted for completeness.

---

## Phase 0 — Diagnose (gates everything below)

Before changing renderer code, narrow the hypothesis on the user's affected box. Stop the list as soon as one step is decisive.

1. **Enable wgpu validation / verbose logs.** Run with `RUST_LOG=wgpu_core=warn,wgpu_hal=warn,info` (DX12 default). A missing `MULTI_DRAW_INDIRECT` feature will surface as a validation error the moment `multi_draw_indexed_indirect` is first called — this is the single cheapest signal for hypothesis 1. Capture:
   - `[Renderer] GPU adapter: …` (confirms GTX 1660 is selected)
   - `[Renderer] Indirect execution …` (tells us whether the singular fallback is active)
   - Any wgpu validation / hal errors
2. **Cycle lighting isolation modes (Alt+Shift+4).** Recent commit `dbc25f8` added an isolation diagnostic. If an unlit / albedo-only mode shows surfaces, the world *is* rendering and the bug is in the lighting path (zero luminance indistinguishable from gray clear). That single keystroke eliminates hypotheses 1–5 in ten seconds.
3. **Try Vulkan on the same box.** `WGPU_BACKEND=vulkan cargo run --release -p postretro …`. If Vulkan renders, the bug is DX12-specific and hypotheses 1 / 3 / 4 / 5 dominate. If Vulkan also fails, hypothesis 2 (visibility) dominates.
4. **Try the WARP software adapter.** `WGPU_ADAPTER_NAME=WARP cargo run …`. If WARP renders correctly, the bug is NVIDIA-driver-specific; if WARP also fails empty, it's wgpu or engine-side.
5. **Take a PIX or RenderDoc capture** on the affected box. A capture shows definitively whether indirect draws were issued, what `index_count` they carried, and whether the fragments survived depth / cull. One capture substitutes for most of the remaining guesswork.
6. **One-shot CPU readback of `indirect_buffer`.** After the first compute dispatch, copy to a `MAP_READ` staging buffer and log the first N entries. All zeros → compute didn't run or its writes didn't reach the indirect read; populated `index_count` but empty frame → render-side consumption issue.
7. **Log visibility set size and the camera's resolved leaf index** on the first frame. If the set is empty or the leaf is solid, hypothesis 2 is live.

The output of Phase 0 dictates which subset of Phase 1 ships.

---

## Phase 1 — Fixes (apply only those Phase 0 implicates)

### Fix A — Enable and correctly probe `MULTI_DRAW_INDIRECT`

In [render/mod.rs:444-477](../../../../postretro/src/render/mod.rs):

- Probe `adapter.features().contains(wgpu::Features::MULTI_DRAW_INDIRECT)` *in addition to* the existing `DownlevelFlags::INDIRECT_EXECUTION` probe. **Keep the downlevel probe** — it is the correct gate for the singular `draw_indexed_indirect` fallback path at [compute_cull.rs:427-430](../../../../postretro/src/compute_cull.rs).
- If the multi-draw feature is supported, add it to `required_features` alongside `FLOAT32_FILTERABLE` and drive `has_multi_draw_indirect` from this new probe.
- If the multi-draw feature is absent but `INDIRECT_EXECUTION` is present, use the singular fallback and log loudly which path was chosen.
- If neither is present, return an error from renderer construction with an actionable message naming the adapter.

This corrects the current conflation of two separate capabilities and is a strong candidate to fix the symptom on its own.

### Fix B — Zero-init `indirect_buffer` (defense in depth)

In [compute_cull.rs:148-160](../../../../postretro/src/compute_cull.rs):

- `encoder.clear_buffer(&self.indirect_buffer, 0, None)` once at renderer construction (cheap, one-time cost).
- Rationale: the existing invariant is that the compute shader writes every leaf's slot every frame along the DFS ([bvh_cull.wgsl:94-151](../../../../postretro/src/shaders/bvh_cull.wgsl)), so on a correct dispatch no reset is needed. This change does *not* patch a known gap; it hardens the failure mode against future mistakes (dispatch dropped, early-out added, split submission) so the observable behavior becomes "nothing draws" instead of "draws random vertex ranges." Update the existing comment to reflect this.

### Fix C — Explicit pipeline winding / cull mode

- Audit the world render pipeline in `render/mod.rs` for an explicit `primitive.front_face` and `primitive.cull_mode`. If either relies on wgpu defaults, set them explicitly to the values we mean (expected: `FrontFace::Ccw`, `Face::Back`). Removes a class of silent backend-default divergence.

### Fix D — Verify depth-attachment contract

- Log the depth buffer's `ClearValue` and the pipeline's `DepthStencilState.depth_compare` at renderer startup. Assert they are consistent with each other (clear-to-`1.0` + `Less`, or clear-to-`0.0` + `Greater` if we ever adopt reverse-Z). If the defaults drifted, fix in place. This is cheap regardless of whether Phase 0 implicates it.

### Fix E — Visibility fallback for ambiguous camera leaf

In [portal_vis.rs](../../../../postretro/src/portal_vis.rs) / [visibility.rs:13-18](../../../../postretro/src/visibility.rs):

- When the camera resolves to a solid leaf, a leaf with no portal connectivity, or (defensively) an out-of-range index, return `VisibleCells::DrawAll` for that frame instead of the current empty `Culled`. Emit a one-shot warning.
- Rationale: the real risk is not "invalid index" — the BSP walk at [prl.rs:193-206](../../../../postretro/src/prl.rs) always returns a stored leaf — but "wrong-side-of-splitter lands the camera in the wrong cell, producing an empty visible set." `DrawAll` trades culling efficiency for a visible frame whenever visibility is ambiguous, a strict win for this failure mode.
- Audit the sign test at [prl.rs:201-206](../../../../postretro/src/prl.rs) for boundary handling. A boundary point on a splitter should land on the same side deterministically across backends; a strict `> 0.0` with an explicit equality branch is less surprising than `>= 0.0`.

### Fix F — Force `HighPerformance` adapter

In [render/mod.rs:444](../../../../postretro/src/render/mod.rs):

- Change `power_preference` to `HighPerformance`. No-op on a desktop with a single discrete GPU; on hybrid laptops it selects the discrete GPU.
- Log adapter info verbosely: name, vendor, device type, driver version, `adapter.limits().max_storage_buffer_binding_size` (DX12 may clamp differently from Metal for large BVHs).

### Fix G — (Diagnostic only) Split compute into its own submission

- If Phase 0 step 6 shows the indirect buffer is populated but draws remain empty, try submitting the compute encoder with `queue.submit([compute_encoder.finish()])` before building the render encoder.
- If this changes behavior, it indicates a fresh wgpu barrier bug on DX12. File a minimal upstream repro and add a comment referencing the issue so we can revert when it lands. This is a workaround, not a shipping fix by preference.

---

## Phase 2 — Regression armor

- **Document the supported backend matrix** in [rendering_pipeline.md](../../../lib/rendering_pipeline.md): which wgpu features are required, which are optional, what falls back. One short subsection.
- **`--debug-cull-readback` CLI flag** that performs the Phase 0 step 6 readback on demand. Keep behind a flag so it costs no frame time in normal runs but is one switch away the next time a backend bug appears.
- **Headless DX12 smoke job** on a Windows CI runner that asserts a non-zero frame draw count via a debug counter. Skip if no runner is available — flag as future work.

---

## Scope

### In scope

- Diagnosis steps that run on the user's affected box.
- Renderer-side fixes A–G, gated by Phase 0 findings.
- Documentation of the supported backend matrix.
- A reusable diagnostic flag for future backend bugs.

### Out of scope

- Switching off the GPU-driven culling path entirely (still the right architecture per [rendering_pipeline.md §5](../../../lib/rendering_pipeline.md)).
- Per-vendor workarounds keyed on adapter name strings.
- Any HAL-level / `unsafe` barrier insertion — violates project invariants ([CLAUDE.md](../../../../CLAUDE.md), `development_guide.md` §3.5).
- Bumping the wgpu version as a fix mechanism. If an upgrade incidentally fixes it, we accept that — but it isn't the plan.

---

## Open questions

1. **Does the bug reproduce on Vulkan on the same box?** Phase 0 step 3 answers this and cleaves the hypothesis space in half.
2. **Does the GTX 1660 expose `MULTI_DRAW_INDIRECT` on the wgpu DX12 backend?** If not, the singular fallback becomes the production path on that hardware and needs a perf check.
3. **Is there a wrong-cell-on-first-frame issue latent in the visibility path?** If Fix E fires in normal play (not just on this bug), that's a secondary issue to chase.
4. **Does splitting the compute submission measurably hurt frame time?** If Fix G ships, measure. If hurt is non-trivial, the upstream wgpu fix is on the critical path for reverting.

---

## Acceptance criteria

- [ ] Phase 0 diagnostics captured on the user's Windows + GTX 1660 box; the implicated hypothesis is named in the implementation PR.
- [ ] On that box, the engine renders world surfaces (not just the gray clear) when launched with the default DX12 backend.
- [ ] macOS (Metal) and Linux (Vulkan, if available) still render correctly — no regression on previously working platforms.
- [ ] If `MULTI_DRAW_INDIRECT` is unavailable on the adapter, the singular fallback is exercised and rendering still works.
- [ ] `[Renderer]` startup logs include adapter name, vendor, device type, driver version, key limits (storage-buffer binding size), and the indirect-execution path actually taken.
- [ ] [rendering_pipeline.md](../../../lib/rendering_pipeline.md) gains a short "Backend support" subsection naming required and optional wgpu features and the fallback policy.

---

## When this plan ships

Durable knowledge migrates to [rendering_pipeline.md](../../../lib/rendering_pipeline.md):

- The renderer requests `FLOAT32_FILTERABLE` and (when available) `MULTI_DRAW_INDIRECT`; documents the singular `draw_indexed_indirect` fallback and how the two capabilities are probed separately.
- The visibility fallback policy ("ambiguous camera leaf → `DrawAll` for that frame") is noted alongside the existing fallback list in §2.
- If Fix G ships, the compute / indirect submission split is recorded with a link to the upstream wgpu issue so it can be reverted cleanly once fixed.

The plan document itself is removed per `development_guide.md` §1.5.
