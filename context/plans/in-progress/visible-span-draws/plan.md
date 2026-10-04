# visible-span-draws — plan of record

mode: compact
status: test-ready
read at: 1b1c1caeb

## Corrections
- The installed stable toolchain (Rust/Clippy 1.98) now flags constant-size `chunks_exact` loops in existing source. The final lint gate required equivalent `as_chunks`/`as_chunks_mut` iterations in 21 files; these preserve remainder truncation and byte order, use no unsafe, and remain compatible with the workspace's Rust 1.89 minimum. They received a separate mechanical review.
- AC3 “draws” means indirect driver slots, as defined by AC1 and the Problem’s `2·L` cost; AC10–11 use “draw calls” for coalesced runs. Zero-index leaf holes may create extra runs, but cannot increase drawn slots beyond whole buckets. The independent seam reviewer verified this clarification preserves the contract.
- `record_pre_scene_compute` now lives in `render/renderer_pre_scene.rs`, not `renderer_render_frame.rs`; build the shared camera ranges at that actual pre-scene seam.
- Existing stress-probe origins used raw Quake units and campaign yaw had the wrong sign; use loader-matching engine conversion `(-qy, qz, -qx) × 0.0254` and reversed Quake yaw. This corrects the fixture Path without changing Decisions or Acceptance.
- No crate source changed from the brief read revision 4278d8789 through the claim commit; grounded Decision reads remain valid.
- Acceptance rows below are numbered in their original order for unambiguous proof tracking. GPU-backed frame tests use the existing offscreen adapter harness and report adapter execution explicitly.

