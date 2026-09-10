# Rocket splash-damage demo

This dev fixture puts the player on a fixed eastward aim line with the reference
rocket launcher in the standard loadout. Fire at the `splash_direct` dummy, then
compare the clear cluster: `splash_near`, `splash_mid`, and `splash_clear_far`
take progressively less damage.

`splash_shadowed` is deliberately behind the worldspawn wall. The wall is static
world geometry, not a mover or brush entity, so its segment from the blast point
blocks splash line of sight. The player starts close enough to the direct target
to take point-blank self damage with the reference rocket's `selfDamage: true`.

Build and run it manually:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/splash-damage-demo.map -o content/dev/maps/splash-damage-demo.prl
cargo run -p xtask -- run content/dev/maps/splash-damage-demo.prl
```
