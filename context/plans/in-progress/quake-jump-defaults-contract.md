# Quake jump defaults — contract

## Goal

The default jump hangs 1.83 s and peaks 4.1 m (9 m/s under −9.81 m/s²). Adopt Quake's jump as the engine default: 800 u/s² gravity and 270 u/s jump speed, converted at the map scale of 0.0254 m per unit. One shared world gravity; no per-character gravity multiplier.

## Decisions

- **World default gravity is −20.32 m/s².** It is `DEFAULT_WORLD_GRAVITY_MPS2`; the compiler writes it when worldspawn omits `initialGravity`. Particles, buoyancy, knockback arcs and agents all fall faster, as in Quake.
- **Player `jumpVelocity` is 6.86 m/s.** Apex 1.16 m, airtime 0.675 s.
- **Dev maps stop pinning gravity.** Every `"initialGravity" "-9.81"` and campaign-test's `-30` is deleted, so maps follow the default. Pinning the old Earth value was the FGD default leaking into map files.
- **Swim sink references the default level gravity, not 9.81.** `sinkSpeed` means the drift at default gravity; it keeps its tuned feel.
- **`fluidJumpVelocity` scales to 9.35 m/s.** `6.5 × √(20.32 / 9.81)` keeps the tuned rise, so the clearance rule passes for the same ledges.
- **E24 test-map volumes keep their tuned ratios against the new default.** Updraft push 15 → 25.5 (same 5.19 m/s² net lift); low-gravity room −3 → −6.8 (about a third of default). Stress-map gravity volumes are absolute variety and stay.

## Invariants

- Gravity is m/s², negative = downward. Map scale 1 u = 0.0254 m.
- Test literals of −9.81 that construct explicit inputs stay; only defaults and their documentation change.
- No PRL format, wire, or tuning-epoch change: the default is a compiler-side value written into the section.

## Acceptance

- `grep -rn '"initialGravity"' content/dev/maps/*.map` → empty.
- `cargo test -p postretro-level-compiler --bin prl-build initial_gravity` → passes, nonzero count.
- `cargo test -p postretro-physics swim` → passes, nonzero count.
- Manual: campaign-test jump feels like the old −30 override, slightly taller; E24 fluid jump reaches the 24 u ledge, updraft lifts from standing.
