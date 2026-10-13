# sh-streaming--pool-growth-device-limit

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Cluster SH residency), §10, `context/lib/boot_sequence.md` §1 (Settling), `context/plans/large-map-spatial-residency.md` §Residency budget tiers · read at 053578f · evidence: `research.md`

## Problem

The owner asked whether a map that is valid in TrenchBroom can fail to run. Through streamed SH
pool growth, it can: an anticipated defect, confirmed in source, that no shipped map has hit yet.
A growth that would pass a device limit returns a drain error, and the frame loop treats any
drain error as fatal, in Settling and in play. The dense limit is reachable on a level whose SH
fits it, because the atlas's layer size follows the initial floor share rather than the level,
and growth doubles before it checks the limit (`research.md` §Reachability). When this is done,
no level ends a session through SH pool growth past a device limit, and a level whose dense set
fits the device is never refused for dense growth, except through fragmentation past the
headroom.

## Decisions

- **Dense layer size covers the whole level.** At load, tiles per layer are the larger of today's
  layer (sized from the planner's initial dense slot count) and the whole-level minimum (the
  whole dense set times a fragmentation headroom, divided by the layer limit and rounded up),
  capped at one full layer. Maps whose today's layer already suffices keep their geometry and
  upload cost; only levels that could reach the layer limit get a larger layer. The initial atlas
  keeps the planner's initial dense slot count, rounded up to whole layers. Geometry stays fixed
  for the session (`plans/done/sh-probe-streaming--cluster-residency`). The planner's copy of the
  geometry rule is deleted, so one rule sizes both. Reason: the 2026-09-28 rule in
  `large-map-spatial-residency.md`, "a miss fallback cannot stand in for residency".
- **Growth clamps to the device.** The dense limit is the array-layer limit; each sparse family's
  limit is its tightest buffer. At the compiler's default delta cap, sparse is reachable only past
  heavy sparse fragmentation (about 8× for the tile pool once growth clamps, less for the
  compaction table); the cap is a prl-build flag with no upper bound, so a raised cap reaches it
  sooner. Growth is capped at the limit and refused only when the needed size alone exceeds it.
  The limit verdict is taken before any retirement wait, in every family.
- **A device-limit refusal fails the cluster, not the session.** The renderer reports a refusal as
  its own outcome, distinct from contract errors. The app routes it through the existing
  per-cluster failure path: payload, ready bytes and permit released once, and a miss attributed
  to the device limit. Unlike a read failure, a device-limit failure never spends the cluster's
  single retry: every time it leaves its targets and re-enters, it is read again. The failure
  records its cause, so the latest failure decides the retry. One device-limit warning per level
  names the first refused cluster; a refusal logs no per-cluster read-failure warning.
- **Refusal handling adds no churn on camera turns.** PostRetro games turn fast, so nothing
  evicted for a refusal should need re-reading within a turn. A cluster whose failure record's
  latest cause is a refusal is retried at most once per hysteresis window (2 s, counted from its
  last refusal), however it leaves and re-enters its targets: through hysteresis, a seam-warm
  neighbour, or an owner's dependent. The pressure trim's candidate filter skips a cluster that
  is currently Failed with refusal as its cause, since it holds no room; a retried cluster that
  now holds room is trimmed normally.
- **A refusal evicts nothing.** The refused cluster waits for its retry; no resident, mandatory or
  optional, leaves to make room for it. Ordinary floor-budget pressure is unchanged, except that
  its trim skips refused clusters. This keeps the pin guarantee in
  `plans/done/sh-probe-streaming--authored-hints`. It amends §4's "never refused" clause for the
  mandatory tier, and supersedes authored-hints' "the named `GpuCapacity` error surfaces" for
  device limits.
- **Other drain errors stay fatal.** A chunk corrupted on disk already fails only its own
  cluster through the loader's hash and body checks (in async streaming; sync-proof mode treats
  a read error as fatal, unchanged here). What the renderer still reports as a drain
  error (a chunk that disagrees with renderer metadata, residency identity and generation
  violations,
  post-growth undersizing, missing device handles, slot-address overflow) is an engine bug, and
  surfaces by ending the session.