## Delegated answers
- None; the brief has no open questions.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| 1. Across both camera passes, indirect draws per pass equal the summed span lengths of the distinct visible cells. A cell named twice counts once. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 2. A visible set that leaves out some of a bucket's cells draws fewer slots in that bucket than the bucket holds. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 3. A set that names every cell draws every drawable leaf, with no more draws than the whole-bucket path. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 4. An empty visible set, or one whose cells own no spans, issues zero indirect draws and no material binds, without error, including on the frame after a nonempty set. Neither falls back to whole buckets (O3, O4). | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 5. One visible cell draws exactly that cell's spans. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 6. A bucket with no visible span issues no draw and no material bind. A bucket with one visible span issues a draw in both camera passes and a material bind in the forward pass only. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 7. On a headless frame over a level whose visible set leaves out some cells, both camera passes draw the list built from that set, and it holds fewer slots than the level's buckets. The same frame with a draw-all set draws whole buckets. | `headless_` production recorder tests and draw-plan trace | achievable as stated | pass |
| 8. After a level install, the first frame draws ranges built from the new level's draw index and bucket ranges, even when its visible set equals the previous level's last set. A draw-all level followed by a celled level draws spans on the celled level's first frame (O11, O12). | `headless_` production recorder tests and draw-plan trace | achievable as stated | pass |
| 9. Building the ranges reads only the visible cells' spans and walks each bucket range at most once. On a synthetic world with many cells and one visible cell, it touches no span or leaf of any other cell. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 10. Two abutting visible spans in one bucket go out as one draw call, whichever order the visible set names their cells. Two spans in one bucket separated by a gap of even one slot go out as two. Two abutting spans that straddle a bucket boundary never merge (O8). | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 11. A bucket with K maximal visible runs issues exactly K draw calls and one forward material bind. The per-draw fallback also binds once per bucket. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 12. No drawn range includes a slot outside a visible span. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 13. Every leaf the camera cull submits lies inside a drawn range. This holds on synthetic worlds for the portal walk, the step-limit fallback, and the solid-cell, exterior and no-portals fallbacks, and in the on-demand stress-map probes. | `candidate_cull_mirror` and ignored `stress_map_probes` | achievable as stated | pass — synthetic paths and all four real-map poses |
| 14. The on-demand stress-map probes include stress-warren-hallway-inspection. At each probe pose, campaign-test included, every submitted leaf lies inside a drawn range, and the slots drawn per camera pass equal the visible cells' summed span lengths. The probe reports the coalesced runs and the drawn slots per camera pass, and the plan of record records both beside the map's total leaves. | `candidate_cull_mirror` and ignored `stress_map_probes` | achievable as stated | pass — exact counts for all four table poses recorded below |
| 15. On a headless frame given a fog-reach set that differs from its drawable set, the camera passes draw the drawable set's spans exactly. | `headless_` production recorder tests and draw-plan trace | achievable as stated | pass |
| 16. A draw-all frame draws whole buckets. A cell set that names every cell takes the span path. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 17. A visible cell id past the loaded index draws whole buckets for that frame, on the portal path and on the solid-cell and exterior paths alike, and whether the bad id comes before or after valid ones. The next frame whose ids are all in range draws spans. A set whose largest id is the last valid cell takes the span path (O5, O6, O7). | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 18. With no draw index loaded, the camera passes draw whole buckets. With one loaded, they draw spans. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 19. On every path above, the depth prepass and the forward pass issue identical ranges within a frame (O10). | `headless_` production recorder tests and draw-plan trace | achievable as stated | pass |
| 20. A recorded frame builds its draw ranges once, before the depth prepass. Neither camera pass rebuilds them. | `headless_` production recorder tests and draw-plan trace | achievable as stated | pass |
| 21. Shadow depth passes recorded between the range build and the camera passes leave the camera list unchanged. On a frame with an occupied shadow slot, the depth prepass and the forward pass draw exactly the list built from the frame's visible set (O9). | `headless_` production recorder tests and draw-plan trace | achievable as stated | pass |
| 22. A frame draws the ranges built from its own visible set. Consecutive frames with different visible sets, on the candidate path, the tree-walk path, and switching between them, never draw the previous frame's ranges. Consecutive frames with the same set draw the same ranges. Path switching is proved on consecutive headless frames that alternate a portal path and a solid-cell path (O1, O2). | `headless_` production recorder tests and draw-plan trace | achievable as stated | pass |
| 23. Without multi-draw-indirect support, the per-draw fallback issues the same ranges one slot at a time. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 24. Shadow passes still draw whole buckets (regression guard). | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 25. Once the builder has handled a visible set at least as large, rebuilding the ranges for any set, changed or unchanged, allocates nothing. | `visible_ranges` + `draw_plan_tests` | achievable as stated | pass |
| 26. The indirect-contract scanner passes, and its owner rules and inventory counts are unchanged from the read revision. This is a diff gate on `indirect_contract_tests.rs`, not a behavior test. | `indirect_contract` (12) plus unchanged-file diff gate | achievable as stated | pass |
| 27. On this Mac, in a release build with the Auto preset, indirect validation at its release default (off, `WGPU_VALIDATION_INDIRECT_CALL` unset), the window in front and no tracer attached, record the `[CpuTiming]` `render_submit` and `work` medians on stress-warren-hallway-inspection and campaign-test, before and after. Use at least five windows under the same recorded shadow-cache state, as in `release-indirect-validation`. `render_submit` falls on stress-warren-hallway-inspection and does not rise on campaign-test. Record the numbers in the plan of record. | owner, release in-engine runbook | manual proof blocks landing | outstanding manual proof |
| 28. A `sample` profile of render-pass encoding on both maps, before and after on the same build base, shows the time in wgpu-hal Metal `draw_indexed_indirect` (the `drawIndexedPrimitives` loop) falling. Report the before and after figures. At 4278d8789 the reconciled baselines were about 0.83 ms and 0.08 ms. | owner, release in-engine runbook | manual proof blocks landing | outstanding manual proof |
| 29. Headless captures at fixed poses are byte-identical before and after: a portal-walk pose on each map, plus one solid-cell or exterior pose. | owner, release in-engine runbook | manual proof blocks landing | outstanding manual proof |
| 30. No visual change in play on either map. | owner, release in-engine runbook | manual proof blocks landing | outstanding manual proof |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Pure visible-span builder, two-bucket coverage slice, reusable scratch and bounds proofs | bounded builder worker + integrating executor | — | done — seven visible_ranges tests pass, including sparse work and zero-allocation proofs |
| 2 | Shared camera range lifecycle and pure draw plan; preserve whole-bucket shadows | integrating executor | 1 | done — two pure draw-plan tests and 12 unchanged indirect-contract tests pass |
| 3 | Synthetic cull coverage and stress-map probes with counts | bounded probe worker | 1 | done — 10 mirror tests, 1 probe-table test, 2 selected ignored probe runs pass over all four table poses |
| 4 | Headless frame ordering, path switches, reinstall and shadow interleave proofs | integrating executor | 2 | done — two production headless tests ran 35 adapter frames, including an occupied shadow slot |
| 5 | Focused readiness, review/fix loop, final preflight and AC results | integrating executor | 3, 4 | done — review/fix loop clean, final preflight passes, all four on-demand poses pass; AC27–30 await manual proof |

