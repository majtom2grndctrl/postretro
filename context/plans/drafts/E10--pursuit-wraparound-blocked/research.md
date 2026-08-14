# Research — E10 Pursuit Wraparound `blocked`

Investigation notes behind `index.md`. Grounded against `crates/postretro/src/nav/path.rs`
(1732 lines), `nav/mod.rs`, `combat_positioning.rs`, `scripting/systems/ai/` (now a directory
module), `agent_steering.rs`, and `crates/level-compiler/src/map_data.rs` (production nav
defaults). Cite by symbol; line numbers drift with edits.

## Freeze chain (end to end)

`find_path` → `None` → `agent_steering.rs` admitted-replan arm: `find_path` `None` with no
surviving prior path sets `agent.blocked = agent.path.is_empty()` → empty →
`blocked:true has_path:false speed:0.00`, the HUD string in the report. The `None` originates in
`ensure_endpoint_clearance` (`nav/path.rs`), NOT endpoint resolution — both `resolve_region_at`
calls succeed, so the target is reachable and the failure is a repair failure.

## Mechanism

Repair loop `ensure_endpoint_clearance`, budget
`repairs_remaining = obstacles.len().saturating_mul(4).max(1)`; a violation with budget 0 →
`return None`. Every splice/projection `continue`s without advancing `segment_index`, so a
non-converging repair burns the budget on one segment.

- **Far-side standoff.** `project_out_of_disk` projects radially:
  `raw_endpoint + normalize_xz(terminal - raw_endpoint) * clearance`. `toward` (adjacent waypoint)
  is consulted only when the terminal sits on the disk center (`radial.length_squared() <=
  MIN_XZ_LEN_SQ`). A terminal on the FAR side of the endpoint → standoff on the far side → onward
  chord re-cuts the disk → falls into the bevel block, which emits only axis-aligned per-gate
  offsets and cannot walk the standoff around the endpoint → re-bevel until budget → `None`. The
  `project_out_of_disk` doc comment documents this exact far-side re-freeze and names this draft.
- **Overlapping-disk ping-pong (production defaults).** `map_data.rs` nav defaults `agent_radius
  0.4`, `cell_size 0.25`; `SKIN_DISTANCE 0.02` (`collision/mod.rs`) → `clearance 0.42`,
  `2*clearance 0.84 > cell_size 0.25`, so distinct wide-portal disks overlap. A standoff projected
  off disk A can land in overlapping disk B and re-project; the budget bounds the churn to a clean
  `None` (documented in the `project_out_of_disk` doc comment).

**Bevel vocabulary** (`bevel_point`): offset is always `± portal_interior`'s single normal axis of
one gate; it cannot tilt to an arbitrary angle, so it cannot emit a segment oblique to both portal
normals via the terminal-projection path.

## Existing recovery paths do not rescue the freeze (grounded this session)

The `blocked && !has_path` state is deliberately outside every recovery mechanism — the fix is the
only thing that clears it:

- `has_stuck_recovery_intent` (`agent_steering.rs`) returns
  `!agent.path.is_empty() && !agent.blocked && goal_speed > STUCK_INTENT_SPEED_EPSILON &&
  steer_velocity.length_squared() > MIN_XZ_LEN_SQ`. A frozen agent fails the first two conjuncts,
  so recovery never arms; the caller's `else` arm ("pathless blocked agent") resets `stuck_ticks`
  and `unstick_window_remaining` and holds position.
- `blocked_destination_now_directly_routable` (`agent_steering.rs`) re-admits a blocked agent for
  replan only when `resolve_region_at(position) == resolve_region_at(destination)` (same region).
  The wraparound target is in a different region, so this never fires; the agent rides the
  staleness cooldown, re-calling the still-failing `find_path`.

Consequence for testing: the steering integration test's "reaches the far region" assertion is a
genuine diagnostic — recovery wandering cannot produce it, so only a real `find_path` route can.

## Class-1 vs class-2 partition (the crux)

The SSF funnel (`funnel` over `inset_portals`) does NOT lose the route — it produces a valid taut
polyline over the inset gate points. The failure is at the seam: SSF reasons about inset gate
*points*; the raw-endpoint *disks* are enforced afterward by `ensure_endpoint_clearance`, whose
(radial + axis-aligned) vocabulary sometimes cannot re-express the route SSF found.

- **Class 1 — repair vocabulary too weak (code-fixable):** the far-side-standoff case, and a gap
  in `2*clearance < gap < 2*clearance + axis-aligned margin`. A clearance-safe route physically
  exists but the terminal-projection repair can't emit it. **The wraparound is predominantly class
  1** — open floor around a freestanding wall end is wide, so the wall-end disks don't overlap-block
  the route; what fails is the radial standoff + axis-aligned bevel.
- **Class 2 — genuinely unroutable (author problem):** gap `< 2*clearance`, disks overlap, no
  clearance-safe crossing. `find_path_returns_none_for_unthreadable_pinch_gap_limit` is canonical;
  correct answer is `None`. Only bake widening fixes it.

Fixtures for contrast (all current):
- `find_path_routes_bent_twin_portal_chicane_without_dropping_to_none` — corners wide enough that
  axis-aligned repair threads the chicane. Its comment names the marginal middle band (a gap only
  ~0.05 m above `2*clearance` whose sole route is a tilted segment).
