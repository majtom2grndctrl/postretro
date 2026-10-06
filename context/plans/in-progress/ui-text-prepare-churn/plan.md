# ui-text-prepare-churn — plan of record

mode: compact
status: active
read at: 481ce6894

## Corrections
- Brief and research cite `glyphon 0.11.0`, `cosmic-text 0.18.2`, `wgpu 29.0.1` → #556 (`c495e49ea`) moved the lock to `glyphon 0.12.0`, `cosmic-text 0.19.0`, `wgpu`/`wgpu-core`/`wgpu-hal 30.0.1`. Re-read at the new versions:
  - glyphon 0.11→0.12 differs only in `Option<VertexBufferLayout>` (`cache.rs`) and a `max ≥ min` clamp on text-area bounds (`text_render.rs`). `TextRenderer::render` still reads only retained state. `prepare_with_depth_and_custom` still clears `glyph_vertices` and `queue.write_buffer`s the whole array. `TextAtlas::trim` still clears `glyphs_in_use`, and `InnerAtlas::try_allocate` still evicts LRU glyphs outside that set before `grow` or `AtlasFull`.
  - `wgpu-core 30.0.1` `Queue::write_buffer` still calls `StagingBuffer::new` per call (`device/queue.rs:662`), as does `create_staging_buffer`.
  - Every Decision premise holds. Read "wgpu 29" in Decisions as "wgpu 30". The grep-gate row checks that glyphon and wgpu lock from the crates.io registry with no `[patch]` or source replacement. It does not pin version numbers, so it survives the next upgrade.
