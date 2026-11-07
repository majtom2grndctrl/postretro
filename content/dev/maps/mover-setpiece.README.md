# Mover Set-Piece Fixture

Measurement and proof surface for **large** theatrical movers. Two movers that
each fill a big share of the screen cycle unattended in a small, quick-to-bake
two-room map. Intended uses:

1. A/B the GPU cost of the mover pass on the Mac target. Timing setup and
   the `kinematic_brush` pass are in `context/lib/rendering_pipeline.md` §12.
2. Prove enemies on movers: the sliding wall stops the chaser until it opens,
   and the rising room carries its riders (see Enemy checks). Movers do not yet
   block shots and sight.
3. Prove mover rest-state lighting (baked lights reach across both movers at
   both ends of their travel). Verifiable once mover light flow's probe and
   world-surface channels land (roadmap, Lighting and GI).

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
| `setpiece_wall` | 8 (4 columns × 2 rows, alternating metal) | `setpiece_wall_closed` (368, 1056, 112) | `setpiece_wall_open` (912, 1056, 112) | +x 544 (13.8 m) | 2 m/s | 8000 | `ping_pong`, `start_on_spawn 1` |
| `setpiece_room` | 8 (floor, roof, 3 walls, doorway wall in 3 pieces) | `setpiece_room_lowered` (992, 320, −96) | `setpiece_room_raised` (992, 320, 80) | +z 176 (4.5 m) | 1 m/s | 6000 | `ping_pong`, `start_on_spawn 1` |

- **Sliding wall.** A 13.8 × 5.7 × 0.8 m slab, authored closed in the opening.
  The floor and lintel frame its top and bottom, so prl-build recognizes it as
  a door that seals the opening (`Door recognition: 1 of 2 kinematic movers
  seal a doorway at their closed pose: movers 0`, then `kinematic mover 0
  seals generated portal …` lines). It slides east into the pocket and fills
  it exactly. One cycle (close, wait, open, wait) takes about 30 s.
- **Rising room.** An 8.1 × 8.1 m room, 4 m tall inside, with a 2.4 × 3.3 m
  doorway facing west (−x). It is authored lowered: its roof is flush with the
  arena floor and plugs the hole. It rises until its floor is level with the
  arena floor, so the floor slab plugs the hole at the top too. A player
  standing on the roof rides it up. One cycle takes about 21 s. Door
  recognition does not count the room as a door, because its lowered bottom
  rests on the shaft floor and fronts no open space. Its lowered roof still
  covers the portals between the arena and the shaft, so the compiler logs
  `kinematic mover 1 seals generated portal …` for them.
- **Carried lights.** Two dynamic lights have `carrier setpiece_room` and ride
  with the room. Both are authored at the lowered pose. `light_dynamic`
  `setpiece_room_lamp` (warm, intensity 75, falloff 640) is at (992, 320, −56),
  near the lowered room's ceiling. `light_dynamic_spot`
  `setpiece_room_corner_spot` (warm, intensity 150, cone 20/40, falloff
  600) is at (900, 220, −40), aimed at the north-east inside corner. A carried
  spot keeps its authored direction; only its position follows the room.

## Lights

Five baked point lights, five baked spots and two carried dynamic lights. Point lights
are dim (intensity 75) with wide falloff (double the earlier values); the spots
add contrast and angle. Every baked falloff crosses the wall or the room. More
than four baked lights reach some large faces, such as the single ceiling brush.
On those faces the lowest-ranked lights lose their shadowmask specular channel,
and the compiler warns once per light and face. Those faces show no static
highlight from the losing lights. Diffuse lighting is unaffected.