- **Placement: renderer mechanism, app policy.** The renderer owns geometry, growth and refusal
  (Renderer owns GPU); the app owns the failure handling and miss attribution.
- **This lands before `portal-walk-bounded-regions`**, whose superset visible set adds mandatory
  SH demand. Whole-level geometry bounds that demand: no visible set exceeds the whole level.
- **Non-goals:**
  - A prl-build check for per-cell sparse demand, and the shared sizing and mandatory-set
    derivation it would need. The runtime refusal covers a raised `--sh-delta-max-size` and
    sparse fragmentation without ending the session (`research.md` §Reachability).
  - A wake on eviction. A refused cluster retries only by leaving and re-entering its targets.
    Known edge: a pinned cluster never leaves its targets, so a refused pin stays unlit until the
    level reloads; only the level's first refusal is named in its warning.
  - The bake's dense cap is about 1.8× looser than the runtime atlas; it matters only past
    hundreds of GB.
  - Sparse fragmentation headroom, and sparse growth clamped to whole-level need: both are VRAM
    efficiency, not survival, since a sparse refusal no longer ends the session.
  - Early Settling release: a refused mandatory cluster holds the level until the existing
    timeout.
  - Capture preload: capture still requires a sampleable settle set, so capturing a level with a
    refused mandatory cluster fails as it does today; capture applies the new outcome without
    ending the session.
  - The 27-bit slot-address space as a growth limit: it binds only past about 200 GB of dense
    data; address overflow stays a fatal engine-bug guard.
  - Out-of-memory below the limits. It is a wgpu uncaptured error shared by every renderer
    allocation, so it needs a renderer-wide error-scope policy (`research.md` §Out of scope).
  - The load-time install panic. It is a deliberate fail-fast before play.
  - An engine VRAM ceiling or player tier: `large-map-spatial-residency` stage 5 owns it.
  - The lightmap pool: its layers are a fixed 2048², and it already defers.

## Acceptance

Pins are in `research.md` §Ordering pins.

### Automated
- [ ] Through the production load path, with the floor's pool budget forced to zero, a fixture
  whose whole dense set exceeds the layer limit times its largest writer closure grows until
  every cluster is resident at once, with no refusal. The test asserts that precondition, so it
  fails if layer size still follows the initial share.
- [ ] At load, tiles per layer equal the larger of today's floor-share layer and the whole-level
  minimum (whole dense set times the headroom divided by the layer limit, rounded up), so the
  layer limit's worth of layers holds the whole level times the headroom. On a fixture where
  today's layer is larger, geometry is unchanged from the pre-change build; on one where the
  minimum is larger, the layer grows to it; a small fixture's layer holds at least one tile. The
  initial atlas is the planner's initial dense slot count rounded up to whole layers, never more
  layers than the limit. Past one full layer's worth, the layer is full and the cap holds. In
  every case, and at zero share, the planner's padded slot count, charged floor bytes and
  largest-closure dense minimum equal the GPU atlas's for the same level and limits, from the
  one geometry rule.
- [ ] Atlas width, height and tiles per layer are identical across every growth in a session, the
  clamped growth to the limit included.
- [ ] A dense pool at 200 layers, limit 256, needing 201 grows to 256 and installs. One needing
  257 is refused without growing. Below the limit, growth is unchanged: 100 layers needing 101
  grow to 200.
- [ ] A sparse family whose doubled buffer would pass the binding limit, but whose needed size
  fits, grows to the limit and installs. One whose needed size exceeds its tightest buffer is
  refused.
- [ ] A growth whose needed size exceeds the limit while that family still retires a previous
  generation is refused as device-limited, in the dense atlas and in every sparse family. One
  that fits but waits on retirement defers as retirement pressure (P4).
- [ ] A cluster whose dense growth fits but whose sparse growth is refused keeps the grown atlas,
  is refused, and leaves the pool sampling correctly. A later cluster in that drain that needs
  further dense growth within the limit defers as retirement pressure; one whose need exceeds the
  limit is refused (P4). When one sparse family of several is refused,
  none of them grows (P3, P9).
- [ ] In the same drain, a cluster that fits installs while another is refused, in either order,
  and the refused cluster leaves no address behind (P1).
