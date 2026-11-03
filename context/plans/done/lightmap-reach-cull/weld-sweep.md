# Weld δ sweep

Task 1c of `plan.md`, by the δ selection procedure in `weld-consensus.md`. Measured 2026-11-02 on the worktree's uncommitted 1c build (base 598ea6949), post-cut geometry from `gate_map_geometry::map_geometries`, `--release`. The instrumented sweep test was temporary and is gone; `coverage_proof::gate_map_tests::coverage_proof_on_gate_maps` re-checks the chosen δ.

## Result

- **δ = 100 µm** (`WELD_RADIUS_METERS`), 2·δ_min.
- **δ_min = 50 µm**, with capped stand-ins (§3, owner-approved): the smallest ladder δ at which hallway, hallway-mini and movement-feel all read 0 unproven spans. Under τ/8 (62.5 µm); 100 µm is under τ/4 (125 µm), and all three read 0 there.
- Without stand-ins no δ ≤ τ/4 proved movement-feel; every leftover was gap-shaped (step 6), which led to the capped fallback.

## Unproven spans by δ (µm)

| map | stand-ins | 0 | 5 | 10 | 20 | 30 | 50 | 75 | 100 | 125 |
|---|---|---|---|---|---|---|---|---|---|---|
| stress-warren-hallway-inspection | off | 213 | 84 | 16 | 0 | 0 | 0 | 0 | 0 | 0 |
| stress-warren-hallway-inspection | uncapped | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| stress-warren-hallway-inspection | capped | 96 | 40 | 12 | 0 | 0 | 0 | 0 | 0 | 0 |
| stress-warren-hallway-inspection-mini | off | 128 | 53 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| stress-warren-hallway-inspection-mini | uncapped | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| stress-warren-hallway-inspection-mini | capped | 58 | 29 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| movement-feel | off | 153 | 119 | 86 | 67 | 46 | 30 | 21 | 20 | 24 |
| movement-feel | uncapped | 6 | 4 | 4 | 0 | 0 | 0 | 0 | 0 | 0 |
| movement-feel | capped | 9 | 6 | 7 | 2 | 1 | 0 | 0 | 0 | 0 |

"uncapped" is the stand-ins before the cap, measured once to classify the leftovers; it never ships.

## Weld per δ (from the capped run; the weld does not depend on stand-ins)

| map | δ µm | clusters welded | widest welded µm | refused: wider than δ | refused: two corners of one triangle |
|---|---|---|---|---|---|
| stress-warren-hallway-inspection | 0 | 0 | 0.00 | 0 | 0 |
| stress-warren-hallway-inspection | 5 | 165 | 4.53 | 6 | 0 |
| stress-warren-hallway-inspection | 10 | 233 | 8.53 | 0 | 0 |
| stress-warren-hallway-inspection | 20 | 247 | 17.06 | 0 | 0 |
| stress-warren-hallway-inspection | 30 | 247 | 17.06 | 0 | 0 |
| stress-warren-hallway-inspection | 50 | 247 | 17.06 | 0 | 0 |
| stress-warren-hallway-inspection | 75 | 247 | 17.06 | 0 | 0 |
| stress-warren-hallway-inspection | 100 | 247 | 17.06 | 0 | 0 |
| stress-warren-hallway-inspection | 125 | 247 | 17.06 | 0 | 0 |
| stress-warren-hallway-inspection-mini | 0 | 0 | 0.00 | 0 | 0 |
| stress-warren-hallway-inspection-mini | 5 | 92 | 4.16 | 0 | 0 |
| stress-warren-hallway-inspection-mini | 10 | 128 | 8.53 | 0 | 0 |
| stress-warren-hallway-inspection-mini | 20 | 128 | 8.53 | 0 | 0 |
| stress-warren-hallway-inspection-mini | 30 | 128 | 8.53 | 0 | 0 |
| stress-warren-hallway-inspection-mini | 50 | 128 | 8.53 | 0 | 0 |
| stress-warren-hallway-inspection-mini | 75 | 128 | 8.53 | 0 | 0 |
| stress-warren-hallway-inspection-mini | 100 | 128 | 8.53 | 0 | 0 |
| stress-warren-hallway-inspection-mini | 125 | 128 | 8.53 | 0 | 0 |
| movement-feel | 0 | 0 | 0.00 | 0 | 0 |
| movement-feel | 5 | 45 | 4.77 | 3 | 1 |
| movement-feel | 10 | 64 | 9.64 | 7 | 1 |
| movement-feel | 20 | 79 | 19.89 | 8 | 2 |
| movement-feel | 30 | 87 | 29.32 | 4 | 2 |
| movement-feel | 50 | 95 | 47.58 | 2 | 2 |
| movement-feel | 75 | 102 | 68.72 | 0 | 2 |
| movement-feel | 100 | 104 | 91.30 | 1 | 2 |
| movement-feel | 125 | 102 | 117.02 | 2 | 2 |

## Leftover shapes

A leftover is classed by the nearest other edge end within τ (axis units) of its run: a same-way edge's end makes it overlap-shaped, an opposite edge's end gap-shaped.

- **Stand-ins off, movement-feel:** at δ ≥ 50 µm every leftover is gap-shaped. At 100 µm, 20 spans at 7 corners, 9–308 µm along the line, corner copies 81–325 µm apart in 3D: crease-crack ends whose copies sit a crack-width apart (widest movement-feel crease crack 198.7 µm), plus along-line gaps in (δ, τ].
- **Capped, movement-feel:** at 20 µm one overlap (copies 19.1 µm apart, 13.0 µm along) and one gap (30.5 µm, 25.4 µm along); at 30 µm one overlap (28.6 µm, 23.8 µm along). None from 50 µm.
- **Hallways:** overlap and gap copies under 17.1 µm; all weld from 20 µm (hallway) and 10 µm (mini).

## Anchor (step 7)

- Weld off, stand-ins off: hallway 213, hallway-mini 128, movement-feel 153 (9 overlap, 144 gap).
- Weld off, uncapped stand-ins: hallway 0, hallway-mini 0, movement-feel 6, all overlap-shaped. This matches the consensus's expected 6 and 0: that expectation described the stand-in build.
- Weld off, capped stand-ins: hallway 96, hallway-mini 58, movement-feel 9.
