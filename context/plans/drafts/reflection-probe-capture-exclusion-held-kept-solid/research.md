# reflection-probe-capture-exclusion-held-kept-solid — research

Read at ffd61b87b (feat/watertight-tracer). Everything here was traced by reading `capture_exclusion.rs`, `capture_fill.rs` and `hit.rs`; nothing was run. Map units are inches (`UNITS_PER_METER` = 1/0.0254 in the tests), so 1 u ≈ 25.4 mm. CSG's contact epsilon is 1 mm, and the capture's contact tolerance is 1.5 mm (`CONTACT_TOLERANCE_M`). τ = 0.5 mm.

## R6 by reading

The room is `textured_room` with a pool, as in `split_platform_under_a_block`, but with no near brush. The probe box is the room. h = `PLATFORM_HEIGHT` = 8.

| Brush | Map box | Role |
|---|---|---|
| A, the block | [100,116]×[−8,8]×[h + 0.8 mm, 40] | excluded |
| N | [100,104]×[−8,8]×[0, h + 0.5 mm] | excluded |
| K | [104,116]×[−8,8]×[0, h] | kept |

CSG removes K's front against N, since it is coplanar with N's back and inside it, and K's whole top under A, a 0.8 mm gap. It also removes N's top and A's bottom, which are excluded anyway.

The ray is built as `over_the_platform(0.0011, 0.008)`: it crosses map x = 100 at h + 1.1 mm and descends 0.008 per unit, with its origin 1 m before that point.

1. The ray enters A's front 0.3 mm above A's bottom edge. That is a real crossing, and A's front is the first skipped face. It leaves A's bottom at x ≈ 101.48.
2. A's fill finds nothing. The first kept-side crossing is K's front plane at x = 104, where the point is h + 0.29 mm, inside the polygon slack and 0.51 mm off A. Or it is K's top at 105.41, if the slack does not admit that point. Both sides have no surviving face.
3. Two neighbours hold A's exit. N holds it at x ≈ 102.95, where the ray reaches N's top, 0.3 mm off A. K holds it at x ≈ 105.41, where the ray reaches K's top, 0.8 mm off A. `nearest_held` returns N.
4. The ray leaves N at x = 104, at h + 0.29 mm. N's fill finds nothing: K's front has no face, and K's top crossing is 36 mm off N. K holds nothing at N's exit, for the same reason. The exit is open at 104.
5. The re-trace starts at 104. K's top and front are gone, and K's back at x = 116 is a back face seen from inside, so the ray reads the wall behind (x = 128) as `Hit`, inside the box.

Expected after the fix: K's depth from step 3 (x ≈ 105.41) is carried forward. The re-trace finds no kept hit at or before it, so the trace returns the leak guard on A's front, which is the hit the unmasked ray takes.

Without the near brush, N's front is exposed. The near brush is a kept contact of A, but it holds nothing and fills nothing here: the ray never re-enters x ≤ 100. Leaving it out keeps the fixture minimal.

## Why depth is the bound

`held_at` returns `at = max(enters, exit)`, and only when `enters < leaves` and `leaves > exit`. So the ray is inside the held hull's convex interior from `at` until it leaves. Past `at`, the ray can meet three kinds of face, none of which it can read:

- The held hull's own faces, which are back faces from inside.
- Faces of another brush inside the hull. CSG removed these.
- Front faces behind the hull. Reaching these means passing through kept solid.

So a read past `at` reads through kept solid. The one legitimate read lands at `at` itself: the held brush's own surviving entry face, when the ray enters through it. That is the kept-behind row's case.

This bound agrees with the existing tie rule. Where kept and excluded brushes overlap at an exit, kept solid wins and the trace takes the leak guard. An excluded neighbour whose exit lies past a carried depth overlaps kept solid in the same way.

## Rivals

