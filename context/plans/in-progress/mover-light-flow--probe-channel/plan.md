# mover-light-flow--probe-channel — plan of record

mode: resumable
status: approved
read at: c4a3d980c (no source change since the brief's 9382e3c23; every cited symbol re-read this session)

## Corrections

Path and wording only. No Decision premise was found false.

- `bordered_tile` is in `level-format/src/probe_depth_tiles.rs`, not `sh_bake/depth_tiles.rs`. `TileRayTable` and `DepthTileSelector` are `pub(super)` in `sh_bake` → widen them to `pub(crate)` for the posed re-gather in `surface_radiance`.
- P31 re-trim: `DepthTileSelector::finish` drops each candidate's score, and cached id 59 keeps only `probe_indices` and texels → re-trim recomputes `visibility_change` for each kept tile from id 34's averaged moments and the layout. Rank is score descending, then probe index ascending. The id-34 cache shape stays as it is.
- `ProbeDepthTileTextureLayout` (`for_tile_count`, `tile_origin`) and `probe_depth_tile_texture_bytes` live in `level-format`, not the renderer → the renderer sizes its texture by calling them with id 59's count plus live tiles plus any GPU-held closed-pose tiles.
- `bake_counters` is a `#[cfg(test)]` module of the `prl-build` bin. `stage_events::PATCH_CACHE_DROPPED` and `Drop for PatchCache` are test-only → every row that counts rays or proves the patch-cache drop is a bin unit test. Integration tests prove "traces no id-34 ray" with the `[cache] sh_base_volume hit` line (research §Acceptance instruments).
- `has_delta_entry` is three hand-written closures (`sh_density.rs:320`, `:664`, `sh_analyze.rs:1533`) plus a `BrickInput` field, and `delta_pin_counts` returns a fixed `[u64; 3]` → the section joins each closure by hand, and the pin count array widens to 4 along with its consumers.
- Ghost links reaches pack and load as its own parameter, not through `PostBakeDeltaSections` → id 57 becomes a `PostBakeDeltaSections` field, because storage ceilings consume it like a delta section. Its pack and loader plumbing follow the ghost-links call sites.
- Pass B's per-light curve scale is `animated_light_full_scale`. `fn animated_light_scale` is in `sh_compose.wgsl` and `billboard_direct_scatter_compose.wgsl`.
- `LIT_ENDPOINT_DRIFT` has two copies: an `i32` local in `capture_frame.rs:662` and an `f64` constant at module level in `capture_probe_depth_tiles.rs:804`. The new capture rows use the `capture_probe_depth_tiles.rs` constant, or a `capture_support` copy if a second target needs it.
- `mover_pose` exists only in the compiler. The Path's "posed brushes resolve through the shared `mover_pose` module" applies to compiler posing. The runtime poses movers through physics, as it does today.
- Capture never calls `rebuild_blocked_portals`: `capture/prepared.rs:174` passes `&[]`. → A forced mover t also seals that mover's portals when the forced pose is docked at waypoint 0, matching the live frame. This clarifies the "Capture poses movers" Decision without changing its meaning.
- `WireKinematicMoverState` has no single phase field. t derives from `segment_index`, `direction`, `segment_elapsed_ms`, `wait_remaining_ms`, `completed` and `target_segment`. There is still no wire change.
- The renderer's headless GPU harness is `pub(crate)`. Game-side GPU rows use the app crate's own offscreen helper (`level_load/test_support.rs`, `gpu_gate_tests.rs`). Rows that mix a game half and a GPU half split by crate (research §Acceptance instruments).

- The direct slice is not purely direct: posing id 35 re-levels id-34 cells through the shared stored-set classifier, so ids 34, 49 and 50 move. A `_sh_coarsen 0` control kept id 34 byte-identical with near-equal luma. → Superseded by task 6: the PRL-section Decision has the channel cap *base* storage levels, which ids 34 and 35 share. So id 34's levels do move, but only under channel cells and where seam smoothing refines beside them. B9 checks exactly that (`sh_density::tests::mover_channel`). Without a section, every path stays byte-identical.

- B7's persistent misses include one from the static tracer's τ = 0.5 mm grazing tolerance, which the posed analytic entry test lacks: a gather ray 0.4 mm outside a doorway jamb escapes in the posed bake. → Task 7 traces posed sides through the static tracer's own triangle test with τ. Growing the planes instead breaks the tracer's grazing-ray rule.

- The Wire format has no field for where the bounce payload lives once the 8 MB bar streams it → task 5 adds a `u8 bounce_placement` after `affinity_dims` (header 15 bytes). 0 = resident: the payload follows the bounce index. 1 = streamed: the payload is absent from id 57. The bounce CSR and masks always stay in id 57. Any other value is a named error, as is streamed with no bounce entries. A streamed placement loads only when id 50's manifest lists a source with `section_id` 57 (`StreamedBounceUnresolved` otherwise). Task 7 adds the id-50 cell side and a legacy read path for `off` loads. `MOVER_BOUNCE_RESIDENT_BYTE_BAR` (8 MiB) lives in level-format, and the compiler decides placement against it.
- Task 5 also pinned choices the Wire format leaves open:
  - Ramp records are derived as the sorted distinct movers across the blocks, so there is no count field.
  - A bounce mask must be a non-empty subset of its cell's valid mask.
  - Tile geometry is fixed at 6×6 RGB f16.
  - The grid check runs against id 34 (which id 35 shares).
  - Namespace 1 is bounded by id 34's animation-descriptor count, because id 45 can be disabled at load.
  - A namespace-0 light must be a static lightmap light.
  - A cell with entries must sit at L0 base storage (`StorageCeiling`).
  - The tile range counts id 59 plus one live tile per distinct depth probe.
- Open for task 8, from task 5:
  - An unflagged direct entry whose id-41/45 entry is gone (id 41 soft-disabled at load) is not refused by the loader. Task 8 decides: refuse the level, or compose ungated.
  - The id-41 L0 cap under gated probes is not checked at load.
  - Entries keep the raw light index, so the renderer maps it to the selection index.

- Bake-only static lights (`_bake_only`) light id 35 but have no AlphaLights record, so namespace 0 cannot name them, and task 6 leaves them ungated. That covers 4 of 17 lights on `kinematic-platform`, 36 of 85 on `stress-warren-showcase` and 419 of 763 on the hallway. The "Lights in scope" Decision covers every id-35 light. → Task 6b adds namespace 2 = the compiler's static-baked-light index, always carrying its own tile, so the Decision holds as written.
- Namespace 2 (task 6b) adds rows to the brief's Wire format and Boundary inventory: "2 = bake-only roster rank: static lightmap lights with no AlphaLights record, ranked by static-baked order, dense from 0; flag bit 0 always set". The roster is section-local, so brief 2 reuses it.
- Own tiles dominate the direct block where they appear. On setpiece, 393 of 519 entries carry one, mostly at cells where id 41 has no selection slot, giving 4.09 MB against research's ~1.3 MB estimate. Tiles also repeat across movers at the same (cell, light). The direct block stays resident and is not under the 8 MB bounce bar. → Recorded for the owner. Sharing tiles per (cell, light) would be a format change.
- The dense per-affinity-cell index, which mirrors id 41, costs about 16 B per cell even when empty (the hallway pays 576 KB of index for 1.6 KB of entries). → Kept, matching id 41. A sparse index is a later optimization.
- Task 6 deleted the foundation's `tests/mover_light_gate_prl_parity.rs`: its byte-identity assertion is false by design once id 57 exists, and B17 replaces it. The pins log label is now `id27/id41/id45/id57`, and the new cache stage id is `mover_probe_direct`, both for task 12's docs.
- The SH gate doesn't price the direct bake's memory (setpiece holds about 4 MB of own tiles). → Task 7 prices it along with the bounce bake.
- B15's "the bakes behind ids 34, 35, 41 and 45 trace nothing" can't hold for 41 and 45: those bakes recompute uncached today (`build_pipeline.md`, Soft-shadow bake cost). → **Owner (2026-11-08): keep B15 as written and cache ids 41 and 45 too.** Task 6d adds memos to the id-41 and id-45 bakes.

- `cargo clippy --workspace` fails today on 2 `type_complexity` errors in `postretro-model` (`clip_library.rs:1148`, `gltf_loader.rs:2648`), from main's 8e4b3d7c1. This branch doesn't touch them. → Preflight fixes them in a separate mechanical commit, or reports them as pre-existing.

## Block (2026-11-08, Task 2)

Three Decision premises failed when measured (`research.md` §Bounce slice measurements):

1. **The bounce domain is map-wide for big movers.** The BC6H round-trip bar admits 11,420 of 11,942 setpiece probes with the wall closed, up to 30 m away, and up to 34 m for the raised room. The far changes are real: 7–20% in dim tiles, and 10–20× closer to the static reference than today. The bounce block is 6.5 MB vs id 34's 531 KB, and a travelling wall would recompose nearly every row. Narrowing the domain needs a perceptual floor, which changes the "Bounce half" Decision.
2. **The depth domain overflows.** On setpiece, "every probe whose gather rays the posed mover changes" is 39,990 closed-pose tiles and 11,566 live tiles; with id 59 that is 159% of 1 << 15, and the depth block is 16.2 MB. If the CPU holds the closed-pose tiles (the delegated CPU-write answer), the GPU count is about 37%. Each travelling-wall frame would still upload about 11k tiles. Bounding it changes the "Closed-pose depth" Decision.
3. **The B7 reference misses its bar.** 14 of 232 raised-room interior probes miss the BC6H bar, by at most 0.0034, mostly at floor height. Re-seeding the patches moves the count between 9 and 14. The reference build changes faces, patches and cells, as research §Risks warned.

**Owner rulings (2026-11-08).** (1) Bounce: perceptual floor. (2) Depth: CPU-held closed-pose tiles plus a significance floor scored with id 59's visibility-change metric. Both are recorded in the brief's Decisions; their values come from a sweep at Stop 2. (3) B7: investigate the floor-height misses before any restatement. The remaining Stop 2 work (captures, M1, sweep) runs now.

**Stop 2 evidence (2026-11-08, second worker; `research.md` §B7 investigation, Captures, Floor sweep, M1, M7).** Awaiting owner rulings:

- **B7:** no bake bug. Patch-seed noise exceeds the bar (the reference against itself re-seeded misses 2–9 probes). The remaining persistent misses are the tracer's τ = 0.5 mm grazing tolerance, which the posed analytic test lacks (task 7 fix: trace posed sides through the static triangle test), and the reference's floor split along the jamb planes. Composed is nearer the reference than today at 232/232 probes at every seed. Proposed restatement: "at every interior probe valid in both, the composed base is nearer the static-room reference's id 34 than today's id 34, and within 3 display levels of it."
- **K1 premise failure:** the closet's probe channel matches the reference with reflections off, but the pool's reflection probe (id 54) is captured with movers absent and mirrors the lit doorway (station 2: 0.129 vs a 0.026 bar). That is a third light path the "Closet water and fog" Decision does not name.
- **Inside-brush rule:** skip leaks exterior direct light onto the room's upper interior walls (1.135 vs the reference's 0.063), and depth doesn't fix it. Dark plus mover backface rejection on matches the reference; with depth in place, dark keeps the west face (38.2 vs today's 38.7). Backface on is a renderer change.
- **Floors:** bounce 2 display levels (the largest that keeps B3's 144 closet probes); depth 0.05 (keeps all 28 reference-tiled closet probes). The bounce floor does not localize big or open-map movers (setpiece wall ≈ 5.8 MB; `kinematic-platform` reaches 50–80 m).
- **M1 cost gate fails any SH-Bake ratio:** one pose costs one whole id-34 bake (hallway: 130 s of later bounces + 85 s of gather, whatever the mover's size). Cold all-mover hallway ≈ 23× the parent's SH Bake (≈ 100 min build vs 20.7); warm one-mover edit 5.0× the total. The fallback (bounce one only) leaves the 85 s map-wide gather. The brief's Rivals entry prefers a split over the unmeasured fallback.

**Stop 2 rulings (owner, 2026-11-08), applied to the brief.** (1) Cost accepted, with a per-mover `light_bounces` key (0 valid: no bounce entry, depth still bakes). (2) Inside-brush rule: dark, with backface rejection on for mover fragments. (3) K1 and M2 judge the closet with reflections off; mover-aware reflection captures go to `drafts/mover-aware-reflection-captures.md`. (4) Bounce floor 2 display levels, depth floor 0.05, B7 restated as proposed. (5) Resident byte bar: 8 MB of bounce payload. Block cleared.

These held: B3 and B4 pass on the closet (only closet probes affected, none in the bright room; S^b₁ is bit-exact). One pose costs about 1.5 s against setpiece's 1.4 s SH Bake.

## Delegated answers

- **Mover backface form (owner, 2026-11-08): the smooth, floored form for movers** (ip-372 lever 5: ((dot+1)/2)² + ε, never zero). Task 8 captures it against the hard form on the room's upper walls and west face, and reports if it leaks more.
- *(Superseded by the ruling above.)* **IP check before task 8.** `ip-372-direction-aware-visibility.md` row 15 and lever 5 flag the forward path's `max(dot, 0)` backface factor as the one point-dependent binary zero in the probe blend. Turning it on for mover fragments widens that existing exposure to more receivers. It is not driven by mover state, so the brief's IP rule holds as written. Lever 5's smooth, floored form for movers alone is the alternative. Task 8 asks the owner which form to use before building it.

- Exact field widths: as the Wire format table pins them. Nothing found while reading needs wider fields.
- Live tiles written from the CPU or by a compute pass: from the CPU, through `UploadQueue::write_texture` into the kept texture at `tile_origin`. A compute pass would need `Rg16Float` storage writes, which require an adapter feature the device does not request (`request_renderer_device`). Staged writes also precede every command in the frame's first submission, which gives P24 and P25 their same-frame ordering for free.
- What q a valid probe inside a posed brush takes: **dark, with mover backface rejection on** (Stop 2, owner, 2026-11-08; superseding Stop 1's working rule, skip). The probe is tested without the brush that contains it; the mover's other brushes still occlude. Skip keeps exterior faces at today's level (west face 38.4 vs 38.7), while dark costs them a third. Skip bleeds onto interior faces, because a probe inside a wall serves both of its faces. Expected remedy: closed-pose depth. A probe inside a brush sees every ray at distance 0, so the Chebyshev weight floors it for both faces. Task 2 re-judges skip vs dark with depth in place, plus mover backface rejection on as a cheap variant. The final rule is set at Stop 2.
- Whether posed mover brushes become patches with their albedo: decided by task 2 against the reference-bake rows (Bake 3, Bake 7). Starting rule: the owner's lean, yes.
- The clamp rule for summed bounce: per-texel `max(0)` on the summed base tile at channel-touched probes only, before id-27 deltas add. This is the owner's lean. Task 9 proves it against the two-mover row (Compose 3). An absent section therefore still equals today's output.
- Depth for a probe two movers change: decided by task 10 against a two-door test row added there. Lean: per texel, take the moment pair whose blended E[d] is smaller, meaning the nearer occluder. The choice is independent of hand-over order (P26) and stays a valid moment set.

## Owner stops

1. **After task 1:** owner A/B of station 2 and the room's own faces on the posed-switch PRL. This is the direct premise.
2. **After task 2, the bounce-cost stop:** the owner sets the bounce cost gate and id 57's resident byte bar from Manual 1 and Manual 7, and judges the later-bounce drift report. Format work (task 5 onward) waits on this. If the cost gate fails, the brief's Rivals entry prefers a split.
3. **Task 3:** resolved 2026-11-08. The owner restated K3 to a measured bar and split the absolute fix into its own brief.

## AC-to-proof

Rows are numbered by block, in brief order: Bake (B), Blend scalar (S), Compose (C), Depth (D), Capture (K), Manual (M).

| AC | Proof | Task | Status |
|---|---|---|---|
| B1 q equals joint-sample ratio, same seed; no-reach light stores none | bin unit test against `bake_probe_direct_rgb` and the new two-trace visibility, within 1/510 | 1, 6 | achievable as stated |
| B2 `mover-setpiece` q<1 / no-entry light sets | integration test over the compiled section, light sets from `mover_light_gate_setpiece.rs` | 6 | achievable as stated |
| B3 closet S^b₀ vs static-door reference id 34 within BC6H round-trip; S^b₁ vs today | integration test, reference map generated in-test | 2, 7 | achievable as stated (stop if it misses: research §Risks) |
| B4 closed-pose tiles vs reference id-59 tiles within f16; none where no ray meets the door; no depth block | integration test | 2, 7 | achievable as stated |
| B5 tile-range overflow is a named error; lowered-cap trim; exactly-at-cap packs/loads; 1<<15 synthetic PRL loads; compiler count = loader count | bin unit tests with a test cap seam + loader test on a synthetic PRL | 7, 10 | achievable as stated |
| B6 warm mover-only edit that changes tile count: no id-34 ray, PRL = cold, log/summary report trimmed vs shrunk cap | integration test (`sh_base_volume hit` line + byte compare) | 7 | achievable as stated |
| B7 raised room composed base vs static reference within BC6H; S₁ within T/255 per blocked light | integration test (base) + bin unit test (direct) | 2, 7 | achievable as stated |
| B8 full re-bake domain check | `#[ignore]` integration test | 7 | achievable as stated |
| B9 storage-level caps under entries; other cells unchanged save smoothing | bin unit test over `storage_level_ceilings` with and without the section | 6 | achievable as stated |
| B10 directional light q<1; spinner no entry | bin unit test | 6 | achievable as stated |
| B11 animated light added leaves bounce bytes unchanged; id-27 composes unchanged | integration test (bytes) + GPU compose test | 7, 9 | achievable as stated |
| B12 section bytes identical `-j 1`/`-j 4` and two cold builds | integration test | 6, 7 | achievable as stated |
| B13 section GPU buffers in ledger, zero after unload | renderer GPU test | 8, 9, 10 | achievable as stated |
| B14 SH gate covers measured peak; patch cache drops once; over-budget named error before any bounce ray | bin unit tests on the `live_bytes` ledger, fixtures named | 7 | achievable as stated |
| B15 last-waypoint move: warm traces nothing for 34/35/41/45, scoped re-bakes, one `[MoverLight]` miss, PRL = cold; static edits re-bake every bounce | integration tests (cache lines + byte compare) | 6, 7 | achievable as stated |
| B16 edit order over one cache, revert hits every memo | integration test | 7 | achievable as stated |
| B17 no movers / no-change movers: no section, byte-identical to parent | opt-in `#[ignore]` test against a baseline, after the foundation's parity test | 6, 7 | achievable as stated |
| B18 malformed-section load errors, each named | loader unit tests | 5 | achievable as stated |
| B19 grep gate on the game-side collector and setter; bounce-only and direct-only sections load and compose | grep test + GPU compose test | 4, 8, 9 | achievable as stated |
| S1–S15 blend scalar rows (P1, P4, P9–P11, P13–P15, P23, ramp parse, reversal, two movers, host/client, allocation) | physics unit tests, compiler parse tests, app-crate frame tests, netcode test beside `client_predicted_mover_phase_matches_host_door_occlusion_phase`, alloc probe | 4 | achievable as stated |
| C1 single-terminus gate: S₀, ungated, exact mix; other light unchanged | renderer GPU test (`POSTRETRO_REQUIRE_GPU`) | 8 | achievable as stated |
| C2 stripped section = no state = today, direct and indirect | renderer GPU test | 8, 9 | achievable as stated |
| C3 indirect blend, two movers add, clamp, unaffected probe | renderer GPU test (q bytes 0/255 and f16-exact fixtures) | 9 | achievable as stated |
| C4 two movers' product, never brighter, never negative | renderer GPU test | 8 | achievable as stated |
| C5 animated gated light × curve; SDF light no entry | renderer GPU test | 8 | achievable as stated |
| C6 handoff (1 − w) × gated / promotion-free full gated | renderer GPU test | 8 | achievable as stated |
| C7 recompose only on t change, covered rows; lagging rows compose before sampling | renderer planner unit tests + GPU test | 8, 9 | achievable as stated |
| C8 curve/w changes raise no mover trigger | planner unit test | 8 | achievable as stated |
| C9 tick-driven closet door ramp, reversal, ramp keys from map | split by crate: app test for the t′ sequence from ticks + renderer GPU test at those t′ (research §Acceptance instruments) | 11 | achievable as stated |
| C10 streaming `off` = `sync-proof` | renderer GPU test | 8, 9 | achievable as stated |
| C11 t change on a non-recording frame recomposes next recording frame, streamed and legacy (P12) | renderer tests | 8, 9 | achievable as stated |
| C12 held + travelling: only traveller's rows; shared row reads held t (P16) | planner unit test + GPU test | 8 | achievable as stated |
| C13 slot re-tenanted mid-travel (P17) | planner/GPU test | 8 | achievable as stated |
| C14 pass routing by entry kind (P18) | planner unit test | 8, 9 | achievable as stated |
| C15 own-tile gated light composes S₀ (P22) | bin unit test (flag) + GPU test | 6, 8 | achievable as stated |
| D1–D14 depth rows (blend, strip, write-on-change, P24–P30, reference texel R/G, alloc, eviction/growth, ledger) | renderer GPU tests (reuse `sh_depth_tiles_harness.rs`; readback adds `COPY_SRC`), shader byte-identity via a pinned hash test, frame-pair rows in an app offscreen test | 10, 11 | achievable as stated |
| K1 closet forced closed vs tiled static reference; forced open vs stripped; replaces `mover_door_closet_does_not_darken` | capture test, `--features capture --ignored`, GPU | 11 | achievable as stated |
| K2 setpiece wall closed, held receiver vs static reference | capture test | 11 | achievable as stated |
| K3 pop at forced termini no worse than the ungated arms (restated by owner 2026-11-08) | capture test: w=0 vs w=1 drift with the channel ≤ the same receiver's drift forced open with the section stripped, + `LIT_ENDPOINT_DRIFT` | 11 | achievable as restated |
| M1 bounce-slice cost on full hallway | owner/executor measurement, idle machine, reported at stop 2 | 2 | **measured 2026-11-08; owner accepted the cost** (machine was not idle: owner present, load 3–9) |
| M2 dark closet judged against reference | owner, in-engine | 12 | manual |
| M3 setpiece station 2 raised-room A/B | owner, in-engine | 12 | manual |
| M4 skinned mesh on enemy spot darkens/relights | owner, in-engine | 12 | manual |
| M5 mover's own faces no darker than today | owner, in-engine (first judged at stop 1) | 1, 12 | manual |
| M6 5300M Metal System Trace ≤ 0.2 ms per travelling mover | owner/executor on the 5300M | 12 | manual |
| M7 byte and tile-count table in `research.md` for the five maps | executor, at stop 2 | 2, 6 | **recorded 2026-11-08** except the direct-half byte column (needs task 6's q bake) and the hallway's full mover set; task 6 adds the direct column |

## Tasks

The integrating executor owns `plan.md`, the id-57 types, the stage order in `pipeline.rs`, the renderer setter signature, and every commit and Cargo gate. Workers get the full brief, `research.md`, this plan, the routed docs and the whole Acceptance list. They never edit `plan.md` or commit.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Direct slice.** Add the two-trace soft visibility (static and static-plus-posed on one joint sample set). Add a dev-only compiler switch that poses `setpiece_room` raised as an occluder in the id-35/41/45 bakes. Capture draws the mover at that pose. Bin unit test for B1's ratio. Decide the inside-brush rule. **Owner stop 1** (station 2, own faces). | worker on main checkout (moved from executor to keep its context for decisions) | — | **done; Stop 1 passed** (owner, 2026-11-08: proceed with skip, re-judge at Stop 2). Temporary switch `POSTRETRO_DEV_POSE_MOVER=<mover>:<first\|last>[:dark\|:skip]`; joint visibility in `mover_probe_channel/visibility.rs`; B1 bin unit test plus 3 more passing. Skip rule: interior baked direct 28.2→6.8 luma, own faces within 0.3 levels of today. Dark rule: west face 38.7→25.7, which fails M5. Captures `captures/t1-*`. |
| 2 | **Bounce and depth slice.** The same switch, for the closet door closed and the room raised: a posed re-gather (closest hit = min(static, entry distance into the posed brushes)), S^b replacing id 34, and closed-pose tiles from the same rays. Prototype reference-build checks B3, B4 and B7. Measure later-bounce drift over the affected domain. Settle the patch question. Measure M1 on the hallway and write M7 into `research.md`. **Owner stop 2.** | integrating executor | 1 | **done; Stop 2 passed** (owner, 2026-11-08; see Block). Built: posed bounce, gather and depth, BC6H bar, dev switches (`dev_bounce.rs`, `dev_trace.rs`, `dev_floor.rs`, `POSTRETRO_DEV_MOVER_BACKFACE`). Stop 2 evidence in, see Block; captures `captures/t2-*`. |
| 3 | **Promotion pop.** Root-cause `drafts/animated-promotion-direct-sh-pop.md` with a discriminating experiment (§6.3), check whether the id-41 arm shares the gap, and fix it within this brief or stop. | worker (worktree), GPU | — (parallel with 1–2) | **done, diagnosis only** (2026-11-08). The owner restated K3 to a measured bar; the fragment-reconstructed baked arm becomes its own brief: no convention bug. The baked arm's representation (6×6 octahedral tile with a 4×4 interior, L2 truncation, probe interpolation) reads 0.81–0.86× the runtime term facing the light and ~2.3× at grazing; the id-41 arm shares it. A CPU model matches the GPU within a few percent. The fix that meets the absolute row is a new format and sampler path, larger than this brief. Diagnosis is in the draft's "Diagnosis (2026-11-08)" section. |
| 4 | **Blend scalar.** In physics: arc-length t, matching `position_for_phase`'s degenerate rule, with terminus t for no-segment states (P9). In the compiler: parse `light_ramp_start` and `light_ramp_end`, plus FGD keys. In the game: collect (mover id, t′) from the registry, culled movers included, and hand it over at both `rebuild_blocked_portals` sites. Add a renderer setter that stores state only, with a no-level guard and a reset at install. Add the grep gate. Covers S1–S15. | worker (worktree) | — (parallel with 1–2) | **done** (2026-11-08). Written in a worktree, compiled and tested on main. Passing: physics `path_fraction` 8 and `kinematic_mover` 165; prl-build `light_ramp` 2 and FGD 1; netcode S6 plus the door test, 2; app `runtime_movers` 49 (grep gate, positive control, alloc probe); renderer `mover_light_state` 2 on GPU. Renderer halves of S10, S12, S13, S14 and P15 land with task 8. The ramp reaches `LoadedKinematicMover.light_ramp`, which task 5 fills from id 57. |
| 5 | **Format and load.** Add `MoverProbeChannel` id 57 to level-format, following the Wire format table, and register it. Loader validation goes beside `validate_direct_sh_delta`. The ramp record reaches mover runtime data. Covers B18. | integrating executor | stop 2 || **done** (2026-11-08). level-format `mover_probe_channel` 5 + registry; level-loader `mover_probe_channel` 28 (one per B18 error, plus streamed, `off` and whole-load end-to-end); full suites level-format 718, level-loader 375; clippy and fmt clean. Exposed as `LevelWorld::mover_probe_channel()`; ramp reaches `LoadedKinematicMover.light_ramp`. See Corrections for the placement byte. |
| 6 | **Compiler direct, production.** Hoist `MoverPoses::resolve` ahead of the SH gates (P21). Add the q bake with a per-light memo on an occlusion-only geometry hash, using the dark inside-brush rule (Stop 2). Decide the own-tile flag against ids 41 and 45 as emitted (P22). Cap storage levels and id 41 through `DeltaSectionsRef`, and widen the pin count array. Emit the direct block. Covers B1, B2, B9, B10, B12, B15, B17 and the C15 flag. | integrating executor (may split the memo to a worker) | 5 || **done** (2026-11-08). prl-build bin 2400 passed; new integration targets `mover_probe_channel_cache` (6), `_setpiece` (2) and `_prl_parity` (1, opt-in B17, passes against a589eec40); level-format 718; clippy and fmt clean. Setpiece's id 57 is 4.09 MB (direct). The closet gates no direct light, so it gets no section until task 7. M7 direct column is in the task 6 report (to go into `research.md`). B15's 41/45 clause awaits restatement. |
| 6b | **Bake-only lights.** Add namespace 2 (the compiler's static-baked-light index, always with its own tile) to the id-57 format, the loader and the direct bake, so `_bake_only` static lights are gated. Measure the bytes on the five maps. | worker | 6 || **done** (2026-11-08). Namespace 2 = the light's rank in the section's bake-only roster: ordered by static-baked index, dense from 0, own tile always present (`MoverProbeChannelOwnTileRequired`). Bake-only lights leave no record in any PRL section, so no load-time index exists. Same epoch. level-format 719, level-loader 377, prl-build bin 2403, cache integration 6, setpiece 2, B17 1. Closet, setpiece and campaign-test are byte-identical to HEAD. The hallway's direct block grows to 19.6 MB (1,725 namespace-2 entries, nearly all own tiles), which task 6c must address. |
| 6c | **Direct-block format revision** (owner, 2026-11-08, before any renderer reads id 57). (1) One own tile per (cell, light), shared by every mover's entry, which then carries only q₀/q₁. (2) A per-(cell, light) probe mask, so tiles and q are stored only at probes where some mover's q < 1 at either terminus. (3) A sparse cell index listing only cells with entries, replacing the dense per-affinity-cell masks and offsets. Update the brief's Wire format table as a Correction. Measure bytes before and after on the five maps. Deferred: reconstructing T from light parameters × a visibility scalar, which goes with the promotion-pop brief. | worker | 6b | |
| 6d | **Cache ids 41 and 45** (owner, 2026-11-08, so B15 holds as written). Per-light memos for the id-41 entity-shadow delta and id-45 animated direct bakes, on the same occlusion-plus-light key shape as task 6's memo. Warm PRL equals cold. | worker | 6b | |
| 7 | **Compiler bounce and depth, production.** Bounce bake per mover, with a memo on id 34's volume key plus posed brushes. Add the affected-probe bar, the patch-cache lifetime (P20, P21) and SH-gate pricing. Add closed-pose tiles, the id-59 re-trim (P31), the named tile-range error and summary/log lines. Stop 2 additions: the dark inside-brush rule, the bounce floor (2 levels) and depth floor (0.05), the `light_bounces` key (parse, FGD, memo key, the depth-only trace at 0), and posed sides traced through the static triangle test with τ. Covers B3–B8, B11, B14–B16 and the bounce side of B17. | integrating executor | 6 | |
| 8 | **Renderer direct compose.** Turn on backface weighting for mover fragments in the smooth, floored form (Stop 2 plus the owner's IP ruling; see Delegated answers). Add the channel storage for Passes A and B within the 8-buffer wall, either by merging matched read-only arrays or by keeping one channel buffer per pass. Add the per-mover t block, per-mover row membership and the t′-changed planner trigger, plus a legacy `should_dispatch` term committed after encode. The gated term goes in both atlases. Covers C1, C2, C4–C8, C10–C15, B13 and B19. | worker, then integrate | 4, 5 | |
| 9 | **Renderer indirect compose.** In `sh_compose.wgsl`, apply the bounce override at affected probes, then the clamp, then id-27 deltas. Covers C3, B11 (GPU), C7, C10, C11 and C14 for the indirect pass. | worker, then integrate | 4, 5 | |
| 10 | **Depth live tiles.** Size `ShDepthTiles` for every tile and keep the texture. Write live tiles from the CPU and point references at them (P24–P30). Live tiles must survive eviction and growth, and appear in the ledger. Add the two-door depth row. Covers D1–D14, B5 (loader side) and B13. | worker, then integrate | 4, 5 | |
| 11 | **Wiring and capture.** World-less frame hand-over. Capture forced mover t covering pose, casters, occluder bounds and portal seal. Add the C9 tick-driven test, K1–K3, and retire `mover_door_closet_does_not_darken`. | integrating executor | 3, 6–10 | |
| 12 | **Close-out.** Delete the dev switches. Update `context/lib` drafts so "not built yet" becomes the contract. Run `/preflight`, then `/review-panel` → `/fix-review-findings` until no new findings, then the full gate. Hand M2–M6 to the owner. | integrating executor | 11 | |

Tasks 8, 9 and 10 run concurrently in worktrees only if free disk stays above 15 GB plus about 30 GB per worktree. Otherwise they run in order on the main checkout. Disk had 76 GB free at planning. While the owner is at the machine, builds stay capped at 6 jobs under `nice`.
