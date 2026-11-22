# area-lights-ab — area light against a template grid

Two maps with the same rooms and entities, lit two ways:

- `area-lights-ab-panel.map`: each glowing panel is a `light_area`.
- `area-lights-ab-grid.map`: each panel is plain glowing glass in worldspawn
  plus a grid of `light` points under it, one per square metre of glass, the
  way template lights (ericw-tools `_surface`) stand in for a panel.

Bake both and flip between them to compare.

## Stations

Coordinates are TrenchBroom editor units, Z up. The player spawns at
`(600,48)` facing west.

| Station | Where | What to see |
|---|---|---|
| Skylight | main room ceiling, glass `x 16..176, y 112..272`, 16 units off the west wall | Panel: an even pool, a soft wall wash. Grid: hot spots on the floor and a ripple on the west wall at each point's foot. |
| Reach | corridor through the dogleg in the north wall; 1 m² panel at its east end, `light 100` | Panel: the light fades out about 370 units from the panel with no visible edge. Grid: one point light with the same reach. |
| Entities | a `target_dummy` under the skylight, one mid-room, a `reference_enemy` near the doorway | Under the panel they read as lit like the walls beside them. The enemy attacks. |

## Grid values

- Skylight: 16 points, `light 300`, `delay 2`, `_falloff_range 630`. Far from
  the panel, a 1 m² `light_area` at `light 300` matches a `light 300`,
  `delay 2` point, and reaches about 630 units.
- Corridor: one point, `light 100`, `delay 2`, `_falloff_range 370`, the
  panel's derived reach.
- Points sit 12 units below the glass. The ericw-tools offset is 2 units, but
  a 0.25 m soft emitter that close sinks about 40% into the glass and the
  compiler warns.
- The grid glass glows by its texture strength; the panel glass glows at
  `_glow × _color`. Compare the room light, not the glass.
- Point lights draw specular highlights and a `light_area` draws none. The
  16 overlapping points also fight over specular mask channels, and the grid
  bake warns about it on many faces.

## Compile and run

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/area-lights-ab-panel.map --no-cache
cargo run -p postretro-level-compiler -- content/dev/maps/area-lights-ab-grid.map --no-cache
cargo run -p xtask -- run --features dev-tools -- content/dev/maps/area-lights-ab-panel.prl
```

On a debug build each bakes in seconds: the panel map in about 4 s, the
grid in about 9 s.
