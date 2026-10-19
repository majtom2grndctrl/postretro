# fluid-surface-t-junction-closure — plan of record

mode: compact
status: active
read at: 0f069d775

## Corrections
- Source cited by Decisions and Path is unchanged since the brief's `read at` (8bb02a18c); commits since touch movement, sim and plans only.
- A parked attempt from the brief's first promotion (branch `claude/fluid-surface-t-junction-closure`, worktree `.claude/worktrees/fluid-surface-t-junction-closure`, base 0ca951403) holds the test split as a commit and, uncommitted, a level-wide closure, install bounds and the runtime drift change. Building on it: the split is cherry-picked as its own commit, the uncommitted work applied, then extended for the rows the brief gained after it was parked (P15 buried-only, P22–P24, angle order, the drift and id-54 map rows). The parked worktree is left untouched; the owner decides its removal at landing.
- Research §Consumers: `grid::cut_face` splits the remaining polygon plane by plane, so its cost is O(V·P) per axis, not only a copy; P24 needs a single sweep per axis. Planning around it by rewriting the per-axis cut as one ring walk that assigns each vertex to its slab, with the same on-plane run rule and per-edge crossings.

## Owner rulings
- 2026-10-19, from review: the angle-order row and the insert edge-bound row conflict for a foreign corner within 0.1 mm of an edge but past a near-flat corner's ray from the centroid. The owner chose strict angle order (rings stay star-shaped about the area centroid, so the centroid fan never inverts and every artifact stays on the edge) and authorized restating the edge-bound row as: "An inserted vertex lies within 0.1 mm of the face's boundary as closure leaves it, which an earlier insert may have bent (P18), on the edge whose sector about the area centroid holds it; beside a near-flat corner that edge may lie up to about half a millimetre off." Applied to `index.md`; an obtuse-corner fixture pins it.

## Delegated answers
- Weld mechanism, canonical choice and index shape — vertices taken in ascending encoded-position order each join the nearest earlier weld anchor within 0.1 mm that holds no vertex of their own face, or found one; the anchor keeps its founder's encoded position. A hash of tolerance-sized cells, probed with its 26 neighbours, finds anchors. Inserts use a bounding-volume tree over the distinct welded positions, queried per edge, repeated on faces whose ring changed until none gains a vertex. Reason: no vertex moves past the tolerance, a weld chain breaks instead of drifting (P10, P21), one face's corners stay distinct (P17), and lookups are output-sensitive and order-independent of thread count.
- Insert-bound proof — "within 0.1 mm of the edge it joins as closure leaves it" read as: at the moment it joins. A ring-neighbour reading contradicts P22, whose two corners join one edge together and each end 0.12 mm from the other's chord. The check peels added vertices back off, singly against their neighbours' chord or as a run against the kept vertices either side, which certifies an insertion order in which each lay within the tolerance of its edge.
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
| 6 move bound, weld chains, insert within tolerance of the face's boundary as left, on its sector's edge (P10, P18, P21; restated, see Owner rulings) | fixtures and maps | achievable as stated |
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

## Follow-ups
- Sibling subtraction is all-pairs within one fluid entity (`fluid_surface_pieces`): a 4,000-brush row takes about 32 s, every warm build, since the stage is uncached. Closure on the same level takes 0.25 s. Outside this brief; it wants the `SolidIndex` treatment for siblings.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Adopt the parked work: split commit, closure, install bounds, drift; baselines (pre-change `prl-build`, guard hashes); rerun the first slice on e24 and stress-env-volumes and every listed map — the riskiest premise is that one level-wide pass closes every map | integrating executor | — | done: every map closes (open before → after: e24 0→0, campaign-test 0→0, reflection-probes-test 10→0, vault-test 0→0, stress-env-volumes 6173→0; 832 welded, 2389 inserted, largest move 0.034 mm); guard hashes re-derived from the pre-change compiler match |
| 2 | Insert edge choice by angular sector; P22, P23, P15 buried-only fixtures; angle-order assertion | integrating executor | 1 | done: `assert_face_kept` (angle order, peel-back insert bound, area, distinct, normal) on every fixture; P23 fails under first-edge choice, passes under the sector rule; maps still close |
| 3 | Map suite: check, nothing-else-moves, drift push, drift map, id-54 bindings | integrating executor | 2 | done: `closure_maps` (ignored) — every listed map closes and keeps its faces; reflection-probes-test 8 and stress-env-volumes 382 bindings unchanged, 0 tilted, drift maps within 0.0012 texels; placement runs in-process through `reflection_probes::placed_bindings` on `fixture_pipeline` |
| 4 | Bounded closure counts on a synthetic ocean | integrating executor | 2 | done: `closure_bounds` (ignored for fixture cost): brick ocean 16→32 columns as one entity and per brush, and one long top beside 1000→4000 small brushes, weld and insert tests within n log n. Per-vertex insert tests plateau (34, 37, 32 at 16/32/48 columns); 8 columns is dominated by the ocean's edge, so the test starts at 16 |
| 5 | Determinism on a level that welds and inserts; id-54 cache hit | integrating executor | 2 | done: `tests/fluid_seam_determinism.rs` — slime brush 0.05 mm off the pool's side forces a weld; the compiler's log line shows welds and inserts; id-52 and id-54 identical cold ×2, one worker, warm fill, warm hit; id-54 misses on the fill and hits next |
| 6 | End to end through the renderer's packing on a lattice | integrating executor | 2 | done: `faces_meeting_at_a_shallow_angle_to_a_lattice_plane_share_their_gridded_edge` — before closure the south top gains a crossing 12.8 mm from the T-vertex the north holds; after, both carry identical vertices |
| 7 | Grid cut as one sweep per axis (P24); every-plane install bound | integrating executor | 1 | done: `cut_axis` matched the successive split on 25,805 generated parts (temporary equivalence test, not kept); the every-plane case fails the old cut (ratio 14.9 vs 4.7 allowed) and passes the sweep |
| 8 | Drift sample point shared from render-cpu; P13 | integrating executor | 1 | done: `face_area_centroid` in render-cpu is the runtime drift sample and the centroid-fan apex; P13 `a_piece_clear_of_a_push_drifts_as_without_thousands_of_inserts_on_its_boundary_edge` (300 m out, 4000 inserts) passes |
| 9 | Context docs: build_pipeline §Fluid surfaces, rendering_pipeline §7.3a | integrating executor | 2 | done: gate retired, closure contract and byte rule stated, accepted limit narrowed, bounded install and area-centroid sample recorded; grep gate clean |
| 10 | Preflight, review panel, fixes; manual timings | integrating executor | 1–9 | |
