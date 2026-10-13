# sh-streaming--pool-growth-device-limit — research

Read at a83e186; source unchanged through 6391db9. No SH streaming, controller or frame-loop source changed since 27ca6a6, where the
premises were first traced (`portal-walk-bounded-regions/research.md` §SH pool device limit).

## Premise check

| Premise | Verdict | Symbol |
|---|---|---|
| Dense growth doubles layers and errors when the doubled count passes the limit, never trying the minimum | Holds. `layers = max(layers * 2, minimum_layers)`; `> max_texture_array_layers` returns `GpuCapacity` | `AtlasShape::grown_for_slots` (renderer `sh_streaming/gpu.rs`), called from `StreamingGpuPools::grow_dense` |
| Sparse growth doubles the same way; buffer validation errors rather than clamps | Holds. `checked_mul(2).max(required)` for entries and tiles; replacement constructors call `validate_storage_buffer_size`, which returns `GpuCapacity` past `max_buffer_size` or `max_storage_buffer_binding_size` | `StreamingGpuPools::grow_sparse` (`gpu/growth.rs`), `validate_storage_buffer_size` (`gpu.rs`), `StreamingIndirectCompose::new` (`gpu/indirect.rs`), direct `sparse.rs` |
| A failed install rolls back; only retirement pressure is retried | Holds. `install` rolls the journal back on a staging error; ranges allocated before a refused growth roll back with it (`allocate_owned_nodes` comment). `install_ready` defers only `is_retryable_retirement_pressure` and returns every other error, discarding the drain's outcome | `ShResidencyState::install`, `install_ready` (`lifecycle.rs`) |
| Any other drain error ends the session, in Settling and Running | Holds. `drain_sh_residency` → `render_frame_indirect` `?` → `frame_loop/mod.rs` sets `app.exit_result = Err(..)` and exits; Settling runs through `app/world_less_frame.rs` (`settling: true`) with the same exit | `Renderer::drain_sh_residency`, `render_frame_indirect` |
| Mandatory demand is never trimmed; overshoot is logged and counted | Holds. `apply_budget_policy` trims only `pressure_evictable_optional`; protected excess goes to `set_non_evictable_overshoot`, warned once per onset | `postretro/src/sh_streaming/pressure.rs` |
| `rendering_pipeline.md` §10 claims streamed pools defer past the layer limit | **Partly wrong.** The parenthetical "a streamed pool instead defers growth past the array-layer limit" qualifies the lightmap pool. For SH, §10 says only that oversized whole-load SH volumes disable cleanly. No doc states that streamed SH growth ends the session. §4's tier table says mandatory demand makes "the pool grow past its budget", with no device ceiling named | `context/lib/rendering_pipeline.md` §4, §10 |
| The lightmap pool defers at the device limit | Holds. `place_with_growth` defers a pair that `allocate_within(.., max_layers)` cannot place, sets `device_limited`, and skips repeat victim walks for no-smaller extents within the drain. `warn_device_limit_once` logs once per level. The app keeps a deferred pair in hand (`lightmap_streaming/drain.rs`) | `render-cpu/src/lightmap_pool/policy/placement.rs`, `renderer/src/lighting/lightmap/stream.rs` |
| No wgpu error scope or uncaptured-error handler | Holds. No `push_error_scope`, `pop_error_scope` or `on_uncaptured_error` in `crates/`. wgpu's default handler panics on an uncaptured error such as OOM (library behavior, not re-read this session) | workspace grep |

## The device's limits

`renderer_init_resources.rs` requests `max_texture_array_layers = 256`, `max_buffer_size` and
`max_storage_buffer_binding_size` = 2 GiB − 8. `device.limits()` returns these requested values,
not the adapter's, so every machine that runs the engine shares them. The SH pools read them
through `device.limits()`.

## Bake-time caps

