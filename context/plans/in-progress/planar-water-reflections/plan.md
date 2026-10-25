# planar-water-reflections — plan of record

mode: resumable
status: blocked
read at: 800bbe186

## Block

**False Decision premise: the bounded-region walk is not built.** Decisions "Mirror visibility rides the bounded-region walk" (the mirror floods "through the seeded entry of `portal-walk-bounded-regions`") and "Sequence. After `portal-walk-bounded-regions`" both assume that brief has landed.

Evidence at 800bbe186:
- `portal-walk-bounded-regions` sits in `context/plans/drafts/`, not `ready/`, `in-progress/` or `done/`. `roadmap.md` line 204 still lists it as "(draft)".
- `crates/visibility/src/portal_vis.rs` is today's step-capped DFS: `MAX_PORTAL_WALK_STEPS = 20_000`, `portal_traverse_with_step_limit`. It has no per-cell rect state and no seeded entry.
- `rendering_pipeline.md` §2 still marks bounded regions "decided, not yet built".
- `origin/plan/portal-walk-bounded-regions-bake-gap` has no commits ahead of `origin/main`. It is a merged planning branch, not an implementation.

Without that walk, the mirror would need the spike's `portal_traverse_seeded` over the step-capped DFS. That is the path the brief rejects: the cap becomes the mirror's normal path, and the walk is the spike's whole extra cost (walk 2.69 ms of 7.68 ms, `research.md`). AC rows "no ordinary pose trips the mirror walk's step cap" and "bake-gap gate holds with planar on" depend on the new walk and its gate.

Owner options:
1. **Build `portal-walk-bounded-regions` first** (promote and build its brief), then resume here. This is the sequence the brief and roadmap already record.
2. Change the Decision so this brief lands without the walk. That changes the Decision and two Acceptance rows, so it is the owner's call.

## Owner directions recorded

- **Counsel gate (Decision "Counsel gates build start").** Owner, 2026-10-25: proceed without waiting on `ip-check.md` §For counsel. The owner has reviewed those patents in depth and identified how to avoid them.
- **IP notes bind every builder.** Owner, 2026-10-25: every building agent follows the IP notes closely. In each worker brief, include the full text of `ip-check.md` §Design rules D1–D9 and §Recommended path (P2); the brief's IP-rules Decision; `rendering_pipeline.md` §7.3a Planar layer IP rules and the SSR IP rules; and `research.md` §IP map. Review checks the planar shader text and its routing against those rules.

## Corrections
- Earlier in this session I logged `portal-walk-bounded-regions` as landed in `done/`. The grep hit came from `drafts/`. The Block above has the correct state.

## Tasks

Not planned. Planning resumes once the block clears.