- `find_path_returns_none_for_unthreadable_pinch_gap_limit` — sub-`2*clearance` corners → correct
  `None`.
- `find_path_repairs_oblique_straight_chord_near_raw_portal_endpoint` — an INTERIOR straight chord
  that cuts a raw endpoint disk (start `(0,0,3)` → goal `(0.74,0,5)`, `agent_radius 0.35`) is
  repaired via a corner insert (`path.len() > 2`, a mandatory waypoint, every segment clears). This
  shows the corner-insertion path already handles oblique interior chords; the residual is
  specifically the marginal-gap band, not oblique chords in general. It is distinct from the
  terminal-projection freeze this spec fixes.

## Fix options (verdict: option ii)

| # | Option | Touches | Verdict |
|---|--------|---------|---------|
| i | True-tangent bevel between two disks | `bevel_point`, `clearance_bevel`, new tangent helper | Correct for the oblique marginal-gap pinch; moderate complexity. Conditional follow-on, not the headline fix. |
| ii | **Bias terminal projection toward corridor** (`toward`) | `project_out_of_disk` only | **Minimal.** Directly fixes the far-side standoff (the leading wraparound cause). Call sites already pass the adjacent waypoint as `toward`. |
| iii | Tangent/visibility string-pull over disks | `funnel`, `inset_portals`, `ensure_endpoint_clearance` (rewrite) | Over-engineering; whole-repair blast radius. Do not. |
| iv | Raise `repairs_remaining` | budget line | **No-op.** `None` is reached because the vocabulary can't express the route (or the capsule doesn't fit), not because 4× is too few. |
| v | Bake-time corridor widening | level-compiler | Out of scope (authoring); only fix for class 2. |

## Combat-positioning: out of scope (latent, not causal)

`resolve_combat_slots` (now `scripting/systems/ai/combat_slots.rs`) sets `outcome.combat_slot` via
`select_combat_positions_batch` (`combat_positioning.rs`); the apply pass does
`outcome.combat_slot.unwrap_or(target.position)` (`scripting/systems/ai/mod.rs`). A candidate only
becomes a `CombatCandidate` — and thus a possible `combat_slot` — after `find_path(query.nav_graph,
query.agent_pos, position)?` succeeds in the candidate `evaluate` path (`combat_positioning.rs`,
the `let path = find_path(...)?;` gate). So `combat_slot` can never be *more* blocked than the raw
target fallback — and the fallback routes the raw target through the SAME `find_path`
(`agent_steering.rs`). Therefore fixing `find_path` unfreezes pursuit regardless of slot scoring.

Separate latent bug: `grounded_candidate_position` (`combat_positioning.rs`) grounds candidates
with strict `region_at` (two call sites), not the snapping `resolve_region_at`, so near-wall cover
slots are dropped before `find_path` sees them. This is a targeting-QUALITY defect (enemies walk
the raw-target fallback straight at the player instead of taking cover), NOT the freeze — a sibling
follow-up ticket, not this spec.

## File sizes / split

`nav/path.rs`: 1732 total, `#[cfg(test)]` boundary at ~824 → **~823 non-test lines** (already past
the `development_guide.md` §2.1 600-line source-line threshold — test files are exempt; the fix +
tests push further). Split the clearance-repair cluster (`CLEARANCE_EPS`, `bevel_point`,
`clearance_bevel`, `route_out_of_disk`, `MIN_XZ_LEN_SQ`, `project_out_of_disk`, `PathPoint`,
`ensure_endpoint_clearance`) to `nav/path/clearance.rs` (~335 lines out, leaving path.rs source
~490 < 600). `segment_point_distance_xz` STAYS in `path.rs` — it is the shared clearance oracle that
find_path-level tests remaining in `path.rs` also call — reached from `clearance.rs` via `super::`.
Seam symbols the cluster reaches across: `FunnelEndpoint`/`FunnelGate`, `segment_point_distance_xz`,
`distance_xz` (in `nav/mod.rs`), the `NavPath` return type, and the `find_path`/`funnel` calls into
`ensure_endpoint_clearance` (which gains `pub(super)`). Other files (for reference): `combat_positioning.rs` 837,
`agent_steering.rs` 1093 (+ `agent_steering/tests.rs` 2612), `nav/mod.rs` 514; the AI system is now
a directory (`scripting/systems/ai/mod.rs` plus `combat_slots.rs`, `targeting.rs`, `brain_*.rs`,
`graph_eval.rs`, `candidate_scope.rs`, `engine_floor.rs`, `facing.rs`).

## Reproduction geometry

Freestanding wall = a solid hole in open floor; the walkable annulus/U splits into a chain of
rectangular regions (near-side → wall-end/tip → far-side, ~3–4 regions, a portal per seam). The
wall end is a convex corner whose wide-portal endpoint disk is the pinch. The goal (player against
the far face, in the eroded band) snaps into the far portal's endpoint disk; radial projection
plants the standoff on the far side → current `None`. Constructible with existing test helpers
(`section`/`region` builders, raw `NavPortal` literals, `navmesh.agent_radius = …`, raw `Vec3`
terminals in the eroded band, the `L_CORRIDOR_ENDPOINTS`-style per-segment clearance oracle).
