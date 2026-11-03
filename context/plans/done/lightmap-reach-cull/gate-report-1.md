# lightmap-reach-cull Task 6 gate report (new = 8ce7eb8ad, base = main b6fc0c4ee)

Overall: every row passes. One anomaly: kinematic-platform timing (row 4), see below. Logs: $SP/runs/*.log (SP = /private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/feb8fef1-0fa0-4a4a-8725-c88f8fbeb0dc/scratchpad). `.log.clean` variants hold CR-stripped copies.

## Row 1: whole-PRL cmp, new with POSTRETRO_LIGHTMAP_REACH_VERIFY=1, --release, vs baseline
| map | cmp identical | Lightmap reach cull line (new) |
|---|---|---|
| hallway (vs hallway-base.prl) | YES | 83022 pair(s) walked, 45360 culled, 45360 re-walked by verify mode; 1732138949 texel(s) skipped, 13358408 traced inside culled pairs; 221801 partition(s) walked this build, 0 from cache |
| hallway-mini | YES | 28769 walked, 14106 culled, 14106 re-walked; 594356448 skipped, 4785763 traced inside culled pairs; 44662 partitions |
| movement-feel | YES | 1740 walked, 1 culled, 1 re-walked; 0 skipped, 6823 traced inside culled pairs; 435 partitions |
| kinematic-platform | YES | 170 walked, 0 culled, 0 re-walked |
| gate-heavily-lit | YES | 226 walked, 0 culled, 0 re-walked |
| combat-demo | YES | 246 walked, 0 culled, 0 re-walked |
| soft_shadow_test | YES | 118 walked, 0 culled, 0 re-walked |
| hallway-mini --no-penumbra-floor | YES | 28769 walked, 14110 culled, 14110 re-walked; 598241287 skipped, 904907 traced inside |
| soft_shadow_test --no-penumbra-floor | YES | 118 walked, 0 culled, 0 re-walked |
All: re-walked == culled, no mismatch error, every bake exit 0. Side facts: hallway culled 54.6% of pairs walked, mini 49.0%; hallway traced-inside / skipped = 13358408/1732138949 = 0.77%. With verify on the hallway Lightmap Bake was 1071.47 s, SH Bake 127.16 s (verify run, not a timing row).
Disk: hallway PRLs deleted after cmp (also deleted stale hallway-final.prl, hallway-new-counted.prl, hallway-base.prl after the last hallway row, to free space; one mid-run ENOSPC hit a verbose rerun, see Row 5 note).

## Row 2: verify switch (hallway-mini, release, cold)
| case | result | PRL == g1-mini-new |
|---|---|---|
| =1 | 14106 culled, 14106 re-walked by verify mode | (is the reference) |
| =0 | `14106 culled; 594356448 texel(s) skipped...` no "re-walked" text, no re-walks | identical |
| unset | same as =0, no re-walks | identical |
| =banana | exactly one live warning: `[WARN prl_build::pipeline::lightmap_stage::reach_verify] POSTRETRO_LIGHTMAP_REACH_VERIFY="banana" is neither 1 nor 0; lightmap reach verify mode stays off`; no re-walks. (The end-of-build WARN summary replays it once more, as it does for every WARN in the log: 1 live emission at log line 669, 1 summary replay.) | identical |
| warm fill (--cache-dir cache-verify, no --release, verify unset) | cold, no re-walk | |
| warm =1 (same cache) | `[cache] lightmap_section miss`; 28769 walked, 14106 culled, 14106 re-walked by verify mode; 44662 partitions walked this build, 0 from cache. Re-walked equals culled, not served from cache | identical (cmp OK) |
Zero mismatches and re-walked > 0 from row 1 logs: hallway 45360, hallway-mini 14106, movement-feel 1. PASS (no mismatch error in any).

## Row 3: coverage proof (logs of row 1)
| map | proof line | cull runs | Watertightness at INFO | hole/watertightness WARN |
|---|---|---|---|---|
| hallway | 48062 of 48062 edge(s) proven, 0 unproven span(s) (0.000 m) in 0.104 s | yes | `[INFO prl_build::pipeline] Watertightness (edge parity): 26 odd-covered edge span(s)...` | none |
| hallway-mini | 23914 of 23914 proven, 0 unproven in 0.049 s | yes | 4 odd-covered spans, INFO | none |
| movement-feel | 1698 of 1698 proven, 0 unproven in 0.005 s | yes (1 pair culled) | 146 odd-covered spans, INFO | none |
Every WARN line (unique after normalising ids; each also repeats once in the end summary):
- hallway: 638 `partition::face_extract` "Coplanar brush sides with mismatched textures ..."; 16 `animated_light_chunks` "face N chunk bottomed out at min-extent with 5 animated lights (cap 4); retaining all overlapping lights"; 2 `animated_light_chunks` "3667481 chunks exceeded cap 4 ..."; 6 `cluster_directory_bake` "SH cluster cell N is an indivisible singleton with P primitives, exceeding limit 64" (3 cells: 1897/169, 1490/163, 5572/140); 2 `delta_sections` "SH delta authoring budget warning: id 27 64553328 bytes, id 41 15021072 bytes ...".
- hallway-mini: 290 `face_extract` coplanar texture mismatch; 2 `cluster_directory_bake` singleton warnings; `buried_lights` "Light inside solid geometry: entity N 'light_spot' ... Excluded" for each of 107 lights, plus "107 static light(s) inside solid geometry were excluded".
- movement-feel: `shadowmask_bake::ownership` "[ShadowmaskAtlas] entity N 'light' ... lost its specular mask channel on face N to four higher-ranked lights" (about 26 entities, 14 to 48 each); 8 `quake_map` "light entity missing '_color'; defaulting to white".
None of them mention a hole or watertightness (grep -iE "hole|watertight|open edge" on WARN lines: 0 in all three).

## Row 4: small-map timings, verify unset, --release, cold, idle (pre-run pgrep empty each time), alternating base/new/base/new
| map | base1 | new1 | base2 | new2 |
|---|---|---|---|---|
| movement-feel Lightmap Bake | 33.29 | 33.29 | 34.00 | 33.98 |
| movement-feel SH Bake | 3.45 | 3.53 | 3.60 | 3.50 |
| kinematic-platform Lightmap Bake | 12.97 | 14.15 | 12.61 | 13.90 |
| kinematic-platform SH Bake | 5.61 | 5.20 | 5.13 | 5.03 |
movement-feel: new within base spread (33.29-34.00): PASS.
kinematic-platform ANOMALY: both new runs (14.15, 13.90) fall OUTSIDE the base spread (12.61-12.97). Two extra alternating pairs confirmed it: base 12.50, 12.64; new 13.70, 13.74. So about +1.1 s (+9%) consistently on this 13 s bake; SH Bake unchanged (control), cull culled 0 pairs there, coverage proof takes 0.001 s, so the cost is somewhere in the reach index build or its flood in the Lightmap Bake stage with nothing to cull. Not within "measurement noise"; the owner should rule on it (the brief's row says within noise).

## Row 5: warm over baseline-filled cache (hallway-mini, verify unset, no --release, cache-dir cache-warm)
- Baseline fill: exit 0, PRL identical to cold new (cmp OK).
- NEW over that cache: `[cache] lightmap_section hit`; stage line: `Lightmap reach cull: stage served from cache (0 partition(s) loaded, no pair walked)`, Lightmap Bake 1.60 s. PRL cmp-identical to g1-mini-new: YES.
- Identifying the section entry: cache files are named by blake3 digest only, no stage id inside (header PRC2 + length + hash), so identification was by atime. Entries read during the warm run were the 9 files with atime 09:08:08-09:08:22. The two 600-800 MB candidates were tried in turn, moved to a side directory:
  - cbb184915f97...a669 (799937593 B): the rebake logged `[cache] shadowmask_atlas miss`, section hit: it was the shadowmask atlas, put back by the rebake and the side copy deleted.
  - 099d802ae6ea72fffb172dd1e28f38240c0ce0679e5f57edac59599ec74ec2fd (599932580 B): the one lightmap_section entry. Rebake with NEW + --verbose: `[cache] lightmap_section miss`, 44662 `[cache] lightmap_layer hit` lines (one per partition), 0 `lightmap_layer miss`; stage line `stage served from cache (44662 partition(s) loaded, no pair walked)`. PRL cmp-identical to cold new: YES.
- Deviation: the first verbose attempt (section hit) died with ENOSPC (disk filled by PRLs and caches, got to 1.1 GB free); I deleted scratch PRLs and cache-verify and reran. Also a WARN every run: cache spared set 10.04 GiB exceeds the 2.00 GiB budget (the cache keeps the spared set; informational).

## Row 6: grep gate, git diff b6fc0c4ee..8ce7eb8ad (66 files)
- (a) PASS: no `#[arg` / `long =` / clap additions; main.rs gains only `pub mod coverage_proof;`, `pub mod lightmap_reach;`, `#[cfg(test)] mod gate_map_geometry;`.
- (b) PASS: no `POSTRETRO_LM_`, `lm_leak_trace`, `REACH_PROFILE` in added lines. `lm_flood_sizing` appears once, in an added plan-doc line (markdown pointer to the spike branch file), not in code.
- (c) PASS: no added or removed line matches `_VERSION` or `_EPOCH`.
- (d) PASS: no `0.0005`, `5e-4`, `5.0e-4` in any non-test coverage_proof file (incl. pipeline/coverage_proof_stage.rs). Hits exist only in tests/fixtures: coverage_proof/test_fixtures.rs:299, hinge_tests.rs:73, 298, 299, 322.
- (e) PASS: `check_watertight` is not read by the cull or proof code; only coverage_proof/tests.rs:12 (import) and :255 (parity test comparison). No `WatertightReport` anywhere in lightmap_reach* or coverage_proof*.
- (f) PASS: callers identical at both commits: affinity_grid.rs:215, chunk_light_list_bake.rs:427, pipeline/runtime_cells/tests.rs:13 (import), :947, :987, plus the definition at affinity_grid.rs:253.
- (g) PASS: `lightmap_reach*` has no `RuntimeCells`, `runtime.portals`, `CellTree`; the one `blocked` hit is a doc comment, lightmap_reach/slab.rs:73.
