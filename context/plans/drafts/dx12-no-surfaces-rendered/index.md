# DX12 / NVIDIA: No Surfaces Rendered

> **Status:** draft — diagnosis-first plan; refine after Phase 0 before implementing fixes.
> **Related:** [rendering_pipeline.md §5, §7](../../../lib/rendering_pipeline.md) · [compute_cull.rs](../../../../postretro/src/compute_cull.rs) · [render/mod.rs](../../../../postretro/src/render/mod.rs)
> **Reported symptom:** On Windows 11 + NVIDIA GTX 1660, the engine runs to the main loop and presents the gray clear color, but no world surfaces appear. macOS (Metal) renders correctly from the same build.

---

## Goal

Make the engine render the world correctly on the wgpu DX12 backend on consumer NVIDIA hardware (Turing+). Treat this as a portability bug in the GPU-driven culling / draw-submission path, not a content or shader-correctness bug — the clear color paints, which rules out surface / swapchain issues.

---

## Diagnosis — hypotheses

Clear paints, geometry does not. Candidate failure modes, ranked by fit to the symptom after code review and a wgpu-29 API recheck:

1. **Camera lands in wrong cell on first frame.** Portal visibility returns an empty set whenever `camera_leaf >= leaf_count` or connectivity is absent — bitmask all zeros, every leaf culled, zero draws submitted. Real risk is not an out-of-range index (the BSP walk always returns a stored leaf) but a wrong-side-of-splitter result that lands the spawn point in a solid leaf or a disconnected one. Backend-dependent FP rounding in the plane-sign test can flip boundary classification between Metal and DX12. Fits the symptom exactly.
2. **DX12 cull-mode / winding-order mismatch.** If the world pipeline relies on wgpu defaults for `FrontFace` / `cull_mode` and the defaults happen to cull on DX12, the result is: clear paints, no surfaces. Cheap to verify: temporarily set `cull_mode: None`.
3. **Depth attachment contract inverted.** If depth clears to a value that fails the depth test against typical fragment depths (e.g. cleared to `0.0` with `CompareFunction::Less`, or vice versa after a reverse-Z flip), every fragment is rejected. One log line of depth clear + compare resolves it.
4. **MDI emulation-path bug on DX12.** wgpu 29 gates `multi_draw_indexed_indirect` on `DownlevelFlags::INDIRECT_EXECUTION` alone — the current probe is correct for correctness. But without `Features::MULTI_DRAW_INDIRECT_COUNT` the call is *emulated* as a loop of `draw_indirect`. Metal renders correctly, which means the native path works; DX12 uses the same emulation path under the same code. Emulation is implemented above the backend boundary, so a DX12-specific emulation regression is unlikely but not impossible. The fix, if implicated, is to opt into `MULTI_DRAW_INDIRECT_COUNT` on adapters that support it and switch to the native path. (Earlier drafts named this hypothesis #1 on the mistaken premise that a separate `Features::MULTI_DRAW_INDIRECT` was required for the non-count variant; no such feature exists in wgpu 29.)
5. **Compute → indirect synchronization.** wgpu inserts compute→indirect barriers within a submission in current versions; historical issues (#503, #2680, #2810) are resolved. Low probability, but Phase 0 readback flushes it out cheaply.
6. **`PowerPreference::default()` resolves to `LowPower`.** On a hybrid system this can pick the iGPU. Inert on a desktop GTX 1660 box, but cheap to rule out.
7. **SRGB swapchain format mismatch.** Would produce dark frames, not empty ones. Low probability; noted for completeness.

---

## Phase 0 — Diagnose (gates everything below)

Before changing renderer code, narrow the hypothesis on the user's affected box. Stop the list as soon as one step is decisive.

1. **Enable wgpu validation / verbose logs.** Run with `RUST_LOG=wgpu_core=warn,wgpu_hal=warn,info` (DX12 default). Capture:
   - `[Renderer] GPU adapter: …` (confirms GTX 1660 is selected)
   - `[Renderer] Indirect execution …` (confirms `DownlevelFlags::INDIRECT_EXECUTION` is present — it must be for any of the indirect paths to run)
   - Any wgpu validation / HAL errors, especially around bind groups, depth attachment, or pipeline state at first draw
2. **Cycle lighting isolation modes (Alt+Shift+4).** Recent commit `dbc25f8` added an isolation diagnostic. If an unlit / albedo-only mode shows surfaces, the world *is* rendering and the bug is in the lighting path (zero luminance indistinguishable from gray clear). That single keystroke eliminates hypotheses 1–5 in ten seconds.
3. **Log visibility set size and the camera's resolved leaf index** on the first frame. If the set is empty or the leaf is solid, hypothesis 1 is live — no later step is needed.
4. **Try Vulkan on the same box.** `WGPU_BACKEND=vulkan cargo run --release -p postretro …`. If Vulkan renders, the bug is DX12-specific; hypotheses 2 / 3 / 4 / 5 dominate. If Vulkan also fails, hypothesis 1 (visibility) dominates.
5. **Try the WARP software adapter.** `WGPU_ADAPTER_NAME=WARP cargo run …`. If WARP renders, the bug is NVIDIA-driver-specific; if WARP also fails empty, it's wgpu or engine-side.
6. **Take a PIX or RenderDoc capture** on the affected box. A capture shows definitively whether indirect draws were issued, what `index_count` they carried, and whether fragments survived depth / cull. Substitutes for most remaining guesswork in one shot.
7. **One-shot CPU readback of `indirect_buffer`.** After the first compute dispatch, copy to a `MAP_READ` staging buffer and log the first N entries. All zeros → compute didn't run or its writes didn't reach the indirect read; populated `index_count` but empty frame → render-side consumption issue (hypotheses 2 / 3 / 4 / 5).

The output of Phase 0 dictates which subset of Phase 1 ships.

---

## Phase 1 — Fixes (apply only those Phase 0 implicates)

### Fix A — Visibility fallback for ambiguous camera leaf

In [portal_vis.rs](../../../../postretro/src/portal_vis.rs) / [visibility.rs:13-18](../../../../postretro/src/visibility.rs):

- When the camera resolves to a solid leaf, a leaf with no portal connectivity, or (defensively) an out-of-range index, return `VisibleCells::DrawAll` for that frame instead of the current empty `Culled`. Emit a one-shot warning naming the resolved leaf index and spawn position.
- Rationale: the real risk is not "invalid index" — the BSP walk at [prl.rs:193-206](../../../../postretro/src/prl.rs) always returns a stored leaf — but "wrong-side-of-splitter lands the camera in the wrong cell, producing an empty visible set." `DrawAll` trades culling efficiency for a visible frame whenever visibility is ambiguous, a strict win for this failure mode.
- Audit the sign test at [prl.rs:201-206](../../../../postretro/src/prl.rs) for boundary handling. A boundary point on a splitter should land on the same side deterministically across backends; a strict `> 0.0` with an explicit equality branch is less surprising than `>= 0.0`.

Promoted to Fix A because Phase 0 step 3 (log visibility set size) is the cheapest decisive signal, and empty-visible-set is the best fit to the symptom.

### Fix B — Zero-init `indirect_buffer` (defense in depth)

In [compute_cull.rs:148-160](../../../../postretro/src/compute_cull.rs):

- `encoder.clear_buffer(&self.indirect_buffer, 0, None)` once at renderer construction (cheap, one-time cost).
- Rationale: the existing invariant is that the compute shader writes every leaf's slot every frame along the DFS ([bvh_cull.wgsl:94-151](../../../../postretro/src/shaders/bvh_cull.wgsl)), so on a correct dispatch no reset is needed. This change does *not* patch a known gap; it hardens the failure mode against future mistakes (dispatch dropped, early-out added, split submission) so the observable behavior becomes "nothing draws" instead of "draws random vertex ranges." Update the existing comment to reflect this.

### Fix C — Explicit pipeline winding / cull mode

- Audit the world render pipeline in `render/mod.rs` for an explicit `primitive.front_face` and `primitive.cull_mode`. If either relies on wgpu defaults, set them explicitly to the values we mean (expected: `FrontFace::Ccw`, `Face::Back`). Removes a class of silent backend-default divergence.

### Fix D — Verify depth-attachment contract

- Log the depth buffer's `ClearValue` and the pipeline's `DepthStencilState.depth_compare` at renderer startup. Assert they are consistent with each other (clear-to-`1.0` + `Less`, or clear-to-`0.0` + `Greater` if we ever adopt reverse-Z). If the defaults drifted, fix in place. This is cheap regardless of whether Phase 0 implicates it.

### Fix E — Opt into native MDI via `MULTI_DRAW_INDIRECT_COUNT`

In [render/mod.rs:482-494](../../../../postretro/src/render/mod.rs):

- `DownlevelFlags::INDIRECT_EXECUTION` is the correct gate for `multi_draw_indexed_indirect` and is already probed correctly. Keep it.
- Additionally probe `adapter.features().contains(wgpu::Features::MULTI_DRAW_INDIRECT_COUNT)`. When present, add it to `required_features`; per wgpu 29 docs, this disables emulation of the non-count multi-draws and drives them onto the native backend path.
- Log which path is in use: `native multi-draw`, `emulated multi-draw (loop of draw_indirect)`, or `singular draw_indirect fallback`.
- Ship only if Phase 0 implicates MDI emulation (hypothesis 4). On its own, this is a perf/clarity improvement, not a correctness fix.

### Fix F — Force `HighPerformance` adapter

In [render/mod.rs:444](../../../../postretro/src/render/mod.rs):

- Change `power_preference` to `HighPerformance`. No-op on a desktop with a single discrete GPU; on hybrid laptops it selects the discrete GPU.
- Log adapter info verbosely: name, vendor, device type, driver version, `adapter.limits().max_storage_buffer_binding_size` (DX12 may clamp differently from Metal for large BVHs).

### Fix G — (Diagnostic only) Split compute into its own submission

- If Phase 0 step 7 shows the indirect buffer is populated but draws remain empty, try submitting the compute encoder with `queue.submit([compute_encoder.finish()])` before building the render encoder.
- If this changes behavior, it indicates a fresh wgpu barrier bug on DX12. File a minimal upstream repro and add a comment referencing the issue so we can revert when it lands. This is a workaround, not a shipping fix by preference.

---

## Phase 2 — Regression armor

- **Document the supported backend matrix** in [rendering_pipeline.md](../../../lib/rendering_pipeline.md): which wgpu features are required, which are optional, what falls back. One short subsection.
- **`--debug-cull-readback` CLI flag** that performs the Phase 0 step 7 readback on demand. Keep behind a flag so it costs no frame time in normal runs but is one switch away the next time a backend bug appears.
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

1. **Does the bug reproduce on Vulkan on the same box?** Phase 0 step 4 answers this and cleaves the hypothesis space in half.
   - *To test:* `WGPU_BACKEND=vulkan cargo run --release -p postretro -- <map>`
2. **Does switching to native MDI via `MULTI_DRAW_INDIRECT_COUNT` change behavior on the affected box?** If yes, the emulation path on DX12 is implicated and the upstream case is worth filing. If no, emulation is inert and hypotheses 1–3 dominate.
   - *To test:* Requires implementing Fix E first (opt-in probe + feature request). Once in, compare with/without by toggling the `required_features |= MULTI_DRAW_INDIRECT_COUNT` line and checking whether rendering changes. Log output will confirm which path was taken.
3. **Is the wrong-cell-on-first-frame failure latent in normal play?** If Fix A fires outside this bug (warn-log in the wild), that's a secondary issue to chase — spawn-point classification is load-bearing.
   - *To test:* After Fix A ships, run with `RUST_LOG=warn,postretro=warn` and watch for the one-shot `[Visibility]` warning. If it fires on the GTX 1660 box at startup, hypothesis 1 is confirmed. If it fires in normal play on other machines, the BSP boundary issue is broader.
4. **Does splitting the compute submission measurably hurt frame time?** If Fix G ships, measure. If hurt is non-trivial, the upstream wgpu fix is on the critical path for reverting.
   - *To test:* Run with `POSTRETRO_GPU_TIMING=1 cargo run --release -p postretro -- <map>` before and after applying Fix G. Compare the per-pass GPU times logged at startup (compute pass + forward pass). A significant increase in forward-pass latency indicates the split is forcing a pipeline stall.

---

## Acceptance criteria

- [ ] Phase 0 diagnostics captured on the user's Windows + GTX 1660 box; the implicated hypothesis is named in the implementation PR.
- [ ] On that box, the engine renders world surfaces (not just the gray clear) when launched with the default DX12 backend.
- [ ] macOS (Metal) and Linux (Vulkan, if available) still render correctly — no regression on previously working platforms.
- [ ] Visibility fallback (Fix A) fires `VisibleCells::DrawAll` with a one-shot warning on any ambiguous camera leaf; empty visible set is no longer reachable on a correctly loaded map.
- [ ] If `DownlevelFlags::INDIRECT_EXECUTION` is absent, the renderer returns an actionable error at startup naming the adapter — it does not silently draw nothing.
- [ ] `[Renderer]` startup logs include adapter name, vendor, device type, driver version, key limits (storage-buffer binding size), and the indirect-execution path actually taken (native multi-draw, emulated multi-draw, or singular `draw_indexed_indirect`).
- [ ] [rendering_pipeline.md](../../../lib/rendering_pipeline.md) gains a short "Backend support" subsection naming required downlevel flags, optional features, and the fallback / emulation policy.

---

## When this plan ships

Durable knowledge migrates to [rendering_pipeline.md](../../../lib/rendering_pipeline.md):

- Backend support: `DownlevelFlags::INDIRECT_EXECUTION` is required; absence is a fatal startup error. `Features::MULTI_DRAW_INDIRECT_COUNT` is optional — when present, multi-draws run natively; when absent, wgpu emulates them as a loop of `draw_indirect`. `Features::FLOAT32_FILTERABLE` remains required.
- Visibility fallback policy: ambiguous camera leaf → `DrawAll` for that frame, noted alongside the existing fallback list in §2.
- If Fix G ships, the compute / indirect submission split is recorded with a link to the upstream wgpu issue so it can be reverted cleanly once fixed.

The plan document itself is removed per `development_guide.md` §1.5.
