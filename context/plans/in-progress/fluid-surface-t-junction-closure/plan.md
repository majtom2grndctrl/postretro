# fluid-surface-t-junction-closure — plan of record

mode: compact
status: active
read at: 0f069d775

## Corrections
- Source cited by Decisions and Path is unchanged since the brief's `read at` (8bb02a18c); commits since touch movement, sim and plans only.
- A parked attempt from the brief's first promotion (branch `claude/fluid-surface-t-junction-closure`, worktree `.claude/worktrees/fluid-surface-t-junction-closure`, base 0ca951403) holds the test split as a commit and, uncommitted, a level-wide closure, install bounds and the runtime drift change. Building on it: the split is cherry-picked as its own commit, the uncommitted work applied, then extended for the rows the brief gained after it was parked (P15 buried-only, P22–P24, angle order, the drift and id-54 map rows). The parked worktree is left untouched; the owner decides its removal at landing.
- Research §Consumers: `grid::cut_face` splits the remaining polygon plane by plane, so its cost is O(V·P) per axis, not only a copy; P24 needs a single sweep per axis. Planning around it by rewriting the per-axis cut as one ring walk that assigns each vertex to its slab, with the same on-plane run rule and per-edge crossings.

## Delegated answers
- Weld mechanism, canonical choice and index shape — vertices taken in ascending encoded-position order each join the nearest earlier weld anchor within 0.1 mm that holds no vertex of their own face, or found one; the anchor keeps its founder's encoded position. A hash of tolerance-sized cells, probed with its 26 neighbours, finds anchors. Inserts use a bounding-volume tree over the distinct welded positions, queried per edge, repeated on faces whose ring changed until none gains a vertex. Reason: no vertex moves past the tolerance, a weld chain breaks instead of drifting (P10, P21), one face's corners stay distinct (P17), and lookups are output-sensitive and order-independent of thread count.
- Edge choice for an insert — a candidate within the tolerance of several edges (P23, a sliver, a point beside a corner) joins the edge whose angular sector about the face's area centroid holds it, else the nearest; it is placed in order along that edge. Reason: the angle-order row holds by construction, and a point near a corner but past both edge interiors still joins.
- Bounding the install consumers — fit each drift map to a spanning triangle found in three linear passes (within a factor of 4 of the largest triangle); ear-clip with ordered candidate sets (n log n); cut each axis in one sweep (P24); the runtime drift sample point is the face's f64 area centroid, shared from render-cpu so the compiler's tests read the same point. Reason: each is linear or n log n in a face's vertex count, and inserts add zero-area fan triangles, so the area centroid does not move toward an inserted edge (P13).

## AC-to-proof

Rows in Acceptance order.

| AC | Proof | Status |
|---|---|---|
| 1 shared boundaries identical on the fixture list, each failing before closure | `surface_tests::closure` fixtures on the level-wide `open_seams` checker; P15's single-face and buried-only levels exempt from failing before | achievable as stated |
| 2 holds on e24; `#[ignore]` campaign-test, reflection-probes-test, vault-test, stress-env-volumes | `surface_tests::closure` (e24), `surface_tests::closure_maps` (ignored) | achievable as stated |
| 3 end to end through the renderer's packing, failing today | `level-compiler` test packing a compiled shallow-angle fixture with `render-cpu` on a lattice, before and after closure | achievable as stated |
| 4 no face count, order or texture change; UV is own side's projection (P5) | `assert_nothing_else_moves` on every fixture and map | achievable as stated |
| 5 P9 and P19 left alone | fixtures | achievable as stated |
| 6 move bound, weld chains, insert within tolerance of its edge as left (P10, P18, P21) | fixtures and maps | achievable as stated |
| 7 no level-format, loader, wire or digest change; old PRL loads (P7) | diff gate against `main` over `crates/level-format`, `crates/level-loader`, `hash_environment_volumes` | gate |
| 8 three distinct vertices, a normal, area bound (P8, P17); P14 | fixtures and maps | achievable as stated |
| 9 angle about the area centroid strictly increases (P23) | angle-order assertion on every fixture and map | achievable as stated |
| 10 regression guard: identical id-52 bytes, e24 among them | blake3 hashes captured from the pre-change compiler this session (3 fixtures + e24 `48901a4b…`) | achievable as stated |
| 11 stress-env-volumes and reflection-probes-test: same drift push, matching drift map, same id-54 bindings except a tilted piece under 6 mm (P14) | `closure_maps` ignored test, before and after closure | achievable as stated |
| 12 id-52 identical cold/cold, warm/cold, worker counts, on a level that welds and inserts; id-54 key hits next warm build (P6) | `tests/fluid_seam_determinism.rs` | achievable as stated |
| 13 `context/lib` grep gate | grep at landing | gate |
| 14 closure candidate tests grow within n log n (synthetic ocean ×2 shapes, one face beside thousands) | closure count test at two sizes | achievable as stated |
| 15 face with thousands of inserts packs ungridded and gridded (one plane; every plane, P24), drift map matches, install count within n log n | `render-cpu` count test at two sizes | achievable as stated |
| 16 piece clear of a push ≥ 100 m out with thousands of inserts resolves the same push (P13) | `postretro` drift test | achievable as stated |
| 17 bake cost cold before/after, stress-env-volumes and campaign-test | manual timing, pre-change binary saved this session | manual |
| 18 fluid install time before/after on stress-env-volumes | manual timing | manual |
| 19 owner look in engine | owner | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Adopt the parked work: split commit, closure, install bounds, drift; baselines (pre-change `prl-build`, guard hashes); rerun the first slice on e24 and stress-env-volumes and every listed map — the riskiest premise is that one level-wide pass closes every map | integrating executor | — | done: every map closes (open before → after: e24 0→0, campaign-test 0→0, reflection-probes-test 10→0, vault-test 0→0, stress-env-volumes 6173→0; 832 welded, 2389 inserted, largest move 0.034 mm); guard hashes re-derived from the pre-change compiler match |
| 2 | Insert edge choice by angular sector; P22, P23, P15 buried-only fixtures; angle-order assertion | integrating executor | 1 | |
| 3 | Map suite: check, nothing-else-moves, drift push, drift map, id-54 bindings | integrating executor | 2 | |
| 4 | Bounded closure counts on a synthetic ocean | integrating executor | 2 | |
| 5 | Determinism on a level that welds and inserts; id-54 cache hit | integrating executor | 2 | |
| 6 | End to end through the renderer's packing on a lattice | integrating executor | 2 | |
| 7 | Grid cut as one sweep per axis (P24); every-plane install bound | integrating executor | 1 | |
| 8 | Drift sample point shared from render-cpu; P13 | integrating executor | 1 | |
| 9 | Context docs: build_pipeline §Fluid surfaces, rendering_pipeline §7.3a | integrating executor | 2 | |
| 10 | Preflight, review panel, fixes; manual timings | integrating executor | 1–9 | |