- The SH bake refuses a probe grid whose octahedral atlas needs more than `MAX_SH_ATLAS_LAYERS`
  (256) layers at `MAX_SH_ATLAS_DIMENSION` (8192): `irradiance_atlas_array_layout` returns `None`,
  and `sh_bake.rs` maps that to `ShAtlasBakeError::LayerOverflow` ("raise `probe_spacing` or split
  the map"). The bake lays out 6-texel tiles (`DEFAULT_IRRADIANCE_TILE_DIMENSION`), 1365 per row;
  the streamed runtime atlas uses 8-texel physical tiles, 1024 per row. The bake's cap is
  therefore about 1.8× looser than 256 full runtime layers. A grid between the two compiles, but
  its whole dense set cannot fit the runtime atlas. At about 268 M tiles and roughly 1.6 KB per
  slot, that is hundreds of GB, far past any device. Whole-level layer sizing makes the dense
  limit unreachable below that scale.
- No bake or load check bounds a whole sparse family against the 2 GiB − 8 binding.
  `validate_sparse_caps` (`floor/validation.rs`) checks only the initial capacity, not
  `SparsePlan::maximum`. The hallway's whole id-50 payload is 246 MiB, an order of magnitude
  under the binding.

## Reachability

The device limit is reachable, but only at the edge of the content range.

- **Dense.** The atlas's per-layer geometry is fixed at load from the initial dense slot count `S`
  (`AtlasShape::for_slots`: about `sqrt(S)` tiles per row, so one layer holds `S` slots). Growth
  doubles layers from 1, so the 256-layer limit binds once demand passes 256 × `S`. `S` comes
  from the dense family's share of the 256 MiB floor (`plan_initial_pool_floor`,
  `DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES`), floored at the largest single-cluster writer closure.
  With a generous share, 256 × `S` is far past the 2 GB VRAM target and OOM comes first. When
  fixed metadata plus whole-resident billboard scatter use the floor up (`pool_budget_bytes = 0`),
  or sparse weight dominates the proportional split, `S` collapses toward one cluster's closure
  and the limit becomes reachable at ordinary sizes.
- **Sparse.** Doubling a family's tile or entry buffer passes the 2 GiB − 8 binding limit once the
  buffer is over 1 GiB, even when the size actually needed fits. That is already past the VRAM
  target.
- **Example in the owner's terms.** A pool at 200 layers needing 201 doubles to 400 and fails
  against 256. Today the initial layer count is 1, so doubling lands on powers of two. Under
  whole-level layer sizing, the initial count is the floor share divided by the whole-level
  layer size, which can be any number. Doubling from it then overshoots 256 at ordinary sizes.
  The clamp is required by the geometry change, not only at the content edge.
- **Measured maps.** campaign-test's whole id-34 payload is about 57 MB
  (`plans/done/lighting-scale--sh-base-atlas-at-rest-slimming`). It fits inside the floor, so it
  never grows past it. Streamed SH payloads (id 50) are 26 MiB on campaign-test and 246 MiB on
  the stress-warren hallway, and owner walks rarely exceed about 40 MB resident
  (`large-map-spatial-residency.md` §Pre-planning measurements). No shipped map is recorded
  reaching the device limit. This is an
  anticipated defect, confirmed in source.
- **The degenerate case is a load-time choice.** `AtlasShape::for_slots` and `floor.rs`
  `atlas_capacity_slots` derive tiles per layer from the initial `dense_slots`, and
  `dense_slots_for_share` falls back to the largest writer closure when the share is zero.
  `whole_dense_slots` is already known at that call (`ShResidencyState::initialize_gpu`,
  `setup.rs`). Sizing the layer from the whole level, divided by the layer limit, removes the
  case for every level whose dense set fits the device. First-fit fragmentation
  (`FirstFitRanges`, whose capacity is an address high-water) can still push the high-water past
  the live total, hence the headroom factor.
- **Probe-indirection addressing** (`PROBE_INDIRECTION_SLOT_SHIFT`, 27 slot bits) is a further
  capacity bound reported as `SlotOverflow`. It binds only past about 200 GB of dense data, so it
  is not a growth limit; overflow stays a fatal guard.
- **Sparse caps on a streamed level.** The loader's 128 MiB per-section floor
  (`MAX_DELTA_SECTION_BINDING_BYTES`) runs only on non-streamed loads; the streamed manifest reads
  ids 27/41/45 with no size floor. The only bound is the compiler's delta cap
  (`DEFAULT_MAX_PAYLOAD_BYTES` = 256 MiB, `--sh-delta-max-size`, no upper bound). A sparse GPU
  buffer can outgrow its source: growth doubles the allocator high-water, and the compaction table
  adds 12 bytes per whole-level affinity cell, which the cap does not count. At the default cap a
  buffer reaches the binding only past about 4× sparse fragmentation.

