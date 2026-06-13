# UI Render-Path Robustness + Text-Shaping Efficiency

## Goal

Harden the UI text/quad encode boundary so the modal-stack glyph-clobber class is unrepresentable, add a headless multi-layer golden as the safety net for that class, and lay the design groundwork (not the rewrite) for per-node shaped-text buffer caching. Follows the already-landed point-fix (commit `f27f1fc`) that composes all stack layers into one `encode`/`prepare` per frame.

This is forward-looking hardening + coverage + design-for-cost. It does **not** re-fix the landed clobber: per-frame glyph output is unchanged (see the behavior-unchanged AC); only the encode *type surface* changes.

## Context

M13 Goal F's modal stack put the on-screen keyboard over the pause menu. The pause-menu's bound text readout drew the keyboard layer's glyphs (the opener's "ENTER TEXT" label, reversed). Root cause: `render_frame_indirect` looped the UI `encode()` once per stack layer; each call ran glyphon `prepare()` against the shared `UiTextRenderer`, whose single internal vertex buffer is overwritten at offset 0; `queue.write_buffer` resolves on the queue timeline, so the last layer's glyphs won at offset 0 and lower layers drew wrong glyphs. The point-fix composes all layers into one `encode`/`prepare`, matching the splash path. A CPU regression already pins the distinct-node-content half (`tree.rs`, `pause_menu_readout_and_opener_resolve_distinct_non_aliasing_text`); the GPU half is currently live-verify-only.

The invariant the text path violated ("one `prepare`/vertex-buffer fill per surface composition") is documented in the quad path (`UiPass::encode`, the per-batch disjoint-region comment) and now in the composed-encode comment in `render_frame_indirect`, but it lives only in comments and a CI-invisible GPU layer.

## Scope

### In scope

