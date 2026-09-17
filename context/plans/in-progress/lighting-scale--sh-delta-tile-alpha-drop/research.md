# lighting-scale--sh-delta-tile-alpha-drop — research

Ephemeral. Grounding and pinned orderings behind `index.md`. Symbols as of 4f9e5c5; they
drift — the brief states only what survives.

## Alpha audit

Writers of the delta alpha channel (all write constant f16 1.0 for a valid probe, 0 otherwise):

| Site | Section | Note |
|---|---|---|
| `pack_octahedral_irradiance_tile` (`sh_bake.rs`) | shared packer | Untouched: id 34/35 reuse it; `sh_sample.wgsl` reads stored alpha for L1 corner presence (`sample.a`, `stored[corner].a`). |
| `bake_direct_delta_subblock` (`direct_sh_bake.rs`) | 41 | `out.extend_from_slice(&texel.rgba)` |
| `bake_subblock` (`delta_sh_bake.rs`) | 27 | `out[dst..dst + 4].copy_from_slice(&texel.rgba)`; comment already says alpha is unused downstream |
| id-45 sub-block bake (`animated_direct_sh_bake.rs`) | 45 | `out.extend_from_slice(&texel.rgba)` |
| `synthesize_l2_mean_tile` (`delta_sections.rs`) | 27/41/45 | Post-bake compaction re-encodes the L2 mean with `valid_alpha`; not in the handoff's enumeration — must move with the writers. |

Readers: none consume the value. `direct_sh_compose.wgsl`, `sh_compose.wgsl`,
`animated_direct_sh_compose.wgsl` take `.rgb` from `read_delta_texel` and from the shared
kept lattice; output alpha is `prior.a` from the base sample or stored-slot validity.
`reconstruct_delta_probe_tile` decodes halves `i..i+2` and its doc says alpha is unused.
`rgb_payload_is_zero` tests `rgba[..3]`. `DenseDirectView`, `EmittedDeltaSectionRef`,
`emitted_reconstruction_error_by_cell` build `Vec3` from three halves.

## Stride sites (lockstep inventory)

Sites that compute a texel offset or tile/entry length from the per-texel half count. Every
row moves in the one change.

| Site | Kind | Today |
|---|---|---|
| `DELTA_TILE_TEXEL_F16_COUNT`, `DEFAULT_DELTA_PROBE_F16_STRIDE`, `DEFAULT_DELTA_PROBE_BYTES`, `delta_probe_f16_stride` (`level-format/delta_sh_volumes.rs`) | constant / derived | 4 |
| `delta_probe_f16_stride_checked` ×3 (`delta_sh_volumes.rs`, `direct_sh_delta_volumes.rs`, `animated_direct_sh_delta_volumes.rs`) | length validation in `from_bytes` | `× DELTA_TILE_TEXEL_F16_COUNT` |
| Doc comments "RGBA16F" / "× 4" in the three format files | layout doc | literal |
| `bake_direct_delta_subblock`, id-27 `bake_subblock` (`base + texel_index * 4`), id-45 bake | writers | `.rgba` |
| `expected_subblock_f16_len` at the three `bake_or_load_delta_subblocks` call sites | cache length assert | `PROBES_PER_CELL × stride` — follows constant |
| `synthesize_l2_mean_tile` (`delta_sections.rs`) | writer + `tile_texels = probe_stride / COUNT` | literal push of alpha |
| `EmittedDeltaSectionRef::new` / `decode_interior`; `emitted_reconstruction_error_by_cell` (`delta_sections.rs`) | readers | `× DELTA_TILE_TEXEL_F16_COUNT` |
| `DenseDirectView::new` / `decode_valid_entry` (`sh_runtime_envelope_scoring.rs`) | reader | `× DELTA_TILE_TEXEL_F16_COUNT` |
| `DeltaView` / `DenseDeltaView` (`from_indirect`/`from_direct`/`from_anim_direct` via `check_delta`, `probe_f16_stride`, `probe_stride`, `decode_entry_local`, border accumulation, size-sweep estimate) (`sh_analyze.rs`) | coarsening classifier decode + byte sweep (id 27/41/45) | literal `× 4` at `:157,:183,:320,:353,:826` and `tile²×4×2` byte estimate at `:942` — must move to the constant, NOT auto-following |
| `rgb_payload_is_zero` (`delta_drop_policy.rs`) | reader | `chunks_exact(4)` |
| `delta_entry_offsets`, `resolve_delta_f16_offset` (`render-cpu/sh_compose.rs`) | compaction meta | take `delta_probe_f16_stride()` — follow |
| `reconstruct_delta_probe_tile`, `DeltaProbeReconstructionContext` doc (`render-cpu/sh_compose.rs`) | CPU reference | `i = base + t * 4` literal |
| `build_compose_grid_bytes` (`render-cpu/sh_compose.rs`) | uniform `delta_probe_f16_stride` | follows constant |
| `read_delta_texel` ×3 (WGSL) | GPU reader | `texel_index * 4u` literal; two-word RGBA unpack |
| `pipeline.rs` working-set `subblock_f16_len` | gate projection | follows `delta_probe_f16_stride` |
| `sh_runtime_envelope.rs` `mutable_cost` / `payload_bytes` | diagnostics | follow `delta_probe_f16_stride()` |
| `delta_drop_policy.rs` `drop_*_zero_entries` | entry stride | `PROBES_PER_CELL × delta_probe_f16_stride()` — follow |

