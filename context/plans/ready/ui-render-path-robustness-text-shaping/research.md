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
  measured cost.
- **Measured shaping frame-time.** Per-frame shaping contribution becomes a
  visible slice under `POSTRETRO_GPU_TIMING` or a CPU profile — i.e. shaping
  shows up as a real bottleneck, not a suspected one.
- **G1 label multiplier.** `ui-layouts-to-script-authoring` (G1) ships
  script-authored UI that multiplies label count beyond the handful the demo
  HUD/menu carry today — script-defined screens can author many more text nodes
  than the hardcoded builders do.

Until one crosses, the deliverable is this note plus the optional cheap counter.

### Optional measurement gate

A per-frame shaped-run counter (or a log behind an existing diagnostic gate) so
the threshold is observable rather than guessed. Off by default; it must not
change steady-state behavior. It counts how many runs `shape_text` /
`measure_run` actually shape per frame — the metric the count-bound and
frame-time thresholds above are read against. Do not rewrite `shape_text` to add
it; a counter increment at the existing shape call sites is enough.
