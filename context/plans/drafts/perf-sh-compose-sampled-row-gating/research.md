# perf-sh-compose-sampled-row-gating — research

Read at 0218faacd (`feature/shadowmask-atlas-compress-at-rest`). Informs; does not decide.

## Originating analysis (not re-derived)

- stress-warren-mini has animated lights of two kinds: authored `brightness_curve`, and pulses
  from `content/dev/scripts/stress-warren.ts`.
- Only about 43% of rows carry any animated contribution.
- Stored probes: 214,768 at 1 m vs 10,229 at 3 m.
- Estimated compose traffic: ~200 MB/frame at 1 m vs ~20 MB at 3 m (~12–22 GB/s at 60 fps).
- Smaller contributors (superseded by the spike, below: dispatch count is the largest lever): many small dispatches per pass; about a quarter of 1 m bricks are
  coarsened, which pushes more pixels onto pricier sampling paths. Upload streaming explains
  hitches, not a steady low frame rate.

## Grounding pins

| Claim | Symbol | Confidence |
|---|---|---|
| Streamed force adds all resident rows when active ∨ was_active ∨ mask changed | `ShResidencyState::dispatch_indirect_compose`, `direct_dispatch_rows` (renderer `sh_streaming/frame.rs`) | high |
| One workgroup per affinity row; contiguous `range_start + workgroup.x` | `compose_main` (`sh_compose.wgsl`) | high |
| Rows with no CSR entries rewrite base (indirect), base × static bit (Pass A), copy of Pass A (Pass B) | `sh_compose.wgsl`, `direct_sh_compose.wgsl` | high |
| Pass A reads no animation data but reruns while direct is active or any promotion weight > 0 | `direct_has_active_animation`, `direct_sh_compose.wgsl` | high |
| Composed atlases are single and persistent, rebuilt from separate base textures | `StreamingGpuPools` (`total`, `direct_intermediate`, `direct_total`) | high |
| Install, evict, slot reuse and growth all route through the dirty-row sets; growth copies old contents | `install.rs`, `sparse_install.rs`, `ShResidencyState::evict`, `sh_streaming/gpu/growth.rs` | high |
| Promotion waits on the compose epoch | `promote_completed` (`required_*_epoch <= *_compose_epoch`) | high |
| Activity reads the descriptor `is_active` word, not the evaluated value; a zero-valued curve counts as active | `descriptor_indices_have_active` (`sh_volume.rs`) | high |
| The CPU holds no current per-light value; the shader evaluates curves | `animated_light_scale` (`animated_direct_sh_compose.wgsl`); `animated_promotion_window_max` is a lookahead max for eligibility only | high |
| Dev freeze stops both script time and the `time` uniform | `Renderer::set_freeze_time`, `update_per_frame_uniforms` | med-high |
| One grid upload per pass, one `set_bind_group` with a dynamic offset per range; no indirect or gather dispatch | `build_dynamic_compose_grid_upload_for_ranges`, `StreamingIndirectCompose::dispatch` | high |
| No per-pass row/dispatch counter; `dirty_affinity_rows` never reaches diagnostics or the capture report | `ShResidencySnapshot`, `ShStreamingLiveDiagnostics::record_renderer_snapshot` | high |
| Capture report has only whole-frame CPU completion | `CpuCompletionReport` (`capture/report.rs`) | high |

## Consumers and the gate

| Consumer | Current cull | Can sample outside visible cells |
|---|---|---|
| Forward world | BVH leaves by VisibleCells | yes: 8-corner blend with normal offset crosses cell edges |
| Kinematic movers | origin cell visible or AABB overlaps a visible cell (current-tick AABB; drawn interpolated) | yes |
| Skinned meshes | origin cell only | yes: the body extends past it |
| Billboards | per-particle cell | yes, through the 8-corner footprint |
| Fog | fog-reachable cells (wider than visible) | yes: anchor and lookahead positions |
| Shadow passes | — | no atlas binding (`sh_depth_moments` only) |
| Dev probe readback | — | copies the whole atlas |

Frame order (`App::window_event` RedrawRequested): visible prep (visible cells, fog-reachable
cells, reachable AABBs) → particles → streaming drain prep → mesh collect → mover collect →
`render_frame_indirect` (drain → indirect compose → direct compose) → fog cell mask. Every gate
input exists before compose is recorded.

Scaled L1/L2 nodes write only through the node-origin brick (`stored_slot_for_invocation`;
`build_probe_indirection_words` → `node_origin_index`). A sampled position can therefore read a
slot that another row writes. The renderer holds no per-cell bounds; the app side holds
`LevelWorld.cells[].bounds_*` and `PlannerTopology::cell_to_cluster`.

## Commitments touched

