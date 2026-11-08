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

| AC | Proof | Status | Result |
|---|---|---|---|
| A1 no tiles: static-door st3 ≥ 0.4 | capture test, static-door variant, stripped PRL | achievable as stated | pass: st3 0.495 |
| A2 tiles: st3 ≤ 0.05, st2 ≤ 0.02 | capture test, static-door variant | achievable as stated | pass: st3 0.033, st2 0.009 |
| A3 sprite + kinematic brush at st3 floor ≤ fraction of no-tile | kinematic: capture test on `mover-closet-kinematic-receiver`; sprite: compiler test through the billboard sampler's CPU mirror (see Corrections); fraction from the pilot (below) | achievable as stated | pass: slab 0.006, sprite 0.033–0.049 (ceiling 0.10) |
| A4 reflection capture parity + id 59 re-keys stage | compiler test: CPU mirror samples tiles through the shared decode and darkens behind the door; stage key changes with id 59 bytes | achievable as stated | pass: mirror floor 0.222 → 0.012 at 0.05 m; key misses on id 59 |
| A5 open doorway within 0.02, ≤ 0.1% px move > 2 | capture test, no-door variant | achievable as stated | pass: means 14.997 → 14.994, 29.783 → 29.789; 432 / 112 px of 518,400 moved |
| A6 hallway Q2 1080p: no px darkens > 2 | `#[ignore]` capture test, bakes stress-warren-hallway-inspection | achievable as stated | pass: 0 darker at a/b/c (11 brighter at a) |
| A7 mover-door closet means ≥ today − 0.02 | capture test, `mover-closet` | achievable as stated | pass: 6.825 → 6.822, 6.602 → 6.605 |
| A8 no id 59 ≡ zero-tile id 59 | capture test, byte compare | achievable as stated | pass: byte-identical at st2, st3 |
| A9 untiled beside tiled: bit-for-bit today | GPU WGSL harness over `sh_corner_depth_visibility` vs today's body | achievable as stated | pass (GPU, Metal) |
| A10 floor 0.03 on every depth-aware path | GPU WGSL harness (tile mean 0 → exactly 0.03) + shader test that per-corner, L1, L2 and pair paths all weigh depth through it | achievable as stated | pass (GPU, Metal) |
| A11 every texel writer keeps R = id-34 `E[d]` | renderer unit tests over whole-load pack, promote, evict, session clear then promote | achievable as stated | pass (GPU readback) |
| A12 raw-bit decode: tiles 0 and 32,767, untiled +0 | GPU WGSL harness | achievable as stated | pass (GPU, Metal) |
| A13 rewrite: next frame, clear restores bit for bit, same-drain both orders, survives evict/reinstall (O5–O7) | renderer tests over the reference table and streamed upload packing | achievable as stated | pass |
| A14 both writers carry the reference (O3) | renderer unit tests | achievable as stated | pass |
| A15 pool growth keeps the tile (O4) | renderer streaming growth test | achievable as stated | pass (binding-18 view in the rebuilt group rests on layout validation) |
| A16 rewrite absent from next load (O10) | renderer per-load state test | achievable as stated | pass |
| A17 bake determinism, warm ≡ cold, stale entry misses, closet blocked-neighbour probes all tiled (O1, O2) | compiler tests (closet compiles in seconds) | achievable as stated | pass: closet 35/35 blocked-neighbour probes tiled (40 tiles) |
| A17b convex room: tiles move no px > 2 against no id 59, along the wall-hugging probes; summary reports count | capture test (restated by owner, 2026-11-07; see Owner rulings) | achievable as restated | pass: 9 tiles, 0 px moved at both views |
| A18 cap ties (O9) | compiler unit test, lowered cap | achievable as stated | pass |
| A19 set sign bit refuses by name | loader tests, whole-load and streamed metadata | achievable as stated | pass; the compiler now refuses the same at pack |
| A20 malformed id 59 refuses by name | `probe_depth_tiles_tests` + loader test naming the section | achievable as stated | pass |
| A21 forward 16/15; stage limits; offscreen full-init with GPU; FXC | existing layout tests updated; offscreen renderer test; `dx12_fxc_test` (compiles on Windows only; Mac skips) | achievable as stated | pass on Metal; DX12 FXC compile outstanding (Windows only) |
| A22 mesh superset keeps 15 fragment-visible; kinematic/skinned budgets from superset | renderer layout tests | achievable as stated | pass |
| A23 tile bytes in SH ledger, zero after unload | renderer allocation test | achievable as stated | pass |
| A24 build summary tile bytes = ledger | compiler summary test + renderer ledger test, both on `probe_depth_tile_texture_bytes` | achievable as stated | pass |
| A25 hallway tile memory ≤ 6.4 MiB | `#[ignore]` test, same hallway bake | achievable as stated | pass: 13,451 tiles, 5.18 MiB |
| M1 cross-build byte-identical to 2968de0ad | two-worktree capture `cmp` | manual | outstanding |
| M2 forward cost ceiling (Mac 4K MST, Windows 1080p timing) | owner/agent run per research §Mac and spike Q2 | manual | outstanding: Mac 4K MST and Windows 1660 runs |
| M3 owner in-engine | owner | manual | outstanding |
| M4 fog bleed observation | owner | manual | outstanding |

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
| 6 | Capture acceptance tests A1–A8, A25 | integrating executor | 4 | done; hallway bake + captures 32 min |
| 7 | Preflight, review panel, fixes | integrating executor | 5, 6 | |

## Review loop
- One panel: 3 slices + seam pass, 9 reviewers (6 depth on Opus, 3 hygiene on Sonnet). Fixed: the `load-prl` gate lost on an id-34 import (standalone `postretro-visibility` / `postretro-lighting` builds broke); tile moments overflowing f16 to +inf past ~256 m, which could turn the weight NaN under the floor (now saturated at the f16 maximum); the compiler now refuses a signed id-34 moment at pack, as the loader does; the convex-room test now asserts no pixel moves either way; cap-consistent summary; Chebyshev constants pinned to `sh_sample.wgsl`; zero-direction guard; `encoded_len`; comment drift from binding 15 going vertex-only; plan and spike pointers out of durable comments.
- Left, with reason: per-texel variance is mostly f16 rounding noise (every reader sees the same bits; noted for counsel question 8); the bake origin and the readers' origin agree only by convention (relocation's concern, below); whole-load rewrites issue one copy per probe and texture writes carry no stale-write guard (no caller yet; the seam documents its frame window). The `with_depth_tiles` signed-moment refusal has no direct test; the pack refusal upstream of it does.

## Follow-ups
- Probe relocation (roadmap): when bake positions move off the lattice, the runtime and the reflection mirror must take the tile direction from the relocated position too; today both compute `grid_origin + idx · cell_size`, which equals the bake position only because nothing relocates. Pin it with an assertion when relocation lands.
- The closet pool reads lit (~17 luma) in any closet variant that carries a `kinematic_mover`, and dark in the static-door map, with or without tiles. Unexplained, unrelated to tiles.