| Rival | R6 | Kept-behind row | Why not |
|---|---|---|---|
| Kept-first, at any depth | guards | guards (wrong) | Rejected in round 5 |
| Carry each passed hull's contacts, held against the current hull | K is 36 mm off N, not held: leaks | reads K's front | Misses R6 |
| Carry each passed hull's contacts, held against the hull they came from | K is 0.8 mm off A: guards | K is 1.11 mm off A: guards (wrong) | Decides on geometry alone, without seeing whether the entry face survived |
| Static rule at A's exit (does N's far side fall short of K's depth?) | 104 < 105.41 | 105 < 106 | Same answer for both rows; only the surviving face differs |
| Verify each final read against every passed kept contact's hull | guards | reads K's front | Closes the same class; inline it reuses `cross`'s spans, after the read it repeats them. A second rule beside `held_at`'s; the open question's candidate oracle |
| Carried depth (chosen) | no kept hit by 105.41: guards | front hit at 106 = depth: reads | Defers the face question to the re-trace, which is the only place it is answered |

The kept-behind row is `a_kept_brush_behind_an_excluded_one_reads_its_front_past_the_excluded_far_side`. It uses a sliver at [100,101], an excluded brush at [101,105] and a kept brush at [106,116]. The ray is `over_the_platform(0.0006, 0.006)`: it enters the excluded brush's top at x ≈ 103.94, and the kept brush holds the block's exit at 106, 1.11 mm under it.

## Class rows

The later-hull sketch is by reading; the executor confirms it with asserted preconditions.

- The split is N1 = [100,102]×[0, h + 0.7 mm] and N2 = [102.5,104]×[0, h + 0.5 mm], both excluded. K is as in R6, and the ray is R6's.
- N1 holds A's exit at x ≈ 101.97, 0.1 mm off A. K holds it at 105.41, so N1 is taken.
- N1 leaves into open space at 102. N2 is not N1's contact, since the gap between them is 12.7 mm.
- The re-trace takes a τ hit on N2's front at 102.5, 0.09 mm over its top edge, then enters N2 through its removed top at 102.95. N2 is crossed and exits open at 104, and K is again 36 mm off.
- Today the ray reads the wall behind. With the carried depth it takes the leak guard on A's front.
- This exercises a carry that survives a second `cross` call after a re-trace.

## Round-5 test map

Plan.md's review loop names the round-5 cases without labels. Here they are by test name:

- Split platform as a beside pass: `a_pass_beside_a_block_meeting_a_kept_brush_before_an_excluded_one_takes_the_leak_guard`.
- Split platform as a real crossing: `a_ray_crossing_a_block_meeting_a_kept_brush_before_an_excluded_one_takes_the_leak_guard`.
- Kept-behind with sliver: `a_kept_brush_behind_an_excluded_one_reads_its_front_past_the_excluded_far_side`.
- Exact tie: `kept_and_excluded_brushes_overlapping_at_a_blocks_exit_tie_to_the_leak_guard`.
- Selection rule: `the_nearest_held_neighbour_decides_and_kept_solid_wins_a_tie`.
- Off-box invariant: `a_tie_at_a_seam_with_an_off_box_brush_captures_the_same_kept_or_excluded`.

## Audit

Five rounds of reading each found one narrower case on this path:

- the phantom beside pass;
- a beside pass stepping over a same-distance entry;
- a beside pass skipping the fill;
- a beside pass skipping exit classification;
- brush order over depth.

R6 is a sixth, found by reading the round-5 fix.

A generated-layout test would replace reading with an oracle. It needs a slack, for the same `f32` rounding as the delegated tie slack. It should also confine itself to what exclusion causes.

One observation, not verified: a ray running inside a CSG contact gap between two brushes, more than τ from both, can meet a face CSG removed even in the unmasked capture. Exclusion did not cause that, so an oracle over "every kept hull" would flag it. Limiting the oracle to the kept contacts of hulls the ray crossed or passed beside avoids the false positive.