## Manual runbook

Manual rows block landing. At the same fixed poses on campaign-test and stress-warren-hallway-inspection, use release Auto, WGPU_VALIDATION_INDIRECT_CALL unset, window foreground and no tracer during timing. Record at least five complete 120-frame CpuTiming windows with the same recorded shadow-cache state, report medians of render_submit and work for baseline 1b1c1caeb and feature. Separately record sample profiles on each build/map and report the Metal draw_indexed_indirect time. Compare byte-identical headless captures at both portal poses plus a solid/exterior pose. Owner plays both maps and confirms no visual change. Measured probe counts are recorded below. Reproduction commands (workspace root):

```sh
# Build baseline 1b1c1caeb and the feature revision separately, preserving both
# binaries and their hashes. Use identical release build settings for both.
CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro -p postretro-script-compiler

# Run each map on both binaries, select Auto, keep the window in front.
env -u WGPU_VALIDATION_INDIRECT_CALL RUST_LOG=info POSTRETRO_CPU_TIMING=1 cargo run -p xtask -- run --release -- content/dev/maps/stress-warren-hallway-inspection.prl
env -u WGPU_VALIDATION_INDIRECT_CALL RUST_LOG=info POSTRETRO_CPU_TIMING=1 cargo run -p xtask -- run --release -- content/dev/maps/campaign-test.prl

# Profile in a separate run after warmup; substitute the launched engine PID.
sample <engine-pid> 10 1 -file /private/tmp/visible-span-draws-<revision>-<map>.sample.txt

# Deterministic headless captures; build both revisions with this same feature.
CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro --features capture
env -u WGPU_VALIDATION_INDIRECT_CALL RUST_LOG=info POSTRETRO_SH_STREAMING=sync-proof target/release/postretro --capture <scene.json>
```

Use the versioned scene inputs under `measurements/release-indirect-validation/screenshots/` as the portal-pose templates (campaign-spawn-off and hallway-spawn-off), copying them with new output paths so existing evidence is preserved. Add a fixed solid-cell or exterior scene, confirm its visibility provenance, and use precisely that scene on both binaries. Compare decoded pixel bytes/PNG hashes on each before/after pair. Captures use the +0.5 m eye offset and FOV 100°, separately from the FOV 90° CPU coverage poses below. Keep map/settings/binary digests with results.

For timing, collect eight complete 120-frame windows, discard the first three and take the final-five median for both render_submit and work. No Cargo, profiler or tracer runs during those samples. Record matching warm shadow-cache state separately with labelled Metal pass counts as in `measurements/release-indirect-validation/runtime/README.md`; unchanged options alone do not prove matching cache state. Profiles report actual Metal draw_indexed_indirect cost, not slot-count estimates. The historical 0.83/0.08 ms figures are context, not an accepted paired baseline for this build. Owner confirms both maps look unchanged at spawn and during play.

## Verification log
- Touched-crate `cargo check -p postretro-renderer -p postretro`: pass.
- Pure builder `cargo test -p postretro-renderer --lib visible_ranges`: 7 matched, 7 pass; includes zero allocations after warmup across changed sets/fallbacks and sparse CSR/bucket work.
- Indirect scanner owner rules and inventory file diff from 4278d8789: empty.
- Headless production recorder: 2 matched tests pass; 30 path/ordering frames + 5 reinstall/shadow frames actually executed on the adapter, with no skip. Both camera traces agree, ranges build once, forward binds once per occupied bucket, depth binds none, and real occupied shadow slot draws whole buckets.
- `draw_plan_tests`: 2 matched, 2 pass; multi-draw and per-slot fallback issue matching slots and binds, including shadow-region offset.
- `indirect_contract`: 12 matched, 12 pass; owner rules/inventory and scanner source remain unchanged.
- CPU mirror: 10 matched tests pass, including real visibility production on portal, step-limit, solid, exterior and no-portals synthetic paths.
- Probe table: 1 matched test passes; includes all four existing/required map names.
- Ignored probes: two selected runs, each 1 matched/1 pass, cover all four table poses. Hallway and campaign use prebuilt PRLs. The final acceptance audit correctly treats “at each probe pose” as all four entries; stress-warren and stress-warren-crates were then baked to temporary PRLs by the production compiler and both pass. Their bakes use the harness’s 10 m SH spacing / 0.5 lightmap density and warm approximate lighting; these CPU topology/slot proofs do not establish image or performance parity.

