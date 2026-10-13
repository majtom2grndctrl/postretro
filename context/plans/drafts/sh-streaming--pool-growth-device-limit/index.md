# sh-streaming--pool-growth-device-limit

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Cluster SH residency, Lightmap cell-block residency), §10, `context/lib/boot_sequence.md` §1 (Settling), `context/plans/large-map-spatial-residency.md` §Residency budget tiers · read at a83e186 · evidence: `research.md`

## Problem

The owner asked whether a map that is valid in TrenchBroom can fail to run. Through streamed SH
pool growth, it can: an anticipated defect, confirmed in source, that no shipped map has hit yet.
A growth that would pass a device limit returns a drain error, and the frame loop treats any
drain error as fatal, in Settling and in play. The limits are reachable on a level whose SH fits
them, because the dense atlas's layer size follows the initial floor share rather than the level,
and growth doubles before it checks the limit (`research.md` §Reachability). When this is done,
no level that compiles ends a session through SH pool growth past a device limit.

## Decisions

- **Dense layer size covers the whole level.** At load, tiles per layer hold the whole dense set
  times a fragmentation headroom within the dense limit, capped at one full layer, and stay fixed
  for the session (`plans/done/sh-probe-streaming--cluster-residency`). Reason: the 2026-09-28
  rule in `large-map-spatial-residency.md`, "a miss fallback cannot stand in for residency".
- **Growth clamps to the device.** The dense limit is the smaller of the array-layer limit and
  the slot address space; each sparse family's limit is its tightest buffer. Growth is capped at
  the limit and refused only when the needed size alone exceeds it.
- **A refused cluster drops and is requested again on a wake.** A device-limit refusal is its own
  outcome, distinct from contract errors. The cluster leaves through the same path as a target
  departure, releasing its payload and permit once, and is marked device-limited. It wakes only
  on an eviction confirmed in its own pool family after the refusal, or on entering the
  mandatory tier. It counts as an attributed miss and warns once per level. This diverges from the
  cluster-residency plan's "named capacity error", which never chose to end the session
  (`research.md` §Commitments touched).
- **A mandatory refusal makes non-mandatory residents yield.** While a mandatory cluster is
  refused at the device limit, every non-mandatory resident, hysteresis and seam-warm included,
  is eligible for the existing pressure trim. One rule on the existing trim decides what gives
  way. A level that passes the build check is then refused for mandatory demand inside its
  baked set only through fragmentation past the headroom.
- **Mandatory demand is never evicted to make room.** This keeps §4's tier contract and the pin
  guarantee in `plans/done/sh-probe-streaming--authored-hints`.
- **Other drain errors stay fatal.** A chunk corrupted on disk already fails only its own
  cluster: the loader's hash check fails the read. What the renderer still reports as a drain
  error (a malformed chunk that passed the hash, identity and generation violations, post-growth
  undersizing, missing device handles, address overflow) is an engine bug, and surfaces by ending
  the session.
- **Placement: renderer mechanism, app policy.** The renderer owns geometry, growth and refusal
  (Renderer owns GPU). The app owns the re-request policy and miss attribution, driven by the
  drain outcome.
- **Limits and sizing rules live once.** The device limits the renderer requests, and each
  sparse family's per-cluster sizing, are shared definitions in `level-format`, named as device
  requests rather than SH limits. The mandatory-set derivation (id-51 reach, pin expansion,
  owner closure) lives in `level-loader`, which both prl-build and the runtime already use. The
  renderer, the app and prl-build call the same code, so the build check cannot drift.
- **prl-build rejects a camera cell whose baked mandatory sparse set cannot fit.** Each sparse
  family must fit its tightest buffer. The error names the cell and the family. Dense is not
  checked: whole-level layer sizing already holds any level whose dense set times the headroom
  fits the dense limit. Whole-level
  size never rejects, since `large-map-spatial-residency` bounds map scale by disk and residency
  budget. A streamed level without a usable id 51 warns and relies on runtime deferral.
