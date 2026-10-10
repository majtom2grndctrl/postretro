# stress-env-volumes — environment-volume stress and test-zone map

A zoned stress-warren variant for the E24 water and environment-volume work
(the E24 water-and-environment-volumes plan). It carries the
warren showcase's full feature set plus ~1,000 `fluid_volume` /
`gravity_volume` brush entities, 64 AI agents and ~10,000 live particles, laid
out in zones so water can be checked alone, beside one feature at a time, and
with everything at once.

| Map | Contents |
|-----|----------|
| `stress-env-volumes.map`          | the zoned fixture: 1,000 volumes |
| `stress-env-volumes-baseline.map` | the same map with zero volumes — byte-identical apart from the trailing volume entities (the stress run's baseline) |

Both maps reference the one data-script sidecar
`stress-env-volumes.generated.ts` (closet spawner reactions and the
script-pulsed animated lights).

prl-build currently ignores unrecognised brush-entity classnames, so the volumes
are inert: they appear in the `Entity classnames:` log line and in
`Entity brushes:`, and nothing else. The two maps compile to byte-identical
`.prl` files (verified with `cmp`), so cell, portal, static-collision and
navmesh output are identical by construction.

The volume entities use the planned E24 vocabulary. `fluid_volume` carries
`fluid` and `priority`; `gravity_volume` carries `gravity` (a straight-down
vector in map axes, m/s²) and `priority`. A few volumes deliberately omit
`priority` to exercise its default of 0. The fixture names two fluids, `water`
and `sludge`; for them to be live the mod manifest must declare both
(`defineFluid`). An undeclared fluid is planned to load dry with a warning.

## Regenerate

```bash
python3 tools/gen_stress_map.py --preset env-volumes \
    -o content/dev/maps/stress-env-volumes.map
python3 tools/gen_stress_map.py --preset env-volumes --env-volumes 0 \
    --data-script content/dev/maps/stress-env-volumes.generated.ts \
    -o content/dev/maps/stress-env-volumes-baseline.map
```

The volume map's printout lists every zone with its `--start-pose`. Knobs:
`--env-volumes N` (0 = baseline), `--emitters N`, `--emitter-rate R`,
`--emitter-lifetime S` (rate × lifetime is one emitter's steady-state live count
and must stay within the 4096 per-emitter cap), `--zones`. Volume shapes,
emitters and zone features live in `tools/stress_environment.py` and
`tools/stress_zones.py`. Each draws from its own RNG stream and volumes are
appended after every other entity, so the volume count never moves anything
else.

## Zones

Grid 8×6×3 (138 rooms plus one NFL arena). The eight single-room zones run
along the top layer's first row. Lift zones are both rooms of each lift shaft.
Every other room is the mixed zone. Poses are engine meters, from a room corner
facing its centre. Pass them as `--start-pose=…`: the `=` form keeps a leading
minus sign from reading as a flag.

| Zone | Cell (i, j, k) | Contents | `--start-pose` |
|------|----------------|----------|----------------|
| water only | (0, 0, 2) | every volume shape (pools, floating blocks, nested, overlapping, slanted, wedge, multi-brush, doorway-spanning); static baked lights only; no enemies, emitters, movers or crates | `114.60,34.95,156.87,45,0` |
| water only | (1, 0, 2) | as above | `114.60,34.95,114.60,45,0` |
| (a) lifts | (0, 2, 0) / (0, 2, 1) | lift 0: carries a dynamic cabin light, its car settles into a pool on the lower floor | `30.07,2.44,156.87,45,0` |
| (a) lifts | (0, 5, 0) / (0, 5, 1) | lift 1: carries a dynamic cabin light, rises through a floating fluid block | `-96.72,2.44,156.87,45,0` |
| (b) animated | (2, 0, 2) | three animated ceiling spots over water, two animated lights (one KVP curve, one script pulse) inside a pool | `114.60,34.95,72.34,45,0` |
| (c) arena | arena | 12 enemies and half the weapon pickups among arena pools | `30.07,2.44,72.34,30,0` |
| (d) particles | (3, 0, 2) | 4 emitters: rising ones start inside a pool, pass a zero-gravity box's top face and a floating block; falling ones drop out of a block into a pool | `114.60,34.95,30.07,45,0` |
| (e) fog | (4, 0, 2) | an axis-aligned (ellipsoid) fog volume over a pool and around a floating block, a yawed plane-bounded fog volume over a pool, a fog lamp, a fog tube | `114.60,34.95,-12.19,45,0` |
| (f) doors | (5, 0, 2) | an action (`use`) door with its trigger volume on the shared doorway, a switch that starts that door, a pool beside the switch, doorway-spanning volumes | `114.60,34.95,-54.46,45,0` |
| (g) crates | (6, 0, 2) | mixed baked/runtime spots, three crate stacks, each standing in a pool with another pool beside it | `114.60,34.95,-96.72,45,0` |
| (h) low gravity | (7, 0, 2) | a room-wide `0 0 -1.62` gravity volume with a pool and a floating fluid block | `114.60,34.95,-138.99,45,0` |
| mixed | everywhere else | mixed baked/runtime lights with crates, 52 enemies, the other weapons, action doors, monster closets with spawners, 36 emitters, fog volumes, static props, a spinning carousel carrying an orbiting dynamic light, and most of the volumes | measurement room (7, 0, 0): `114.60,2.44,-138.99,45,0` |

The mixed measurement room is the mixed room holding the most enemies and
emitters.

## Compile

```bash
prl-build content/dev/maps/stress-env-volumes.map \
    -o content/dev/maps/stress-env-volumes.prl \
    --sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache
prl-build content/dev/maps/stress-env-volumes-baseline.map \
    -o content/dev/maps/stress-env-volumes-baseline.prl \
    --sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache
cmp content/dev/maps/stress-env-volumes{,-baseline}.prl   # identical: the volumes are inert
```

Long bakes are intended: this map exists to stress the baker as well as the
runtime. Use an interactive (non-`--release`) prl-build bake with `--no-cache`.
Stages to watch: the lightmap bake, the cell residency set, the navmesh and the
SH volume. The volume map's extra brushes are dropped, so they add only parse
time. Recorded bake times and output sizes are in the E24
water-and-environment-volumes plan's research notes.

## Measure

CPU stage timing (`context/lib/rendering_pipeline.md` §12) from a windowed
release build with dev-tools, at the mixed measurement pose. Run each map the
same way:

```bash
caffeinate -dimsu env POSTRETRO_CPU_TIMING=1 RUST_LOG=info \
    cargo run -p xtask -- run --release --features dev-tools -- \
    content/dev/maps/stress-env-volumes.prl \
    --start-pose=114.60,2.44,-138.99,45,0
```

Measurement hygiene: keep the window in front and check
`pgrep -x ScreenSaverEngine` stays empty, because a screen saver distorts the
frame regime. Read the `[CpuTiming]` lines after warmup. Particle cost is under `render_prep`
as `particle_emit` (emitter bridge spawns) and `particle_sim`
(`particle_sim::tick`). The `sim_*` stages sum over the frame's fixed ticks: divide by `ticks` for
per-tick cost. `particle_emit` and `particle_sim` run once per rendered frame
under `render_prep`, so do not divide them.

Metrics to compare between the two maps: frame `total`, `ticks` per frame,
`sim_tick`, `sim_ai`, `sim_steering`, `sim_movement`, `particle_emit` and
`particle_sim`. The `.prl` files are identical, so any gap while the volumes are
inert is run-to-run drift, not volume cost. AI cost grows over a run as agents
with an open aggro gate converge, so compare windows at matched elapsed times.
AI and particle cost dominate this fixture's frame: the fixed-step loop is
CPU-bound, and that upstream cost can swamp any environment-resolution signal.
The E24 water-and-environment-volumes plan's research notes hold the recorded
numbers.
