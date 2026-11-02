# lightmap-reach-cull gate 2 (HEAD 80610f03e; new binary bin-fix; base = main b6fc0c4ee bin-base)
Logs: $SP/runs2/*.log.clean (SP = /private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/feb8fef1-0fa0-4a4a-8725-c88f8fbeb0dc/scratchpad). Scripts queue2..5.sh. Throttle lifted by coordinator before the hallway new bake (hallway base ran niced -j 6; hallway new uncapped).

## Preflight
fmt --check OK; clippy (-D warnings) OK; clippy --all-targets OK; cargo test workspace: 13931 passed, 0 failed, 100 ignored (postretro 2333/0/16, level-compiler lib 2243/0/25, others all 0 failed); cargo check --release OK; crate-graph --check up to date.

## Ignored tests
- compiler_cli_contract full_pipeline_closes_layer_cache_reads_at_the_fused_return_boundary: PASS
- coloured_bounce_cache pre_colour_stage_cache_misses_both_sh_stages_while_unchanged_stages_hit: PASS
- coloured_bounce_cache cold_and_warm_builds_cache_entries_only_for_pre_change_stages: FAIL. Assert on PRE_CHANGE_CACHED_STAGES: expected list contains no "cell_residency_set" but the cold build caches it (stage in cell_residency_bake.rs, CELL_RESIDENCY_SET_STAGE_ID). Stale test list, not a verify/reach issue (test sets reach_verify=false and every reach stage matches). The list was last touched by d2c1bc946 (older); this branch's edit was only the 8-line reach_verify opt-out. Needs "cell_residency_set" added to PRE_CHANGE_CACHED_STAGES (not fixed, per rules).

## Binary
bin-fix/{prl-build,scripts-build}, COMMIT = 80610f03e2cc5531261c1a18779066efb5467f55.

## Whole-PRL cmp (new verify=1 vs base verify unset, --release): ALL IDENTICAL, all exit 0, no ERROR lines, no verify mismatch
| map | walked | culled | skipped | traced in culled | re-walked | proof |
|---|---|---|---|---|---|---|
| hallway-mini | 28769 | 14106 | 594356506 | 4785705 | 14106 | 23914/23914, 0 unproven |
| hallway-mini --no-penumbra-floor | 28769 | 14110 | 598241334 | 904860 | 14110 | 23914/23914 |
| movement-feel | 1740 | 1 | 0 | 6823 | 1 | 1698/1698 |
| kinematic-platform | 170 | 0 | 0 | 0 | 0 | 374/374 |
| gate-heavily-lit | 226 | 0 | 0 | 0 | 0 | 184/184 |
| combat-demo | 246 | 0 | 0 | 0 | 0 | 364/364 |
| soft_shadow_test | 118 | 0 | 0 | 0 | 0 | 88/88 |
| soft_shadow_test --no-penumbra-floor | 118 | 0 | 0 | 0 | 0 | 88/88 |
| hallway (3,256,131,523 B PRL) | 83022 | 45360 | 1732200170 | 13297187 | 45360 | 48062/48062, 0 unproven (0.000 m) |
Harness note: my first script flagged hallway-mini on grep "mismatch" matching the benign face_extract WARN "mismatched textures"; cmp was OK; the check was changed to ^[ERROR|^error and mini was kept as a valid row (its stage line above is from that run); mini-npf onward ran under the fixed script.

## Border cost (hallway)
13297187 / 1732200170 = 0.768% (gate <= 10%): PASS. Versus earlier run: culled pairs 45360 of 83022 unchanged; skipped texels 1732138949 -> 1732200170 (+61221); traced inside culled 13358408 -> 13297187 (-61221).

## Warm verify (hallway-mini, cache dir, no --release, new binary)
- fill (verify unset): lightmap_section miss, 14106 culled, cold.
- warm verify=1: `[cache] lightmap_section bypassed (reach verify)`; 14106 culled, 14106 re-walked (equal); 44662 partitions walked, 0 from cache; PRL identical to fill: YES.
- warm verify unset: `[cache] lightmap_section hit`; `Lightmap reach cull: stage served from the lightmap_section memo, no pair walked`; PRL identical to fill: YES.

## Anomalies
1. Stale ignored test (above), pre-existing cell_residency_set omission.
2. Disk dipped to 10 GB during the warm rows (10 GB cache dir); cleaned (cache, PRLs, target/debug/incremental 11 GB deleted). All scratch PRLs deleted at end.
3. Skipped-texel counts are 58-61k above gate-report 1 (mini 594356448 -> 594356506, hallway as above), consistent with the review fixes; PRLs still identical to baseline.