- **This lands before `portal-walk-bounded-regions`**, whose superset visible set adds
  never-refused SH demand.
- **Non-goals:**
  - Out-of-memory below the limits. It is a wgpu uncaptured error shared by every renderer
    allocation, so it needs a renderer-wide error-scope policy (`research.md` §Out of scope).
  - The load-time install panic. It is a deliberate fail-fast before play.
  - An engine VRAM ceiling or player tier: `large-map-spatial-residency` stage 5 owns it.
  - Capping sparse growth at whole-level need. That is VRAM efficiency.
  - The lightmap pool: its layers are a fixed 2048², and it already defers.
  - Early Settling release: a mandatory cluster that is Failed or device-limited holds the level
    until the existing timeout, which already bounds it.
  - Sparse fragmentation headroom: the build check charges minimum need; the runtime compares the
    allocator high-water.

## Acceptance

Pins are in `research.md` §Ordering pins.

### Automated
- [ ] Through the production load path, with the floor's pool budget forced to zero, a fixture
  whose whole dense set exceeds the layer limit times its largest writer closure grows until
  every cluster is resident at once, with no refusal. The test asserts that precondition, so
  it fails if layer size still follows the initial share.
- [ ] At load, tiles per layer hold the whole dense set times the headroom within the dense limit.
  Past one full layer's worth, the layer is full and the cap holds. In both cases, and at zero
  share, the planner's physical slot count, charged floor bytes and largest-closure dense
  minimum equal the GPU atlas's for the same level and limits.
- [ ] Atlas width, height and tiles per layer are identical across every growth in a session, the
  clamped growth to the limit included.
- [ ] A dense pool at 200 layers, limit 256, needing 201 grows to 256 and installs. One needing
  257 is refused without growing. Below the limit, growth is unchanged: 100 layers needing 101
  grow to 200. The growth rule alone, given a layer size at which 256 layers pass the slot
  address space, stops at the address bound and refuses a cluster needing more, without
  allocating. That bound is the same definition the indirection word encoder checks.
- [ ] A sparse family whose doubled buffer would pass the binding limit, but whose needed size
  fits, grows to the limit and installs. One whose needed size exceeds its tightest buffer is
  refused.
- [ ] A growth whose needed size exceeds the limit while that family still retires a previous
  generation is refused as device-limited, in the dense atlas and in a sparse family alike.
  One that fits but waits on retirement defers as retirement pressure (P4).
- [ ] A cluster whose dense growth fits but whose sparse growth is refused keeps the grown atlas,
  is refused as device-limited, and leaves the pool sampling correctly. A later cluster in that
  drain needing dense growth defers as retirement pressure. When one sparse family of several is
  refused, none of them grows (P3, P9).
- [ ] With the limit lowered, a renderer drain whose demand exceeds it returns a device-limited
  refusal, not an error. A level whose mandatory set cannot fit stays unsettled until Settling
  releases at its timeout into Running.
- [ ] In the same drain, a cluster that fits installs while another is refused, in either order,
  and the refused cluster leaves no address behind (P1).
- [ ] A refused cluster releases its payload and permit exactly once, through the departure path;
  the permit count afterwards equals reads in flight plus ready clusters. It is not requested
  again in the drain that refused it, even if that drain also confirmed an eviction. After a
  later drain confirms an eviction in its own pool family, it is requested again, reads again and
  installs once room holds it; an eviction in another family does not wake it. Separately, with
  no eviction, entering the mandatory tier requests it again. Through refusal and re-request, the
  ready-byte ledger equals the ready clusters' charges (P2, P11, P12). The proof is split at the drain outcome: a GPU renderer test records the real outcomes of the refusing and confirming drains, and a controller test is fed exactly those outcomes.
