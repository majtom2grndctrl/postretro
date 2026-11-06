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
- **Arena ring**: north-side open area with the `player_spawn` centered and 4 `reference_enemy` spawns on a 432 Quake-unit rim. At 1 Quake unit = 0.0254 m, that is about 10.97 m, inside the reference enemy 16 m detection range, so the wave should actively chase and produce path, velocity, and destination overlay data.
- **Crossfire raiders**: three low-tolerance `crossfire_raider` spawns in the northwest corner, about 20 m from the arena wave. They hold their own ground and reach the arena as a group, and they can still take damage in the crossfire tests.
- **Rifle troopers**: three `rifle_trooper` spawns in the southeast corner, faction `rifle.troopers`.
- **Rifle warriors**: three `rifle_warrior` spawns in the southwest corner, faction `rifle.warriors`. Each squad is hostile to the other and to the raiders, and both are mesh-only characters bound to the shared clip library.
- **Pose fixture enemy**: a `pose_fixture_enemy` beside the player start. It uses the content-facing copy of the model crate's three-joint `joint_zones` test fixture. Its no-op animation clip lets its AI target acquisition drive E21 aim pose inputs and the fixture's `poseMask`/`aimBendWeight` extras; the `hand_r` socket carries the rigid attachment marker so it visibly follows the bent leaf joint. It is a deliberately tiny triangle marker, not final character art.
- **Narrow doorway**: northeast barrier with a 35 Quake-unit opening. That is about 0.89 m, just over the default 0.8 m nav-agent diameter.

## Faction Layout

Each faction starts in its own area: the unfactioned arena wave around the player start, raiders northwest, troopers southeast, warriors southwest. Keep open floor between groups when adding placements, so they meet rather than spawn mixed.

## Diagnostics Check

With `dev-tools`, enable the agent overlay (`Alt+Shift+A`) and navmesh overlay (`Alt+Shift+N`) on this map. The arena wave and crossfire cluster should draw per-agent path corridors, waypoint markers, velocity vectors, and destination markers simultaneously.

The expected debug-line budget remains far below `MAX_DEBUG_SEGMENTS = 256 * 1024`: the arena fixtures emit only a few dozen segments per agent, plus the navmesh region/portal overlay. If overflow warnings appear, treat that as a regression in emitted segment volume rather than expected fixture behavior.

## Verification Status

Verified with:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/movement-feel.map -o content/dev/maps/movement-feel.prl
```

The generated `.prl` is a local build artifact and is not committed by default.
