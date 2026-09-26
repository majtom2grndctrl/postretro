# perf-sh-compose-sampled-row-gating

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4 (Animated SH delta volumes, Cluster SH residency), §7.1 steps 4–5, §7.8 · `context/lib/testing_guide.md` §Resource bounds · read at 0218faacd (`feature/shadowmask-atlas-compress-at-rest`, merge to main pending) · evidence: `spike-findings.md`

## Problem

Developer-observed defect, confirmed by spike: on stress-warren-mini at 1 m SH spacing, streamed
animated SH compose is 90–95% of frame time (41.9 ms live vs 4.1 ms with compose suspended;
GTX 1660 SUPER). Cause: while any animated light is flagged active, every pass recomposes every
resident row every frame, one dispatch per contiguous row range. Residency covers most of the
map, and resident rows are fragmented, so a frame issues over a thousand small dispatches over
rows that are mostly not sampled. When this is done, compose cost follows the sampled rows that
carry animated contribution, not resident volume or row fragmentation. Every stored slot sampled
in a frame holds exactly what full-resident compose would have written.

## Decisions

- **Gather dispatch replaces range dispatch.** Each streamed pass composes an uploaded list of
  rows: one dispatch per pass, or one per chunk when the list exceeds a per-dispatch capacity.
  Why: dispatch overhead was the spike's largest lever, and any row selection over ranges
  fragments them (row scoping over ranges regressed 3 m). A list holds exactly the selected
  rows, which satisfies `plans/done/sh-probe-streaming--cluster-residency` AC10 more tightly
  than ranges and replaces that plan's per-range form. Constraint: no compose binding is added
  or renumbered. Indirect and animated direct already sit at the default per-stage ceiling of
  8 storage buffers, the legacy path shares the shaders, and AC10 also pins binding numbers and
  counts.
- **Per-pass triggers read only that pass's inputs.** Indirect fires while an animated indirect
  light is active, plus a one-frame tail. Animated direct fires while an animated direct light
  is active, plus a one-frame tail.
  Static direct fires only when the promotion weights it
  uploads change, plus a one-frame tail. Today it
  shares the direct trigger and fires while any weight is above zero, yet it reads no animation
  data; in the spike it was a third of all dispatches, all redundant. When a pass fires, it
  composes only gated rows carrying that pass's contribution. A light-term mask or dev-override
  change forces one full-resident frame in every pass; both change only under dev-tools.
- **Residency changes ignore the gate.** Rows touched by an install, a slot reuse or a partial
  eviction always compose, together with the writer row of any scaled node whose probes they
  touch. Promotion keeps meaning "composed", and a slot never promotes over a previous tenant's
  texels. A row left with no resident contributor leaves residency and is never dispatched.
- **Per-light change scoping is a follow-up.** Once dispatches collapse it adds little (see
  research.md), it needs a per-light row index plus change tracking for curve samples that have
  no dirty signal today, and the maps this engine targets animate most lights continuously. The
  trigger for revisiting: counters show gated rows composing on frames where nothing changed.
- **Sampled-row gate, at region grain.** Each frame the app sends the renderer the world
  regions consumers can sample: bounds of visible and fog-reachable cells, plus bounds of drawn
  movers (covering their interpolated transform), drawn skinned meshes (whole body) and
  first-person viewmodels. An empty fog-reachable list means every cell. The renderer maps
  regions to rows, dilated by the sampler footprint, and adds the node-origin writer row of any
  scaled node a sampled row reads. The probe→row and writer rules keep one copy. Cluster grain
  was measured and rejected (research.md).
- **Staleness.** A gated row that missed a compose it would have received under full-resident
  compose composes the first frame it is gated, before any consumer samples it. Animated direct
  follows static direct on any row static direct rewrote.
- **Invariant restated.** This replaces `plans/done/indirect-sh-compose-gate`'s whole-atlas
  byte-identity and its "full-grid write makes skipping safe" argument. New rule: every stored
  slot a consumer can sample in frame N equals what full-resident compose would write in
  frame N. Unsampled rows may lag. The gate is view-dependent, unlike the §4 warm set, so
  turning in place composes lagging rows as they come into view, bounded by gate size.
- **A dev force-full-resident switch** makes every streamed pass compose every resident row
  every frame, as today's whole-resident compose does, bypassing both the gate and the split
  triggers. It is the exactness reference and the undo path.
- **Future consumers.** Any new SH consumer (a picture-in-picture scope view, GPU particles,
  reflection probes) adds its sample regions before compose, or forces full-resident compose.
  It has the same shape as the animated-lightmap visibility rule in §7.1 step 4.
- **Placement.** Region collection is app-side, where visibility, fog reach and draw culls
  already run before compose is recorded. Row resolution, staleness and gather lists are
  renderer-side. Renderer owns GPU; regions are data.
- **Counters are the proof instrument.** Per pass, per frame: rows composed, dispatches, rows
  composed because they lagged, and resident rows still lagging, plus CPU planning time. They
  are exposed in streaming live diagnostics, the periodic log and the capture measurement
  report. Planning cost counts: the
  spike measured 2.7 ms/frame of CPU recording at 1 m.
