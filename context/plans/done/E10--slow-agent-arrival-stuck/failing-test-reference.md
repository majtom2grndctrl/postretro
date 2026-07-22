# Reference: the failing regression test

This is a verbatim copy, captured for reference, of the test that surfaced this
issue. It lives here so the spec stays self-contained after the test is
**narrowed** on the `feature/E10--navmesh-capsule-clearance` branch to cover only
the clearance-vertex regression E10 actually fixed (asserting no false stuck
while easing onto the mandatory clearance vertices, and stopping before the
final arrival-slowdown band). The full-traversal form below — which loops to
`arrived` and therefore also asserts a stuck-free *final arrival* — is what this
spec exists to make pass.

**Provenance:** `crates/postretro/src/agent_steering/tests.rs`
(`slow_agent_funnel_path_routes_concave_corner_without_stuck_detection`), as of
the E10 review.

**Observed failure (unmodified form):** panics at tick 837, `pos ≈ (5.42, _, 4.52)`,
`stuck_ticks = 20` — roughly `0.64 m` from the goal `(5, 5)`, *past* every
mandatory clearance vertex (the interior funnel waypoints at `x ≈ 7.37`), inside
the `is_final` arrival-slowdown band. The stuck floor (`STUCK_PROGRESS_EPSILON`,
absolute) is tripped by the agent's legitimately-eased arrival velocity, not by a
block. See the spec's root-cause section.

The reusable instruments are the `ConcaveCorner` fixture and the `stuck_ticks` /
`unstick_window_remaining` signals.

```rust
#[test]
fn slow_agent_funnel_path_routes_concave_corner_without_stuck_detection() {
    // Regression: the mandatory-waypoint easing band was silently calibrated to
    // move_speed ~= 4.0. An author-defined slower enemy eases onto a mandatory
    // clearance vertex with per-tick goal-projected progress below
    // STUCK_PROGRESS_EPSILON (an absolute distance), which used to trip false
    // stuck recovery at the clearance vertex this machinery exists to smooth.
    let corner = ConcaveCorner::fixture();
    let world = corner.collision_world();
    let graph = corner.nav_graph();
    let params = graph.agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.2, 1.2, &params);
    // 1.0 m/s: well below the canonical 4.0 the easing band was tuned against,
    // and inside the regime where the eased mandatory tail dips under the stuck
    // floor while the intent gate still arms recovery.
    {
        let mut agent = registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .clone();
        agent.move_speed = 1.0;
        registry.set_component(id, agent).unwrap();
    }
    let destination = Vec3::new(5.0, rest_y(&params), 5.0);
    set_destination(&mut registry, id, destination);

    // Slower agent, same route: allow proportionally more ticks than the 4.0
    // m/s variant's 600.
    for tick_index in 0..4000 {
        // Neutral vertical channel, matching the canonical no-stuck test, so the
        // assertion exercises the funnel/capsule route rather than floor settle.
        tick(&mut registry, &world, Some(&graph), 0.0, DT);
        let agent = registry.get_component::<AgentComponent>(id).unwrap();
        if tick_index == 0 {
            assert!(
                agent.path.len() >= 3,
                "concave route must retain an interior funnel waypoint: {:?}",
                agent.path
            );
        }
        assert!(
            agent.stuck_ticks < STUCK_TICKS_THRESHOLD,
            "slow funnel route reached stuck detection on tick {tick_index}: pos={:?}, stuck_ticks={}, path={:?}",
            agent_position(&registry, id),
            agent.stuck_ticks,
            agent.path,
        );
        assert_eq!(
            agent.unstick_window_remaining, 0,
            "slow funnel route must not fire recovery on tick {tick_index}: pos={:?}",
            agent_position(&registry, id),
        );
        if agent.arrived {
            return;
        }

        if tick_index == 0 {
            // Freeze the fresh plan so periodic refresh cannot replace the route
            // mid-traverse, matching the canonical-speed variant.
            let mut frozen_plan = agent.clone();
            frozen_plan.replan_cooldown_ticks = u32::MAX;
            registry.set_component(id, frozen_plan).unwrap();
        }
    }

    panic!(
        "slow funnel route did not reach the concave-corner goal within 4000 ticks: pos={:?}",
        agent_position(&registry, id),
    );
}
```
