# E10 — Pursuit Goes `blocked` on a Wraparound Route

> **Draft.** Deferred out of E10 play-testing. Sibling to `E10--slow-agent-arrival-stuck`.
> Fixes the far-side pinch-gap residual in `nav/path.rs` that freezes pursuit around a
> wall. Code grounding and fix-option analysis in [`research.md`](./research.md).

## Goal

A chasing enemy that tracks the player fine around one corner freezes —
`state:alert speed:0.00 arrived:false blocked:true has_path:false` — when the player wraps
around a freestanding wall to its far-middle, even though the U-shaped route around the wall
end is navmesh-reachable and within leash. Make `find_path` return a valid route for a
genuinely-threadable wraparound so pursuit continues, instead of returning `None`.

**Not a line-of-sight issue.** There is no LOS system today; the target is simply on the far
side of a wall and the route must wrap around it. The freeze is a pathing failure, not a
perception one (see the separate future line-of-sight spec).

## Background (the cause)

Confirmed by code trace (see `research.md`). The freeze is `find_path` → `None` out of
`ensure_endpoint_clearance` (`nav/path.rs`); both endpoint region resolutions succeed, so the
target IS reachable — the `None` is a repair failure, not an unreachable goal.

- **Radial terminal projection lands the standoff on the wrong side.** `project_out_of_disk`
  projects a start/goal that snapped into a wall-end endpoint's clearance disk **radially**
  outward (`raw_endpoint + normalize(terminal - raw_endpoint) * clearance`). When the terminal
  is on the FAR side of the endpoint from the corridor, the standoff is planted on the far
  side; the next chord (standoff → next waypoint, back toward the corridor) re-cuts the same
  disk, and the axis-aligned bevel vocabulary (`bevel_point`) can only emit per-gate
  normal-axis offsets — it cannot walk the standoff around the endpoint to the corridor side.
  The repair re-bevels the same incursion until `repairs_remaining` (`obstacles.len()*4`)
  exhausts → `None`.
- **The route physically exists.** The Simple-Stupid-Funnel string-pull (`funnel` over
  `inset_portals`) already produces a valid taut polyline around the wall end; the failure is
  at the seam where `ensure_endpoint_clearance` cannot re-express that route in its
  (radial + axis-aligned) vocabulary. This is a "repair vocabulary too weak" failure
  (class 1, code-fixable), distinct from a genuinely-too-tight corridor (class 2 — see Out of
  scope).

## Scope

### In scope

- **Split `nav/path.rs` first.** It is at 793 production lines (~800 split-before-extend
  threshold). Extract the clearance-repair cluster (`CLEARANCE_EPS`,
  `segment_point_distance_xz`, `bevel_point`, `clearance_bevel`, `route_out_of_disk`,
  `MIN_XZ_LEN_SQ`, `project_out_of_disk`, `PathPoint`, `ensure_endpoint_clearance`) into a
  sibling `nav/path/clearance.rs`, leaving the core A*/SSF funnel in `path.rs`. Behavior-
  preserving; the seam is `FunnelEndpoint`/`FunnelGate` plus the `find_path` call.
- **Corridor-biased terminal projection.** Redirect `project_out_of_disk` so a terminal
  snapped into a wall-end endpoint's clearance disk projects to a standoff on the CORRIDOR
  side (toward the adjacent waypoint the call sites already pass as `toward`), not radially
  outward — so the onward chord does not re-cut the disk. The true target stays on
  `AgentComponent.destination` (unchanged).
- **Wraparound regression coverage.** A `find_path` fixture for a freestanding-wall
  wrap-to-far-middle route (currently `None`) that now returns `Some` with every segment
  clearing every wide endpoint; and a steering-level test that a chaser on such a route
  reaches the far region without ever latching `blocked && !has_path`.

### Out of scope

- **`combat_positioning` strict-`region_at` grounding gap** (`combat_positioning.rs:289,297`
  ground candidates with `region_at`, not the snapping `resolve_region_at`). Confirmed a
  latent targeting-QUALITY defect (drops good near-wall cover slots), NOT the freeze cause —
  the `unwrap_or(target.position)` fallback (`ai.rs:753`) routes the raw target through the
  same `find_path`, so fixing `find_path` unfreezes pursuit regardless. File as a separate
  follow-up.
- **Genuinely-unroutable pinches (class 2).** A wall-end passage narrower than `2 * clearance`
  (0.84 m under production defaults `agent_radius` 0.4, `cell_size` 0.25) has no clearance-safe
  crossing; only bake-time/authoring widening fixes it. `find_path_returns_none_for_unthreadable_pinch_gap_limit`
  must still return `None`.
- Tangent/visibility string-pull rewrite; raising `repairs_remaining` (confirmed no-op); LOS /
  disengage-on-sight; the arrival-deceleration "pause before arriving" feel
  (`E10--slow-agent-arrival-stuck`).

## Acceptance criteria

- [ ] `find_path` on a freestanding-wall wrap-to-far-middle fixture — where the around-the-end
      corridor is wider than `2 * clearance` and the goal sits in the far-side eroded band —
      returns `Some`, and every emitted segment clears every wide-portal endpoint by the
      effective clearance (within epsilon). The same query returns `None` before the fix.
- [ ] A chasing agent following that route reaches the goal's region and never latches
      `blocked && !has_path && speed≈0` on any tick while the target is live and within leash.
