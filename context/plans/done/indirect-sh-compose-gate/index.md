# Indirect SH Compose Dispatch Gate

## Goal

Gate the indirect SH compose compute pass so it dispatches only when its composed atlas would change, mirroring the direct SH compose sibling's `dispatch_if_needed` gate. On levels and frames with no live animated indirect lights this removes a per-frame compute pass plus a full atlas-sized storage write that today recomputes byte-identical output.

## Scope

### In scope

- Add whole-pass dispatch gating to `ShComposeResources` (indirect SH compose, `crates/renderer/src/render/sh_compose.rs`): dispatch on initial level-load copy-through, while any animated indirect descriptor is active, once when activity returns to zero (settling the atlas to base-only), and whenever the per-frame `LightTermMask` differs from the mask that produced the current atlas.
- Retain the per-light animation descriptor-index list (`animation_descriptor_indices`, binding 25) on `ShComposeResources`; add `has_active_animated_descriptor` delegating to the shared `AnimatedLightBuffers::any_active_for_descriptor_indices`.
- Thread the per-frame `LightTermMask` into the indirect dispatch and into `record_pre_scene_compute`.
- Unit-test the gating predicate; add struct/source coverage as needed.
- Update the `ShComposeResources` doc-comment and `context/lib/rendering_pipeline.md` §7.1 step 5.

### Out of scope

- Direct, animated-direct, billboard-scatter, and animated-lightmap passes — already gated or unaffected.
- Bind group layout, the compose shader, atlas allocation, and the bake / PRL pipeline — unchanged.
- Per-cell / per-tile culling of the indirect pass. When the pass dispatches it still writes the full affinity grid; only the whole-pass dispatch is gated. The pipeline doc's "(no culling)" distinction is preserved.
- Narrowing the mask comparison to only the indirect-relevant bits (see Direction → Alternatives rejected).

## Direction

**Problem.** The indirect SH compose pass dispatches unconditionally every world frame — `ShComposeResources::dispatch` is always encoded at its `record_pre_scene_compute` call site — recomputing and rewriting the entire composed indirect atlas even when neither the animated deltas nor the mask changed. Its direct sibling already gates this exact work.

**Prior commitments.** The direct SH compose pass established the gate: `direct_compose_should_dispatch(active, pending_copy_through, was_active, frame_light_term_mask, last_composed_mask)` (`direct_sh_compose.rs`), with the descriptor-activity signal `has_active_animated_descriptor` reading `AnimatedLightBuffers::any_active_for_descriptor_indices` (`sh_volume.rs`). Both passes share one `AnimatedLightBuffers` instance and one total-atlas persistence model: the atlas is written wholesale each dispatch — the full affinity grid, no cull — and never cleared, so a completed dispatch leaves every cell's slot valid and skipping the next dispatch leaves those values intact and correct. The never-cleared write model is shared with the animated-lightmap atlas (rendering_pipeline.md §7.1 step 4), but that atlas is culled to the frame's visible cells while this one is not; the full-grid write is what makes skipping safe here. This spec mirrors the direct gate onto the indirect pass. The one divergence from the sibling: the indirect shader reads the live group-0 `light_term_mask` directly (not a private per-pass uniform), so the gate needs no per-frame uniform write — only a CPU-side `last_composed_mask` comparison. This divergence removes work relative to the direct pattern; it does not add any.

**Alternatives rejected.**

