# lighting-scale--sh-delta-cone-reach-cull — plan of record

mode: compact
status: landed
read at: 6c4946ac4

## Corrections

- Byte comparison found a pre-existing id-45 exception the brief did not name:
  `drop_animated_direct_zero_entries` retains exact-zero records for
  script-mutable descriptor slots. Those slots now use direct cube reach so ids
  45/48 remain byte-identical; immutable animated spots still use cone reach.
- The generated warren has three KVP-animated lights and three script-targeted
  lights. The KVP slots use cone reach. Script membership makes the other three
  slots mutable, so they use direct cube reach.
- Review traced id 47 through the shared direct reach index; its static
  billboard-scatter cache epoch also advances so old cube-reach entries miss.

## Delegated answers

- Conservative cone test — in the bake's f32 coordinate domain, enclose each affinity-cell AABB in a sphere, then reject the cell only when that sphere cannot intersect the spotlight's finite outer cone (angular overlap plus padded falloff-sphere overlap and outward angular tolerance). This is conservative for every baked probe in the AABB; degenerate or non-finite authored/derived data retains the cell.
- Post-cull warren bound — measure after regenerating the fixture with six deterministic animated baked lights and applying script membership. Keep the 16 GiB default unchanged; if ids 27/45 leave the projection above it, stop at the measured Phase-2 owner call.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| 1. Warren compiles within the default 16 GiB working-set budget | Executor manual cold compile of `stress-warren-hallway-inspection.map` using the fixture's documented bake flags | achievable as stated | passed manually — cold build completed in 297.96 s and emitted a 66 MiB PRL |
| 2. Record the first-slice post-cull warren projection | Ignored real-map CLI test runs the production plan with `--sh-delta-working-set-max-size 0` and refuses before base SH | achievable as stated | passed — current-policy peak is 6,545,498,112 bytes at default 1 m spacing |
| 3. Default working-set cap remains exactly 16 GiB | Unit/constant guard over `DEFAULT_MAX_WORKING_SET_BYTES` plus source grep in final evidence | achievable as stated | passed — existing default guard remains green; no cap edit |
| 4. Cone-culled direct CSR equals post-drop unculled CSR | Focused spot fixture bakes the cube-reach oracle, applies `drop_direct_zero_entries`, and compares offsets/lights/payload with the cone-reach result | achievable as stated | passed — `cone_culled_direct_delta_matches_post_drop_cube_reach_bytes` |
| 5. Plan-phase and id-41 bake CSRs cannot diverge | Shared direct-plan builder plus a production assertion at the bake handoff; focused mismatch fixture exercises the check | achievable as stated | passed — production handoff assertion plus mismatch regression |
| 6. Id-41 and id-45 fixtures are byte-identical before/after, cold and warm | Pre-change and post-change SHA-256 for named fixtures, recorded in `research.md`; section hashes accompany whole-file hashes to isolate BC6H | achievable as stated | passed — whole-file plus ids 35/41/45/48 hashes match in cold and warm modes |
| 7. Id-27 stays cube-reach for an animated spot also present in id-45 | Focused end-to-end dual-transport bake compares emitted id-27 CSR with script-mutable id-45 cube reach, then proves immutable id-45 drops outside-cone cells | achievable as stated | passed — `animated_spot_bakes_cube_indirect_and_policy_selected_direct_transport` |
| 8. Direct and delta cache epochs advance and new warm builds are stable | Constant assertions/cache-key tests plus two new-predicate warm builds with recorded SHA-256 | achievable as stated | passed — direct epochs pinned at 4/2/2, indirect remains 1, old-cache rebuild and second warm run both hash-identical |
| 9. Fully outside cells are absent and a fully culled selected light retains its canonical id-41 cell | Focused affinity/id-41 tests compare the retained cell with the unculled drop-policy oracle | achievable as stated | passed — outside/behind cells rejected and canonical output equals cube oracle |
| 10. Grazing and mixed-probe cells are conservative | Focused geometry tests cover tangent/no-valid-probe and one-in/one-out probe arrangements; payload comparison pins the kept mixed cell | achievable as stated | passed — enclosing-sphere overlap keeps the mixed cell; post-drop payload equals cube oracle |
| 11. Directional lights are never cone-culled | Focused affinity/direct-SH regression compares full-grid CSR and id-35/id-41 bytes | achievable as stated | passed — full-grid reach and id-41 byte equality regressions |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Regenerate the warren with deterministic animated coverage, capture pre-change cold/warm identity baselines and the zero-cap projection, add the thinnest direct-only conservative cone predicate, then remeasure the projection to falsify or confirm the 16 GiB premise | integrating executor | — | complete — current-policy default-spacing peak is 6,545,498,112 bytes |
| 2 | Harden transport separation, id-41 canonical retention, plan/bake CSR identity, cache epoch bumps, and focused cone-boundary/directional/id-27 regression coverage | integrating executor | 1 | complete — focused suite green |
| 3 | Run named cold/warm byte-identity fixtures and the admitted warren compile; record hashes, projection, and exact automated results in `research.md` and this plan | integrating executor | 2 | complete — hashes, manual compile, and current-policy projection recorded |
| 4 | Run readiness checks, review/fix/retest loop, final preflight, update durable lighting/build contracts, and land the brief | integrating executor | 3 | complete — second review's 2 must-fix and 6 should-fix findings resolved; full preflight green |

## Final verification

- Review panel traced direct, animated, and cache consumers. Its findings were
  resolved by evaluating cone reach in the bake's `f32` domain, normalizing the
  cone axis with an outward tolerance, retaining non-finite/degenerate cells,
  advancing the static billboard-scatter epoch, and correcting durable notes.
- Focused regressions passed for affinity geometry, direct and animated direct
  SH, billboard scatter, plan/bake CSR mismatch detection, and cache epochs.
- The post-review implementation passed `cargo fmt --all --check`, `cargo
  clippy --target-dir target/preflight-clippy -- -D warnings`, the full
  workspace `cargo test`, and all doc tests. The focused gate passed 13
  affinity tests, 7 animated-direct tests, the cache-epoch test, the animated
  slot test, and the ignored real-warren projection test.
