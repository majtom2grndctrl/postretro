# Anti-Penumbra PVS Tightening

> **Status:** draft
> **Depends on:** the current PVS generator (`postretro-level-compiler/src/visibility/portal_vis.rs`). `--pvs` mode must be in use at compile time for the runtime to benefit.
> **Related:** `context/lib/build_pipeline.md` §PRL Compilation · `context/lib/rendering_pipeline.md` §visibility · `context/plans/done/portal-bsp-vis/task-04-portal-vis.md` · `context/plans/done/exterior-leaf-cull/index.md`

---

## Context

The compile-time PVS generator at `postretro-level-compiler/src/visibility/portal_vis.rs:30` (`compute_pvs`) is a pure topological BFS through the portal graph. `leaf_portals` at `portal_vis.rs:13` builds adjacency; the per-leaf BFS at lines 41–62 marks every non-solid leaf reachable via any portal chain as visible. The portal polygon geometry (`Portal.polygon: Vec<DVec3>` at `portals.rs:30`) is carried by the graph but not consulted.

This is the most conservative PVS possible: any leaf in the same connected portal component is marked visible from any other. A map-sized corridor with a dozen side rooms marks every side room visible from every other side room, even when no sightline threads the portals between them. Every leaf in the PVS is a leaf whose face range feeds the per-frame draw submission path in `determine_prl_visibility` (`postretro/src/visibility.rs:434`, the PVS branch at line 617+), whose AABB-frustum cull runs on the decompressed PVS set per frame.

Classical portal vis (Teller 1992, §4; Quake 3 `vis.c`) tightens this by applying a geometric sightline test through the portal chain. The tightening that matters in practice is the **anti-penumbra** construction: a line from leaf A through portal P1, then P2, then P3 into leaf B exists only if that line lies inside the intersection of the anti-penumbra wedges of every portal pair on the chain. Leaves still reachable topologically but not reachable under the wedge-intersection test are provably never visible and drop out of the PVS.

Anti-penumbra is strictly a tightening. Every leaf removed from a PVS row is a leaf that provably cannot be seen from that source leaf. No runtime false negatives — geometry does not vanish, draws just get skipped that the BFS-PVS would have submitted anyway.

**Methodology note.** Like render-perf, this is a static-analysis finding, not a profiling result. The BFS PVS produces PVS rows that grow with connected-component size rather than actual visibility, and the runtime PVS branch at `visibility.rs:617+` iterates every leaf in that row per frame. A tighter PVS is a direct multiplier on post-cull leaf count, face iteration, and the downstream draw stream. The exact win depends on map topology and will be measured in acceptance.

Runtime portal flood (`postretro/src/portal_vis.rs`, default non-`--pvs` mode) already does geometric sightline tightening per frame via `clip_polygon_to_frustum` and narrowing frustums. The anti-penumbra baker is the compile-time analog for the `--pvs` branch — closely related math, distinct routine. Cross-reference; do not share code blindly. Runtime frustum planes are viewer-dependent; anti-penumbra wedge planes are view-independent and constructed from pairs of static portal polygons.

---

## Goal

Replace the topological BFS in `compute_pvs` with an anti-penumbra-tightened portal flood. Each per-leaf PVS bitset popcount drops on every test map. Runtime visual output is bit-identical; only draw-stream volume changes. `--pvs` compiles may go from seconds to tens of seconds on small maps; a `--pvs-fast` opt-out retains the BFS behavior for iteration-loop builds.

---

## Approach

Two tasks. Task A is the wedge construction and the flood rewrite, file-scoped to the visibility module. Task B is CLI and logging: a `--pvs-fast` flag that selects the old BFS, plus per-leaf PVS popcount reporting for before/after comparison.

```
A (visibility/portal_vis.rs + visibility/anti_penumbra.rs) ─── merge
                                                            \
B (main.rs + visibility/mod.rs log_stats)  ─────────────────── merge
```

---

### Task A — Anti-penumbra wedge construction and portal flood

