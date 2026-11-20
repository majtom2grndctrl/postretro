# emissive-volumes-demo — scripted glow on emissive volumes

A sealed room for the emissive-volume scripting surface and its manual checks.
The map's data script is `scripts/emissive-volumes-demo.ts`; its Luau twin
`scripts/emissive-volumes-demo.luau` emits identical wire. Every scripted value
is brightness in [0, 1] across each volume's authored `emissive_min`..`emissive_max`.

## Stations

Coordinates are TrenchBroom editor units, Z up. The player spawns at `(224,40)`
facing north.

| Station | Where | What to see |
|---|---|---|
| Bar signs (`bar_sign`) | north wall, `x 24..272` | Bright (`neon/glow_panel`) and dim (`neon/dim_panel`) signs flicker within [0.3, 1] from load. |
| Exit sign (`exit_sign`) | north wall, `x 296..360` | Fades up once over 1.5 s, then holds full glow. |
| Status lamp (`status_lamp`) | north wall, `x 392..424` | A grey `_e` lens authored green (`emissive_color 40 255 60`), pulsing in [0.6, 1]. |
| Breathing panel (`breathe_panel`) | west wall, `y 112..240` | The bright panel texture pulsing in [0, 1] over 3 s. |
| West pad | floor, `x 64..160, y 96..192` | Fires `powerCut` once per load: both bar signs hold brightness 0. |
| East pad | floor, `x 288..384, y 96..192` | Fires `doorLocked` on every entry: the lamp turns red, its pulse running on. |
| North-east pad | floor, `x 288..384, y 232..296` | Fires `doorUnlocked` on every entry: the lamp returns to its authored green. |

## Manual checks

- **Breathing.** The west panel visibly breathes. At its trough (brightness 0)
  it shows its unlit diffuse, with no glow and no bloom halo; at its crest it
  blooms as the bright panel does.
- **Off state.** After the west pad, both bar signs show their unlit diffuse.
- **Lamp colour.** Walk between the east and north-east pads: the grey-`_e`
  lamp switches red and green on each entry, and its pulse never restarts.
- **Flash limiter.** With `accessibility.flashLimiter` on (the default), the
  bar signs' rate-6 flicker presents at most 3 flashes a second.

`emissive_volumes_surface_tests.rs` checks the scripted values (flicker range,
power cut, lamp colour cycle, the panel's 0 and 1, TS and Luau wire). What the
screen shows is the owner's check.

## Compile and run

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/emissive-volumes-demo.map \
  -o content/dev/maps/emissive-volumes-demo.prl
cargo run -p xtask -- run --features dev-tools -- content/dev/maps/emissive-volumes-demo.prl
```

To play the Luau twin, copy the map, point its `data_script` at
`../scripts/emissive-volumes-demo.luau`, and compile the copy.
