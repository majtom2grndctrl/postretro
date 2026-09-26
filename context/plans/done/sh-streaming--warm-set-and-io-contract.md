# SH Streaming — Warm Set, Ordered I/O, Install Budget, Diagnostics (contract)

Follow-up to the SH probe streaming epic (`context/plans/done/sh-probe-streaming/`).
Every track reads this file first. Background: `context/lib/rendering_pipeline.md`
§"Cluster SH residency"; `context/lib/build_pipeline.md` §PRL section IDs (ids 46, 49, 50).

## Goal

Make async SH cluster residency stable and disk-friendly. Prefetch follows where the
camera *is*, not where it looks, so turning around in a room causes no prefetch churn.
One ordered read issuer serves disk requests in file-offset order with nearby reads
coalesced, and a small decode pool keeps CPU parallelism. Installs per frame are bounded
by decoded bytes rather than cluster count, and one install no longer costs time
proportional to the whole map's residency state. Always-on counters make every run a
tuning measurement, surfaced as a periodic log line, a dev-tools panel tab, and the
capture report. Measurements guide tuning; none of them gate the capability.

## Decisions

1. **Warm set replaces the two-hop horizon.** The horizon becomes `visible ∪ warm`. The
   warm set is a function of the camera cell alone, so it is cached per camera cell and
   recomputed only when that cell changes. Consequence: frustum rotation cannot change
   prefetch; only visible-class targets move with the view.
2. **Warm walk = bounded shortest-path over the graded CellVisibility pairs.** Starting at
   the camera cell, a Dijkstra walk over id-46 coupled pairs (undirected, edge weight =
   stored fixed-point `distance`) settles cells in ascending path distance; path aperture
   is the minimum edge aperture along the chosen path. Each settled cell maps to its
   cluster; the walk stops when `WARM_SET_CLUSTERS` distinct clusters (the camera's own
   cluster counts) are collected or `WARM_WALK_MAX_SETTLED_CELLS` cells are settled.
   Consequence: the camera cell's own ≤ 32 stored partners rarely leave its cluster, so
   seeding from those pairs alone would shrink the set below today's horizon; the walk
   composes pairs to reach farther.
3. **The reachability component gate is never used on its own as a target set.** It
   usually spans the whole connected map.
4. **Fallback without usable id 46.** When the section is absent, or its cell count
   disagrees with the id-49 cell map, the warm set is the `PREFETCH_HOPS` (2) cluster-
   adjacency expansion from the *camera's cluster* (not the visible set), logged once per
   controller at `warn`. The camera cell is `None` (no level) → the warm set is empty.
5. **Warm ranking.** Each warm cluster carries `(distance_bucket, aperture, cluster_id)`
   from the first settled cell of that cluster: `distance_bucket = path_distance_fixed /
   1024` (whole metres), smaller first; then aperture, larger first; then cluster id.
   Authored `_stream_priority` still outranks distance: prefetch request order is
   `(class, Reverse(effective_priority), warm_rank, cluster_id)`. Under pressure, among
   equal class and priority, the **farthest** warm cluster yields first.
6. **Unchanged:** visible targets are mandatory; pins, owner closure, seam warm-up,
   hysteresis (2 s monotonic), failure/retry, suppression semantics, and the target
   class order `Visible, Pinned, SeamWarm, Prefetch, Hysteresis`.
7. **Ordered I/O: one issuer thread + decode pool.** One thread issues every positional
   read. It keeps a pending set, split into two tiers: **mandatory** requests (class
   `Visible` or `Pinned`, which includes their owner closure) and **optional** requests.
   It reads the mandatory tier first, then optional, each tier in ascending file offset.
   Consequence: offset order never lets prefetch delay a visible cluster. After every
   coalesced read the issuer takes newly submitted requests, so new mandatory work preempts
   remaining optional work.
8. **Coalescing.** Adjacent chunks in the chosen tier's offset order merge into one read
   when the gap between them is ≤ `COALESCE_MAX_GAP_BYTES` and the merged span is ≤
   `COALESCE_MAX_SPAN_BYTES`. Gap bytes are read and discarded. A single chunk larger than
   the span cap is read alone.
