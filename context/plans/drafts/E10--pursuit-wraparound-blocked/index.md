# E10 — Pursuit Goes `blocked` on a Wraparound Route

> **Draft.** Deferred out of E10 play-testing; identified as a prerequisite for further enemy
> movement work. A named residual of the shipped parent `E10--navmesh-capsule-clearance`
> (Euclidean erosion + funnel corner-offset), which explicitly scoped this "two-corner freeze"
> out. Fixes the far-side pinch-gap residual in `nav/path.rs` that freezes pursuit around a
> freestanding wall. Lineage: the shipped `E10--slow-agent-arrival-stuck` (arrival-deceleration
> feel) and `E10--enemy-stuck-recovery` (tangent-bias unstick) each own a distinct pursuit-motion
> failure; this spec owns the pathing-repair freeze neither addresses — and which stuck-recovery
> cannot reach by construction (see Direction). Code grounding and fix-option analysis in
> [`research.md`](./research.md).

## Goal

A chasing enemy that tracks the player fine around one corner freezes —
`state:alert speed:0.00 arrived:false blocked:true has_path:false` — when the player wraps
around a freestanding wall to its far-middle, even though the U-shaped route around the wall
end is navmesh-reachable and within leash. Make `find_path` return a valid route for a
genuinely-threadable wraparound so pursuit continues, instead of returning `None`.

**Not a line-of-sight issue.** There is no LOS system today; the target is simply on the far
side of a wall and the route must wrap around it. The freeze is a pathing failure, not a
perception one (see the separate future line-of-sight spec).

## Direction

**Problem.** The cause is a repair-vocabulary gap, not an unreachable goal. `find_path` returns
`None` out of `ensure_endpoint_clearance` even though both endpoint region resolutions succeed
and the string-pull already found a taut polyline around the wall end. `project_out_of_disk`
projects a terminal that snapped into a wall-end endpoint's clearance disk **radially** outward;
when the terminal sits on the far side of that endpoint, the standoff lands on the far side, the
onward chord re-cuts the same disk, and the axis-aligned bevel vocabulary cannot walk the
standoff around to the corridor side — so the repair re-bevels until its budget exhausts and
bails to `None`.

**Prior commitments this coexists with.** The clearance-repair vocabulary this refines
(`ensure_endpoint_clearance`, `project_out_of_disk`, `bevel_point`) is owned by the shipped parent
`E10--navmesh-capsule-clearance`, which landed both the bake-time Euclidean erosion and the
runtime funnel corner-offset repair. This spec touches only the runtime projection seam — it does
none of that spec's out-of-scope items (corridor re-derivation, per-archetype radii, dynamic
clearance queries) — and preserves the class-1/class-2 line (code-fix vs author-widens) that
mirrors the parent's own authoring boundary.

The `blocked && !has_path` state this spec eliminates is deliberately outside every existing
recovery path, so the fix is load-bearing — nothing else rescues it:
- `has_stuck_recovery_intent` (`agent_steering.rs`) gates the tangent-bias unstick on
  `!agent.path.is_empty() && !agent.blocked && goal_speed > ε && steer_velocity ≠ 0`. A frozen
  agent has an empty path and `blocked == true`, so recovery never arms — the `else` arm resets
  `stuck_ticks`/`unstick_window_remaining` and the agent holds position (`agent_steering.rs`,
  the "pathless blocked agent" branch).
- The replan-admission clause `blocked_destination_now_directly_routable` re-admits a blocked
  agent only when its current position and destination resolve to the **same** region. The
  wraparound target sits in a **different** region reachable solely by the around-the-end route
  `find_path` rejects, so that clause never fires either. The agent rides only the staleness
  cooldown, re-calling `find_path`, which keeps returning `None`.

  This is why the steering integration test (Task 3) is a real diagnostic: an agent can only
  reach the far region by a genuine `find_path` route, never by recovery wandering.

**Alternatives rejected.** A true-tangent bevel between the two endpoint disks (research option i)
would also address an oblique marginal-gap pinch, but it is a larger change with whole-repair
blast radius and is not the far-side-standoff cause; it stays a conditional follow-on (Open
questions). Raising `repairs_remaining` (option iv) is a confirmed no-op — the budget is
exhausted because the vocabulary cannot express the route, not because 4× is too few. A
tangent/visibility string-pull rewrite over disks (option iii) is over-engineering for this bug.
Fixing it on the **construction side** — in funnel-waypoint generation, so the terminal never
snaps into a far-side disk — is the rival most aligned with the parent's "traversable by
construction" northstar, but it loses here: the funnel already emits a valid taut polyline
post-erosion, so the `None` is provably in the post-funnel projection seam, not in generation;
touching generation is the same whole-repair blast radius option iii warns against, while the
projection bias targets the documented cause in one function.