1. *Coarse "level has any animated indirect light" bool*, like `animated_lightmap.is_active()` (which is level-fixed, true iff the compose pipeline exists). It captures the headline no-animated-lights win but keeps dispatching every frame on levels whose animated indirect lights are runtime-toggled off (`start_active == 0` at bake, or script-disabled via `set_active`). The descriptor-activity gate idles those frames too, reuses the existing shared helper, and adds little over the coarse bool.
2. *Masking the comparison to only the indirect-relevant mask bits* (static-indirect and animated-indirect). Saves at most one redundant compose per toggle of an unrelated mask bit — a rare, cheap event — at the cost of diverging from the direct sibling's full-mask compare and hard-coding bit semantics the gate otherwise never reads. The full-mask compare is the faithful, safe mirror.
3. *Extracting one shared `compose_should_dispatch` predicate* (and its test suite) instead of a local `indirect_compose_should_dispatch` twin. The predicate body is identical to `direct_compose_should_dispatch`, so DRY is tempting. Rejected: it is a single pure boolean expression, and sharing it would either invert the module dependency (`sh_compose` depending on `direct_sh_compose`) or pull a renderer-internal gate into a shared crate for one line — both worse couplings than a duplicated one-liner, especially since the two `dispatch_if_needed` methods legitimately diverge (the direct pass writes a private mask uniform, the indirect pass reads the live group-0 mask). Keep the predicate local to `sh_compose.rs`; the constraint is only that it stay behaviorally identical, which its own unit tests pin.

**Foreclosures / one-way doors.** Nothing material. The change is internal to the renderer; bind group layout, shader, atlas, and PRL format are untouched. Reverting is deleting the predicate and restoring unconditional dispatch.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Composed indirect atlas is byte-identical to unconditional-dispatch output on every frame | The predicate triggers on every per-frame-varying input to the compose output (see enumeration below) | The skip path must never drop a dispatch that would change the atlas | AC 5, predicate unit tests (AC 1, 3, 4) |
| Atlas settles to base-only on the first world frame after animated activity ends | `was_active` trailing term forces one final dispatch | Dropping `was_active` freezes the atlas at the last animated value | AC 3 |
| Base indirect irradiance is composed before the first world sample | `pending_copy_through == true` at construction | Atlas is zero-initialized at creation; first world frame must see the base | AC 1 |

**Output-input enumeration (the byte-identical warrant).** The compose output per texel is a pure function of: the base atlas and all delta/affinity/indirection/grid buffers (all immutable after level load); the animation descriptors, the animation samples, and frame `time`; and the group-0 `light_term_mask`. The descriptors' `is_active` flags, their curve params and `base_color`, and the scripted region of the animation samples are not baked-immutable — the `setLightAnimation` bridge rewrites them at runtime through `write_descriptor` and `upload_bridge_samples`, into the same shared `AnimatedLightBuffers` this pass reads. But every one of them reaches the output only through an active descriptor: `animated_light_scale` returns zero for any descriptor whose `is_active` is clear, so while a descriptor is inactive its curve params, its `base_color`, its samples, and advancing `time` cannot change any texel — and any change to an active descriptor falls on a frame where `active` is true. Therefore the complete set of per-frame inputs that can change the output is: (a) descriptor activity — any `is_active` flip, and any runtime change to an active descriptor's curve, color, or samples — all captured by `active`, which dispatches on every frame any descriptor is active, plus `was_active` for the one settling frame after activity ends; (b) `light_term_mask`, captured by the `last_composed_mask` compare. When all four predicate terms are false, none of these changed, so the atlas is already correct and the dispatch is safely skipped. No golden-image comparison task is required: completeness is established by this enumeration and exercised by the predicate unit tests.

## Orderings

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Mask changes during a non-world frame, world rendering resumes next frame | Non-world frames do not call the indirect dispatch, so `last_composed_mask` is not updated | Next world frame observes `frame_mask != last_composed_mask` and dispatches |
| No animated indirect lights (static level) | `active` is false every frame after load | One dispatch at level load (copy-through), then skipped every world frame until a mask change |
| Last active animated indirect descriptor toggled off | `active`: true → false | Frame of the transition dispatches (`was_active` true), settling to base-only; subsequent frames skip |
| Animated indirect descriptor toggled off then back on across consecutive frames | `active`: true → false → true | Dispatch, settling dispatch, dispatch — no frame left stale |
| Animated indirect descriptor active but its curve is momentarily constant | `active` true | Dispatches every frame (redundant but safe; matches the direct sibling, which dispatches every frame while active) |
| Animated indirect descriptor toggled active during a run of non-world frames | One or more non-world frames pass without dispatching, so `was_active`/`last_composed_mask` stay untouched; the descriptor is `set_active(true)` during that run | Next world frame observes `active` true and dispatches — the rising edge survives the non-world run |
| Animated indirect descriptor toggled inactive during a run of non-world frames | The last world dispatch left `was_active` true; the descriptor is `set_active(false)` during a run of non-world frames, which do not touch `was_active` | Next world frame observes `was_active` true, dispatches once (settling to base-only), then idles — the falling edge is not lost |