| Light | Origin | Intensity | Falloff | Reaches |
|---|---|---|---|---|
| `light` | (256, 256, 256) | 75 | 1800 | West arena, south face of the wall |
| `light` | (640, 320, 192) | 75 | 1800 | West face and doorway of the risen room; the wall |
| `light` | (992, 320, 288) | 75 | 1280 | The roof over the hole, the room's roof and walls |
| `light_spot` (cone 30/50, pitch −21, yaw 90) | (368, 560, 288) | 400 | 1100 | Aimed at the wall's south face; through the opening into the back room |
| `light` | (240, 1376, 256) | 75 | 1800 | North face of the wall, the enemy spot, the arena through the opening |
| `light` | (1000, 1376, 256) | 75 | 1800 | Back room east, the pocket end, the arena through the opening |
| `light_spot` (cone 20/40, angles −23 19 0) | (60, 900, 250) | 250 | 1000 | Rakes across the wall's south face from the west |
| `light_spot` (cone 12/28, angles −44 319 0) | (150, 1420, 290) | 300 | 650 | A tight pool on the enemy plinth |
| `light_spot` (cone 14/32, angles −59 319 0) | (560, 760, 305) | 300 | 700 | A cool shaft landing in the arena, off the station 1 sightline |
| `light_spot` (cone 18/38, angles −28 44 0) | (560, 60, 300) | 200 | 700 | Rakes across the rising room's doorway wall |
| `light_dynamic` `setpiece_room_lamp` (carried) | (992, 320, −56) | 75 | 640 | Inside the room, at every height |
| `light_dynamic_spot` `setpiece_room_corner_spot` (carried, cone 20/40, angles −22 46 0) | (900, 220, −40) | 150 | 600 | The room's north-east inside corner |

## Stations

Use dev-tools noclip, or walk: the back room is reachable through the opening
while the wall is open.

- **Station 1 — wall ahead (player start).** (368, 720, 48), yaw 90
  (facing +y, north). The closed wall is 8.1 m away, centred, and covers about
  37% of the frame at FOV 100. While open, only the pocket edge and the back room show. This is
  the perf station.
- **Station 2 — rising room from outside.** (560, 320, 48), yaw 0 (facing +x,
  east). The room's doorway wall is 6.9 m ahead. Watch the roof sit flush in
  the floor, then the room rise with the lamp lit inside.
- **Station 3 — behind the wall.** (368, 1536, 48), yaw 270 (facing −y,
  south). The north face of the wall is centred ahead, with the enemy spot in the
  foreground.

**Enemy spot.** A red 64 × 64 × 8 plinth (`debug_floor_red`) at x
336–400, y 1200–1264 in the back room; its centre is 4.1 m behind the closed wall. It
lies on the line from station 1 through the closed wall's centre. A
`reference_enemy` (the chaser) stands on it at (368, 1232, 16), facing south.
It proves the door and lift routing in Enemy checks. Movers do not yet block
shots and sight, so the closed wall does not hide it from the player.

## Enemies

Three `reference_enemy` instances, all tagged `enemy`. This map ships no
combat script of its own, so they use the dev mod's shipped behavior: idle until
the player comes within about 16 m (about 630 units), then chase and melee.

| Enemy | Origin | Tag | Where |
|---|---|---|---|
| Chaser | (368, 1232, 16) | `setpiece_chaser` | On the enemy spot in the back room, 4.1 m behind the closed wall |
| Rider A | (912, 280, -160) | `setpiece_rider` | Inside the rising room, authored at its lowered pose |
| Rider B | (1040, 376, -160) | `setpiece_rider` | Inside the rising room, near the north-east corner |

The riders' origins sit 16 units above the room floor top (z -176 lowered).

## Enemy checks

The map is unbaked until you build it, and the enemy routing in these checks
needs a rebuild with a current `prl-build` (a stale `.prl` loads with no door or
lift routing and one warning). These are manual, in-engine checks.

1. **Chaser behind the sliding wall.** Spawn at station 1. The chaser is 13 m
   from the start, so it wakes at once and walks to the wall. The wall starts
   moving at spawn, so the chaser may meet it closed, sliding, or resting open.
   A moving wall does not hold it back in its route, so it walks into the slab
   and stands pressed against it, facing it, until the gap fits it. It must not
   turn left and right, drift side to side, or lurch in repeated stuck-recovery
   nudges. At the closed wall it stops and waits without jitter or
   foot-shuffling for the rest of the closed wait. Once the opening fits it,
   it passes through and comes at you. Give it a full wall cycle (about 30 s).
2. **Riders in the rising room.** Walk to station 2 (560, 320, 48), within 16 m
   of both riders, and face east. The room is lowered, so the riders cannot
   reach you: their doorway fronts the shaft wall. Expect them to stand inside
   with the lamp on, not pace or shuffle. When the room reaches the top they
   leave through the west doorway onto the arena and chase you. They must ride
   up with their feet planted (idle pose, no walk cycle), not slide or sink into
   the floor.
