# sh-streaming--pool-growth-device-limit

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Cluster SH residency, Lightmap cell-block residency), §10, `context/lib/boot_sequence.md` §1 (Settling), `context/plans/large-map-spatial-residency.md` §Residency budget tiers · read at a83e186 · evidence: `research.md`

## Problem

The owner asked whether a map that is valid in TrenchBroom can fail to run. Through streamed SH
pool growth, it can: an anticipated defect, confirmed in source, that no shipped map has hit yet.
A growth that would pass a device limit returns a drain error, and the frame loop treats any
drain error as fatal, in Settling and in play. The limits are reachable on a level whose SH fits
them, because the dense atlas's layer size follows the initial floor share rather than the level,
and growth doubles before it checks the limit (`research.md` §Reachability). When this is done,
no level that compiles ends a session through SH pool growth.

## Decisions

- **Dense layer size covers the whole level.** At load, tiles per layer hold the whole dense set
  times a fragmentation headroom within the layer limit, capped at one full layer, and stay fixed
  for the session (`plans/done/sh-probe-streaming--cluster-residency`). Reason: the 2026-09-28
  rule in `large-map-spatial-residency.md`, "a miss fallback cannot stand in for residency".
- **Growth clamps to the device.** Growth is capped at the device limit and refused only when
  the needed size alone exceeds it.
- **A refused growth defers; it never ends the session.** A device-limit refusal is its own
  outcome, distinct from contract errors. The cluster rolls back and parks, keeping its payload
  but releasing its read permit. It is not re-offered until an eviction confirms, its target
  changes or the generation changes. It counts as an attributed miss and warns once per level.
  This diverges from the cluster-residency plan's "named capacity error", which never chose to
  end the session (`research.md` §Commitments touched).
- **Mandatory demand is never evicted to make room.** This keeps §4's tier contract and the pin
  guarantee in `plans/done/sh-probe-streaming--authored-hints`.
- **Other drain errors stay fatal.** Malformed chunks, generation and identity violations,
  arithmetic overflow and post-growth undersizing are broken contracts, not content scale; the
  lightmap pool also treats its contract class as fatal. Unlike the lightmap, SH has no decline:
  a broken SH contract is a bug to surface, not a scale to absorb.
- **Placement: renderer mechanism, app attribution.** The renderer owns geometry, growth and
  refusal (Renderer owns GPU); the app uses the drain outcome only to attribute misses.
- **Limits and sizing rules live once, in `level-format`.** The device limits SH requests, and
  each family's per-cluster sizing, become shared definitions next to `MAX_SH_ATLAS_LAYERS`.
  The renderer and prl-build call the same code, so the build check cannot drift from runtime.
- **prl-build rejects a camera cell whose baked mandatory sparse set cannot fit.** The set is
  the id-51 cells within the baked maximum L, plus pins and owner closure; each sparse family
  must fit its tightest buffer. The error names the cell and the family. Dense is not checked:
  whole-level layer sizing already holds any level below one full layer. Whole-level size never
  rejects, since `large-map-spatial-residency` bounds map scale by disk and residency budget. A
  streamed level without a usable id 51 warns and relies on runtime deferral.
- **This lands before `portal-walk-bounded-regions`**, whose superset visible set adds
  never-refused SH demand.
- **Non-goals:**
  - Out-of-memory below the limits. It is a wgpu uncaptured error shared by every renderer
    allocation, so it needs a renderer-wide error-scope policy (`research.md` §Out of scope).
  - The load-time install panic. It is a deliberate fail-fast before play.
  - An engine VRAM ceiling or player tier: `large-map-spatial-residency` stage 5 owns it.
  - Capping sparse growth at whole-level need. That is VRAM efficiency.
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
- [ ] While a refused cluster stays parked, it is not offered to the drain, holds no read permit,
  and fitting SH clusters and lightmap blocks install at their usual rate. An eviction, a target
  change or a generation change re-offers it.
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
  fits compiles. The renderer requests exactly the shared limits the check uses.

- [ ] The SDK's level-design compile-errors table lists the new error with its cause and fix,
  and notes that pinned volumes count in every camera cell's set.

### Manual
- [ ] At default limits, campaign-test and the stress-warren hallway log no device-limit warning.
  Report their initial dense atlas bytes and pool growth counts against the pre-change build.

## Path

- Seams: `AtlasShape::for_slots` / `grown_for_slots`, `atlas_capacity_slots` (`floor.rs`, a
  second copy of the geometry rule), `plan_initial_pool_floor`, `whole_dense_slots`,
  `grow_dense` / `grow_sparse`, `validate_storage_buffer_size`, `install_ready`,
  `ShDrainOutcome`. The device-limit and contract split of `GpuCapacity` is in `research.md`
  §Premise review notes.
- The limits are private renderer constants today (`renderer_init_resources.rs`); hoisting them
  to `level-format` is the shared seam (`research.md` §Build-check seams). The check runs after
  the ClusterDirectory stage.
- Refuse before growing whatever cannot fit. A dense growth kept while the sparse half is refused
  is valid, per the install's own comment.
- Deferral precedent: lightmap `place_with_growth` / `device_limited`, `warn_device_limit_once`,
  and its per-drain skip of extents known not to fit.
- Tests: SH state captures its limits once at load, and tests override them, as the lightmap
  plan does. The verification gate requires a GPU for the refusal rows, so a skip fails.
- The bake's `LayerOverflow` cap counts 6-texel tiles, so it is looser than 256 runtime layers
  (`research.md` §Bake-time caps).
- Rivals, including deferral alone: `research.md` §Rivals.
- Durable capture at promotion: §4's mandatory row gains the device-limit backstop; §10's
  runtime-guard sentence covers streamed SH; `build_pipeline.md` records the per-cell check and
  its shared limits. `docs/level_design.md` gains the error row.
- First slice: the geometry rule plus the zero-share fixture.

## Open questions
- Fragmentation headroom factor — **delegated**: the smallest factor at which the zero-share
  fixture and a churn walk are never refused.
- Whether untargeted hysteresis residents release early when a visible cluster is refused —
  **delegated** (`research.md` §Deferral semantics).
