# Sub-plan 3 — Check-in

> **Parent plan:** [BVH Foundation](./index.md) — read first for goals and architectural commitments.
> **Scope:** lightweight conversation gate. Daniel runs the engine, eyeballs visual parity against Milestone 3.5, spot-checks frame time. We talk, we sign off (or we pivot).
> **Depends on:** sub-plans 1 and 2 (the runtime BVH path is shipped end-to-end).
> **Blocks:** Milestone 5 (Lighting Foundation). Until this gate signs off, no work starts on `lighting-foundation/`.

---

## Why this is lightweight

The Phase 3.5 close-out established the precedent: visual parity is verified by Daniel running the engine on the test maps, walking around, and saying "yep, looks right." Frame time is checked by glancing at the frame-time HUD on a cell-heavy map and confirming we're well ahead of 60fps vsync. No SSIM harness, no formal capture pipeline, no reference-image diffing — those are tools for a future regression-testing investment, not for this gate.

The reason this works: the BVH refactor is a spatial-structure swap with **no rendering changes**. Same vertex format, same shaders, same lights (none, still flat ambient), same indirect-draw architecture. If the picture looks the same, it *is* the same to within rendering precision. If something drifts visually, that's a real bug in the BVH path, not a measurement question.

---

## What to check

**Visual parity (manual screenshot review):**

- [ ] Walk every test map in `assets/maps/`. Confirm geometry, textures, and culling behavior match Milestone 3.5 by eye.
- [ ] Specifically watch for: missing geometry (BVH leaf not emitted), extra geometry (frustum cull too loose), z-fighting (index range collision), texture swaps (material bucket misassignment).
- [ ] Toggle the wireframe overlay (`Alt+Shift+\`) and confirm cull-status colors look reasonable — green/red/cyan distribution should resemble Milestone 3.5.

**Frame time:**

- [ ] On the cell-heaviest test map (20+ visible cells), confirm frame time is comfortably under 16.67ms (60fps). The exact number doesn't matter — "well ahead of 60fps vsync" is the bar, same as Phase 3.5 close-out.
- [ ] If frame time has *regressed* meaningfully from Milestone 3.5, that's the pivot signal — see below.

**Edge cases worth a quick poke:**

- [ ] Camera in solid leaf — render path shouldn't crash, geometry still draws via the appropriate fallback.
- [ ] Camera in exterior void — X-ray behavior preserved.
- [ ] Empty visible set (camera looking at a wall) — no GPU errors.
- [ ] First frame after load — no flicker, no missing geometry.

**Code health:**

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` clean
- [ ] No `POSTRETRO_FORCE_LEGACY` references anywhere
- [ ] No `chunk_grouping`, `CellChunkTable`, `chunks_for_cell`, `determine_prl_visibility` references anywhere

---

## Sign-off or pivot

**Sign-off path:** if visuals look right and frame time is fine, we mark this sub-plan complete, archive the `bvh-foundation/` plan to `context/plans/done/`, update the roadmap to flip Milestone 4 to ✓, and Milestone 5 (Lighting Foundation) is unblocked.

**Pivot path:** if global BVH falls short on cell-heavy maps, we have three options to discuss before any Milestone 5 work begins:

1. **Per-region BVH.** Split the global BVH into one BVH per portal cell. Better cache locality, smaller traversal per cell. The pre-committed fallback. Costs: more BVH bookkeeping, more storage buffers, baker (Milestone 5) needs to know which BVH to query for a probe ray.
2. **Different traversal strategy.** Maybe (b) multi-frustum traversal (sub-plan 2's rejected option) wins after all on cell-heavy maps. Re-evaluate with real numbers.
3. **Scope adjustment.** Re-examine whether the BVH refactor needs to ship before Milestone 5, or whether we can defer it and have Milestone 5's baker build its own structure. Lower-priority option — the whole point of doing BVH now is to share one structure across both consumers.

Whichever path we pick, document the decision in this file before Milestone 5 starts.

---

## Doc updates on sign-off

- [ ] Update `context/lib/rendering_pipeline.md` §5 to describe the BVH architecture; replace cell/chunk language throughout.
- [ ] Update `context/lib/build_pipeline.md` to describe BVH construction in `prl-build` (replace the cell-chunk table description).
- [ ] Update `context/plans/roadmap.md` Milestone 4 to ✓ with a one-line note on what shipped.
- [ ] Move `context/plans/drafts/bvh-foundation/` to `context/plans/done/bvh-foundation.md` (collapse to a single file) per the lifecycle convention.