- **A.** Reshape the UI encode boundary so a *composition* (all stack layers' quads + text) is the unit of encoding — a caller cannot loop `encode` per layer and reintroduce the clobber. Add a debug guard against a second glyphon `prepare` within a frame.
- **B.** A headless multi-layer text golden: render a 2-layer stacked UI to an offscreen target, read back, assert each layer keeps its own text. Self-skips when no GPU adapter is present. Modeled on `multi_batch_test` / `splash_golden_test`.
- **C.** *Design only*, plus a cheap measurement gate: specify correct per-node shaped-buffer caching with invalidation, the cache-key/namespacing risks, and the threshold that warrants landing it. No rewrite lands in this plan unless the threshold is already crossed.

### Out of scope

- Re-fixing the landed clobber (shipped in `f27f1fc`).
- Migrating Rust-hardcoded UI layouts to script/JSON authoring — that is the separate `ui-layouts-to-script-authoring` draft (G1). Cross-reference only.
- True grapheme-cluster backspace (currently a char-pop floor) — a small separate follow-up, noted under Related work, not specced here.
- Multiple `TextRenderer`s sharing one `Cache` for genuinely separate passes (glyphon's documented multi-pass pattern). The single-composition-encode design makes it unnecessary here; note as the escape hatch if a future pass truly needs an independent text draw.
- Switching to a growing-vertex/growing-index single-upload buffer model (egui-style) or a `StagingBelt`. Relevant only at C's eventual scale-up; not warranted now.
- Landing the C cache rewrite (gated; see Task C and Open questions).

## Acceptance criteria

- [ ] The UI encode entry point consumes a single composition value spanning all stack layers. The per-layer `UiBatch`/`UiText` assembly is reachable only through the composition constructor, so there is no `pub(crate)` per-layer encode a caller can invoke in a loop: reintroducing a per-layer encode loop fails to compile. (The debug guard in the next AC is a runtime backstop, not this structural guarantee.)
- [ ] A debug-build guard fires if glyphon `prepare` (inside the `prepare_text` wrapper) is reached more than once per encoded composition on the shared `UiTextRenderer`; the guard resets at `UiPass::encode` entry. Release builds carry no guard cost.
- [ ] The composed-encode invariant is documented at the type/signature, not only in a free comment, and the quad-path disjoint-region comment is cross-referenced as the sibling invariant.
- [ ] All existing UI tests (`multi_batch_test`, `splash_golden_test`, the `tree.rs` readout-aliasing regression, `theme_gate_test`, `demo_ui_gate_test`, `gameplay_ui_gate_test`) still pass unchanged in behavior.
- [ ] A headless test renders two stacked composition layers (two retained trees / per-layer draws, as the modal stack would, not two text runs in a single `prepare`) — a lower layer whose bound text readout shows string S0 and an upper layer showing a different string S1 — into an offscreen target, reads it back, and asserts the lower layer's text region renders S0's glyph coverage, not S1's. The test self-skips (prints a skip line, returns) when no GPU adapter is present and never fails CI for adapter absence.
- [ ] The golden's assertion discriminates the bug: a `#[cfg(test)]`-only helper that drives the two layers through separate `prepare` calls (replicating the pre-fix per-layer loop) clobbers and fails the assertion, while the single-composition path passes. Because Task A removes the loopable encode from the production surface, this clobbering path exists only as the test helper.
- [ ] A design note in this plan's `research.md` specifies the per-node shaped-buffer cache: where buffers live, the invalidation triggers (content / theme / device-scale, where a backbuffer-resolution change is the device-scale trigger in the fixed logical space), the node-identity cache key, per-stack-layer namespacing, and the measurement threshold that warrants landing it. No premature cache rewrite lands.
- [ ] If a cheap measurement is added (e.g. a per-frame shaped-run counter behind an existing diagnostic gate), it is documented and off by default; it does not change steady-state behavior.

## Tasks

### Task A: Composition-as-unit encode boundary

Reshape the UI encode API so the unit of encoding is the whole frame's composition — every stack layer's quad batches and text runs together — rather than one layer. Today `UiPass::encode(&mut self, …, batches: &[UiBatch], texts: &[UiText])` is composition-capable, but the discipline that all layers go in one call lives only in the caller (`render_frame_indirect`) and comments. Introduce a composition type (e.g. a `UiComposition` / `UiFrameDraw` owning the ordered per-layer batches + concatenated text in painter order) that the caller builds once and hands to a single encode entry point. The per-layer `UiBatch`/`UiText` assembly that currently lives inline in `render_frame_indirect` (the `layer_draws` → `batches`/`texts` fold) moves behind that type's constructor so the caller cannot interleave a second encode. Keep the existing `&[UiBatch]`/`&[UiText]` shape internal to the pass if convenient, but the *public* boundary must take the composition.

Add a debug guard against a second glyphon `prepare` per composition on the shared `UiTextRenderer`: a flag set in `prepare_text` (which wraps glyphon `prepare`), cleared at `UiPass::encode` entry — the single per-frame call site both the splash and gameplay paths funnel through. `UiTextRenderer` has no existing reset hook, so this reset is newly added at that entry. Feed the flag a `debug_assert!`. Per development_guide §3.6, gate any guard-only helper with `#[cfg(debug_assertions)]` so it compiles out in release. The splash path (single composition already) must satisfy the same guard.

Document the composed-encode invariant on the composition type and the encode signature; cross-reference the quad-path disjoint-region comment in `UiPass::encode` as the sibling "one fill per composition" rule the text path must obey.

### Task B: Headless multi-layer text golden

Add a headless offscreen golden (a new `#[cfg(test)] mod` sibling under `render/ui/`, registered in `mod.rs` like `multi_batch_test`) that builds a 2-layer stacked composition and verifies each layer keeps its own text. Reuse the `try_init_gpu` / `read_texture_rgba8` pollster + readback precedent from `multi_batch_test` (self-skip on no adapter; copy_texture_to_buffer with 256-byte row alignment; de-pad). Lower layer renders a bound (or literal) text run with string S0 at a known device-pixel position; upper layer renders a different string S1 at a non-overlapping position. Encode the whole composition through the Task A boundary in one call, read back, and assert the lower layer's text region carries non-background glyph coverage consistent with S0 and not S1.

Discrimination over exactness: AA glyph rasterization differs per backend, so assert structural coverage (text region differs from background; the two regions differ from each other; the lower region's coverage signature is not the upper string's), not a committed PNG — matching `splash_golden_test`'s structural-readback rationale. Pick S0/S1 with clearly different ink footprints (e.g. a short vs. long string, or disjoint glyph sets) so coverage-mass or per-column-ink assertions discriminate. State in the test header that this is the safety net for the multi-layer compositing path that `cargo test` otherwise can't see, and that it self-skips in adapter-less CI.

### Task C: Cached per-node shaped-text buffers — design + measurement gate

Design (do not land the rewrite) a per-node shaped-buffer cache and add a cheap measurement gate. Today `UiTextRenderer::shape_text` builds a fresh cosmic-text `TextBuffer` for every text node every frame and re-shapes it, and `measure_run` shapes the same content again in the taffy measure seam — text is shaped roughly twice per node per frame. cosmic-text caches shape/layout per line and short-circuits clean lines, so the idiomatic pattern is long-lived `Buffer`s updated with `set_text` only on content change. The retained tree already knows when a bound string changed (`last_resolved` on `NodeContext::Text`), and trees are retained per stack layer (`UiPass::gameplay_trees`).

Per development_guide §1.4: at current retro scale (a handful of HUD/menu labels) this is not a measured bottleneck, and the guide forbids speculative optimization. So the deliverable is a design note + a threshold, plus an optional cheap measurement, NOT a rewrite. The design must specify: (1) where per-node buffers live (candidate: alongside the node in the retained tree, so identity and lifetime track the node); (2) invalidation triggers — content change (already detected via `last_resolved`), theme change (font family / size — `theme_generation` already bumps the retained tree), device-scale/viewport change (font_size is device-scaled; a scale change must re-shape); (3) the cache key = retained-tree node identity (taffy `NodeId`), explicitly namespaced per stack layer so two layers' same-position nodes never collide; (4) how the measure seam and the draw shape could unify on one buffer to kill the double-shape; (5) the threshold that warrants landing — e.g. text-node count crossing a stated bound, a measured frame-time contribution from shaping under `POSTRETRO_GPU_TIMING`/a profile, or G1 shipping script-authored UI that multiplies label count.

Optional, cheap, off by default: a per-frame shaped-run counter (or a behind-existing-diagnostic-gate log) so the threshold is observable rather than guessed. Do not rewrite `shape_text` in this plan.

## Sequencing

**Phase 1 (sequential):** Task A — defines the composition boundary Task B encodes through; both other tasks touch the encode/shape path.
**Phase 2 (concurrent):** Task B, Task C — B builds on A's boundary; C is design + an isolated optional counter, independent of B. Different files, no shared contract.

## Rough sketch

- **A.** New `pub(crate)` composition type in `render/ui/mod.rs` (e.g. `UiComposition<'a>`) holding the ordered batches + concatenated text in bottom→top painter order (own vs. borrow per Open question A); its constructor takes the per-layer `UiDrawData` slice (+ the white bind group, image registry resolver) and performs the fold currently inline in `render_frame_indirect` (`render/mod.rs` ~5419–5515) — a cross-module move, since the new type lands in `render/ui/mod.rs` while the caller stays in `render/mod.rs`. `UiPass::encode` becomes `encode(&mut self, …, composition: &UiComposition)`; the internal quad/text loop is unchanged. Frame-`prepare`-guard: a `bool`/counter on `UiTextRenderer` set in `prepare_text`, asserted `< 2`, reset at `UiPass::encode` entry; gate the helper with `#[cfg(debug_assertions)]`. Splash path (`record_splash_ui`) builds a single-layer `UiComposition` so it shares the boundary and guard.
- **B.** New `render/ui/multi_layer_text_golden_test.rs` registered under `#[cfg(test)] mod` in `mod.rs`. Reuse `multi_batch_test`'s `try_init_gpu`, `Readback`, `read_texture_rgba8` shapes (extract into a shared `#[cfg(test)]` sibling per testing_guide §4 only if duplication is real; otherwise copy is acceptable for two test modules). Build two `UiDrawData` (or two `UiText` runs at distinct positions standing in for two layers), fold into one composition, encode into an offscreen `Rgba8UnormSrgb` target, read back, assert per-region coverage.
- **C.** Design note in `research.md` (decisions only; keep out of `index.md` per draft-plan §2). Reference: `NodeContext::Text { last_resolved, .. }` (tree.rs ~256), `measure_run`/`shape_text` (text.rs), `UiPass::gameplay_trees` (mod.rs), `theme_generation` gate (mod.rs `layout_gameplay_tree`).

## Boundary inventory

Not applicable — no Rust ↔ JS/Lua ↔ wire ↔ FGD boundary crossed. All identifiers are renderer-internal `pub(crate)`.

## Related work

- `ui-layouts-to-script-authoring` (G1): moving hardcoded layouts to script/JSON. C's threshold cites it as a label-count multiplier that would warrant landing the cache; otherwise independent.
- Grapheme-cluster backspace (currently char-pop floor): a small separate text-entry follow-up, not part of this render-path plan.

## Open questions

- **C landing gate.** Should this plan land the per-node cache now, or strictly defer behind the threshold? Brief frames it as design-for-cost / land-when-warranted; default here is defer + design + optional counter. Confirm the threshold metric (text-node count bound vs. measured shaping frame-time vs. G1 trigger) the reviewer wants pinned.
- **A composition ownership.** Should `UiComposition` borrow the per-layer `UiDrawData` (lifetime-tied to the frame's `layer_draws`, zero-copy) or own a flattened copy? Borrow keeps it zero-alloc and matches `UiBatch<'a>`; confirm no borrow-checker fight with the `&mut self.ui` encode call.
- **B layer modeling.** Two real retained gameplay trees through `layout_gameplay_tree`, or two hand-built `UiText` runs standing in for layers? The former exercises more of the real path (and the readout-binding seam); the latter is simpler and more stable. The brief's "2-tree (stacked-layer)" framing favors the former — confirm the cost is acceptable for a self-skipping golden.
- **prepare-guard reset site.** Resolved: the guard clears at `UiPass::encode` entry, the single per-frame call site both the splash and gameplay paths funnel through, so a multi-frame run never trips it spuriously.