- **Capture gains a measurement-mode time step.** Exactness needs it. At a frozen instant, a
  row the gate wrongly omits keeps its install-time value, which equals full compose, so byte
  identity can't catch the miss. A fixed step per sampled frame, still VM-free, amends §7.8's
  single-instant contract for measurement mode only. Cost measurement runs frozen, like the
  spike baseline, because the flag-based triggers still fire at a frozen instant.
  `E20--scripted-run-capture` still owns scripted ticks.
- **`context/lib/` records the built contract:** §4 "Sampled-row compose" and its diagnostics
  sentence, §7.1 step 5, and §7.8's measurement-mode time step now describe the implemented
  gather, gate, staleness, diagnostics, and capture behavior.
- **Non-goals:**
  - The legacy whole-load path keeps its full-grid row set (owner: streamed only). Because it
    shares the compose shaders, gather must keep it composing every affinity row.
  - Amortizing or rate-limiting compose across frames: it breaks exactness on sampled rows.
  - Widening residency targets or narrowing owner closure. The spike's 91% residency finding
    is handed to the streaming epic. A gated region that isn't resident keeps today's
    ambient-floor fallback.
  - Absolute frame-time targets (owner: relative; the spike emulations set expectations).
  - Other handed-off findings are listed in research.md.

## Acceptance

### Automated
Gather dispatch (GPU-free planner and upload tests):
- [ ] A fragmented row set composes in one dispatch per pass. A list of exactly the chunk
  capacity takes one dispatch; one more row takes two. Each chunk decodes to only its own rows
  from the pass's one upload, every listed row composes exactly once, and no dispatch exceeds
  the device's per-dimension workgroup limit. (pin O12)
- [ ] A pass with no rows encodes no dispatch and reports zero rows and dispatches.
- [ ] The list never contains a non-resident row or a row twice.
- [ ] The compose pipelines' per-stage storage-buffer count stays at or below the device limit
  the renderer requests, summed over every bind group of each streamed and legacy compose
  layout, from the same layout builders the pipelines use. A full chunk's list fits the uniform
  binding size the renderer requests.
- [ ] The legacy whole-load path's compose upload still decodes to every affinity row exactly
  once, through the shared shaders.

Row selection:
- [ ] Trigger fires: gated rows with that pass's contribution compose. Gated rows without it,
  and every ungated row, do not.
- [ ] No trigger and no lag: zero rows, except install, evict and slot-reuse rows.
- [ ] A promotion weight change is judged on the weights static direct actually uploads, after
  any cache-layer zeroing, not on the requested weights.
- [ ] An animated direct light is active and no promotion weight changes: static direct composes
  only install, evict, slot-reuse and lagging rows. A promotion weight changes: static direct
  composes its gated promotion rows, and animated direct composes every row static direct
  rewrote, the same frame, after it. (pin O5)
- [ ] The last active light deactivates: gated contributing rows compose once more, then stop.
  When that happens on a frame that records no compose (occluded window, no surface), the tail
  composes on the next frame that records compose. (pin O3)
- [ ] A mask or dev-override change composes every resident row in every pass that frame, static
  direct before animated direct.
- [ ] With force-full-resident on, every pass composes every resident row every frame, whatever
  the gate and the triggers say.

Gate and staleness:
- [ ] For any frame sequence that moves the regions, toggles lights, changes promotion weights,
  skips compose frames, and installs or evicts clusters: after each frame that records compose,
  no gated resident row lags in any pass.
- [ ] A lagging row re-enters the gate on a frame when no trigger fires: it composes that frame.
  A row that never lagged enters the gate: it does not compose.
- [ ] An animated light stays active while the gate holds no row the pass would compose: nothing
  dispatches. A contributing row that enters the gate on a later frame composes that frame.
  (pin O9)
- [ ] A light deactivates on the frame its row leaves the gate. On re-entry the row composes
  with the light off. (pin O2)
- [ ] A row whose promotion weights changed while outside the gate composes in both direct
  passes on re-entry, static before animated. (pin O5)
- [ ] The camera cuts (teleport, respawn) into a room whose rows lag. That frame's gate includes
  them, and they compose that frame, not the next. (pin O8)
- [ ] A row whose last contributor is evicted leaves residency and never dispatches. A partial
  eviction composes the row that frame, gated or not, and leaves it current. A new residency
  generation starts with no lagging rows. (pin O7)
- [ ] A cluster installs, or a slot is reused, while its rows lie outside the gate: those rows
  compose that frame anyway, before the cluster can promote. A row composed that way, with no
  trigger since, later enters the gate: it does not compose again. (pins O1, O13)
- [ ] An install or eviction touches probes inside a scaled node whose writer row belongs to
  another cluster: that writer row composes the same frame. (pin O14)
- [ ] A cluster evicted in this frame's drain has no row in this frame's list. A cluster promoted
  in this frame's drain, with its writer row outside the regions: a gated row that reads it
  gates that writer row this frame. (pin O11)