- [ ] A genuinely-unroutable sub-`2 * clearance` pinch still returns `None`
      (`find_path_returns_none_for_unthreadable_pinch_gap_limit` stays green).
- [ ] The R1 standoff regressions and all existing `nav::` funnel/clearance/path tests stay
      green; the split is behavior-preserving (no test changes beyond module paths).
- [ ] `nav/path.rs` production code drops below the ~800-line threshold after the split; the
      clearance cluster lives in its own module.

## Tasks

### Task 1: Split the clearance-repair cluster out of `nav/path.rs`
Behavior-preserving extraction of the clearance-repair block (`CLEARANCE_EPS`,
`segment_point_distance_xz`, `bevel_point`, `clearance_bevel`, `route_out_of_disk`,
`MIN_XZ_LEN_SQ`, `project_out_of_disk`, `PathPoint`, `ensure_endpoint_clearance`) into a new
`crates/postretro/src/nav/path/clearance.rs` submodule (`path.rs` gains `mod clearance;`),
leaving the core A*/SSF funnel (`find_path`, `astar_corridor`, `funnel`, `inset_portals`,
`FunnelEndpoint`/`FunnelGate`) in `path.rs`. Thread visibility so `find_path`/`funnel` still
call `ensure_endpoint_clearance` and the extracted helpers see `FunnelEndpoint`/`FunnelGate`.
Move the clearance-specific `#[cfg(test)]` cases alongside their code. No logic changes; every
`nav::` test stays green.

### Task 2: Corridor-biased terminal projection
In `project_out_of_disk` (now in `clearance.rs`), change the standoff direction so a terminal
inside a wall-end endpoint's clearance disk projects toward the corridor side rather than
radially. Use the `toward` argument — the adjacent waypoint, already passed at both call sites
in `ensure_endpoint_clearance` — to select the disk-boundary point that faces the onward route,
so the following chord `(standoff → next_waypoint)` clears the disk. Preserve the existing
degenerate fallbacks (coincident terminal → `toward` → portal normal) and the clearance-safe
guarantee (standoff still `clearance_radius` from `raw_endpoint`, on/outside the boundary). The
emitted terminal may differ from the raw start/goal; the true target remains on the steering
`destination` (no consumer change — engagement keys off raw target distance).

### Task 3: Wraparound fixtures and regressions
Add, in the clearance module's tests: (a) a `find_path` wraparound fixture — 3–4 regions around
a wall-shaped hole, `agent_radius` tuned so the around-the-end corridor exceeds `2 * clearance`,
goal in the far-side eroded band — asserting `Some` plus per-segment clearance against every
wide endpoint (reuse the `L_CORRIDOR_ENDPOINTS`-style oracle); (b) a characterization test that
a sub-`2 * clearance` wall-end pinch still returns `None`; (c) a steering integration test
(reusing the `ConcaveCorner`/`section` fixture style) that a chaser on the routable wraparound
reaches the goal region without latching `blocked && !has_path`. The wraparound `find_path`
fixture must fail before Task 2 and pass after. Epsilon comparisons only;
`<subject>_<verb>_<expected_outcome>` names; a one-line comment on each naming what it guards.

## Sequencing

**Phase 1 (sequential):** Task 1 — the split; Task 2 edits the extracted module.
**Phase 2 (sequential):** Task 2 — the projection fix in `clearance.rs`.
**Phase 3 (sequential):** Task 3 — fixtures consume the fixed projection (the wraparound
`find_path` fixture must fail pre-Task-2, pass post).

## Rough sketch

The fix lives in `project_out_of_disk`. Today `direction = normalize_xz(terminal -
raw_endpoint)` (radial), with `toward` consulted only when the terminal coincides with the disk
center. Change: when a non-degenerate `toward` exists, place the standoff on the disk boundary in
the half-plane toward the corridor — e.g. the boundary point nearest the terminal but
constrained to the corridor side of the endpoint, or a direction blended toward
`normalize_xz(toward - raw_endpoint)` — so `(standoff → next_waypoint)` no longer crosses the
disk. Exact vector construction is an implementation decision; the invariant to hold is:
standoff is `clearance_radius` from `raw_endpoint` (on/outside the boundary) AND the onward chord
to the next waypoint clears the disk, verifiable with the existing per-segment oracle
(`segment_point_distance_xz(...) + CLEARANCE_EPS >= clearance_radius`).

## Open questions

- **Residual oblique-gap pinch.** If, on the wraparound fixture, the corridor-biased projection
  still leaves an oblique interior pinch the axis-aligned bevels cannot thread (a gap marginally
  above `2 * clearance` whose only route is a tilted segment), the next-smallest addition is a
  true-tangent bevel between the two disks in `bevel_point`/`clearance_bevel`. Decide during
  implementation from the fixture — add it only if the far-side projection fix alone does not
  make the routable wraparound succeed. Do NOT pre-emptively build the general tangent
  string-pull (over-engineering for this bug, whole-query blast radius).
- **Combat-slot cover quality.** The out-of-scope `region_at → resolve_region_at` swap in
  `combat_positioning` would let enemies take near-wall cover slots instead of walking the
  raw-target fallback straight at the player — worth a sibling ticket if corner-combat
  positioning feels flat after this fix.
