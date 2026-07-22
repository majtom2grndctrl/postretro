# E10 — Slow-Agent Arrival Stuck False-Positive

> **Ready.** Promoted from drafts after structural + implementability review. Deferred
> out of `E10--navmesh-capsule-clearance` review; generalizes the E10 mandatory-waypoint
> stuck gate to the ordinary arrival-deceleration case.

## Goal

Stop stuck-recovery from firing on a slow agent that is decelerating into a
destination on a clear route. An author-defined low-`move_speed` enemy must ease
through the arrival-slowdown band to `arrived` without tripping tangent recovery,
while a genuinely wedged agent — slow or fast — is still detected. Why it matters:
low-`move_speed` enemies are a supported authoring configuration, and the false
recovery makes them visibly jink sideways a few tenths of a metre from their goal.

## Background (the cause)

Stuck detection mixes an ABSOLUTE spatial floor with an INTENDED-speed arming gate,
and the two disagree for slow agents. All in `crates/postretro/src/agent_steering.rs`.

- **Absolute progress floor.** `update_stuck_ticks` (:962) increments `stuck_ticks` when
  the goal-projected per-tick displacement is below `STUCK_PROGRESS_EPSILON` (0.005 m/tick,
  `agent_steering.rs:118`). At the 60 Hz fixed tick (`DT = 1/60`) that floor is ≈ 0.3 m/s
  of intended speed. It is a distance, deliberately independent of the agent's speed.
- **Intent arming gate.** `has_stuck_recovery_intent` (:1005) arms accumulation only
  when `goal_speed > STUCK_INTENT_SPEED_EPSILON` (0.05 m/s, :113).
- **The mismatch band.** An agent whose intended `goal_speed` sits between ~0.05 and
  ~0.3 m/s is "trying" (intent armed) yet its own intended per-tick step is already
  below the absolute floor — so it accumulates `stuck_ticks` toward
  `STUCK_TICKS_THRESHOLD` (20, :106) with nothing blocking it.
- **Arrival deceleration drives the agent into that band.** The `is_final` slowdown
  branch of `goal_speed` (:758-766) returns
  `move_speed * (final_distance / slowdown_radius).clamp(0,1)`, where BOTH radii scale
  off the capsule radius: `slowdown_radius = ARRIVAL_SLOWDOWN_RADIUS_FACTOR (7.0) * radius`
  and `arrival_radius = ARRIVAL_RADIUS_FACTOR (1.5) * radius` (:461-462). For the canonical
  radius 0.35 m: `arrival_radius = 0.525`, `slowdown_radius = 2.45`. At `move_speed = 1.0`,
  `goal_speed` drops below 0.3 m/s once `final_distance < ~0.735 m`, while `arrived` only
  latches at `final_distance <= arrival_radius` (0.525 m, :759). That leaves a
  ~0.21 m tail — roughly 50 consecutive sub-floor ticks — comfortably exceeding the
  20-tick threshold (consistent with the observed ~0.64 m failure point, which sits inside
  that 0.735→0.525 m band). The canonical 4.0 m/s agent never enters the band: it stays
  above 0.3 m/s until inside `arrival_radius`, where `arrived` latches first.
- **Slow enemies are supported, not misconfigured.** `move_speed` is authored on
  `AiDescriptor` (`crates/foundation/src/data_descriptors/types/combat.rs`, wire key
  `moveSpeed`), validated only as finite and `> 0` — no lower bound, no coupling to the
  capsule radius. It flows unchanged through
  `attach_agent(registry, id, &params, ai_desc.move_speed)`
  (`scripting/builtins/data_archetype.rs:492`) into `AgentComponent.move_speed`, while
  the capsule radius comes from the baked canonical nav agent params. A slow enemy is a
  first-class configuration.

### Prior art (out of scope, the pattern to generalize)

