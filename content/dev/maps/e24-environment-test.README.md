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
| Ledges | North rim of the pool | Solid blocks 24 u (0.61 m), 32 u (0.81 m) and 48 u (1.22 m) above the water; the plain rim is 16 u (0.41 m) | The fluid jump. By estimate (eye just out, 6.5 m/s, 9.81 m/s²) the feet clear about 0.85 m: the 24 u ledge should be reachable, 32 u marginal, and 48 u out of reach as a control. |
| Floating block | Pool room, north-east corner, z 112–208 | `fluid_volume` `water` in mid-air | Jumping up into its bottom enters swim; swimming down out of its bottom drops you back under level gravity. Surfaces visible from inside and outside. |
| Low-gravity room | Middle south room | `gravity_volume` `-3` covering the room, pit included | Falling and jumping at −3 m/s², with level gravity back on the first tick outside the doorway. |
| Low-gravity pool | Pit inside that room | `fluid_volume` `water` | Sinking at about a third of the normal rate with no input; a much higher fluid jump. |
| Low-gravity floating block | That room, north side, z 192–320 | `fluid_volume` `water` in mid-air | Reaching and leaving a floating block under low gravity. |
| Sludge pool | South-east room | `fluid_volume` `sludge` | A thicker fluid: speed bleeds off three times faster than in water, with a darker, near-opaque tint and surface. |
| Updraft | North hall, west, shaft x 128–256 | `push_volume` `0 0 15` up to z 352 | Lift-off from standing, rising to a terminal speed, and steering west onto the balcony (top at z 304, 7.7 m). |
| Wind corridor | North hall, middle, x 320–1216 | `push_volume` `8 0 0` | Standing still drifts east at a steady speed; airborne drift is not clipped by the air speed cap; walls absorb it. |
| Calm pocket | Middle of the wind corridor, x 704–832 | `push_volume` `0 0 0`, `priority` 1 | The pocket wins over the wind; standing in it, nothing pushes you. |
| Current | North hall, east, channel x 1296–1424 | `fluid_volume` `water` plus a coincident `push_volume` `0 6 0` | Swimming with no input drifts north at about push ÷ viscosity (3 m/s). |

## Manual checks

- **X3, swim and fluid jump:** in the pool, swim, tread at the surface, and
  fluid-jump onto each ledge. The jump should feel reliable, and the ledges
  within reach should be climbable with it. Record which ledge heights you
  reach.
- **X4, low-gravity room:** jump and fall in the low-gravity room, swim its
  pool, and reach and leave its floating block. Walk out through a doorway and
  confirm normal gravity returns at once.
- **X5, co-op:** host this map, join with a second client, and repeat the swim,
  the low-gravity room, the updraft, the wind corridor and the current on the
  client. The client should show no visible corrections.
