# Kinematic Visual-Parity Fixture

Manual QA map for E17-B mover material parity, static-light promotion, and
rigid mover shadow casting. It is a sealed east-west corridor. Start at the
west end and move east through the stations.

Build a local artifact on demand:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/kinematic-platform.map -o content/dev/maps/kinematic-platform.prl
```

Do not commit the generated `.prl` or `baked/materials/` sidecars. Remove the
local artifact after the run when it is no longer useful.

## Fixture Material

The movers and comparison wall use
`50-free-textures/concrete_stone_022`. Its diffuse, `_n`, and `_s` sources are
present under `content/dev/textures/50-free-textures/`, so the fixture exercises
the complete world-material bundle.

There is currently no tracked `metal_*`, `glass_*`, or `neon_*` source bundle
with both `_n` and `_s` siblings. The available fixture therefore validates the
Concrete (broad, shininess 4) promoted-static lobe, but **cannot demonstrate the
tighter Metal (shininess 64) comparison**. Add an authored Metal source bundle
before claiming that part of the manual acceptance check.

## Stations

- **West dynamic/parity station (x 0–650).** Platform A is docked by default;
  its touch pad starts it. Platform B moves automatically. The north comparison
  wall shares their concrete bundle. Inspect both north-facing faces from the
  north aisle to compare diffuse and normal-map response at matching
  orientation. The explicitly authored `light_dynamic_spot` and
  `light_dynamic` both set `_cast_entity_shadows 1`; the point light covers the
  two movers and the mesh beside platform B.
- **Mover-only promotion station (x 960–1,568).** Two adjacent concrete movers
  cross under one bright static spotlight. No skinned mesh is placed here.
  They make the light promotion-relevant, receive the promoted-static broad
  specular lobe, and cast onto each other. The floor is deliberately not a
  promoted-static receiver.
- **Mixed ranker station (x 2,040–2,464).** A mover and skinned mesh overlap
  three bright static point lights. The lights compete for the fixed two-slot
  promoted-cube budget, so use this station to confirm graceful ranker
  selection rather than a guarantee that all three are promoted.

## Manual Checks

1. At the west station, compare a north-facing mover side and the north-facing
   static wall. Their concrete diffuse and bump response should agree under a
   comparable view and light angle.
2. Under the mover-only station's promoted static spotlight, observe the
   concrete specular lobe and the mover-to-mover shadow. Move with either
   platform until it leaves the light's influence: the lobe must fade with
   de-promotion, not pop. The floor must not receive that promoted-static mover
   shadow.
3. At the west station, the dynamic spot and point lights may brighten movers
   and the wall diffusely, but must not create a specular glint. Watch a moving
   platform shadow sweep across the floor under both dynamic lights, and across
   the nearby mesh and other mover under the dynamic point light.
4. Return to the mover-only station and confirm the same mover-to-mover cast
   under the promoted static light. At the mixed station, verify the mover and
   mesh remain correctly lit/shadowed as the existing ranker selects its two
   cube slots from the three competing lights.
5. Before touching platform A's start pad, confirm that its docked geometry is
   still a stable dynamic-light occluder. After starting it, its cast shadow
   must track every frame and remain stable again at an endpoint.

These are visual, in-engine checks. Map compilation only validates authored
content and cannot prove shadow-pool receipt, promotion crossfades, or the
absence of dynamic specular.