## Background (the mechanism)

Confirmed by code trace (see `research.md`). The `None` originates in `ensure_endpoint_clearance`
(`nav/path.rs`), not in endpoint resolution; both `resolve_region_at` calls succeed, so the
target IS reachable — the `None` is a repair failure.

- **Radial terminal projection lands the standoff on the wrong side.** `project_out_of_disk`
  projects a start/goal that snapped into a wall-end endpoint's clearance disk **radially**
  outward (`raw_endpoint + normalize_xz(terminal - raw_endpoint) * clearance`); `toward` (the
  adjacent waypoint) is consulted only when the terminal coincides with the disk center. When the
  terminal is on the FAR side of the endpoint from the corridor, the standoff is planted on the
  far side; the next chord (standoff → next waypoint, back toward the corridor) re-cuts the same
  disk, and the axis-aligned bevel vocabulary (`bevel_point`) can only emit per-gate normal-axis
  offsets — it cannot walk the standoff around the endpoint to the corridor side. The repair
  re-bevels the same incursion until `repairs_remaining` (`obstacles.len().saturating_mul(4).max(1)`)
  exhausts → `None`. The `project_out_of_disk` doc comment already names this draft and describes
  the far-side re-freeze.
- **The route physically exists.** The Simple Stupid Funnel string-pull (`funnel` over
  `inset_portals`) already produces a valid taut polyline around the wall end; the failure is at
  the seam where `ensure_endpoint_clearance` cannot re-express that route in its (radial +
  axis-aligned) vocabulary. This is a "repair vocabulary too weak" failure (class 1,
  code-fixable), distinct from a genuinely-too-tight corridor (class 2 — see Out of scope).

## Scope

### In scope

- **Split `nav/path.rs` first.** It is at ~823 production lines (past the ~800 split-before-extend
  threshold; the fix + tests push further). Extract the clearance-repair cluster (`CLEARANCE_EPS`,
  `segment_point_distance_xz`, `bevel_point`, `clearance_bevel`, `route_out_of_disk`,
  `MIN_XZ_LEN_SQ`, `project_out_of_disk`, `PathPoint`, `ensure_endpoint_clearance`) into a sibling
  `nav/path/clearance.rs`, leaving the core A*/SSF funnel in `path.rs`. Behavior-preserving; the
  seam is `FunnelEndpoint`/`FunnelGate` plus the `find_path` call and the shared helpers the
  cluster reaches (`distance_xz`, the `NavPath` return type).
- **Corridor-biased terminal projection.** Redirect `project_out_of_disk` so a terminal snapped
  into a wall-end endpoint's clearance disk projects to a standoff on the CORRIDOR side (toward the
  adjacent waypoint the call sites already pass as `toward`), not radially outward — so the onward
  chord does not re-cut the disk. The true target stays on `AgentComponent.destination` (unchanged).
- **Wraparound regression coverage.** A `find_path` fixture for a freestanding-wall
  wrap-to-far-middle route (currently `None`) that now returns `Some` with every segment clearing
  every wide endpoint; and a steering-level test that a chaser on such a route reaches the far
  region without ever latching `blocked && !has_path`.

### Out of scope

- **`combat_positioning` strict-`region_at` grounding gap.** `grounded_candidate_position`
  (`combat_positioning.rs`) grounds candidates with strict `region_at`, not the snapping
  `resolve_region_at`. Confirmed a latent targeting-QUALITY defect (drops good near-wall cover
  slots), NOT the freeze cause — a `combat_slot` is only ever `Some` for a candidate that already
  round-tripped through `find_path` (`combat_positioning.rs`, `evaluate`-path `find_path` gate),
  and the apply pass's `outcome.combat_slot.unwrap_or(target.position)` fallback
  (`scripting/systems/ai/mod.rs`) routes the raw target through the same `find_path`, so fixing
  `find_path` unfreezes pursuit regardless of slot scoring. File as a separate follow-up.
- **Genuinely-unroutable pinches (class 2).** A wall-end passage narrower than `2 * clearance`
  (0.84 m under production defaults `agent_radius` 0.4, `cell_size` 0.25, `SKIN_DISTANCE` 0.02)
  has no clearance-safe crossing; only bake-time/authoring widening fixes it.
  `find_path_returns_none_for_unthreadable_pinch_gap_limit` must still return `None`.
