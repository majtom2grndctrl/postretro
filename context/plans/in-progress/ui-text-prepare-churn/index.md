# ui-text-prepare-churn

Brief · compact · reads: `context/lib/ui.md` §1, §3, §5 · `context/lib/rendering_pipeline.md` §7.8, §12 · `context/lib/development_guide.md` §1.4, §2.1, §6.4 · read at a2138f30c · evidence: `research.md`

## Problem
Developer profiling on the compatibility-floor Mac, through `drafts/render-submit-pass-overhead`. At `dcde8f292`, every `queue_write_buffer` in the recorded `sample` profiles is glyphon's. The UI pass reshapes every text run and re-prepares every text span each frame. Each prepare writes its vertex buffer through the raw queue. In wgpu 29 that allocates one Metal staging buffer per write and frees it synchronously at the next submit. With the HUD idle, that costs about 0.7–0.85 ms of `render_submit` (the staging frees, the largest single bucket on campaign) and about 0.4 ms of `rec_ui` (shaping plus allocation), on both maps, for text that did not change. When done:
- A frame whose text is unchanged shapes, prepares and writes nothing for text.
- A change re-prepares only spans in its own layer.
- Text never goes stale or missing.
- The per-frame prepare count is a timing row.

## Decisions
- **Gate glyphon's prepare per span, in the UI pass.** An unchanged span draws from its retained vertices; glyphon's render reads only retained state (`research.md` §glyphon at 0.11.0). This follows the "writes driven by change" rule (`development_guide.md` §1.4) and the settled-frame rule (`ui.md` §3), and it stays inside the renderer.
- **Supersedes `done/ui-render-path-robustness-text-shaping` Task C.** Today's profile crosses Task C's landing threshold. The vertex write outweighs the reshape, so this brief gates at prepare instead of caching shaped buffers per node. It keeps Task C's per-layer scoping.
- **Everything is scoped per layer.** A layer boundary ends a text span. A slot is (layer, span index within that layer). Text and shapes in each layer paint inside a depth band fixed by the layer's stack position, so no layer's keys move when another layer's spans change, or when a modal is pushed or popped above it. A slot's text index and depths are slot-local, never frame-wide. The modal stack has no cap, so a frame whose layer count exceeds the band bound falls back to whole-frame painter depth for every layer, and that frame re-prepares everything.
- **The key is everything prepare reads, and any key change reshapes:**
  - the whole text record, plus the extent it is shaped and clipped against (today the viewport);
  - its depth within the layer's band;
  - the viewport;
  - a font-registration generation.

  Text scale reaches the key as a size, or as a future wrap width carried on the text record. Theme variants reach it as a colour.
- **A slot that an encode does not draw forgets its key.** When it returns, it prepares again, even with identical text. A span whose prepare fails draws nothing and prepares again on the next frame.
- **Reclaim is trim first, then every live span prepared, in one prepare phase.** It runs at a fixed cadence once a span changed or left since the last trim, at once when glyphon reports the atlas full, and on a frame where every live span prepares anyway; no other frame trims. A settled UI never reclaims. *(Owner, 2026-10-05, at review.)* Glyphon silently evicts glyphs outside its in-use set before it grows or reports full, and trim empties that set. Trimming first keeps every live span's glyphs protected until the next reclaim.
- **One count, under the UI stage: spans prepared this frame.** It is present at zero whenever the UI pass ran, in every build, behind the timing env var (`development_guide.md` §6.4, `rendering_pipeline.md` §12).
- **At promotion, `ui.md` §5 is restated.** Changed spans prepare before the pass opens, and unchanged spans draw from retained vertices. A layer boundary ends a span. Glyphon's vertex writes go direct, an exception to the frame upload batch.
- **No glyphon or wgpu fork, and no `StagingBelt`.**
  - `done/per-frame-upload-batching` ("Direct writes left") chose to leave glyphon's writes direct.
  - wgpu 29 stages every `write_buffer` and `write_buffer_with` alike.
  - The residue from spans that change every frame is measured before any dependency door opens.
- **Non-goal: the quad, ring and uniform writes, and egui.** Those already stage in the frame batch, and egui is `dev-tools` only.

## Acceptance
### Automated
- [ ] Two consecutive frames with identical UI text: the second prepares no span, issues no text write and no trim, and its pixels equal the first frame's.
- [ ] One span's content changes on a frame while the others hold. That frame prepares exactly that span, and the next frame prepares none. The changed text shows, and the others are unchanged.
- [ ] A span whose text changes every frame, beside a static span, runs for more frames than the reclaim cadence.
  - Every frame prepares one span, except reclaim frames, which prepare every live span.
  - Reclaim frames occur at least once per cadence and are the only frames that trim.
  - The static span's pixels never change.
- [ ] With the atlas held at its initial size by a device limit, a span that gains a new glyph every frame runs beside a static span, across at least two reclaims and at least one atlas-full report.
  - The static span's pixels equal its first frame's on every frame of the run.
  - The atlas-full frame prepares every live span again, including those already prepared that frame, and a debug build does not trip the once-per-submit guard.
  - The run's target, depth target and atlas all fit inside the device limit, and the glyph sizes it names are device pixels after UI scaling (O1–O3).
