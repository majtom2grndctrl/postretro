# occluder-first-partition: Task 9l prism-cost diagnosis

Recorded 2026-10-29 on `claude/occluder-first-partition` at HEAD `165b4e2ea`. No committed source changed. The ablation hooks were scratch edits, removed afterwards. `git status --short` then showed only this folder, and a release rebuild of the restored source hashes the same as 9k's frozen cap26 binary (`71fa88a6a140`). The hooks are kept as `ablation-scratch.patch`, which passes `git apply --check` at HEAD, so the run can be repeated.

## Method

**Plane kinds.** Each prism splitter is tagged when the prism is built.
- **Side, free or abutting.** Probes are taken past each edge, offset 0.1 m beyond the edge plane on the prism side. They sit at 5 points along the edge (fractions 0.1, 0.3, 0.5, 0.7 and 0.9) and at 3 depths behind the face (0.1, 0.5 and 1.0 m), for 15 probes per edge. A probe counts as open when the first-pass `OpenSpace::is_blocked` is false, so solid and exterior void both block. **Rule: an edge is free when strictly more than half its probes are open (8 or more of 15); otherwise it is abutting.** Sensitivity runs move the threshold to "any probe open" and to "all probes open".
- **Far.** The plane at the length bound, present only when the prism is truncated.
- **Source.** Each plane is also tagged as coming from an occluding brush (B) or an occluding section (S).

**Variants.** A kind mask filters `ShadowPrism::splitters` after `shadow_prisms` builds and dedups them, and a prism left with no splitter is dropped. Overlap still tests the full prism. Two filters were added for narrower variants:
- drop the prisms of horizontal faces (slabs such as decks and roofs);
- drop horizontal side planes (wall tops and bottoms).

**Counts.** The partition stage only, in process: parse, then `partition_with_occluders`, then the full-tree exterior flood, then `RuntimeCells::build`. Columns:
- runtime portals;
- interior cells;
- interior cells whose smallest AABB extent is under 0.5 m ("slivers");
- in-process faces.

The `none` and `all` rows reproduce 9k's cap26-noprism and cap26 exactly, for example occlusion-test 241/430 portals and 48/76 slivers. `select_prism_splitter` also logged every prism-rank tier split, with the kinds and the prisms that offered the chosen plane.

**C18 fixture.** The `walk_from_in_front_of_a_laned_wall…` harness was run once per variant, through the production `compile` and `load_prl` (75 poses × 12,032 hidden samples).

**Walks.** Two temporary-edit release `prl-build` binaries were built, with the mask and filter defaults set to a variant. They are in `target/occluder-9l/bin/`:
- `prl-build-bfwv` (`a0d978680146`);
- `prl-build-free` (`b067d2443913`).

The street was baked with 9k's flags: `--sh-probe-spacing 4.0 --lightmap-density 0.25` and a private fresh `--cache-dir`, deleted after the bake. The walks then used 9k's frozen harness binaries, unchanged: the sweep, the 608-pose rooftop sweep and `uncap9g` (`run9l.sh`). Trips are paired against 9g's before records. The occlusion-test C18 map sweep runs on a bfwv bake with 9k's `prl-build-tests-cap26`.

## Plane kinds per map (all prisms, HEAD)

| | occlusion-test | hallway | campaign-test | arena | street |
|---|---|---|---|---|---|
| Prisms: brush / section (truncated) | 12 / 5 (6) | 994 / 6 (164) | 4 / 7 (2) | 236 / 2 (68) | 76 / 12 (14) |
| Side free, B / S | 27 / 0 | 291 / 2 | 10 / 0 | 252 / 1 | 168 / 34 |
| Side abutting, B / S | 21 / 20 | 3,685 / 22 | 6 / 28 | 692 / 7 | 136 / 14 |
| Far, B / S | 3 / 3 | 164 / 0 | 0 / 2 | 68 / 0 | 11 / 3 |
| Edges partly open (0 < open < 1) | 1 | 1,127 (956 at 0.33–0.5) | 0 | 143 | 65 |
| Prism-rank tier splits: free / abut / far | 72 / 33 / 11 | 564 / 1,007 / 555 | 9 / 41 / 12 | 246 / 57 / 128 | 1,231 / 160 / 121 |
| Overlapping prism pairs (max per prism) | 40 (11) | 3,230 (77) | 22 (8) | 657 (31) | 244 (41) |