Tests that hard-code the old stride and will need the new one: `delta_drop_policy.rs` tests
(`STRIDE = PROBES_PER_CELL * 4`, `block` pushes alpha); `sh_runtime_envelope_tests.rs`
(`dense_direct` pushes `[v, v, v]` + alpha); `sh_coarsen.rs` tests (`PROBES_PER_CELL * 4`,
`sub.extend([h, h, h, 0])`); `render-cpu/sh_compose.rs` tests (`stride = tile_texels * 4`,
`[bits, bits, bits, 0]`, `stride = 4u32`); `renderer/render/direct_sh_compose.rs` footprint
test (`delta_subblocks_bytes: 18_432`); `postretro/tests/capture_receiver_controls`
(`4 * 6 * 6 * 4`); `delta_sections.rs` tests (`payload[i + 3] = alpha`); `delta_sh_bake.rs`
test (`flat_map(|texel| texel.rgba)`); `animated_direct_sh_bake.rs` test (`chunks_exact(4)`).

Not on the stride and untouched: `sh_reconstruct.rs` (`kept_mask`, `stored_delta_tiles`),
loader validation (`prl_loader.rs` uses `expected_delta_subblock_f16_count`), the base-atlas
serializers (`direct_sh_bake.rs` atlas blob, `sh_group.rs`, `sh_density.rs`), section 48
(`BILLBOARD_DIRECT_SCATTER_DELTA_RGBA_F16_COUNT`, `F16_PER_SAMPLE`).

## Size arithmetic (6×6 tiles, all-valid L0 cell)

- Per probe tile: 36 texels × 4 halves × 2 B = 288 B → 36 × 3 × 2 = 216 B.
- Per dense CSR entry: pre-change 64 × 288 = 18,432 B; post-change 64 × 216 = 13,824 B.
  The compile-peak-ram gate's per-entry projection and the `direct_sh_compose.rs` footprint
  test both carry 13,824 B.
- Payload reduction is exactly 25% at every level (L1 corners and the L2 mean use the same
  texel). Section headers, masks, levels and CSR tables are unchanged, so the `.prl`
  reduction is 25% of the delta payload, not of the file.

## Word packing at stride 3

Storage binding is `array<u32>`, `unpack2x16float` gives (low, high). Entry bases are
`kept_tiles × stride` and probe bases `rank × stride`; at 108 halves per tile both stay
even. Texel base `texel_index × 3` is odd for odd texels, so R sits in the high half of
word `⌊half/2⌋` and G, B in the next word. Reader loads two words either way; parity
chooses which halves form (R, G, B). The last texel of a tile (index 35, base 105) ends at
half 107 inside the tile — no read past the tile.

## Pinned orderings

