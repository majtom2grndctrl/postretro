# Research — E10 Pursuit Wraparound `blocked`

Investigation notes behind `index.md`. Grounded against `crates/postretro/src/nav/path.rs`
(1639 lines), `nav/mod.rs`, `combat_positioning.rs`, `scripting/systems/ai.rs`,
`agent_steering.rs`, and `crates/level-compiler/src/map_data.rs` (production nav defaults).

## Freeze chain (end to end)

`find_path` → `None` → `agent_steering.rs:414-435`: on an admitted replan, `find_path` `None`
with no surviving prior path sets `agent.blocked = agent.path.is_empty()` → empty →
`blocked:true has_path:false speed:0.00`, the HUD string in the report. The `None` originates in
`ensure_endpoint_clearance` (`nav/path.rs:562`), NOT endpoint resolution — both
`resolve_region_at` calls succeed, so the target is reachable and the failure is a repair failure.

## Mechanism

Repair loop `ensure_endpoint_clearance` (`path.rs:562-708`), budget
`repairs_remaining = obstacles.len().saturating_mul(4).max(1)` (`path.rs:594`); a violation with
budget 0 → `return None` (`path.rs:606`). Every splice/projection `continue`s without advancing
`segment_index`, so a non-converging repair burns the budget on one segment.

- **Far-side standoff.** `project_out_of_disk` (`path.rs:506-547`) projects radially:
  `raw_endpoint + normalize(terminal - raw_endpoint) * clearance`. `toward` (adjacent waypoint) is
  consulted only when the terminal sits on the disk center (`radial.len_sq <= MIN_XZ_LEN_SQ`). A
  terminal on the FAR side of the endpoint → standoff on the far side → onward chord re-cuts the
  disk → falls into the bevel block, which emits only axis-aligned per-gate offsets and cannot
  walk the standoff around the endpoint → re-bevel until budget → `None`.
- **Overlapping-disk ping-pong (production defaults).** `map_data.rs:611-617`: `agent_radius 0.4`,
  `cell_size 0.25`; `SKIN_DISTANCE 0.02` (`collision/mod.rs:127`) → `clearance 0.42`,
  `2*clearance 0.84 > cell_size 0.25`, so distinct wide-portal disks overlap. A standoff projected
  off disk A can land in overlapping disk B and re-project; the budget bounds the churn to a clean
  `None` (documented in the `project_out_of_disk` doc comment).

**Bevel vocabulary** (`bevel_point`, `path.rs:405-417`): offset is always `± portal_interior`'s
single normal axis of one gate; it cannot tilt to an arbitrary angle, so it cannot emit a segment
oblique to both portal normals.

## Class-1 vs class-2 partition (the crux)

The SSF funnel (`funnel` over `inset_portals`) does NOT lose the route — it produces a valid taut
polyline over the inset gate points. The failure is at the seam: SSF reasons about inset gate
*points*; the raw-endpoint *disks* are enforced afterward by `ensure_endpoint_clearance`, whose
(radial + axis-aligned) vocabulary sometimes cannot re-express the route SSF found.

- **Class 1 — repair vocabulary too weak (code-fixable):** gap in `2*clearance < gap <
  2*clearance + axis-aligned margin`, and the far-side-standoff case. A clearance-safe route
  physically exists but the repair can't emit it. **The wraparound is predominantly class 1** —
  open floor around a freestanding wall end is wide, so the wall-end disks don't overlap-block the
  route; what fails is the radial standoff + axis-aligned bevel.
- **Class 2 — genuinely unroutable (author problem):** gap `< 2*clearance`, disks overlap, no
  clearance-safe crossing. `find_path_returns_none_for_unthreadable_pinch_gap_limit`
  (`path.rs:1578-1637`) is canonical; correct answer is `None`. Only bake widening fixes it.

