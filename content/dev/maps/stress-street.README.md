# stress-street — occluder classifier proving ground

A generated city street built to exercise occluder-first partitioning
(`context/plans/done/occluder-first-partition/`). It is the classifier's
first consumer: the sweep reads which of its buildings, decks, barriers and
small brushes occlude, and the owner plays it. Every dimension below is world
metres; the generator converts to map units (1 u = 0.0254 m) at emission.

| File | Contents |
|------|----------|
| `stress-street.map`          | the street: 1,190 world brushes plus 23 lamp-post linked groups |
| `stress-street.generated.ts` | data-script sidecar named by worldspawn `data_script`; pulses the two tagged neon lights |

Layout and geometry live in `tools/stress_street.py`; `tools/gen_stress_map.py`
writes the map. The layout is fixed: no seed, and the warren knobs do not apply.

## Regenerate

```bash
python3 tools/gen_stress_map.py --preset street \
    -o content/dev/maps/stress-street.map
```

The sidecar is written beside the map as `stress-street.generated.ts`. The
printout lists brush, deck, side-street and lamp counts and the sweep poses.
The generator test checks the committed files match the generator's output, so
regenerate after any change to `stress_street.py`:

```bash
python3 -m unittest discover -s tools/tests -p test_gen_stress_map.py
```

## Layout

The street canyon is 12 m between facade A (left of travel) and facade B. It
leaves the plaza heading east, turns left (north) at x = 88 m, then right (east)
at y = 74 m, and dead-ends at the hull's east wall. The whole map sits inside a
140 × 84 m hull under a ceiling slab 30 m up, so it is sealed by six slabs. The
space above every roof is one open-air region.

| Piece | Where | Dimensions |
|-------|-------|------------|
| Promenade | the canyon floor, plaza to hull | 12 m wide; 5.5 m clear under the elevated street, a 4 m light well along facade A |
| Elevated street | slab hanging off facade B | 8 m of the 12 m canyon (67 %), top 6.0 m up, 0.5 m thick; slabs 52, 44 and 40 m long |
| Upper deck (opposite direction) | slab hanging off facade A | 7 m of the 12 m canyon (58 %), top 11.0 m up; slabs 47, 44 and 45 m long; 4.5 m headroom over the elevated street |
| Ramps | plaza → elevated street; elevated street → upper deck | 16 m long, about 20°, 0.2 m lip at the foot |
| Abutting row | all of facade A, plaza to hull, round both corners | 9 buildings (10 frontages, the corner piece has two) touching end to end, one strip cluster |
| Side-B buildings | facade B, broken by side streets | 7 buildings, 8 frontages |
| Side streets | three gaps in facade B | 8 m wide, 18 m deep to a block or the hull |
| Barriers | one in each side street, 6 m in | full width, 0.5 m thick, 3.0 m tall |
| Deck guards | elevated street's edge across each side-street gap | full width, 0.3 m thick, 3.0 m tall |
| Free-standing building | in the 48 × 48 m plaza | 16 × 24 m footprint, 16 m tall, at least 8 m clear of every other solid |
| Lamp posts | promenade, both decks, plaza | 23 linked-group instances |
| Small exempt brushes | promenade and elevated street | curb 0.3 × 0.3 m; deck columns 0.8 m square, ground to soffit, every 10 m; railing 0.15 × 1.0 m on the elevated street's well edge |

**Facades.** Each facade is a slab 0.4 m thick of narrow strips around
see-through windows. A storey is a 1.0 m spandrel band over a 1.5 m row of
windows between piers. The bands run the frontage's full length, the piers are
0.3–1.0 m wide, and a top band caps the building. Every strip stays under the
1.2 m floor on one of its two broad axes, so each is exempt alone. A building is
6, 7 or 8 storeys (16, 18.5 or 21 m). The decks sit inside spandrel bands, so no
deck cuts a window. Behind most facades is a room 3 m deep, closed by 0.3 m
end walls (0.6 m between neighbours) and a roof, so the windows see through. The two corner pieces are
solid behind their strips (recessed windows); they carry the facade round the
abutting row's inner corner and side B's inner corner.

**Lamp posts.** Each is a `func_group` with `_tb_type _tb_group`, a unique
`_tb_id`, the shared `_tb_linked_group_id`
`{5f0c3e2a-7b1d-4c8e-9a6f-2d4b8e1c0a71}` and its own `_tb_transformation` (a
quarter-turn and translation of one base lamp). It holds three brushes: a 0.2 m
pole 5 m tall, a 1.5 m arm and a head. A baked point `light` 0.25 m under the
head names the group through `_tb_group`. Parse keeps each group as a
`MapAssembly` and flattens its brushes into the static world.

**Lights.** The lamps' 23 baked lights (12 m falloff) and two script-pulsed
neon lights by the free-standing building's east face. The neon lights carry
the generator's script-pulse tag, which the sidecar drives.

## Against the classifier constants

Starting constants: 1.2 m floor, 20 m absolute length or half the open span,
2.5 m see-over height, 50 % cluster coverage. Several pieces sit on either
side of one on purpose, so the sweep is informative.

| Piece | Measure | Constant it straddles |
|-------|---------|-----------------------|
| Deck slabs | 40–52 m long; 8 and 7 m of a 12 m canyon | pass the absolute length and half the span |
| Facade frontages | 6–26.4 m; only B1 (26.4 m) passes 20 m from the street | absolute length; their rooms' inner faces see the whole room |
| Facade coverage | 61–72 % for most; B5 (glassy, 6 m, isolated between a side street and the hull) 49.4 % | 50 % coverage |
| Free-standing building | west and east faces 23.2 m; north and south 16 m, a third of the plaza's 48 m | absolute length, against half the open span |
| Barriers and guards | 3.0 m tall, over the 2.76 m a player reaches by jump plus vault; full side-street width | see-over height |
| Curb, railing, columns, lamp parts | each under 1.2 m on a broad axis | floor |

The abutting row is one touching cluster from the plaza to the hull, through
an inner and an outer corner, so it is judged in near-planar sections. Side B's
frontages also touch through its corner piece and its outer corner, except where
a side street breaks them.

## Compile and play

```bash
prl-build content/dev/maps/stress-street.map \
    -o content/dev/maps/stress-street.prl \
    --sh-probe-spacing 4.0 --lightmap-density 0.25
```

The hull holds about 350,000 m³ of air, so the 1.0 m default probe spacing bakes
a dense SH grid. 4.0 m keeps the bake modest.

Sweep poses (engine metres; pass as `--start-pose=…`, where the `=` keeps a
leading minus sign from reading as a flag):

| Pose | `--start-pose` |
|------|----------------|
| plaza (the player start), facing up the street | `-26.00,0.81,-40.00,0,0` |
| promenade, after the first corner | `-32.00,0.81,-90.00,90,0` |
| elevated street, after the first corner | `-30.00,6.81,-96.00,90,0` |
| upper deck, after the second corner | `-70.00,11.81,-100.00,0,0` |

```bash
cargo run -p xtask -- run --release --features dev-tools -- \
    content/dev/maps/stress-street.prl --start-pose=-32.00,0.81,-90.00,90,0
```