| id | scenario | ordering pinned | expected outcome |
|----|----------|-----------------|------------------|
| R1 | A `.prl` written before this change is loaded by the new binary. | Section version check precedes any payload length identity. | Rejected with the section's named "recompile the .prl" error; no length-mismatch or panic path is reached. |
| R2 | The same bake serialized RGBA (old) and RGB (new); both decoded by their own reader. | Identity is per reconstructed tile, compared as f16 bit patterns, after compaction and at each level. | Bit-equal at L0, L1 kept corner, L1 dropped-valid (trilinear of kept corners), L2 mean. The L2 mean is re-encoded from RGB in both cases, so its f16 rounding is unchanged. |
| R3 | One site keeps the old multiplier, or a compose shader swaps the multiplier but keeps the fixed even-offset two-word unpack, while the stride is 3. | Fixture channels distinct per texel and per probe (e.g. R = texel index, G = probe rank, B = entry); the source guard checks both the texel multiplier and the parity-select read against the format constant. | The lagging site reads a neighbour's channel and the identity row fails; a shader that keeps the even-offset unpack misreads every odd texel and fails the guard; a fixture with uniform channels would not detect it. |
| R4 | Odd texel index; first texel of a probe at odd/even kept rank; texel 35 of a tile. | Rust port of the word-packed read vs. half-indexed CPU reference (the shipped WGSL packed read has no headless harness); the shipped WGSL is pinned to that port by the R3 guard, E20 is its on-GPU proof. | Same R, G, B; no read outside the tile's 108 halves. |
| R5 | Warm cache populated by the pre-change binary; then two warm builds on the new binary. | Stage-version bump vs. `decode_subblock` length rejection. | Every pre-change entry is a miss by key (not by length); second warm build is a full hit and byte-identical to the first. |
| R6 | Cone-reach-cull lands before or after this brief. | Each brief's byte-identity baseline is taken at the format version in effect. | No cross-brief byte comparison; stage versions bump once per landing. |
| R7 | adaptive-probe-spacing lands before or after this brief; both edit the shared SH compose grid uniform / octahedral sampler. | Neither brief compares delta bytes against the other's base format; the second to land re-verifies the delta compose reads its per-texel stride from the format constant, not a literal. | No cross-brief byte comparison; the shared compose/sampler code carries one stride source after both land. |

## Rival shapes considered

- Strip alpha at load into the storage buffer: VRAM only, no disk win, one extra CPU copy at
  load — violates the RAM constraint. Rejected.
- Keep four halves and repurpose alpha: no size win. Rejected.
- Sub-f16 RGB encodings and BC6H: see Non-goals in `index.md`.

## Size delta (populated by the executor)

| Map | Section | Payload bytes before | after | `.prl` before | after | Storage buffer before | after |
|---|---|---|---|---|---|---|---|
| campaign-test | 41 | 2,075,328 | 1,556,496 | 177,924,897 | 174,098,246 | 2,075,328 | 1,556,496 |
| campaign-test | 45 | 6,198,624 | 4,648,968 | 177,924,897 | 174,098,246 | 6,198,624 | 4,648,968 |
| campaign-test | 27 | 7,032,672 | 5,274,504 | 177,924,897 | 174,098,246 | 7,032,672 | 5,274,504 |

`campaign-test` emits ids 27, 41, 45, and 48, so it is also the resolved real id-45 map;
no substitute fixture was needed. Each delta payload and its verbatim storage upload fell
by exactly 25%. The whole file fell by 3,826,651 bytes; the three payloads account for
3,826,656 bytes and id 28 build statistics grew by 5 bytes because the recorded timings
changed. Section-internal versions were verified as 5/3/3 before and 6/4/4 after, guarding
against accidentally comparing two builds from the same compiler.

The dev-tools footprint log could not be captured because the pre-existing
`DiagnosticsTab::ALL` array-length mismatch prevents a dev-tools renderer build. Storage
bytes above are nevertheless exact: the loader retains the decoded `Vec<u16>` and the
renderer uploads that slice verbatim (the no-clone source guard pins this path), and all
three payload byte counts are already four-byte aligned.

For the same cold before/after artifacts, sections 34, 35, and 48 were byte-identical:

| Section | bytes | SHA-256 (both artifacts) |
|---|---:|---|
| 34 base indirect SH | 2,917,644 | `a67e07479dfbf0c1c695bd82fe3b1973e2b259dd7f14da1e321a668e6855e98b` |
| 35 base direct SH | 1,364,284 | `17391eb88ec87dc6e761831b61f1b7e7eb927b475c7cf76b5f747fae4ae77ad5` |
| 48 billboard direct scatter | 479,730 | `185423fbb38e52f2f7a844d91dde692ab12bed46851455b664cf6deceab91607` |