9. **Pre-read cancellation.** Each frame, after the target update and again after the
   drain batch's budget policy, the session publishes the controller's target set to a
   shared atomic bitset. The issuer checks it immediately
   before issuing a read; a request whose cluster is not targeted is not read and returns
   a `Cancelled` completion. Consequence: a deeper queue does not waste disk bandwidth on
   departed work.
10. **Permits 4 → 8.** `MAX_STREAM_PERMITS = 8`, so the issuer has a useful window to
    sort. A permit still covers a cluster from request until install, drop, or failure.
11. **Decode pool size** = `clamp(available_parallelism / 2, 1, 3)` threads.
12. **Install budget replaces `MAX_INSTALLS_PER_DRAIN`.** A drain admits ready clusters
    in the existing priority order while the running sum of decoded bytes stays ≤
    `MAX_INSTALL_DECODED_BYTES_PER_DRAIN`. The first eligible cluster is always admitted
    whatever its size. Selection **stops** at the first cluster that would exceed the
    budget; it does not skip ahead to smaller, lower-priority work. Applies to both the
    sync-proof and async batch paths.
13. **Install clones.** The renderer's install transaction must not copy residency state
    whose size scales with the map (today: 16 whole-structure `.clone()`s for rollback).
    The mechanism (undo journal, validate-then-commit, or other) is the renderer track's
    call, after it measures the current cost. Failure atomicity is preserved exactly.
14. **Diagnostics surfaces.** (a) Always-on counters, compiled without any feature.
    (b) Periodic `log::info!` line prefixed `[SH streaming]` at most every
    `DIAGNOSTICS_LOG_INTERVAL_SECONDS` of monotonic render time, only when a counter
    changed since the last line, reporting window deltas plus current gauges.
    (c) A **Streaming** tab in the dev-tools diagnostics panel. (d) The capture report's
    streaming lifecycle JSON gains the new fields.

## Invariants (no track may break)

| Name | Value / rule | Owner |
|---|---|---|
| `WARM_SET_CLUSTERS` | `8` (`usize`), includes the camera's own cluster | T2 |
| `WARM_WALK_MAX_SETTLED_CELLS` | `4096` (`usize`) | T2 |
| `PREFETCH_HOPS` | stays `2`, now used only by the fallback | T2 |
| `MAX_STREAM_PERMITS` | `8` (`usize`), in `sh_streaming/controller.rs` | T2 |
| `MAX_INSTALL_DECODED_BYTES_PER_DRAIN` | `8 * 1024 * 1024` (`u64`), in `controller.rs` | T2 |
| `MAX_INSTALLS_PER_DRAIN` | deleted; no remaining reference | T2 |
| `COALESCE_MAX_GAP_BYTES` | `256 * 1024` (`u64`) | T3 |
| `COALESCE_MAX_SPAN_BYTES` | `16 * 1024 * 1024` (`u64`) | T3 |
| `DIAGNOSTICS_LOG_INTERVAL_SECONDS` | `5.0` (`f64`), monotonic render seconds | T3 |
| Distance units | id-46 fixed point, 1024 units per metre; path sums in `u64` | T2 |
| Aperture along a path | minimum edge aperture (bottleneck), larger ranks first | T2 |
| Decoded bytes of a cluster | `prepared.chunk.bytes.len()`; the same value as the ready byte charge | T2 |
| File offsets | absolute byte offsets in the PRL file, from the id-50 index (never cluster id as a proxy) | T3 |
| Latency | measured with `std::time::Instant`; submit → encoded bytes in hand; reported in milliseconds (`f32`) | T3 |
| CPU install time | measured in the renderer around its per-drain install work; reported in microseconds (`u64`) | T1 |
| Counters | cumulative `u64` since controller/renderer-state creation, checked or saturating adds, never wrap | all |
| Layering | no new crate edges; `postretro` → `renderer`/`level-loader` only; `layering_invariants_hold` stays green | all |
| GPU | all wgpu calls remain in `renderer`; the panel receives only plain data | T1 |
| No `unsafe` | none, anywhere | all |
| Sampler | no new binding, shader branch, or per-fragment lookup | all |

### Pinned seams between tracks

- **`ShClusterRequest`** (app, `controller.rs`) gains `pub(crate) mandatory: bool`,
  true when the cluster's class at request time is `Visible` or `Pinned`. T2 sets it;
  T3 consumes it.