- `plans/done/indirect-sh-compose-gate`: whole-atlas byte-identity invariant; its safety
  argument rests on the full-grid write. The brief replaces both with the sampled-slot rule.
  Its Orderings row accepting redundant flat-curve dispatches was never refined.
- `plans/done/sh-probe-streaming--cluster-residency` AC10: streamed dispatch never covers
  unrelated rows or dense indices.
- `plans/done/animated-direct-sh-dynamic-receivers`: pass merging deferred until a profile
  shows dispatch count matters.
- `rendering_pipeline.md` §4: the laptop iGPU smoothness floor is scoped to adaptive spacing;
  §10's desktop floor is renderer-wide.
- Compose dispatch time has never been measured, because no adapter used so far supported
  timestamp queries (`plans/done/perf-animated-sh-light-culling/findings.md`).

## Not verified

- Whether fog-reachable ⊇ visible on every path (exterior camera, solid cell, no-portals
  fallback). The gate takes their union, so the answer doesn't change the design. On
  stress-warren-mini the two sets were equal at every spike pose.
- Why dispatches cost ~20 µs each (inferred: barriers between dispatches writing one atlas,
  plus idle GPU on tiny workgroup counts). The gather decision doesn't depend on the cause.
- Why a few visible clusters close over most installed clusters (inferred: coarsened L1/L2
  owner nodes spanning large volumes). The region gate's writer closure must not inherit it.
- Cost of the region gate's per-frame planning. The spike's cluster-gated set cost ~8 ms of
  CPU to build per frame; the cell-grain emulation's planning was within the 2.7 ms baseline.

## Orderings (`/review-brief`)

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| O1 | Install or slot reuse outside the gate | drain installs into a freed slot while the cluster's rows are outside the gate, then compose, then promote | install/reuse rows compose that frame regardless of the gate, so promotion still means composed. Each pass clears its whole dirty set after dispatch today, and promotion checks only the epoch, so a gate-filtered dirty set would promote a slot holding a freed tenant's texels |
| O2 | Tail meets gate exit | on one frame the light deactivates and its row leaves the gate | row goes stale; composes with the light off on re-entry |
| O3 | Tail across a skipped frame | light deactivates, the next frame records no compose (occluded, no surface), then a normal frame | tail composes once on the next recorded frame; skipped time counts as changed. Today's per-pass `*_was_active` updates only after a dispatch (`frame.rs`), which is why it survives a skipped frame; a per-light set advanced at drain would lose it |
| O4 | Freeze, then stale re-entry (holds by construction under the pass-level trigger; kept for the follow-up) | row goes stale, freeze on, camera moves the row into the gate | row composes that frame; rows that were never stale stay idle. A whole-pass "time unchanged → return" early-out at the top of the pass would miss this |
| O5 | Pass A rewrite, Pass B follows | promotion weight changes on a row with no active id 45 light (in gate, or out of gate then re-entering) | Pass B composes the row on the same frame as Pass A, after it. Pass B reads Pass A's intermediate; the only existing test seeds the reverse direction under whole-resident force |
| O7 | Stale row leaves residency | a stale row loses its last contributor; separately, a new generation resets residency | row leaves residency and never dispatches (cluster-residency AC10); a partial eviction composes the row that frame (residency changes ignore the gate); `clear_session` must reset lag tracking with the other row sets |
| O8 | Camera cut | a teleport or respawn puts a stale room in view on frame N | frame N's gate includes it and it composes on frame N |
| O9 | Trigger with nothing to dispatch | on a frame that records compose, the trigger fires but the gate holds no row the pass would compose | lag tracking advances anyway; out-of-gate rows compose on re-entry. Today the epoch and the indirect pass's activity and mask memory move only after a dispatch, so tracking placed beside them never advances while the gate is empty |
| O10 | Planned, not encoded | the planner selects rows, then the pass's encode fails or is abandoned | remembered lag state and the install, evict and reuse sets change only after the pass encodes; the next frame that records compose composes them. Today dirty sets clear and the epoch advances only after a successful dispatch |
| O11 | Drain before resolution | this frame's drain evicts a cluster, or promotes one whose writer row lies outside the regions | rows resolve after the drain, against what consumers sample this frame: the evicted cluster's rows leave this frame's list; a gated row that now reads the promoted node gates its writer row this frame |
| O12 | Chunks in one frame | a pass's list splits into chunks, all recorded before one submit | each chunk reads only its own rows: records sit at distinct aligned offsets of one upload written before the pass. Rewriting one region per chunk makes every chunk read the last list |
| O13 | Install then entry | a row composes for install outside the gate; no trigger fires; it then enters the gate | it does not compose again; the install compose counts as a compose for lag tracking |
| O14 | Foreign patch in a scaled node | cluster A installs or evicts probes inside a scaled node whose origin-brick writer row belongs to cluster B | the writer row composes that frame. Its brick reads the corner probes' words for tile validity, and today install and evict dirty only the patched probes' own rows. Whether the bake ever splits a node across clusters is unverified; the rule is a cheap guard |

