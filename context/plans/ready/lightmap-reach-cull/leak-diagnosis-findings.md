# Shadow-ray leak diagnosis — findings

> Copied from branch `spike/lightmap-flood-cull-sizing` at 8b2eb15a9 (`context/plans/drafts/lightmap-stage-sizing/leak-diagnosis-findings.md`). The patch, raw report directories and spike-only source files it names stay on that branch; read them with `git show spike/lightmap-flood-cull-sizing:<path>`.

Diagnosis spike, 2026-10-31, Intel Mac. Follows `flood-cull-findings.md` §Doubts about (B)'s exactness. No production change landed. Instrumentation is on branch `spike/lightmap-flood-cull-sizing`: `POSTRETRO_LM_LEAK`, `POSTRETRO_LM_EDGES` and `POSTRETRO_FACE_SPLIT_EPS` (see §Instrumentation). Raw reports are in `leak-diagnosis-raw/`.

## Verdict

**Root cause: compiler geometry, two kinds, plus a small floating-point residue. The hit test is sound.**

1. **Face extraction drops exposed slivers narrower than 10 cm (dominant).** `face_extract` splits brush sides against BSP planes with `SPLIT_EPSILON = 0.1` m. Its comment says it matches the BSP builder, but that 0.1 m tolerance is now test-only. Production BSP classifies exact polytopes with a 1 mm tolerance. A side whose visible part lies within 10 cm of a splitter is routed whole to one side. When that side is solid, the visible sliver is dropped, and the BSP keeps an open leaf with no face. This causes every lit and every large partial leak on movement-feel. It also lets visible texels at inside corners take light through metres of wall: 7 traced on the mini hallway. And 446 of the hallway's 557 open spans are real holes of this kind. **Fix: one constant, `1e-3` m, matching `REGION_CLASSIFICATION_EPSILON`.** At 1 mm, the hallway has 0 holes (557 → 26 spans, all parity artifacts), the mini hallway has 0 (232 → 4), and movement-feel has 0 large holes.
2. **Crease micro-cracks (residual).** Two brushes meet at a sloped crease. Each face's crease edge comes from its own brush's planes, and the two edges miss each other by 1–250 µm. That is below the 1 mm split tolerance and below the watertightness check's 0.1 mm weld. A ray from inside solid slips through about once per 32-sample texel. It occurs only on non-axial geometry, and it survives the epsilon fix: 78 texels on movement-feel, all at 1/32.
3. **Floating-point grazing (H3).** A ray crossing within 1 µm of a shared edge misses both triangles. Measured: 5–6 deep texels, and open-origin rays that clip a convex corner by under 1 mm (33 on movement-feel, 350 on the mini hallway). One mini-hallway ray lies exactly in a 45° wall's plane: the texel and the light centre both sit on it. The ray runs 5 m along the face, tangent to it, and the BSP rounds it into solid.

**Implication for the cull.** The flood's precondition fails today, and not only for buried texels. On the mini hallway, a texel within 1 mm of a wall sends its ray into the wall inside the ray's skipped first millimetre. The ray crosses 3–6 m of solid and leaves through a cause-1 hole into a leaf up to 4 portal hops away. That is a ray between two open leaves through solid, the case that would make a flood skip change bytes.

Cause 1 is a bounded compiler bug. At 1 mm, every such pass is gone on both small maps. The open-to-open rays that remain cross ≤ 1 mm of solid at a convex corner, apart from the one tangent ray in a wall's plane. Cause 2 can still leak a ray that **starts inside solid** (a buried texel), or one that enters solid inside its skip next to a crease crack (not observed).

**Recommendation:**

1. Land the epsilon fix first, as its own change.
2. Use the relaxed chart rule, but trace every texel whose padded footprint slab touches a closed leaf.
3. Keep a re-trace verify mode.

Details in §Recommendation.

## Method

