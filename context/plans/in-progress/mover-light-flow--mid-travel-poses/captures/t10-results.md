# Task 10: gate report, memory gate and ledger, results

Measured 2026-11-11 on the Intel i9 MacBook Pro (`-j 6` under `nice`), on this branch at eca307498 plus the task 10 working tree. New builds are this tree's cargo `--release` `prl-build`; pre-change builds are 83dfb5317's (`t5/bin/prl-build-83df`). Every build is `prl-build <map> --no-tui -v -j 6`, run once per scenario, back to back.

## What changed

- The mover light gate's report counts, per mover, the baked lights the probe channel gates only at a mid pose: some probe's q below 255 at a mid pose, and every probe's q 255 at both termini, over every cell's entry of that mover. The direct half computes it from its own bake (`mover_probe_channel/mid_pose_only.rs`), naming each light by source index (`ChannelLight::source_index`).
- The Build Summary block prints one line per mover with any, after its four total lines: `  <mover>: N baked light(s) gated only at a mid pose`. `-v` adds one info line per such mover after the per-terminus lines, naming up to four lights. A mover with none adds nothing, so the block is unchanged on every map without such a light.
- The per-terminus rows, lines and totals are untouched.

## Report comparison (AC 13, report half)

The `[MoverLight]` per-terminus `-v` lines and the four Build Summary lines, pre-change `--release` against new `--release`, diffed as text:

| Map | Mover | Per-terminus lines (first / last waypoint), old → new | Mid-pose-only lights (new) |
|---|---|---|---|
| mover-closet | `closet_door` | 3 / — → 3 / —, identical | 0 |
| mover-setpiece | `setpiece_wall` | 8 / 6 → 8 / 6, identical | 0 |
| mover-setpiece | `setpiece_room` | 6 / 6 → 6 / 6, identical | 0 |
| enemy-movers | `em_lift` | 4 / 7 → 4 / 7, identical | 0 |
| enemy-movers | `em_sweeper` | 7 / 7 → 7 / 7, identical | 0 |
| enemy-movers | `em_crusher_open` | 8 / 7 → 8 / 7, identical | 0 |
| enemy-movers | `em_crusher_wall` | 7 / 6 → 7 / 6, identical | 0 |

- The whole report diffs empty on all three maps: rows (3, 26, 53), gated texel and probe estimates, side counts and mask estimates are byte-identical.
- No mover on these maps has a light only a mid pose gates, so the new lines print nowhere. A cross-check over the direct bake itself (fixture scene, every static light) agrees. On the setpiece, 12,986 (probe, light, mover) cells hold q below 255 at some mid pose, 12,640 of them also at a terminus. On enemy-movers it is 2,330, with 1,401 also at a terminus. Every light a mid pose gates there is also gated at a terminus of the same mover.
- The nonzero path is proved by the crossing-slab fixture: `a_light_only_a_mid_pose_blocks_is_gated_at_that_pose_alone` counts its light for the slab. The classification unit test is `a_light_counts_only_where_a_mid_pose_alone_gates_it`, and the summary line test is `each_mover_with_mid_pose_only_lights_adds_one_line_after_the_totals`.

## Memory (AC 30)

The brief's "termini's bounce-one layers over the domain" is stale. Since task 6b a terminus gathers every valid probe and never the domain, and task 8 found that memo moot, so there is nothing of that kind to price. The gate prices what the bake holds now:
- every pose's posed scene
- one pose's posed bounce and later-bounce buffers
- a mid pose's near-field states, gather, domain patches and slots
- id 34's later bounces kept for the movers (the mover-absent bake, read past the domain)
- the raw states one mover's poses hold before it reduces
- the accumulated states, direct entries, posed solids and one id-57 section

Fidelity never enters the price: `low` and `high` price the same 130,585,882 B on the setpiece. A `low` pose bakes one posed bounce and allocates no posed later-bounce buffer, so its bound is looser.

### Priced against measured, `mover-setpiece`

