# Environment-volume playtest hall: fluids, gravity and push

A sealed hall of small areas, one per environment-volume behaviour, for the
manual swim, low-gravity and co-op playtests. Its fluids, `water` and `sludge`,
come from content/dev's manifest (`start-script.ts`); the player's swim tuning
is the `movement.swim` block in `scripts/player.ts`. Build and run it:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/e24-environment-test.map -o content/dev/maps/e24-environment-test.prl
cargo run -p postretro-tool -- run --install-root . maps/e24-environment-test.prl
```

`postretro-tool run` passes `--mod`, so its map path is relative to the mod
folder (`content/dev`). The `.prl` is a local build artifact; do not commit it. Units: 1 map unit =
0.0254 m. Room floors are at z 0, pits bottom out at z −112, and every fluid
surface sits at z −16.

## Areas

You spawn in the pool room, facing the pool.

| Area | Where | Volumes | Tests |
|---|---|---|---|
| Pool | Pool room (south-west), pit x 128–384 | One `fluid_volume` `water` with two brushes, sunk into the floor | Swim entry and exit, treading at the surface, the tint and muffle, and no surface drawn between the two brushes or against the pit walls. |
| Ledges | North rim of the pool | Solid blocks 24 u (0.61 m), 32 u (0.81 m) and 48 u (1.22 m) above the water; the plain rim is 16 u (0.41 m) | The fluid jump. By estimate (eye just out, 9.35 m/s, 20.32 m/s²) the feet clear about 0.85 m: the 24 u ledge should be reachable, 32 u marginal, and 48 u out of reach as a control. |
| Floating block | Pool room, north-east corner, z 112–208 | `fluid_volume` `water` in mid-air | Jumping up into its bottom enters swim; swimming down out of its bottom drops you back under level gravity. Surfaces visible from inside and outside. |
| Low-gravity room | Middle south room | `gravity_volume` `-6.8` covering the room, pit included | Falling and jumping at −6.8 m/s², a third of default, with level gravity back on the first tick outside the doorway. |
| Low-gravity pool | Pit inside that room | `fluid_volume` `water` | Sinking at about a third of the normal rate with no input; a much higher fluid jump. |
| Low-gravity floating block | That room, north side, z 192–320 | `fluid_volume` `water` in mid-air | Reaching and leaving a floating block under low gravity. |
| Sludge pool | South-east room | `fluid_volume` `sludge` | A thicker fluid: speed bleeds off three times faster than in water, with a darker, near-opaque tint and surface. |
| Updraft | North hall, west, shaft x 128–256 | `push_volume` `0 0 25.5` up to z 352 | Lift-off from standing, rising to a terminal speed, and steering west onto the balcony (top at z 304, 7.7 m). |
| Wind corridor | North hall, middle, x 320–1216 | `push_volume` `8 0 0` | Standing still drifts east at a steady speed; airborne drift is not clipped by the air speed cap; walls absorb it. |
| Calm pocket | Middle of the wind corridor, x 704–832 | `push_volume` `0 0 0`, `priority` 1 | The pocket wins over the wind; standing in it, nothing pushes you. |
| Current | North hall, east, channel x 1296–1424 | `fluid_volume` `water` plus a coincident `push_volume` `0 6 0` | Swimming with no input drifts north at about push ÷ viscosity (3 m/s). |
| Still pool | Fluid-motion wing (east, through the north hall's east doorway), x 1616–1872, y 64–320 | `fluid_volume` `water`, `fluids/water_64` at scale 1 | The surface shimmers and rings in place, stepping in whole texels. The low wall on its north rim is at the same scale: water and wall share one grain. |
| Current pool | Wing, x 1936–2192, y 64–320 | `fluid_volume` `water`, `fluids/water_64` at scale 1, plus a coincident `push_volume` `0 6 0` | Beside the still pool, the surface drifts north at the speed it carries a swimmer (3 m/s); standing in it carries you that way. |
| Fine pool | Wing, x 2256–2512, y 64–320 | `fluid_volume` `water`, `fluids/water_64` at scale 0.25 | Four times finer grain, matching the scale-0.25 low wall on its rim. Seen from far away, it settles calm rather than noisy. |
| Partial push | Wing, x 1616–2000, y 448–640 | One two-brush `fluid_volume` (`fluids/water_128`) under a `push_volume` `6 0 0` over x 1616–1904 | The pushed part drifts east and the rest stays still, with the east brush's face split at x 1904 and no surface between the brushes. |
| Bending river | Wing, east, an L from x 2064 to 2512 then north to y 960 | One two-brush `fluid_volume` (`fluids/water_neon_64`) under two `push_volume`s, `6 0 0` then `0 6 0`, both `force` `0` | The surface drifts along each bend; swimming in it, nothing pushes you. |
| Force-3 trap | Wing, x 1616–1872, y 768–960 | `fluid_volume` `water`, `fluids/water_64` at scale 1, plus a `push_volume` `0 -3 0`, `force` `3` | The surface drifts gently south, but the current shoves three times as hard as it looks. |

## Manual checks

- **Swim and fluid jump:** in the pool, swim, tread at the surface, and
  fluid-jump onto each ledge. The jump should feel reliable, and the ledges
  within reach should be climbable with it. Record which ledge heights you
  reach.
- **Low-gravity room:** jump and fall in the low-gravity room, swim its
  pool, and reach and leave its floating block. Walk out through a doorway and
  confirm normal gravity returns at once.
- **Fluid motion:** in the wing, compare the still pool with the current pool, and check that standing in the current carries you the way, and at about the speed, its surface moves. Check the bending river pushes nothing and the force-3 pool shoves harder than it looks. Look closely at cell borders on the still and fine pools for smear, and at a view-filling still pool for flicker.
- **Co-op:** host this map, join with a second client, and repeat the swim,
  the low-gravity room, the updraft, the wind corridor and the current on the
  client. The client should show no visible corrections.
