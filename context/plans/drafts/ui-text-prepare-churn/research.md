# ui-text-prepare-churn — research

Derivation behind `index.md`. Source read at `b26d929bc` (postretro-briefs). Third-party source at the locked versions: `glyphon 0.11.0`, `cosmic-text 0.18.2`, `wgpu-core 29.0.1`, `wgpu-hal 29.0.1` (`Cargo.lock`). Profiles and timing windows are the recorded `release-indirect-validation` runtime set (`measurements/release-indirect-validation/runtime`), taken 2026-10-01 at `dcde8f292`. Written 2026-10-04.

## Question

`drafts/render-submit-pass-overhead/research.md` §The non-pass bucket attributes ≈ 0.8 ms of `render_submit` plus ≈ 0.3 ms of `rec_ui` to glyphon's per-frame text writes, and its index files the fix as a sibling. Is the attribution right, can the renderer remove the cost without a dependency fork, and what must a cache key cover?

## What the profiles show

Release, no `dev-tools`, 10 s `sample` at 1 ms, in level, vsync on (~58 fps, so ~580 frames per profile). Main-thread samples, read from the `.sample.txt` files:

| Row | Campaign | Hallway |
|---|---:|---:|
| `wgpu_core … queue_write_buffer`, all callers | 139 | 127 |
| of which under `glyphon::TextRenderer::prepare_with_depth_and_custom` | 138 | 125 |
| of which inside `StagingBuffer::new` | 97 | 91 |
| `UiTextRenderer::shape_text` (cosmic-text `Buffer::new` + `set_text` + `shape_until_scroll`) | 123 | 79 |
| `UiPass::encode` inclusive | ≈ 280 | ≈ 225 |
| `UiPass::layout_gameplay_tree` inclusive | 11 | 15 |
| `LifetimeTracker::triage_submissions` inclusive (inside `render_submit`) | 404 | 403 |
| of which `ioAccelResourceFinalize` (Metal buffer free) | 359 | 352 |

Every `queue_write_buffer` sample in both profiles is glyphon's: the only caller line above each is `wgpu::Queue::write_buffer`, reached from `prepare_with_depth_and_custom`. No renderer-owned per-frame write creates a staging buffer any more (`done/per-frame-upload-batching` AC32). With indirect validation off (`done/release-indirect-validation`), glyphon is the sole per-frame staging creator, so the `triage_submissions` bucket is its frees: ≈ 0.7 ms per frame by sample share (404 / ~580 frames), ≈ 0.8 ms scaled to the stage median as the sibling did. Both figures say the same thing.

Timing windows at the same head (`*-timing-unset.log`, last five windows): `render_submit` campaign 3.41–3.53 ms, hallway 3.67–3.94 ms; `rec_ui` campaign 0.65–0.77 ms, hallway 0.47–0.51 ms. `rec_ui` wraps `record_ui_layer` (`RenderStage::Ui` in `renderer_render_frame.rs`): layout of every layer plus `UiPass::encode`. The samples put encode at about two thirds of it, split between shaping (≈ 0.2 ms campaign) and glyphon prepare (≈ 0.23 ms, mostly the staging allocation).

So the stake is ≈ 0.7–0.85 ms of `render_submit` and ≈ 0.4 ms of `rec_ui` on both maps, every frame, with the HUD idle.

## The engine's text path per frame

`record_ui_layer` (`renderer_ui_layer.rs`) lays out the presentation layer and every modal-stack layer into `UiDrawData`, folds them into one `UiComposition` (`composition.rs`), and calls `UiPass::encode` once. Inside `encode` (`render/ui/mod.rs`):

1. `UiTextRenderer::shape_text` builds a fresh cosmic-text `Buffer` per `UiText` — `Buffer::new`, `set_size` to the surface extent, `set_text` with `Attrs::metadata(i)`, `shape_until_scroll`. Nothing is retained between frames. `measure_run` in `postretro-ui` (`crates/ui/src/text.rs`) shapes again for layout, but only when the retained tree relays out.
2. `UiTextRenderer::prepare_text_batches` calls `Viewport::update`, grows `text_renderers` to the span count, then one `TextRenderer::prepare_with_depth` per span. A span is a run of consecutive text commands in the paint stream; a quad or ring between two texts starts a new span (`append_ordered_text_batch`). `queue.raw()` is handed in, so `UploadQueue`'s `direct_writes` counter never sees these writes.
3. The pass opens; spans record at their paint-stream positions through `render_batch`.
4. `trim` after the pass, every frame.

Span count on the dev HUD (`content/dev/scripts/hud.ts`): always-on trees `hud` (health text + bar), `hud.openSeats` (text, empty until a roster arrives), `hud.xp` (text), `hud.ammo` (weapon label + ammo + reserve texts), plus the reticle ring and the hidden reload meter. That is about four spans, three with glyphs, so about three vertex writes per idle frame, each a staging buffer. The presentation layer (damage numbers, `presentation_layout.rs`) adds spans whose positions move every frame while instances live. A pause or frontend menu adds its own.