- `cosmic-text 0.19` `Buffer::set_size`/`set_text` no longer take `&mut FontSystem` (already adapted in #556). Shaping still needs it in `shape_until_scroll`.
- The engine calls `prepare_with_depth`, not `_and_custom`. Same path (it forwards).
- `ui.md` §5 already carries the restatement as "decided, not yet built". Landing removes those markers. No new wording is needed for the grep gate.

## Owner decisions (review, 2026-10-05)
- **Idle reclaim held until a change.** With the cadence equal to the 120-frame timing window, every idle window held one reclaim, and M1 could not pass. A due cadence reclaim now waits until a span has prepared for a change, or a drawn slot has left, since the last trim. Eviction safety is unchanged: a settled UI's in-use set cannot grow. The brief's reclaim Decision was reworded to match.
- **A full re-prepare trims first.** Before this change glyphon trimmed every frame. Without that, a resize drag piled every frame's new device-scaled font size into the in-use set and grew the atlas, with a re-raster stall at each growth step. A frame where no span can keep its vertices (viewport or font change, every slot returning) is now a reclaim, at no extra prepare cost. A mutation without this trigger fills the pinned atlas at drag frame 20.
- **Debounce considered and rejected** (owner asked to consider it).
  - Debouncing the trim lets stale sizes pile up, which is exactly the atlas-fill this trigger prevents.
  - Debouncing the re-prepare holds stale text through a drag, which contradicts A6 and "text never goes stale".
  - The trim itself only clears a hash set. Re-rasterising each new size is inherent to resizing and was true before this change.

## Delegated answers
- Reclaim cadence — `TEXT_RECLAIM_CADENCE = 120` encodes with text, one CPU-timing window. It is held until a change; see Owner decisions.
- Band bound and division — `UI_DEPTH_BANDS = 32` layers × `UI_BAND_ORDERS = 16384` fixed order slots per layer. An item's depth is `painter_depth(layer * UI_BAND_ORDERS + order_in_layer, UI_DEPTH_BANDS * UI_BAND_ORDERS)`. A fixed per-item step, rather than one normalised by the layer's item count, keeps an appended shape from moving earlier spans in its layer. Each step is about 2⁻¹⁹ (roughly 32 Depth24 levels), so adjacent orders stay distinct in f32 and in the depth target. A frame with more than 32 layers, or a layer with more than 16384 paint items, takes the whole-frame painter-depth fallback. The item cap is the second trigger of the same fallback; the band division implies it.

## AC-to-proof

Adapter-gated tests live in `crates/renderer/src/render/ui/text_prepare_gate_test.rs` (new) unless noted. Each one self-skips with a printed reason.

| AC | Proof | Status |
|---|---|---|
| A1 identical frames: no prepare/write/trim, equal pixels | `identical_frames_prepare_nothing_and_match_pixels` (adds a settled run of 2× the cadence that prepares and trims nothing) | achievable as stated |
| A2 one span changes | `one_changed_span_prepares_only_itself_then_nothing` | achievable as stated |
| A3 changing span beside static past the cadence | `changing_span_reclaims_on_cadence_and_static_pixels_hold` | achievable as stated |
| A4 pinned atlas, new glyph per frame, ≥2 reclaims, ≥1 atlas-full | `pinned_atlas_new_glyph_counter_keeps_static_label` (a moving span prepares before the overflow; its per-slot prepare count of 2 proves the re-prepare); `resize_drag_trims_each_frame_and_never_fills_the_atlas` (owner trigger) | achievable as stated |
| A5 zero text; reclaim deferral (O9) | `zero_text_frame_prepares_nothing_and_defers_reclaim` (the rig reuses its target, so the clear check can fail) | achievable as stated |
| A6 viewport change | `viewport_change_reprepares_every_span_once` | achievable as stated |
| A7 font size/colour, shape insert, font registration | `appearance_change_prepares_only_its_span`, `shape_insert_prepares_only_its_layer`, `font_registration_prepares_every_span_once` | achievable as stated |
| A8 damage number in bottom layer; HUD meter in middle layer | `presentation_spawn_prepares_no_other_layer`, `middle_layer_meter_toggle_prepares_no_other_layer` | achievable as stated |
| A9 adjacent layers keep separate spans (O11) | `layer_boundary_ends_span` | achievable as stated |
| A10 pause menu open/close (O10) | `modal_push_prepares_only_pushed_layer_and_pop_prepares_nothing` | achievable as stated |
| A11 per-layer depth occlusion | existing `upper_layer_panel_occludes_lower_layer_text_without_losing_other_text` (now on banded depth) + `banded_depth_keeps_own_panel_under_own_text` | achievable as stated |
| A12 layer close/reopen re-prepares; correct after reclaim/atlas-full/viewport; slot beyond count draws nothing (O5, O6) | `returning_layer_reprepares_and_draws_correct_text`, `returning_layer_is_correct_after_atlas_full_while_away` | achievable as stated |
| A13 reclaim and atlas-full after lower-layer count change keep depths (O22) | `reclaim_after_lower_layer_change_keeps_occlusion` (asserts a reclaim), `atlas_full_after_lower_layer_change_keeps_occlusion` (asserts an atlas-full recovery); both check occlusion ink directly on every trim frame | achievable as stated |
| A14 layer count over band bound (O23) | `over_band_bound_falls_back_then_reprepares_once`; `band_bound_counts_items_per_layer_and_depth_follows_paint_order` (item-cap trigger, depth order across paint-stream and legacy layers) | achievable as stated |
| A15 failed prepare draws nothing, retries (O4) | `failed_prepare_draws_nothing_and_retries` (the span drew last frame and fails with a partial vertex count; mutation without the `key_valid` render check fails it) | achievable as stated |
| A16 tween settle / reduce-motion snap (O18) | `settled_tween_prepares_nothing`, `reduce_motion_snap_prepares_once_then_nothing` (retained gameplay path) | achievable as stated |
| A17 timing count under UI stage | `ui_frame_reports_text_spans_prepared_under_the_ui_stage` (real `record_ui_layer` path; folded sample sits under `rec_ui`, present at zero; settled frames count zero), `ui_text_prepare_count_is_absent_with_timing_off`, `ui_text_spans_prepared_counts_under_the_ui_stage`, `count_rows_show_average_and_max_and_stay_present_at_zero` (Performance tab); the log line uses the generic `StageKind::Count` window format | achievable as stated |
| A18 debug guard counts encodes (O20); drift guard passes | `second_encode_before_submit_trips_guard_even_without_prepare` (debug; matches the guard message) + `renderer_direct_uploads_and_submissions_have_lifecycle_owners`, `renderer_uses_only_the_shared_staging_pool` | achievable as stated |
| A19 grep gates | `glyphon_and_wgpu_come_from_the_registry_unpatched`, `ui_md_states_the_change_gated_text_contract` | achievable as stated |
| A20 adapter-gated rows ran, count reported | run the module on this machine's adapter; report ran/skipped from the test output | achievable as stated |
| M1 Mac release medians | owner, Mac. A settled window now counts zero (owner decision), so the row can pass as worded | manual |
| M2 post-change `sample` shares | owner, Mac | manual |
| M3 combat prepare-count residue | owner, Mac | manual |
| M4 visual: no stale/missing text | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 0 | Move UI pipeline constructors out of `render/ui/mod.rs` (structural only) | integrating executor | — | done: `render/ui/pipelines.rs`; mod.rs 1032→807; 12 UI tests pass on GTX 1660 Super (Vulkan) |
| 1 | Thin slice: per-slot gate in `UiTextRenderer` (exact key, retained shaped buffers, slot-local metadata), trim-first reclaim at cadence + atlas-full recovery, encode stats; harness with pinned limits; A1–A4 goldens | integrating executor | 0 | done: `text.rs` slot gate + `TEXT_RECLAIM_CADENCE`; `try_init_gpu_with_limits`; A1–A4 pass. Mutation: per-frame trim after prepares fails the pinned-atlas golden (static label evicted, frame 35) |
| 2 | Per-layer scoping: composition ends spans at layer boundaries and carries layer per span; banded depth for quads/rings/text with fallback; A8–A14 | integrating executor | 1 | done: `PaintSlot`, `LayerCursor`, `UI_DEPTH_BANDS`/`UI_BAND_ORDERS` in `composition.rs`; A8, A9, A11–A14 pass. Mutation: banding off fails the four layer-scope rows |
| 3 | Edge rows: font generation, zero text + reclaim deferral, viewport, prepare failure; A5–A7, A15 | integrating executor | 1 | done: A5–A7, A15 pass |
| 4 | Count stage `ui_text_spans_prepared` under `rec_ui`; guard at encode entry; A17, A18 | integrating executor | 1 | done: stage + `record_ui_layer` wiring + encode-entry guard; A17 via `renderer_tests/ui_text_timing.rs`; A18 passes |
| 5 | Retained-path rows: tween settle, pause push/pop; A10, A16 | integrating executor | 2 | done: A16 through `layout_gameplay_tree`; A10 at the encode seam with hand-built stack layers (the gate reads only the composition) |
| 6 | `ui.md` §5 restated as built; grep gates; adapter run report; A19, A20 | integrating executor | 1–5 | done: `ui.md` §5 and `rendering_pipeline.md` §12 updated; A19 passes; A20 reported at preflight |
