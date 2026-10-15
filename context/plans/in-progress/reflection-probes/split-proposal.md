# Two-trigger probe split — contract wording

Status: draft for owner approval. It applies the 2026-10-15 "two-trigger probe split adopted" entry in `plan.md` to `index.md`. Read at `382878754`. Owner chose both §1 recommendations on 2026-10-15: cut faces, and the hidden-hit parallax test.

## 1. Two points to settle before the wording lands

**1a. The split unit.** Bindings are per id-52 face, so as recorded the split can only move whole faces. The dock fixture cannot pass that way. A post standing in the lake clips the lake face with successive plane cuts. Each cut runs to the face's edge, so the pieces are strips crossing the whole lake, and none of them is the water under the deck. A deck with no posts in the water leaves the lake as one face.

- **Recommended: the split may cut a face.** A face that straddles a trigger is cut by a vertical plane, the way a push brush already cuts one (T-junctions close, pieces sum to the face, no piece under 1 mm²). Cutting halves the face's horizontal bounds along the longer axis, repeating down to a minimum piece size (about 1 m, tuned). Id 52 then carries the cuts, so its encode moves after the probe stage. A map with no split keeps identical id-52 bytes, as a map with no push cut does today.
- Alternative: whole faces only. The L-shaped pool and room B's pillar work, because the pillar clips the canal face. The dock fixture is replaced by one whose posts happen to isolate the under-deck water.

**1b. The parallax measure.** As recorded, the measure is |water − capture point| ÷ distance to the nearest geometry the water reflects, with a budget of about 2 capture texels. That is about 0.056 as a ratio. It fires on almost every pool. Water 2 m from the point beside a coping 0.3 m away scores about 6.7. Water 0.5 m from the point under a 3 m ceiling scores about 0.17. Distance correction already resolves geometry the capture point can see, to within 0.6° on the crate fixture. The false reflections come only from geometry the capture point **cannot** see.

- **Recommended: count only the parallax the search cannot remove.** From each water sample, trace a fixed fan of mirror rays against the BVH: a few eye heights and azimuths, the same fan for every probe. Each ray's true hit H fails when the distance tile places H behind a surface the capture point saw, by more than the budget in capture texels (start 2). This is the shader's own capture-shadow test, applied at H with that margin. Misses fail nothing. Water fails when its failing share passes a threshold, tuned against falsely-reflected-water share and probe count.
- Alternative: keep the recorded ratio. Its budget then needs a much larger starting value, and it will still split near every wall.

The wording below assumes both recommendations. Under the alternatives, drop the face-cut sentences, or restore the ratio sentence.

## 2. Decisions

**New Decision, after "Probes are compiler-derived by default…":**

> - **A probe splits where its capture point fails its water** (owner, amendment 2026-10-15). After capture, two triggers test each probe's bound water against its own distance tile. *Sight:* the capture point sees under half of its bound water area, by the capture-shadow test the fluid shader runs. *Parallax:* some bound water reflects geometry the capture point does not see. A mirror ray from the water fails when its true hit lies behind a surface the capture saw, by more than a budget in capture texels. A capture miss fails nothing, so open sky never splits a probe. When either trigger fires, the water that fails leaves as a new probe. The new probe shares the parent's projection box. Its capture point sits 2 mm above that water's horizontal area centroid, moved onto the water by the nearest-point rule. Both probes recapture and retest, up to a fixed round limit. The unit is the face. A face that straddles a trigger is cut by vertical planes, as a push brush cuts one, down to a minimum piece size, and id 52 carries the cuts. An authored box splits the same way: it keeps its projection box and its claim, and the split only adds capture points. A probe whose capture point already sees its water within budget never moves. Splitting is deterministic across worker counts, and the stage key covers it. Rival rejected: warning and leaving the hidden water dark, which leaves every dock and pillar to an author's box, and a box cannot fix them anyway.

