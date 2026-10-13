# sh-streaming--pool-growth-device-limit

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Cluster SH residency, Lightmap cell-block residency), §10, `context/lib/boot_sequence.md` §1 (Settling), `context/plans/large-map-spatial-residency.md` §Residency budget tiers · read at a83e186 · evidence: `research.md`

## Problem

The owner asked whether a map that is valid in TrenchBroom can fail to run. Through streamed SH
pool growth, it can. This is an anticipated defect, confirmed in source; no shipped map has hit
it. When a growth would pass a device limit, the renderer returns a drain error. The frame loop
treats any drain error as fatal and ends the session, in Settling and in play.

The engine's limits are fixed constants (256 array layers, 2 GiB − 8 per storage binding). Two
runtime choices make them reachable on a level whose SH fits them:
- The dense atlas's layer size comes from the initial floor share, not the level. When that
  share is small, 256 layers hold little (`research.md` §Reachability).
- Growth doubles before it checks the limit, so it fails even when the size actually needed
  fits.

When this is done, no level that compiles ends a session through SH pool growth. Out-of-memory
below the limits and the load-time install panic are other routes, outside this brief.

## Decisions

- **Dense layer size covers the whole level.** At load, tiles per layer are set so the whole
  dense set, times a fragmentation headroom, fits the layer limit, capped at one full layer.
  Geometry stays fixed for the session (`plans/done/sh-probe-streaming--cluster-residency`).
  Reason: the owner's 2026-09-28 rule in `large-map-spatial-residency.md`, "a miss fallback
  cannot stand in for residency". Cost: a first layer of whole ÷ layer limit × headroom.
- **Growth clamps to the device.** Each family grows by its usual step, capped at the device
  limit and never below what the install needs. Growth is refused only when the needed size
  alone exceeds the limit.
- **A refused growth defers; it never ends the session.** A device-limit refusal is its own
  outcome, distinct from contract errors. The cluster rolls back, keeps its payload and permit,
  installs once room frees, counts as an attributed miss and warns once per level. Settling
  releases at its timeout. This diverges from the cluster-residency plan's "named capacity
  error", which never chose to end the session (`research.md` §Commitments touched).
- **Mandatory demand is never evicted to make room.** This keeps §4's tier contract and the pin
  guarantee in `plans/done/sh-probe-streaming--authored-hints`.
- **Other drain errors stay fatal.** Malformed chunks, generation and identity violations, and
  arithmetic overflow are broken contracts, not content scale; the lightmap pool also treats its
  contract class as fatal. Its decline after repeated rolled-back drains has no SH counterpart,
  because a streamed SH level omits the whole-load bodies a decline would fall back to.
- **Placement: renderer mechanism, app attribution.** The renderer owns limits, geometry, growth
  and refusal (Renderer owns GPU). The drain outcome tells the app which deferrals are
  device-limited, and the app uses that only to attribute misses.
- **prl-build rejects a camera cell whose baked mandatory SH set cannot fit.** The set is the
  id-51 cells within the baked maximum L, plus pins and owner closure; it must fit each
  family's device limit. The error names the cell and the family. Whole-level size never
  rejects, since `large-map-spatial-residency` bounds map scale by disk and residency budget.
  Deferral stays the runtime backstop for fragmentation and drawn demand outside the baked set.
- **This lands before `portal-walk-bounded-regions`**, whose superset visible set adds
  never-refused SH demand.
- **Non-goals:**
  - OOM below the limits. It is a wgpu uncaptured error shared by every renderer allocation, so
    it needs a renderer-wide error-scope policy (`research.md` §Out of scope).
  - The load-time install panic. It is a deliberate fail-fast before play.
  - An engine VRAM ceiling or player tier: `large-map-spatial-residency` stage 5 owns it.
  - Capping sparse growth at whole-level need. That is VRAM efficiency; for dense, the geometry
    already makes the device cap the whole-level cap.
  - The lightmap pool: its layers are a fixed 2048², and it already defers.

## Acceptance

Pins P1–P10 are in `research.md` §Ordering pins.

### Automated
- [ ] Through the production load path, with the floor's pool budget forced to zero, a fixture
  whose whole dense set exceeds the layer limit times its largest writer closure grows until
  every cluster is resident at once, with no refusal. The test asserts that precondition, so
  it fails if layer size still follows the initial share.
- [ ] At load, tiles per layer hold the whole dense set times the headroom within the layer limit.
  Past one full layer's worth, the layer is full and the cap holds. In both cases, and at zero
  share, the planner's physical slot count, charged floor bytes and largest-closure dense
  minimum equal the GPU atlas's for the same level and limits.
- [ ] Atlas width, height and tiles per layer are identical across every growth in a session, the
  clamped growth to the limit included.
- [ ] A dense pool at 200 layers, limit 256, needing 201 grows to 256 and installs. One needing
  257 is refused without growing. Below the limit, growth is unchanged: 100 layers needing 101
  grow to 200.