E10 already fixed the SAME class of false-stuck at a mandatory clearance vertex (funnel
corner-offset waypoint). `easing_onto_mandatory_waypoint` (:944) detects an agent easing
onto an un-arrived mandatory vertex, and a branch in `update_stuck_ticks` (:993-998)
relaxes the stuck floor to `MANDATORY_EASING_PROGRESS_EPSILON` for those ticks (so the
eased-but-progressing agent stays above the floor and does not accumulate). That gate is
narrow: it covers ONLY mandatory clearance vertices, whose tight collision-scale band
forces sub-`move_speed` easing.
This issue is the GENERAL case — the same absolute-floor-vs-intended-speed mismatch
during ordinary arrival deceleration at ANY destination, for any slow-enough agent.

### Current state of the regression instrument

The in-crate test has been **narrowed** to
`slow_agent_clears_mandatory_clearance_vertices_without_stuck_detection`
(`crates/postretro/src/agent_steering/tests.rs:846`), which is **green**. It drives a
`move_speed = 1.0` agent around the `ConcaveCorner` fixture but stops asserting once the
cursor passes the last mandatory waypoint — its own comment defers the final
arrival-slowdown crawl to this spec by name. So on the current branch there is **no
red instrument in the tree** for the final-arrival case; the mandatory-vertex regime the
E10 branch actually fixed is what the :846 test guards.

The full-traversal form — which loops to `arrived` and therefore also asserts a
stuck-free FINAL arrival — is preserved verbatim in
[`failing-test-reference.md`](./failing-test-reference.md). Restored into `tests.rs`, it
**fails** at the final destination — around tick 837, ~0.64 m from goal (5,5), inside the
`is_final` slowdown band (not on a mandatory vertex). Re-introducing that form and making
it pass is this spec's headline instrument; the mandatory-vertex ticks earlier in the same
route already pass via the E10 gate, so only the final arrival deceleration is unfixed.

## Scope

### In scope

- Remove the false stuck accumulation for a slow agent decelerating into a destination
  on a clear route, across the `is_final` arrival-slowdown band.
- Also remove it for a very slow agent cruising a long CLEAR route below the absolute
  floor mid-route — not only in the arrival tail. `move_speed` has no authored lower
  bound (see below), so an arbitrarily-slow-but-clear cruise (intended `goal_speed` under
  the ~0.3 m/s absolute-floor equivalent, e.g. `move_speed ≈ 0.2`) is a representable,
  supported configuration that today false-trips the same way. Covering it is what selects
  the progress-relative-to-intent fix over an arrival-band-only patch (see Rough sketch).
- Keep genuine wall-wedge detection for slow agents — an agent that intends to move but
  cannot must still reach `STUCK_TICKS_THRESHOLD` and fire recovery.
- Widen/extend the stuck-signal regression coverage to assert clean slow-agent arrival
  (funnel route to `arrived`, plus a straight open-floor route with no mandatory
  waypoints so the fix is proven for ordinary arrival, not only the funnel).
- Add a slow-agent genuine-wedge test that still trips recovery (the discriminator).

### Out of scope

- The E10 mandatory-waypoint gate (`easing_onto_mandatory_waypoint` + its
  `update_stuck_ticks` branch). Prior art. The fix here must compose with it, not
  duplicate or remove it.
- Navmesh bake, funnel offset, or clearance geometry changes (all E10-owned).
- Retuning the canonical 4.0 m/s agent's arrival, acceleration, or slowdown radii.
- New authoring surface or validation bounds on `AiDescriptor.move_speed`.
- Any change to what stuck recovery DOES once armed (tangent-slide window is unchanged).

## Acceptance criteria

- [ ] A `move_speed = 1.0` agent following the `ConcaveCorner` funnel route reaches
      `arrived` with `stuck_ticks` never reaching `STUCK_TICKS_THRESHOLD` and
      `unstick_window_remaining` never leaving 0 — the entire arrival deceleration, not
      just the mandatory-vertex ticks. Restore the full-traversal form from
      [`failing-test-reference.md`](./failing-test-reference.md) (absent in-tree today; the
      in-crate test is narrowed to the green `tests.rs:846` clearance-vertex case) and make
      it pass.
