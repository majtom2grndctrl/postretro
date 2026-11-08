# Mover Closet Fixture

Measurement and proof surface for **static bounce behind a closed door**. A
bright room and an unlit closet share one doorway, which a thin `kinematic_mover`
door seals at its first waypoint. Intended uses:

1. Leak diagnosis (`mover-light-flow--probe-channel`, Manual row 1). Compare this
   map with a reference build whose door is a static brush. A glow in the
   reference is the sampler's, and exists for every thin static wall. A glow only
   here is the bake's, because every probe bake runs with movers absent.
2. Prove that a closed door keeps bounce out of a dark room, and that opening it
   ramps the glow in with its travel. Verifiable once the mover probe channel
   lands (roadmap, Lighting and GI).

Build a local artifact on demand:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/mover-closet.map -o content/dev/maps/mover-closet.prl
```

Do not commit generated `.prl` files or `baked/materials/` sidecars.

## Layout

TrenchBroom units (1 unit = 0.0254 m, z up). Floor top is z 0.

- **Bright room (south):** x 0–640, y 0–448, ceiling z 320 (16.3 × 11.4 × 8.1 m).
  Holds the player start.
- **Divider:** y 448–512 (1.6 m thick). Its doorway spans x 272–368, z 0–112
  (2.4 × 2.8 m). The divider is deliberately thicker than the 1 m probe spacing,
  so the door is the only thin thing between the two rooms: away from the doorway
  no probe cell can straddle them.
- **Closet (north):** x 160–480, y 512–760, ceiling z 128 (8.1 × 6.3 × 3.25 m).
  It holds no light. Its north wall sets the probe lattice; see below.
- **Pool:** a shallow pit in the closet floor at x 224–416, y 544–688. Its bottom
  is z −24 and the `fluid_volume` `water` surface is z −8 (0.41 m deep). The pit
  starts 0.8 m inside the doorway, so any glow there reaches the water.
- **Fog:** one `fog_volume` fills the closet, with the default `ambient_scatter`,
  so it takes its light from the same probes the closet does.

## Door

| Mover | Brushes | Waypoint 0 | Waypoint 1 | Travel | Speed | `wait_ms` | Mode |
|---|---|---|---|---|---|---|---|
| `closet_door` | 1, x 272–368, y 488–504, z 0–112 | `closet_door_closed` (320, 496, 56) | `closet_door_open` (320, 496, 168) | +z 112 (2.8 m) | 1 m/s | 3000 | `ping_pong`, `start_on_spawn 1` |

The door is 16 units thick (0.41 m), thinner than the probe spacing. It sits 8
units back from the closet's wall face, so it never lies coplanar with the
divider as it slides up into the lintel. One cycle (open, wait, close, wait)
takes about 12 s.

## Probe lattice

Keep the closet's north wall at y 760. The probe grid starts at the geometry's
minimum corner, and in map axes that corner is this wall, so moving it moves
every probe plane along y. At y 760 the planes fall at y 484.4 and y 523.8: a
valid probe 3.6 units from the door's bright face shares a cell with the closet.
That is the worst case for the sampler.

A door is exposed only when a valid plane falls within one spacing of its far
face and still short of its near face. At y 768 a plane falls inside the door
instead, so the plane is invalid in a static-door build, and the reference reads
perfectly black for no good reason. A comparison at that phase proves nothing.
For a door this thick, about 59% of placements are exposed:
(39.37 − 16) ÷ 39.37.

## Lights

Three baked `light` entities, none in the closet.

| Origin | Light | Falloff |
|---|---|---|
| (128, 392, 288) | 400 | 900 |
| (320, 392, 288) | 400 | 900 |
| (512, 392, 288) | 400 | 900 |

No bright-room light has a line of sight into the closet, even with the door
absent, as every bake has it. All three stand 56 units from the divider and 176
above the lintel, so a ray from a light's centre that clears the lintel falls 3.1
units for each unit forward. Even from the far, low edge of the default 0.25 m
emitter, it lands on the doorway floor by y 493, short of the closet. Direct
light on world surfaces belongs to the world-surface channel, so this fixture
must leave the closet with bounce alone. A light moved south of y 392 or down
from z 288 can break this. Check with a capture at `light_term_mask` 8 (static
baked direct only): the closet must read black.

## Stations

Use dev-tools noclip, or walk: the closet is reachable while the door is open.

- **Station 1 — bright room (player start).** (320, 200, 48), yaw 90 (facing +y,
  north). The doorway is centred ahead.
- **Station 2 — closet, facing the door.** (320, 720, 64), yaw 270 (facing −y,
  south). The whole pool and the door's closet face are ahead.
- **Station 3 — closet floor at the door.** (320, 610, 96), yaw 270, pitch −38.
  The pool's near edge, the strip of floor before the door, and the door's foot.

## Reference variants

Four sibling maps share this map's layout, lights, pool, fog, stations and
probe lattice, and differ from it only as below. They are checked in and
hand-maintained: the static-door and no-door variants were generated once from
this map's layout, the kinematic-receiver map is hand-edited from the
static-door one, and the no-door kinematic-receiver map from the no-door one. An
edit to the layout, lights, pool, fog, stations or the north wall at y 760
belongs in all five maps.

- **`mover-closet-static-door.map`** (`--static-door`): the `kinematic_mover` and
  its two waypoints are gone, and the door's brush sits in worldspawn at the
  same place. A glow behind it is the sampler's, and exists for every thin
  static wall. Direction-aware probe visibility tiles every closet probe with a
  blocked lattice neighbour here.
- **`mover-closet-no-door.map`** (`--no-door`): the doorway stays open. The
  open-doorway baseline: per-direction depth must leave it reading as before.
- **`mover-closet-kinematic-receiver.map`**: the static-door reference plus a
  dynamic receiver in station 3's view: a `kinematic_mover` slab (x 296–344,
  y 505–521, z 0–8) resting at its first waypoint on the closet floor just
  behind the door, where the averaged moments glow. Hand-edited from the
  static-door map, not generated.
- **`mover-closet-no-door-kinematic-receiver.map`**: the no-door reference plus a
  fourth light and a promoting receiver, for proving promotion never changes a
  lit fluid. The `doorway_light` (320, 400, 64), light 400, falloff 900, sits
  low behind the doorway, so unlike the other three it lights the pool's far
  part directly; the closet is no longer bounce-only here. A `kinematic_mover`
  slab (x 560–608, y 24–40, z 0–8) rests in the bright room's south-east
  corner: in the doorway light's reach, so it promotes it, but outside every
  line from station 2 through the doorway and south of the light, so it is
  never seen from station 2 and never shadows the pool. The slab sits between
  `// receiver_slab begin` and `end` comments; the capture proof cuts that
  block for its slab-free reference, so keep the markers.

Build any of them the same way as this map, with its own name. Each sits beside this
map, so the compiler finds the same content root.

These are manual, in-engine checks. Compiling the map validates only the
authored content.