## Commitments touched

- `plans/done/sh-probe-streaming--cluster-residency`: growth doubles, or rises to the exact
  non-evictable minimum if that is larger. Its words: "If adapter limits prevent append-only
  growth, the install fails with a named capacity error rather than silently changing slot
  geometry." The clause guards slot geometry, and clamped growth keeps the geometry. That plan
  did not decide whether a failed install ends the session; the frame loop's handling of a
  drain `Err` decided it. Its failure table also says: "DrawAll or non-evictable set over floor
  | grow, edge-log overshoot, preserve correctness."
- `plans/done/sh-probe-streaming--authored-hints`: "If GPU pool growth is impossible, the
  existing named `ShResidencyDrainError::GpuCapacity` surfaces; a pin is not silently evicted to
  make it fit." Its AC asked for a pure renderer-capacity fixture that reports the error. No
  test pins that error today (grep for the reason string finds only its definition). Deferral
  keeps the half that matters: no pin is evicted.
- `rendering_pipeline.md` §10: runtime guards degrade, "rather than panicking during texture
  creation or upload". This brief extends that rule to streamed SH growth.
- `rendering_pipeline.md` §4, lightmap: "A pair that needs growth while a generation retires,
  or past the device's array-layer limit, is deferred and counts as a miss." This brief is that
  sentence's SH counterpart.
- `plans/large-map-spatial-residency.md` outcome: "Map scale is bounded by disk and residency
  budget rather than a whole-level GPU upload." A build check on whole-level SH size would
  contradict it; a check per camera cell's mandatory set does not.
- `plans/large-map-spatial-residency.md` §Residency budget tiers, owner decisions 2026-09-28:
  "No pop-in on interior maps, including large arenas. A tier is valid for a map only when every
  camera cell's mandatory set fits it, so a miss fallback cannot stand in for residency." Still
  open for the owner there: "Whether a tier below mandatory warns, clamps, or stays silent."
  Deferral as the answer for a map that fits the device would contradict the first; deferral past
  a true device limit is the second question.
- Lightmap parity is partial. Lightmap layers are a fixed 2048², so 256 layers are a real
  capacity. SH dense layer size follows the engine's load-time choice.
- `boot_sequence.md` §1 Settling: 10 s timeout, one warning naming unsettled counts, reveal
  with miss fallbacks.
- `index.md` §1.1: 2 GB VRAM target. The device limit sits above that target for all but the
  degenerate dense case. A VRAM ceiling belongs to stage 5's tiers.

## Refusal semantics

- `ShDrainOutcome::deferred` keeps a chunk's payload, ready bytes and stream permit, and the
  controller returns it to `Ready` (`apply_drain_outcome`). Retirement-pressure deferrals keep
  this path, and their misses count as `ShMissBuckets::held_by_drain_budget`.
- A device-limit refusal is a separate outcome. The app marks the cluster failed through the
  existing failure state, releasing payload, ready bytes and permit once. A permit covers a
  cluster from request until install, drop or failure, and every exit releases it exactly once.
- Retry: the existing `FailureState` allows one retry per identity (`retry_spent`) and warns per
  chunk identity. A device-limit failure leaves the retry unspent and warns through a level-scoped
  latch instead, so it is retried on every leave and re-entry of its targets. A retry requires the
  cluster to leave the target set and re-enter on a later horizon change, and a refusal-caused
  failure retries at most once per hysteresis window (round 6 ruling).
- No wake and no yield: nothing evicts to make room for a refused cluster.
- Partial growth: a cluster that needs dense growth (fits) and sparse growth (does not) keeps
  the dense growth. The install's own comment accepts this: "a larger pool with the same live
  contents is still a valid pool."

## Out of scope, with reasons

- **OOM below the limits.** An allocation the device refuses for memory surfaces as a wgpu
  uncaptured error, and the default handler panics. Every renderer allocation shares this, not
  only SH growth, and handling it needs an error-scope policy across the renderer. Separate
  brief.
