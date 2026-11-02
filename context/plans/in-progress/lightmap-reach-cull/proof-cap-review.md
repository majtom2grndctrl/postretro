# Coverage proof review: capped stand-ins, weld, prefilter (f76b1c429)

**Verdict: unsound.** I traced these by hand and ran nothing. Two new attacks break the cap. A third breaks the strict core under the brief's own metric. The weld, the prefilter, the joint-cap construction and the lazy matching hold.

## 1. Ramp: nested closed prisms inflate `window_max` (critical, cap)

**Geometry.**
- Take box [0, S]³ with S = 2 mm, and drop its x = 0 and x = S faces.
- Rim ℓ = (0, 0, z), z ∈ [0, S]. The present edge on it comes from the y = 0 face.
- Add prisms P_i, i = 1..5, with tips z_i = 0.4·i mm.
- Each prism's cross-section in xy is the triangle O = (0, 0), 1.5 mm·(−cos φ, −sin φ) and 1.5 mm·(−cos(φ+10°), −sin(φ+10°)), with φ = 10° + 15°(i−1). The wedges are disjoint and lie in x<0, y<0.
- Each prism spans z ∈ [z_i, S + 1 mm] and is closed.

**Trace.**
- On rim piece [z_{i−1}, z_i], the present + edges are the rim plus P_1..P_{i−1}, i in all. The present − edges are P_1..P_{i−1}, i−1 in all.
- The piece above holds i − edges, so `window_max` = i and the cap is 1.
- P_i's − edge ends at z_i, 0.4 mm away, so it stands in. In 3D, the rim stretch lies ≤ 0.4 mm from P_i's face, and P_i's tip sits on the rim face.
- Above S, both counts are 5.
- So the rim reads proven. Repeat on all 8 rims (40 prisms) and a 2 mm tunnel through solid reads zero unproven. In general the cost is L/0.4 mm prisms per rim.

**Why it works.** `window_max` counts present edges. Closed shells add to both directions, so they raise the window without touching the deficit, and a staircase of them carries the deficit off the rim's end in steps under τ. The header's claim (settle.rs:14–16), "a direction short on every piece within τ … stays short", is false.

**Partial fix.** Cap by net balance instead: cap_b(p) = max over q within τ of (b_q − a_q), minus (b_p − a_p). Closed shells add zero, which kills the ramp. It does not survive §2.

## 2. Overlap row (high, cap; also defeats a net-balance cap)

**Geometry.** On the same rim, add shells k = 0..4, with c_k = 0.45k mm:
- Each shell is the box [−0.1, 0] mm × [−0.1, 0] mm × [c, c + 0.3] mm.
- Its x = 0 face is split into two faces, overlapping 0.15 mm along ℓ (more than δ, so they do not weld): z ∈ [c, c + 0.2] and z ∈ [c + 0.05, c + 0.3].
- Those two faces carry e1 and e2, which run in the deficit direction (+z). The shell's y = 0 face carries one −z edge.

**Trace.**
- The overlap piece holds a = 2 (rim and shell) against b = 2, so it is balanced.
- Every other piece within τ of it has a deficit of 1 and cap ≥ 1. e1 or e2 of this shell or the neighbouring one ends within 0.15 mm and stands in.
- Gap pieces between shells take e2_k or e1_{k+1}.
- So the rim reads proven. Each shell alone reads unproven (the R12 cost), but the rim's deficit absorbs it.

**Fix.** I found no local window that survives both §1 and §2. Sound minimal fix: delete stand-ins (go back to consensus §2), accept movement-feel's false holes, and land crease-edge snapping as the source fix. Both attacks fail under strict matching.

## 3. Ladder: deficit migrates across lines (owner level; strict core, δ-independent)

**Geometry.**
- Take a slab [0, 1 m]² × [0, H], H = 10 mm, with both broad faces (z = 0 and z = H) dropped.
- Beside side face S (y = 0), add closed rods R_j = [−1 mm, 1001 mm] × [−0.2, −0.1] mm × [0.4j, 0.4j + 0.1] mm, for j = 1..24.
- Repeat on all 4 sides: 96 rods.