- [ ] With the limit lowered, a renderer drain whose demand exceeds it returns a device-limit
  refusal, not an error. Fed that outcome, the controller marks the cluster failed and releases
  its payload, ready bytes and permit once. The permit count afterwards equals reads in flight
  plus ready clusters, and the ready-byte ledger equals the ready clusters' charges (P5).
- [ ] A visible episode that begins while a cluster, or an owner in its closure, is failed by a
  device-limit refusal counts one miss in the device-limit bucket. A cluster refused partway
  through a visible episode that already counted a miss counts nothing more, and that miss keeps
  its earlier bucket; the bucket total still equals the miss count (P10).
- [ ] A cluster refused in the reveal frame's own drain keeps the miss the reveal frame counted for
  it, adds no second miss, and logs the level's device-limit warning only if none was logged
  before (P11).
- [ ] A refused visible cluster that leaves view and returns within the hysteresis window stays
  failed. One that left its targets and re-enters on a later horizon change is requested again
  in that frame's request pass when a permit for its class is free and pressure does not trim it
  in that drain. A cluster refused, retried and refused again is retried again on its next leave
  and re-entry, any number of times (P12, P14).
- [ ] A cluster refused, retried, whose re-read then fails the loader's hash check is read once more
  on its next leave and re-entry; if that read fails again, it is not read again. A cluster whose
  read failed, was retried and was then refused, retried and failed its read again is not read
  again. In both sequences, every refusal along the way is retried on leave and re-entry (P14,
  P17).
- [ ] With the camera changing the horizon every frame for several seconds, under floor-budget
  pressure, each of these refused clusters is read again at most once per 2 s: one inside the
  horizon, one targeted only as a seam-warm neighbour whose near side flickers in and out of view,
  and an owner targeted only through a dependent that pressure suppresses. A cluster Failed by a
  refusal is never a trim candidate, and the trim skip adds no refused optional cluster to the
  overshoot gauge (P19).
- [ ] A refused cluster that stays targeted is not read again when a later drain evicts other
  clusters and frees room; only leaving and re-entering its targets reads it again (P16).
- [ ] While an owner is failed by a refusal, none of its dependents is requested, holds a permit or
  holds ready bytes (P13). When a closure holds both a read-failed and a device-limit-failed
  owner, the episode's one miss lands in the device-limit bucket whatever the owners' order.
- [ ] Two clusters refused in one level log exactly one device-limit warning between them, naming
  the first, and no per-cluster read-failure warning. A cluster refused and retried whose re-read
  then fails the loader's hash check still logs its read-failure warning once. The settle-timeout
  warning, when the level times out, is separate and unchanged.
- [ ] A level whose mandatory cluster is refused during Settling stays unsettled until the timeout
  reveals it into Running; a refused optional cluster does not hold Settling. The session never
  ends. One device-limit warning is logged per level, across Settling, reveal and SH session
  replacement; the next level's first refusal warns again (P6, P7, P15).
- [ ] With the floor budget covering every target, the refused cluster included, so that pressure
  trims nothing, and with a pinned cluster, a lead cluster, a visible cluster's owner and an
  optional resident loaded, and a refused visible cluster that would fit if any one of them left,
  the controller's batches after the refusal request none of them for eviction (P18), and a renderer drain that refuses
  a cluster evicts nothing the batch did not request. All four stay sampleable.
- [ ] Regression guards: a chunk that passes the loader's hash and body checks but disagrees with
  renderer metadata still ends the session with its named error, and a retirement-pressure
  deferral behaves as before.
- [ ] A capacity-class error that is a broken contract rather than a device limit, such as an
  install reaching the pools without device handles or a post-growth undersized pool, still
  ends the session, as do identity, generation and slot-address-overflow violations.
- [ ] An outcome that names a cluster as both refused and accepted, dropped or deferred, refuses a
  cluster twice, refuses a cluster the drain did not carry, or omits a drained cluster from every
  list is rejected as invalid before any state changes.
- [ ] The pool-floor planner holds no tiles-per-layer derivation of its own: a search of the SH
  floor planner finds no row, layer or tiles-per-layer computation, and its slot count comes from
  the atlas's geometry rule.