## Tasks

### Task 1: Gate the indirect SH compose dispatch

Give `ShComposeResources` (`crates/renderer/src/render/sh_compose.rs`) the same dispatch gate the direct sibling has. Retain `buffers.animation_descriptor_indices` as a new `Vec<u32>` field (mirroring `DirectShResources.animated_descriptor_indices`); the constructor already builds this list via `build_delta_buffers` and currently drops it. Add three cross-frame state fields initialized exactly as the direct pass does at construction: `pending_copy_through = true`, `was_active = false`, `last_composed_mask = LightTermMask::ALL` (import `LightTermMask` from `postretro_render_cpu::frame_uniforms`). Add `has_active_animated_descriptor(&self, animation: &AnimatedLightBuffers) -> bool` delegating to `animation.any_active_for_descriptor_indices(&self.animation_descriptor_indices)` — no new activity mechanism; both passes share the one `AnimatedLightBuffers` instance (`full.sh_volume_resources.animation`).

Convert `dispatch` to `dispatch_if_needed(&mut self, encoder, uniform_bind_group, active: bool, frame_light_term_mask: LightTermMask, timestamp_writes)`. Add a free predicate `indirect_compose_should_dispatch(active, pending_copy_through, was_active, frame_light_term_mask, last_composed_mask) -> bool` returning `active || pending_copy_through || was_active || frame_light_term_mask != last_composed_mask` (the compare is over the full mask, per Direction). On a false result, return without encoding; otherwise encode the existing compute pass unchanged, then set `pending_copy_through = false`, `was_active = active`, `last_composed_mask = frame_light_term_mask`. The indirect pass reads the live group-0 mask through `uniform_bind_group`, so — unlike the direct pass — no private mask uniform is written; the mask is used only for the CPU-side compare.

Plumb the inputs to the single dispatch call site (`record_pre_scene_compute`, `renderer_shadow_passes.rs`, the `if render_world` block that currently calls `full.sh_compose.dispatch`). Add a `frame_light_term_mask: LightTermMask` parameter to `record_pre_scene_compute`; its one caller (`renderer_render_frame.rs`, the `record_pre_scene_compute` call) supplies it from the existing `frame_light_term_mask()` getter (`renderer_light_terms.rs`) — that method already backs the direct pass's `frame_light_term_mask` local later in the same frame method. At the call site compute `let indirect_active = full.sh_compose.has_active_animated_descriptor(&full.sh_volume_resources.animation);` (an immutable read that ends before the mutable dispatch borrow), then call `full.sh_compose.dispatch_if_needed(encoder, &full.uniform_bind_group, indirect_active, frame_light_term_mask, sh_compose_ts)`; `sh_compose` and `uniform_bind_group` are disjoint fields of `full`, so the split borrow holds. The indirect `active` has no debug-override or promoted-weight terms — the descriptor-activity result is the whole signal.

Update the `ShComposeResources` doc-comment (currently "Unconditional dispatch avoids branching in the frame loop") to describe the gate, keeping the note that the pass writes a valid dummy dispatch for SH-less levels. The durable `context/lib/rendering_pipeline.md` §7.1 step 5 already states the gate (captured at promotion); confirm the shipped behavior matches it — dispatch on level-load copy-through, while any animated indirect light is active, once when activity returns to zero, and on mask change, with no per-cell/tile culling when it dispatches — rather than rewriting it.

