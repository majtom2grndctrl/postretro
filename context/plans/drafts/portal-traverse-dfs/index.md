# Per-Path DFS in Portal Traversal

> **Status:** draft
> **Depends on:** none. The portal polygon clipping (`drafts/portal-polygon-clipping/`) it builds on is already in the engine code; only the traversal topology is changing.
> **Related:** `context/lib/build_pipeline.md` §Runtime visibility · `context/plans/drafts/portal-polygon-clipping/index.md` · `postretro/src/portal_vis.rs`

---

## Goal

Replace `portal_traverse`'s BFS-keyed-on-visited-leaves topology with recursive DFS keyed on portals already crossed in the current chain. Each leaf can be reached through any number of independent chains, each carrying its own narrowed frustum. The visible bitset becomes a write-only union of every successful chain.

Match the algorithmic shape of id Tech 4's `FloodViewThroughArea_r` (Doom 3, GPL release 2011, `neo/renderer/RenderWorld_portals.cpp`) — the standard published portal-vis flood-fill.

---

## Motivation

`portal_traverse` today is BFS over leaves with this cycle-breaker:

```rust
if visible[neighbor] {
    continue;
}
```

That pattern is correct for PVS-style flood fill where reachability is binary — "did anything reach this leaf at all" is the only question. It is incorrect for portal-clipped visibility, where the *frustum carried by the path to a leaf* determines what becomes visible downstream of that leaf. Two chains converging on the same leaf can produce dramatically different downstream reach. The chain that arrives first wins, and the alternative chain's wider sub-frustum is silently dropped by the early skip.

This is the concrete failure observed in `assets/maps/map-2.prl` at camera position `(-7, 2, -12)`. When the camera faces the central corridor, the corridor is culled. When the camera moves a few units forward, it reappears. Both poses live in leaf 168, both reach leaf 167, both have the same outbound portals from leaf 167. The only thing that differs is which intermediate chain reached leaf 167 first, and therefore which narrowed frustum 167 inherited:

| Pose | 167 reached via | 167 outbound portals |
|---|---|---|
| Bug (corridor missing) | `170 → 167`, narrowed frustum from 170-path | All seven outbound portals (156, 157, 158, 159, 163, 193, 196) clip to empty |
| Working (corridor visible) | `171 → 167`, narrowed frustum from 171-path | Six outbound portals (156, 158, 163, 193, 196, 174) accept |

Under BFS the leaf-168 outbound portals are processed in iteration order. In the bug pose, leaf 168 accepts portal 170 before portal 171, so 170 is processed first and plants its narrower sub-frustum at 167 before 171's wider chain ever has the chance. The 171→167 portal is then silently rejected by the `visible[167]` early-skip — it shows up only in the `rejected_already_visited` counter, never in the per-portal trace.

The fix is to stop keying termination on the leaf set and instead key it on the per-path portal set. A leaf may be re-visited through any chain that does not loop on a portal it already crossed. The visible bitset is the union of all chains.

---

## Scope

### In scope

- Rewrite `portal_traverse` in `postretro/src/portal_vis.rs` from a `VecDeque`-based BFS to recursive DFS with per-path portal tracking.
- Port the existing `postretro::portal_trace` instrumentation onto the recursive form: same target, same chord (`Alt+Shift+1`), same one-shot capture flag passed in from `determine_prl_visibility`. The `capture: bool` parameter on `portal_traverse` stays in the signature unchanged; no upstream caller needs to know the topology changed.
- Add one regression test in `portal_vis::tests` that captures the "two paths to the same leaf, narrower path wins, downstream reach is lost" topology in a hand-built `LevelWorld`. The test must fail against the current BFS code and pass against the DFS code.
- Document the recursion-depth bound and the per-path allocation overhead in a code comment on the new function.

### Out of scope

- Performance optimization. Profile after correctness lands. Pre-optimization here is one of the things that produced the original bug.
- Changes to `clip_polygon_to_frustum`, `narrow_frustum`, or any other helper. They are correct as written.
- Splitting `portal_vis.rs` into a submodule directory. The file is large enough that this will eventually be worth doing, but per the development guide we do not mix structural refactoring with bug fixes. Tracked as a follow-up.
- Retiring the BSP legacy visibility path in `visibility.rs`. Independent.
- Updating `context/lib/build_pipeline.md` §Runtime visibility. The text describes single-pass polygon clipping, which remains correct. Only the traversal topology is changing, and topology is not described in that document.
- Changing the PRL file format, the level compiler, or the portal generation step.