## Real-map probe results

| Map / fixed pose | Total leaves | Coalesced runs per camera pass | Drawn slots per camera pass | Submitted leaves | Path |
|---|---|---|---|---|---|
| stress-warren-hallway-inspection; [42.2656, 2.4384, 63.3984], yaw 0°, pitch 0°, horizontal FOV 90°, 16:9 | 8,437 | 4 | 32 | 13 | PrlPortal, walk reach 8 |
| campaign-test; [-65.8368, 1.8288, -45.9232], yaw -90°, pitch 0°, horizontal FOV 90°, 16:9 | 774 | 29 | 149 | 127 | PrlPortal, walk reach 32 |
| stress-warren; [81.28, 2.4384, 97.536], yaw 0°, pitch 0°, horizontal FOV 90°, 16:9 | 4,425 | 4 | 4 | 4 | PrlPortal, walk reach 1 |
| stress-warren-crates; [48.768, 2.4384, 65.024], yaw 0°, pitch 0°, horizontal FOV 90°, 16:9 | 4,473 | 11 | 19 | 12 | PrlPortal, walk reach 5 |

All four maps prove exact distinct-visible-cell span lengths and coverage of every submitted leaf. These are CPU coverage probes, not performance or image proof.

- Final focused readiness: format passed after trimming the extracted fixture's trailing blank line; touched-crate check passed; both headless tests re-ran on final fixture layout and executed all 35 adapter frames.

## Review and fix loop
- Isolated review panel: two slices plus the range-handoff seam; 3 correctness tracers, 2 adversarial testers (Sol xhigh), and 2 hygiene/drift reviewers (Sol medium).
- Verdict: approve with documentation nits; no functional or architectural findings.
- Two nit findings touch three comment locations: durable context references in both new headless test headers, and converted spawn-yaw wording in the probe table. Prescribed Luna medium workers handled one file each; no behavior, Decisions, Acceptance, or indirect scanner rules changed.
- Post-fix gate: touched-crate check passes; probe_table 1 matched/1 pass. Sandbox headless retry found no adapter and skipped both tests; host-access retry then executed all 35 adapter frames and both tests passed. The skipped retry is not counted as GPU proof.
- Final lint repair review: a fresh Luna medium hygiene/drift reviewer approved the separate 21-file, 112-line mechanical slice with no findings. Chunk order, ignored tails, mutable iteration, endian decoding and slice coercions are unchanged; fixed-array slice APIs are available at Rust 1.89.
- Focused retest after lint repairs: all 577 level-format unit tests pass.
- Renderer retest after lint repairs: 12/12 indirect-contract tests pass, scanner/owner/inventory diff from 4278d8789 stays empty, and 2/2 headless tests execute all 35 adapter frames.
- Final preflight: `cargo fmt --check` and `cargo clippy --target-dir target/preflight-clippy -- -D warnings` pass. Full `cargo test` on the host passes: 9,266 tests, zero failures, 42 expected ignored tests across 60 target results. The required ignored coverage probes are run separately; no code changes followed this gate.

## Landing state

All 26 automated Acceptance rows pass. AC27–30 remain outstanding manual proof and block landing: paired release timings, paired Metal profiles, byte-identical captures, and owner live play on both measured maps. Keep the brief in `in-progress/`; wait for those results and the owner’s “land the plane.”

No source changes followed final verification. A concurrent edit to `context/lib/testing_guide.md` appeared during verification and is excluded from this feature commit.