- **Controller API added by T2** (names exact):
  - `pub(crate) fn targets(&self) -> &BTreeSet<u32>`
  - `pub(crate) fn admit_cancelled_request(&mut self, request: ShClusterRequest) -> Result<(), ShResidencyControllerError>`:
    if identity matches and the state is `Queued`, release the permit, set `Absent`,
    and count one cancelled request, **whether or not the cluster is targeted now**.
    Otherwise it is a no-op. It never marks `Failed` and never warns.
  - `update_targets(&mut self, visible: &VisibleCells, camera_cell: Option<usize>, monotonic_seconds: f64)`;
    the session method of the same name takes the same parameters.
  - `report_snapshot()` becomes unconditionally compiled (drop its `cfg`), and its
    snapshot type gains `warm_clusters: usize`.
  - `ShResidencyCounters` gains: `discarded_reads`, `discarded_read_bytes` (a
    completed read that admission dropped as stale, not targeted, or duplicate; bytes are
    the encoded chunk length, the same unit as `read_bytes`),
    `cancelled_requests`, `decoded_bytes_installed` (sum over clusters handed to the
    renderer in batches), `last_drain_decoded_bytes`, `max_drain_decoded_bytes`,
    `budget_limited_drains` (drains that left eligible ready work behind because of the
    byte budget). All `u64`.
- **Controller construction** receives `Option<&postretro_level_loader::CellVisibility>`
  and builds its own per-cell adjacency. `Session::prepare_sh_streaming_drain` gains
  `cell_visibility: Option<&CellVisibility>` and `camera_cell: Option<usize>` parameters.
- **Renderer snapshot** (`ShResidencySnapshot`, T1) gains cumulative
  `install_cpu_total_micros`, `install_cpu_max_drain_micros`, `install_cpu_last_drain_micros`,
  `pool_growth_events`, `pool_growth_bytes` (all `u64`).
- **Panel data type** (T1, renderer crate, always compiled, re-exported from
  `postretro_renderer`): `ShStreamingLiveDiagnostics`, plain `Clone + Debug + Default`
  data with exactly these public fields:

  ```text
  // gauges
  target_clusters, warm_clusters, sampleable_clusters, queued_clusters,
  ready_clusters, permits_in_use: u64
  active_capacity_bytes, logical_occupancy_bytes: u64
  // controller counters
  misses, installs, evictions, retries, cancelled_requests,
  discarded_reads, discarded_read_bytes,
  decoded_bytes_installed, last_drain_decoded_bytes, max_drain_decoded_bytes,
  budget_limited_drains: u64
  // worker counters
  reads_issued, coalesced_reads, read_bytes, gap_bytes: u64
  read_latency_p50_ms, read_latency_p95_ms, read_latency_max_ms: f32
  decode_latency_max_ms: f32
  // renderer counters
  install_cpu_total_micros, install_cpu_max_drain_micros,
  install_cpu_last_drain_micros, pool_growth_events, pool_growth_bytes: u64
  ```

  `reads_issued` counts physical reads; `coalesced_reads` counts physical reads that
  covered more than one chunk. `draw_diagnostics_panel` gains a trailing parameter
  `sh_streaming: Option<&ShStreamingLiveDiagnostics>`; `None` renders "SH streaming
  inactive".
- **Loader API added by T3** on `ShStreamManifest`:
  `chunk_file_range(cluster_id) -> Result<Range<u64>, PrlLoadError>` (absolute; empty for a
  canonical empty cluster) and `read_file_span(range) -> Result<Vec<u8>, PrlLoadError>`,
  which rejects any range outside the id-50 payload region.

## File ownership

| Track | Model | Owns | May touch (compile-forced only, flag it) |
|---|---|---|---|
| **T1 renderer** | opus | `crates/renderer/src/render/sh_streaming/**`, `sh_streaming.rs`, `sh_residency.rs`, `debug_ui/**` (new tab goes in a new `debug_ui/` submodule; `debug_ui/mod.rs` is already 1.3k lines), renderer re-exports | the single `draw_diagnostics_panel` call in `crates/postretro/src/main.rs` (pass `None`); struct literals of `ShResidencySnapshot` elsewhere |
| **T2 controller** | opus | `crates/postretro/src/sh_streaming/**`, `session/sh_residency.rs` (camera-cell/CellVisibility plumbing and the new `update_targets` signature only), `capture/prepared.rs` call site, the `prepare_sh_streaming_drain` call in `main.rs` | `session/sh_async_workers.rs` only for the `ShClusterRequest` field addition in test literals |
| **T3 I/O + surfacing** | opus | `crates/level-loader/src/sh_stream/**`, `session/sh_async_workers.rs` (may split into `session/sh_async_workers/`), remaining `session/sh_residency.rs` glue, `capture/report.rs`, the panel-data fill and periodic log, `main.rs` panel wiring | — |

