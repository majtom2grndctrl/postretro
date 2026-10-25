# lightmap-chart-plane-drift

Brief · resumable · reads: `context/lib/build_pipeline.md` §Compiler pipeline step 12, §Build Cache · read at 076470c8c

## Problem

The `lightmap-exact-skips` review (`plans/drafts/lightmap-exact-skips/research.md` §Receiver drift) found an anticipated defect, not one seen in content: lightmap texel centres drift off the face they light.

The cause is that the bake treats a shading attribute as geometry. Chart planning builds `v_axis` from the stored vertex normal, which octahedral quantisation tilts slightly off the true face normal. Every reader that derives a plane or a position from the chart frame then sits on a tilted plane: texel-centre reconstruction, the footprint clip band, and others. Texel centres leave the face plane linearly with distance along v from the chart origin.

Past the ray lift plus the tracer's skip (1–2 mm), a shadow ray starts below its own receiver, and the double-sided hit test can shadow the texel with its own face. An oblique face at the largest extent the atlas admits drifts at least 2.5 mm, and more for cut faces rebuilt on the parent frame, so modders can author it (research §Self-hit threshold).

When this is done, every geometric reader of the chart frame works on the true face plane within f32 rounding, at any chart size. The fix chosen moves as few bytes as it can and is visually identical outside the defect.

## Decisions

- **Fix whatever the content reach.** Authored geometry can cross the threshold, and modders author long oblique faces. Iteration 0 still measures reach on the content maps and builds the defect fixture on the base commit. They are the loop's instruments and the permanent regression test, not a gate.
- **Bounded iteration with two goals.** The owner chose a measured search over a single argued build. Churn, and which consumers a change reaches, are settled only by baking (`development_guide.md` §1.3: increments earn their cost where the path is uncertain).
  - Each iteration is one commit and one scoreboard row in the plan of record, giving its numbers and verdict.
  - A discarded candidate is reverted and its row kept.
- **Hard constraints, for every candidate.** A candidate failing any of them is discarded:
  - texel centres within f32 rounding of their face plane, on every fixture and measurement map;
  - no runtime crate, shader, or PRL section layout change. This is the bake-ray contract's no-layout-change ruling (`plans/done/bake-ray-speed-contract.md`), applied here.
  - production still matches the reference bake, warm equals cold, and output is identical across worker counts;
  - Lightmap Bake wall time within measurement noise.
- **Primary goal: split the chart frame by role.**
  - The chart carries a true-plane frame, built from the true face normal (already computed in chart planning), beside its stored frame.
  - Every geometric reader uses the true-plane frame: texel-centre reconstruction, the clip band, the footprint pitch, the animated chunk bounds, and any other reader iteration 0's inventory classifies as geometric.
  - Every identity reader keeps the stored frame's exact bits: soft-visibility seeds, vertex lightmap UVs, face-cut grid coordinates, UV planning and the layout fingerprint.
  - Only lightmap-family sections move. It is met by a candidate that passes the hard constraints and leaves the geometry section byte-identical.
  - Variants are research §Candidates, the R family.
- **Fallback goal: build the frame in the face plane.** `v_axis` comes from the true face normal (C1), and shading stays on the stored normal. It is met by any candidate that passes the hard constraints; global churn and moved vertex UVs are accepted.
- **Stop condition.** Iteration 0 (instruments) does not count toward the cap.
  - **Primary met:** take the passing candidate. Allow at most one further iteration, only to cut churn, then stop.
  - **Primary unobtainable:** declared after three primary iterations each fail a hard constraint, or earlier if one failure is shown to be structural. Switch to the fallback.
  - **Fallback met:** stop.
  - **Fallback unobtainable:** declared after two fallback iterations fail. Stop and return to the owner with the scoreboard.
  - **Absolute cap:** six counted iterations. When it is hit, stop and return to the owner, whatever state the search is in.
- **The winner moves bytes, so it goes through the owner.** Bump each cache stage version it invalidates (`build_pipeline.md` §Build Cache). The owner A/B judges visual identity before landing, as the bake-ray contract's step 2 and the penumbra floor did. A discarded candidate bumps nothing.
- **Rejected shapes:**
  - **Correcting positions only.** Projecting texel centres while the frame stays tilted leaves the clip band on the tilted plane. It is 2.5 mm off the corrected centres on a full-extent oblique face, against a 0.5–1.5 mm band tolerance. The band then rejects real walls, and clip segments pass through them. That re-opens the contact-edge error the penumbra floor closed.
  - **Excluding the receiver's own face from its shadow test.** It misses the clip, the lead-in, the exact-skips hull, and neighbouring or cut faces.
  - **Lifting the ray origin along the true normal.** It is too narrow: the other centre readers stay drifted.
