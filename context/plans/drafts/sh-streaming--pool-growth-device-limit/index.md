# sh-streaming--pool-growth-device-limit

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Cluster SH residency), §10, `context/lib/boot_sequence.md` §1 (Settling), `context/plans/large-map-spatial-residency.md` §Residency budget tiers · read at c78b4eb · evidence: `research.md`

## Problem

The owner asked whether a map that is valid in TrenchBroom can fail to run. Through streamed SH
pool growth, it can: an anticipated defect, confirmed in source, that no shipped map has hit yet.
A growth that would pass a device limit returns a drain error, and the frame loop treats any
drain error as fatal, in Settling and in play. The dense limit is reachable on a level whose SH
fits it, because the atlas's layer size follows the initial floor share rather than the level,
and growth doubles before it checks the limit (`research.md` §Reachability). When this is done,
no level ends a session through SH pool growth past a device limit, and a level whose dense set
fits the device is never refused, except through fragmentation past the headroom.

## Decisions

- **Dense layer size covers the whole level.** At load, tiles per layer hold the whole dense set
  times a fragmentation headroom within the dense limit, capped at one full layer, and stay fixed
  for the session (`plans/done/sh-probe-streaming--cluster-residency`). The planner's copy of the
  geometry rule is deleted, so one rule sizes both. Reason: the 2026-09-28 rule in
  `large-map-spatial-residency.md`, "a miss fallback cannot stand in for residency".
- **Growth clamps to the device.** The dense limit is the array-layer limit; each sparse family's
  limit is its tightest buffer. Sparse is unreachable at the compiler's default delta cap, but
  the cap is a prl-build flag with no upper bound, so a raised cap or sparse fragmentation can
  reach it. Growth is capped at
  the limit and refused only when the needed size alone exceeds it. The limit verdict is taken
  before any retirement wait, in every family.
- **A device-limit refusal fails the cluster, not the session.** The renderer reports a refusal as
  its own outcome, distinct from contract errors. The app routes it through the existing
  per-cluster failure path: payload, ready bytes and permit released once, and a miss attributed
  to the device limit. Unlike a read failure, a device-limit failure never spends the cluster's
  single retry: every time it leaves its targets and re-enters, it is read again, so a transient
  loss of room never leaves it unlit for the rest of the level. One device-limit warning per
  level names the first refused cluster; a refusal logs no per-cluster read-failure warning.
- **A refusal evicts nothing.** The refused cluster waits for its retry; no resident, mandatory or
  optional, leaves to make room. This keeps the pin guarantee in
  `plans/done/sh-probe-streaming--authored-hints` and amends only §4's "never refused" clause for
  the mandatory tier.
- **Other drain errors stay fatal.** A chunk corrupted on disk already fails only its own
  cluster through the loader's hash and body checks. What the renderer still reports as a drain
  error (a chunk that disagrees with renderer metadata, identity and generation violations,
  post-growth undersizing, missing device handles, slot-address overflow) is an engine bug, and
  surfaces by ending the session.
- **Placement: renderer mechanism, app policy.** The renderer owns geometry, growth and refusal
  (Renderer owns GPU); the app owns the failure handling and miss attribution.
- **This lands before `portal-walk-bounded-regions`**, whose superset visible set adds mandatory
  SH demand. Whole-level geometry bounds that demand: no visible set exceeds the whole level.
- **Non-goals:**
  - A prl-build check for per-cell sparse demand, and the shared sizing and mandatory-set
    derivation it would need. At the compiler's default 256 MiB delta cap the sparse limit is
    unreachable; the runtime refusal covers a raised `--sh-delta-max-size` and sparse
    fragmentation without ending the session (`research.md` §Reachability).
  - A wake on eviction. A refused cluster retries only by leaving and re-entering its targets.
    Known edge: a pinned cluster never leaves its targets, so a refused pin stays unlit until the
    level reloads; only the level's first refusal is named in its warning.
  - Sparse fragmentation headroom, and sparse growth clamped to whole-level need: both are VRAM
    efficiency, not survival, since a sparse refusal no longer ends the session.
  - Early Settling release: a refused mandatory cluster holds the level until the existing
    timeout.
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
- [ ] At load, tiles per layer hold the whole dense set times the headroom within the dense limit.
  Past one full layer's worth, the layer is full and the cap holds. In both cases, and at zero
  share, the planner's slot count, charged floor bytes and largest-closure dense minimum equal
  the GPU atlas's for the same level and limits, from the one geometry rule.
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
  is refused, and leaves the pool sampling correctly. A later cluster in that drain needing
  dense growth defers as retirement pressure. When one sparse family of several is refused,
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
  in that frame's request pass. A cluster refused, retried and refused again is retried again on
  its next leave and re-entry, any number of times; a read failure keeps its single retry, and a
  device-limit retry does not spend it (P12, P14).