- Tangent/visibility string-pull rewrite; raising `repairs_remaining` (confirmed no-op); LOS /
  disengage-on-sight; the arrival-deceleration "pause before arriving" feel
  (`E10--slow-agent-arrival-stuck`, shipped).

## Acceptance criteria

- [ ] `find_path` on a freestanding-wall wrap-to-far-middle fixture — where the around-the-end
      corridor is wider than `2 * clearance` and the goal sits in the far-side eroded band —
      returns `Some`, and every emitted segment clears every wide-portal endpoint by the
      effective clearance (within epsilon). The same query returns `None` before the fix.
- [ ] A chasing agent following that route reaches the goal's region and never latches
      `blocked && !has_path && speed≈0` on any tick while the target is live and within leash.
      Reaching the far region is achievable only via a real `find_path` route: stuck-recovery
      cannot arm on a pathless blocked agent, so it cannot mask a still-broken `find_path`.
- [ ] A genuinely-unroutable sub-`2 * clearance` pinch still returns `None`
      (`find_path_returns_none_for_unthreadable_pinch_gap_limit` stays green).
- [ ] The split (Task 1) is behavior-preserving: every existing `nav::` funnel/clearance/path
      test passes with no edits beyond module paths.
- [ ] After the projection change (Task 2), the terminal-standoff regressions
      (`find_path_projects_start_inside_wide_endpoint_disk_to_a_standoff`,
      `find_path_projects_goal_inside_wide_endpoint_disk_to_a_standoff`) and the coincident-terminal
      fallback (`project_out_of_disk_uses_portal_normal_for_fully_coincident_terminal`) pass without
      edits — they assert projected-off-raw, on/outside the disk boundary, and onward-chord-clears,
      not a radial coordinate, so the corridor-biased projection preserves them.
- [ ] `nav/path.rs` production code drops below the ~800-line threshold after the split; the
      clearance cluster lives in its own module.

## Tasks

### Task 1: Split the clearance-repair cluster out of `nav/path.rs`
Behavior-preserving extraction of the clearance-repair block (`CLEARANCE_EPS`,
`segment_point_distance_xz`, `bevel_point`, `clearance_bevel`, `route_out_of_disk`,
`MIN_XZ_LEN_SQ`, `project_out_of_disk`, `PathPoint`, `ensure_endpoint_clearance`) into a new
`crates/postretro/src/nav/path/clearance.rs` submodule (`path.rs` gains `mod clearance;`;
`path.rs` + a `path/` directory coexist under the Rust 2018 module convention), leaving the core
A*/SSF funnel (`find_path`, `astar_corridor`, `funnel`, `inset_portals`,
`FunnelEndpoint`/`FunnelGate`) in `path.rs`. Give the new module `use super::…` for the symbols
the cluster reaches across the seam: `FunnelEndpoint`, `FunnelGate`, `distance_xz`, and the
`NavPath` return type; keep `find_path`/`funnel` in `path.rs` calling `ensure_endpoint_clearance`
across the seam. Move the clearance-specific `#[cfg(test)]` cases (the `route_out_of_disk` and
`project_out_of_disk` unit tests) alongside their code; leave the `find_path`-level tests in
`path.rs`. No logic changes; every `nav::` test stays green with only module-path edits.

### Task 2: Corridor-biased terminal projection
In `project_out_of_disk` (now in `clearance.rs`), change the standoff direction so a terminal
inside a wall-end endpoint's clearance disk projects toward the corridor side rather than
radially. Use the `toward` argument — the adjacent waypoint, already passed at both call sites in
`ensure_endpoint_clearance` (start terminal: `toward = end`; goal terminal: `toward = start`) — to
select the disk-boundary point that faces the onward route, so the following chord
`(standoff → next_waypoint)` clears the disk. Preserve the existing degenerate fallbacks
(coincident terminal → `toward` → portal normal) and the clearance-safe guarantee (standoff still
`clearance_radius` from `raw_endpoint`, on/outside the boundary) — Invariant I1. The emitted
terminal may differ from the raw start/goal; the true target remains on the steering
`destination` (no consumer change — engagement keys off raw target distance). The existing
terminal-standoff tests assert properties, not a radial coordinate (see AC), so this change needs
no edits to them.