T1 runs in an isolated worktree concurrently with T2 on the branch. T3 runs on the
branch after both merge.

## Acceptance

Focused tests only. Every `cargo test` line must report a nonzero passed count.

**T1**
- `cargo test -p postretro-renderer --lib sh_streaming` → all pass, including every
  existing malformed-cluster/rollback test unchanged in intent.
- A new test proves a failed install after partial allocation leaves allocator slots,
  compose words, sparse pools, dirty/resident rows, and row refs identical to the pre-install state.
- `grep -nE "previous_[a-z_]+ = self\.[a-z_]+\.clone\(\)" crates/renderer/src/render/sh_streaming/install.rs` → empty.
- Report: measured install CPU cost before and after on the same input (a timed
  test over a synthetic large state, or a `sync-proof` capture of
  `content/dev/maps/campaign-test.prl`), and which structures dominated.
- `cargo test -p postretro-renderer --lib debug_ui` → passes; the Streaming tab appears in `DiagnosticsTab::ALL`.
- `cargo check -p postretro --features dev-tools` compiles.

**T2**
- `cargo test -p postretro --bin postretro sh_streaming` → all pass (adjust the binary
  target name if it differs; report the count).
- New controller tests, each named for its claim:
  - same camera cell, visible set rotated through disjoint subsets across frames →
    the Prefetch-class target set is identical every frame and no Prefetch request is issued after the first frame's;
  - the warm walk reaches clusters beyond the camera cell's direct stored pairs when those all lie in the camera cluster;
  - warm set size ≤ `WARM_SET_CLUSTERS` and it includes the camera cluster;
  - ranking follows distance bucket, then aperture, then id; authored priority outranks distance;
  - pressure yields the farthest equal-priority prefetch first;
  - id 46 absent → two-hop from the camera cluster, and one warning;
  - byte budget: one oversized cluster installs alone; several small ones fill up to the
    budget; selection stops rather than skipping; `budget_limited_drains` counts;
  - `admit_cancelled_request` on a re-targeted cluster returns it to `Absent` with the permit released, no `Failed`, no warning.
- `grep -rn MAX_INSTALLS_PER_DRAIN crates/` → empty.

**T3**
- `cargo test -p postretro-level-loader --lib sh_stream` → pass, including range and
  out-of-region rejection tests for the two new manifest methods.
- `cargo test -p postretro --bin postretro sh_async_workers` → pass, with new tests using
  an injectable source that records read offsets:
  - shuffled submissions → mandatory reads precede optional, each ascending by offset;
  - chunks within the gap cap merge into one physical read and split into correct per-chunk bytes;
  - span cap respected; a chunk larger than the span cap is read alone;
  - a request for a cluster cleared from the shared bitset is never read and completes `Cancelled`;
  - the existing delayed-reader non-blocking test and retirement tests still hold;
  - decode runs on a pool thread, never the issuer (assert thread names).
- A session-level test: the periodic log line appears once per interval when counters
  change and not at all when idle (use `crates/test-log-capture`).
