# Research — UI render-path robustness + text-shaping efficiency

Design notes and decisions for Task C. The spec (`index.md`) carries the
observable acceptance criteria; this file carries the cache design and the
landing threshold. No cache rewrite lands in this plan — this is design-for-cost,
per development_guide §1.4 (no speculative optimization without a measured
bottleneck).

## Per-node shaped-buffer cache

### Today's double-shape

Text is shaped roughly twice per text node per frame:

- **Measure seam.** `measure_run` (text.rs) builds a fresh cosmic-text
  `TextBuffer` and shapes the run to return `(width, height)` for the taffy
  measure callback. One buffer per measured node, discarded after measuring.
- **Draw shape.** `UiTextRenderer::shape_text` builds a fresh `TextBuffer` per
  `UiText` and shapes it again before `prepare_text` uploads it. The returned
  `Vec<TextBuffer>` lives only for the frame.

Neither buffer survives the frame. cosmic-text caches shape/layout per line and
short-circuits clean lines, so the idiomatic pattern is a long-lived `Buffer`
updated with `set_text` only on content change. Postretro keeps neither, so a
settled frame re-shapes every label twice from scratch.

### Where per-node buffers live

Candidate: alongside the node in the retained tree, so buffer identity and
lifetime track the node. The retained gameplay tree (`UiPass::gameplay_trees`,
one `RetainedGameplayTree` per stack layer) already retains the taffy tree and
per-node `NodeContext` across frames; a cached `TextBuffer` hangs off the text
node's context (next to `NodeContext::Text { last_resolved, .. }`). A structural
rebuild (descriptor change or theme bump) drops the retained tree and its
buffers with it — the existing rebuild path is the coarse invalidation, and
in-flight buffers are discarded the same way display/tween state already is.

Trees are retained per stack layer, so buffer storage is naturally per-layer:
two layers' nodes never share a buffer store.

### Invalidation triggers

A cached buffer is reusable only while the shaped output it holds is still
correct. Re-shape (via `set_text` or rebuild) on any of:

| Trigger | Signal already present | Why it invalidates |
|---|---|---|
| Content change | `last_resolved` on `NodeContext::Text` — the diff records the last resolved bound string; a tween writes the rounded/formatted display string here too | The shaped glyph run changes when the string changes |
| Theme change | `theme_generation` (a `u64` on `RetainedGameplayTree`); `layout_gameplay_tree` rebuilds the retained tree when it differs | Font family / size resolve from the theme; a reshape is mandatory, and the rebuild already drops the buffer |
| Device-scale / viewport change | viewport `[u32; 2]` passed into `layout_gameplay_tree` each frame; `font_size` on `UiText` is device-scaled by the caller | Glyph metrics are in device pixels; a backbuffer-resolution change in the fixed 1280×720 logical space rescales every font_size, so cached device-pixel shaping is stale |

The first two already drive existing invalidation (content relayout, theme
rebuild). Device-scale is the one trigger with no existing per-node hook: a
backbuffer-resolution change is the device-scale trigger — when the device size
fed to the draw build differs from the size a buffer was shaped at, that buffer
re-shapes. Track the shaped-at device size per buffer (or per layer) and compare
on build.

### Cache key and namespacing

Key = retained-tree node identity = the taffy `NodeId` of the text node. The
`NodeId` is stable across frames for a retained tree (the tree is reused; only
bound values diff), so it identifies the same node frame-over-frame — the right
identity for a per-node buffer.

`NodeId` is unique only within one taffy tree, and each stack layer has its own
retained tree (its own taffy tree). Two layers' same-position nodes can therefore
carry the same raw `NodeId`. Namespace the key per stack layer — store buffers in
the per-layer retained tree (so the store is implicitly layer-scoped), or key on
`(layer_index, NodeId)` if buffers ever live in a shared side-table. Per-layer
storage is preferred: it makes the namespacing structural rather than a key
discipline that a later edit could forget, and it matches where the retained
tree already lives.

### Unifying the measure seam and the draw shape

The double-shape collapses when `measure_run` and `shape_text` read one buffer
per node instead of each building a throwaway. With a per-node buffer:

- The measure callback shapes (or reuses) the node's buffer and reads its
  measured size — no fresh `TextBuffer::new` per measure.
