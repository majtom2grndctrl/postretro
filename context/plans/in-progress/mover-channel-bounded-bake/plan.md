# mover-channel-bounded-bake — plan of record

mode: resumable
status: approved (built; awaiting land the plane)
read at: 1a66523a

Source check: no file under `crates/level-compiler/src/mover_probe_channel/`,
`surface_radiance/`, `pipeline/mover_*`, `pipeline/delta_working_set.rs`,
`pipeline/sh_working_set_gates.rs`, `live_bytes.rs`, `cache.rs` or
`delta_sections.rs` changed between the brief's 176d0bf7 and 1a66523a, and no
mover or SH-gate line of `pipeline.rs` did. Every symbol the Decisions and Path
cite was re-read at 1a66523a and matches the brief. No Decision premise is false.

## Corrections
- Path: "check whether the stage cache can tell a pose is present without
  reading it whole" → it cannot. `StageCache::get` (`cache.rs`) is the only
  read; it hashes the whole payload, and `PoseMemo::get` must decode to call
  an entry corrupt, which M5 needs to fall back from `high` to `low`.
  Planning around it by: the lookup reads, decodes and drops each pose
  (keeping only key, count and `held`), and the slot reads again, as the Path
  allows.
- Brief: carried bytes join the SH-delta projection → that projection is made
  (`project_delta_working_set`, `pipeline.rs` ~L726) before the bounce plan
  and the SH base projection exist (~L811, ~L828). Planning around it by
  adding the carried term after all three exist, before
  `admit_sh_working_sets`, so gate order (G1) reads the final figures.
- Brief M1: "the ledger reports each kind's high-water mark" → `live_bytes`
  reports the live set by kind at the overall peak only. Planning around it
  by adding a per-kind high-water map to `Report`.
- Brief D1/D3: the in-process gate harness (`bake_at`,
  `pipeline/mover_bounce_stage_tests.rs`) measures the SH base stage only; the
  SH delta, direct-SH and direct windows run later in `run_after_parsing`.
  Planning around it by a measured whole-pipeline harness: the sections alive
  in those stages (ids 27, 34 and its analysis clone, 35 and its clone, 41,
  45, the entity-shadow section, the posed solids, the blocks, id 57) routed
  through the ledger by capacity under `#[cfg(test)]`, with phase marks at
  the SH-delta and direct windows.
- Brief Path: `pipeline.rs` 2,600 lines → 2,664 now;
  `mover_bounce_stage_tests.rs` 1,484. The ledger routing and the moved
  skip/finish wiring grow `pipeline.rs`, so it splits first in its own commit
  (Task 4).
- Today's stop check runs at the start of each slot on the union of earlier
  movers (`bake_mover_bounce`), and `refuse_cut_short` runs later in
  `skip_mid_poses`' caller over every mover. Per-mover finish moves the
  verdict to just after each mover joins the union and before its skip. The
  union only grows, so the build fails iff the final union (or a cut) passes
  the range: the same verdict in any order (I2). The carried count in the
  error may differ by order, as the brief allows.

## Delegated answers
- The layer cap's size — 16 layers in flight, a fixed constant with a test
  seam (`resolve_layers` takes the cap). Fixed, not the worker count, so the
  pre-ray price is the same on every machine; 16 keeps a 6-core desktop busy
  while most of the stress map's ≈2.9e9 B summed scratch drops to its 16
  largest layers.
- Whether the payload cap check runs before or after a mover's blocks join —
  before. The mover's block bytes are computed exactly from its skipped
  states; past the cap the build fails naming the flag, the joined plus new
  bytes and the movers joined, with nothing appended. Otherwise its encode
  appends in place into the joined blocks, reserved exactly, so the blocks
  never pass the cap and the price adds no mover's blocks above it. During
  the encode its states still live; the per-mover states term covers them.
  The blocks' bytes are counted by capacity, as the ledger counts them, so a
  cap equal to them admits and one byte under fails (C1).

## AC-to-proof