## glyphon at 0.11.0 — what the API allows

Read in `~/.cargo/registry/src/*/glyphon-0.11.0/src`.

- `TextRenderer::prepare_with_depth_and_custom` (`text_render.rs`) clears `glyph_vertices`, walks every text area, and when any vertex exists either `queue.write_buffer`s the whole vertex array into the retained `vertex_buffer` or, if it outgrew it, creates a new mapped-at-creation buffer. There is no change detection and no path that skips the write. With no visible glyph it returns before writing — an empty span costs no staging buffer.
- `TextRenderer::render` reads only retained state: `glyph_vertices.len()`, `vertex_buffer`, the atlas bind group and the viewport bind group. **A renderer prepared on an earlier frame can be rendered again without `prepare`**, provided its glyphs are still at the same atlas texels. This is the lever.
- `Viewport::update` (`viewport.rs`) writes its uniform only when the resolution changes. The sibling's "plus the viewport per frame" is wrong; the viewport contributes nothing to the per-frame churn.
- `TextAtlas::trim` (`text_atlas.rs`) clears `glyphs_in_use` on both inner atlases. `glyphs_in_use` is refilled only by `prepare`. `InnerAtlas::try_allocate` evicts least-recently-used glyphs that are not in `glyphs_in_use` when the packer is full, before `grow` doubles the texture (up to `max_texture_dimension_2d`) and `prepare` returns `AtlasFull` only when growth is exhausted. The shader normalises UVs by `textureDimensions`, and `grow` re-uploads every cached glyph at its old position, so retained vertices survive growth. They do not survive eviction: a span that skipped `prepare` has no glyph in `glyphs_in_use` after a trim, so a later allocation can overwrite its atlas texels while its vertices still point at them. Any skip policy must keep trim away from frames that skipped a span.
- `Cache`, `TextAtlas` and `TextRenderer` fields are `pub(crate)`. Vendoring only `text_render.rs` is impossible; routing the vertex write through the renderer's batch means a fork of the whole crate.

## wgpu 29 — why each write is a Metal allocation and a free

`Queue::write_buffer` (`wgpu-core/src/device/queue.rs`) calls `StagingBuffer::new` per call: a hal buffer with `MAP_WRITE | COPY_SRC` and `TRANSIENT` memory flags, mapped, written, flushed, then `pending_writes.consume` keeps it as a `TempResource`. `write_buffer_with` goes through `create_staging_buffer` and `write_staging_buffer`: the same allocation per call. On Metal `create_buffer` is `newBufferWithLength:options:` with shared storage (`wgpu-hal/src/metal/device.rs`), a driver allocation each time. At the next `queue_submit`, `Device::maintain(Poll)` → `triage_submissions` drops the temp resources of completed submissions; `FlushedStagingBuffer::drop` calls `destroy_buffer`, which on this driver is a synchronous `ioAccelResourceFinalize`. That is the `render_submit` bucket. `wgpu::util::StagingBelt` would reuse buffers, but it needs the encoder on the caller's side, glyphon does not use it, and `done/per-frame-upload-batching` holds a grep gate against any `StagingBelt` in the renderer.

## Removing the cost without a fork

Change-gate `prepare` per span slot, in `UiTextRenderer`:

- Keep one slot per span index: its `TextRenderer`, its shaped cosmic-text buffers, and a hash of what `prepare` read.
- Each frame, hash the span's inputs. Equal → skip shaping and `prepare`; `render_batch` draws the retained vertices. Different → reshape the span's texts, `prepare`, store the hash.
- Trim only on a frame that prepared every span (a *full-prepare frame*). Force a full-prepare frame at a fixed cadence and immediately when any `prepare` returns `AtlasFull` (trim, then prepare every span again in the same frame). Between reclaims `glyphs_in_use` holds every glyph touched since the last one, so growth is bounded by that set, which for a HUD is digits and labels at a few sizes, times cosmic-text's four subpixel bins per axis.

On the idle HUD this removes every text write and every reshape. In combat the presentation spans still change every frame (world-anchored positions), so one or two writes remain; health and ammo spans change on damage and fire, not per frame. That residue is measured, not assumed.

Granularity is the span, not the text node. Spans are one to three texts; a per-node shaped-buffer cache would have to re-plumb `Attrs::metadata` (the global text index that the depth closure reads) and buy little. The sibling's "caching shaped cosmic-text `Buffer`s" is folded into the span cache.

## What the key must cover

Everything `prepare` reads, so a change in any of it re-prepares that span and nothing more:

| Input | Why it changes | Covered by |
|---|---|---|
| `content`, `family`, `font_size` | bound values, text scale (E23 U2 multiplies the device font size through the measure path, `ui.md` §1), font tokens | hashed per text |
| `color` | theme variant selection, high contrast (`ui.md` §2) | hashed per text |
| `position` | layout, presentation projection, resize | hashed per text, as bit patterns |
| painter depth | a shape inserted before the span, layer push or pop | hashed per text |
| viewport | resize, scale factor (`rendering_pipeline.md` §7.8) | hashed per span; also bounds clipping |
| font database | `register_font` at mod init or reload | a generation counter bumped by `register_font` |
| slot identity | a span index reused by different text after a level change or pop | the hash differs; a slot beyond the current span count is never rendered |