### Non-goals

- Matching id Tech 4's source code line for line. Adopt the algorithmic shape, not the variable names.
- Eliminating the per-path allocation entirely. A small Vec clone per branch is the obvious starting point. Promotion to `SmallVec` or a bitmask is a profile-driven follow-up.
- Catching every theoretically pathological recursion depth. Document the bound and revisit only if real maps exceed it.

---

## Approach

### Algorithm

```
flood(leaf, frustum, path):
    visible[leaf] = true
    for portal in leaf.outbound_portals:
        if portal in path:                         # would loop
            continue
        neighbor = portal.other_side(leaf)
        if neighbor invalid or solid:
            continue
        clipped = clip_polygon_to_frustum(portal.polygon, frustum)
        if clipped.len() < 3:                      # not visible through cone
            continue
        narrowed = narrow_frustum(camera_pos, clipped, frustum)
        if narrowed is None:
            continue
        flood(neighbor, narrowed, path + [portal])
```

Initial call: `flood(camera_leaf, camera_frustum, [])`. The visible bitset is the OR across all branches, accumulated by the in-place writes.

The recursion terminates naturally because each chain's narrowed frustum is a strict geometric subset of its parent (the invariant established by the polygon-clipping plan). At sufficient depth, every outbound portal clips to empty and the chain stops.

### Inherited safety invariant

The DFS rewrite removes one under-culling failure mode (the leaf-keyed early skip dropping wider chains) but does not introduce an over-drawing failure mode. The reason is structural and worth stating explicitly.

The polygon-clipping work established that every narrowed frustum produced by clipping the portal polygon against the parent frustum and then building edge planes from the clipped polygon is a strict geometric subset of that parent frustum. Under DFS, every leaf marked visible was reached through some chain whose final frustum is — by induction from that single-hop invariant — a strict subset of the camera frustum. Therefore every leaf in the visible set lies inside the camera cone.

Letting more chains run does not let any chain produce a frustum wider than the camera frustum, because the operation that builds each chain's frustum is monotonically narrowing. The fix expands the visible set to include leaves that were wrongly culled; it cannot include leaves outside the camera cone. The "more is visible now" outcome is corrected under-counting, not new over-counting. Over-drawing is impossible by construction as long as the polygon-clipping invariant holds.

### Why DFS rather than BFS

The call stack itself encodes "the portals already crossed to get here." BFS would have to materialize that path explicitly on every queue entry, which doubles per-step memory pressure with no benefit — there is no ordering advantage to BFS in this algorithm because we visit every reachable chain anyway. DFS is also closer to the published reference implementations.

### Per-path portal tracking

The path is a `Vec<usize>` (portal indices). At each recursion step, the candidate portal is checked for membership via linear scan. For typical chain depths (5–10, occasionally ~20) linear scan over a small Vec beats HashSet hashing.

The Vec is cloned at each recursive call: when leaf L recurses through portal P into neighbor N, the recursive call receives a clone of L's path with P appended. The clone is necessary because sibling branches at the same depth must see independent path histories. In the simple case where a leaf has only one outbound portal worth recursing into, the clone could be elided — but the optimization is not worth the code complexity until profiling shows it matters.

For typical maps, this is a handful of small allocations per visited leaf. If profiling later shows allocation pressure, the path can move to `SmallVec<[usize; 16]>` (stack-allocated up to 16 entries) without changing the algorithm.

### Recursion depth and failure mode at the limit

Real-map portal chains run 5–10 deep, occasionally ~20. Rust's default stack is 8 MB; even a 100-deep chain using a few hundred bytes per frame is comfortable. No explicit-stack rewrite is needed for typical content.

