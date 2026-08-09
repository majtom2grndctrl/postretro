# Research — animated-weight-map packer collapse

Derivation notes for `index.md`. Everything here was verified against source or
reproduced this session; nothing is design intent.

## Reproduction

Generated a 4×4×2 warren (`tools/gen_stress_map.py --grid 4 4 2 --lights static
--lights-per-room 3 --animated-frac 1.0 --seed 7`), then marked all 58
non-animated static lights KVP-animated (`brightness_curve` + `period_ms`
appended to the entity blocks — 64 animated lights total, bypassing the
generator's `ANIMATED_LIGHT_CAP = 6`). Baked with
`prl-build … --sh-probe-spacing 10.0 --lightmap-density 0.5 --no-cache`:

```
animated-light chunks 5876 and 5877 on face 337 produced overlapping atlas
rects under center-based half-open ownership (1x1+63+239 vs 1x1+63+239);
chunk UVs [[0.0, 0.0]..[0.8128, 0.7112]] vs [[0.0, 0.7112]..[0.8128, 1.4224]]
```

Exit 101. The two chunks have distinct, non-overlapping UV ranges; both rects
are the identical 1×1 at (63, 239).

**Attribution.** Only one code path produces that pair. For face 337's chart
(extents ≈ 0.8128 × 1.4224 m at 0.5 m/texel: `interior_w = 2`, `interior_h = 3`),
`chunk_atlas_rect`'s center-based ownership yields disjoint rects — rows [0, 1)
and [1, 3), width 2 (hand-evaluated; also see the simulation below). The
degenerate-chart branch requires `uv_extent <= 0`, which these UVs exclude. The
only remaining source of `(x, y, 1, 1)` with x/y equal for both chunks is the
`placement.layer != 0` early-return in `bake_one_chunk`, which returns
`(placement.x, placement.y, 1, 1)` for **every** chunk of a spilled face.
(63, 239) is inside the 256×256 layer — it is the chart's placement on its own
layer ≥ 1.

The same 4×4×2 map with only the generator's capped 6 animated lights compiles,
but logs `332 chunks, … 0 covered texels` on a 256×256×**8** atlas — nearly
every animated chunk hit the layer ≥ 1 skip, so the map bakes **no** animated
direct light at all, silently. Layer spill is the norm, not the exception: the
multi-bin packer sizes the layer dimension to fit the largest single BSP leaf,
so even small maps go multi-layer (`animated-lightmap-array-atlas` measured
switch-demo at 512×512×2).

## Same-layer arithmetic holds

A float32-faithful re-implementation of `recurse` (with its per-chart pitch
floor and `split_eps`) plus `chunk_atlas_rect` (snap, ceil, clamps) was run over
4 000 randomized trials — chart extents 3–32 m × 0.1–32 m, densities 0.25/0.5,
5–24 lights with radii 0.5–12 m: **zero** same-face rect overlaps for in-bounds
layer-0 charts. The known historical collapse modes are already guarded:

- sub-texel chunks — per-chart pitch floor in `build_animated_light_chunks`
  (regression: `coarse_chart_chunks_stay_wider_than_one_chart_texel`);
- shared-boundary drift — `BOUNDARY_SNAP_EPS` (regression:
  `sibling_chunks_with_drifted_shared_uv_edge_pack_without_overlap`).

Simulated edge-of-atlas / out-of-bounds placements collapse siblings readily
(the min-corner clamp pins them to the last texel column/row), but a correct
packer never emits such placements for layer-0 charts — `pack_layers` bounds
every chart within the layer or errors `ChartTooLarge`. That path stays an
assert-worthy packer violation.

Conclusion: the fix does not need to touch `chunk_atlas_rect`'s rounding. The
defect is the degenerate placeholder **representation**, not the arithmetic.
The panic message's "Likely causes" text blames the subdivider or drift; the
actual cause was the module's own layer-skip placeholder.

## Zero-area rects are already legal downstream

Consumer inventory for `ChunkAtlasRect { width: 0, height: 0 }` (all verified
against source this session):

| Consumer | Behavior with zero-area rect |
|---|---|
| `AnimatedLightWeightMapsSection::is_consistent` (level-format) | Σ width×height arithmetic; a 0-area rect contributes 0 texels and 0 offset_counts entries — passes |
| Section encode/decode (`to_bytes`/`from_bytes`) | Fixed-stride records; no positivity check |
| `validate_cross_section` (render-cpu) | Prefix-sum + Σ arithmetic with checked math — passes; rect count still pairs 1:1 with `AnimatedLightChunks` |
| `expand_dispatch_tiles` (renderer) | Explicit `width == 0 \|\| height == 0` skip → no dispatch tile, no compose write |
| Renderer `AnimatedLightmapResources::new` | If **all** chunks degrade, `texel_lights` is empty → existing dummy-atlas early-out (same gate the all-SDF path uses); mixed sections build normally |
| `prl_loader` | Logs counts, no per-rect validation |
| Compiler concat loop / stats / byte-size log | `running_texel_offset += 0`; stats formulas tolerate 0 |

By contrast the current 1×1 placeholder **does** get a dispatch tile and
`textureStore`s black (irradiance) plus a zero-coverage direction texel at
`(placement.x, placement.y)` — coordinates from a layer ≥ 1 chart, landing on
whatever layer-0 chart owns them in the single-layer compose atlas. Unordered
with respect to that chart's real write → write race. Same finding as
`animated-lightmap-array-atlas` § Background ("possible corruption").

## Assert strict-inequality false positive on zero-area rects

`assert_no_overlapping_rects_per_face`'s test
(`a.x < b.x + b.w && b.x < a.x + a.w`, likewise Y) does **not** automatically
exempt zero-area rects: a 0×0 rect at coordinates strictly inside a texel-bearing
rect satisfies both conjuncts (e.g. a = (2,2,0,0), b = (0,0,4,4)). Mathematically
the empty interval overlaps nothing; the code disagrees. So degrading to
zero-area requires an explicit width/height-zero skip in the assert. Two
zero-area rects at the same coordinates do *not* trip it (both conjuncts need
strict `<`). Pinning degraded rects to `atlas_x = atlas_y = 0` alone is not
sufficient — (0,0) can sit inside a real layer-0 rect.

## Scale observation (out of scope)

With 64 animated lights on the 4×4×2 warren, the chunk subdivider logged
`112106 chunks exceeded cap 4 at the min-extent floor; 1091627 extra light
entries retained beyond the cap`. Heavily overlapped animated influence sets
blow up chunk counts and section sizes long before the packer is the problem.
That is subdivider/coverage capacity, orthogonal to this fix; noted in the
spec's out-of-scope list.

## Fixture note

The in-module test fixtures (`bake_with_geometry_and_chunks`) derive placements
from a real `bake_lightmap` run, which lands everything on layer 0. The repro
test constructs `ChartPlacement { layer: 1, .. }` directly — acceptable here
because `bake_one_chunk` reads only the placement fields, and building a real
two-layer pack inside a unit test is `animated-lightmap-array-atlas`'s burden
for its own layer-spanning bake tests, not this bugfix's.