**Drops and cap.** Old: "…then derived probes for unclaimed faces and their drops, then the cap. A dropped box's faces bind as if it were absent; a dropped derived probe's faces bind nothing. A per-level cap fits the probe array's portable layer limit: authored probes past it refuse the build, naming the cap; derived probes past it are dropped, largest bound surface area kept first, with one warning naming the count."
New: "…then derived probes for unclaimed faces and their drops, then splitting, then the cap. A dropped box's faces bind as if it were absent; a dropped derived probe's faces bind nothing; a split probe whose capture point would drop is never made, and its water stays with its parent. A per-level cap fits the probe array's portable layer limit: authored probes past it refuse the build, naming the cap; derived and split probes past it are dropped, largest bound surface area kept first, with one warning naming the count, and a dropped split probe's water returns to the probe it split from. Splitting never refuses the build."

**Capture point.** Append: "A split probe's point is the horizontal area centroid of the water it took, 2 mm above its plane, by the same nearest-point rule."

**Storage.** Append to the distance-tile sentence: "0 is reserved for a miss and no finite hit stores it, so a later sky term can fill exactly the directions the capture saw nothing."

**Capture shadow.** Old: "The compiler warns, naming the probe, when the capture point sees less than half of a probe's bound water area."
New: "Splitting makes capture shadows rare. The compiler warns, naming the probe, when a capture point still sees less than half of its bound water after splitting, because the round limit, the minimum piece or the cap stopped it."

## 3. Wire format

Old: "authored probes in map entity order then derived probes in fluid entity order"
New: "authored probes in map entity order, then derived probes in fluid entity order, then split probes in the order splitting makes them (by round, then parent order). A split probe's source entity index is its parent's."

No format change: section 54 stays at internal version 2.

## 4. Acceptance

**Replace** "Capture-visibility warning on an L-shaped pool whose bound area is mostly hidden from its capture point." with:

- [ ] Split by sight: an L-shaped pool whose bound area is mostly hidden from its capture point splits, with no warning, into probes that share the parent's box and whose capture points each see at least half of their water. With splitting held to zero rounds, it warns as before.
- [ ] Room B: the canal water behind the pillar, as seen from the authored capture point, binds a split probe and reflects with no capture shadow. The authored box keeps its projection box and claim.
- [ ] Dock fixture (a lake, a dock on posts, water under the deck, a near shore): the water under the deck binds a probe whose capture point is under the deck. The falsely-reflected-water share falls below the tuned bound, at a probe count reported beside it. The open lake under open sky does not split.
- [ ] A probe whose capture point sees its water within budget keeps its box, point and binding. The test map's pool probe is unchanged by the split.
- [ ] Face cuts: a face straddling a trigger is cut by vertical planes into pieces that sum to its area, with T-junctions closed and no piece under the minimum size. A map with no split emits id-52 bytes identical to an unsplit build.
- [ ] Cap: split probes count toward the cap and drop past it, largest bound area kept first, in the one cap warning. Their water returns to the probe each split from. Splitting never refuses the build.
- [ ] Two `--no-cache` builds of the dock fixture, at `--jobs 1` and at `--jobs 4` or more, emit byte-identical sections 52 and 54.
- [ ] A finite capture hit at any in-level distance stores a non-zero distance, and a miss stores 0. The parallax trigger treats a miss as passing.

**Manual row 43**, append: "Probe count and cold stage time with splitting, on e24 and the test map."

## 5. Tasks (plan.md, after the wording lands)

| # | Task | Depends on |
|---|---|---|
| 12a | Split core: sight and parallax triggers (mirror-ray fan, hidden-hit test against the tile), split probe placement, recapture rounds, cap and drops, warning moved after splitting; determinism and cache key; L-pool and room B tests | — |
| 12b | Face cuts: vertical-plane cuts reusing the push-cut machinery, id-52 encode moved after the probe stage, unsplit byte identity | 12a |
| 12c | Dock fixture map plus tuning (budget, threshold, round limit, minimum piece) by falsely-reflected-water share against probe count; sky substrate test | 12a, 12b |
| 12d | Re-capture the test map and e24: images for the owner, and timing run alone with kills by PID | 12c |
| 12e | Reassess the capture-shadow fade's five constants and gate once shadows are rare, and simplify if it is largely moot | 12d |