**Crate:** `postretro-level-compiler` · **Files:** `src/visibility/portal_vis.rs`, new `src/visibility/anti_penumbra.rs`

**Construction (sketch).** Given an ordered portal chain P_source → P_1 → P_2 → … → P_target, the anti-penumbra of (P_i, P_{i+1}) is the set of rays originating in any point of P_i that pass through P_{i+1}. Its boundary is built from separating planes between pairs of edges (one from P_i, one from P_{i+1}) such that P_i lies on one side and P_{i+1} on the other. The intersection of anti-penumbras along the chain clips a candidate target portal polygon; if the clipped polygon has non-zero area, the target leaf is visible from the source.

**`separating_planes` pseudocode** (corresponds to `ClipToSeperators` in Quake 3 `flow.c`):

```
fn separating_planes(P_i: &[DVec3], P_j: &[DVec3]) -> Vec<Plane>:
    planes = []
    for edge e in edges(P_i):          // each consecutive vertex pair of P_i
        for vertex v in P_j:           // each vertex of P_j
            plane = Plane::through(e.a, e.b, v)
            // flip normal so P_i is on the positive (front) side
            if dot(plane.normal, centroid(P_i)) < plane.d:
                plane = plane.flipped()
            // keep only if all of P_j is also on the positive side
            if all vertices of P_j satisfy dot(plane.normal, vtx) >= plane.d - EPS:
                planes.push(plane)
    return planes
```

The set of kept planes bounds the anti-penumbra wedge for (P_i, P_j). Reference: Quake 3 `vis/flow.c::ClipToSeperators` for the canonical production implementation.

ASCII (edge-on, chain of three portals):

```
source leaf   P1        P2        P3   target leaf
              |         |         |
              |   anti-penumbra(P1,P2)
              |<========|========>|
              |         |   anti-penumbra(P2,P3)
              |         |<========|========>|
                            intersection ──► clip P3 ──► non-empty ⇒ visible
```

References: Seth Teller, *Visibility Computations in Densely Occluded Polyhedral Environments*, PhD thesis, UC Berkeley 1992, §4 ("Anti-penumbra"). Quake 3 `vis/flow.c` `ClipToSeperators`, `FindPassages` — canonical implementation that ships in production. Use Teller's separating-plane construction; Quake 3 is the implementation reference.

**Replace the BFS.** In `compute_pvs` at `portal_vis.rs:30`, the BFS is structurally wrong for anti-penumbra because the wedge-intersection test depends on the ordered chain of portals, not just reachability. Rewrite as a recursive flood from each source leaf:

- For each non-solid source leaf, iterate its portals from `leaf_portals` at `portal_vis.rs:13`. Each portal is the chain's first link.
- At each step, we hold: the running anti-penumbra (a set of clipping planes) and the current portal polygon clipped into that anti-penumbra (the "visible slice" still passing through).
- For each outgoing portal from the neighbor leaf, construct the new anti-penumbra from the previous portal and the new portal, intersect with the running wedge, and re-clip the new portal polygon against it. If the clipped polygon has positive area (above `MIN_PORTAL_AREA` at `portals.rs:22` — reuse the same threshold), mark the neighbor leaf visible and recurse.
- Recursion terminates when no outgoing portal survives clipping (wedge-empty). No chain-depth cap: rely on wedge-empty termination alone. A cap can be added later if a pathological map surfaces — see Out of scope.

Each source leaf's flood is independent, so the `rayon` `into_par_iter` at `portal_vis.rs:33` stays.

**New helper module.** Put the wedge math in `postretro-level-compiler/src/visibility/anti_penumbra.rs`:

- `fn separating_planes(source: &[DVec3], target: &[DVec3]) -> Vec<(DVec3, f64)>` — enumerate candidate edge-pair separating planes; keep those with `source` wholly on one side and `target` wholly on the other. Return `(normal, distance)` in the same convention as `BspNode` and `PlaneEntry` in `portals.rs`.
- `fn clip_polygon_against_planes(poly: &[DVec3], planes: &[(DVec3, f64)]) -> Vec<DVec3>` — Sutherland–Hodgman against the stack. `geometry_utils::clip_polygon_to_front` (used by `portals::make_node_portal`, see `portals.rs:8`) is the existing single-plane clipper; the multi-plane variant here is a thin loop over it. Do not reuse `postretro/src/portal_vis.rs::clip_polygon_to_frustum` — that operates on view frustum planes in engine coords and is structured for runtime scratch-buffer threading (see `context/plans/done/render-perf/index.md` Task A); different call shape, different ownership story.
- `fn polygon_area(poly: &[DVec3]) -> f64` — reject slivers below `MIN_PORTAL_AREA`.

Double precision throughout (matches `DVec3` in `partition`/`portals`). `DVec3 → [f32; 3]` narrowing only at PRL emit (`visibility/mod.rs::dvec3_to_f32_array` at line 21).

**Cross-reference, do not conflate.** The runtime portal flood (`postretro/src/portal_vis.rs`) clips portal polygons against a viewer-derived narrowing frustum every frame. The baker clips portal polygons against static anti-penumbra wedges once at compile. Same Sutherland–Hodgman shape, different inputs, different ownership (scratch-buffered at runtime, one-shot at bake), different call sites. Keep them in separate modules.

**Correctness note.** A leaf removed from the PVS is a leaf no sightline can reach from the source. No missing-geometry regressions at runtime are possible. The existing round-trip tests (`leaf_pvs_section_round_trips` at `visibility/mod.rs:449`) exercise encode/decode, not coverage; they pass unchanged. The topology tests in `portal_vis.rs` (lines 83–204) need to be re-examined — several currently assert full visibility through a chain (e.g. `three_leaves_in_chain_all_see_each_other` at line 100) that remains true for collinear portals. The test-only `make_portal` at line 74 uses a degenerate 3-vertex polygon (`DVec3::ZERO, DVec3::X, DVec3::Y`); these degenerate fixtures produce zero-area clip wedges under anti-penumbra and must be retargeted to real convex polygons as part of Task A. Scoping them to `--pvs-fast` only would leave the anti-penumbra chain path without unit coverage, which is worse than rewriting the fixtures.

---

### Task B — `--pvs-fast` opt-out and before/after popcount logging

**Crate:** `postretro-level-compiler` · **Files:** `src/main.rs`, `src/visibility/mod.rs`

**Fix 1: `--pvs-fast` flag.** `parse_args` in `main.rs:293` already handles `--pvs` at line 318. Add a sibling `--pvs-fast` bool to `Args` at line 276. Thread through to `compute_pvs` as a mode enum (`Bfs` vs `AntiPenumbra`). Default when `--pvs` alone: anti-penumbra (on by default — compile cost is paid once per map, and the engine targets a published format for modders). Default when `--pvs --pvs-fast`: BFS (current behavior, for iteration speed during authoring). Update the usage string at line 379 to match.

Rationale for opt-out: anti-penumbra cost grows with portal chain depth (empirically tens of seconds for `vis -full` on small-to-medium Quake 3 maps, minutes for large ones). The iteration loop benefits from a fast path that still produces a correct-but-loose PVS.

**Compile-time budget.** Log bake time for both modes on every test map (`occlusion-test.map`, `test.prl`, and any others compiled during PR verification). Soft target: anti-penumbra bake completes within 3× the BFS bake time on all current test maps. If a map exceeds this threshold, automatically degrade to `--pvs-fast` for that map and emit a warning log line: `anti-penumbra bake exceeded 3× BFS time on <map>; falling back to topological BFS`.

**Fix 2: popcount logging.** `log_stats` at `visibility/mod.rs:298` already computes and logs min/max/avg visible-leaves-per-leaf (lines 325–341). That output is the primary before/after signal. Keep the call site. Add a one-line log at stats emit summarizing the PVS mode (`anti-penumbra` vs `topological BFS`) so the log line identifies which generator produced the numbers.

No format change. PVS bitsets are encoded identically; only their content differs.

---

## Files to modify

