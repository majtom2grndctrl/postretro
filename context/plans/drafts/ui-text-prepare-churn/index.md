# ui-text-prepare-churn

Brief · compact · reads: `context/lib/ui.md` §1, §3, §5 · `context/lib/rendering_pipeline.md` §7.8, §12 · `context/lib/development_guide.md` §1.4, §2.1, §6.4 · `context/lib/testing_guide.md` §3 · read at b26d929bc · evidence: `research.md`

## Problem
Developer profiling on the compatibility-floor Mac, through `drafts/render-submit-pass-overhead`. At `dcde8f292` every `queue_write_buffer` in the recorded `sample` profiles is glyphon's: the UI pass reshapes every text run and re-prepares every text span each frame, and each prepare writes its vertex buffer through the raw queue, which in wgpu 29 allocates one Metal staging buffer per write and frees it synchronously at the next submit. With the HUD idle that costs ≈ 0.7–0.85 ms of `render_submit` (the staging frees, the largest single bucket on campaign) and ≈ 0.4 ms of `rec_ui` (shaping plus the allocation), on both maps, for text that did not change. When done, a frame whose text spans are unchanged shapes nothing, prepares nothing and writes nothing for text. A change re-prepares only spans in its own layer, so a damage number spawning or a meter toggling never re-prepares the HUD. Text never goes stale or missing, and the per-frame prepare count is a timing row.

## Decisions
- **Change-gate glyphon's prepare per span, in the renderer's text half.** Each span slot keeps its renderer, its shaped buffers and a hash of what prepare read; an unchanged span is drawn from its retained vertices. glyphon's `render` reads only retained state, so a skipped prepare is legal (`research.md` §glyphon). This is `development_guide.md` §1.4's "writes driven by change" and the UI model's settled-frame rule (`ui.md` §3) carried to the GPU boundary; it stays inside the renderer per `index.md` §2.
- **Supersedes `done/ui-render-path-robustness-text-shaping` Task C.** Task C designed a per-node shaped-buffer cache keyed on the retained tree's taffy `NodeId`, scoped per stack layer, behind a landing threshold that today's profile crosses. The presentation layer has no persistent tree identity, and the write outweighs the reshape, so this gate keys on the renderer's prepared output per span, keeping Task C's per-layer scoping. The span is the cache unit: spans are one to three texts (`ui.md` §5).
- **Span identity and depth are scoped per layer.** A slot is (layer, span index within that layer), and each layer paints inside its own depth band. Today depth is spread over the whole frame's paint order and slots are positional across layers, so any span appearing in one layer shifts every other layer's keys. A layer's band and slots never move when another layer's spans change.
- **The key covers everything prepare reads:** content, family, device font size, colour, position, the span's depth within its layer's band, viewport, and a font-registration generation. Text scale and theme variants (`ui.md` §1, §2, decided not built) arrive as a changed font size or colour, so they need no hook of their own.
- **Trim never runs on a frame that skipped a span.** glyphon evicts any glyph outside its in-use set once the atlas is full, and only prepare refills that set, so a skipped span's glyphs could be overwritten under its retained vertices. A full-prepare frame — every span prepared, then trim — runs at a fixed cadence and at once when prepare reports the atlas full; between them the atlas grows at most by the glyphs touched since the last reclaim.
- **At promotion, `ui.md` §5 is restated.** "All spans prepare before the pass opens" becomes: changed spans prepare before the pass opens, and unchanged spans draw from retained vertices. "UI buffer writes stage in the renderer's per-frame upload batch" gains the exception that glyphon's vertex writes go direct, which is already true.
- **No glyphon or wgpu fork, no `StagingBelt`.** `done/per-frame-upload-batching` "Direct writes left" chose to leave glyphon's writes direct and grep-gates `StagingBelt`; wgpu 29 allocates a staging buffer per `write_buffer` and `write_buffer_with` alike. Change-gating removes the shaping cost a fork would keep; the residue from spans that change every frame is measured before any dependency door opens.
- **Non-goal: one renderer for all text.** A shape between text runs must start a new prepared span so source-over order holds (`ui.md` §5).
- **Non-goal: the quad, ring and uniform writes, and egui.** Those already stage in the frame batch; egui is `dev-tools` only.
- **One count row, `ui_text_prepares`, under `rec_ui`.** Count-kind, renderer-owned, in every build behind the timing env var (`development_guide.md` §6.4), zero present when the UI pass ran and prepared nothing (`rendering_pipeline.md` §12). The build also exposes prepare, skip and trim tallies to tests.
- **Proof is headless goldens plus windowed timing.** Capture binds no UI (`rendering_pipeline.md` §7.8), so pixel rows run through the renderer's headless GPU harness and self-skip without an adapter (`testing_guide.md` §3); timing rows follow `done/release-indirect-validation`'s conditions.

