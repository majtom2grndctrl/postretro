# Mover Set-Piece Fixture

Measurement and proof surface for **large** theatrical movers. Two movers that
each fill a big share of the screen cycle unattended in a small, quick-to-bake
two-room map. Intended uses:

1. A/B the GPU cost of the mover pass on the Mac target.
2. Prove that movers block shots, sight and enemies (the sliding wall stands
   between the player start and a marked enemy spot). Verifiable once
   `E17--movers-block-shots-and-sight` and `E17--enemy-mover-interaction` land.
3. Prove mover rest-state lighting (baked lights reach across both movers at
   both ends of their travel). Verifiable once mover state lighting lands.

Build a local artifact on demand:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/mover-setpiece.map -o content/dev/maps/mover-setpiece.prl
```

Do not commit generated `.prl` files or `baked/materials/` sidecars.

## Layout

TrenchBroom units (1 unit = 0.0254 m, z up). Floor top is z 0, ceiling z 320
(8.1 m).

- **Arena (south):** x 0–1280, y 0–1024 (32.5 × 26 m). Holds the player start
  and the rising-room shaft.
- **Dividing wall:** y 1024–1088 (1.6 m thick). Its opening spans x 96–640,
  z 0–224 (13.8 × 5.7 m). East of the opening a door pocket runs x 640–1184
  inside the wall, y 1040–1072.
- **Back room (north):** x 0–1280, y 1088–1600 (32.5 × 13 m).
- **Rising-room shaft:** a 320 × 320 hole in the arena floor at x 832–1152,
  y 160–480, sealed below by shaft walls and a bottom at z −192.

## Movers

| Mover | Brushes | Waypoint 0 | Waypoint 1 | Travel | Speed | `wait_ms` | Mode |
|---|---|---|---|---|---|---|---|
| `setpiece_wall` | 8 (4 columns × 2 rows, alternating metal) | `setpiece_wall_closed` (368, 1056, 112) | `setpiece_wall_open` (912, 1056, 112) | +x 544 (13.8 m) | 2 m/s | 3000 | `ping_pong`, `start_on_spawn 1` |
| `setpiece_room` | 8 (floor, roof, 3 walls, doorway wall in 3 pieces) | `setpiece_room_lowered` (992, 320, −96) | `setpiece_room_raised` (992, 320, 80) | +z 176 (4.5 m) | 1 m/s | 6000 | `ping_pong`, `start_on_spawn 1` |

- **Sliding wall.** A 13.8 × 5.7 × 0.8 m slab, authored closed in the opening.
  The floor and lintel frame its top and bottom, so prl-build recognizes it as
  a door that seals the opening (`Door recognition: 1 of 2 kinematic movers
  seal a doorway at their closed pose: movers 0`, then `kinematic mover 0
  seals generated portal …` lines). It slides east into the pocket and fills
  it exactly. One cycle (close, wait, open, wait) takes about 20 s.
- **Rising room.** An 8.1 × 8.1 m room, 4 m tall inside, with a 2.4 × 3.3 m
  doorway facing west (−x). It is authored lowered: its roof is flush with the
  arena floor and plugs the hole. It rises until its floor is level with the
  arena floor, so the floor slab plugs the hole at the top too. A player
  standing on the roof rides it up. One cycle takes about 21 s. Door
  recognition does not count the room as a door, because its lowered bottom
  rests on the shaft floor and fronts no open space. Its lowered roof still
  covers the portals between the arena and the shaft, so the compiler logs
  `kinematic mover 1 seals generated portal …` for them.
- **Carried light.** `light_dynamic` `setpiece_room_lamp` (warm, falloff 320)
  has `carrier setpiece_room`. It is authored at (992, 320, −56), near the
  lowered room's ceiling, and rides with the room.

## Lights

Six baked lights and one carried dynamic light. Every baked falloff crosses the
wall or the room. More than four baked lights reach some large faces, such as
the single ceiling brush. On those faces the lowest-ranked lights lose their
shadowmask specular channel, and the compiler warns once per light and face.
Those faces show no static highlight from the losing lights. Diffuse lighting
is unaffected.

| Light | Origin | Falloff | Reaches |
|---|---|---|---|
| `light` | (256, 256, 256) | 900 | West arena, south face of the wall |
| `light` | (640, 320, 192) | 900 | West face and doorway of the risen room; the wall |
| `light` | (992, 320, 288) | 640 | The roof over the hole, the room's roof and walls |
| `light_spot` (cone 30/50, pitch −21, yaw 90) | (368, 560, 288) | 1100 | Aimed at the wall's south face; through the opening into the back room |
| `light` | (240, 1376, 256) | 900 | North face of the wall, the enemy spot, the arena through the opening |
| `light` | (1000, 1376, 256) | 900 | Back room east, the pocket end, the arena through the opening |

## Stations

Use dev-tools noclip, or walk: the back room is reachable through the opening
while the wall is open.

- **Station 1 — wall fills the screen (player start).** (368, 720, 48), yaw 90
  (facing +y, north). The closed wall is 8.1 m away, centred, and fills most of
  the view. While open, only the pocket edge and the back room show. This is
  the perf station.
- **Station 2 — rising room from outside.** (560, 320, 48), yaw 0 (facing +x,
  east). The room's doorway wall is 6.9 m ahead. Watch the roof sit flush in
  the floor, then the room rise with the lamp lit inside.
- **Station 3 — behind the wall.** (368, 1536, 48), yaw 270 (facing −y,
  south). The north face of the wall fills the view, with the enemy spot in the
  foreground.

**Enemy spot.** A red 64 × 64 × 8 plinth (`debug_floor_red`) at x
336–400, y 1200–1264 in the back room; its centre is 4.1 m behind the closed wall. It
lies on the line from station 1 through the closed wall's centre. Place an
enemy on it, for example at (368, 1232, 32), to prove that the closed
wall blocks shots, sight and enemy perception, and that the open wall does not.
The map places no enemy.

## Measuring the mover pass

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/mover-setpiece.map -o content/dev/maps/mover-setpiece.prl
RUST_LOG=info POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run --profile profiling --features dev-tools -- content/dev/maps/mover-setpiece.prl
```

Command forms come from `context/lib/development_guide.md` §Build and run and
`context/lib/rendering_pipeline.md` §12. Add `POSTRETRO_CPU_TIMING=1` for the
`rec_kinematic_brush` CPU recording stage.

1. Spawn at station 1 and do not touch the input. The movers start on spawn.
2. Record at least two full wall cycles (about 40 s). GPU timing logs one
   window every 120 readbacks. Read the mover pass (`kinematic_brush` in the
   GPU timing window) beside `forward` and `depth_prepass`.
3. Per-window values swing with the wall's phase: the closed wall covers most
   of the screen, and the open wall covers almost nothing. Compare means over
   the same number of whole cycles, or compare only the windows logged while
   the wall rests closed.
4. A/B two builds under the same machine state, resolution and window mode.
   Follow §12's confounder rules: no bake or build running alongside, pin the
   GPU clock with a saturating resolution, and record the surface and scene
   extents.
5. If the adapter lacks timestamp queries, GPU timing reports unsupported. Use
   an Instruments Metal System Trace instead (§12). The mover pass is labelled
   `Kinematic Brush Pass`.

These are manual, in-engine checks. Compiling the map validates only the
authored content.
