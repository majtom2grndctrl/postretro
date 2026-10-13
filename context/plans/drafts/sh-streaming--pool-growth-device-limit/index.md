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

### Automated
- [ ] With the floor share forced to zero, a fixture level grows until every cluster is resident
  at once, with no refusal. Before this change, the same fixture is refused.
- [ ] Atlas width, height and tiles per layer are identical across every growth in a session.
- [ ] A dense pool at 200 layers, limit 256, needing 201 grows to 256 and installs. One needing
  257 is refused without growing. Below the limit, growth is unchanged: 100 layers needing 101
  grow to 200.
- [ ] A sparse family whose doubled buffer would pass the binding limit, but whose needed size
  fits, grows to the limit and installs. One whose needed size exceeds it is refused.
- [ ] With the limit forced low, a drain whose demand exceeds it returns no error. The session
  keeps running in play, and Settling releases at its timeout and reveals.
- [ ] In the same drain, a cluster that fits installs while another is refused, in either order.
  A refused cluster installs on a later drain, once room frees, with no new read issued.
- [ ] No mandatory cluster, or owner of one, is evicted while another is refused.
- [ ] A refusal counts as a device-limit miss, not as `held_by_drain_budget`, and repeated
  refusals log one warning per level.
- [ ] Regression guards: a malformed chunk still ends the session with its named error, and a
  retirement-pressure deferral behaves as before.

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
- Should prl-build reject a level that could reach a device limit through baked mandatory demand
  alone? Recommended: yes. The check is per camera cell: its mandatory SH set (id-51 cells within
  the baked maximum L, plus pins and owner closure) must fit each family's limit. Whole-level
  size never rejects, since `large-map-spatial-residency` bounds map scale "by disk and
  residency budget". The build error names the cell and family. Deferral then stays a backstop
  for fragmentation and for drawn demand outside the baked set. The alternative is deferral
  alone. This sets what valid content means for modders — owner: project owner —
  **blocks build**
- Fragmentation headroom factor — **delegated**: the smallest factor at which the zero-share
  fixture and a churn walk are never refused.
- Whether untargeted hysteresis residents release early when a visible cluster is refused —
  **delegated** (`research.md` §Deferral semantics).