| File | Task | Change |
|------|------|--------|
| `postretro-level-compiler/src/visibility/portal_vis.rs` | A | Replace BFS in `compute_pvs` with anti-penumbra flood; accept a mode parameter; retain BFS path for `--pvs-fast` |
| `postretro-level-compiler/src/visibility/anti_penumbra.rs` | A | New file: `separating_planes`, `clip_polygon_against_planes`, `polygon_area` |
| `postretro-level-compiler/src/visibility/mod.rs` | A, B | Thread mode into `encode_vis`; log PVS mode in `log_stats` |
| `postretro-level-compiler/src/main.rs` | B | Add `--pvs-fast` flag; thread to `encode_vis`; update usage string |

---

## Acceptance Criteria

1. `cargo test -p postretro-level-compiler` passes. Topology tests retargeted per Task A's correctness note (either given real portal polygons or scoped to `--pvs-fast`/BFS).
2. No new `unsafe` blocks.
3. **Task A:** On every `.prl` output produced with `--pvs` (no `--pvs-fast`), average PVS popcount per empty leaf is strictly less than or equal to the popcount produced by BFS on the same map, and strictly less on `occlusion-test.map` specifically. Verification: compile each test map with and without `--pvs-fast`, compare the `Visible leaves per leaf: min=… max=… avg=…` log line from `log_stats:339`.
4. **Task A:** Runtime rendering of any `--pvs`-compiled map is bit-identical to the same map compiled with `--pvs-fast`. Verification: spot-check `assets/maps/test.prl` and `assets/maps/occlusion-test.map` outputs in both textured and wireframe modes; no geometry disappears.
5. **Task B:** `prl-build --pvs --pvs-fast input.map` produces byte-identical output to the current `prl-build --pvs input.map` (until the default flip). The default `prl-build --pvs` path uses anti-penumbra.
6. **Task B:** Bake time is logged for both modes on every test map. Anti-penumbra completes within 3× BFS time on all current test maps (soft cap); maps that exceed the cap fall back automatically with a warning. Quote actual before/after times in the PR description. `--pvs-fast` compile time matches the current BFS time.

---

## Out of scope

- PVS storage format changes. `LeafPvsSection` (`postretro-level-format/src/leaf_pvs.rs`, id 14 in `build_pipeline.md` §PRL section IDs) is unchanged; compressed bitset layout unchanged.
- The runtime portal-flood path (`postretro/src/portal_vis.rs`, non-`--pvs` mode). That path already applies geometric tightening per frame via `clip_polygon_to_frustum`; anti-penumbra is the compile-time analog for the other branch, not a replacement.
- PVS for anything other than leaf-to-leaf visibility — no entity-, light-, or sound-propagation PVS.
- Parallelizing the anti-penumbra flood beyond the existing per-source-leaf `rayon` split at `portal_vis.rs:33`.
- Hierarchical PVS or potentially-audible-set (PAS) extensions.
- Sharing Sutherland–Hodgman code between the baker and the runtime portal clipper — cross-reference only.
- Chain-depth cap for anti-penumbra recursion — wedge-empty termination is sufficient; add a cap only if a pathological map surfaces.
- Exterior-leaf PVS membership: exterior leaves have `face_count = 0` and emit zero draws regardless of PVS membership. Anti-penumbra may drop them from more PVS rows than BFS did, which is a further free tightening. No test should assert exterior leaves appear in a PVS bitset.

---

## Open Questions

None currently open. Resolved decisions:

- **Always-on vs. flag-gated:** anti-penumbra is on by default under `--pvs`; `--pvs-fast` opts out. (Resolved: published format, modder-friendly, compile cost paid once.)
- **Recursion depth cap:** wedge-empty termination only; no chain-depth cap. Add cap if a pathological map surfaces.
- **Exterior leaves:** no-op at runtime (`face_count = 0`); moved to Out of scope.
- **Legacy topology tests:** retarget to real polygon geometry as part of Task A. Do not scope to `--pvs-fast`; that would leave the anti-penumbra chain path without unit coverage.
