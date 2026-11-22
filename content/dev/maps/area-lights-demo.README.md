# area-lights-demo — scripted area lights

A sealed room for the area-light scripting surface and its manual checks. The
map's data script is `scripts/area-lights-demo.ts`; its Luau twin
`scripts/area-lights-demo.luau` emits identical wire.

## Stations

Coordinates are TrenchBroom editor units, Z up. The player spawns at `(256,40)`
facing north.

| Station | Where | What to see |
|---|---|---|
| West skylight (`skylight`) | ceiling, glass `x 96..160, y 96..160` | A `light_area` panel, no `_animated` key. Its glass and the floor under it pulse together within [0.3, 1] every 0.9 s from load. |
| East skylight (`skylight`) | ceiling, glass `x 352..416, y 96..160` | The same panel, still carrying `delay`, `_light_size`, `_cone` and `mangle` from a converted `light_spot`. Each warns once at build and changes nothing. |

Each panel is a concrete frame brush and a glass brush whose underside alone
carries `neon/glow_panel`, so only the underside emits and glows.

## Manual checks

- **Glow with the room.** Each pane's glow and the floor beneath it brighten
  and dim on the same frame.
- **Warm glass.** The panes glow the lights' warm `_color`, not the texture's
  white.

`area_lights_surface_tests.rs` checks the scripted values: each panel's
animated reservation and its glow on that slot, `lightType` `"Area"`, the
centroid position, the pulse, the inert-key snapshot, and TS and Luau wire.
What the screen shows is the owner's check.

## Compile and run

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/area-lights-demo.map \
  -o content/dev/maps/area-lights-demo.prl
cargo run -p xtask -- run --features dev-tools -- content/dev/maps/area-lights-demo.prl
```

To play the Luau twin, copy the map, point its `data_script` at
`../scripts/area-lights-demo.luau`, and compile the copy.