- [ ] A cluster refused at the class it gained in the same frame is not requested again until a
  later wake. A re-requested cluster refused again when its read lands is not requested again
  because of evictions confirmed while that read was in flight. When two wakes arrive in the same
  frame, it is requested once (P21).
- [ ] With a device-limited owner and a targeted dependent of it, no request names either cluster
  until the owner wakes. The owner is then requested first (P20).
- [ ] A device-limited cluster that falls back to hysteresis keeps its mark and is not read again
  during its hysteresis window. Its mark clears once it leaves the target set (P22).
- [ ] With more device-limited mandatory clusters than read permits, and an eviction confirmed in
  another family on every drain, reads for clusters that fit keep issuing every frame, and
  fitting SH clusters and lightmap blocks keep installing.
- [ ] A renderer that refuses a cluster keeps none of its payload. A later drain that confirms
  evictions but does not carry the cluster again installs nothing for it. The renderer installs
  a refused cluster only when the controller hands it over again.
- [ ] Unloading a level with device-limited clusters leaves no marks behind, and the next level
  starts with every permit free. A re-request read still in flight at unload completes into the
  next level as stale and takes none of its permits (P8, P25).
- [ ] A cluster whose target departs in the frame whose drain would refuse it is dropped as
  departed, never refused; it carries no mark and logs no warning (P26).
- [ ] An outcome that names a cluster as both refused and accepted, or that refuses a cluster the
  drain did not carry, is rejected as invalid before any state changes.
- [ ] With a pinned cluster, a lead cluster and a visible cluster's owner resident, and a refused
  visible cluster that would fit if any one of them left, every drain over the refusal keeps all
  three sampleable and returns no error.
- [ ] After a camera cell change, with the previous cell's set in hysteresis and seam-warm residents
  holding room, a refused mandatory cluster makes them eligible for the pressure trim: the next
  drain evicts them, the confirmed evictions wake the refused cluster, and it installs. No
  mandatory resident leaves, and a refused optional cluster makes nothing eligible. A level that
  passed the per-cell check is refused for mandatory demand inside its baked set only through
  fragmentation past the headroom (P10, P23). The proof is split at the drain outcome: a GPU renderer test records the real outcomes of the refusing and confirming drains, and a controller test is fed exactly those outcomes.
- [ ] Fed a drain outcome that marks one refusal device-limited and one deferral
  retirement-limited, the controller counts the first visible miss as a device-limit miss and
  the second as held by the drain budget. A visible dependent waiting on a device-limited owner
  also counts as a device-limit miss. The mark clears when the cluster installs or leaves its
  targets. A cluster refused again after a re-request keeps its mark and does not warn again
  (P5).
- [ ] A level refused during Settling and again after its timeout reveal logs one device-limit
  warning. Settling counts no device-limit miss, and the reveal frame counts one per
  device-limited visible cluster. The next level's first refusal warns again (P6, P7).
- [ ] Regression guards: a malformed chunk that passed the loader's hash still ends the session with
  its named error, and a retirement-pressure deferral behaves as before.
- [ ] A capacity-class error that is a broken contract rather than a device limit, such as an
  install reaching the pools without device handles or a post-growth undersized pool, still ends
  the session with its named error, as do identity and generation violations. A cluster that
  needs slots past the address bound is refused before it allocates and never reaches the
  address-overflow error. An address overflow raised anyway still ends the session.
- [ ] A scripted churn walk that installs and evicts clusters of mixed sizes, at the zero-share
  geometry and the chosen headroom, is never refused while its live dense set stays within the
  whole level.
- [ ] SH state reads the device's limits once, at load. After that, every dense and sparse growth
  check and every sparse replacement's validation uses the captured limits, so a test override
  below the device's grant governs all of them. A search of the SH streaming code finds no other
  read of the device's limits.