- [ ] A refused cluster that stays targeted is not read again when a later drain evicts other
  clusters and frees room; only leaving and re-entering its targets reads it again (P16).
- [ ] While an owner is failed by a refusal, none of its dependents is requested, holds a permit or
  holds ready bytes (P13).
- [ ] Two clusters refused in one level log exactly one device-limit warning between them, naming
  the first, and no per-cluster read-failure warning. A cluster refused and retried whose re-read
  then fails the loader's hash check still logs its read-failure warning once. The settle-timeout
  warning, when the level times out, is separate and unchanged.
- [ ] A level whose mandatory cluster is refused during Settling stays unsettled until the timeout
  reveals it into Running; a refused optional cluster does not hold Settling. The session never
  ends. One device-limit warning is logged per level, across Settling, reveal and SH session
  replacement; the next level's first refusal warns again (P6, P7, P15).
- [ ] With a pinned cluster, a lead cluster, a visible cluster's owner and an optional resident
  loaded, and a refused visible cluster that would fit if any one of them left, the controller's
  batches after the refusal request none of them for eviction, and a renderer drain that refuses
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
- [ ] A seeded churn walk at the zero-share geometry, with a fixed seed set, cluster sizes drawn
  from a named fixture level, random eviction order, and live demand that reaches the whole dense
  set on every seed, is never refused at the chosen headroom. The plan of record reports the
  largest factor that was refused.
- [ ] SH state reads the device's limits once, at load; every dense and sparse growth check and
  sparse replacement validation uses them, so a test override below the device's grant governs
  all of them. No growth or replacement check reads the device's limits directly.
- [ ] The final gate runs the refusal and growth tests with a GPU required, and they report as
  run, not skipped. Their lowered limits sit at or below the test device's own grants.

### Manual
- [ ] At default limits, campaign-test and the stress-warren hallway log no device-limit warning.
  Report their initial dense atlas bytes and pool growth counts against the pre-change build.

## Path

- Seams: `AtlasShape::for_slots` / `grown_for_slots`, `atlas_capacity_slots` (`floor.rs`, the
  planner's copy of the geometry rule), `plan_initial_pool_floor`, `whole_dense_slots`,
  `grow_dense` / `grow_sparse`, `validate_storage_buffer_size`, `install_ready`,
  `ShDrainOutcome`. The device-limit and contract split of `GpuCapacity` is in `research.md`
  §Premise review notes (round 1).
- Failure path: `mark_failed` and `FailureState` in the app controller. `admit_failed_request`
  acts only on Queued clusters, so a refused (Ready, drain-carried) cluster needs its own entry.
  `mark_failed` releases the permit but not the ready-byte charge, and spends `retry_spent` and
  warns per chunk identity; the refusal path releases both, leaves the retry unspent, and uses the
  level-scoped device-limit latch instead.
- `grow_sparse` stops at the first family waiting on retirement before later families' limits
  are checked; take every family's limit verdict first.
- Refuse before growing whatever cannot fit. A dense growth kept while the sparse half is refused
  is valid, per the install's own comment.
- Tests: SH state captures its limits once at load and tests override them, as the lightmap plan
  does (the test seam for the clamp rows, not a product rule); the harness requires a GPU through `POSTRETRO_REQUIRE_GPU`. Cross-crate rows split at the
  drain outcome: a renderer GPU test records the real outcome, and a controller test is fed it.
- Refusal precedent: lightmap `place_with_growth` / `device_limited`, `warn_device_limit_once`.
- Rivals: `research.md` §Rivals.
- Durable capture at promotion: §4's mandatory row gains the device-limit refusal; §10's
  runtime-guard sentence covers streamed SH and its clamp.
- First slice: the geometry rule plus the zero-share fixture.

## Open questions
- Fragmentation headroom factor — **delegated**: the smallest factor the churn-walk row accepts.