Splits whose plane is offered by prisms of two kinds are counted under each kind.

## Variants: runtime portals / interior cells / slivers, and C18

| Variant | occlusion-test | hallway | campaign-test | arena | street | C18 fixture mean / worst |
|---|---|---|---|---|---|---|
| none | 241 / 107 / 48 | 2,368 / 1,516 / 11 | 288 / 129 / 66 | 608 / 395 / 0 | 3,756 / 1,607 / 1,172 | 28.46 / 87.77% |
| all (HEAD) | 430 / 174 / 76 | 5,357 / 3,268 / 28 | 402 / 165 / 75 | 1,128 / 683 / 0 | 7,667 / 3,144 / 1,930 | 0 / 0 |
| free + far | 402 / 165 / 73 | 4,236 / 2,452 / 11 | 332 / 138 / 66 | 1,106 / 662 / 0 | 7,355 / 2,991 / 1,789 | 0 / 0 |
| free, no far | 358 / 148 / 68 | 2,936 / 1,865 / 11 | 306 / 136 / 66 | 883 / 558 / 0 | 6,849 / 2,822 / 1,765 | 0 / 0 |
| brush only | 384 / 157 / 70 | 5,355 / 3,266 / 28 | 307 / 137 / 67 | 1,128 / 683 / 0 | 6,453 / 2,642 / 1,675 | 0 / 0 |
| section only | 274 / 118 / 52 | 2,376 / 1,525 / 11 | 383 / 157 / 74 | 608 / 395 / 0 | 4,440 / 1,912 / 1,338 | 28.46 / 87.77% |
| abutting only | 262 / 110 / 49 | 3,084 / 2,137 / 28 | 305 / 136 / 67 | 668 / 441 / 0 | 4,094 / 1,794 / 1,320 | 28.46 / 87.77% |
| far only | 278 / 121 / 51 | 3,297 / 1,946 / 11 | 314 / 131 / 66 | 783 / 475 / 0 | 4,037 / 1,702 / 1,189 | 28.46 / 87.77% |
| brush free | 358 / 148 / 68 | 2,936 / 1,864 / 11 | 306 / 136 / 66 | 883 / 558 / 0 | 5,989 / 2,457 / 1,550 | 0 / 0 |
| brush free, wall faces | 270 / 119 / 55 | 2,936 / 1,864 / 11 | 306 / 136 / 66 | 844 / 537 / 0 | 4,167 / 1,763 / 1,241 | 0 / 0 |
| **brush free, wall faces, vertical planes (bfwv)** | **270 / 119 / 55** | **2,926 / 1,859 / 11** | **306 / 136 / 66** | **844 / 537 / 0** | **3,885 / 1,648 / 1,196** | **0 / 0** |
| bfwv, any probe open | 270 / 119 / 55 | 3,598 / 2,422 / 28 | 306 / 136 / 66 | 868 / 552 / 0 | 4,020 / 1,738 / 1,283 | 0 / 0 |
| brush free, all probes open | 358 / 148 / 68 | 2,858 / 1,818 / 11 | 306 / 136 / 66 | 836 / 528 / 0 | 5,762 / 2,305 / 1,401 | 0 / 0 |
| free, wall faces, vertical planes (with sections) | 270 / 119 / 55 | 2,926 / 1,859 / 11 | 306 / 136 / 66 | 844 / 537 / 0 | 4,138 / 1,791 / 1,299 | 0 / 0 |

Before-build portals were 693, 4,722, 524, 1,333 and 6,288. Faces are in `ablation-a.out` and `ablation-b.out`, for example street 10,755 none, 11,415 all and 10,848 bfwv.