- [ ] A sparse family whose doubled buffer would pass the binding limit, but whose needed size
  fits, grows to the limit and installs. One whose needed size exceeds it is refused.
- [ ] A growth whose needed size exceeds the limit while that family still retires a previous
  generation is refused as device-limited, in the dense atlas and in a sparse family alike.
  One that fits but waits on retirement defers as retirement pressure (research P4).
- [ ] A cluster whose dense growth fits but whose sparse growth is refused keeps the grown atlas,
  defers device-limited, and leaves the pool sampling correctly. A later cluster in that drain
  needing dense growth defers as retirement pressure. When one sparse family of several is
  refused, none of them grows (research P3, P9).
- [ ] With the limit lowered, a renderer drain whose demand exceeds it returns a device-limited
  deferral, not an error. A controller fed that outcome on every drain keeps the mandatory set
  unsettled, and Settling releases at its timeout into Running.
- [ ] In the same drain, a cluster that fits installs while another is refused, in either order,
  and the refused cluster leaves no address behind (research P1). When a later drain confirms
  an eviction whose freed range holds it, the refused cluster installs in that same drain from
  its held payload, with no new read (research P2).
- [ ] A refused cluster whose target departs returns its payload, ready bytes and permit on the
  next drain. Unloading a level with refused clusters held leaves none behind, and the next
  level starts with every permit free (research P8).
- [ ] With a pinned cluster, a lead cluster and a visible cluster's owner resident, and a refused
  visible cluster that would fit if any one of them left, every drain over the refusal keeps
  all three sampleable and returns no error. If untargeted hysteresis residents release early,
  only those leave.
- [ ] Fed a drain outcome that marks one deferral device-limited and one retirement-limited, the
  controller counts the first visible miss as a device-limit miss and the second as held by
  the drain budget. A visible dependent waiting on a device-limited owner also counts as a
  device-limit miss. The mark clears when the cluster installs or leaves its targets (research
  P5).
- [ ] A level refused during Settling and again after its timeout reveal logs one device-limit
  warning. Settling counts no device-limit miss, and the reveal frame counts one. A new
  generation in the same level does not warn again; the next level's first refusal does
  (research P6, P7).
- [ ] Regression guards: a malformed chunk still ends the session with its named error, and a
  retirement-pressure deferral behaves as before.
- [ ] A capacity-class error that is a broken contract rather than a device limit, such as an
  install reaching the pools without device handles, still ends the session with its named
  error, as does an address overflow.
- [ ] A scripted churn walk that installs and evicts clusters of mixed sizes, at the zero-share
  geometry and the chosen headroom, is never refused while its live dense set stays within the
  whole level.
- [ ] The per-cell check takes its layer and binding limits as inputs. With limits lowered for the
  test, a small fixture whose one camera cell's mandatory set (reach at the baked maximum
  lead, pins, owner closure) exceeds a family's limit fails prl-build naming that cell and
  family, and a fixture whose whole level exceeds the lowered limit while every cell's set
  fits compiles. A separate test pins the production limits to the ones the engine requests.

### Manual
- [ ] At default limits, campaign-test and the stress-warren hallway log no device-limit warning.
  Report their initial dense atlas bytes and pool growth counts against the pre-change build.

## Path

- Seams: `AtlasShape::for_slots` / `grown_for_slots`, and `atlas_capacity_slots` in `floor.rs`,
  which duplicates the geometry rule and must agree with it. Also `plan_initial_pool_floor`,
  `whole_dense_slots` (`setup.rs`), `grow_dense` / `grow_sparse`,
  `validate_storage_buffer_size`, `install_ready` and `ShDrainOutcome`.
- The bake's `LayerOverflow` cap counts 6-texel tiles, so it is looser than 256 runtime layers
  (`research.md` §Bake-time caps). A level past the runtime cap keeps full layers and falls to
  the deferral backstop.
- Classify every `GpuCapacity` site reachable from install. Only device-limit refusals defer.
- Refuse before growing whatever cannot fit. A dense growth kept while the sparse half is refused
  is valid, per the install's own comment.
- Deferral precedent: lightmap `place_with_growth` / `device_limited` and
  `warn_device_limit_once`, including its per-drain skip that bounds re-staging cost.
- Tests force limits by passing `wgpu::Limits`, as the floor tests do. `gpu_test_harness` covers
  the GPU half.
- Rivals, including deferral alone: `research.md` §Rivals.
- Durable capture at promotion: §4's mandatory row gains the device-limit backstop, and §10's
  runtime-guard sentence covers streamed SH.
- First slice: the geometry rule plus the zero-share fixture. `portal-walk-bounded-regions`
  depends on this slice.

## Open questions
- Fragmentation headroom factor — **delegated**: the smallest factor at which the zero-share
  fixture and a churn walk are never refused.
- Whether untargeted hysteresis residents release early when a visible cluster is refused —
  **delegated** (`research.md` §Deferral semantics).