- Binary: `cargo build --release -p postretro-level-compiler` on the spike branch. Bakes: `prl-build <map> --release --no-tui`, cold, one at a time, under `caffeinate -dims`.
- **Ray trace** (`POSTRETRO_LM_LEAK=<path>`, needs `POSTRETRO_LM_SIZING`): every clear shadow ray the lightmap walk traces is walked through the full compiler BSP tree. Every crossing between an open leaf and a closed (solid or exterior) leaf past the ray's skipped first `RAY_EPSILON` gets a record: the distance to the nearest emitted triangle, the nearest pre-cull extracted face, and the nearest watertightness open span. The record also holds the production f32 Möller–Trumbore verdict (which barycentric failed) and an f64 recompute. For deep-closed texels, the side variant's 188, every traced ray is logged with its full leaf path.
- **Span classification** (`POSTRETRO_LM_EDGES=<path>`, `POSTRETRO_LM_EDGES_EXIT=1` stops after the exterior flood, under 3 s even for the hallway): each watertightness open span is probed with rings of 32 points at 0.5 mm, 5 mm and 2 cm at three points along the span. Wherever adjacent ring points flip between open and closed, the chord between them is tested exactly (f64, zero tolerance) against every extracted face. A chord no face intersects is a **hole**. Otherwise the span is **covered**: a parity artifact such as overlap or double cover, not a gap. The chord is then continued 2 m into the solid, to find whether open space lies past the hole and how many full-tree portal hops away.
- **Experiment:** `POSTRETRO_FACE_SPLIT_EPS` overrides the split epsilon. Tested 0.1 (default), 1e-3, 1e-4, 1e-5 and 1e-6.
- **Bytes:** a movement-feel bake with the counters on is `cmp`-identical to a bake with them off.
- **Reproduction:** the trace finds exactly the side variant's 188 deep texels with visibility > 0 (28 lit early-outs, 160 partial). Every one has at least one clear ray that crosses a closed-to-open boundary past its skip.

## Cause table (movement-feel, the 188 deep-closed texels with visibility > 0)

Each texel is classified by its worst clear ray: the distance from that ray's first closed-to-open crossing past the skip to the nearest emitted triangle.

| Cause | Texels | Stored values | Evidence |
|---|---|---|---|
| Cause 1, sliver dropped by the 0.1 m split epsilon (H2 + H4) | **108** | 28 lit (9/9 probes), 80 partial 2/32–30/32 | No triangle, and no pre-cull extracted face, within 1–20 mm of the crossing. Every one lies within 5 cm of a 2 cm-wide hole span. All gone at epsilon ≤ 1 mm. |
| Cause 2, crease micro-crack (H2) | **74** | 73 at 1/32, 1 at 2/32 | The crossing lies 1 µm–1 mm from two non-coplanar triangles of different brushes. The f32 test misses both on one barycentric, by 1e-6 to 5e-5. The crease endpoints disagree by 7 µm–0.25 mm. Survives every epsilon. |
| H3, f32 edge grazing | **6** | 1/32 | The crossing lies within 1 µm of a triangle edge. |
| H1, one-sided test | 0 | | `ray_triangle_hit` takes `det.abs()`: two-sided. No traced miss was a back face. |
| H5, measurement artifact | 0 | | Every traced ray starts in a solid (1,425) or exterior (7) leaf, and no first crossing falls inside the skip. The deep flag is sound. |

The mini hallway has no deep leaks: 1.02 M deep texels, all dark. Its only non-axial geometry is 45° walls, and it shows no crease cracks at 1e-3.

Ray-level, the first crossing past the skip on all 1,432 clear deep rays: 6 under 1 µm from a triangle, 32 at 1–10 µm, 52 at 10–100 µm, 67 at 0.1–1 mm, 1,007 at 1–20 mm, 268 with nothing within 20 mm.

Every clear ray on the map that starts in a closed leaf (315,717 of 79.2 M clear rays): 310,893 cross into open space inside their skipped first 1 mm. Those are texels at inside corners, legitimate by the sampler's design. The other 4,824 cross past the skip: 1,404 within 1 mm of a triangle (micro-cracks) and 3,420 with no triangle within 1 mm (slivers). The leak sites cluster at two corners: where the sloped brushes 27 and 28 meet the z = 35.56 wall under the y = 4.4704 ceiling (x ≈ 14.6 and x ≈ −27.6). There the sloped face's corner vertex stops 8.1 cm short of the wall, inside the 10 cm epsilon, and a 1.15 m × 8 cm triangle of that face is missing.