## Review pins (`/review-brief`)

| Claim | Symbol | Confidence |
|---|---|---|
| The live frame-rate readout is defined in `crates/sim`, not the renderer's frame timing | `FrameRateMeter` (`crates/sim/src/sim/frame_timing.rs`) | high |
| The renderer holds no light→row index. Payloads drop after install, and `InstalledCluster` keeps only (section, row). The CPU-resident `affinity_lights` CSR can supply it | `InstalledCluster`, `row_refs.rs`, `level-loader/src/sh_stream/projection.rs` | high |
| `RowSet::sources` holds table routing only, not light ids | `row_refs.rs` | high |
| Dev probe readback copies the legacy total atlas, which is a 1×1 dummy on streamed loads | `ShProbeReadback::encode_copy`, `sh_volume_resources.total_atlas_texture` | high |
| Empty dispatch plans produce one dummy dispatch with one workgroup | `sh_compose_dispatch.rs` | high |
| Compose reads the whole descriptor (period, phase, curve offsets, base color) and scripted curve samples, not only `is_active`. `setLightAnimation` rewrites both. Scripted samples upload with no dirty flag | `sh_compose.wgsl`, `animated_direct_sh_compose.wgsl`, `renderer_lighting.rs`, `sh_volume.rs` | high |
| Each pass clears its whole dirty set after dispatch. Promotion checks only the epoch, and the required epoch is the current one plus one. A freed tile keeps stale texels | `frame.rs`, `rows.rs`, `sh_streaming.rs` | high |
| The only counting allocator is the approved `unsafe` exception in `crates/sim` | `crates/sim/src/alloc_probe.rs`; `development_guide.md` | high |
| Static direct binds 6 storage buffers; indirect and animated direct bind 8, the default per-stage ceiling the renderer keeps | `direct_sh_compose` layout test; `renderer_init_resources.rs` | high |
| The direct passes share one trigger: promotion weight > 0, id 45 light active, or a dev override enabled with weight > 0. Nothing in it reacts to a value changing | `renderer_pre_scene.rs` direct trigger; `frame.rs` | high |
| The light-term mask changes only under dev-tools; dev overrides compile out without it | `renderer_light_terms.rs`, `renderer_pre_scene.rs` | high |
| Frozen-instant capture still composes every frame under the flag-based triggers (capture seeds pulse lights force-active). The spike note's "skip-when-unchanged composes nothing at a frozen time" refers to the deferred per-light design | `capture/prepared.rs`, `frame.rs` | high |
| Capture time is hard-coded to 0; §7.8 capture is VM-free and single-instant | `renderer_capture.rs`; `rendering_pipeline.md` §7.8 | high |
| Scaled base nodes never span a delta-bearing brick, so node-origin writer rows carry no animated or promotion contribution; the writer closure can't re-inflate the gate the way cluster ownership did | `rendering_pipeline.md` §4 Adaptive base-probe spacing | med |
| The spike's 213 gated rows are a lower bound: no writer closure, no mover/mesh/viewmodel regions, 1-spacing dilation vs the brief's ~1.1 floor | `spike-findings.md` Q3 | high |
| No dev switch forces full-resident compose today; full-resident is automatic while any light is active | `frame.rs` | high |
| The animated direct dev override solos one light; it never forces a light on | `animated_direct_sh_compose.wgsl` | high |
| Capture's force-active pulse descriptors carry no curve, and a zero-count curve evaluates to 1.0, so stepping time changes only authored-curve lights | `capture/setup.rs`, `curve_eval.wgsl` | high |
| The cache layer can zero promotion weights after the ramp; the uploaded weights, not the requested ones, drive static direct | `renderer_light_slots.rs` | high |
| Today a fully evicted row still dispatches in the pass that follows; the brief drops that (no slot to write, indirection already zeroed, reuse re-dirties) | `frame.rs` | med |

## Spike summary (full note: `spike-findings.md`)

GTX 1660 SUPER, Vulkan, release, stress-warren-mini baked at `--lightmap-density 0.16`. All five
honesty gates passed. Frame time in ms (capture = `cpu_completion` median at spawn; live = frame
interval at spawn, vsync off):