- The draw build reuses the same already-shaped buffer for `prepare_text` —
  `shape_text` stops building a fresh `Vec<TextBuffer>`.

A settled frame (no content/theme/scale change) then shapes zero times: the
measure seam and the draw share the clean cached buffer, and cosmic-text's
clean-line short-circuit keeps even a forced re-measure cheap. This is the unify
target; the seam is the same string flowing to both consumers, so one buffer
per node is the natural shared home.

One reconciliation the unify design must handle: the measure seam shapes at logical-reference px (the caller passes `measure_run` the un-device-scaled size — see text.rs), while the draw seam shapes `UiText` at `font_size * scale` (device px). A single shared buffer cannot serve both at one size unmodified — either shape at device size and divide back for the measure result, or treat a device-scale change as a re-shape trigger for the measure buffer too. This strengthens the device-scale invalidation argument above.

### Landing threshold

Land the rewrite only when one of these crosses — otherwise defer (design +
optional counter only):

- **Text-node count.** Steady-state text-node count across all live layers
  crosses a stated bound (proposed: ~64 simultaneously-shaped text nodes). At
  current retro scale (a handful of HUD/menu labels) the double-shape is not a
  measured cost. **The ~64 bound is PROPOSED, not measured** — it is a placeholder
  large enough to clear today's handful of labels, not a profiled inflection
  point. A reviewer should either confirm it against a real shaping profile or
  replace it outright with a measured-frame-time-only gate (the second bullet
  below). Do not treat ~64 as load-bearing until it is backed by a measurement.
- **Measured shaping frame-time.** Per-frame shaping contribution becomes a
  visible slice under `POSTRETRO_GPU_TIMING` or a CPU profile — i.e. shaping
  shows up as a real bottleneck, not a suspected one.
- **G1 label multiplier.** `ui-layouts-to-script-authoring` (G1) ships
  script-authored UI that multiplies label count beyond the handful the demo
  HUD/menu carry today — script-defined screens can author many more text nodes
  than the hardcoded builders do.

Until one crosses, the deliverable is this note. The optional counter is
deferred too (see *Optional measurement gate* below for why).

### Cache-key / namespacing risks

The cache is a per-frame correctness hazard: a stale buffer draws last frame's
glyphs. Each risk below, and the design's mitigation:

| Risk | How it bites | Mitigation in this design |
|---|---|---|
| **NodeId reuse across rebuilds** | A structural rebuild builds a fresh taffy tree; taffy may hand a recycled `NodeId` to a different node, so a buffer keyed on the old id would attach to the wrong node | Buffers live *inside* the retained tree (per-node `NodeContext`), so a rebuild drops the whole tree and its buffers together — `layout_gameplay_tree`'s `needs_build` path replaces `RetainedGameplayTree` wholesale. A recycled id can never carry a buffer from the prior tree because no buffer survives the rebuild |
| **Stale buffer (content/theme/scale)** | A buffer kept across a content edit, theme bump, or backbuffer resize would render the old run | The three invalidation triggers above. Content keys on `last_resolved`; theme on `theme_generation` (forces rebuild, drops buffers); device-scale on the shaped-at device size compared per build. Any mismatch re-shapes via `set_text`/rebuild before the buffer is read |
| **Cross-layer collision** | The same raw `NodeId` in two stack layers' trees would alias one buffer | Per-layer storage. Buffers hang off each layer's own retained tree, so the store is structurally layer-scoped — two layers cannot reach each other's buffers without a shared side-table, which this design avoids. If a side-table is ever introduced, the key MUST be `(layer_index, NodeId)`, never the bare id |
| **NodeId reuse frame-over-frame (settled tree)** | None — this is the *intended* case | A settled tree reuses its taffy nodes and ids; that stability is precisely what makes the per-node buffer reusable. The hazard is only at rebuild boundaries, handled by the first row |

The unifying mitigation: buffer lifetime is tied to retained-tree lifetime, not
to an independent id table. Every invalidation that matters either already drops
the tree (theme, structural change) or is a per-node/per-layer freshness check
(content, device-scale). No buffer outlives the conditions that make it correct.

### No rewrite in this plan