Rows in brief order. "Routine" runs in plain `cargo test`; "on demand" is
`#[ignore]` on four workers. Goldens are section digests captured at the
parent commit (1a66523a plus this plan) before any production edit, the
`budget_regression.rs` precedent. The parity harness is an `#[ignore]` A/B
test against the parent's `prl-build`, named by
`POSTRETRO_MOVER_BOUNDED_BAKE_PARENT_BIN` (the
`mover_probe_channel_prl_parity.rs` precedent), over every map under
`content/dev/maps` whose parent bake reports an analysed mover.

| AC | Proof | Status | Result |
|---|---|---|---|
| 1 Output identity, every fixture (I1) | routine: `mover_sections_match_parent_goldens` (closet and its edits, `low`/`high`, cold/warm, 1 and 4 workers); on demand: `mover_bounded_bake_parent_parity` (every mover fixture, stress-env-volumes included) | achievable as stated | pass: goldens (closet and two-door edit, `low`/`high`, cold/warm/hit, 1 and 4 workers) after every task; parent parity identical on mover-closet and mover-setpiece, cold/warm/hit, `low`/`high`. **Outstanding:** parity on the other mover fixtures and stress-env-volumes (on demand, hours) |
| 2 Job order reversed (I2) | `reversed_job_order_keeps_sections_and_the_tile_range_verdict` (two doors of different reach; lowered `--probe-depth-tile-cap` variant puts one past the range) | achievable as stated | pass |
| 3 No two movers' states live (M1) | `one_movers_states_are_live_at_a_time` (two-door edit, setpiece on demand; cold, warm empty, full hit; per-kind high-water ≤ largest one mover's) | achievable as stated | pass on the two-door closet (cold, warm empty, full hit). **Outstanding:** setpiece variant (`#[ignore]`, on demand) |
| 4 Held mover not live during the other's bake (M2) | `a_held_door_is_not_live_while_the_other_bakes` (two-door edit primed with one door's `light_bounces` changed, both orders; mover-bake phase price ≥ ledger peak) | achievable as stated | pass (both orders; mover-bake price ≥ window) |
| 5 Failed slot read bakes there (M3) | `a_failed_slot_read_bakes_the_pose_and_matches_cold` (test hook fails the read after lookup; `low`/`high`; all held, and one held one pending) | achievable as stated | pass (seam proven to fire; `low`/`high`; all held and one pending) |
| 6 Corrupt `high`, whole `low`; and slot failure at `high` (M5) | `a_corrupt_high_entry_serves_low_as_the_parent_did` (golden captured at the parent on the same crafted cache) and `a_high_read_failing_at_the_slot_bakes_at_the_high_count` | achievable as stated | pass (corrupt-`high` golden; slot failure at `high` count) |
| 7 Lazy layers at a failed slot routed and priced (M4) | `a_slot_failure_after_blocks_join_is_priced` (two-door edit, `low`, full hit, second door's read fails) | achievable as stated | pass (bounce one proven built at the slot) |
| 8 Layers in flight ≤ cap; no stall at one permit (L1, L2) | `layer_resolution_never_exceeds_its_cap` (cap below workers, fixture with more layers than workers, 1 and 4 workers, cold/warm) and `layer_resolution_completes_at_one_permit` | achievable as stated | pass |
| 9 Payload cap error and flag parsing (C1) | `the_payload_cap_fails_by_name_and_admits_at_its_bytes` (exact cap admits, one under fails, message names flag, bytes, movers; nothing dropped) and CLI parse tests in `main.rs` (past `u64`, malformed, missing) | achievable as stated | pass |
| 10 Tile range before cap, single mover (I4) | `a_mover_past_both_fails_by_the_tile_range_before_skipping` | achievable as stated | pass |
| 11 Cap on one mover, range on another (I3) | `cap_and_range_on_different_movers_fail_by_name_in_both_orders` | achievable as stated | pass |
| 12 Each priced phase ≥ ledger peak in its window (P1, P2, W1, D1, D3) | routine: `every_priced_phase_covers_its_window_on_the_closet` (whole-pipeline harness; cold, warm empty, full hit, id-34 hit with memo misses, `low` over `high`-primed; cap set to the fixture's block bytes); on demand: same on the setpiece and every mover fixture | achievable as stated | pass: closet routine in every mode, cap at the blocks in cold, warm, full-hit and id-34-hit runs; on demand setpiece, mover-pins, closet-reveal, enemy-movers at `low`/`high`, cold/warm/hit (run before the review rounds, which only raised prices). **Outstanding:** remaining mover fixtures |
| 13 Finish-loop buffers routed and priced (P1) | asserted inside row 12's harness: skip and encode buffers, skip bars, mover records, probe cells held by capacity in the mover-bake phase | achievable as stated | pass |
| 14 Direct window routes every section ≤ its term (D1, D2) | `the_direct_window_covers_every_section_alive` (closet; animated-light fixture with `--sh-analyze` for the clones) | achievable as stated | pass (animated light, `--sh-analyze`) |
| 15 `--sh-delta-max-size` below dense, at or above post-drop (D1) | `a_delta_cap_below_dense_still_prices_the_dense_window` | achievable as stated | pass (premise asserted) |
| 16 SH-delta projection covers carried bytes (D3) | `the_sh_delta_projection_covers_what_the_channel_carries` (peak and carried term alone) | achievable as stated | pass |
| 17 Layer price covers largest in-flight layers, hit payload included (L1) | inside row 8's test: layer-phase price ≥ ledger peak, warm with some layers hit | achievable as stated | pass (hits proven) |
| 18 Edges: no mover zero; bounces 0; fill 0 | `mover_channel_price_edges` (unit, `cost.rs`/`working_set.rs`) | achievable as stated | pass |
| 19 Same SH base projection at `low` and `high` | `the_sh_base_projection_ignores_fidelity` over every mover fixture (plan only, no ray; routine closet, on demand the rest) | achievable as stated | pass: closet routine; four fixtures on demand; stress-env-volumes gate figures identical at `low`, `high`, `--release` |
| 20 One byte under SH base peak refuses by its name (G1) | `one_byte_under_the_sh_base_peak_refuses_by_its_name_through_gate_order` (fixture with delta projection below the base peak, through `run`) | achievable as stated | pass (both SH gates admit at the peak; the lightmap gate may refuse later: clarification) |
| 21 `build_pipeline.md` rows; `docs/level_design.md` flag | doc edit, reviewed against this table; a grep test that `--mover-payload-max-size` appears in `main.rs` help and `docs/level_design.md` | achievable as stated | pass |
| 22 stress-env-volumes admitted at 16 GiB, `low`, every gate | stop-at-gate release run of `prl-build` (all gates, no bounce ray); a refusal moved to another gate stops the build for the owner | achievable as stated | pass for both SH gates (SH base 11,426,906,815 B, SH-delta 9,536,186,384 B at 16 GiB, `low`, `high`, `--release`). **Outstanding:** the lightmap gate, which runs after every SH bake (full cold bake) |
| 23 Estimate ÷ largest cold window per phase; `--release` admission | `#[ignore]` findings harness on setpiece and stress-env-volumes at `low` and `high`; stress cold may need the owner's machine (this container: 4 cores, 15 GB) | achievable as stated | setpiece: estimate ÷ largest cold window 1.59× (`low`), 1.58× (`high`), per phase in Findings. **Outstanding:** stress-env-volumes (full cold bake) |
| 24 Block bytes against 256 MiB | the same harness's block counts, both maps, `low`/`high` | achievable as stated | setpiece: 21.7 MB (`low`), 22.2 MB (`high`) of 256 MiB. **Outstanding:** stress-env-volumes |
| 25 SH base time before and after | parent and branch binaries, cold, same two maps, layer resolution apart; warm full hit on setpiece at `low` | achievable as stated | setpiece: no regression cold (33.81→33.00 s `low`, 135.97→137.26 s `high`), full hit +0.18 s; layers apart. **Outstanding:** stress-env-volumes |
| 26 Owner's 16 GB desktop sampling | owner, cold `--no-cache`, private bytes every 30 s | manual | **outstanding: owner, manual** |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 0 | Before production edits: capture parent goldens (closet family, M5's crafted cache) and build the parent `prl-build` comparison binary for the parity harness; add the parity harness | integrating executor | — || done: `tests/mover_bounded_bake_goldens.rs` (5 goldens at 10df2fcb; every cache mode and worker count hashed alike), `tests/mover_bounded_bake_parity.rs` (closet identical against the parent binary, low and high, cold/warm/hit); parent `prl-build` kept at `<scratchpad>/parent/prl-build-parent` for this session |
| 1 | Per-mover finish, the riskiest slice: each slot resolves, reduces, edge-copies, writes memos, joins the depth union, takes the tile-range verdict, skips, encodes into incremental `BounceBlocks` and drops its states; held slots finish at their slot too; id-59 trim once after the last mover; ledger per-kind high-water. Proof rows 1 (closet goldens, setpiece parity), 2, 3, 10 | integrating executor | 0 || code done: `bake_mover_bounce` finishes each slot (`DepthUnion` verdict before skip, `BounceBlocks::push_mover`), pipeline's `skip_mid_poses` gone, ledger per-kind and group high-water. Goldens pass (25 builds); 276 mover/depth/skip unit tests pass; `finish::reversed_job_order_keeps_sections_and_the_tile_range_verdict` passes. M1 passes cold and warm empty; full hit fails until Task 2 (the lookup holds every held mover's states: 818,712 B against 719,544 B alone). Row 10 (I4) moves to Task 3, which adds the cap. Setpiece parity: see Task 2 |
| 2 | One finish site: the lookup plans only (reads, decodes, drops); every mover reads back at its slot; a failed slot read builds layers, bounce one and id 34's later bounces lazily on the volume-hit path. Rows 4, 5, 6 | integrating executor | 1 || code done: `PoseLookup` reads, decodes and drops (no `HitReduction`); one `MoverSlot` kind; `SlotRadiance` builds bounce one and id 34's later bounces at the first slot whose read-back fails (`HitRadiance`, which extends the progress total); test seam `fail_read_back`. Goldens pass (M5's corrupt-`high` golden included); `finish::` M1 (cold, warm empty, full hit), M2, M3, M5 slot-failure and I2 pass. Mover sweep: 341 passed, 0 failed. Setpiece parity against the parent binary: identical at `low` and `high`, cold, warm empty and full hit (741 s) |
| 3 | `--mover-payload-max-size`: flag, default 256 MiB, pre-join check, named error, help text. Rows 9, 10, 11 | integrating executor | 1 || done: flag (default 256 MiB, `parse_size`), `BounceBlocks::mover_bytes` checked before a mover's blocks join (exact capacities; debug-asserted against `heap_bytes`), `MoverPayloadCapExceeded` names the flag, bytes, mover and movers joined; `MoverBounceRefusal` carries either refusal. Tests pass: C1 (`the_payload_cap_fails_by_name_and_admits_at_its_bytes`, three CLI parse tests), I4, I3; docs row and error row with `level_design_docs_list_the_mover_payload_cap`; goldens still match |
| 4 | Split `pipeline.rs` along its stage seams (own commit, no behaviour change); then the whole-pipeline measured harness with sections routed and phase marks at the SH-delta and direct windows | integrating executor | 1 || done: split commit 4e2dde1a (`pipeline/delta_plan.rs`, `pipeline/sh_analysis_stage.rs`, pipeline.rs 2,664 → 2,194). Harness: `pipeline/window_ledger.rs` routes ids 27/34/35/41/45/57, clones, entity-shadow, solids and direct entries by capacity; phase marks `mover bake`, `sh delta stages`, `mover direct`; `BounceBlocks` and the finish loop's buffers route themselves; `sh_working_set_gates::recorded` hands the checked projections to `pipeline/mover_window_tests.rs` |
| 5 | Pricing: drop the accumulated-states term; price one mover's states, the blocks at the cap, the finish loop's buffers and the lazily built slot inputs in the mover-bake phase; the direct window after the SH delta and direct-SH stages with ids 27/41/45 at the dense bound; the carried term in the SH-delta projection. Rows 7, 12–16, 18–20 | integrating executor | 2, 3, 4 || done: `price_movers(cost, &DirectWindow)` prices one mover's states, `raw_mover_bytes`, `finish_bytes` and the blocks at the cap in the mover bake; the direct window beside ids 27/41/45 at the copy chain less its base-copy share, id 35 and its clone from the grid, the entity-shadow selection, id 34 and its clone, solids, 3× the cap and 2× the direct bound; `carried_bytes` joins the SH-delta projection. Every term at the mover's own `light_bounces`. Tests pass: `mover_window_tests::{every_priced_phase_covers_its_window_on_the_closet, the_direct_window_covers_every_section_alive, a_delta_cap_below_dense_still_prices_the_dense_window, one_byte_under_the_sh_base_peak_refuses_by_its_name_through_gate_order}`, `finish::{the_mover_bake_price_covers_a_held_door_beside_a_baking_one, a_slot_failure_after_blocks_join_is_priced}`, `cost_tests` edges; on demand `every_priced_phase_covers_its_window_on_every_mover_fixture`. Sweep: 398 passed after gating the `mover bake` mark on movers |
| 6 | Layer cap: `resolve_layers` in batches of 16 (test seam), layer-phase price over finished layers plus the 16 largest in-flight scratches, hit payload included. Rows 8, 17, and row 1 re-run | integrating executor | 5 || code done: `LAYERS_IN_FLIGHT` = 16, `resolve_layers_capped` batches with `par_extend` (plan order kept); layer phase prices finished layers plus `in_flight_scratch` of the 16 largest. `layer_cap_tests::{layer_resolution_never_exceeds_its_cap, layer_resolution_completes_at_one_permit}` pass. Goldens still match (row 1 re-run); sweep 516 passed, 0 failed |
| 7 | Context and modder docs: `build_pipeline.md` flag rows, *Memory* paragraph ("Decided, not yet built" retired), `docs/level_design.md`. Row 21 | integrating executor | 6 || done: `build_pipeline.md` *Memory* paragraph, `--mover-payload-max-size` and `--working-set-max-size` rows rewritten as built; `docs/level_design.md` flag and error rows landed in Task 3 |
| 8 | Findings: stress-env-volumes stop-at-gate admission, findings harness on setpiece and stress-env-volumes, block bytes, stage times; on-demand parity over every mover fixture. Rows 22–25 | integrating executor | 6 | |

Delegation: none planned. Tasks 1–3 and 5 share `bake_mover_bounce`, the
pose lookup and the price; task 4 rewires the file they all touch. Sequential
on one warm `target/`.

## Build notes
- The SH-delta window's ledger routes what that gate claims: the dense
  deltas and their analysis clones, plus what the mover channel carries
  (id 34 and its clone, id 59, the blocks, the posed solids). Id 35, its
  clone and the entity-shadow selection are routed from the direct window's
  start, where the direct phase prices them; before it, the SH-delta gate's
  base-copy share covers them, as it did before this brief.
- The direct window prices the dense deltas at the SH-delta gate's copy
  chain less its base-copy share (dense and compaction, plus the analysis
  clone when kept), because valid-probe compaction runs inside that window.
- G1 at the peak: both SH gates admit; on the two-door closet the lightmap
  stage's own working-set gate (863 MB predicted) then refuses the same
  budget. G1 names the two SH gates, so the test accepts only that later
  refusal. Clarification, not a change to the row.
- `assemble` (level-format) now reserves every vector exactly, so the id-57
  section holds no more than its parts; output bytes unchanged.

## Findings

### stress-env-volumes, stop at the SH gates (release binaries, this container)

| Build | SH-delta gate | SH base gate | Verdict |
|---|---|---|---|
| parent, `--no-cache --bake-fidelity low` | 7,005,325,824 | 21,806,445,163 (mover direct; mover bounce 19,850,839,371) | refused by the SH base gate, matching the owner's Windows refusal byte for byte |
| branch, `low` | 9,535,633,664 (2,530,307,840 carried) | 10,957,856,875 | both admit at 16 GiB |
| branch, `high` | 9,535,633,664 | 10,957,856,875 | both admit |
| branch, `--release` | 9,535,633,664 | 10,957,856,875 | both admit |

Branch SH base phases (bytes): gate 423,648,296; layers 3,147,257,343 (was
5,923,093,759); sum 3,038,596,835; later bounces 638,902,859 (was
10,467,698,803); probe gather 4,233,418,135 (was 14,062,214,079); id-34 entry
3,741,564,551; mover bounce 8,792,042,699 (was 19,850,839,371); mover direct
10,957,856,875 (was 21,806,445,163). The projection is the same at `low`,
`high` and `--release`. The lightmap stage's own gate runs after every SH
bake, hours into a cold build here; its verdict at the default needs the
full bake (row 26's owner runs, or a longer session).

### mover-setpiece, SH base stage time (release binaries, 4 workers, this container)

Both sides log layer resolution apart with the same line (the parent
binary patched with that log line alone). Every pair of PRLs is
byte-identical.

| Build | Parent SH Bake | Branch SH Bake | Layers (parent / branch) |
|---|---|---|---|
| cold `low` | 33.81 s | 33.00 s | 16 in 0.06 s / 0.05 s |
| cold `high` | 135.97 s | 137.26 s (+0.9%) | 16 in 0.07 s / 0.06 s |
| warm fill `low` | 33.70 s | 33.10 s | 16 in 0.07 s / 0.05 s |
| warm full hit `low` | 2.12 s | 2.30 s (+0.18 s) | none resolved |

The full hit's extra 0.18 s is each pose read twice: the lookup reads and
drops it, the slot reads it back, which the stage cache needs to call an
entry whole. Stress-env-volumes timings need its full cold bake.

### Bounce and depth blocks against the 256 MiB cap

| Map | `low` | `high` |
|---|---|---|
| mover-setpiece | 21,674,212 B (bounce 6,240,100 + depth 15,434,112), 8.1% | 22,191,776 B (6,757,664 + 15,434,112), 8.3% |
| stress-env-volumes | needs the full mover bake | needs the full mover bake |

### mover-setpiece, each priced phase against its cold window (in process, 4 workers)

Estimate 1,018,908,526 B at `low` and `high` alike; largest cold window the mover bake: 640,790,538 B at `low` (1.59×), 645,061,930 B at `high` (1.58×).

| Window | Priced | `low` measured | `high` measured |
|---|---|---|---|
| gate | 35,300,164 | 26,530,980 (1.33×) | 26,530,980 (1.33×) |
| plan and probe layout | 56,134,228 | 26,377,340 (2.13×) | 26,377,340 (2.13×) |
| layers | 56,134,228 | 32,569,876 (1.72×) | 32,093,788 (1.75×) |
| sum | 45,335,676 | 28,875,556 (1.57×) | 28,875,556 (1.57×) |
| later bounces | 37,961,620 | 30,492,308 (1.24×) | 30,492,308 (1.24×) |
| probe gather | 114,485,580 | 59,761,972 (1.92×) | 59,761,972 (1.92×) |
| mover bake | 747,725,524 | 640,790,538 (1.17×) | 645,061,930 (1.16×) |
| volume store | 373,391,776 | 56,477,366 (6.61×) | 56,996,166 (6.55×) |
| sh delta stages | 404,342,880 | 58,278,930 (6.94×) | 58,797,730 (6.88×) |
| mover direct | 1,018,908,526 | 108,984,202 (9.35×) | 110,021,106 (9.26×) |

Every window of mover-setpiece, mover-pins, closet-reveal and enemy-movers is priced at or above its ledger, at `low` and `high`, cold, warm empty and full hit, each section at or under its term (`every_priced_phase_covers_its_window_on_every_mover_fixture`, 2,037 s). Stress-env-volumes' measured windows need its full cold bake.

## Review round 1

Panel: 3 correctness tracers, 1 contract verifier, 2 adversarial testers
(Opus), 3 hygiene/drift (Sonnet), over 4 slices. Every bake flow traced held
its contract (output identity, order-independent verdict, one mover's
states at a time, progress at its total).

Fixed:
- A huge `--mover-payload-max-size` overflowed the gate's unchecked
  arithmetic (3 reviewers): `price_movers` and `carried_bytes` are checked
  and refuse by `ArithmeticOverflow`; test
  `a_payload_cap_too_large_to_price_refuses_by_name`.
- The direct window priced the direct entries twice; `finish_section`
  holds them three times (bake, record parts, section): now 3×.
- The loop's buffers (two mover-absent depths, skip slots, cells, `valid`,
  the tile-range union, names) were priced only in the finish term: now
  `loop_bytes`, in every mover-bake term. Cells priced exactly; names at
  256 B.
- No mover: the bake returns before building anything.
- Posed solids priced whole (pose copy, `Arc`, read knots); the direct
  window adds the delta sections' per-cell vectors.
- The blocks' bytes against the cap are logged and in the Build Summary;
  the SH base refusal names the cap; the modder docs say the cap is charged
  three times in the direct phase and how `low` and `--release` differ.
- `BounceBlocks` keeps a running byte total: the cap check no longer walks
  every block per mover.
- Ledger groups no longer underflow on a guard made before the watch.
- Tests: the read-back seam counts its firings (`read_backs_failed`, M3,
  M5), M4 fails the second job's read and proves bounce one built there,
  the closet window test applies the cap to warm, full-hit and id-34-hit
  runs and asserts each routed section non-zero, L1 proves layers hit,
  the delta-cap test proves its premise, a new test checks progress ends
  at its total after lazy builds, a price edge test checks no bounce-one
  copy at `light_bounces` 0, the parity harness checks its full-hit log.
- Comment drift: the dead `refuse_cut_short` link, `PoseSlot::held`, the
  stage's progress doc, `window_ledger` header, `finish_bounce_half`'s
  flag, `pipeline.rs`'s gate comment, `blocks.rs` header, `kinds()` doc.

Left, with reasons:
- `compiler_cli_contract::warren_zero_budget_projects_current_membership_before_base_sh_bake`
  (`#[ignore]`) fails on the parent too: the mover poses' own gate refuses
  the warren before the SH gates. Its parse now reads the new SH-delta
  message; the earlier failure is pre-existing.
- On mover maps with no dense deltas a zero budget now refuses by the
  SH-delta gate's name (it carries the channel): consistent with G1's
  cheaper-gate-first rule.
- Id 59 is priced about three times over inside `section_bytes` (≈25 MB at
  the full range): conservative, left.
- G1's "before any bounce ray" holds by construction (the gates return
  before the SH stage begins); not asserted by a counter.
