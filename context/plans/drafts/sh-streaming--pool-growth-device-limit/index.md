# sh-streaming--pool-growth-device-limit

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Cluster SH residency), §10, `context/lib/boot_sequence.md` §1 (Settling), `context/plans/large-map-spatial-residency.md` §Residency budget tiers · read at 0e1f3d1 · evidence: `research.md`

## Problem

The owner asked whether a map that is valid in TrenchBroom can fail to run. Through streamed SH
pool growth, it can: an anticipated defect, confirmed in source, that no shipped map has hit yet.
A growth that would pass a device limit returns a drain error, and the frame loop treats any
drain error as fatal, in Settling and in play. The dense limit is reachable on a level whose SH
fits it, because the atlas's layer size follows the initial floor share rather than the level,
and growth doubles before it checks the limit (`research.md` §Reachability). When this is done,
no level ends a session through SH pool growth past a device limit.

## Decisions

- **Dense layer size covers the whole level.** At load, tiles per layer hold the whole dense set
  times a fragmentation headroom within the dense limit, capped at one full layer, and stay fixed
  for the session (`plans/done/sh-probe-streaming--cluster-residency`). The planner's copy of the
  geometry rule is deleted, so one rule sizes both. Reason: the 2026-09-28 rule in
  `large-map-spatial-residency.md`, "a miss fallback cannot stand in for residency".
- **Growth clamps to the device.** The dense limit is the smaller of the array-layer limit and
  the slot address space; each sparse family's limit is its tightest buffer. Growth is capped at
  the limit and refused only when the needed size alone exceeds it. The limit verdict is taken
  before any retirement wait, in every family.
- **A device-limit refusal fails the cluster, not the session.** The renderer reports a refusal as
  its own outcome, distinct from contract errors. The app routes it through the existing
  per-cluster failure path: payload, ready bytes and permit released once, a miss attributed to
  the device limit, one warning per level, and the existing retry when the cluster leaves and
  re-enters its targets or the generation changes.
- **Mandatory demand is never evicted to make room.** This keeps §4's tier contract and the pin
  guarantee in `plans/done/sh-probe-streaming--authored-hints`.
- **Other drain errors stay fatal.** A chunk corrupted on disk already fails only its own
  cluster through the loader's hash and body checks. What the renderer still reports as a drain
  error (a chunk that disagrees with renderer metadata, identity and generation violations,
  post-growth undersizing, missing device handles, address overflow) is an engine bug, and
  surfaces by ending the session.
- **Placement: renderer mechanism, app policy.** The renderer owns geometry, growth and refusal
  (Renderer owns GPU); the app owns the failure handling and miss attribution.
- **This lands before `portal-walk-bounded-regions`**, whose superset visible set adds mandatory
  SH demand. Whole-level geometry bounds that demand: no visible set exceeds the whole level.
- **Non-goals:**
  - A prl-build check for per-cell sparse demand, and the shared sizing and mandatory-set
    derivation it would need. The sparse limit is unreachable on any streamed level: the loader
    turns streaming off for a delta section over 128 MiB, and the compiler caps the delta
    payload at 256 MiB, both far below the requested binding (`research.md` §Reachability).
    Reopen if either cap rises above the granted binding.
  - Per-family wake on eviction. The only reachable refusal is dense fragmentation past the
    headroom, which the churn-walk row bounds. Known edge: a pinned cluster refused that way
    never leaves its targets, so it stays unlit for the level; its warning names it.
  - Sparse fragmentation headroom: the runtime compares the allocator high-water.
  - Early Settling release: a refused mandatory cluster holds the level until the existing
    timeout.
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
  grow to 200. The growth rule alone, given a layer size at which 256 layers pass the slot
  address space, stops at the address bound and refuses a cluster needing more, without
  allocating.
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
  refusal, not an error. Fed that outcome, the controller marks the cluster failed, releases its
  payload, ready bytes and permit once, and counts its visible miss as a device-limit miss;
  a dependent waiting on it counts the same. The permit count afterwards equals reads in
  flight plus ready clusters, and the ready-byte ledger equals the ready clusters' charges. The
  cluster is read again after it leaves and re-enters its targets (P5).
- [ ] A level refused during Settling stays unsettled until the timeout reveals it into Running;
  the session never ends. One device-limit warning is logged per level, across Settling, reveal
  and generation changes; the next level's first refusal warns again (P6, P7).
- [ ] With a pinned cluster, a lead cluster and a visible cluster's owner resident, and a refused
  visible cluster that would fit if any one of them left, every drain keeps all three
  sampleable and returns no error.
- [ ] Regression guards: a chunk that passes the loader's hash and body checks but disagrees with
  renderer metadata still ends the session with its named error, and a retirement-pressure
  deferral behaves as before.
- [ ] A capacity-class error that is a broken contract rather than a device limit, such as an
  install reaching the pools without device handles or a post-growth undersized pool, still
  ends the session, as do identity and generation violations. A cluster that needs slots past
  the address bound is refused before it allocates and never reaches the address-overflow
  error; an address overflow raised anyway still ends the session.
- [ ] An outcome that names a cluster as both refused and accepted, or that refuses a cluster the
  drain did not carry, is rejected as invalid before any state changes.
- [ ] A seeded churn walk with a fixed seed set, cluster sizes spanning the level's range and
  random eviction order, at the zero-share geometry, is never refused at the chosen headroom;
  one step smaller is refused on at least one seed.
- [ ] SH state reads the device's limits once, at load; every dense and sparse growth check and
  sparse replacement validation uses them, so a test override below the device's grant governs
  all of them. A search of the SH streaming code finds no other read of the device's limits.
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
- Failure path: `admit_failed_request`, `mark_failed` and `FailureState` in the app controller.
  `mark_failed` releases the permit but not a drain-carried cluster's ready-byte charge; the
  refusal path must release both.
- `grow_sparse` stops at the first family waiting on retirement before later families' limits
  are checked; take every family's limit verdict first.
- Refuse before growing whatever cannot fit. A dense growth kept while the sparse half is refused
  is valid, per the install's own comment.
- Tests: SH state captures its limits once at load and tests override them, as the lightmap plan
  does; the harness requires a GPU through `POSTRETRO_REQUIRE_GPU`. Cross-crate rows split at the
  drain outcome: a renderer GPU test records the real outcome, and a controller test is fed it.
- Refusal precedent: lightmap `place_with_growth` / `device_limited`, `warn_device_limit_once`.
- Rivals: `research.md` §Rivals.
- Durable capture at promotion: §4's mandatory row gains the device-limit refusal; §10's
  runtime-guard sentence covers streamed SH and its clamp.
- First slice: the geometry rule plus the zero-share fixture.

## Open questions
- Fragmentation headroom factor — **delegated**: the smallest factor the churn-walk row accepts.