This note is design-for-cost ahead of the threshold. **No cache lands here.**
`shape_text` and `measure_run` keep building per-frame throwaway buffers until a
landing-threshold trigger crosses. Per development_guide §1.4, a cache with no
measured bottleneck is a speculative optimization; the discipline is to specify
it now (so the future change is cheap and the threshold is observable) and
implement it only when warranted.

### Optional measurement gate — skipped, with rationale

The intended metric is the *per-frame total shaped-run count* — both seams,
since the double-shape is the whole point. `shape_text` is a method on
`UiTextRenderer` and could cheaply hold an `Option<ShapeStats>` cached at
construction under `POSTRETRO_GPU_TIMING=1` (the `PoseSampleStats` pattern in
`render/mesh_pass.rs` is the exact precedent: an `Option` constructed only under
the gate, a rate-limited `[Renderer]` log, zero steady-state cost beyond an
`Option` check). But `measure_run` is a free function taking only
`&mut FontSystem`, invoked from inside taffy's measure closure
(`build_draw_data_retained` → `compute_layout_with_measure`). Counting its
shapes would require threading new diagnostic state through the layout-measure
seam — touching the hot path in exactly the way the task forbids — and a counter
on `shape_text` alone would report half the shaping work, misleading the very
threshold it exists to observe.

So the counter is **skipped** here, deliberately. When it is added (with the
cache, or as a one-off to confirm the ~64 bound), it should follow the
`PoseSampleStats` shape: an `Option<ShapeStats>` on `UiTextRenderer` plus a
sibling accumulator threaded to the measure seam, both `Some` only under
`POSTRETRO_GPU_TIMING=1`, off by default, rate-limited log, no steady-state
behavior change.

### Code anchors — verified against source

Every identifier this note leans on was checked against the tree on
`claude/beautiful-tesla-mou0q3`. All exist as named; one path correction:

| Anchor | Location | Note |
|---|---|---|
| `NodeContext::Text { last_resolved, .. }` | `tree.rs` (`enum NodeContext`) | `last_resolved: Option<String>` present; the diff (`drive_text_binding`) writes the resolved/displayed string here. Content invalidation keys on this exactly as the note states |
| `measure_run` | `tree.rs` measure seam → `text.rs` free fn `measure_run(&mut FontSystem, content, font_size, family)` | Shapes a throwaway `TextBuffer` per measure; doc-comment confirms it measures in **logical-reference px** (un-device-scaled), validating the unify-reconciliation note |
| `shape_text` | `text.rs`, method on `UiTextRenderer` | Builds a fresh `Vec<TextBuffer>` per frame, shaping at **device-px** `font_size`. Confirms the device-px-vs-logical-px split the unify section must reconcile |
| `UiPass::gameplay_trees` | `mod.rs` (`Vec<RetainedGameplayTree>`) | One entry per modal-stack layer, indexed bottom→top — the per-layer storage the namespacing relies on |
| `RetainedGameplayTree.theme_generation` | `mod.rs` | `u64`; compared in `layout_gameplay_tree`'s `needs_build`. The theme-invalidation trigger as described |
| `layout_gameplay_tree` `needs_build` path | `mod.rs` | Rebuilds on `descriptor != *tree || theme_generation` mismatch, replacing the `RetainedGameplayTree` wholesale — the coarse invalidation the NodeId-reuse mitigation depends on |
| `UiTree.last_viewport` / `cached_draw_data` / `recompute_count` | `tree.rs` (`struct UiTree`) | `last_viewport: Option<[u32;2]>` already drives a per-tree viewport-change recompute; a per-node shaped-buffer cache adds its own shaped-at-device-size check on top (layout recompute alone does not re-shape a node's buffer) |
| `PoseSampleStats` | `mesh_pass.rs` | The exact `Option<_>`-under-`POSTRETRO_GPU_TIMING` precedent the optional counter would copy: rate-limited `[Renderer]` log, `Option`-check-only steady-state cost |

**Path correction to the plan's identifiers.** The plan lists these under
`render/ui/...`; the real crate paths are `crates/postretro/src/render/ui/tree.rs`,
`.../text.rs`, `.../mod.rs`, and `crates/postretro/src/render/mesh_pass.rs`. The
bare filenames used throughout this note (`tree.rs`, `text.rs`, `mod.rs`) are
correct as filenames. No named function/struct/field in the plan was missing or
misnamed — only the directory prefix drifted.