**Trace.**
- The rod lines lie ≤ 1 mm apart, so S's bottom rim, every rod edge and S's top rim link into one cluster (clusters.rs:96).
- A perfect matching exists:
  - S_bottom(+) ↔ R_1's lower −, at 0.41 mm;
  - R_j's upper + ↔ R_{j+1}'s lower −, at 0.3 mm;
  - R_24's + ↔ S_top(−), at 0.32 mm;
  - each rod's remaining edges pair among themselves.
- So the proof reads zero unproven on a 1 m² hole.

**Same-face self-pairing.** A simpler form needs no shift. `Measure::pairs` (settle.rs:77) lets an edge pair with an edge of its own face, at distance 0. So any face whose opposite edges share a cluster pairs with itself. That covers widths up to 2τ directly, and any width with a link ladder. Gap 3's "narrower than τ" understates it.

**Why this is the brief's own limit.** Each pair really is within τ, so the face set is genuinely closed within τ, but the closed surfaces it forms do not bound the solid that BSP sees. Edge balance cannot see this.

**Fix.**
- Forbid same-face pairs (`a.face != b.face` in `pairs`), then re-run the gate maps.
- Reword gap 3 to its general form: "a solid region whose remaining faces close within τ without enclosing it". Verify mode guards it.
- A real fix needs a cross-check against BSP solidity. That is an owner decision.

## 4. Axis skew (medium, cap)

`stand_in_reach` and the window are measured in axis units (settle.rs:204–223). In R6's axis-skew cluster, cos 89.977° ≈ 4.0e-4, so τ along the axis is about 1.25 m along the hinge: the whole hinge sits in one window.

The 0.37 mm sliver stand-in passes the crack-width test (3D-checked, ≤ τ), but its licence comes from counts up to a metre away. Under skew, one count boost anywhere, plus a row of tiny shells every < τ, replaces §1's staircase.

**Fix.** Measure reach and window in 3D along each edge (scale by |d·axis|), or refuse stand-ins in clusters with off-axis members.

## 5. Joint cap via Mendelsohn–Dulmage: valid

- The two runs live in one graph: X = P0 ∪ S0 ∪ D1 and Y = P1 ∪ S1 ∪ D0.
- Its edges are P0–P1, P0–S1, S0–P1, and dummies complete to their own side's stand-ins. There are no stand-in–stand-in edges.
- `pairs` is symmetric, so M1 covers P0 ∪ D1 and M2 covers P1 ∪ D0. The combined matching therefore leaves at most cap of each side's stand-ins to real nodes.
- Doc drift: consensus §3 says "do not combine two one-sided runs". The code does, validly. Update the text.

## 6. Weld: no new break

- A cluster is refused when it is wider than δ or shares a triangle. The representative is the minimum bit key, so it is order-free.
- A face can still fold through corners that do not share a triangle (for example a thin rhombus fanned from one tip). Only faces narrower than δ fold this way, which is inside gap 3.
- An edge shorter than 2δ can have its welded direction rotated up to ~45° when its two ends weld to opposite-side representatives. That hides at most 2δ of length.
- A link at exactly δ can be missed through rounding in the grid floor (weld.rs:113). That can split a cluster that would have been refused, but every move stays ≤ δ.

## 7. Face-box prefilter: necessary only

- Removing candidates cannot enlarge a maximum matching, and dummies never consult `may_pair`. So even an inaccurate frame on a sliver costs only false holes.
- The box bounds every corner in its own frame. The pad covers τ plus slack, and a degenerate face gets `None`, which prunes nothing.

## 8. Lazy matching and determinism: sound

- Dummies augment first, and augmentation never uncovers a left slot. `lead_held` fails the piece if a dummy is ever free, so a dummy cannot be left free silently.
- Carried pairs skip slots a dummy holds. Kuhn's argument holds after any start.
- The verdict does not depend on order. Which equal edge gets reported depends only on the sorted order of the geometry keys.
