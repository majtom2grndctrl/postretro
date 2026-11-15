# Task 9: bake fidelity, results

Measured 2026-11-11 on the Intel i9 MacBook Pro (`-j 6` under `nice`), on this branch at 96e7b9644 plus the task 9 working tree. Every build is this tree's cargo `--release` `prl-build` (`prl-build <map> --no-tui -v -j 6`), run once per scenario, back to back.

## What changed

- `prl-build --bake-fidelity low|high`, default `low`. `low` caps every mover's posed bounces at 1 (`light_bounces 0` stays 0); `high` bakes each mover's own (`light_bounces` clamped to id 34's). `--release` always bakes `high`; naming `--bake-fidelity` beside it fails at parse, before any stage, naming both flags. It is a quality flag, so it suppresses the TUI form.
- The mode reaches the bounce half as an explicit stage input (`Args::bake_mode()` → `BouncePlanInputs.mode`), read from the flags, never from whether a stage cache opened.
- A `low` build reads each pose's `high` entry first, then its own count's, and bakes at its own on a miss. Each pose carries the count it resolved at, served or baked; the bake and its memo write use that count.
- Id 34's bake time (layers to gather, without the movers) is recorded under an `sh_base_bake_time` entry keyed on id 34's volume key whenever id 34 bakes with a cache open; a volume hit reads it.
- The Build Summary gains a fidelity block (sample below).

## Timing (wall, seconds) and id-57 bytes

| Map | Scenario | Wall | SH Bake stage | id 57 |
|---|---|---|---|---|
| mover-closet | `low` cold (`--no-cache`) | 3.10 ¹ | 0.32 | 54,096 |
| mover-closet | `high` cold (`--release`) | 2.26 | 0.71 | 56,904 |
| mover-closet | `low` fill / warm | 1.99 / 0.23 | | 54,096 |
| mover-closet | `high` fill / warm | 2.40 / 0.23 | | 56,904 |
| mover-closet | `low` after `high` (cache served) | 0.27 | 0.01 | 56,904 |
| mover-setpiece | `low` cold | 16.95 | 4.02 | 8,740,563 |
| mover-setpiece | `high` cold | 23.17 | 10.59 | 9,144,759 |
| mover-setpiece | `low` fill / warm | 17.26 / 1.06 | — / 0.16 | 8,740,563 |
| mover-setpiece | `high` fill / warm | 23.98 / 1.08 | | 9,144,759 |
| mover-setpiece | `low` after `high` | 1.13 | 0.21 | 9,144,759 |
| enemy-movers | `low` cold | 3.65 | 2.43 | 3,070,130 |
| enemy-movers | `high` cold | 7.87 | 6.57 | 3,194,622 |
| enemy-movers | `low` fill / warm | 3.98 / 0.60 | — / 0.11 | 3,070,130 |
| enemy-movers | `high` fill / warm | 7.99 / 0.60 | | 3,194,622 |
| enemy-movers | `low` after `high` | 0.68 | 0.15 | 3,194,622 |

¹ First run of the batch; the closet's `low` fill a moment later took 1.99 s.

- `low` saves 6.2 s cold on the setpiece (−27%) and 4.2 s on enemy-movers (−54%), all of it in SH Bake.
- `low` id 57 is 4.4% smaller on the setpiece and 3.9% on enemy-movers.
- Byte identity: every `low` fill and warm PRL equals its `low` cold PRL; every `high` fill and warm PRL equals `--release`; every `low`-after-`high` PRL equals `--release` (every mover served wholly from `high` entries), on all three maps.

## Low against high at rest (first measurement, for M2)

`tests/mover_probe_channel_fidelity_rest.rs` (`--ignored`): both builds at uniform L0, the bounce composed at each terminus (stored state, else id 34) and compared in display levels, `low` minus `high`. Direct and depth do not depend on fidelity; id 34 is byte-identical between the two.

| Map | Mover | Terminus | Probes | Largest darker | Largest brighter | Past 1 / past 3 levels |
|---|---|---|---|---|---|---|
| mover-closet | `closet_door` | t′ 0 (closed) | 144 | 0.00 | **+10.75** at (325.0, 523.8, 15.4) | 144 / 102 |
| mover-closet | `closet_door` | t′ 1 (open) | 144 | 0.00 | 0.00 | 0 / 0 |
| mover-setpiece | `setpiece_wall` | t′ 0 | 10,377 | −9.09 at (413.9, 1009.4, 280.4) | **+11.42** at (413.9, 1206.3, 319.8) | 10,149 / 4,205 |
| mover-setpiece | `setpiece_wall` | t′ 1 | 10,377 | **−11.23** at (610.7, 1048.8, 44.2) | 0.00 | 137 / 90 |
| mover-setpiece | `setpiece_room` | t′ 0 | 3,048 | **−6.91** at (807.6, 537.0, 123.0) | +0.43 | 2,400 / 412 |
| mover-setpiece | `setpiece_room` | t′ 1 | 3,048 | **−7.84** at (768.2, 340.2, 83.6) | +4.21 | 2,059 / 428 |

Positions in Quake units. Mover ids follow each map's entity order. The closed closet reads brighter at `low`: it keeps id 34's later bounces, which see the open doorway. The setpiece's room reads darker at `low` at both termini.

## Build Summary sample

`mover-setpiece`, `low` cold:

```
Mover bounce fidelity: low build; 2 of 2 mover(s) approximate
  kinematic_mover 'setpiece_wall' (#0): 15 pose(s), low, approximate: 15 of 15 pose(s) at 1 of 5 posed bounce(s); --release bounce bake est. 2.2 s (id 34's recorded 1.1 s × 2); mid-pose states skipped: bounce 1625 of 2820, depth 1894 of 4771
  kinematic_mover 'setpiece_room' (#1): 6 pose(s), low, approximate: 6 of 6 pose(s) at 1 of 5 posed bounce(s); --release bounce bake est. 2.2 s (id 34's recorded 1.1 s × 2); mid-pose states skipped: bounce 2109 of 3059, depth 1468 of 3336
```

`enemy-movers`, `low` after `high`: `low build; 0 of 4 mover(s) approximate`, each mover `high`.

**The projection underestimates on these maps.** Id 34 bakes in 1.1 s on the setpiece, so the brief's formula projects 2.2 s per mover. The measured `high` mover bake is about 6.6 s over both movers (SH Bake 10.59 s `high` against 4.02 s `low`), and most of that cost is mid poses: 21 poses. On enemy-movers the formula projects 1.0 s per mover against about 4.1 s measured over four movers. "One terminus costs about one id-34 bake" came from the hallway, where later bounces over every static patch dominate. On small maps a mid pose's domain trace is not negligible. Owner to judge whether the formula should count mid poses.

## Scripts and logs

`scratchpad/t9/` (`run.sh`, `scenarios.sh`, `id57.py`, `logs/`, `out/`, `caches/`).