The hard question is what happens if a future map produces a chain deeper than the stack can hold. Native recursion on overflow panics — a player-visible crash, not graceful degradation. To make the failure mode bounded, the recursive helper enforces an explicit `MAX_PORTAL_CHAIN_DEPTH` constant (initial value: 256, well above any realistic chain depth and well below stack-overflow territory). When a chain hits the limit, the helper logs a warning under the `postretro::portal_trace` target (regardless of whether capture is on) naming the camera leaf and the truncation point, marks the current leaf visible, and returns without recursing further. The visible set is conservative under the limit — leaves reachable only through chains deeper than the limit are missed — but no crash occurs and the warning surfaces the anomaly. The constant lives at the top of `portal_vis.rs` so it can be tuned without searching for the call site.

### Trace instrumentation

The `postretro::portal_trace` capture instrumentation ports cleanly:

| Trace point | Old (BFS) | New (DFS) |
|---|---|---|
| `start camera_leaf={N}` | Top of `portal_traverse` | Top of `portal_traverse`, before the first recursive call |
| `accept src=A dst=B clipped_verts=N` | Inside the BFS loop after a successful narrow | Inside the recursive helper, just before the recursive descent |
| `reject src=A dst=B reason=clipped_to_empty` | Inside the BFS loop on empty clip | Inside the recursive helper at the same site |
| `reject ... reason=narrow_frustum_failed` | Inside the BFS loop on narrow failure | Inside the recursive helper at the same site |
| `reject ... reason=solid_neighbor` | Inside the BFS loop on solid neighbor | Inside the recursive helper at the same site |
| `summary reach=... considered=... ...` | After the BFS loop exits | After the top-level recursive call returns |

Counter changes:

- `rejected_already_visited` is removed. The rejection class no longer exists — a leaf can be re-reached through any path that does not loop on a portal.
- New: `rejected_path_cycle`. Counts the case where an outbound portal is already in the current chain's path (would form a loop). On a well-formed map this counter should be zero or very near zero; portals typically connect distinct leaves and the natural shape of leaf-to-portal traversal does not produce loops. A persistently non-zero count at runtime means the level compiler emitted a portal layout where the same portal can be re-entered along a chain that does not revisit through the leaf side. This is structurally unusual and is a signal that PRL portal generation produced a degenerate topology worth investigating in the level compiler — not in `portal_traverse` itself, where the per-path skip is the correct response.

The on-screen reading of the trace becomes depth-first instead of breadth-first. That is a more legible representation of "the chain from leaf X dives into Y, then Z, then comes back and dives into W" anyway.

---

## Acceptance Criteria