| Condition | Capture 1 m | Live 1 m | Capture 3 m | Live 3 m |
|---|---|---|---|---|
| Today (whole-resident, ranges) | 47.9 | 41.9 | 3.67 | 5.02 |
| Row-scoped, ranges | 30.3 | 26.0 | 4.72 | 5.72 |
| One merged range per pass, same rows | 14.8 | 12.9 | 3.33 | 4.65 |
| Scoped, one merged range | 12.6 | — | 3.05 | — |
| Scoped ∩ cell-grain gate, ranges | 7.2 | 5.9 | 2.85 | 4.37 |
| Compose suspended | 2.55 | 4.11 | 2.12 | 4.06 |

Rows / dispatches per pass at 1 m spawn: resident 8,127 / 530; scoped indirect 4,214 / 419,
static direct 0, animated direct 3,586 / 320; cluster-gated 3,733 / 421; cell-grain 213 / 27.
22 of 482 clusters installed yet covering 91% of the grid. 3 visible clusters close over 19.

Consequences for the brief:
- Gather dispatch moved from non-goal to first slice. Tolerant range merging was rejected:
  it overcomposes gap rows, a stale gap row isn't byte-identical, and AC10 forbids spanning
  non-resident rows, which leaves hundreds of dispatches at 1 m.
- Cluster-grain gate rejected; region grain kept.
- Per-light row scoping deferred after the second `/validate-plan` (Reshape, narrow): once
  dispatches collapse it adds ~15% (14.8 → 12.6 ms), and its change tracking has no dirty signal
  for curve samples. Row selection keeps today's pass-level trigger, restricted to gated
  contributing rows. Static direct still falls to promotion-bearing gated rows.

## Rejected alternatives

| Alternative | Why not |
|---|---|
| Row scoping plus counters alone (validate-plan's rival) | Measured: 30.3 ms at 1 m vs 7.2 ms with the gate; regresses 3 m without gather |
| Cluster-grain gate (review-brief rival) | Owner closure of 1–3 visible clusters covers 18–19 of 22 installed clusters; no row reduction |
| Gap-tolerant range merging | See above; gather is exact and simpler |
| Per-light change scoping (first draft of this brief) | Deferred: small win after gather, costly new state; follow-up if counters show gated rows composing with nothing changed |
| Gather plus contributing rows, no gate | Scoped + one merged range measured 12.6 ms at 1 m vs 7.2 ms with the gate over ranges; the gate is the remaining large lever after gather |
| Pre-brief spike then rewrite | Done: this spike |

## Measurement protocol

- Fixture: stress-warren-mini, baked by the owner at `--sh-probe-spacing 1.0` and `3.0` with
  `--lightmap-density 0.16` (lower densities fail on this map; see Handed off). Confirm PRL
  ids 49/50 are present.
- Pin: machine, GPU/driver, backend (Vulkan), release build, resolution (1280×720),
  streaming mode (`POSTRETRO_SH_STREAMING=sync-proof` for capture), and the spike's poses:
  spawn (16.26, 3.23, 65.02) yaw 0; animroom (41.71, 3.23, −20.50) yaw 0; floor = spawn at
  pitch −89.
- Capture at a frozen instant (matches the baseline): 120 warm-up + 600 sample frames,
  `cpu_completion` median/p95. Live: windowed spawn run, vsync off, 45 s, last 3 × 240-frame
  windows. 3 runs each, alternating conditions.
- Report per-pass rows, dispatches, lag counters, CPU planning time and frame time against the
  spike baseline table above.

## Handed off

| Finding | Owner |
|---|---|
| 22 of 482 clusters cover 91% of the 1 m grid; a few visible clusters close over most installed ones | `in-progress/sh-probe-streaming` (Open questions) |
| stress-warren-mini fails to bake animated lightmaps below `--lightmap-density 0.16` (atlas over budget; >65,535 dispatch tiles with no 2D fallback) | spawned task "Fix stress-warren-mini animated lightmap bake at default density" |
| Coarsened-brick sampling cost in forward | not this brief: the spike measured compose, not sampling, as the 1 m bottleneck; revisit if frame time stays high after this lands |
| Bake-side density and coarsening policy | the lighting-scale coarsenability line; this brief removes per-frame work at any density |

## Direction review notes (`/validate-plan`, Direction sound)

- Static capture renders one pose at animation time 0 with no script VM (`capture/prepared.rs`).
  Under the flag-based triggers it still composes every frame, so it measures cost; exactness
  needs the measurement-mode time step.
- An empty fog-reachable list means "draw every cell" (`render_preparation.rs`).
- Strongest rival: row scoping plus counters alone, with the gate decided later from counter
  data. The owner kept the gate unconditional, and the first slice records scoped vs
  projected-gated rows as evidence.
- Sticky part: once other code relies on the "unsampled rows may lag" wording in
  `context/lib/`, it is hard to take back.
