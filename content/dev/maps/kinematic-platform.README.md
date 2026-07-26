# Kinematic Visual-Parity Fixture

Manual QA map for E17-B mover material parity, static-light promotion, and
rigid mover shadow casting. It is a sealed east-west corridor. Start at the
west end and move east through the stations.

Build a local artifact on demand:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/kinematic-platform.map -o /private/tmp/kinematic-platform.prl
```

Do not commit generated `.prl` files or `baked/materials/` sidecars.

## Fixture Material

The fixture uses two complete world-material bundles:

- `50-free-textures/concrete_stone_022` supplies the broad Concrete (shininess
  4) reference used at the west station and by promotion mover B.
- `metal/metal_rough_046a` supplies the rough Metal (shininess 64) reference
  used by promotion mover A and its in-range static wall. Its reusable asset
  provenance and source-derived sibling-map details live in
  [`content/dev/textures/metal/SOURCE.md`](../textures/metal/SOURCE.md).

## Stations

- **West dynamic/parity station (x 0–650).** Platform A is docked by default;
  its touch pad starts it. Platform B moves automatically. The north comparison
  wall shares their concrete bundle. Inspect both north-facing faces from the
  north aisle to compare diffuse and normal-map response at matching
  orientation. The explicitly authored `light_dynamic_spot` and
  `light_dynamic` both set `_cast_entity_shadows 1`; the point light covers the
  two movers and the mesh beside platform B.
- **Mover-only promotion station (x 960–1,568).** A rough-Metal mover and a Concrete
  mover cross under one bright static spotlight. No skinned mesh is placed
  here. They make the light promotion-relevant, receive promoted-static
  specular, and cast onto each other. An in-range Metal static comparison wall
  sits immediately north of mover A's track. The floor and wall are deliberately
  not promoted-static shadow receivers.
- **Mixed ranker station (x 2,040–2,464).** A mover and skinned mesh overlap
  three bright static point lights. The lights compete for the fixed two-slot
  promoted-cube budget, so use this station to confirm graceful ranker
  selection rather than a guarantee that all three are promoted.
- **Rotating-carousel station (x 560–720, y -48–48).** The asymmetric Metal
  slab is a pure rotator around its single `carousel_origin` waypoint. It
  starts at 45 deg/s about the authored vertical axis, carries rider yaw, and
  declares a 180 deg/s² spin acceleration to support external/scripted
  rate-command testing via the mover `setSpinRate` command. Its open
  bay has no nearby wall or mover face that can pinch a rider during rotation.

## Manual Checks

1. At the west station, compare a north-facing mover side and the north-facing
   static wall. Their concrete diffuse and bump response should agree under a
   comparable view and light angle.
2. Under the mover-only station's promoted static spotlight, compare rough-Metal
   mover A's subdued, tight material response with the in-range rough-Metal
   static wall. It should match in character, not pixel-for-pixel: the wall
   uses its baked static path while the mover uses the promoted runtime record.
   Compare that response with Concrete mover B's broader one under the same
   light. Move either platform until it leaves the light's influence: its lobe
   must fade with de-promotion, not pop. The floor and comparison wall must not
   receive that promoted-static mover shadow.
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
6. In the open west bay, stand on the carousel for several turns. The slab
   should rotate in place, keep the rider planted, and carry view yaw without
   camera pitch or roll. Do not use the nearby moving-platform tracks as a
   rotation test area; the carousel station is intentionally isolated for the
   displace-only mover policy.

These are visual, in-engine checks. Map compilation only validates authored
content and cannot prove shadow-pool receipt, promotion crossfades, or the
absence of dynamic specular.
