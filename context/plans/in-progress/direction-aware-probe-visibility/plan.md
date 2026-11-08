# direction-aware-probe-visibility — plan of record

mode: compact
status: active
read at: 5932287f4

Source since the brief's read-at (2968de0ad) changed only navmesh, mover and AI code; no SH bake, renderer, loader or format file the Decisions cite moved, so the grounded Decision reads stand.

## Corrections
- Path names no tile-texture shape → `level-format::probe_depth_tiles::ProbeDepthTileTextureLayout`: one `Rg16Float` 2D texture, a power of two tiles per row (smallest row ≥ √count), rows only as tall as needed. Build summary and allocation ledger both price from it, so they agree by construction. Hallway's 13,451 tiles → 1280×1060 texels, 5.18 MiB.
- Path's "tile-row constant" has no uniform slot → `ShGridInfo` spare words carry it: `_pad0` (28) the tiles-per-row shift, `_pad1` (44) and `_pad3` (92) the texture's reciprocal width and height as f32, so the lookup needs no texture-size query. The 96-byte ABI is unchanged.
- Path cites no tile binding number → group 3 binding 18 (`BIND_SH_DEPTH_TILES`), `sh_depth_tiles: texture_2d<f32>`.

- A3 says a sprite is read at station 3, but capture draws no billboards (emitters spawn, no particle ever draws) → the sprite reads through the CPU mirror of the SH sampler in a billboard mode matching `billboard.wgsl` `sample_sh_indirect` (no normal offset, no backface rejection), whose tile decode A4's parity tests pin; the kinematic brush is captured. Same meaning, different instrument.
- The closet pool reads lit (~17 luma) in any closet variant that carries a `kinematic_mover`, and dark in the static-door map, with or without tiles. Not caused by tiles; unrelated to this brief. Recorded for the owner, not chased.

## Delegated answers
- Hallway Q2 view poses for the Windows ceiling run — reuse the spike's capture scenes (`kit/mk_scenes.py` on `spike/per-direction-depth`), so the ceiling compares against the run that set it.

## Owner rulings
- 2026-11-07, convex room row. The bake gives the convex-room fixture 9 tiles (an off-phase room 16): every one on a probe millimetres from a wall or ceiling, because the lattice origin sits on the geometry bounds, changing visibility 0.105–0.29 toward a neighbour along the surface. These are correct per-direction depth, not false flags; selection stays unchanged. The row is restated as a no-visible-harm capture check, and the cause is recorded against probe relocation in the roadmap.

## AC-to-proof

Capture tests live in `crates/postretro/tests/capture_probe_depth_tiles.rs` (feature `capture`, GPU-gated; `POSTRETRO_REQUIRE_GPU` makes a missing adapter fail). "Today" is the same tiled PRL with id 59 stripped.