- **Load-time capacity errors.** `renderer_resources.rs` panics with "cannot install streamed SH
  level resources" when `initialize_gpu` fails, for example when the floor's fixed bindings
  exceed limits. This is a deliberate fail-fast at install, before play, with a comment arguing
  that unlit is worse. It is not a mid-session exit, and it scales with whole-level metadata,
  which a compile-time check rejects better.
- **Sparse growth clamped to whole-level need.** Doubling can allocate past what the level could
  ever use (`whole_sparse_capacity` is known at setup). Fixing that is VRAM efficiency, not
  survival, now that a sparse refusal does not end the session.

## Rivals

- **Refusal handling alone, without load-time geometry.** It keeps today's small-layer degenerate
  case and turns it into ambient-floor pop-in on a level the device can hold, which the
  2026-09-28 owner decision rules out.
- **Load-time feasibility instead of runtime refusal.** After whole-level geometry and clamped
  growth, the remaining refusals are fragmentation and a sparse family past the binding under a
  raised delta cap. Fragmentation is runtime state and needs a non-fatal backstop either way; a
  per-cell build check was weighed and cut (§Reshape).
- **Compact dense slots at the limit before refusing**, as the lightmap pool repacks before it
  grows. Costlier for SH: every moved node's probe-indirection words must be rewritten. It only
  helps fragmentation past the headroom, which is speculative.
- **Hold the refused cluster's payload until a wake (parking), or wake on evictions.** Rejected:
  the app learns of confirmed evictions only after the drain, a held payload has no bound, and a
  per-family wake needs eviction reports the outcome lacks. The failure path with an unspent
  retry reuses existing state and costs a re-read only for content at the device limit.

## Ordering pins

From `/review-brief` (rows lens, round 1).

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Two clusters in one drain: one refused at the limit, one that fits, either order | The refused install rolls back its provisional dense and sparse ranges, address high-water included, before the next ready cluster stages | The fitting cluster installs; the refused one is refused and dropped as device-limited; neither order changes the result |
| P3 | One cluster needs dense growth that fits and sparse growth that does not | Dense growth commits during staging; the sparse refusal then rolls back the install, not the growth | Dense growth stays (valid pool, one dense retirement pending); cluster is refused and dropped as device-limited; a later cluster this drain whose dense need fits the limit defers as retirement pressure; one whose need exceeds it is refused (P4) |
| P4 | A cluster's needed size exceeds the limit while that family's previous generation still retires | The limit verdict precedes the retirement wait, in dense and sparse alike (today dense checks the limit first, sparse checks retirement first) | Device-limited refusal and drop; a cluster that fits but waits defers as retirement pressure |
| P5 | Refusal in drain N; visible misses recorded at frame N+1's target update | The refusal's failure mark lands with outcome N and is read before N+1 retargets | A visible episode that starts after the refusal counts device-limited, as does a dependent's episode that starts behind the refused owner; an episode already counted keeps its bucket (P10) |
| P6 | Refusal during Settling; timeout decided on frame N; reveal on N+1 | Settling counts no misses; the reveal frame is the first Running frame | Settling warns once; reveal proceeds; the reveal frame counts one device-limit miss per refused visible cluster and no second warning |
| P7 | Refusals across generations within one level, then a level change | Warning latch is level-scoped, not generation- or boot-state-scoped | One warning per level; the next level's first refusal warns again |
| P9 | Several sparse families need growth for one cluster; one is refused | Every replacement is built before any copy or swap | No family commits when any is refused (current transaction); the cluster is refused and dropped as device-limited |
| P10 | Visible cluster missing at frame N (counted read-in-flight or held-by-drain-budget), refused in drain N | Misses are counted per visible episode, bucketed at episode start, before the frame's drain | N+1 counts no second miss and moves none between buckets; the device-limit bucket counts only episodes that start after the refusal |
| P11 | Refusal in the reveal frame's own drain | The reveal frame records misses before its drain | The refused visible cluster's reveal miss stays in held-by-drain-budget; the device-limit warning logs once if no earlier refusal in the level warned; Running continues |
| P12 | Refusal in drain N; cluster leaves the view at N+1 and returns | A drained cluster is targeted when refused; `left_target` is set only when the cluster leaves the target set, after hysteresis expires or when a seam-warm or owner-only target drops, stamped with that update's horizon revision (the trim never selects a refused cluster) | A return within the 2 s hysteresis window is no retry; a return after the cluster left the targets, on a later horizon change, retries, at most once per 2 s (P19); a suppression lifted without a horizon change does not retry |
| P13 | Owner refused while dependents want it | Dependents are requested only after their whole owner closure is Sampleable | No dependent is Ready or holds a permit behind a refused owner; dependents stay Absent and unrequested while it is failed, and are requested after its retry installs |
| P14 | A cluster refused, retried, then refused again in the same level; or read-failed, retried, then refused | Only a retry out of a read failure sets `retry_spent`, at the retry transition; a retry out of a device-limit failure leaves it unchanged, and a refusal never clears it | The twice-refused cluster retries again on its next leave and re-entry; a read failure that already spent its retry is not retried again, and a later refusal of that cluster still retries on leave and re-entry |
| P15 | Cluster failed by refusal; SH session replaced within the level (mode change) | A new residency generation is a new controller; level-scoped state survives | The new controller holds no failure and requests the cluster afresh; the device-limit latch, held at level scope, does not warn again |
| P16 | Eviction and refusal in one drain; eviction in a later drain | The renderer evicts before it stages installs | A cluster refused in drain N already saw that drain's freed room; an eviction in N+k does not change its failed state |
| P17 | Refused; retried; the re-read fails the loader's hash check; later retried and refused again | The latest failure's cause decides the retry. `retry_spent` is the cluster's read-failure budget for the controller's life: only a read-failure retry sets it, and nothing clears it | The read failure after a refusal still gets its one retry and logs its one read-failure warning. Once that retry is spent, a later refusal still retries on every leave and re-entry, and a later read failure is not retried |
| P18 | Pressure trim and refusal in one drain; a later retry under pressure | Pressure runs at drain start against the floor budget and counts the Ready cluster; its evictions apply before staging; the refusal is learned only from the outcome | The trim is ordinary floor-budget pressure, not a refusal yield: if it frees room the cluster installs, otherwise it is refused and the trimmed residents stay evicted. A failed cluster adds nothing to projected demand, so no later batch trims on its account until it is read again |
| P19 | A refused cluster while the camera turns, changing the horizon every frame: inside the horizon, targeted only as a seam-warm neighbour, or an owner targeted only through a dependent that pressure suppresses | A refusal-caused failure is retried at most once per hysteresis window, whatever path takes it out of and back into its targets; the trim's candidate filter skips it | Each is read again at most once per 2 s, never once per horizon change; refused clusters add nothing to the overshoot gauge |