Add unit tests mirroring the direct pass's predicate tests: copy-through, active, was-active-trailing, and quiet-skip cases; mask-change and return-to-same-mask re-dirty; and the mask-change-stays-dirty-until-a-world-dispatch-records-it ordering (a skipped world frame is impossible here since the call is inside `if render_world`, but a non-world frame not calling the dispatch must leave the change pending — cover the predicate directly). Verify the no-animated-lights idle without a GPU device — the direct sibling tests its retained-index behavior at the CPU `build_*_buffers` layer, not through the device-bound `new`: assert that `build_delta_buffers` yields an empty `animation_descriptor_indices` for a section with no animated indirect delta lights, so the retained list is empty and `any_active_for_descriptor_indices` (`any()` over no indices) is false, leaving the pass idle after copy-through.

## Sequencing

Single phase, single task. The change is one cohesive unit (struct state + method + predicate + dispatch rename + call-site plumbing + tests + doc) confined to `sh_compose.rs`, `renderer_shadow_passes.rs`, `renderer_render_frame.rs`, and `rendering_pipeline.md`; `sh_compose.rs` is well under the split-before-extend threshold.

## Acceptance criteria

- [ ] On a level with no animated indirect delta lights, the indirect SH compose pass dispatches once at level load and is skipped on every subsequent world frame while the frame mask is unchanged.
- [ ] On a level with an active animated indirect descriptor, the pass dispatches every world frame (pre-gate behavior preserved).
- [ ] When the last active animated indirect descriptor becomes inactive, the pass dispatches exactly once more (settling the atlas to base-only) and then idles.
- [ ] A change in the per-frame `LightTermMask` triggers a dispatch on the next world frame, including when the mask changed during a non-world frame (the mask compare stays dirty until an actual dispatch records it).
- [ ] The composed indirect atlas is identical to the pre-gate output on every frame; the gate never skips a dispatch that would have changed the atlas.
- [ ] The indirect `active` signal reuses `AnimatedLightBuffers::any_active_for_descriptor_indices` over the retained per-light descriptor-index list — no new activity-tracking mechanism is introduced.
- [ ] The `ShComposeResources` doc-comment and rendering_pipeline.md §7.1 step 5 describe the gate rather than unconditional dispatch, and both preserve the fact that the pass performs no per-cell/tile culling when it dispatches.

**Verification altitude.** AC 1–4 reduce to predicate unit tests (mirroring the direct sibling) plus the empty-indices assertion; the per-frame dispatch/skip sequence is argued from the predicate and the placement of the state updates, not executed against a dispatch counter — the same altitude at which the direct gate is verified. AC 5 is a reasoning gate: the output-input enumeration plus those predicate tests, with no golden-image comparison. AC 6 and AC 7 are review gates, not runnable tests.

## Rough sketch

Mirror `direct_sh_compose.rs` closely, minus the private mask uniform and debug-override machinery:

- New fields on `ShComposeResources`: `animation_descriptor_indices: Vec<u32>`, `pending_copy_through: bool`, `was_active: bool`, `last_composed_mask: LightTermMask`.
- New method `has_active_animated_descriptor` → `animation.any_active_for_descriptor_indices(&self.animation_descriptor_indices)`.
- New free fn `indirect_compose_should_dispatch(...) -> bool` (unit-tested, like `direct_compose_should_dispatch`).
- `dispatch` → `dispatch_if_needed(&mut self, ..., active, frame_light_term_mask, ...)`; early-return on the predicate, else encode existing pass and record `pending_copy_through/was_active/last_composed_mask`.
- Call site: `renderer_shadow_passes.rs` `record_pre_scene_compute` gains a `frame_light_term_mask` param; caller in `renderer_render_frame.rs` passes `self.frame_light_term_mask()`.