### Cause 1 in detail

Brush-side projection walks each side through the tree and calls `split_polygon(…, 0.1)`. A vertex within 0.1 m of the plane counts as on the plane and goes to both sides, and no intersection point is cut. So a side that crosses a splitter by under 10 cm is not cut there. It goes whole to the side holding its other vertices. When that side is solid, its exposed sliver beyond the splitter goes too. The BSP itself is exact (`region_polytope`, `REGION_CLASSIFICATION_EPSILON = 1e-3`). It marks the sliver's region open, and nothing bounds it. The BSP builder's 0.1 m `PLANE_EPSILON` is `#[cfg(test)]` only, so the "matches the BSP builder" comment is stale.

Typical shapes: on the hallway, a box stacked on a box set back 1 inch (2.54 cm) loses the lower box's exposed top strip, for example brushes 1535 and 1536. On movement-feel, a sloped brush's corner triangle within 8 cm of a wall is lost.

| Split epsilon | movement-feel spans (hole / big hole) | movement-feel deep leaks (lit) | hallway spans (holes) | mini spans (holes) | mini open-origin rays through ≥ 10 mm of solid |
|---|---|---|---|---|---|
| 0.1 (today) | 147 (22 / 9) | 188 (28) | 557 (446) | 232 (177) | 16 |
| 1e-3 | 146 (15 / 0) | 78 (0) | 26 (0) | 4 (0) | 1 (the tangent ray) |
| 1e-4 | 146 (15 / 0) | — | 26 (0) | — | — |
| 1e-5 | 134 (15 / 0) | 90 (0) | 26 (0) | — | — |
| 1e-6 | 140 (11 / 0) | — | 22 (0) | — | — |

All remaining holes at 1e-3 and below are crease micro-cracks (hole only at the 0.5 mm ring).

### Cause 2 in detail

Example: light 12, texel at (7.254, 15.639, 15.091) on chart 249, buried in brush 39. Its clear ray leaves solid at (7.904, 14.189, 15.555). That point lies between face 246 (brush 39) and face 244 (brush 33), 4.4 µm and 11.8 µm away. Production f32: `1−u−v = −1.5e-6` on one triangle and `v = −1.2e-6` on the other. The two faces' crease endpoints are (13.308239, 14.655983, 0.0) and (13.308233, 14.655979, −1.1e-7), 7 µm apart, then (5.930468, 14.018328, 21.234406) and (5.930455, 14.018326, 21.234392), 19 µm apart. At the default epsilon the worst creases (brushes 30/32 and 30/34) miss by up to 0.25 mm.

The edges are not welded: each comes from its own brush's planes, and split tolerance absorbs anything under it. A tighter epsilon does not close the cracks. At 1e-5 it emits 25–300 µm-tall sliver triangles 3–23 m long along every crease, and the cracks stay (90 texels). It also costs speed: shaft-clear texels fall 67.6 M → 53.3 M, and the instrumented Lightmap Bake grows 71 s → 105 s, because the slivers touch shaft hulls. At 1e-3: shaft-clear 65.1 M, instrumented Lightmap Bake 77 s.

### H3 residue

Rays crossing within 1 µm of an edge shared by two triangles miss both, a property of the f32 barycentric test at map coordinates of 10–40 m, where an f32 ulp is 1–4 µm. Measured on movement-feel: 6 deep texels at 1/32, and 33 open-origin rays that clip a convex corner, crossing at most 60 µm of solid (9 under 1 µm, 24 at 1–100 µm). Their entry points lie within 1 µm of a triangle (13–14 rays) or 10–100 µm from one (11).

## Rays between open leaves

This is the case that decides a flood skip's bytes.

Movement-feel, all 78.9 M clear rays that start in an open leaf, by the longest stretch of closed space between two open stretches:

| Split epsilon | < 1 µm | 1–100 µm | 0.1–1 mm | 1–10 mm | ≥ 10 mm |
|---|---|---|---|---|---|
| 0.1 | 9 | 24 | 0 | 0 | **1** |
| 1e-3 | 9 | 24 | 0 | 0 | 0 |

- The 33 short runs are corner grazes. The ray passes within 60 µm of a convex edge, and both sides are open leaves around that edge. They connect leaves an offset ray would connect through open space. They cannot strand a receiver behind a wall.
- **The ≥ 10 mm ray** is a two-defect composite. The texel sits on the z = 35.56 wall at the ceiling corner (open leaf 128). Its ray enters brush 27 0.47 mm out, inside its own skipped first millimetre, so the entry face is not tested by design. It then runs 11.4 cm through solid and exits through the cause-1 sliver hole into leaf 528. Leaves 128 and 528 are adjacent slabs, so a flood would hold both. It still shows the dangerous pattern: one skip-entry and one hole are enough to cross a wall. It is gone at epsilon 1e-3.

Mini hallway (`stress-warren-hallway-inspection-mini.map`, 261 M clear open-origin rays; 316 M at 1e-3):

| Split epsilon | < 1 µm | 1–100 µm | 0.1–1 mm | 1–10 mm | ≥ 10 mm |
|---|---|---|---|---|---|
| 0.1 | 74 | 269 | 5 | 0 | **16** |
| 1e-3 | 73 | 272 | 5 | 0 | 1 |

- Runs under 1 mm are convex-corner grazes. Entry and exit both lie within 3 µm of a triangle (`open_wall_entry_geom`).
- 8 of the 16 long runs fit under the example cap. 7 of the 8 are **skip-entry plus a cause-1 hole**. A floor or wall texel sits 0.3–0.8 mm from a wall, and its ray toward a light beyond that wall enters it inside the skipped first millimetre. The ray then crosses 3.4–6.5 m of solid, often into another storey, and leaves where no triangle lies within 0.2–32 mm, each exit on a 2 cm hole span. Origin and exit leaves are 1–4 full-tree portal hops apart (`leak-diagnosis-raw/mini.eps0.1.wallpass-hops.txt`). All 7 are gone at 1e-3.
- The 8th, which survives at 1e-3, is the tangent ray. The texel at (−46.73, 19.51, −37.80) and its target (−63.40, 30.28, −21.13) both satisfy x + z = −84.531, the plane of a 45° wall. The ray runs 5.06 m inside that plane, `ray_triangle_hit` rejects it as parallel, and the BSP rounds it into solid. Its two leaves are 9 hops apart. The open side of the face runs alongside the whole ray, so a ball flood should reach both, but nothing proves it.

- Hallway, from the span probe (default epsilon): of 446 holes, 370 open into solid with no open leaf within 2 m behind them. 72 have open space behind within 2 portal hops (49 behind solid under 10 cm thick), and 4 at 3 hops. No probed chord reaches a leaf more than 3 portal hops away. The prior spike's relaxed chart rule skipped 0 lit texels on the hallway with these holes present.

## Open-edge link

**movement-feel, 147 spans (default epsilon):** 125 covered (parity artifacts, no gap at 0.5 mm–2 cm), 9 big holes (cause 1, all at the two sloped-brush corners), 13 crack spans (cause 2, long sloped creases). Of the deep leak crossings:

Crossings by distance to the nearest span:

| Leak class | < 1 mm | 1 mm–1 cm | 1–5 cm | ≥ 5 cm |
|---|---|---|---|---|
| Cause 1 (lit, partial > 1/32) | 75 | 617 | 659 | 0 |
| Cause 2 / H3 (1/32) | 51 (29 on crack spans, 22 on "covered" spans) | 0 | 1 | **27** |

Every cause-1 crossing's nearest span is a big-hole span. A third of the 1/32 leaks sit on no flagged span. The watertightness check welds lines at 0.1 mm (`ANCHOR_QUANTUM`), so a crack narrower than that cancels in its parity count. Where a crack varies along a crease, the three probe points can miss it, and those spans read "covered".