1. The new regression test described under *Regression Test* below fails against the current BFS-keyed-on-leaves implementation and passes against the DFS implementation. Both runs are recorded in the PR description.
2. All existing `portal_vis::tests::*` tests continue to pass without modification. They are simple chain topologies where DFS and BFS converge on the same answer.
3. All existing `visibility::tests::*` tests continue to pass without modification.
4. After the fix lands, re-capturing the `map-2.prl` traces at camera position `(-7, 2, -12)` in the previously-broken pose shows leaf 167 reached via the wider-frustum chain, with its outbound portals (156, 158, 163, 193, 196, 174) accepted rather than rejected. The summary line's `reach` count from the broken pose is at least the working pose's reach count from the prior capture.
5. Manual verification in `assets/maps/map-2.prl`: at position `(-7, 2, -12)`, the central corridor is visible regardless of camera angle. The wireframe overlay (`Alt+Shift+\`, mode `Culled`) shows the corridor's wireframes in both the previously-broken and previously-working poses, and the textured pass renders the corridor in both poses.
6. No visible degradation on any other test map. Spot-check `test.prl` and `test-2.prl` if present, and any other maps used during the regression hunt.
7. No new `unsafe` blocks. No `unwrap()` or `expect()` in non-test code beyond what the development guide allows for structurally-guaranteed invariants.
8. `cargo check -p postretro` clean. `cargo test -p postretro` passes the full suite (current count is 268 tests).
9. **Test suite runtime does not regress.** The `portal_vis::tests` module currently runs in well under a second; under DFS it should still run in well under a second on the same hardware. Any meaningful slowdown indicates an algorithm bug, not an optimization opportunity.
10. **Frame rate on `map-2.prl` does not regress meaningfully.** Manual verification: launch the engine on `map-2.prl`, fly through the level for at least one minute (including the previously-broken position at `(-7, 2, -12)`), and confirm the framerate is comparable to the pre-fix run. The fix increases the visible leaf count in some poses, so a small performance cost is expected; a large drop indicates a separate optimization concern but is not a reason to roll the fix back. Document the observed framerate range (pre-fix and post-fix) in the PR description so the change is recorded.

---

## Regression Test

A minimal hand-built `LevelWorld` with this topology:

```
        +-----+
        |  X  |
        +--+--+
       /   |   \
      /    |    \
+----+   (P3)   +----+
|  B |    |    |  C |
+----+    |    +----+
   \      |      /
   (P1)   |   (P2)
     \    |    /
      \   |   /
       +--+--+
       |  A  |   <- camera leaf
       +-----+
```

- Five leaves: `A` (camera), `B`, `C`, `X`, `Y`.
- Portals: `A→B` (wide aperture, accepted by camera frustum), `A→C` (also wide, accepted), `B→X` (carries a narrow sub-frustum that clips X's outbound portal `X→Y` to empty), `C→X` (carries a wider sub-frustum that does not clip `X→Y` to empty).
- `Y` is reachable only from `X`.

Expected behavior:

- **Under BFS-keyed-on-leaves (current code):** `A→B` fires before `A→C` in iteration order. `B→X` is accepted, marks `X` visible with the narrow frustum. When `A→C` is later processed, `C→X` is silently rejected because `visible[X]` is already true. `X→Y` is then evaluated against the narrow B-path frustum, clips to empty, and `Y` is missed. `visible[Y] == false`.
- **Under DFS-with-per-path-tracking (target code):** Both `A→B→X→Y` and `A→C→X→Y` chains run independently. The C-path produces a wider sub-frustum at `X` that does not clip `X→Y` to empty. `visible[Y] == true`.

The test asserts `visible[Y] == true && visible[X] == true && visible[B] == true && visible[C] == true`.

Constructing the geometry requires choosing portal polygon coordinates so the sub-frustum widths come out unambiguously different. The test fixture should pre-compute portal positions (literal `Vec3` arrays) such that:

- Both `A→B` and `A→C` are wide enough to be accepted by a default test camera frustum.
- `B→X` is a narrow slit positioned so its narrowed frustum cone's edges fall *inside* `X→Y`'s bounds.
- `C→X` is wider, positioned so its narrowed frustum cone's edges fall *outside* `X→Y`'s bounds.

The geometry is structural rather than numerically marginal — small changes to `narrow_frustum`'s precision should not destabilize it. The fixture lives in `portal_vis::tests` alongside the other nine traversal tests. It should be named `portal_traverse_two_paths_to_same_leaf_uses_widest_frustum` or similar.

**Implementation note: the geometry is the hard part of the test.** The test depends on hand-built portal coordinates satisfying four constraints simultaneously: both `A→B` and `A→C` accepted by the camera frustum, the narrowed frustum from `B→X` clips `X→Y` to empty, the narrowed frustum from `C→X` does not, and the BFS iteration order over `world.leaf_portals[A]` puts `B` before `C` so the BFS code deterministically picks the bug-causing path first. The first and last constraints are trivial. The middle two are the load-bearing claim — they require pre-computing the shape of the narrowed frustum for two specific input polygons and confirming the difference is structurally robust under reasonable epsilon tweaks.

If hand-built coordinates can't satisfy the middle two with structural margin during implementation, fall back to a programmatically-generated topology: build a chain where `B→X` is forced narrow through repeated narrowing across a sequence of slits, while `C→X` is reached through a single wide aperture. The fallback is more code but eliminates the dependence on hitting specific numeric thresholds in one shot. Decide which approach during implementation; the Acceptance Criteria do not depend on which fixture style ships, only on the test capturing the topology bug deterministically.

**Honest framing of what the test asserts.** Because the test fixture controls portal insertion order in `world.leaf_portals[A]`, the test is technically asserting "for this specific portal ordering on this specific topology, BFS produces a wrong result and DFS produces a right result." It is not a general proof that BFS is wrong on all inputs. It is a specific, deterministic, reproducible counterexample — which is what a regression test is supposed to be. A general proof of BFS-vs-DFS correctness would require an exhaustive enumeration of portal-graph topologies, which is out of scope and almost certainly the wrong shape of guarantee anyway.

---

## Risks

- **`MAX_PORTAL_CHAIN_DEPTH` guard fires on a real map.** The 256-deep limit (see Approach §Recursion depth) is well above realistic chain depth, but if a future map produces chains longer than that the guard kicks in and the visible set silently undercounts at those leaves, with a warning logged under `postretro::portal_trace`. Leaves missed are the ones reachable only via paths longer than 256 portals — almost certainly not user-noticeable, but technically a correctness gap. Mitigation: tune the constant upward if observed; long-term, an explicit-stack rewrite would eliminate the gap entirely if it ever became real.
- **Per-path Vec clone cost on highly-branching maps.** Each branch clones the path. For maps with 1000+ portals and a high branching factor this is real allocation pressure. Mitigation if needed: `SmallVec` or stack-allocated path. Not pre-optimized; flagged as a future profiling target.
- **Test dependence on `narrow_frustum`'s numerical behavior.** If the regression test geometry depends on `narrow_frustum` returning a specific frustum, future tweaks to numerical handling could destabilize the test. Mitigation: design the test geometry so the BFS-vs-DFS distinction is structurally clear, not numerically marginal. Document the geometric reasoning in test comments so future maintainers understand which numbers can move and which cannot.
- **The fix increases visible leaf counts on some maps.** That is the whole point — leaves that were wrongly culled now appear — but it has performance implications. Manual verification on `map-2.prl` should include a framerate sanity check. If the overall framerate regresses meaningfully on a complex map, that is a separate optimization concern, not a reason to roll the fix back. Visibility correctness comes first.
- **The visible bitset semantics are unchanged** (still "set true if any chain reaches the leaf"), so downstream consumers in `determine_prl_visibility` need no changes. Their consumption pattern (`portal_visible.get(leaf_idx).copied().unwrap_or(false)`) is robust to the change.
- **The trace output ordering changes from breadth-first to depth-first, and saved captures are not comparable line-for-line across the fix.** Diagnostic captures taken before this change reflect BFS-level ordering — all of leaf 168's outbound portals first, then all of 170's, then all of 171's. Captures taken after this change reflect DFS chain ordering — one chain to its terminal leaves, then the next chain, then the next. The summary line is the only field that is stable across the fix: `reach`, `considered`, `accepted`, `rejected_clipped`, `rejected_narrow`, and `rejected_solid` retain their meaning. Two counters change: `rejected_already_visited` is removed entirely, and `rejected_path_cycle` is added with different semantics. Anyone trying to spot a regression by diffing old and new captures should diff at the summary level only, not at the per-portal level. The two `map-2.prl` traces from this debugging session are useful as historical evidence of the bug but cannot be directly compared to post-fix captures.

---

## What Carries Forward

| Output | Consumed by |
|--------|-------------|
| DFS-with-per-path-tracking traversal | All future runtime visibility work — area portals, dynamic occluders, mirrors, anything else built on portal flood-fill |
| Regression test capturing the multi-path-to-same-leaf topology | Guard against future BFS-style "optimizations" that re-introduce the leaf-keyed early-skip |
| Corrected `postretro::portal_trace` output ordering (depth-first) | More legible chain reading during diagnostic captures |
| Documented recursion-depth bound and per-path allocation profile | Inputs to any future portal-vis profiling pass |

---

## Open Questions

None at draft time. Promote to ready when scheduled.

---

## Follow-ups (out of scope, captured for the roadmap)

- **Split `portal_vis.rs`** along its real seams: traversal driver, polygon clipping, frustum narrowing, helpers. The file is over 1100 lines. Defer until after this fix lands; never mix structural refactoring with a bug fix.
- **Retire the BSP visibility path** in `visibility.rs` once PRL is the only runtime format. Independent of this fix.
- **Profile portal_traverse on a complex map** after the DFS rewrite stabilizes. The per-path Vec clone is the most likely hotspot if one exists.
