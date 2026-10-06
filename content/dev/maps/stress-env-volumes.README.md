# stress-env-volumes — environment-volume stress and test-zone map

A zoned stress-warren variant for the E24 water and environment-volume work
(`context/plans/drafts/E24--water-and-environment-volumes`). It carries the
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

Until E24 lands, prl-build drops `fluid_volume` and `gravity_volume` as
unrecognized brush entities: they appear in the `Entity classnames:` log line
and in `Entity brushes:`, and nothing else. The two maps compile to
byte-identical `.prl` files (verified with `cmp`), so cell, portal,
static-collision and navmesh output are identical by construction. Once E24
lands the same `.map` gains behavior with no regeneration.

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
cmp content/dev/maps/stress-env-volumes{,-baseline}.prl   # identical until E24
```

Long bakes are intended: this map exists to stress the baker as well as the
runtime. Measured bake (dev profile `prl-build`, `--no-cache`, Intel Mac,
2026-10-05): 60–75 s wall per map on a quiet machine, and ~155 s with the
machine busy (load average ~20). The volume map's ~1,200 extra (dropped)
brushes add 1.5–5 s of parsing. The stage shares hold across both regimes:
the lightmap bake dominates (21–25 s quiet, 54 s busy, ~35%), then the cell
residency set (14–16 s quiet, 34–40 s busy, ~22–26%), the navmesh (7–10 s
quiet, 16–22 s busy) and the SH volume (5–13 s). Output: 9,205 cells, 9,362
portals, 27,770 triangles, 1,281 lights (845 runtime-loaded), an 86 MB `.prl`.
The 710 `AlphaLights: … inside a solid leaf` warnings come from the warren's
reserved-corridor hallway spots, not from this map's additions.

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

Keep the window in front and check `pgrep -x ScreenSaverEngine` stays empty.
Read the `[CpuTiming]` lines after warmup. Particle cost is under `render_prep`
as `particle_emit` (emitter bridge spawns) and `particle_sim`
(`particle_sim::tick`). Sim stages sum over the frame's fixed ticks: divide by
`ticks` for per-tick cost.

First run (2026-10-05, Intel Mac, release + dev-tools, mixed pose, 120-frame
windows, steady state = windows 3 and later):

| Metric | Baseline | 1,000 volumes |
|---|---|---|
| frame `total` avg | 88 ms | 109 ms (drifted to an 11-tick regime late in the run) |
| `ticks` per frame | 5.3 | 6.5 |
| `sim_tick` per tick | 11.8 ms | 12.9 ms |
| `sim_ai` per tick | 7.4 ms | 8.2 ms |
| `sim_steering` per tick | 3.6 ms | 3.7 ms |
| `sim_movement` per tick | 0.026 ms | 0.028 ms |
| `particle_emit` per frame avg / max | 0.29 / 2.1 ms | 0.35 / 2.0 ms |
| `particle_sim` per frame avg / max | 4.5 / 7.0 ms | 4.9 / 9.7 ms |

The `.prl` files are identical, so the gap is run-to-run drift, not volume
cost. AI cost grows over a run as agents with an open aggro gate converge, so
compare windows at matched elapsed times. Until AI cost is bounded, the frame
is CPU-bound in the fixed-step loop (~5 ticks per frame at ~11 fps). That
upstream cost swamps any environment-resolution signal.
