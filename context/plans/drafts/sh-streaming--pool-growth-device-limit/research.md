# sh-streaming--pool-growth-device-limit — research

Read at a83e186. No SH streaming, controller or frame-loop source changed since 27ca6a6, where the
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
  against 256. Doubling from 1 lands on powers of two, so this exact shape arises only from a
  non-power-of-two initial layer count (`S` past one layer's 1024 × 1024 tile cap); it remains
  the cleanest unit test of the clamp.
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
  capacity bound reported as `SlotOverflow`. It sits orders of magnitude past the layer limit,
  so it is out of scope.

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

## Deferral semantics

- `ShDrainOutcome::deferred` keeps a chunk's payload, ready bytes and stream permit, and the
  controller returns it to `Ready` (`apply_drain_outcome`). Retirement-pressure deferrals use
  this path today, and their misses count as `ShMissBuckets::held_by_drain_budget`
  (`targeting.rs` `MissCause::HeldByDrainBudget`).
- Permits: `MAX_STREAM_PERMITS` = 8. Device-limited clusters that stay deferred hold permits.
  That blocks only reads of other clusters, and those reads could not install either while the
  pool is full of mandatory content. When targets change, `install_ready` drops untargeted
  deferred chunks and their permits release. Optional work cannot hold the limit:
  `apply_budget_policy` trims evictable optional targets whenever projected demand exceeds the
  floor, and the device limit sits above the floor. Clusters kept resident by hysteresis can
  hold capacity until their window expires.
- Retry cost: each drain re-stages a device-limited cluster and rolls it back. The lightmap pool
  bounds the equivalent walk with a per-drain "no smaller extent can fit" skip. Without a bound,
  an SH level past the limit pays one failed allocation per deferred cluster per drain.
- Priority at the limit: with mandatory never evicted and hysteresis residents holding room
  until their window passes, a refused Visible cluster can sit on the ambient floor while
  off-screen Lead or hysteresis clusters stay resident, inverting §4's Visible-first order.
  Lead is mandatory and cannot yield; untargeted hysteresis residents could release early.
  Only reachable past a true device limit once the layer geometry covers the level.
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
  survival. For dense, whole-level layer sizing makes the device's layer cap equal whole need
  times the headroom.

## Rivals

- **Deferral alone, without load-time geometry.** It keeps today's small-layer degenerate case
  and turns it into ambient-floor pop-in on a level the device can hold, which the 2026-09-28
  owner decision rules out.
- **Load-time feasibility instead of runtime deferral.** After whole-level geometry and clamped
  growth, the remaining runtime refusals are fragmentation past the headroom and a sparse family
  past the binding. The second is a property of the compiled level and can be rejected at build,
  like `LayerOverflow`; that is the brief's blocks-build owner question. Fragmentation is a
  runtime state, so it still needs a non-fatal backstop.
- **Drop the refused cluster and re-read it later.** This releases the permit, but it churns I/O
  every frame while the pool stays full. Holding the payload wins.