- **Lands apart from `lightmap-exact-skips`.** That brief's gate is byte identity against its base, and whichever lands second rebaselines on the other. Once the drift is gone, that brief's cull also clears tiles on long charts again (its margin Decision).
- **Measurement tools are on-demand tests, not prl-build flags.** They follow `lightmap_bake/penumbra_ab_tests.rs`: ignored tests driven by env vars naming the PRLs or maps.
- **Non-goals:**
  - **The octahedral encoder** (`crates/level-format`). Changing it moves runtime vertex normals and breaks the runtime constraint. Oblique normals would also keep about one code step of error, so the bake fix is needed either way.
  - **Smoothed shading normals.**
  - **The SH and probe bakes.** They place samples without chart frames.

## Acceptance

### Automated
- [ ] A reach report computes, per chart, the largest signed distance from any texel centre to the true face plane, including sub-charts on their parent grid. On a fixture with a known tilted normal, it reproduces the analytic drift within 1%.
- [ ] The defect fixture is an oblique face at the largest extent the atlas admits, under a light above it. On the base commit it records the minimum ray-origin height above the face and the count of texels whose shadow, clip or lead-in segments hit their own face, and that count is above zero. With the winner, the count is zero.
- [ ] Plane bound. With the winner, every chart in the fixture set and on the measurement maps has its texel centres and its clip band plane within the f32 rounding of its coordinates. The set includes an axis-aligned and an oblique face, each both short and at full extent, and a cut face.
- [ ] Iteration 0's reader inventory lists every reader of the chart frame as geometric or identity, with the symbol for each. Under the winner, every geometric reader is shown to take the true-plane frame, and every identity reader the stored bits.
- [ ] The wall-on-ramp fixture is a wall standing on a long oblique ramp, on the drift side, at full extent. With the winner, its clip and lead-in results match a run that admits every triangle to the clip band. On the base commit and under a position-only correction, the comparison is recorded.
- [ ] Regression guard (it cannot tell candidates apart, since the reference shares the chart readers): the existing parity tests pass with the winner: production matches the reference at each floor setting; warm equals cold across worker counts; overlap texels match across face cuts; the weight-map oracle matches. Any pinned literal that moves is listed in the plan of record against the scoreboard row that explains it.
- [ ] Each cache stage whose bytes the winner moves has its version bumped. The layout fingerprint hashes the stored frame, which the primary leaves unchanged, so without the bump warm builds would serve stale bytes. A toggling test shows the old entry is missed.
- [ ] Grep gate on the diff: nothing under `crates/renderer/`, `crates/level-format/` or shader sources changes, and no new prl-build flag. If the primary goal was met, the geometry section is byte-identical on every measurement map.
- [ ] The scoreboard in the plan of record ends in one of the stop condition's terminal states, with its counted iteration total at or under the cap.

### Measured findings (per iteration)
- [ ] Iteration 0, on the base commit, per content map: the largest chart extent along v and the worst signed texel-centre distance from the face plane.
- [ ] Per candidate:
  - sections whose bytes move;
  - texel churn and the largest change on `shadowmask-groups-capture`, `soft_shadow_test`, `gate-heavily-lit` and `combat-demo`;
  - BC5 and BC6H error against the base commit;
  - lines changed.
- [ ] Winner only: cold Lightmap Bake wall time on `stress-warren-hallway-inspection-mini` against a same-session base-commit baseline. Idle machine, one bake at a time.
- [ ] Findings note in this folder: the scoreboard, the terminal state reached, the winner and why it won.

### Manual
- [ ] Owner A/B in engine between the base and the winner: no visible change on the measurement maps, and the self-shadowed band on the defect fixture is gone.

## Path

- **Seams.**
  - Chart frame: `lightmap_bake/charts.rs::plan_charts`. It already computes `edge1 × edge2` (`normal_raw`), so the true face normal is free to keep on the chart.
  - Texel centres: `chart_raster.rs::chart_texel_world_position`. Vertex UVs: `atlas_layout.rs::assign_lightmap_uvs` through `chart_texel_position`.
- **Seed coupling.** `chart_texel_seed` keys on the frame's exact f32 bits. That is why the fallback churns every texel and the primary does not.
- **Frame readers.** The fallback reaches all of them. Starting inventory, to be confirmed in iteration 0: research §Frame readers. `ClipBand::for_chart` (`texel_footprint/clip_reach.rs`) builds its plane as `u_axis × v_axis`, which is the trap a position-only fix falls into.
- **Comparison harness.** `lightmap_bake/penumbra_ab_tests.rs` (two PRLs by env var; BC5 and BC6H error) and the `reseed_baseline_tests.rs` texel-diff harness.
- **First slice.** Iteration 0: the reach report, the reader inventory, the defect fixture and the wall-on-ramp fixture. Then the plainest primary candidate, R1.
- **Machine.** Disk-bound: one cargo build, test or bake at a time. Score candidates on the small maps, and time only the final pair on hallway-mini.

## Open questions

None.