## Review history

Rounds 2–4 below argued designs the reshape cut (parking, per-family wake, yield, shared derivations, per-cell build check). Live rulings from them: no yield rule; the `GpuCapacity` device-limit/contract split; corrupt-on-disk chunks fail per cluster through the loader's hash and body checks. Everything else in those rounds is history.

## Premise review notes (round 1)

- A sparse family's device limit is its tightest buffer, not the entry or tile pool alone. Each replacement validates the tile pool (tiles ÷ 2 words), the entry pool (entries words) and the compaction table (3 × affinity rows + entries words) against the 2 GiB − 8 binding; the compaction table binds first on entries. A clamp or build check that caps entries at binding ÷ 4 still fails (`StreamingIndirectCompose::new`, direct sparse replacement, `validate_storage_buffer_size`).
- `GpuCapacity` from install splits cleanly at two growth seams. Device-limit: `grown_for_slots` (layers) and `validate_storage_buffer_size` inside a sparse replacement. Contract, still fatal: missing device handles, lost id-35 or compose views during growth, no id-35 base, unsupported format, upload misalignment after commit, and the post-growth checks "streamed indirect sparse pool needs growth", "streamed indirect compaction metadata needs growth" and "streamed direct sparse row exceeds its GPU pool capacity". Those last three fire only when growth undersized a pool; classing them as device-limit would turn a growth bug into silent deferral.
- The per-cell build check can run only after the ClusterDirectory stage: id 51 is baked after the SH bake, and the cell→cluster map, pins and directory exist from ClusterDirectory on.

## Premise review notes (round 2)

- The dense slot address is 27 bits (`ownership.rs`, `sh_indirection.rs` raise `SlotOverflow`
  past 2^27 − 1), while one full layer × 256 is 2^28 slots. The dense limit is the smaller of the
  two; the scale is far past VRAM.