**hallway, 557 spans:** 446 real holes (366 open at the 2 cm ring, 60 at 5 mm, 20 only at 0.5 mm), totalling 306 m of edge, all cause 1 (0 left at epsilon ≤ 1 mm). 85 covered on an open/solid boundary and 26 covered inside solid are harmless. The map is axis-aligned, so it has no cause-2 creases. That explains its zero deep leaks despite its holes.

## Visible impact

- **Cause 1 is visible today, without the cull.** Each hole is a missing strip of world surface, 1–10 cm wide: 306 m of edge on the hallway and two 1.15 m sliver triangles on movement-feel. Brush interiors have no faces, so the strip shows whatever lies behind the solid. Lightmaps: a visible texel at an inside corner can take light through solid. The 11 cm ray on movement-feel's ceiling corner and the seven 3–6 m rays on the mini hallway all start from open-leaf texels, so this is visible bleed today. It is rare: 16 rays in 261 M on the mini, and a texel's stored value moves only by its share of the 9 or 32 rays. The same BVH feeds the SH, direct-SH and scatter bakes, and their probe rays can leak through these holes too (not measured).
- **Cause 2 and H3 are invisible in practice.** Every leaking texel is buried: its whole padded footprint slab (radius × 1.05 + 2 mm) touches no open leaf. Bilinear sampling of an in-polygon point reads texels within 1 texel of it, so none of these is sampled directly. The one indirect path is BC6H block encoding. A 1/32 texel in a 4×4 block beside visible dark texels shifts that block's endpoints. The effect is a quantization change, not light. A screenshot was not taken.

## Fix sizing

| Cause | Fix | Where | Size | PRL bytes | Precondition checkable at compile time? |
|---|---|---|---|---|---|
| 1, sliver drop | `SPLIT_EPSILON` 0.1 → 1e-3 (= `REGION_CLASSIFICATION_EPSILON`); fix the stale comment; fixtures: a box set back 2.54 cm on another emits its exposed strip; a sloped corner 8 cm from a wall emits its triangle | `partition/face_extract.rs` | ~1 line + 2–3 tests; geometry/stage cache epochs bump | **Change on every map with such slivers**: geometry, BVH, atlas, every bake downstream. The hallway and the mini change; movement-feel gains 3 faces and 13 triangles. | Yes. Promote the span probe (exact chord test at flips) into the watertightness check: report **holes**, not parity spans. At 1e-3 it reads 0 holes on the hallway and the mini, and 15 crack spans on movement-feel. Cheap: under 3 s on the hallway. |
| 1, perf side effect | none needed; measure on the hallway | | | Restored faces are real, and the shaft cull clears less: shaft-clear texels −3.7% on movement-feel (67.6 M → 65.1 M), −1.8% on the mini (348.1 M → 342.0 M). Instrumented Lightmap Bake +8% and +5% (441 → 463 s on the mini); the trace's per-ray cost inflates both. | |
| 2, crease micro-cracks | (a) tracer: a hit test with an absolute in-plane tolerance of ~0.1 mm, carried into the shaft cull's margin proof; or (b) compiler: snap crease edges of different brushes onto the exact BSP crease line, or weld vertices within 1 mm across faces | (a) `lightmap_bake::ray_triangle_hit` and its shaft-cull mirror; (b) `face_extract` post-pass | (a) small code, but it reopens the shaft-cull exactness proof (`plans/done/lightmap-exact-skips`); (b) medium, geometry-wide | Both change bytes slightly: buried texels' 1/32 values go to 0, and grazing rays block. | Only with a finer check. The parity check's 0.1 mm weld hides them. An exact check would test chords at 0.5 mm along every crease, every few cm. |
| H3 | None proportionate. A watertight ray–triangle test (Woop et al. 2013) closes shared-vertex edges, but not cause-2 cracks, whose vertices differ. | | | | No |

A "closed open/solid boundary" precondition is achievable and checkable at the scale that matters, ≥ 0.5 mm, once cause 1 is fixed. It is not achievable at the micron scale without (a) or (b).

## Recommendation for the cull's exactness rule