- [ ] Zero text: no span is prepared, written or trimmed, and the layer still clears. A reclaim that falls due moves to the next frame with text (O9).
- [ ] A viewport change re-prepares every span on that frame and none on the next, with text at the new positions.
- [ ] A font-size or colour change prepares only that span. A shape inserted into a layer prepares only spans in that layer. A font registration prepares every span once. Each case prepares none on the next frame.
- [ ] A damage number spawning or despawning in the bottom layer prepares no span in any other layer, on that frame or the next. So does a HUD meter showing or hiding in a middle layer (O12, O13).
- [ ] Two adjacent layers with text on both sides of their boundary keep separate spans. A change in the lower layer prepares nothing in the upper one, and each draws its own text (O11).
- [ ] Opening a pause menu over a visible HUD prepares only the menu's spans on that frame. Closing it prepares nothing in the layers that stayed visible (O10).
- [ ] With depth assigned per layer, an opaque panel in an upper layer still hides a lower layer's text where they overlap. Each layer's text still draws over its own panel.
- [ ] A layer that closes and later reopens with identical text, such as the pause menu, prepares its spans again on reopening. Its text is correct even if a reclaim, an atlas-full recovery or a viewport change happened while it was closed (O5, O6). A slot beyond the current span count draws nothing.
- [ ] A reclaim, and an atlas-full recovery, on a frame after a lower layer's text count changed leave every unchanged span's text at its own depth: an upper layer's panel still hides lower text, and each layer's text still draws over its own panel.
- [ ] A frame whose layer count exceeds the band bound draws every layer in paint order with correct occlusion, and the next frame within the bound prepares every span once.
- [ ] A span whose prepare fails draws nothing that frame, and prepares again on the next frame though its text is unchanged (O4).
- [ ] Once a tween that moves text finishes, or reduce motion snaps it, the next frame prepares no span (O18).
- [ ] With CPU timing on, each UI frame's timing record carries the prepare count under the UI stage.
  - The count is present at zero on a frame that prepared nothing, including a zero-text frame.
  - A reclaim or atlas-full frame counts each live span once.
  - Each window's log line and the Performance tab show the count. With timing off, nothing is counted.
- [ ] In a debug build, two compositions encoded before one submit trip the once-per-submit guard, even when the first prepared no span (O20). The direct-write drift guard still passes.
- [ ] Grep gates:
  - The workspace still takes glyphon and wgpu from the registry at their locked versions, with no patch, fork or vendored copy.
  - `ui.md` §5 states that unchanged spans draw from retained vertices, that a layer boundary ends a span, and that glyphon's vertex writes go direct.
- [ ] The build runs every adapter-gated row above on a machine with an adapter and reports how many ran without skipping. A self-skip prints its reason and does not count as a pass.
### Manual
- [ ] Mac release, same binary before and after, both maps, spawn pose, under `research.md` §Measurement conditions to pin. The `rec_ui` and `render_submit` medians fall on both maps, and the prepare count is exactly zero across settled windows.
- [ ] A post-change `sample` profile on each map reports the `triage_submissions` and `queue_write_buffer` shares of `render_submit`, with text writes gone at rest.
- [ ] A combat window with damage numbers on screen reports the per-frame prepare-count residue.
- [ ] Visual: no stale or missing text through:
  - HUD value changes;
  - the pause and frontend menus opening, closing and reopening;
  - a window resize;
  - a level change;
  - damage numbers in combat.

## Path
Non-binding.
- Seams:
  - `UiTextRenderer::shape_text` and `prepare_text_batches` (`render/ui/text.rs`) become a per-slot "shape and prepare if changed".
  - `UiTextRenderer::trim` carries the reclaim.
  - `register_font` bumps the font generation.
  - `append_ordered_text_batch` (`render/ui/composition.rs`) ends spans at layer boundaries.
  - `painter_depth` (`render/ui/mod.rs`) becomes per-layer banding for quads, rings and text.
  - `UiPass::encode` calls the gate and adds the count.
  - `RenderStage` gains a count variant under `Ui`, after the precedent `MeshPoseSamples`.
- Each slot keeps its `TextRenderer`, shaped buffers and key. Today shaping stamps a frame-wide text index into `Attrs::metadata`, and the depth lookup reads it (`text.rs`). A reclaim re-prepares unchanged spans from retained buffers, so that index must become slot-local.
- The once-per-submit debug guard counts once per encode, at encode entry, not per prepare call (O20).
- `register_font` bumps the font generation on every call, because a `false` return can still add faces.
- Shape: rival is an engine-owned glyph path (`research.md` §Rivals).
- First slice: the gate, its tallies, and the static-label-beside-a-new-glyph-counter golden with the atlas pinned. That falsifies the riskiest assumption, that skipped spans survive eviction and reclaim.
- Harness:
  - Capture binds no UI, so pixel rows use the headless GPU harness (`gpu_test_harness.rs`), which self-skips without an adapter.
  - Multi-frame goldens follow `multi_layer_text_golden_test.rs`, with `mark_submitted` between frames.
  - The pinned limit needs a harness variant that accepts limits, plus a target sized to fit.
- `render/ui/mod.rs` is past the §2.1 size guidance. Move the pipeline constructors out before `encode` grows.

## Open questions
- Reclaim cadence: a frame count, and its value — **delegated** (recommendation: a named constant near one timing window).
- The band bound, as a named constant, and how bands divide the depth range — **delegated**.
- If the combat residue still leaves a measurable staging-free share, fork glyphon onto the upload batch, or own the glyph path? — owner: Dan — after measurement. Recommendation: own the glyph path over a fork, and neither until the residue is measured.
