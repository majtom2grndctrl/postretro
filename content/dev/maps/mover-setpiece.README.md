# Mover Set-Piece Fixture

Measurement and proof surface for **large** theatrical movers. Three movers that
each fill a big share of the screen cycle unattended in a small, quick-to-bake
map: two rooms, an L-shaped hallway and a lift chamber. Intended uses:

1. A/B the GPU cost of the mover pass on the Mac target. Timing setup and
   the `kinematic_brush` pass are in `context/lib/rendering_pipeline.md` §12.
2. Prove enemies on movers: the sliding wall stops the chaser until it opens,
   and the rising room carries its riders (see Enemy checks). Movers do not yet
   block shots and sight.
3. Prove mover lighting through the probe channel, at rest and in motion:
   baked lights reach across every mover at both ends of its travel, and the
   rising rooms' faces cross probe layers as they move. The big rising room is
   the thick-walled case: its 64-unit walls and slabs hold whole probe layers.

Build a local artifact on demand:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/mover-setpiece.map -o content/dev/maps/mover-setpiece.prl
```

Do not commit generated `.prl` files or `baked/materials/` sidecars.

## Layout

TrenchBroom units (1 unit = 0.0254 m, z up). Floor top is z 0, ceiling z 320
(8.1 m).

- **Arena (south):** x 0–1280, y 0–1024 (32.5 × 26 m). Holds the player spawn
  and the rising-room shaft.
- **Dividing wall:** y 1024–1088 (1.6 m thick). Its opening spans x 96–640,
  z 0–224 (13.8 × 5.7 m). East of the opening a door pocket runs x 640–1184
  inside the wall, y 1040–1072.
- **Back room (north):** x 0–1280, y 1088–1600 (32.5 × 13 m).
- **Rising-room shaft:** a 320 × 320 hole in the arena floor at x 832–1152,
  y 160–480, sealed below by shaft walls and a bottom at z −192.
- **Hallway (east):** an L, 128 wide and 160 tall (3.3 × 4.1 m). Its mouth is
  in the arena's east wall at y 640–768, z 0–160. Leg 1 runs east to x 1792;
  leg 2 runs north over x 1664–1792 to y 1136, where a 128 × 160 opening in
  the chamber's south wall lets it in.
- **Lift chamber:** x 1344–2112, y 1152–1984, z 0–320 (19.5 × 21.1 m).
- **Big rising-room shaft:** a 512 × 560 hole in the chamber floor at x
  1472–1984, y 1296–1856, sealed below by shaft walls and a bottom at z −336.

Every brush is convex. All are axis-aligned boxes except the big room's ten
arch voussoirs.

## Movers

| Mover | Brushes | Waypoint 0 | Waypoint 1 | Travel | Speed | `wait_ms` | Mode |
|---|---|---|---|---|---|---|---|
| `setpiece_wall` | 8 (4 columns × 2 rows, alternating metal) | `setpiece_wall_closed` (368, 1056, 112) | `setpiece_wall_open` (912, 1056, 112) | +x 544 (13.8 m) | 2 m/s | 8000 | `ping_pong`, `start_on_spawn 1` |
| `setpiece_room` | 8 (floor, roof, 3 walls, doorway wall in 3 pieces) | `setpiece_room_lowered` (992, 320, −96) | `setpiece_room_raised` (992, 320, 80) | +z 176 (4.5 m) | 1 m/s | 6000 | `ping_pong`, `start_on_spawn 1` |
| `setpiece_room_big` | 17 (floor, roof, 3 walls, doorway wall in 2 pieces, 10 arch voussoirs) | `setpiece_room_big_lowered` (1728, 1576, −192) | `setpiece_room_big_raised` (1728, 1576, 80) | +z 272 (6.9 m) | 1 m/s | 6000 | `ping_pong`, `start_on_spawn 1` |

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
- **Big rising room.** A 13 × 14.2 m room outside, its walls, floor and roof
  64 units (1.6 m) thick, so a probe layer can sit wholly inside one. Inside
  it is 9.8 × 11 × 4.1 m. A round-arch doorway, 100 wide (2.5 m) and 130 tall
  (3.3 m) at its apex, faces south toward the hallway. It is authored lowered,
  its roof top 48 units below the chamber floor, so the roof is the floor of a
  pit. It rises until its floor is level with the chamber floor. One cycle
  takes about 26 s.
- **Carried lights.** Two dynamic lights have `carrier setpiece_room` and ride
  with the room. Both are authored at the lowered pose. `light_dynamic`
  `setpiece_room_lamp` (warm, intensity 10, falloff 4) is at (992, 320, −56),
  near the lowered room's ceiling. `light_dynamic_spot`
  `setpiece_room_corner_spot` (warm, intensity 150, cone 20/40, falloff
  600) is at (900, 220, −40), aimed at the north-east inside corner. A carried
  spot keeps its authored direction; only its position follows the room. The
  big room carries the same pair, authored at its lowered pose:
  `setpiece_room_big_lamp` at (1728, 1576, −152) and
  `setpiece_room_big_corner_spot` (intensity 150, cone 20/40, falloff 700) at
  (1600, 1424, −136), aimed at its north-east inside corner.

## Lights

Seven baked point lights, nine baked spots and four carried dynamic lights. The
arena and back-room point lights are dim (intensity 75) with wide falloff
(double the earlier values), and the chamber's two are dimmer still (intensity
5); the spots add contrast and angle. Every arena and back-room falloff crosses
the wall or the room. The hallway is lit by spots alone. More
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
| `light_spot` (cone 20/40, angles −36 131 0) | (1424, 660, 144) | 250 | 400 | Rakes the hallway mouth's north jamb, so the opening reads from the arena |
| `light_spot` (cone 16/34, angles −48 135 0) | (1770, 662, 144) | 250 | 350 | A pool at the hallway's corner |
| `light_spot` (cone 12/26, angles −10 95 0) | (1776, 784, 150) | 400 | 750 | Down leg 2, through the chamber opening, toward the arched doorway |
| `light` | (1728, 1576, 288) | 5 | 1280 | The chamber over the big shaft, the big room's roof |
| `light` | (1440, 1232, 192) | 5 | 1000 | The chamber's south-west corner |
| `light_spot` (cone 18/38, angles −32 20 0) | (1400, 1180, 300) | 200 | 700 | Rakes across the big room's arched doorway wall from the south-west |
| `light_dynamic` `setpiece_room_lamp` (carried) | (992, 320, −56) | 10 | 4 | A faint glow at the lamp, at every height |
| `light_dynamic_spot` `setpiece_room_corner_spot` (carried, cone 20/40, angles −22 46 0) | (900, 220, −40) | 150 | 600 | The room's north-east inside corner |
| `light_dynamic` `setpiece_room_big_lamp` (carried) | (1728, 1576, −152) | 10 | 4 | A faint glow at the big room's lamp |
| `light_dynamic_spot` `setpiece_room_big_corner_spot` (carried, cone 20/40, angles −18 49 0) | (1600, 1424, −136) | 150 | 700 | The big room's north-east inside corner |

## Stations

Use dev-tools noclip, or walk: the back room is reachable through the opening
while the wall is open.

**Spawn.** The player spawns in the arena's south-west corner at (64, 64, 48),
yaw 75, looking north-east through the opening at the enemy spot. Every enemy is
more than 16 m away (the chaser 30.7 m, the riders 22.8 m and 26.6 m), so none
wakes at spawn.

- **Station 1 — wall ahead.** (368, 720, 48), yaw 90
  (facing +y, north). The closed wall is 8.1 m away, centred, and covers about
  37% of the frame at FOV 100. While open, only the pocket edge and the back room show. This is
  the perf station. Walk or noclip here from the spawn; the measurement
  harness sets its own camera.
- **Station 2 — rising room from outside.** (560, 320, 48), yaw 0 (facing +x,
  east). The room's doorway wall is 6.9 m ahead. Watch the roof sit flush in
  the floor, then the room rise with the lamp lit inside.
- **Station 3 — behind the wall.** (368, 1536, 48), yaw 270 (facing −y,
  south). The north face of the wall is centred ahead, with the enemy spot in the
  foreground.
- **Station 4 — big room from the hallway.** (1728, 880, 48), yaw 90 (facing
  +y, north), in the hallway's second leg. Through the chamber opening, the big
  room's arched south wall is 10.6 m ahead: lowered, only the pit shows; raised,
  the arch is centred in the opening. Reach it through the hallway mouth in the
  arena's east wall.

**Enemy spot.** A red 64 × 64 × 8 plinth (`debug_floor_red`) at x
336–400, y 1200–1264 in the back room; its centre is 4.1 m behind the closed wall. It
lies on the line from station 1 through the closed wall's centre. A
`reference_enemy` (the chaser) stands on it at (368, 1232, 16), facing south.
It proves the door and lift routing in Enemy checks. Movers do not yet block
shots and sight, so the closed wall does not hide it from the player.

## Enemies

Three `reference_enemy` instances, all tagged `enemy`. This map ships no
combat script of its own, so they use the dev mod's shipped behavior. Until the
player comes within 16 m (about 630 units) each walks a back-and-forth patrol
through three points measured from its spawn: (0, 0), (6, 0) and (6, 6) m on the
engine's floor axes. Once the player is within 16 m it chases and melees. Walls
and movers do not hide the player from that check.

The chaser's patrol runs through the sliding wall's opening. In map units its
points are the enemy spot (368, 1232), (368, 996) just south of the wall in the
middle of the opening, and (132, 996) at the opening's west end. The riders'
points lie outside the room, on the arena floor south of the shaft, so they
leave the room whenever it is up.

Patrol has two known defects, held for the enemy AI sprint (roadmap, Epic 10,
Enemy AI playtest defects):

- **An enemy stops just short of its next point** and never moves on. The
  chaser stands just south of the wall after its first pass; the riders stop
  short of their first point.
- **No route rounds the shaft hole's south-west corner.** The riders' route
  out of the risen room needs it, so on the first rise they stay inside. They
  leave on the second rise, pause at the doorway until the room lowers, then
  cross its roof.

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

1. **Chaser on patrol.** Stay at the spawn and watch through the opening. The
   chaser walks from the enemy spot to just south of the wall, meeting it
   closed, sliding, or open, and stops there (a known patrol defect, above). At
   the wall it waits facing the slab until the opening fits it. It must not
   turn left and right, drift side to side, or lurch in repeated
   stuck-recovery nudges. Give it a full wall cycle (about 30 s).
2. **Chaser behind the sliding wall.** Walk to station 1, 13 m from the enemy
   spot; the chaser wakes as you come within 16 m and heads for you through the
   wall. The wall cycles from spawn, so the chaser may meet it closed, sliding,
   or resting open.
   A moving wall does not hold it back in its route, so it walks into the slab
   and stands pressed against it, facing it, until the gap fits it. It must not
   turn left and right, drift side to side, or lurch in repeated stuck-recovery
   nudges. At the closed wall it stops and waits without jitter or
   foot-shuffling for the rest of the closed wait. Once the opening fits it,
   it passes through and comes at you. Give it a full wall cycle (about 30 s).
3. **Riders in the rising room.** Walk to station 2 (560, 320, 48), within 16 m
   of both riders, and face east. The room is lowered, so the riders cannot
   reach you: their doorway fronts the shaft wall. Expect them to stand inside
   with the lamp on, not pace or shuffle. When the room reaches the top they
   leave through the west doorway onto the arena and chase you. They must ride
   up with their feet planted (idle pose, no walk cycle), not slide or sink into
   the floor. Stand on the roof or inside the room and they stop at the arena
   floor's edge: an enemy routes to the floor under a target standing on a
   mover, so it follows only once you step off.