| AC | Proof | Status |
|---|---|---|
| A1 no tiles: static-door st3 ≥ 0.4 | capture test, static-door variant, stripped PRL | achievable as stated |
| A2 tiles: st3 ≤ 0.05, st2 ≤ 0.02 | capture test, static-door variant | achievable as stated |
| A3 sprite + kinematic brush at st3 floor ≤ fraction of no-tile | kinematic: capture test on `mover-closet-kinematic-receiver`; sprite: compiler test through the billboard sampler's CPU mirror (see Corrections); fraction from the pilot (below) | achievable as stated |
| A4 reflection capture parity + id 59 re-keys stage | compiler test: CPU mirror samples tiles through the shared decode and darkens behind the door; stage key changes with id 59 bytes | achievable as stated |
| A5 open doorway within 0.02, ≤ 0.1% px move > 2 | capture test, no-door variant | achievable as stated |
| A6 hallway Q2 1080p: no px darkens > 2 | `#[ignore]` capture test, bakes stress-warren-hallway-inspection | achievable as stated |
| A7 mover-door closet means ≥ today − 0.02 | capture test, `mover-closet` | achievable as stated |
| A8 no id 59 ≡ zero-tile id 59 | capture test, byte compare | achievable as stated |
| A9 untiled beside tiled: bit-for-bit today | GPU WGSL harness over `sh_corner_depth_visibility` vs today's body | achievable as stated |
| A10 floor 0.03 on every depth-aware path | GPU WGSL harness (tile mean 0 → exactly 0.03) + shader test that per-corner, L1, L2 and pair paths all weigh depth through it | achievable as stated |
| A11 every texel writer keeps R = id-34 `E[d]` | renderer unit tests over whole-load pack, promote, evict, session clear then promote | achievable as stated |
| A12 raw-bit decode: tiles 0 and 32,767, untiled +0 | GPU WGSL harness | achievable as stated |
| A13 rewrite: next frame, clear restores bit for bit, same-drain both orders, survives evict/reinstall (O5–O7) | renderer tests over the reference table and streamed upload packing | achievable as stated |
| A14 both writers carry the reference (O3) | renderer unit tests | achievable as stated |
| A15 pool growth keeps the tile (O4) | renderer streaming growth test | achievable as stated |
| A16 rewrite absent from next load (O10) | renderer per-load state test | achievable as stated |
| A17 bake determinism, warm ≡ cold, stale entry misses, closet blocked-neighbour probes all tiled (O1, O2) | compiler tests (closet compiles in seconds) | achievable as stated |
| A17b convex room: tiles move no px > 2 against no id 59, along the wall-hugging probes; summary reports count | capture test (restated by owner, 2026-11-07; see Owner rulings) | achievable as restated |
| A18 cap ties (O9) | compiler unit test, lowered cap | achievable as stated |
| A19 set sign bit refuses by name | loader tests, whole-load and streamed metadata | achievable as stated |
| A20 malformed id 59 refuses by name | `probe_depth_tiles_tests` + loader test naming the section | achievable as stated |
| A21 forward 16/15; stage limits; offscreen full-init with GPU; FXC | existing layout tests updated; offscreen renderer test; `dx12_fxc_test` (compiles on Windows only; Mac skips) | achievable as stated |
| A22 mesh superset keeps 15 fragment-visible; kinematic/skinned budgets from superset | renderer layout tests | achievable as stated |
| A23 tile bytes in SH ledger, zero after unload | renderer allocation test | achievable as stated |
| A24 build summary tile bytes = ledger | compiler summary test + renderer ledger test, both on `probe_depth_tile_texture_bytes` | achievable as stated |
| A25 hallway tile memory ≤ 6.4 MiB | `#[ignore]` test, same hallway bake | achievable as stated |
| M1 cross-build byte-identical to 2968de0ad | two-worktree capture `cmp` | manual |
| M2 forward cost ceiling (Mac 4K MST, Windows 1080p timing) | owner/agent run per research §Mac and spike Q2 | manual |
| M3 owner in-engine | owner | manual |
| M4 fog bleed observation | owner | manual |

A3 fraction: **0.10**, for both receivers. First-slice pilot (Mac, Radeon Pro 5300M): the kinematic slab at station 3 keeps 0.006 of its no-tile brightness (3.434 → 0.020 luma over 7,644 px); a sprite on the closet floor at the door keeps 0.033–0.049 of its no-tile indirect (three points, billboard sampler mirror). 0.10 is the worst measured share with about 2× margin.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Id 59 format, reference encoding, texture layout, CPU tile sampler (`level-format::probe_depth_tiles`) | integrating executor | — | done |
| 2 | Bake (done, 33 + 8 compiler tests; convex-room unit test now held `#[ignore]` pending its capture restatement): bin gather hits into tiles, select by 26-neighbour change, cap and rank, ride the `sh_base_volume` entry (stage version 3), pack id 59, build summary tile bytes; closet static-door, no-door and convex-room fixtures | worker (compiler) | 1 | |
| 3a | Runtime: load and validate id 59 and id-34 sign bits; reference table; tile texture and binding 18 in every SH group incl. growth; direct atlas drops fragment visibility in the shared layout; WGSL decode and lookup; whole-load packer; ledger (A9, A10, A12, A19–A23, A8 CPU side) | worker (renderer + loader) | 1 | done: render-cpu 792, loader 347, renderer 1255 passed (GPU required) |
| 3b | Runtime: streamed writers pack G from the table; runtime rewrite seam (whole-load keeps id-34 `E[d²]` for clear); O3–O7, O10, A11, A13–A16 | worker (renderer) | 3a | done: renderer 1263 passed (GPU required) |
| 4 | First slice: bake closet, capture st2/st3 with and without tiles, pilot the A3 fraction | integrating executor | 2, 3 | done: st3 0.495 → 0.033, st2 0.149 → 0.009 |
| 5 | Reflection capture parity: CPU mirror decode + stage key | worker (compiler) | 2 | done in task 2's slice |
| 6 | Capture acceptance tests A1–A8, A25 | integrating executor | 4 | |
| 7 | Preflight, review panel, fixes | integrating executor | 5, 6 | |