- `cargo check -p postretro --features dev-tools` and `cargo check -p postretro --features capture` compile.
- Manual (owner, not machine-verified): `RUST_LOG=info cargo run -p xtask -- run --features dev-tools -- content/dev/maps/stress-warren-mini.prl`
  (the owner's yardstick map, baked with `--lightmap-density 0.8`; `campaign-test.prl` is stale);
  turning in place produces no new reads in the log or Streaming tab; walking produces
  ordered reads with nonzero coalescing.

**Landing:** `cargo test -p xtask layering_invariants_hold` passes; `/preflight` green.

## Resolutions from T2 (landed `bf133414b`)

- Warm rank orders requests within every class, not only Prefetch. The install drain
  order stays `(class, Reverse(priority), cluster_id)`.
- In the id-46 fallback every cluster has warm rank 0, so the legacy ordering holds.
- A camera cell outside the id-49 map is `InvalidTopology`. An id 46 with zero pairs is
  usable, and the warm set is then the camera cluster alone.
- `last_drain_decoded_bytes` updates only on drains that hand over at least one cluster.
  A deferred chunk is counted in `decoded_bytes_installed` again when it is handed over again.
- The warm set is built in `sh_streaming/warm_set.rs`. Pressure and eviction ordering
  moved to `sh_streaming/pressure.rs`.

## Resolutions from T1 (landed `3590fe5c9`)

- Install rollback is an undo journal (`sh_streaming/install_journal.rs`); resident-row
  sets update incrementally. A synthetic 1M-probe install went from 1.82 s to about 4 ms.
  The dominant cost was rebuilding resident sets per sparse row, not the clones.
- Eviction still rebuilds resident sets once per evicted sparse row. It is the next hotspot in the same drain.
- `postretro-renderer` lib tests do not compile with `--features dev-tools`, because of
  code that predates this work (`direct_sh_compose.rs` uses `ComposeStorageFootprint`/`footprint()`).
  The T1 debug-UI acceptance needs that file fixed first.

## Resolutions from T3 (landed `2b46583e8`) and later fixes

- `sh_async_workers` is a directory: pure `schedule.rs` planner, `issuer.rs`,
  `decode_pool.rs`, `stats.rs`, `target_bitset.rs`. Diagnostics assembly and the log
  throttle are in `session/sh_streaming_diagnostics.rs`.
- Controller owner-walk fixes: a diamond in the owner graph no longer reads as a cycle
  (`2b46583e8`). A dependent is no longer requested while its owner is blocked behind
  in-flight work (`fe6176c97`).
- The async frame takes read requests only after the drain's budget policy
  and the second publish, so no request is submitted for a cluster that the
  same frame suppresses.

- Sync-proof mode (which capture requires) counts its frame-thread reads into
  session-owned read stats: one uncoalesced read per chunk, with latency covering read
  plus decode. The capture report's I/O fields are therefore real, not zero.
- `ShDrainBatch::validate_contract` kept the retired two-ready-cluster cap after
  decision 12, so any map whose clusters fit more than two per 8 MiB drain failed its
  first drain ("drain batch exceeds the two-ready-cluster cap" on `campaign-test`).
  `stress-warren-mini` never tripped it because two of its clusters already fill the
  budget. The boundary now checks identity and structure only; ready count stays
  controller policy.

## Resolutions from T4 (landed `c36cbe9ba`)

- Real-content install cost was dominated by per-call wgpu queue writes, each paying its
  own staging allocation. Every install now uses one upload batch (`gpu/staged_uploads.rs`
  over a best-fit recycled `gpu/staging_pool.rs`), and so does each drain's promotion plus
  eviction. Row bookkeeping is per affinity row (`row_refs.rs`), and eviction releases rows incrementally.
- `ShStreamingLiveDiagnostics` and `ShResidencySnapshot` gain `pool_growth_cpu_micros`
  and `install_cpu_max_steady_drain_micros` (the slowest drain that grew no pool).
- Measured on `stress-warren-mini`, 45 s idle, dev profile: first-window install CPU went
  from 1208 ms to 223 ms. Headless capture: 2.22 s to 0.36 s for the same 22 installs.

## Open questions

- **Count-only warm bound on fine-grained maps.** Largely answered by measurement. The
  earlier "eight clusters reach about eight cells" estimate divided all cells by all
  clusters, but about 90% of clusters are one-cell solid clusters or exterior clusters
  that the warm walk never reaches (id 46 omits solid cells; exterior components never
  connect to playable space). Only playable clusters count: `stress-warren-mini` has 26
  (about 12 cells and a 6 MiB median each), `campaign-test` has 22, so eight cover roughly
  a third of either map. The open risk is the reverse: clusters are coarse, and several
  on `stress-warren-mini` exceed the 8 MiB drain budget on their own. The owner's walk
  decides whether cluster size needs a follow-up.

- Default magnitudes (`WARM_SET_CLUSTERS`, coalescing caps, byte budget, permits) are
  first guesses to be tuned from the new diagnostics on real content; changing them is not
  a contract change.
- Whether to cap the warm set by bytes as well as by count is deferred; the existing
  pressure policy trims optional work by bytes today.
