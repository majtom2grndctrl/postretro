# lighting-scale--sh-delta-tile-alpha-drop — plan of record

mode: compact
status: test-ready
read at: 29e8611c6

## Corrections

- None. No cited crate changed between the brief's `read at` commit and current `main`; the live stride, writer, decoder, shader, footprint, and version sites still match the brief.

## Delegated answers

- Real id-45 size map — start with `campaign-test`; if its compiled section inventory lacks id 45, use the smallest existing animated-light fixture that emits id 45 and record the substitution in `research.md`.
- Packed WGSL reader — use branchless parity selection over two adjacent `u32` words; it directly mirrors the half-index derivation and avoids divergent control flow.
- Cone-reach landing order — cone-reach is already on `main`; bump each delta section and stage version once from the landed values and take all byte-identity baselines from this post-cull format. Adaptive probe spacing has not landed, so leave base ids 34/35 untouched and keep the shared stride source explicit for its later rebase.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| A1 Three-half round trip, old length rejection, and stale-version recompile errors for ids 27/41/45 | Focused `postretro-level-format` section tests | achievable as stated | pass: 47 delta tests plus 3 old-RGBA-length tests |
| A2 Id-41 composed-atlas f16 identity at L0, L1 kept, L1 dropped-valid, and L2 | New delta RGB identity integration test over compiler emission and CPU reconstruction | achievable as stated | pass: `rgb_payload_reconstructs_bit_identically_for_ids_27_41_45_at_all_levels` |
| A3 Id-45 and id-27 composed-atlas f16 identity at all coarsening levels | Same identity integration test fixtures for animated-direct and indirect sections | achievable as stated | pass: same three-section identity test |
| A4 Stride lockstep through format, compaction, emitted view, envelope view, CPU decoder, entry drop, and all compose shaders | Distinct-channel fixture plus shader source guard | achievable as stated | pass: 79 compiler delta tests, render-cpu reconstruction tests, three-shader guard |
| A5 Odd-half alignment through packed reader at odd texels, kept ranks, and tile tail | Rust packed-word reader parity test plus shader source guard | achievable as stated | pass: `word_packed_rgb_reader_matches_half_indexed_reference_at_odd_offsets` plus shader guard |
| A6 Entry dropping and coarsening decisions unchanged for ids 27/41/45 | Before/after-layout decision fixtures retaining identical entry sets, levels, and validity masks | achievable as stated | pass: legacy/new zero-decision parity plus output-identity and compiler drop/coarsen suites |
| A7 Working-set and raw-cap summaries use 13,824 B while defaults stay unchanged | Compiler pipeline projection/cap tests and renderer compose-footprint test | achievable as stated | partial: compiler delta suite passes; the repaired dev-tools regression test awaits a machine with enough disk for its cold feature build, then the manual footprint log |
| A8 Cache epochs invalidate old keys and new warm builds are byte-identical | Delta cache-key tests, stage-version pin, and cross-bake warm determinism tests | achievable as stated | pass: previous-version miss/current-version repeated hit, epoch pin, and cross-bake locality/determinism |
| A9 No new runtime RAM copy | Source gate over loader decode and renderer verbatim upload paths | achievable as stated | pass: compose builders now own metadata only; `delta_loader_and_upload_paths_add_no_payload_clone` covers loader → render-cpu → all three renderer staging paths |
| A10 Adaptive landing-order guard | Shader source guards derive multiplier from the format-fed grid stride; no base/delta cross-format byte baseline | achievable as stated | pass: `all_delta_compose_shaders_derive_rgb_stride_and_select_odd_half_parity` |
| M1 Real-map disk/payload/storage size row | `campaign-test` plus resolved id-45 map; compiler summaries, dev-tools footprint log, and file sizes recorded in `research.md` | manual-measurement | partial: exact cold artifact and verbatim-upload sizes recorded; dev-tools log remains to be captured |
| M2 E20 promoted animated-light capture is pixel-identical before/after | Owner, same-adapter before/after E20 capture | manual-visual | pending |
| R1 Base atlases ids 34/35 byte-identical | Existing base-section bytes captured on the same focused bake fixtures before and after | achievable as stated | pass: campaign-test cold artifacts match byte-for-byte; hashes recorded in `research.md` |
| R2 Billboard scatter section 48 byte-identical | Existing section-48 bytes captured on the same focused animated fixture before and after | achievable as stated | pass: campaign-test cold artifacts match byte-for-byte; hash recorded in `research.md` |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Prove the highest-risk id-41 slice end to end: RGB wire stride/version, direct writer, compaction/reconstruction, packed CPU reader, direct compose shader, and identity/odd-half tests | integrating executor | — | done — focused format/render-cpu/renderer tests pass |
| 2 | Extend the RGB layout to ids 27/45 and every shared writer, classifier, envelope, entry-drop, compose, and shader consumer; bump all remaining section/stage versions | integrating executor | 1 | done — compiler delta suite passes 79 tests |
| 3 | Close the cross-cutting acceptance matrix: lockstep/source/RAM guards, cache proofs, decision-equivalence fixtures, footprint/gate updates, and untouched base/scatter regressions | integrating executor | 2 | done — automated matrix and cold-artifact regressions pass; the dev-tools regression was repaired and its manual log remains |
| 4 | Run focused verification, review/fix loop, final preflight, and populate every automated result; prepare and execute the real-map/manual runbook where locally possible | integrating executor | 3 | done — review/fix and final preflight pass; external gates below remain |

## Review and verification

- Review panel: request changes, five deduplicated findings; all five acted on. Fixed mixed
  RGBA-base/RGB-delta analyzer byte accounting, the final-RGB-triple bound, the intermediate
  runtime payload clone, and two stale context comments.
- Focused post-fix gate: all four touched crates check; new analyzer tests match and pass;
  render-cpu SH compose 18/18; no-clone and empty-direct renderer tests pass; level-format
  delta suite 50/50.
- Final preflight: `cargo fmt --check`, workspace
  `cargo clippy --target-dir target/preflight-clippy -- -D warnings`, and `cargo test` pass.

## External runbook

Two manual rows block landing. Do not move this brief to `done/` until both are recorded.

1. **Dev-tools storage log (M1/A7).** The stale `DiagnosticsTab::ALL` regression assertion
   was repaired. On a machine with enough disk for the dev-tools build, launch the pre-change
   and branch `campaign-test.prl` artifacts with `--features dev-tools` and capture
   the once-per-load compose footprint lines for ids 27, 41, and 45. Confirm the logged
   delta-subblock bytes match the exact artifact/upload rows in `research.md`:
   7,032,672 → 5,274,504; 2,075,328 → 1,556,496; and
   6,198,624 → 4,648,968.
2. **Same-adapter E20 capture (M2).** Use commit `29e8611c6` as the before binary and this
   branch as the after binary, each with its correspondingly baked `spawner-test.prl`. Build
   both with `--features capture`. For both captures use the camera constants from
   `spawner_capture_forced_alarm_reds_dynamic_receivers_and_keeps_baked_rest`, force
   `alarm_light` active with radiance `[4.0, 0.0, 0.0]`, and force promotion weight `1.0`.
   Run both binaries on the same adapter and compare decoded RGBA pixels (PNG byte equality is
   also acceptable on one adapter). Expected: pixel-identical before/after. Record adapter,
   commands, and hashes in `research.md`.
