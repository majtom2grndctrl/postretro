# Probe Tiles Convex Room Fixture

Negative control for direction-aware probe visibility (`ProbeDepthTiles`, id
59). One sealed box room with one light: no wall is thinner than the probe
spacing, so no lattice segment between two valid probes crosses a wall. Its
wall-hugging probes still take tiles (see Probe lattice), so the proof is
visual: `capture_probe_depth_tiles` renders it with and without id 59 and
expects no pixel to darken by more than 2 levels.

Build a local artifact on demand:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/probe-tiles-convex-room.map -o content/dev/maps/probe-tiles-convex-room.prl
```

Do not commit generated `.prl` files or `baked/materials/` sidecars.

## Layout

TrenchBroom units (1 unit = 0.0254 m, z up). Floor top is z 0.

- **Room:** x 0–512, y 0–384, ceiling z 256 (13.0 × 9.8 × 6.5 m).
- **Walls, floor and ceiling:** each 64 units thick (1.63 m), thicker than the
  default 1 m probe spacing, so a valid probe's lattice neighbour across one is
  always inside it or outside the hull, and invalid.
- **Light:** one baked `light` at (256, 192, 192), `light` 300, `_falloff_range`
  640.
- **Player start:** (256, 64, 48), yaw 90.

## Probe lattice

The probe grid starts at the geometry's minimum corner in engine axes, which
here is an inner corner of the room, so probe planes lie on the north wall
(y 384), the floor and the east wall (x 512). On the other sides the nearest
planes fall 4.8 mm from the west wall (x 0.19), 0.75 m from the south wall and
0.5 m below the ceiling. Moving a wall moves that phase.

The 9 probes 4.8 mm from the west wall each take a depth tile: their texels
facing along the wall mix wall hits near 0 m with open-room depth, so their
visibility toward a neighbour along the wall changes by about 0.105, past the
selection threshold. That is correct per-direction depth, not a leak; probe
relocation (roadmap) moves such probes off the wall, and their tiles with them.