Fixture contrast: `find_path_routes_bent_twin_portal_chicane_without_dropping_to_none`
(`path.rs:1377`) corners ~0.94 apart (>0.64 = 2*clearance there) → axis-aligned repair threads
it. The unthreadable test corners 0.5 apart (<0.64) → correct `None`. The missing middle band
(gap marginally above `2*clearance`, only a tilted segment threads) is named in the chicane
comment (`path.rs:1391-1394`).

## Fix options (verdict: option ii)

| # | Option | Touches | Verdict |
|---|--------|---------|---------|
| i | True-tangent bevel between two disks | `bevel_point`, `clearance_bevel`, new tangent helper | Correct for the oblique middle-band pinch; moderate complexity. Conditional follow-on, not the headline fix. |
| ii | **Bias terminal projection toward corridor** (`toward`) | `project_out_of_disk` only | **Minimal.** Directly fixes the far-side standoff (the leading wraparound cause). Call sites already pass the adjacent waypoint as `toward`. |
| iii | Tangent/visibility string-pull over disks | `funnel`, `inset_portals`, `ensure_endpoint_clearance` (rewrite) | Over-engineering; whole-query blast radius. Do not. |
| iv | Raise `repairs_remaining` | `path.rs:594` | **No-op.** `None` is reached because the vocabulary can't express the route (or the capsule doesn't fit), not because 4× is too few. |
| v | Bake-time corridor widening | level-compiler | Out of scope (authoring); only fix for class 2. |

## Combat-positioning: out of scope (latent, not causal)

`resolve_combat_slots` (`ai.rs:922`) sets `outcome.combat_slot` via `select_combat_positions_batch`
(`combat_positioning.rs:77`); the apply pass does `outcome.combat_slot.unwrap_or(target.position)`
(`ai.rs:753`). A `combat_slot` is only ever `Some` for a position that already round-tripped
through `find_path` (`combat_positioning.rs:269`), so it can never be *more* blocked than the raw
target fallback — and the fallback routes the raw target through the SAME `find_path`
(`agent_steering.rs:414`). Therefore fixing `find_path` unfreezes pursuit regardless of slot
scoring.

Separate latent bug: `grounded_candidate_position` (`combat_positioning.rs:284`) grounds candidates
with strict `region_at` (`:289`, `:297`), not the snapping `resolve_region_at`, so near-wall cover
slots are dropped before `find_path` sees them. This is a targeting-QUALITY defect (enemies walk
the raw-target fallback straight at the player instead of taking cover), NOT the freeze — a sibling
follow-up ticket, not this spec.

## File sizes / split

`nav/path.rs`: 1639 total, `#[cfg(test)]` at 794 → **793 production lines** (right at the ~800
split-before-extend threshold; the fix + tests push past). Split the clearance-repair cluster
(`CLEARANCE_EPS`, `segment_point_distance_xz`, `bevel_point`, `clearance_bevel`, `route_out_of_disk`,
`MIN_XZ_LEN_SQ`, `project_out_of_disk`, `PathPoint`, `ensure_endpoint_clearance`) to
`nav/path/clearance.rs`. Seam is clean — the two clusters interface only via
`FunnelEndpoint`/`FunnelGate` and the `find_path` call. Other files (for reference):
`combat_positioning.rs` 837, `ai.rs` 1026, `agent_steering.rs` 1074, `nav/mod.rs` 505.

## Reproduction geometry

Freestanding wall = a solid hole in open floor; the walkable annulus/U splits into a chain of
rectangular regions (near-side → wall-end/tip → far-side, ~3–4 regions, a portal per seam). The
wall end is a convex corner whose wide-portal endpoint disk is the pinch. The goal (player against
the far face, in the eroded band) snaps into the far portal's endpoint disk; radial projection
plants the standoff on the far side → current `None`. Constructible with existing test helpers
(`section`/`region` builders, raw `NavPortal` literals, `navmesh.agent_radius = …`, raw `Vec3`
terminals in the eroded band, the `L_CORRIDOR_ENDPOINTS`-style per-segment clearance oracle).