- [ ] A seeded churn walk at the whole-level-minimum geometry, with a fixed seed set, a fixed step
  count, cluster sizes drawn from a named fixture level, random eviction order, and live demand
  that reaches the whole dense set on every seed, is never refused at the chosen headroom. The plan of record reports the
  largest factor that was refused.
- [ ] Test seam: SH state reads the device's limits once, at load; the load-time geometry rule, the
  floor planner, every dense and sparse growth check and every sparse replacement validation use
  them, so a test override below the device's grant governs all of them. None of these reads the
  device's limits directly.
- [ ] The final gate runs the refusal and growth tests with a GPU required, and they report as
  run, not skipped. Their lowered limits sit at or below the test device's own grants.

### Manual
- [ ] At default limits, campaign-test and the stress-warren hallway log no device-limit warning.
  Report their initial dense atlas bytes, tiles per layer, pool growth counts, upload copy regions
  per install and install CPU time against the pre-change build.

## Path

- Seams: `AtlasShape::for_slots` / `grown_for_slots`, `atlas_capacity_slots` (`floor.rs`, the
  planner's copy of the geometry rule), `plan_initial_pool_floor`, `whole_dense_slots`,
  `grow_dense` / `grow_sparse`, `validate_storage_buffer_size` (both copies: `gpu.rs`, which
  reads device limits, and `floor/validation.rs`), `install_ready`,
  `ShDrainOutcome`. The device-limit and contract split of `GpuCapacity` is in `research.md`
  §Premise review notes (round 1).
- Failure path: `mark_failed` and `FailureState` in the app controller. `admit_failed_request`
  acts only on Queued clusters, so a refused (Ready, drain-carried) cluster needs its own entry.
  `mark_failed` releases the permit but not the ready-byte charge, carries `retry_spent` forward
  for the same identity, and warns per chunk identity; `transition_failed_retries` spends the
  retry. The refusal path releases both, records the failure's cause so the transition spends
  nothing on a device-limit retry, and uses the level-scoped device-limit latch instead.
- Retry rate limit: key it on the failure record's latest cause and the time of its last
  refusal; the record survives the retry. Key the trim skip on the cluster's current state
  (Failed) plus refusal as the cause, so a retried cluster that now holds room is trimmed
  normally. The trim skip belongs in the trim's candidate filter only, not in `pressure_evictable_optional`, which would
  count refused clusters as protected demand and inflate the overshoot gauge.
- Warnings: `mark_failed` warns only when no earlier failure of the same identity exists, so a
  recorded refusal would silence a later read-failure warning. Key that warning on an earlier
  read failure, not on any failure.
- New miss bucket consumers read the buckets field by field: the renderer's
  `debug_ui/streaming_tab.rs`, `capture/report.rs`, and the periodic log in
  `session/sh_streaming_diagnostics.rs`. Add the device-limit bucket to all three.
- `grow_sparse` stops at the first family waiting on retirement before later families' limits
  are checked; take every family's limit verdict first.
- Refuse before growing whatever cannot fit. A dense growth kept while the sparse half is refused
  is valid, per the install's own comment.
- Tests: SH state captures its limits once at load and tests override them, as the lightmap plan
  does (the test seam for the clamp rows, not a product rule); the harness requires a GPU through `POSTRETRO_REQUIRE_GPU`. Cross-crate rows split at the
  drain outcome: a renderer GPU test records the real outcome, and a controller test is fed it.
- Refusal precedent: lightmap `place_with_growth` / `device_limited`, `warn_device_limit_once`.
- Rivals: `research.md` §Rivals.
- Durable capture at promotion: §4 gains the session-fixed dense geometry (larger of floor-share
  and whole-level layer) and the mandatory row gains the device-limit refusal; §10's runtime-guard
  sentence covers streamed SH and its clamp.
- First slice: the geometry rule plus the zero-share fixture.

## Open questions
- Fragmentation headroom factor — **delegated**: the smallest factor the churn-walk row accepts,
  times a stated margin of at least 1.5×, since dense fragmentation only grows within a
  generation; the plan of record states both numbers.