### Task 3: Wraparound fixtures and regressions
Add, in the clearance module's tests: (a) a `find_path` wraparound fixture — 3–4 regions around a
wall-shaped hole, `agent_radius` tuned so the around-the-end corridor exceeds `2 * clearance`,
goal in the far-side eroded band — asserting `Some` plus per-segment clearance against every wide
endpoint (reuse the `L_CORRIDOR_ENDPOINTS`-style per-segment oracle); (b) a characterization test
that a sub-`2 * clearance` wall-end pinch still returns `None`; (c) a steering integration test
(reusing the `ConcaveCorner`/`section` fixture style in `agent_steering/tests.rs`, which already
ticks an agent against a `nav_graph` and tracks `has_path`/`arrived`/`blocked`) that a chaser on
the routable wraparound reaches the goal region without latching `blocked && !has_path` — and note
in the test that stuck-recovery does not arm for a pathless blocked agent, so reaching the far
region proves a real route (Invariant I2). The wraparound `find_path` fixture must fail before
Task 2 and pass after. Epsilon comparisons only; `<subject>_<verb>_<expected_outcome>` names; a
one-line comment on each naming what it guards.

## Sequencing

**Phase 1 (sequential):** Task 1 — the behavior-preserving split; Task 2 edits the extracted
`clearance.rs`, so it must exist first.
**Phase 2 (sequential):** Task 2 — the projection fix in `clearance.rs`.
**Phase 3 (sequential):** Task 3 — fixtures consume the fixed projection (the wraparound
`find_path` fixture must fail pre-Task-2, pass post).

## Invariants

| # | Invariant | Established by | Preserved / threatened at | Verified by |
|---|-----------|----------------|---------------------------|-------------|
| I1 | A projected terminal standoff sits exactly `clearance_radius` from `raw_endpoint` (on/outside the disk boundary), AND the onward chord to the adjacent waypoint clears the disk by the effective clearance. | Task 2 (corridor-biased projection) | Threatened if the corridor-biased direction is placed inside the disk or off-boundary; the degenerate fallbacks must still emit a finite on-boundary point. | AC 1, AC 5 (per-segment oracle + standoff-regression properties) |
| I2 | The `blocked && !has_path` freeze is only ever cleared by a genuine `find_path` route, never by stuck-recovery or a replan-admission clause. | Prior commitments (`has_stuck_recovery_intent`, `blocked_destination_now_directly_routable`) | Preserved — this spec adds no recovery path; the steering test must not let recovery mask a broken `find_path`. | AC 2 |
| I3 | Class-2 geometry (wall-end gap `< 2 * clearance`) still returns `None`; the fix widens only class-1 (vocabulary-too-weak) coverage. | Task 2 (change is direction-only, not a clearance relaxation) | Threatened if the projection ever emits a standoff closer than `clearance_radius`. | AC 3 |

## Rough sketch

The fix lives in `project_out_of_disk`. Today `direction = normalize_xz(terminal - raw_endpoint)`
(radial), with `toward` consulted only when the terminal coincides with the disk center. Change:
when a non-degenerate `toward` exists, place the standoff on the disk boundary in the half-plane
toward the corridor — e.g. the boundary point nearest the terminal but constrained to the corridor
side of the endpoint, or a direction blended toward `normalize_xz(toward - raw_endpoint)` — so
`(standoff → next_waypoint)` no longer crosses the disk. Exact vector construction is an
implementation decision; the invariant to hold is I1, verifiable with the existing per-segment
oracle (`segment_point_distance_xz(...) + CLEARANCE_EPS >= clearance_radius`).

## Open questions

- **Residual oblique-gap pinch.** The interior-chord oblique case is already handled — the repair
  routes an oblique straight chord that cuts an endpoint disk via a corner insert
  (`find_path_repairs_oblique_straight_chord_near_raw_portal_endpoint`). What remains genuinely
  unthreaded is the marginal-gap band the chicane test comment names: a gap only ~0.05 m above
  `2 * clearance` whose sole clear route is a tilted segment the axis-aligned bevels cannot emit.
  If the corridor-biased projection still leaves such a pinch on the wraparound fixture, the
  next-smallest addition is a true-tangent bevel between the two disks in
  `bevel_point`/`clearance_bevel`. Decide during implementation from the fixture — add it only if
  the far-side projection fix alone does not make the routable wraparound succeed. Do NOT
  pre-emptively build the general tangent string-pull (over-engineering for this bug, whole-repair
  blast radius).
- **Combat-slot cover quality.** The out-of-scope `region_at → resolve_region_at` swap in
  `grounded_candidate_position` would let enemies take near-wall cover slots instead of walking the
  raw-target fallback straight at the player — worth a sibling ticket if corner-combat positioning
  feels flat after this fix.
- **Promotion cleanup.** At promotion, update the `project_out_of_disk` doc comment's draft-path
  reference (`context/plans/drafts/E10--pursuit-wraparound-blocked`) to the `ready/` path.
