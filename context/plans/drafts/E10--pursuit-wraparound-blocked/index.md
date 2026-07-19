# E10 — Pursuit Goes `blocked` on a Wraparound Route

> **Draft.** Deferred out of E10 play-testing. Sibling to the deferred
> `E10--slow-agent-arrival-stuck` draft; generalizes the R1 far-side pinch-gap
> residual in `path.rs` to a live pursuit failure.

## Goal

A chasing enemy tracks the player fine around ONE corner (~90° from the player's
last position). But when the player rounds two corners — from one side of a
freestanding wall, around its end, to the middle of the far side — the enemy goes
`blocked` and stops: the debug HUD reads
`state:alert speed:0.00 arrived:false blocked:true has_path:false`. The target is
still within leash and navmesh-reachable by a U-shaped route around the wall end,
so the enemy should keep pursuing instead of freezing.

**Not a line-of-sight issue.** There is no LOS system today — this is NOT "lost
sight of the player" (see the separate future line-of-sight spec). The target is
simply on the far side of a wall and the route requires wrapping around it; the
freeze is a pathing failure, not a perception one. Do not conflate the two.

## Suspected roots (hypotheses, not decided — confirm against code)

1. **Leading suspect: the R1 far-side pinch-gap residual.** The just-landed R1 fix
   (`crates/postretro/src/nav/path.rs`, `ensure_endpoint_clearance` +
   `project_out_of_disk`) projects a start/goal snapped into a wide portal
   endpoint's clearance disk out to a walkable standoff. That reliably routes when
   the standoff and the onward corridor are on the SAME side of the endpoint. When
   the target/standoff instead lands in the eroded band on the FAR side of an
   endpoint from the corridor it must take, the funnel bends at that endpoint's
   inset corner and the axis-aligned bevel repair can still churn to `None` — the
   architecture's pre-existing pinch-gap limit, documented in path.rs's chicane
   regression test. A wraparound-to-far-middle route is exactly that far-side
   geometry.

   This residual is reachable under the PRODUCTION navmesh defaults, not just an
   edge config. Baked portal endpoints are cell-lattice-aligned, so distinct
   endpoints are only `>= cell_size` apart; two endpoint clearance disks overlap
   whenever their centers are `< 2 * clearance` apart, where `clearance =
   agent_radius + SKIN_DISTANCE`. With the shipped `nav_cell_size` (0.25 m) and
   `nav_agent_radius` (0.4 m), `2 * clearance` is 0.84 m — well above `cell_size`
   — so the `cell_size > 2 * clearance` no-overlap condition does NOT hold, and
   adjacent wide-portal clearance disks CAN overlap. A start/goal terminal
   projected out of one disk (`project_out_of_disk`) can then land inside an
   overlapping neighbor and be re-projected; `ensure_endpoint_clearance`'s repair
   budget hard-bounds that churn to a clean `None` rather than a spin, panic, or
   grazing path — which is exactly the wraparound `blocked` freeze this spec
   describes. See the `project_out_of_disk` doc comment in
   `crates/postretro/src/nav/path.rs` for the mechanism.
2. **Combat-slot reachability.** `combat_positioning` scores candidate slots
   around the target via `find_path`. If every wraparound candidate hits the same
   repair limit and the raw-target fallback also fails to route, the agent is left
   path-less — `blocked`.
3. **A genuine multi-clearance-vertex funnel/repair gap** on U-shaped routes
   carrying more than one clearance vertex in series.

## Why it matters

Chasing a player who breaks around a wall is core boomer-shooter behavior;
freezing there reads as broken AI.

## Relationship

Generalizes the R1 terminal-projection pinch-gap residual (`path.rs`). Sibling to
the deferred `E10--slow-agent-arrival-stuck` draft. Distinct from the (future)
line-of-sight / last-known-position work.

## Rough validation instruments (sketch)

- A freestanding-wall fixture with the agent on one side and the target routed to
  the far-middle of the other side. Assert the agent never enters
  `blocked && !has_path && speed≈0` while the target is live and within leash, and
  that it closes distance.
- A direct `find_path` test for a wraparound route whose endpoint sits in the
  far-side eroded band.

## Out of scope

- Line-of-sight / disengage-on-sight (separate future spec).
- The arrival-deceleration "pause before arriving" feel (the
  `E10--slow-agent-arrival-stuck` draft).