- Test-file headers longer than two lines follow the existing mover test
  files' precedent.

## Review round 2

A pricing re-trace and a hygiene pass over the round-1 fix commit. Every
round-1 claim checked held. Fixed:
- `cold` in `price_movers` was still an unchecked add the cap reached (both
  reviewers): checked; the overflow test now covers caps just under each
  later term's overflow, not only u64::MAX.
- The posed solids were priced whole only in the direct window: now beside
  the scenes in every SH base phase. `valid` priced at its growth capacity;
  the finished movers' names priced exactly from the jobs' labels; the
  direct window adds the direct half's own cells.
- The lazy-build progress test now proves its seam fires.
- Doc wording: `price_movers`, `delta_metadata_bytes`, `loop_bytes`'
  union, `BounceBlocks::log`, the build_pipeline Report bullet (the
  summary's cap line), the modder error row ("while the movers' lighting is
  assembled").

Left: `animation_descriptor_indices` in the delta metadata term (a u32 per
animated light, negligible); the direct half's `geometry_vertices` copy and
reach decomposition (pre-existing, inside the window's slack at the default
cap).

Stress-env-volumes after both rounds (release, stop at the SH gates): SH
base 11,426,906,815 B (mover direct), SH-delta 9,536,186,384 B (2,530,860,560
carried); both admit at 16 GiB, the same at `low`, `high` and `--release`.

## Final gate

After review round 3 (`5af4960b`): `cargo fmt --check` ok; `cargo clippy
--target-dir target/preflight-clippy -- -D warnings` ok (two pre-existing
unused-dependency manifest warnings in `postretro` and `combat-model`);
`cargo test --no-fail-fast` 113 targets, 17,541 passed, 0 failed, 157
ignored; `cargo check --release` ok; `crate-graph --check` up to date;
telemetry and playback-trace reducer unittests ok. The workspace build
needed `libasound2-dev` and `libudev-dev` installed in this container.
On-demand suites not run here: the parity harness beyond the closet and
setpiece, the setpiece M1 variant, and any stress-env-volumes full bake.