- [ ] A slow agent (`move_speed = 1.0`) crossing an open floor with NO mandatory
      waypoints decelerates into its destination and latches `arrived` without stuck
      accumulation reaching the threshold (runnable stuck-signal test; proves the fix is
      general arrival behavior, independent of the mandatory-waypoint machinery).
- [ ] A very slow agent (`move_speed ≈ 0.2` — intended cruise step below the absolute
      floor for the whole traverse) crossing a long, CLEAR, straight open route at full
      intended speed never accumulates `stuck_ticks` to `STUCK_TICKS_THRESHOLD` (proves the
      fix covers sub-floor cruise mid-route, not only the arrival tail — the case that
      distinguishes the progress-relative-to-intent fix from an arrival-band-only patch).
- [ ] A slow agent (`move_speed = 1.0`) driven into a wall with sustained goal intent
      still reaches `STUCK_TICKS_THRESHOLD` and fires recovery (`unstick_window_remaining`
      becomes nonzero). Real wedges remain detectable at low speed. Follow the canonical
      fire-next-tick pattern (`stuck_detection_reaches_threshold_then_recovery_fires_next_tick`):
      recovery arms the tick AFTER the threshold is reached, so the test ticks once past
      threshold before asserting `unstick_window_remaining != 0`.
- [ ] The canonical 4.0 m/s agent is unchanged: existing steering tests stay green,
      including `funnel_path_routes_concave_corner_without_stuck_detection` and
      `stuck_detection_reaches_threshold_then_recovery_fires_next_tick`.
- [ ] The E10 mandatory-waypoint gate still holds:
      `slow_agent_clears_mandatory_clearance_vertices_without_stuck_detection`
      (`agent_steering/tests.rs:846`) stays green as the required anchor, and the new
      detector logic does not double-count or bypass that gate. Concretely: KEEP the
      mandatory-easing branch (the `MANDATORY_EASING_PROGRESS_EPSILON` floor) intact and
      relax only the non-mandatory (`else`) floor — a unified relative test that drops the
      mandatory branch as "redundant" re-opens the exact false positive that branch fixed
      (a slow agent's legitimate goal-projected dip while rounding a mandatory vertex).

## Rough sketch

The fix lives entirely in the stuck detector (`update_stuck_ticks` /
`has_stuck_recovery_intent` / their constants). **Direction 1 (progress-relative-to-intent)
is the approach**; Directions 2 and 3 are documented fallbacks, taken only if the
wall-wedge discriminator cannot be made to trip cleanly under Direction 1. The root cause is
precisely an ABSOLUTE floor compared against a SPEED-dependent intended step, so the fix that
compares achieved progress against the agent's OWN intended step dissolves the bug class
rather than patching around it — and it is the only direction that covers the sub-floor
mid-route cruise scope item for free. Whichever is chosen must satisfy every AC above.

1. **Progress-relative-to-intent (the chosen approach).** Flag stuck only when actual
   per-tick progress falls far below the agent's OWN intended step (`goal_speed * DT`),
   e.g. progress below some fraction of intended, rather than below a fixed absolute
   epsilon. Naturally scales with `move_speed` and with arrival easing, and covers
   sub-floor cruise as well as the arrival tail. Risk: a genuinely blocked agent still
   "intends" to move, so the relative test must compare achieved vs. intended and still
   trip when achieved ≈ 0 while intended > 0 — the wall-wedge AC is the guard. Touches the
   core detector; most general.
2. **(Fallback) Suspend/relax accumulation in the intended arrival-deceleration band.**
   Extend the E10 gate pattern: when the agent is in the `is_final` slowdown branch
   (intended `goal_speed` throttled below the absolute floor by arrival easing, not by a
   block), hold `stuck_ticks` clear — analogous to `easing_onto_mandatory_waypoint`.
   Narrower, lower-risk, composes cleanly with the existing gate, but a targeted patch that
   does NOT satisfy the sub-floor-cruise AC (a slow agent cruising a long open corridor
   below the floor mid-route is still mis-flagged) — the reason it is a fallback, not the
   pick.
3. **(Fallback) Scale the floor by speed.** Make `STUCK_PROGRESS_EPSILON` a function of
   `move_speed` (or `goal_speed`). Simplest change, but couples a spatial floor to a
   kinematic parameter and picks a slope that must still catch a slow wedge.

Interaction constraints for any choice: must not regress the canonical ~4.0 m/s agent;
must still catch real wall-wedge stalls (the reason recovery exists — AC 3/4); must
compose with the E10 mandatory-waypoint gate rather than duplicate it; the mandatory-gate
branch and the arrival-band relaxation should not both zero `stuck_ticks` in a way that
masks a mandatory-vertex wedge.

Validation instruments already exist: the `ConcaveCorner` fixture and the `stuck_ticks` /
`unstick_window_remaining` signals in `agent_steering/tests.rs`. Reuse them; add the
open-floor arrival and slow-wedge cases. Note there is NO wall-free fixture in the tree
today (both `ConcaveCorner` and `LWall` carry walls), so the AC2/AC3 open-floor route must
be constructed — either a wall-free route through `ConcaveCorner`'s floor (e.g. `x = 5`,
`z` from `-1 → 7`, clear of the walls) driven via `set_manual_path` (which clears
`mandatory_waypoints`), or a trivial floor-only `CollisionWorld` quad. Either is small;
the AC4 wall-wedge case can reuse the existing
`stuck_detection_reaches_threshold_then_recovery_fires_next_tick` straight-into-wall
pattern with `move_speed = 1.0`.

