# Movement Feel Fixture

Dev map for E10 agent diagnostics and follow-up steering-feel playtests. The stations live in one connected sealed playspace so a normal launch from the arena `player_spawn` can walk to each fixture area. Build on demand:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/movement-feel.map -o content/dev/maps/movement-feel.prl
```

Do not commit the built `.prl` unless a later workflow explicitly asks for compiled dev-map artifacts.

## Stations

- **Pillar wedge**: west-side pillar and offset cheek walls. Use for stuck-recovery and tangent-recovery repros around concave approaches.
- **Corridor corners**: south-side 90-degree obstacle run. Use for waypoint snap, bounded turn-rate, and lookahead evaluation.
- **Straight run**: east-side long lane with low side rails. Use for acceleration/deceleration and walk-cycle reading.
- **Arena ring**: north-side open area with the `player_spawn` centered and 8 `reference_enemy` spawns on a 432 Quake-unit rim. At 1 Quake unit = 0.0254 m, that is about 10.97 m, inside the reference enemy 16 m detection range, so the wave should actively chase and produce path, velocity, and destination overlay data.
- **Narrow doorway**: northeast barrier with a 35 Quake-unit opening. That is about 0.89 m, just over the default 0.8 m nav-agent diameter.

## Diagnostics Check

With `dev-tools`, enable the agent overlay (`Alt+Shift+A`) and navmesh overlay (`Alt+Shift+N`) on this map. The arena wave should draw per-agent path corridors, waypoint markers, velocity vectors, and destination markers simultaneously.

The expected debug-line budget remains far below `MAX_DEBUG_SEGMENTS = 256 * 1024`: the 8-agent wave emits only a few dozen segments per agent, plus the navmesh region/portal overlay. If overflow warnings appear, treat that as a regression in emitted segment volume rather than expected fixture behavior.

## Verification Status

Verified with:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/movement-feel.map -o content/dev/maps/movement-feel.prl
```

The generated `.prl` is a local build artifact and is not committed by default.