## Acceptance
### Automated
- [ ] Two consecutive frames with identical UI text: the second prepares no span, issues no text write and no trim, and its pixels equal the first frame's.
- [ ] One span's content changes on a frame while the others hold: that frame prepares exactly that span; the next frame prepares none; the changed text shows and the others are unchanged.
- [ ] A span whose text changes every frame beside a static span, over more frames than the reclaim cadence: every frame prepares one span except full-prepare frames, which occur at least once per cadence and are the only frames that trim; the static span's pixels never change.
- [ ] With the atlas held at its initial size by a device limit, a span cycling through more distinct glyphs than the atlas holds forces a full-prepare frame when the atlas reports full; that frame and the next still draw every span, and the static span's pixels are unchanged.
- [ ] Zero text: no span is prepared, written or trimmed, and the layer still clears.
- [ ] A viewport change re-prepares every span on that frame and none on the next, with text at the new positions.
- [ ] A font-size change, a colour change, a shape inserted before a span, and a font registration each re-prepare the affected spans on that frame and none on the next.
- [ ] A span appearing or disappearing in one layer, such as a damage number spawning or a HUD meter toggling, re-prepares no span of any other layer on that frame. A layer's own spans after the change re-prepare only when their key changed.
- [ ] A layer popped from the modal stack or replaced by a level change leaves no stale text: a span slot reused by different text re-prepares, and a slot beyond the current span count draws nothing.
- [ ] With CPU timing on, each counted frame's log line and the Performance tab report the frame's prepare count under the UI stage, zero on a settled frame and never absent when the UI pass ran; with timing off nothing is counted.
- [ ] The direct-write drift guard and the once-per-submit prepare guard still pass.
### Manual
- [ ] Mac release, same binary before and after, both maps, spawn pose, window in front, eight 120-frame windows with the median of the final five, idle `ioreg` and thermal snapshots per run (`rendering_pipeline.md` §12): `rec_ui`, `render_submit` and `ui_text_prepares` medians, with the prepare count near zero at rest.
- [ ] A post-change `sample` profile on each map reports the `triage_submissions` and `queue_write_buffer` shares of `render_submit`, with text writes gone at rest.
- [ ] A combat window with damage numbers on screen reports the prepare-count residue per frame, split into spans whose own content changed and spans re-prepared by a structural shift in their layer.
- [ ] Visual: no stale or missing text through HUD value changes, pause and frontend menus opening and closing, a window resize, a level change, and damage numbers in combat.

## Path
Non-binding.
- Seams: `UiTextRenderer::shape_text` and `prepare_text_batches` (`render/ui/text.rs`) become a per-slot `shape + prepare if changed`; `UiTextRenderer::trim` carries the full-prepare policy; `register_font` bumps the font generation; `UiPass::encode` (`render/ui/mod.rs`) calls the gate and reports the count; `RenderStage` gains a count variant under `Ui`, precedent `MeshPoseSamples`.
- Shape: hash of the span's `UiText` fields, depths and viewport, kept beside each `TextRenderer`. Strongest rival: an engine-owned glyph path. cosmic-text and swash still rasterize, but into an engine atlas, and glyphs become ordinary UI instances in the upload batch. That removes the direct write on every backend, changing spans included, with no fork, at the cost of owning atlas packing, eviction and growth (`research.md` §Rivals).
- First slice: the gate, the tallies, and the static-label-beside-a-counter golden with the atlas pinned to 256. That falsifies the riskiest assumption, that skipped spans survive atlas eviction and reclaim.
- Multi-frame goldens follow `multi_layer_text_golden_test.rs` with `mark_submitted` between frames; the 256 limit goes through a harness variant that accepts limits.
- `render/ui/mod.rs` is past the §2.1 size guidance; move the pipeline constructors out before `encode` grows.

## Open questions
- Reclaim cadence: a frame count, and its value — **delegated** (recommendation: a named constant near one timing window, 120 frames).
- If the combat residue still leaves a measurable staging-free share: fork glyphon onto the upload batch, or own the glyph path? — owner: Dan — after measurement; recommendation: own the glyph path over a fork, and neither until the residue is measured.
- Whether skipped-span and trim tallies become timing rows too, or stay test-only — **delegated**.