- [ ] A pass whose encode fails leaves its planned rows lagging and its install, evict and
  slot-reuse rows pending. The next frame that records compose composes them. (pin O10)
- [ ] A sampled position one footprint width outside a visible cell resolves to a gated row. A
  position beyond the footprint does not gate its row.
- [ ] A position inside a gated region whose scaled node's origin brick lies outside the region:
  that writer row is gated. The writer closure adds at most one row per sampled row.
- [ ] A skinned mesh whose body leaves its origin cell gates the body's rows. A mover drawn at an
  interpolated transform between two cells gates both. A viewmodel poking through a wall gates
  the rows it overlaps.
- [ ] An empty fog-reachable list gates every resident row. A non-empty list gates only its
  cells' rows, plus the other regions.
- [ ] Review check: the per-frame region input carries world bounds and has no row-index field.

Counters, capture and cost:
- [ ] Counters equal the planned row, dispatch and lag sets for each pass. The capture
  measurement report carries them and the CPU planning time.
- [ ] Capture measurement mode advances animation time by a fixed step per sampled frame, runs no
  script VM, and keeps pulse lights force-active. Default capture still renders one instant.
- [ ] In the exactness capture, frames at two different time steps differ, and compose writes a
  nonzero number of rows between them. An authored-curve light is in view; force-active pulse
  lights have no curve and would not change.
- [ ] After warm-up, planning reuses caller-owned scratch: its capacity stops growing across
  frames.

### Manual
- [x] Exactness (§7.8 capture, measurement mode): at the spike's pinned poses (spawn, animated
  room, floor), with animation time advancing, frame PNGs are byte-identical between the shipped
  path and force-full-resident, at several time steps.
- [x] Measurement (§Resource bounds): owner-corrected Windows A/B of the same baked 1 m
  `stress-warren-mini.prl` on main and this branch. The feature branch was materially smoother;
  a minor intermittent hitch was observed but accepted as not a demonstrated regression. The
  earlier 1 m/3 m comparison is secondary rather than the landing criterion.
- [x] Visual, live with script pulses: a pulsing room leaves view and fog reach, then returns,
  with no stale flash. Turning in place shows no pop. A light switched off returns its region to
  base. Movers, skinned meshes and the viewmodel straddling unseen cells light correctly. Fog
  looking into non-visible rooms keeps its ambient scatter. Mask toggles and streaming-boundary
  crossings with active lights show no seams.

## Path

- Seams: `ShResidencyState::dispatch_indirect_compose` and `direct_dispatch_rows` (the
  `force_resident` union), `coalesce_rows`, `build_dynamic_compose_grid_upload_for_ranges`,
  `StreamingIndirectCompose::dispatch`, `sh_compose.wgsl`'s `range_start + workgroup.x`
  indexing, `should_dispatch`, the shared direct trigger in `renderer_pre_scene.rs`, and
  `promote_completed`. Today the epoch and activity memory move only after a dispatch; lag
  tracking must advance even when the gate is empty (pin O9).
- Carrier candidate: the existing dynamic-offset grid record binding (binding 18) can hold a
  chunk header plus row ids, which keeps binding numbers and counts intact. Static direct binds
  6 storage buffers, the other two passes 8.
- Gather precedent: `AnimatedLightmapCompose::dispatch` filters `master_tiles` and uploads the
  list, laid out as a balanced 2D workgroup grid padded with skip records so counts past the
  per-dimension limit still dispatch; that is the chunking precedent. Storage-count guard
  precedent: `billboard_pipeline_vertex_storage_request_matches_bgl_definitions`.
- Per-row contribution per pass: the per-row ref tables in `row_refs.rs` (id 27 indirect;
  id 41 promotion vs id 35 base-only for static direct; id 45 animated direct).
- Gate inputs: `VisibleRenderPreparation::for_level`, `mover_visible_against_cell_bounds`,
  `mesh_visible`, `collect_sprite`, the viewmodel plan. Only `cam_vis` reaches compose today.
  Resolve rows after the drain (pin O11). Writer mapping sits beside
  `build_probe_indirection_words` and `affinity_row_for_dense`.
- Lag tracking: per-pass change generations with a remembered generation per row is one shape.
  Commit it only after the pass encodes (pin O10).
- Spike emulation code (uncommitted, `sh_streaming/spike.rs` in the spike worktree) shows lever
  wiring and counter placement. Reference only; do not merge it.
- Sequence: (1) counters, force-full-resident switch, and gather over today's whole-resident
  set: exact, the largest single win, measured against the baseline; (2) per-pass triggers, the
  region gate and staleness; measure. Each slice measures before the next one starts.
- Measure `sh_streaming/frame.rs` before extending it; split first if it is past ~800 lines.

## Open questions

- Footprint dilation width and whether it applies per region or per row — **delegated**: at
  least the 8-corner stencil plus the normal offset (about 1.1 probe spacings); report the value
  in the plan of record.
- Gather list carrier, encoding (row ids vs brick coordinates) and chunk capacity — **delegated**,
  within the no-new-binding constraint.