- [ ] The device limits and each sparse family's per-cluster sizing are defined once, in
  `level-format`, and the mandatory-set derivation once, in `level-loader`. A search of the
  renderer, the app and the level compiler finds no second definition. The renderer's device
  request and prl-build's per-cell check read them from there. SH growth checks read the
  device's grant, captured at load, which equals that request outside tests.
- [ ] The shared limits are named for the renderer's device request, not for SH. The renderer's
  device request, the lightmap pool's layer checks and the lightmap dry run's runtime layer floor
  read the same definitions.
- [ ] For every camera cell of a fixture with pins and owner chains, the runtime's targets with lead
  at the baked maximum, nothing visible and no hysteresis equal the set prl-build's per-cell
  check charges for that cell.
- [ ] The per-cell check takes its buffer and binding limits as inputs and charges only the sparse
  families. With limits lowered for the test, a small fixture whose one camera cell's mandatory
  set (reach at the baked maximum lead, pins, owner closure) passes a sparse family's tightest
  buffer fails prl-build naming that cell and family. A fixture whose whole level passes the
  lowered limit in that family while every cell's set fits compiles. A fixture whose dense set
  alone is large compiles, and its error output names no dense family. A rejected map writes no
  output, and a rebuild with a warm cache fails the same way (P19).
- [ ] A streamed fixture that bakes no usable id-51 set compiles. The build logs one warning that
  its camera cells go unchecked and that runtime deferral backs it. A fixture with a usable set,
  or one that streams no SH, logs no such warning (P18).
- [ ] The final gate runs the refusal and growth tests with a GPU required, and they report as
  run, not skipped. Their lowered limits sit at or below the test device's own grants.

### Manual
- [ ] At default limits, campaign-test and the stress-warren hallway log no device-limit warning.
  Report their initial dense atlas bytes and pool growth counts against the pre-change build.
- [ ] The SDK's level-design Compilation Errors table lists the new error by its leading words as
  prl-build prints them, with its cause and a fix that names only map edits and prl-build flags.
  The pinned-volume section notes that a pin counts in every camera cell's set.

## Path

- Seams: `AtlasShape::for_slots` / `grown_for_slots`, `atlas_capacity_slots` (`floor.rs`, a
  second copy of the geometry rule), `plan_initial_pool_floor`, `whole_dense_slots`,
  `grow_dense` / `grow_sparse`, `validate_storage_buffer_size`, `install_ready`,
  `ShDrainOutcome`. The device-limit and contract split of `GpuCapacity` is in `research.md`
  §Premise review notes.
- Shared definitions: the limits are private renderer constants today
  (`renderer_init_resources.rs`), and owner closure is derived in the app (`topology.rs`,
  `targeting.rs`) and again in the renderer (`research.md` §Build-check seams). The check runs
  after ClusterDirectory and before Packing, outside any cached stage.
- Refuse before growing whatever cannot fit. A dense growth kept while the sparse half is refused
  is valid, per the install's own comment.
- Refusal precedent: lightmap `place_with_growth` / `device_limited`, `warn_device_limit_once`.
  The departure path (`targeting.rs`, `lifecycle.rs`) is the single permit release.
- Tests: SH state captures its limits once at load and tests override them, as the lightmap plan
  does; the harness requires a GPU through `POSTRETRO_REQUIRE_GPU`.
- The bake's `LayerOverflow` cap counts 6-texel tiles, so it is looser than 256 runtime layers
  (`research.md` §Bake-time caps).
- Rivals, including deferral alone: `research.md` §Rivals.
- Durable capture at promotion: §4's mandatory row gains the device-limit backstop; §10's
  runtime-guard sentence covers streamed SH; `build_pipeline.md` records the per-cell check, the
  shared definitions, and that `level-loader` now holds the mandatory-set derivation.
  `docs/level_design.md` gains the error row.
- First slice: the geometry rule plus the zero-share fixture.

## Open questions
- Fragmentation headroom factor — **delegated**: the smallest factor at which the zero-share
  fixture and a churn walk are never refused.
