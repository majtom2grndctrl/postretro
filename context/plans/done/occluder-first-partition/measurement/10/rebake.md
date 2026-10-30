# occluder-first-partition: held re-bake on the final compiler

Recorded 2026-10-29 on Windows 11 at HEAD `35a587db8` (`handoff-windows.md` §"After B and C" step 3). One bake at a time, in this worktree, with the documented debug compile path: `RUST_LOG=info cargo run -q -p postretro-level-compiler -- content/dev/maps/<m>.map -o content/dev/maps/<m>.prl --no-tui -v` (the `--no-tui` only makes the log line-oriented for capture; no `--release`, default warm cache). `prl-build` and `scripts-build` were pre-built in debug first, so no bake started a cargo build of its own. The outputs are the gitignored `content/dev/maps/<m>.prl`; `git status --short` stayed clean apart from `measurement/`. Logs (progress lines stripped): `round2/rebake/<map>.log`.

All ten maps compiled with exit 0. No log contains a leak, unsealed-seam or boundary-leak line.

| Map | Exit | Wall | Runtime cells (interior / solid / exterior) | Runtime portals | Door recognition | Seal lines per mover | Warnings (distinct kinds) |
|---|---|---|---|---|---|---|---|
| campaign-test | 0 | 188 s | 840 (136 / 463 / 241) | 306 | 2 of 3 (movers 0, 1) | m0 5, m1 5 | 8 kinds: 43 open edges (watertightness, diagnostic), light_spot missing `_cone` / `_cone2` defaults, 2 coplanar-texture mismatches, 1 oversize SH cluster (singleton, over the 64 limit), warm-SH note |
| kinematic-platform | 0 | 102 s | 66 (10 / 26 / 30) | 13 | 0 of 6 | none | 3: one coplanar-texture mismatch, one specular-mask owner note, warm-SH note |
| movement-feel | 0 | 190 s | 484 (65 / 401 / 18) | 175 | 0 of 0 | none | 5 kinds, 321 lines: 147 open edges (watertightness, diagnostic), about 300 specular-mask ownership notes, 4 lights missing `_color`, warm-SH note, cache over budget (2.25 GiB spared set against 2.00 GiB) |
| occlusion-test | 0 | 23 s | 510 (119 / 255 / 136) | 270 | 0 of 1 | none | 4 kinds: coplanar-texture mismatches (brushes 112 / 117 / 121 and 9 / 14), warm-SH note |
| combat-demo | 0 | 6 s | 35 (1 / 16 / 18) | 0 | 0 of 0 | none | 7 kinds: inert trigger volume, 3 embedded emitters (one light_spot 49.8% inside geometry), specular-mask ownership notes |
| ghost-links | 0 | 3 s | 156 (24 / 71 / 61) | 23 | 6 of 7 (movers 0-5) | m0 2, m1 2, m2 2, m3 2, m4 3, m5 2 | 10 kinds: 8 open edges (watertightness, diagnostic, around brushes 0, 1, 50-53), inert trigger volumes `swap_pad`, `split_pad`, `echo_plate` |
| reflection-lake-test (fluid) | 0 | 115 s | 340 (183 / 114 / 43) | 496 | 0 of 0 | none | 3: warm-SH note, 1 oversize SH cluster (66 primitives against 64), cache over budget (2.59 GiB) |
| reflection-probes-test (fluid) | 0 | 82 s | 168 (20 / 114 / 34) | 27 | 0 of 0 | none | 5 kinds: 10 open edges (watertightness, diagnostic, brushes 19, 40), warm-SH note |
| vault-test (fluid) | 0 | 22 s | 58 (8 / 32 / 18) | 10 | 0 of 2 | none | 2: warm-SH note, cache over budget (3.17 GiB) |
| e24-environment-test (fluid) | 0 | 81 s | 264 (63 / 140 / 61) | 99 | 0 of 0 | none | 2: warm-SH note, cache over budget (3.59 GiB) |

Notes
- `ghost-links` has no `fluid_volume` in its map text; it was in the list as named by the coordinator. The other four fluid maps (`reflection-lake-test`, `reflection-probes-test`, `vault-test`, `e24-environment-test`) do carry `fluid_volume`, and each compiled and wrote a PRL.
- Watertightness reports are diagnostic only ("the build still succeeds"); they are the existing open edges around brush joints on `campaign-test` (43), `movement-feel` (147), `ghost-links` (8) and `reflection-probes-test` (10). There is no sealing or exterior-leak line, which is the compiler's own leak report. I have no earlier bake of these maps on this machine to say whether the open-edge counts moved.
- Door recognition: the movers that did not seal were not recognised as doorways by the compiler (`kinematic-platform` 6 movers, `vault-test` 2, `occlusion-test` 1; I did not check whether any of them is meant to be a door); `ghost-links` seals 6 of 7. There is no earlier record on this machine to compare, except `campaign-test` (2 of 3, m0 5 / m1 5) and `occlusion-test` (0 of 1), which match 9l.
- The cache-budget warnings (spared set 2.25 to 3.59 GiB against the 2.00 GiB budget) mean the default warm cache is over its configured size; the cache keeps the spared set, so no entry was lost.
- Wall times are debug builds on a warm cache; they are not comparable to the release bakes in the README.
