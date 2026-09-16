# lighting-scale--sh-delta-cone-reach-cull — plan of record

mode: compact
status: active
read at: 6c4946ac4

## Corrections

- None. The cited compiler seams are unchanged since the brief's `e36e86b` grounding commit.

## Delegated answers

- Conservative cone test — enclose each affinity-cell AABB in a sphere, then reject the cell only when that sphere cannot intersect the spotlight's finite outer cone (angular overlap plus padded falloff-sphere overlap). This is conservative for every point in the AABB, therefore it cannot exclude an in-cone probe; degenerate or non-finite authored cone data retains the cell.
- Post-cull warren bound — measure after regenerating the fixture with six deterministic animated baked lights. Keep the 16 GiB default unchanged; if ids 27/45 leave the projection above it, stop at the measured Phase-2 owner call.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| 1. Warren compiles within the default 16 GiB working-set budget | Ignored real-map CLI gate plus an executor cold compile of `stress-warren-hallway-inspection.map` using the fixture's documented bake flags | achievable as stated | pending |
| 2. Record the first-slice post-cull warren projection | `--sh-delta-working-set-max-size 0` diagnostic captured in `research.md` before the admitted run | achievable as stated | passed — 5,968,926,720 bytes at default 1 m spacing |
| 3. Default working-set cap remains exactly 16 GiB | Unit/constant guard over `DEFAULT_MAX_WORKING_SET_BYTES` plus source grep in final evidence | achievable as stated | passed — existing default guard remains green; no cap edit |
| 4. Cone-culled direct CSR equals post-drop unculled CSR | Focused spot fixture bakes the cube-reach oracle, applies `drop_direct_zero_entries`, and compares offsets/lights/payload with the cone-reach result | achievable as stated | passed — `cone_culled_direct_delta_matches_post_drop_cube_reach_bytes` |
| 5. Plan-phase and id-41 bake CSRs cannot diverge | Shared direct-plan builder plus a production assertion at the bake handoff; focused mismatch fixture exercises the check | achievable as stated | passed — production handoff assertion plus mismatch regression |
| 6. Id-41 and id-45 fixtures are byte-identical before/after, cold and warm | Pre-change and post-change SHA-256 for named fixtures, recorded in `research.md`; section hashes accompany whole-file hashes to isolate BC6H | achievable as stated | pending |
| 7. Id-27 stays cube-reach for an animated spot also present in id-45 | Focused dual-transport fixture compares pre/post id-27 CSR bytes and proves id-45 drops outside-cone cells | achievable as stated | passed — real `decompose_affinity` wrapper remains full-cube while direct drops cells; warren id-27 coarse bytes unchanged |
| 8. Direct and delta cache epochs advance and new warm builds are stable | Constant assertions/cache-key tests plus two new-predicate warm builds with recorded SHA-256 | achievable as stated | partial — direct epochs pinned at 4/2/2 and indirect remains 1; warm identity run pending |
| 9. Fully outside cells are absent and a fully culled selected light retains its canonical id-41 cell | Focused affinity/id-41 tests compare the retained cell with the unculled drop-policy oracle | achievable as stated | passed — outside/behind cells rejected and canonical output equals cube oracle |
| 10. Grazing and mixed-probe cells are conservative | Focused geometry tests cover tangent/no-valid-probe and one-in/one-out probe arrangements; payload comparison pins the kept mixed cell | achievable as stated | passed — enclosing-sphere overlap keeps the mixed cell; post-drop payload equals cube oracle |
| 11. Directional lights are never cone-culled | Focused affinity/direct-SH regression compares full-grid CSR and id-35/id-41 bytes | achievable as stated | passed — full-grid reach and id-41 byte equality regressions |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Regenerate the warren with deterministic animated coverage, capture pre-change cold/warm identity baselines and the zero-cap projection, add the thinnest direct-only conservative cone predicate, then remeasure the projection to falsify or confirm the 16 GiB premise | integrating executor | — | complete — default-spacing peak is 5,968,926,720 bytes |
| 2 | Harden transport separation, id-41 canonical retention, plan/bake CSR identity, cache epoch bumps, and focused cone-boundary/directional/id-27 regression coverage | integrating executor | 1 | complete — focused suite green |
| 3 | Run named cold/warm byte-identity fixtures and the admitted warren compile; record hashes, projection, and exact automated results in `research.md` and this plan | integrating executor | 2 | in progress |
| 4 | Run readiness checks, review/fix/retest loop, final preflight, update durable lighting/build contracts, and land the brief | integrating executor | 3 | pending |