Some reads from the table:
- **C18 needs only the free side planes of brush prisms.** Every variant without them leaks as much as no prisms: section only, abutting only and far only.
- **The kinds interact.** On the hallway, the single-kind deltas (free +568, abutting +716, far +929) sum to less than all prisms (+2,989).

## Street and rooftop walks

| | noprism (9k) | **bfwv** | free (owner's candidate) | all (9k cap26) |
|---|---|---|---|---|
| Runtime portals | 3,756 | 3,885 | 6,849 | 7,667 |
| Street trips (also before / new) | 19 (19 / 0) | **29 (29 / 0)** | 245 (217 / 28) | 312 (245 / 67) |
| Rooftop trips (also before / new) | 27 (27 / 0) | **26 (26 / 0)** | 145 (141 / 4) | 168 (153 / 15) |
| Uncapped street mean / max | 1,259.5 / 6,045 | 1,292.5 / 6,467 (`g13.30.0@0`) | 1,989.3 / 10,713 | 2,150.6 / 11,587 |
| Uncapped rooftop mean / max | 1,939.7 / 6,466 | 1,967.2 / 6,608 (`roof:B4.s2.c@180/-15`) | 3,176.1 / 11,053 | 3,418.5 / 11,955 |
| 2× rule | 12,932 | **13,216** | 22,106 | 23,910 |
| New drawn misses | 0 | 0 | 0 | 0 |

The step cap still trips under bfwv, so M1 fails as it did without prisms, but every trip also tripped before. The occlusion-test C18 map sweep on the bfwv bake is 0.00% (70 poses × 11,538).

## Mechanism

- **Rank after occluder planes means per-fragment splits.** A prism plane is offered only in a region that no occluder plane spans, after occluder and section planes have cut the space into fragments. The same plane is then chosen once per fragment it spans, across that fragment's full width. On the street, deck brush 1166 (44 × 8 m, top at 6.0 m) has an up-prism 24 m tall with four free sides, which was chosen at **217** tier nodes, and its down-prism at 48. That is 265 of the street's 1,494 prism splits. Slab prisms are the street's largest cost: brush free 5,989 against 4,167 with wall faces only.
- **Abutting is local, but planes are infinite.** On the hallway, 956 edges are 33–50% open. Brushes 29 and 126 are 13 m tall walls 32.5 m long that face each other, and their top and bottom planes lie on level floors that have openings. Their prisms are truncated at 65 m and won 60 and 51 splits. Their far planes cross open space by definition: far only adds +929 hallway portals.
- **Sections.** Facade section #85 (85 members, 52.8 m truncated) won 169 splits. Sections cost +684 on the street and +95 on campaign-test, where every section edge abuts, and the fixture does not use them.
- **Useless prisms.** occlusion-test's yellow-room roof b113 sweeps both ways (22 splits), but the room is closed. Brush 102's "abutting" end plane lies in the exterior void past the hall's far wall.
- **Overlap.** Hallway prisms overlap heavily: 3,230 pairs, up to 77 per prism.

## Not tested by C18 (inferred risk of bfwv)

- **Wall tops.** bfwv drops wall-top planes, so it relies on the top plane splitting before the back face, as it does on both C18 cases.
- **Slabs and sections.** Decks, roofs and facade sections no longer bound their shadows.
- **Far planes.** Deep shadows run to the open space's end.

## Files

- `ablation-a.out`, `ablation-b.out`: per-map rows, split histograms, per-prism top 10, and the portal-plane cross-check. Side planes almost always coincide with a brush plane, so that cross-check cannot separate prism from brush planes.
- `c18-fixture.out`: the fixture result per variant.
- `{bfwv,free}/stress-street/`: sweep, rooftop sweep, structure, uncapped top 40, bake extract.
- `bfwv/occlusion-test/c18-sweep.out`: the occlusion-test map sweep on the bfwv bake.
- `run9l.sh`: the bake and walk driver.
- `ablation-scratch.patch`: the scratch hooks and the `#[ignore]` tests `scratch_9l_ablation` and `scratch_9l_c18_per_prism_variant`.
