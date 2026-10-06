# ui-text-prepare-churn — plan of record

mode: compact
status: active
read at: 481ce6894

## Corrections
- Brief and research cite `glyphon 0.11.0`, `cosmic-text 0.18.2`, `wgpu 29.0.1` → #556 (`c495e49ea`) moved the lock to `glyphon 0.12.0`, `cosmic-text 0.19.0`, `wgpu`/`wgpu-core`/`wgpu-hal 30.0.1`. Re-read at the new versions:
  - glyphon 0.11→0.12 differs only in `Option<VertexBufferLayout>` (`cache.rs`) and a `max ≥ min` clamp on text-area bounds (`text_render.rs`). `TextRenderer::render` still reads only retained state. `prepare_with_depth_and_custom` still clears `glyph_vertices` and `queue.write_buffer`s the whole array. `TextAtlas::trim` still clears `glyphs_in_use`, and `InnerAtlas::try_allocate` still evicts LRU glyphs outside that set before `grow` or `AtlasFull`.
  - `wgpu-core 30.0.1` `Queue::write_buffer` still calls `StagingBuffer::new` per call (`device/queue.rs:662`), as does `create_staging_buffer`.
  - Every Decision premise holds. Read "wgpu 29" in Decisions as "wgpu 30". The grep-gate row checks the current locked versions.
- `cosmic-text 0.19` `Buffer::set_size`/`set_text` no longer take `&mut FontSystem` (already adapted in #556). Shaping still needs it in `shape_until_scroll`.
- The engine calls `prepare_with_depth`, not `_and_custom`. Same path (it forwards).
- `ui.md` §5 already carries the restatement as "decided, not yet built". Landing removes those markers. No new wording is needed for the grep gate.

## Delegated answers
- Reclaim cadence — `TEXT_RECLAIM_CADENCE = 120` encodes with text: one CPU-timing window, so a settled window holds at most one reclaim.
- Band bound and division — `UI_DEPTH_BANDS = 32` layers × `UI_BAND_ORDERS = 16384` fixed order slots per layer. An item's depth is `painter_depth(layer * UI_BAND_ORDERS + order_in_layer, UI_DEPTH_BANDS * UI_BAND_ORDERS)`. A fixed per-item step, rather than one normalised by the layer's item count, keeps an appended shape from moving earlier spans in its layer. Step ≈ 2⁻¹⁹: 32 Depth24 levels, exact in f32 near 1.0. A frame with more than 32 layers, or a layer with more than 16384 paint items, takes the whole-frame painter-depth fallback. The item cap is the second trigger of the same fallback; the band division implies it.

## AC-to-proof

Adapter-gated tests live in `crates/renderer/src/render/ui/text_prepare_gate_test.rs` (new) unless noted. Each one self-skips with a printed reason.

| AC | Proof | Status |
|---|---|---|
| A1 identical frames: no prepare/write/trim, equal pixels | `identical_frames_prepare_nothing_and_match_pixels` | achievable as stated |
| A2 one span changes | `one_changed_span_prepares_only_itself_then_nothing` | achievable as stated |
| A3 changing span beside static past the cadence | `changing_span_reclaims_on_cadence_and_static_pixels_hold` | achievable as stated |
| A4 pinned atlas, new glyph per frame, ≥2 reclaims, ≥1 atlas-full | `pinned_atlas_new_glyph_counter_keeps_static_label` (harness `try_init_gpu_with_limits`) | achievable as stated |
| A5 zero text; reclaim deferral (O9) | `zero_text_frame_prepares_nothing_and_defers_reclaim` | achievable as stated |
| A6 viewport change | `viewport_change_reprepares_every_span_once` | achievable as stated |
| A7 font size/colour, shape insert, font registration | `appearance_change_prepares_only_its_span`, `shape_insert_prepares_only_its_layer`, `font_registration_prepares_every_span_once` | achievable as stated |
| A8 damage number in bottom layer; HUD meter in middle layer | `presentation_spawn_prepares_no_other_layer`, `middle_layer_meter_toggle_prepares_no_other_layer` | achievable as stated |
| A9 adjacent layers keep separate spans (O11) | `layer_boundary_ends_span` | achievable as stated |
| A10 pause menu open/close (O10) | `modal_push_prepares_only_pushed_layer_and_pop_prepares_nothing` | achievable as stated |
| A11 per-layer depth occlusion | existing `upper_layer_panel_occludes_lower_layer_text_without_losing_other_text` (now on banded depth) + `banded_depth_keeps_own_panel_under_own_text` | achievable as stated |
| A12 layer close/reopen re-prepares; correct after reclaim/atlas-full/viewport; slot beyond count draws nothing (O5, O6) | `returning_layer_reprepares_and_draws_correct_text` | achievable as stated |
| A13 reclaim and atlas-full after lower-layer count change keep depths (O22) | `reclaim_after_lower_layer_change_keeps_occlusion` | achievable as stated |
| A14 layer count over band bound (O23) | `over_band_bound_falls_back_then_reprepares_once` | achievable as stated |
| A15 failed prepare draws nothing, retries (O4) | `failed_prepare_draws_nothing_and_retries` (pinned atlas, glyph larger than the limit) | achievable as stated |
| A16 tween settle / reduce-motion snap (O18) | `settled_tween_prepares_nothing` (retained gameplay path) | achievable as stated |
| A17 timing count under UI stage | `RenderStage` unit tests + renderer frame test asserting `ui_text_spans_prepared` present at zero, zero-text present, reclaim counts each once, gate off counts nothing; existing log/panel formatters cover `StageKind::Count` | achievable as stated |
| A18 debug guard counts encodes (O20); drift guard passes | `second_encode_before_submit_trips_guard_even_without_prepare` (`#[should_panic]`, debug) + existing direct-write drift guard | achievable as stated |
| A19 grep gates | source test: `Cargo.lock` glyphon/wgpu from registry at locked versions, no `[patch]`/vendored copy; `ui.md` §5 wording | achievable as stated |
| A20 adapter-gated rows ran, count reported | run the module on this machine's adapter; report ran/skipped from the test output | achievable as stated |
| M1 Mac release medians | owner, Mac | manual |
| M2 post-change `sample` shares | owner, Mac | manual |
| M3 combat prepare-count residue | owner, Mac | manual |
| M4 visual: no stale/missing text | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 0 | Move UI pipeline constructors out of `render/ui/mod.rs` (structural only) | integrating executor | — | done: `render/ui/pipelines.rs`; mod.rs 1032→807; 12 UI tests pass on GTX 1660 Super (Vulkan) |
| 1 | Thin slice: per-slot gate in `UiTextRenderer` (exact key, retained shaped buffers, slot-local metadata), trim-first reclaim at cadence + atlas-full recovery, encode stats; harness with pinned limits; A1–A4 goldens | integrating executor | 0 | |
| 2 | Per-layer scoping: composition ends spans at layer boundaries and carries layer per span; banded depth for quads/rings/text with fallback; A8–A14 | integrating executor | 1 | |
| 3 | Edge rows: font generation, zero text + reclaim deferral, viewport, prepare failure; A5–A7, A15 | integrating executor | 1 | |
| 4 | Count stage `ui_text_spans_prepared` under `rec_ui`; guard at encode entry; A17, A18 | integrating executor | 1 | |
| 5 | Retained-path rows: tween settle, pause push/pop; A10, A16 | integrating executor | 2 | |
| 6 | `ui.md` §5 restated as built; grep gates; adapter run report; A19, A20 | integrating executor | 1–5 | |