## Related observations

- **Play-test: enemies pause before arriving at the player.** Live play shows chasing
  enemies visibly decelerate/pause shortly before reaching the player. Likely the
  ordinary arrival-deceleration band (`is_final` slowdown, this spec's subject) and/or
  the combat-slot engagement band (`combat_positioning.rs`) holding enemies at ~attack
  range — i.e. the same arrival-deceleration tuning this spec governs. A symptom to
  validate the fix against once implemented, not a confirmed bug in its own right; may
  turn out to be intended engagement-range behavior rather than a stuck-adjacent defect.

## Open questions

- **Threshold vs. epsilon tuning.** The new relative fraction (progress vs. intended step)
  needs a value proven by the slow-wedge discriminator test to still trip within a bounded
  tick budget. Left as an implementation decision anchored by the AC — not a blocker.

### Resolved / spun out

- **Slow cruise below the absolute floor mid-route — resolved: in scope.** `move_speed`
  has no authored lower bound, so arbitrarily-slow-but-clear cruise is a representable,
  supported configuration; it is now covered (the sub-floor-cruise In-scope bullet and its
  AC) and is precisely why Direction 1 is the pick rather than the arrival-band-only
  Direction 2.
- **Tangential/jittering wedge at a mandatory vertex escapes escalation — spun out.** This
  is the MIRROR IMAGE defect: a false NEGATIVE in the prior-art E10 mandatory-waypoint gate
  (a real wedge that never escalates), not the false positive this spec removes. It needs
  its own nav-geometry repro fixture (a mandatory vertex the capsule cannot plane-pass while
  still sliding along it — none exists today), and folding it in would drag an
  opposite-signed bug into a tidy detector fix. Tracked separately in
  [`drafts/E10--mandatory-vertex-wedge-escapes/`](../../drafts/E10--mandatory-vertex-wedge-escapes/index.md).
  Constraint retained here: any fix there must compose with this spec's arrival-band
  relaxation without the two jointly masking a mandatory-vertex wedge.