1. **Fix cause 1 first, as its own small change.** It is a real compiler bug with visible holes and visible bleed, independent of the cull, and it moves bytes once. The cull's byte-identity checks then run against the fixed baseline. Add the hole-level watertightness report at the same time.
2. **Rule: relaxed chart test, then a per-texel slab test inside skipped pairs.** Skip a pair when every open leaf its padded chart slab touches lies outside the ball flood. Within a skipped pair, write 0 without a ray only for a texel whose own padded footprint slab (the side variant's: radius × 1.05 + 2 mm, lifts 0.25, 1 and 2 mm) touches **only open leaves**. Trace every other texel as today. Why:
   - **Buried texels.** Cause 2 and H3 reach them, and they are traced.
   - **Skip-entry texels.** A texel whose ray can enter solid inside its skipped first millimetre sits within the slab's reach of a closed leaf, so it is traced. This is the path the mini hallway's 3–6 m passes took.
   - **Texels left to skip.** Every ray starts more than a footprint from any closed leaf. To reach a flooded leaf it must cross a face at entry and at exit. That takes two aligned defects (crack or hole on both sides of a wall), or a ray tangent to a face plane. After the cause-1 fix, no hole ≥ 0.5 mm remains on any measured map.
   - **Cost (estimate, not measured).** It traces the texels within about one footprint of a wall, plus every overhang texel. On the hallway, about a 1–2 texel border of each skipped chart, roughly 4% of the 1,733 M skipped dark early-outs: about 100 CPU-s against about 2,700 saved. The buried share alone is at most 1.12 M. The brief should count it with one more sizing counter.
   - **Cheaper variant.** Trace only texels whose lifted centre is closed. It keeps buried texels exact but leaves the skip-entry path, which then depends on the cause-1 fix holding everywhere and on cause 2 never sitting behind a corner texel. Not recommended.
   - **Strict rule.** It keeps only 4% of the win (`flood-cull-findings.md` §4) and buys nothing the per-texel slab test does not.
3. **Keep a verify mode**, as the shaft cull does. Under `cfg(test)` and on a diagnostic switch, re-trace every skipped texel and assert visibility 0. Run it on the three measured maps and on any corpus map with sloped creases or 45° walls. It guards the residual risks: two aligned cracks, and a tangent ray in a wall plane through the light centre whose open side does not run continuously beside it.
4. **State the exactness standard** as the shaft cull's does: byte-identical to the unculled walk, with named gaps: a ray that crosses solid only through sub-millimetre cracks at both entry and exit, and a ray lying in a face's plane. The verify mode is the check.
5. **If the verify mode ever fires,** close cause 2 with option (b), compiler-side edge snapping. Option (a) changes the hit test both culls' proofs rest on. The shaft cull is exact against the triangles themselves, so holes never affected it. The flood is the first cull that reasons from BSP solidity, which is why it needs the geometry to match the BSP.

## Instrumentation

All diagnostic hooks are off unless their env var is set. Bytes with them on are `cmp`-identical to off (checked on movement-feel).

- `crates/level-compiler/src/lm_leak_trace.rs`: per-ray BSP walk, crossing classification, deep-texel ray logs, open-span classifier. Hooks: the shadow-ray closure and per-texel bracket in `lightmap_layer/chart_walk.rs`, `install` and the report in `pipeline.rs`, the span classifier after the exterior flood.
- `partition/manifold.rs`: `WatertightReport.all_spans` keeps every open span's endpoints.
- `partition/face_extract.rs`: `POSTRETRO_FACE_SPLIT_EPS` experiment override. Unset, the epsilon is 0.1 and bytes are unchanged.
- `lm_flood_sizing::leak_context`: deep flag and light/chart context for the trace.

Runs: `POSTRETRO_LM_SIZING=<a> POSTRETRO_LM_LEAK=<b> prl-build content/dev/maps/movement-feel.map --release …` (~80 s), and `POSTRETRO_LM_EDGES=<c> POSTRETRO_LM_EDGES_EXIT=1 prl-build <map> …` (span classification only, ≤ 3 s).
