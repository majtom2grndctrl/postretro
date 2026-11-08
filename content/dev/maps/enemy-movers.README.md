# Enemy-Movers Fixture

Manual QA map for how enemies and corpses meet movers: riding a lift, being
pushed by a sweeper, being crushed against a wall, and a corpse on a lift. One
straight hall with a bay per case, small enough to bake quickly (11 world
brushes, 4 movers, 13 baked lights).

Build a local artifact on demand:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/enemy-movers.map -o content/dev/maps/enemy-movers.prl
```

Do not commit the `.prl` or any `baked/materials/` sidecars. The `.prl` is not
checked in.

Enemy routing over doors and lifts needs a map built with the current
`prl-build`. A stale `.prl` loads with no door or lift routing and logs one
warning.

The `.map` was written by a one-off script that is not checked in. Edit it
directly or in TrenchBroom.

## Layout

One sealed hall, x 0–2048, y 0–512, ceiling at z 320 (map units, 1 unit =
0.0254 m). You spawn at the west end (64, 256, 48), facing east. Bays run west
to east, 18–50 m from the start.

Every enemy is a `reference_enemy` (tagged `enemy` plus a bay tag). It idles
until the player comes within about 16 m (about 630 units), then chases and
melees. **Watch each bay from beyond that range first** to see the enemies
before they wake, using the straight hall's sightline or dev-tools noclip. Once
you walk into range they chase you, which is also the point of the lift check.
Reload the map to reset a bay.

| Bay | x | Mover | Policy | Enemies |
|---|---|---|---|---|
| 1 lift | pit 704–896, ledge 896–1152 | `em_lift`, `ping_pong`, 1.5 m/s, 4 s wait, rises 96 u (2.4 m) | `displace` | Two on the lift (`em_lift_rider`), one on the floor beyond the ledge (`em_lift_chaser`) |
| 2 sweeper | 1280→1536 | `em_sweeper`, 384 wide, `ping_pong`, 4.5 m/s, 1.5 s wait | `displace` | Two in its course (`em_sweeper_victim`) |
| 3 open crusher | 1728→1920, y 48–208 | `em_crusher_open`, `ping_pong`, 1 m/s, 3 s wait | `crush`, 100 dmg / 400 ms | One ahead of it (`em_crusher_open_victim`) |
| 3 wall crusher | 1728→2032, y 304–464 | `em_crusher_wall`, `ping_pong`, 1 m/s, 3 s wait | `crush`, 100 dmg / 400 ms | One between it and the east wall (`em_crusher_wall_victim`) |

All movers start on spawn. The lift's slab fills its pit and sits flush with the
floor at the bottom, and flush with the ledge top at the top. The ledge is 2.4 m
high, too tall to jump, so the lift is the only way up. The crusher damage
uses the dev mod's default damage type.

## Lights

Faint point lights are the ambient; one bright spot picks out each bay, and
four angled accent spots throw pockets of brighter light. Every point light
reaches most of the hall, so on large faces the lowest-ranked lights lose their
shadowmask specular channel and the compiler warns once per light and face:
mostly the points, and three spots on a few small patches. Those faces show no
static highlight from the losing lights. Diffuse lighting is unaffected.

| Light | Origin | Intensity | Falloff | Lights |
|---|---|---|---|---|
| `light` × 5 | (400, 900, 1300, 1700, 1980; 256, 256) | 10 | 1600 | Ambient for the hall, alternating warm and cool |
| `light_spot` (cone 22/42, angles −39 39 0) | (560, 96, 300) | 350 | 700 | The lift and the ledge edge |
| `light_spot` (cone 25/48, angles −42 317 0) | (1180, 480, 300) | 300 | 750 | The sweeper's course |
| `light_spot` (cone 18/36, angles −47 22 0) | (1640, 40, 300) | 300 | 600 | The open crusher lane, cool |
| `light_spot` (cone 16/34, angles −52 335 0) | (1820, 470, 300) | 350 | 600 | The wall crusher's pin against the east wall, red |
| `light_spot` (cone 14/30, angles −46 294 0) | (240, 470, 300) | 250 | 600 | A warm pool by the start |
| `light_spot` (cone 14/30, angles −18 105 0) | (600, 40, 290) | 250 | 700 | A cyan rake up the north wall |
| `light_spot` (cone 16/32, angles −46 288 0) | (1000, 490, 300) | 250 | 500 | A cool pool on the ledge top |
| `light_spot` (cone 12/26, angles −22 288 0) | (1500, 440, 300) | 300 | 700 | An amber rake down the south wall, between the sweeper and the crushers |

## Checks

These are manual, in-engine checks. Compiling the map validates only the
authored content. Each one also says how to reach it.

1. **Riders do not foot-slide (bay 1, host view).** From the start, look east
   at the pit (18.7 m away, outside wake range). The two riders stand on the
   lift and rise with it. Expect their feet planted in the idle pose, no walk
   cycle and no sliding relative to the slab, through the rise, the 4 s wait at
   the top, and the descent. They are carried back down when the lift returns.
   The check needs the host view; a joined client draws riding enemies slightly
   off the slab, which is expected.
2. **A chaser boards and rides (bay 1).** Walk onto the lift while it is docked
   at the bottom and ride it to the ledge. Stand on the ledge. The chaser at
   (1024, 448) wakes, cannot climb the ledge, walks to the pit, boards the lift
   while it is docked flush with the floor, stays on through the wait, and rides
   up. Expect no shuffling on the slab during the wait, and it steps off onto
   the ledge to reach you. It must never walk into the open pit while the lift is
   up.
3. **A sweeper pushes them (bay 2).** From x 600 or so, outside range, watch the
   two enemies in front of the slab. When it sweeps east it pushes them along
   ahead of it, with open floor beyond, and they take no damage. At 4.5 m/s it
   outruns them (they walk at 3 m/s), so it shoves them the length of its sweep
   and does the same to you. When it returns
   they are left standing clear. Once you walk into range they chase you instead,
   stepping off the slab's course.
4. **A crusher damages only against a wall (bay 3).** The open crusher (south
   lane) pushes its enemy along 192 u with nothing behind it: expect no
   damage. The wall crusher (north lane) pushes its enemy the last 32 u into
   the east wall: expect damage every 400 ms until it dies, and the crusher
   to keep moving to the wall. Watch from x 1100 or farther west to stay out of
   range.
5. **A crushed corpse never holds the crusher (bay 3).** The enemy the wall
   crusher killed stays pinned against the wall while the slab sits on it for the
   3 s wait. The crusher never stops or backs off for it, and keeps its cycle.
   When the slab backs off, the corpse is left at the wall (the dev mod removes
   it after about four seconds).
6. **A corpse rides a lift (bay 1).** Wait until the lift is at the bottom with a
   rider on it, then shoot that rider dead with the starting weapon. Expect the
   corpse to stay on the slab and rise with it, and the lift to carry on without
   holding or reversing. The dev mod removes a corpse about four seconds after
   death, so shoot it just as the lift is about to leave the bottom.

## Notes

- This map has no door, so it does not exercise a door gating a route;
  `mover-setpiece` does (the sliding wall).
- At the top of its travel the lift leaves an open pit 16 units deep. Enemies on
  the floor must never walk into it.