- The shared sparse rule gives the minimum need (owned-row sizes with per-row padding). The
  runtime compares the allocator high-water, so the two agree only up to sparse fragmentation;
  the headroom covers dense only.
- The hoisted limits are renderer-wide: 256 layers is justified by the lightmap, and the buffer
  limits apply to every storage buffer. `level-format` already holds `MAX_SH_ATLAS_LAYERS` and
  `MAX_SH_ATLAS_DIMENSION`, which the renderer duplicates privately.
- Owner closure is derived on `level-loader` types in the app and again in the renderer;
  `build_pipeline.md`'s claim that `level-format` is the only crate below both is stale, since
  level-compiler already depends on level-loader.
- The lightmap's fatal class is batch identity and generation only; its capacity and upload
  errors roll back and retry, and a payload that does not fill its block counts as a failed read.

## Review notes (round 3)

- A chunk corrupted on disk already fails only its own cluster: the loader's hash check fails the read through `admit_failed_request` / `mark_failed`, with one warning, and the session continues. The renderer's `MalformedChunk` fires only for hash-valid chunks that disagree with metadata, and shares its variant with contract breaks (some reporting cluster 0, some after the commit point). It stays fatal.
- Hysteresis residents are targets by construction (`TargetClass::Hysteresis`, protected, not pressure-eligible). The yield rule therefore works through pressure eligibility, not by untargeting them.
- The renderer never derives a mandatory set; it derives the owner graph (`derive_owner_dependencies` in `setup.rs`). The app's owner derivation skips validity-0 probes and counts halo owners only from id-50 sparse sources, while the renderer's covers every probe and all three delta sections. The shared derivation picks one relation; prl-build has no stream manifest before Packing, so it takes sections.
- The 27-bit slot address bound is a fixed constant (`PROBE_INDIRECTION_SLOT_SHIFT`), defined in the level format and again locally in the renderer's ownership code; test limits cannot lower it.
- Existing per-cluster latches to reuse for the device-limited mark: the `suppressed` flag with its pressure clear, and `FailureState`'s retry on leave and re-enter.
- Cross-crate rows are proven by splitting at the drain outcome; GPU tests exist only in the renderer crate.

## Review notes (round 4)

- The pressure trim runs only while projected demand exceeds the floor budget (`pressure.rs`), and projected demand leaves out a refused, now-Absent cluster. A refusal-driven yield therefore trims nothing below the floor, which is every case where device limits bite. Seam-warm and band residents are already trim-eligible; hysteresis is the only class a yield rule would add, and its window expires within 2 s. Owner ruling: no new yield rule.
- `ShDrainOutcome.evicted` carries cluster ids only, and the app tracks no per-cluster family membership. The renderer's installed record knows each cluster's owned nodes and sparse rows, so it reports refused families and per-eviction freed families. `grow_sparse` stops at the first family that cannot proceed; the limit verdict must be taken for every family before any retirement wait so the refused set is complete.
- Two owner relations answer different questions: the app's (skips validity-0 probes, halo owners from the id-50 manifest) drives demand; the renderer's `derive_owner_dependencies` (every probe, all three delta sections) guards install order. Owner ruling: share the app's in `level-loader`; keep the renderer's; a row proves the app's closure satisfies the renderer's order.
- After the hash, the loader also validates the chunk body layout, so a fixture that only recomputes the hash becomes a per-cluster read failure, not a fatal error.

## Reshape (validate-plan, 2026-10-13)

- Verdict Reshape, accepted by the owner. After whole-level geometry and the clamp, the reachable refusals are dense fragmentation past the headroom and sparse past its tightest buffer (fragmentation, or a raised delta cap; §Reachability). The per-cell build check, the shared sizing and mandatory-set derivation, and the per-family wake served the build check and were cut, with the pins that pinned them. Pins P10 onward in §Ordering pins were written after the reshape and are live.
- Rival kept: the existing per-cluster failure path (`FailureState`) as the refusal handler, instead of a new wake protocol.
- Owner ruling (round 6): PostRetro games turn fast; refusal handling must not evict anything that would need re-reading within a turn. The pressure trim skips refused clusters.
