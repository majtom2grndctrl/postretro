# mover-channel-bounded-bake — plan of record

mode: resumable
status: approved
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

| AC | Proof | Status |
|---|---|---|
| 1 Output identity, every fixture (I1) | routine: `mover_sections_match_parent_goldens` (closet and its edits, `low`/`high`, cold/warm, 1 and 4 workers); on demand: `mover_bounded_bake_parent_parity` (every mover fixture, stress-env-volumes included) | achievable as stated |
| 2 Job order reversed (I2) | `reversed_job_order_keeps_sections_and_the_tile_range_verdict` (two doors of different reach; lowered `--probe-depth-tile-cap` variant puts one past the range) | achievable as stated |
| 3 No two movers' states live (M1) | `one_movers_states_are_live_at_a_time` (two-door edit, setpiece on demand; cold, warm empty, full hit; per-kind high-water ≤ largest one mover's) | achievable as stated |
| 4 Held mover not live during the other's bake (M2) | `a_held_door_is_not_live_while_the_other_bakes` (two-door edit primed with one door's `light_bounces` changed, both orders; mover-bake phase price ≥ ledger peak) | achievable as stated |
| 5 Failed slot read bakes there (M3) | `a_failed_slot_read_bakes_the_pose_and_matches_cold` (test hook fails the read after lookup; `low`/`high`; all held, and one held one pending) | achievable as stated |
| 6 Corrupt `high`, whole `low`; and slot failure at `high` (M5) | `a_corrupt_high_entry_serves_low_as_the_parent_did` (golden captured at the parent on the same crafted cache) and `a_high_read_failing_at_the_slot_bakes_at_the_high_count` | achievable as stated |
| 7 Lazy layers at a failed slot routed and priced (M4) | `a_slot_failure_after_blocks_join_is_priced` (two-door edit, `low`, full hit, second door's read fails) | achievable as stated |
| 8 Layers in flight ≤ cap; no stall at one permit (L1, L2) | `layer_resolution_never_exceeds_its_cap` (cap below workers, fixture with more layers than workers, 1 and 4 workers, cold/warm) and `layer_resolution_completes_at_one_permit` | achievable as stated |
| 9 Payload cap error and flag parsing (C1) | `the_payload_cap_fails_by_name_and_admits_at_its_bytes` (exact cap admits, one under fails, message names flag, bytes, movers; nothing dropped) and CLI tests in `tests/compiler_cli_contract.rs` (past `u64`, malformed) | achievable as stated |
| 10 Tile range before cap, single mover (I4) | `a_mover_past_both_fails_by_the_tile_range_before_skipping` | achievable as stated |
| 11 Cap on one mover, range on another (I3) | `cap_and_range_on_different_movers_fail_by_name_in_both_orders` | achievable as stated |
| 12 Each priced phase ≥ ledger peak in its window (P1, P2, W1, D1, D3) | routine: `every_priced_phase_covers_its_window_on_the_closet` (whole-pipeline harness; cold, warm empty, full hit, id-34 hit with memo misses, `low` over `high`-primed; cap set to the fixture's block bytes); on demand: same on the setpiece and every mover fixture | achievable as stated |
| 13 Finish-loop buffers routed and priced (P1) | asserted inside row 12's harness: skip and encode buffers, skip bars, mover records, probe cells held by capacity in the mover-bake phase | achievable as stated |
| 14 Direct window routes every section ≤ its term (D1, D2) | `the_direct_window_covers_every_section_alive` (closet; animated-light fixture with `--sh-analyze` for the clones) | achievable as stated |
| 15 `--sh-delta-max-size` below dense, at or above post-drop (D1) | `a_delta_cap_below_dense_still_prices_the_dense_window` | achievable as stated |
| 16 SH-delta projection covers carried bytes (D3) | `the_sh_delta_projection_covers_what_the_channel_carries` (peak and carried term alone) | achievable as stated |
| 17 Layer price covers largest in-flight layers, hit payload included (L1) | inside row 8's test: layer-phase price ≥ ledger peak, warm with some layers hit | achievable as stated |
| 18 Edges: no mover zero; bounces 0; fill 0 | `mover_channel_price_edges` (unit, `cost.rs`/`working_set.rs`) | achievable as stated |
| 19 Same SH base projection at `low` and `high` | `the_sh_base_projection_ignores_fidelity` over every mover fixture (plan only, no ray; routine closet, on demand the rest) | achievable as stated |
| 20 One byte under SH base peak refuses by its name (G1) | `one_byte_under_the_sh_base_peak_refuses_by_its_name_through_gate_order` (fixture with delta projection below the base peak, through `run`) | achievable as stated |
| 21 `build_pipeline.md` rows; `docs/level_design.md` flag | doc edit, reviewed against this table; a grep test that `--mover-payload-max-size` appears in `main.rs` help and `docs/level_design.md` | achievable as stated |
| 22 stress-env-volumes admitted at 16 GiB, `low`, every gate | stop-at-gate release run of `prl-build` (all gates, no bounce ray); a refusal moved to another gate stops the build for the owner | achievable as stated |
| 23 Estimate ÷ largest cold window per phase; `--release` admission | `#[ignore]` findings harness on setpiece and stress-env-volumes at `low` and `high`; stress cold may need the owner's machine (this container: 4 cores, 15 GB) | achievable as stated |
| 24 Block bytes against 256 MiB | the same harness's block counts, both maps, `low`/`high` | achievable as stated |
| 25 SH base time before and after | parent and branch binaries, cold, same two maps, layer resolution apart; warm full hit on setpiece at `low` | achievable as stated |
| 26 Owner's 16 GB desktop sampling | owner, cold `--no-cache`, private bytes every 30 s | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 0 | Before production edits: capture parent goldens (closet family, M5's crafted cache) and build the parent `prl-build` comparison binary for the parity harness; add the parity harness | integrating executor | — || done: `tests/mover_bounded_bake_goldens.rs` (5 goldens at 10df2fcb; every cache mode and worker count hashed alike), `tests/mover_bounded_bake_parity.rs` (closet identical against the parent binary, low and high, cold/warm/hit); parent `prl-build` kept at `<scratchpad>/parent/prl-build-parent` for this session |
| 1 | Per-mover finish, the riskiest slice: each slot resolves, reduces, edge-copies, writes memos, joins the depth union, takes the tile-range verdict, skips, encodes into incremental `BounceBlocks` and drops its states; held slots finish at their slot too; id-59 trim once after the last mover; ledger per-kind high-water. Proof rows 1 (closet goldens, setpiece parity), 2, 3, 10 | integrating executor | 0 || code done: `bake_mover_bounce` finishes each slot (`DepthUnion` verdict before skip, `BounceBlocks::push_mover`), pipeline's `skip_mid_poses` gone, ledger per-kind and group high-water. Goldens pass (25 builds); 276 mover/depth/skip unit tests pass; `finish::reversed_job_order_keeps_sections_and_the_tile_range_verdict` passes. M1 passes cold and warm empty; full hit fails until Task 2 (the lookup holds every held mover's states: 818,712 B against 719,544 B alone). Row 10 (I4) moves to Task 3, which adds the cap. Setpiece parity runs after Task 2 |
| 2 | One finish site: the lookup plans only (reads, decodes, drops); every mover reads back at its slot; a failed slot read builds layers, bounce one and id 34's later bounces lazily on the volume-hit path. Rows 4, 5, 6 | integrating executor | 1 || code done: `PoseLookup` reads, decodes and drops (no `HitReduction`); one `MoverSlot` kind; `SlotRadiance` builds bounce one and id 34's later bounces at the first slot whose read-back fails (`HitRadiance`, which extends the progress total); test seam `fail_read_back`. Goldens pass (M5's corrupt-`high` golden included); `finish::` M1 (cold, warm empty, full hit), M2, M3, M5 slot-failure and I2 pass. Full mover sweep and setpiece parity running |
| 3 | `--mover-payload-max-size`: flag, default 256 MiB, pre-join check, named error, help text. Rows 9, 10, 11 | integrating executor | 1 | |
| 4 | Split `pipeline.rs` along its stage seams (own commit, no behaviour change); then the whole-pipeline measured harness with sections routed and phase marks at the SH-delta and direct windows | integrating executor | 1 | |
| 5 | Pricing: drop the accumulated-states term; price one mover's states, the blocks at the cap, the finish loop's buffers and the lazily built slot inputs in the mover-bake phase; the direct window after the SH delta and direct-SH stages with ids 27/41/45 at the dense bound; the carried term in the SH-delta projection. Rows 7, 12–16, 18–20 | integrating executor | 2, 3, 4 | |
| 6 | Layer cap: `resolve_layers` in batches of 16 (test seam), layer-phase price over finished layers plus the 16 largest in-flight scratches, hit payload included. Rows 8, 17, and row 1 re-run | integrating executor | 5 | |
| 7 | Context and modder docs: `build_pipeline.md` flag rows, *Memory* paragraph ("Decided, not yet built" retired), `docs/level_design.md`. Row 21 | integrating executor | 6 | |
| 8 | Findings: stress-env-volumes stop-at-gate admission, findings harness on setpiece and stress-env-volumes, block bytes, stage times; on-demand parity over every mover fixture. Rows 22–25 | integrating executor | 6 | |

Delegation: none planned. Tasks 1–3 and 5 share `bake_mover_bounce`, the
pose lookup and the price; task 4 rewires the file they all touch. Sequential
on one warm `target/`.
