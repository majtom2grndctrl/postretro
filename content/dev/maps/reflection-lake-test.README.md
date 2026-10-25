# Reflection lake: capture exclusion at lake scale

A sealed hall holding one lake more than 50 m across, for the manual visual
check of reflection-probe capture exclusion: which brushes the lake's probe
leaves out, so they reflect only through screen-space reflections (SSR), and
which it keeps. The lake has posts, a dock, a boathouse, shore rails and
crates, and one `func_group` override of each kind. The `water` fluid comes
from content/dev's manifest (`start-script.ts`), so the map has no data
script. Build and run it:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/reflection-lake-test.map -o content/dev/maps/reflection-lake-test.prl
cargo run -p postretro-tool -- run --install-root . maps/reflection-lake-test.prl
```

`postretro-tool run` passes `--mod`, so its map path is relative to the mod
folder (`content/dev`). The `.prl` is a local build artifact; do not commit it.
Units: 1 map unit = 0.0254 m. The hall is 3840 u (97.5 m) square, and the
shore runs 832 u (21 m) from each wall to the water. The shore is at z 0, the
water surface at z −48 (1.2 m below the shore), the lake bed at z −160 and the
ceiling at z 2112 (53.6 m). Worldspawn sets `_lightmap_density` 0.125 to keep
the bake of the hall quick: about 2 minutes cold, 10 seconds warm, with the
debug `prl-build`.

You spawn on the south shore, in the dark, facing north across the lake at the
dock (left of centre) and the boathouse (right).

## Lighting

The hall is lit in pools, with dark floor, water and roof between them. Eleven
lamps hang at z 160–224 (4–5.7 m), and the boathouse keeps its warm interior
light. Every light's `_falloff_range` is less than half its distance to the
nearest other light, so no two pools overlap; the tightest pair, the south lake
and south quay lamps, stands 781 u apart with ranges 384 and 352. Falloff is
linear (`delay` 0). No light reaches the roof, so the probe sees a dark ceiling
over the lake.

One runtime blue spot (`light_spot`, colour 0 126 255, `light` 60) hangs at
1824 2208 752, over the centre lamp, pointing straight down with a 10°/80°
cone and a 5000 u range. It is the exception to the pools: its cone reaches
the water and shore below it.

| Lamp | Origin | Range | `light` | Runtime | Lights |
|---|---|---|---|---|---|
| Centre | 1824 2208 224 | 448 | 500 | yes | P6 and P7 side by side, and P1 |
| North shore | 1584 3104 160 | 384 | 450 | yes | the dock's shore end, the shore crates, the near ends of both rails, the `never` wall |
| North-west shore | 800 3136 192 | 320 | 450 | bake only | the west end of the `never` wall and of the thin rail |
| West lake | 928 2048 224 | 384 | 500 | bake only | P3 |
| South lake | 1600 1312 224 | 384 | 500 | bake only | P2 |
| East lake | 2752 1536 224 | 384 | 500 | bake only | P4 |
| South quay | 2176 784 224 | 352 | 500 | bake only | P5 and the south quay |
| West wall | 192 1600 224 | 384 | 450 | bake only | the yellow wall |
| East wall | 3648 2304 224 | 384 | 450 | bake only | the red wall |
| South wall | 1024 192 224 | 384 | 450 | bake only | the stone wall |
| North wall | 3328 3648 224 | 384 | 450 | bake only | the green wall |
| Boathouse, warm | 2560 3104 160 | 448 | 220 | yes | the boathouse inside, out through the boat opening |

Only three lights are runtime lights, so no face has more than four lights
competing for its specular channels and the bake gives no lost-channel
warning. The rest are `_bake_only`. Some of the lake's features stand between
pools: the dock's lake end and its legs, the middle of each rail, and the
spawn. They reflect too, against the lit pools behind them.

## The lake's numbers

The lake is x 832–3008, y 832–3008: 2176 u = 55.27 m square, 3055 m². Solid
standing through the surface (posts, dock legs, boathouse walls in the water)
takes about 6.5 m², so the bound water is about 3048.5 m² and
W = √area ≈ **55.2 m**. At that W:

- size threshold t = p(W)·W ≈ **17.2 m** (677 u); p ≈ 0.31
- thin-brush floor f = W · 2·tan(θv/2) / 180 ≈ **0.41 m** (16 u)

The lake binds **one authored probe**: a `reflection_volume` drawn over the
hall interior (x 0–3840, y 0–3840, z −160 to 768), so W is the whole lake.
Without it, the derived fit splits the lake into eight probes: the water under
the dock and inside and around the boathouse sees a lower ceiling or nearer
walls than the open lake, so its faces fit boxes of their own, each with a
smaller W and its own excluded set. Delete the `reflection_volume` to see that
split; it is the per-probe content seam the brief accepts.
The capture point sits at the lake's centre, about x 1918, y 1919, 2 mm above
the water.

A brush is left out when its longest horizontal extent is below t, its height
is at least f, and no downward face of it overhangs the lake above the water.
`always` and `never` override that.

## Features and expected classification

| Feature | Where | Size | Expected | Why |
|---|---|---|---|---|
| Hall walls, ceiling, shore ground, lake bed | Shell | 97.5 m | Kept | Room scale, above t. |
| Shore walls (quay) | Ring the lake, 16 u thick, bed to shore top | 55–56 m long | Kept | Above t. |
| Posts P1–P5, P7 | In the water, 12 u (0.3 m) square, bed to z 96 (3.7 m above the water) | 0.3 m | **Excluded** | Below t, taller than f, no underside. P1 is about 7 m from the capture point, P2 16 m, P3 and P4 22 m, P5 26 m (near the south shore). |
| Post P6, purple, `func_group` `always` | In the water at x 1664, y 2304, beside its twin P7 at x 1792 | 0.3 m | **Kept** | The `always` override. P6 and P7 are identical but for the override, so they compare side by side, both in the centre lamp's pool. |
| Dock deck | x 1408–1536, y 2608–3008, z −40 to 0, flush with the quay top, 8 u (0.2 m) above the water | 10.2 m long, 1.0 m thick | **Kept** | Below t and above f, but its underside overhangs the water: the underside rule. |
| Dock legs (4) | Under the deck, 16 u square, bed to the deck | 0.41 m | **Excluded** | Below t, taller than f, no underside above the water. |
| Crate on the dock | 32 u cube sitting flush on the deck | 0.81 m | **Excluded** | CSG removes its bottom face, which lies on the deck's top, and its hull's bottom does not count as an underside. |
| Boathouse west and east walls, water parts | In the water, y 2816–3008, bed to z 192 | 4.9 m | **Excluded** | No downward face above the water. |
| Boathouse west and east walls, shore parts | On the quay and shore, y 3008–3200 | 4.9 m | **Excluded** | Below t, nothing over the water. |
| Boathouse lake-front stubs (2) | In the water either side of the boat opening | 1.2 m | **Excluded** | No downward face above the water. |
| Boathouse lake-front lintel | Over the boat opening, z 128–192 | 6.5 m long, 1.6 m deep | **Kept** | Its bottom face hangs over the water: the underside rule. |
| Boathouse back-wall stubs and door lintel | On the shore, y 3184–3200 | 2.8 m, 3.3 m | **Excluded** | The lintel's bottom face is over the shore, not the water. Compare with the lake-front lintel. |
| Boathouse roof | x 2336–2784, y 2784–3232, z 192–216, 32 u overhang | 11.4 m, 0.61 m thick | **Kept** | Thicker than f, so the floor does not keep it; its underside overhangs the water. |
| Thin rail top bar | North quay top, x 864–1376, 4 u (0.1 m) tall | 13 m long | **Kept** | Below t, but thinner than f: the thin floor. |
| Thin rail posts (5) | Under that bar, 4 u square, 40 u tall | 0.1 m | **Excluded** | Below t, 1 m tall. |
| Thick rail cap | North quay top, x 1792–2304, 24 u (0.61 m) tall | 13 m long | **Excluded** | Below t and thicker than f: size. |
| Thick rail posts (5) | Under that cap, 4 × 8 u | 0.2 m | **Excluded** | As the thin rail's. |
| Shore crates (2) | North shore beside the dock, 32 u and 48 u cubes | 0.8 m, 1.2 m | **Excluded** | Below t, on the shore. |
| Long wall, purple, `func_group` `never` | North shore, x 768–1792, y 3232–3264, 4 m tall | 26 m | **Excluded** | Above t, which would keep it; the `never` override. |

That is 34 brushes left out of the probe: posts 6, dock legs 4, dock crate 1,
boathouse 9, rail posts 10, thick rail cap 1, shore crates 2, `never` wall 1.
The bake logs it as `probe 0: 34 brush(es) excluded from its capture`.

## Manual checks

From the spawn, and walking the south, west and east shores:

- **Small geometry through SSR.** The posts, dock legs, rail posts, crates,
  boathouse walls and the `never` wall reflect with true parallax as you
  strafe; the hall walls, ceiling, quay and kept brushes come from the probe
  and hold steady. Judge the posts at each distance, each in its own pool, and
  the rail posts, for SSR losses on thin decor, at the screen edge, and past
  the march's reach.
- **Overrides.** P6 (`always`) shows in the probe as well as through SSR; its
  twin P7 does not. Toggle reflection quality to `low`, where only the probe
  reflects: P6 stays, P7 and every other excluded brush vanish. The purple
  `never` wall reflects only through SSR, and not at all at `low`.
- **Undersides.** The dock deck's and the boathouse roof's and lake-front
  lintel's undersides still reflect, including at `low`. Look at the boathouse
  from the water and from the south shore.
- **Filled patches.** Where excluded brushes touch kept ones (the legs under
  the dock deck, the boathouse walls under the roof and lintel, the rail posts
  on the quay, the crates and the `never` wall on the shore), the probe's view
  through them reads as the kept surface around them.
- **The floor.** The thin rail bar is the only brush the floor keeps. Its east
  end and the thick rail's west end share the north shore lamp's pool, at the
  same distance from it. Judge whether the thin bar reads better in the probe
  than its thick neighbour does through SSR.
- **High reflectivity from above.** Climb onto the dock or the quay and look
  down at the water near the posts: judge where reflected rays leave the top
  of the screen and excluded decor drops out.
- **Dark between pools.** The reflections of lit pools should sit in dark
  water; a lit patch in the water where no pool is above it, or a seam where a
  probe-reflected pool and its SSR copy disagree, is worth noting.
- Record the anchors' final values after retuning.