| | `low` (`--no-cache --bake-fidelity low`) | `high` (`--release`) | No-mover control (`high` / `low`) |
|---|---|---|---|
| SH base gate priced peak | 130,585,882 B (124.5 MiB) | 130,585,882 B (124.5 MiB) | 40,606,856 B (38.7 MiB) |
| …phase: mover bounce / mover direct | 123,174,188 / 130,585,882 B | same | — |
| In-process ledger peak, bounce half (cold / warm empty / full hit) | 26.6 / 28.5 / 27.4 MB (4.9× / 4.6× / 4.8× under priced) | 27.9 / 29.5 / 28.8 MB (4.7× / 4.4× / 4.5×) | — |
| Process RSS at SH stage start | 19.7 MiB | 26.1 MiB | 13.0 / 14.3 MiB |
| Process RSS peak inside the SH stage (mover bounce half included) | 100.0 MiB | 102.5 MiB | 53.2 / 43.3 MiB |
| Process RSS peak from the SH stage to the end of the direct stages (ids 35, 41, 45, mover direct half) | 182.6 MiB | 178.5 MiB | 119.5 / 100.0 MiB |
| Whole-build peak RSS (`/usr/bin/time -l`) | 862,699,520 B (822.7 MiB) | 807,620,608 B (770.2 MiB) | — |
| Whole-build peak RSS (sampled) | 799.2 MiB | 802.6 MiB | 697.7 / 714.5 MiB |

- **Process RSS, SH stage.** Inside the stage, the whole process never exceeds the priced peak: 100.0 and 102.5 MiB, against 124.5 MiB priced. That RSS includes the 20–26 MiB earlier stages left resident.
- **The mover channel's share.** Against the control, which strips both movers, their waypoints and the two carried dynamic lights (the direct stages bake the same lights), movers add 56.7 MiB (`low`) and 49.3 MiB (`high`) to the SH stage's RSS peak. The gate prices the mover increment at 89,953,982 B (85.8 MiB): the same map priced with and without its movers (40,631,900 B without).
- **Direct-stage window.** The window after the SH stage also holds ids 35, 41 and 45, which the SH-delta gate prices, not this one. There movers add 82.6 MiB (`low`) and 59.0 MiB (`high`) over the control. The gate prices the direct phase's mover terms at 106,217,838 B (101.3 MiB):
  - states ×2: 78,839,096 B
  - direct entries ×2: 27,112,246 B
  - posed solids: 266,496 B

  RSS also keeps pages freed in the SH stage, so these increments overstate what the direct half holds.
- **Whole-build peak.** The whole-build peak (~770–823 MiB) falls in the lightmap bake, outside the SH gate's scope. `--working-set-max-size` is a per-stage ceiling, not a process RSS bound.
- **Pre-change setpiece, for scale.** Priced 112,326,656 B; `/usr/bin/time -l` 816,848,896 B.

### Refusal

`a_budget_under_the_mover_priced_peak_refuses_before_any_bounce_ray` now runs on the setpiece at `--bake-fidelity low` and at `--release`. For each it builds once admitted and reads the priced peak, checking that the movers bake mid poses (2 movers, 21 poses). It then builds at a budget one byte under that peak, which refuses with `SH base working-set gate refused before patch shading`, the same peak and the `mover bounce` phase named, and logs no posed bounce.

## Ledger (AC 31)

- `tile_bytes_and_held_closed_pose_tiles_are_ledgered_and_leave_with_the_level` (GPU) now covers a two-pose channel and a posed one (4- and 3-pose movers, with Base and Across marks). The host row `mover_closed_pose_depth_tiles` equals every stored pose's tile plus one base tile per probe. A frame mid-travel writes every probe's live tile, and the report is gone after unload. The separate `mid_pose_closed_pose_tiles_are_ledgered_and_leave_with_the_level` (task 4) folded into it.
- `every_stored_poses_closed_pose_tile_is_ledgered_on_a_compiled_map_and_leaves_with_it` (`--ignored`, `MOVER_LEDGER_PRLS`), whole-loaded and streamed:

| PRL | Depth records | Probes | Stored tiles (mid poses) | Host row |
|---|---|---|---|---|
| mover-setpiece `--release` | 1,257 | 1,248 | 6,651 (4,745) | 6,319,200 B = (6,651 + 1,248) × 800 B |
| mover-setpiece `low` | 1,257 | 1,248 | 6,651 (4,745) | 6,319,200 B |
| enemy-movers `--release` | 847 | 749 | 2,084 (982) | 2,266,400 B |

Every report reads nothing after unload, in both modes.

## Reproduce

- Run scripts, sampler and logs: scratchpad `t10/` (`run.sh`, `batch.sh`, `scripts/rss_sample.py`, `logs/`, `out/`). The no-mover control map is `t10/maps/mover-setpiece-no-movers.map`, built from `t9/content/dev/maps/`.
- Ledger on compiled maps: `MOVER_LEDGER_PRLS=<setpiece.prl>:<enemy-movers.prl> POSTRETRO_REQUIRE_GPU=1 cargo test -p postretro-renderer -- --ignored every_stored_poses_closed_pose_tile`.
