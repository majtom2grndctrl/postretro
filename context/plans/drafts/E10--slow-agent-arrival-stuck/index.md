# E10 — Slow-Agent Arrival Stuck False-Positive

> **Draft.** Deferred out of `E10--navmesh-capsule-clearance` review. Generalizes the
> E10 mandatory-waypoint stuck gate to the ordinary arrival-deceleration case.

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

- **Absolute progress floor.** `update_stuck_ticks` increments `stuck_ticks` when the
  goal-projected per-tick displacement is below `STUCK_PROGRESS_EPSILON` (0.005 m/tick,
  agent_steering.rs:92). At the 60 Hz fixed tick (`DT = 1/60`) that floor is ≈ 0.3 m/s
  of intended speed. It is a distance, deliberately independent of the agent's speed.
- **Intent arming gate.** `has_stuck_recovery_intent` (:894) arms accumulation only
  when `goal_speed > STUCK_INTENT_SPEED_EPSILON` (0.05 m/s, :87).
- **The mismatch band.** An agent whose intended `goal_speed` sits between ~0.05 and
  ~0.3 m/s is "trying" (intent armed) yet its own intended per-tick step is already
  below the absolute floor — so it accumulates `stuck_ticks` toward
  `STUCK_TICKS_THRESHOLD` (20, :80) with nothing blocking it.
- **Arrival deceleration drives the agent into that band.** The `is_final` slowdown
  branch of `goal_speed` (:683-692) returns
  `move_speed * (final_distance / slowdown_radius).clamp(0,1)`, where
  `slowdown_radius = ARRIVAL_SLOWDOWN_RADIUS_FACTOR (7.0) * arrival_radius` and
  `arrival_radius = ARRIVAL_RADIUS_FACTOR (1.5) * radius`. For the canonical radius
  0.35 m: `arrival_radius = 0.525`, `slowdown_radius = 3.675`. At `move_speed = 1.0`,
  `goal_speed` drops below 0.3 m/s once `final_distance < ~1.1 m`, while `arrived` only
  latches at `final_distance <= arrival_radius` (0.525 m, :684). That leaves a
  ~0.575 m tail — roughly 130+ consecutive sub-floor ticks — far exceeding the
  20-tick threshold. The canonical 4.0 m/s agent never enters the band: it stays above
  0.3 m/s until inside `arrival_radius`, where `arrived` latches first.
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
corner-offset waypoint). `easing_onto_mandatory_waypoint` (:834) detects an agent easing
onto an un-arrived mandatory vertex, and a gate in `update_stuck_ticks` (:864-874)
zeroes `stuck_ticks` for those ticks. That gate is narrow: it covers ONLY mandatory
clearance vertices, whose tight collision-scale band forces sub-`move_speed` easing.
This issue is the GENERAL case — the same absolute-floor-vs-intended-speed mismatch
during ordinary arrival deceleration at ANY destination, for any slow-enough agent.

### Current state of the regression instrument

`slow_agent_funnel_path_routes_concave_corner_without_stuck_detection`
(`crates/postretro/src/agent_steering/tests.rs:688`) drives a `move_speed = 1.0` agent
around the `ConcaveCorner` fixture and asserts `stuck_ticks < STUCK_TICKS_THRESHOLD`
every tick until `arrived`. As committed it **fails** at the final destination — around
tick 837, ~0.64 m from goal (5,5), inside the `is_final` slowdown band (not on a
mandatory vertex). The mandatory-vertex ticks earlier in the same route already pass via
the E10 gate; only the final arrival deceleration is unfixed. (This is the un-narrowed,
currently-red form of the instrument the E10 review referenced.)

A verbatim copy of this test, with its observed-failure signature, is preserved
adjacent to this spec in [`failing-test-reference.md`](./failing-test-reference.md)
— the E10 branch narrows the in-crate test to the clearance-vertex regression, so
the reference captures the full-traversal form this spec exists to make pass.

## Scope

### In scope