Text scale and theme variants are decided, not built (`ui.md` §1, §2). They need no hook here: both reach the composition as a different `font_size` or `color`, and the key already covers those.

## Proof surfaces

- CPU timing: a count-kind stage under `rec_ui`, precedent `RenderStage::MeshPoseSamples` (`cpu_stages.rs`, `StageKind::Count`, added through `add_count`). `stage-timing` reports a zero count as present, not absent (`frame.rs`), which fits §12's absent-versus-zero rule. The binary folds it by label with no edit.
- Headless GPU goldens: `gpu_test_harness.rs` (`try_init_gpu`, `read_texture_rgba8_staged`) and `multi_layer_text_golden_test.rs` already drive `UiPass::encode` over the retained gameplay path and read pixels back; a multi-frame test is the same shape with `mark_submitted` between frames. `try_init_gpu_with_features` requests `Limits::default()`; a test that pins `max_texture_dimension_2d` to the atlas's initial size (256) exhausts the atlas with a few dozen 48 px glyphs, which makes the atlas-full recovery and the eviction hazard testable without a 64 MB atlas.
- Capture has no UI (`renderer_capture.rs`: "This path has no UI"; `rendering_pipeline.md` §7.8 binds an empty layer), so capture measurement cannot see this work. Windowed `[CpuTiming]` windows and `sample` are the measurement.
- `done/per-frame-upload-batching`'s direct-write drift guard classifies renderer sites only; glyphon's internal writes were never a site. Skipping them adds no site.

## Rivals

| Shape | Removes | Rejected because |
|---|---|---|
| Vendor or patch glyphon so the vertex write rides `UploadQueue::write_buffer` | every staging buffer, changing spans included | a dependency fork, an owner door (`done/per-frame-upload-batching` "Direct writes left"); leaves the reshape and prepare CPU in place; change-gating is needed anyway and covers the idle case |
| One `TextRenderer` for all text | all but one write | breaks mixed paint order: a shape between text runs must start a new span (`ui.md` §5) |
| `write_buffer_with` or a `StagingBelt` | nothing | same per-call staging allocation in wgpu 29; `StagingBelt` is grep-gated |
| Per-node shaped-buffer cache | the reshape only | the write is the larger cost; metadata re-plumbing for little gain |
| Engine-owned glyph path: cosmic-text and swash rasterize into an engine atlas; glyphs are UI instances in the upload batch | every direct text write on every backend, changing spans included; no fork; the span concept | owning atlas packing, eviction and growth is too large for the idle-HUD stake; the better next step than a fork if the combat residue matters |
| Reuse the whole UI layer when nothing changed | all UI work on a settled frame | the reticle ring is bound to `player.spread` through a tween and changes most frames, so whole-layer reuse rarely fires; per-span granularity is why the gate works |

## Key scoping (direction-review finding)
- `painter_depth` in `render/ui/mod.rs` is `1 - (order + 1) / (order_count + 1)` over the whole frame's paint order.
- `renderer_ui_layer.rs` folds the presentation layer first, then the modal stack, into one composition, and span slots are positional across it.
- So a damage number spawning or despawning, or a HUD meter toggling, changes every span's depth or slot, and a global key would re-prepare everything in combat. Per-layer slots and per-layer depth bands confine a change to its own layer.
- `done/ui-render-path-robustness-text-shaping` Task C (`research.md` §Cache key and namespacing) reached the same per-layer scoping for its `NodeId` cache.

## Stale or wrong claims in the sibling research

- "plus the viewport per frame": `Viewport::update` is change-gated; it writes on resolution change only.
- "one write per text batch": true for spans with at least one visible glyph; an empty span (the dev HUD's seat line before a roster) writes nothing.
- The ≈ 0.8 ms free cost and ≈ 0.3 ms creation cost hold; sample shares give ≈ 0.7 ms and ≈ 0.4 ms (shaping included) with the same profiles.

## Measurement conditions to pin

As `done/release-indirect-validation` and the sibling: compatibility-floor Mac (Radeon Pro 5300M, Metal), plain release with symbols, no `dev-tools`, `POSTRETRO_CPU_TIMING=1`, window in front, spawn pose on `stress-warren-hallway-inspection.prl` and `campaign-test.prl`, warm shadow cache, eight 120-frame windows with the median of the final five, `ioreg -c IOAccelerator` idle snapshots before each run, CPU speed limit and GPU temperature recorded. Same binary before and after. A separate `sample` run after the change reports the `triage_submissions` and `queue_write_buffer` shares. A combat window (damage numbers on screen) reports the prepare count residue. Mac/Metal only; Windows is a handoff.