- Remove the false stuck accumulation for a slow agent decelerating into a destination
  on a clear route, across the `is_final` arrival-slowdown band.
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
      just the mandatory-vertex ticks (the currently-red tests.rs:688 test passes).
- [ ] A slow agent (`move_speed = 1.0`) crossing an open floor with NO mandatory
      waypoints decelerates into its destination and latches `arrived` without stuck
      accumulation reaching the threshold (runnable stuck-signal test; proves the fix is
      general arrival behavior, independent of the mandatory-waypoint machinery).
- [ ] A slow agent (`move_speed = 1.0`) driven into a wall with sustained goal intent
      still reaches `STUCK_TICKS_THRESHOLD` and fires recovery (`unstick_window_remaining`
      becomes nonzero). Real wedges remain detectable at low speed.
- [ ] The canonical 4.0 m/s agent is unchanged: existing steering tests stay green,
      including `funnel_path_routes_concave_corner_without_stuck_detection` and
      `stuck_detection_reaches_threshold_then_recovery_fires_next_tick`.
- [ ] The E10 mandatory-waypoint gate still holds: its easing/no-stuck tests stay green
      and the new detector logic does not double-count or bypass that gate.

## Rough sketch

The fix lives entirely in the stuck detector (`update_stuck_ticks` /
`has_stuck_recovery_intent` / their constants). Three directions, with trade-offs — pick
during implementation; each must satisfy every AC above.

1. **Progress-relative-to-intent (most principled).** Flag stuck only when actual
   per-tick progress falls far below the agent's OWN intended step (`goal_speed * DT`),
   e.g. progress below some fraction of intended, rather than below a fixed absolute
   epsilon. Naturally scales with `move_speed` and with arrival easing. Risk: a genuinely
   blocked agent still "intends" to move, so the relative test must compare achieved vs.
   intended and still trip when achieved ≈ 0 while intended > 0 — the wall-wedge AC is
   the guard. Touches the core detector; most general.
2. **Suspend/relax accumulation in the intended arrival-deceleration band.** Extend the
   E10 gate pattern: when the agent is in the `is_final` slowdown branch (intended
   `goal_speed` throttled below the absolute floor by arrival easing, not by a block),
   hold `stuck_ticks` clear — analogous to `easing_onto_mandatory_waypoint`. Narrower,
   lower-risk, composes cleanly with the existing gate, but a targeted patch rather than a
   root fix (a slow agent cruising a long open corridor below the floor mid-route would
   still be mis-flagged — see open questions).
3. **Scale the floor by speed.** Make `STUCK_PROGRESS_EPSILON` a function of `move_speed`
   (or `goal_speed`). Simplest change, but couples a spatial floor to a kinematic
   parameter and picks a slope that must still catch a slow wedge.

Interaction constraints for any choice: must not regress the canonical ~4.0 m/s agent;
must still catch real wall-wedge stalls (the reason recovery exists — AC 3/4); must
compose with the E10 mandatory-waypoint gate rather than duplicate it; the mandatory-gate
branch and the arrival-band relaxation should not both zero `stuck_ticks` in a way that
masks a mandatory-vertex wedge.

Validation instruments already exist: the `ConcaveCorner` fixture and the `stuck_ticks` /
`unstick_window_remaining` signals in `agent_steering/tests.rs`. Reuse them; add the
open-floor arrival and slow-wedge cases.

## Open questions

- **Slow cruise below the absolute floor mid-route.** Direction 2 only relaxes the
  arrival band; a `move_speed < ~0.3 m/s` agent cruising a long straight corridor at full
  intended speed is ALSO below the absolute floor and would still false-trip. Is such an
  agent in scope? Directions 1 and 3 cover it; direction 2 does not. Decide whether the
  spec must cover arbitrarily-slow cruise or only the arrival tail. (Note: `move_speed`
  has no authored lower bound, so arbitrarily-slow cruise is representable.)
- **Threshold vs. epsilon tuning.** Whichever direction, the new relative fraction /
  scaled floor / band predicate needs a value proven by the slow-wedge discriminator test
  to still trip within a bounded tick budget. Left as an implementation decision anchored
  by the AC.
