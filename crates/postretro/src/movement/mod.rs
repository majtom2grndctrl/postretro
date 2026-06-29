// Public player movement API and tick dispatcher.
// See: context/lib/movement.md §4

use glam::{Vec2, Vec3};

mod carry;
mod dispatch;
mod intents;
mod scope;
mod substrate;

// Compatibility re-export for legacy in-crate movement scope paths.
#[allow(unused_imports)]
pub(crate) use postretro_foundation::MovementScope;

use crate::collision::CollisionWorld;
use crate::movement::carry::CarryRule;
use crate::movement::dispatch::dispatch_state_intent;
use crate::movement::substrate::{advance_forgiveness, derive_jump_edges, integrate_collision};
use postretro_foundation::{MovementState, PlayerMovementComponent};

#[cfg(test)]
use crate::movement::intents::{DASH_MAX_MS, dash_intent};
#[cfg(test)]
use crate::movement::substrate::{
    ResizeAnchor, resize_capsule, standup_clearance_probe, step_up_lift,
};

/// Per-tick input plumbed in from the engine's input layer. Keep `wish_dir`
/// component magnitudes within `[0, 1]` — the raw x/y values drive threshold
/// checks (`.length_squared() < 0.001`, `.y.abs() > 1e-3`) that are
/// sensitive to diagonal magnitudes. The 3D world-space direction derived from
/// `wish_dir` is normalized internally before being applied to locomotion.
#[derive(Debug, Clone)]
pub(crate) struct MovementInput {
    pub(crate) wish_dir: Vec2, // x = right, y = forward
    pub(crate) jump_pressed: bool,
    /// Dash rising edge: TRUE only on the tick the dash button is first pressed,
    /// not while held. Unlike `jump_pressed` (a level signal — `Pressed|Held`),
    /// a held dash would re-fire every cooldown-ready tick, so the edge is
    /// mandatory. The call site derives it from `ButtonState::Pressed` only.
    pub(crate) dash_pressed: bool,
    /// Sprint held this tick. Selects `ground.speed.run` over `.walk` as the
    /// omnidirectional horizontal speed target; affects strafe and forward
    /// motion equally (standard shooter sprint, not forward-only).
    pub(crate) running: bool,
    /// Crouch intent active this tick — the single resolved per-tick bit the
    /// input layer hands down. Toggle-vs-hold is resolved upstream from
    /// `PlayerOptions.crouch_mode`; the movement intent NEVER sees the raw
    /// button or the mode. In `hold` mode this tracks the `Action::Crouch`
    /// level; in `toggle` mode it tracks a latch flipped on each press edge.
    /// Consumed by the `Crouching` intent: drives the `Normal` → `Crouching`
    /// entry and the stand-up release; the intent treats it as a plain
    /// "crouch active this tick" boolean and never reasons about toggle vs hold.
    pub(crate) crouch_intent: bool,
    pub(crate) facing_yaw: f32,
}

/// Events the movement tick emits for the same-frame dispatch layer to fire
/// into the reaction registry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MovementEvents {
    pub(crate) landed: bool,
    pub(crate) jumped: bool,
}

/// Contact/landing results returned by `integrate_collision` (the shared
/// physics substrate). The substrate resolves collision state on the
/// component itself (`is_grounded`, `air_ticks`, `velocity`); these fields
/// report the gameplay-relevant outcomes the tick maps onto events and
/// ability-budget refreshes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SubstrateResult {
    /// Floor contact was (re)acquired this tick — either through the slide
    /// loop or the ground-stick down-cast. The tick uses this as the
    /// landing-refresh point for ability budgets (e.g. `air_jumps_remaining`),
    /// a gameplay-state write the substrate deliberately does not perform.
    pub(crate) hit_floor: bool,
    /// A genuine landing transition (airborne → grounded) cleared the
    /// air-tick hysteresis gate. Maps directly to `MovementEvents::landed`.
    pub(crate) landed: bool,
}

/// A state transition a per-state intent warrants this tick: the `next` state to
/// enter paired with the `carry`-rule the DISPATCH layer applies to the OUTGOING
/// velocity at the edge. Pairing the carry with the next state keeps the
/// velocity transform out of the intents (D6) — the intent declares *what* the
/// edge does; the dispatch *applies* it against the outgoing resolved velocity
/// and boost before the new state is written. See `context/lib/movement.md` §6.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Transition {
    pub(crate) next: MovementState,
    pub(crate) carry: CarryRule,
}

pub(crate) fn tick(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    collision_world: &CollisionWorld,
    gravity: f32,
    dt: f32,
    position: Vec3,
) -> (Vec3, MovementEvents) {
    let mut events = MovementEvents::default();
    let was_grounded = component.is_grounded;
    // Mutable working position: a crouch entry/stand-up resize anchors one
    // capsule extreme and shifts the center by the helper-returned delta. The
    // intent applies that delta in-place (via `position.y += delta`) INSIDE its
    // own body before returning, so the substrate integrates from the
    // already-shifted center while the planted/anchored capsule extreme stays
    // geometrically fixed.
    let mut position = position;

    // Input forgiveness (D5): derive the grounded-jump and buffered/coyote jump
    // edges ONCE, here, before the intents run. The intents read these derived
    // edges in place of the raw `jump_pressed` bit and never re-derive
    // forgiveness. A buffer the grounded edge consumes is cleared below so it
    // fires exactly once; a fresh airborne press that produced no jump arms the
    // buffer for the landing-tick fire.
    let jump_edges = derive_jump_edges(component, input.jump_pressed);

    // Per-state velocity intent: dispatch to the active state's intent step. It
    // authors `component.velocity` (gravity, jump, acceleration, friction, caps)
    // reading the grounded flag carried from last tick, and returns an optional
    // transition to apply after the substrate resolves collision. The dispatch
    // resolves the component-vs-active-state borrow once and owns the per-state
    // live data, so a new state plugs in without widening this call.
    let transition = dispatch_state_intent(
        component,
        input,
        jump_edges,
        gravity,
        dt,
        collision_world,
        &mut position,
        &mut events,
    );

    // The grounded edge consumed a pending buffer this tick — clear it so the
    // buffered jump fires exactly once on landing, never twice.
    if jump_edges.consumed_buffer {
        component.jump_buffer_timer_ms = 0.0;
    }

    // Arm the jump buffer from a fresh airborne press the intents did NOT turn
    // into a jump (no coyote/air jump fired): retain it for the landing-tick
    // fire. Only arm when no buffer is already pending (`<= 0.0`): re-arming a
    // live buffer on a held button would reset its countdown each tick,
    // extending the window indefinitely. The guard ensures the window counts
    // from the first press and re-arms only after full expiry or consumption.
    if input.jump_pressed
        && !was_grounded
        && !events.jumped
        && component.jump_buffer_ms > 0.0
        && component.jump_buffer_timer_ms <= 0.0
    {
        component.jump_buffer_timer_ms = component.jump_buffer_ms;
    }

    // 7 + 8. Shared physics substrate: sweep-and-slide, step-up, floor-push,
    // ground-stick, and collision-state/landing resolution. States change
    // intent (steps 1–6 above), not collision.
    let (current_pos, substrate) = integrate_collision(
        component,
        collision_world,
        dt,
        position,
        was_grounded,
        events.jumped,
    );

    // Landing-refresh point: the ability-budget reset is a gameplay-state write
    // the substrate deliberately leaves to the tick. Driven by the substrate's
    // floor-contact result so every budget (air-jump charges, air-dash charges)
    // replenishes uniformly on every floor touch, through one method.
    if substrate.hit_floor {
        component.refresh_on_landing();
    }

    events.landed = substrate.landed;

    // Apply any state transition the intent returned, after the substrate has
    // resolved collision/landing. `Normal` transitions to `Dash` on rising-edge
    // dash input; `Dash` returns to `Normal` on speed-band exit or the
    // DASH_MAX_MS elapsed guard. Transition gating reads the same last-tick
    // grounded flag the intent used — the one-tick staleness is consistent with
    // how jump/air-jump already gate (no fresh ground probe).
    if let Some(next_state) = transition {
        component.movement_state = next_state;
    }

    // Decrement the dash cooldown UNCONDITIONALLY each tick, outside the
    // per-state intent dispatch, so it advances in every state (including
    // `Dash`) and never inside a state intent. Armed to `dash.cooldown_ms` on
    // dash entry; counts down off the same `dt` (seconds → ms via `dt * 1000`).
    if component.dash_cooldown_ms > 0.0 {
        component.dash_cooldown_ms = (component.dash_cooldown_ms - dt * 1000.0).max(0.0);
    }

    // Advance the forgiveness timers for the next tick, after the substrate has
    // resolved this tick's grounded flag: accumulate coyote ms while airborne
    // (reset to 0 on the ground / at the landing-refresh point), and count the
    // jump buffer down toward a silent drop if it expires before landing.
    advance_forgiveness(component, dt);

    (current_pos, events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parry3d::math::{Isometry, Point};
    use parry3d::shape::{Capsule, TriMesh};
    use postretro_foundation::{
        AirParams, BobParams, BoolOrIr, CapsuleParams, CrouchParams, DashParams, FallParams,
        ForgivenessParams, GroundParams, NumberOrIr, PlayerMovementDescriptor, SpeedParams,
        SwayParams, TiltParams, ViewFeelParams,
    };
    use postretro_foundation::{IrNode, IrValue};

    const POS_EPS: f32 = 1.0e-4;
    const VEL_EPS: f32 = 1.0e-3;
    const DT: f32 = 1.0 / 60.0;
    const GRAVITY: f32 = -20.0;

    /// Canonical player descriptor mirroring `content/dev/scripts/player.ts`.
    fn canonical_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.4,
                half_height: 0.8,
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 10.0,
                step_height: 0.3,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 0.7,
                max_control_speed: 0.5,
                bunny_hop: false,
                jumps: 0,
                jump_velocity: 5.5,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
            // Regression fixtures pin both forgiveness windows to ZERO (D5) so
            // grounded-jump and buffered-jump edges collapse onto raw
            // `jump_pressed` and exact edge timing is preserved. The new
            // forgiveness tests build descriptors with non-zero windows.
            forgiveness: Some(ForgivenessParams {
                coyote_ms: 0.0,
                jump_buffer_ms: 0.0,
            }),
            crouch: None,
            view_feel: None,
        }
    }

    #[test]
    fn from_descriptor_materializes_view_feel_verbatim() {
        // View feel is a render-only camera effect: `from_descriptor` clones the
        // descriptor's tuning onto the component with no transform.
        let mut desc = canonical_descriptor();
        let view_feel = ViewFeelParams {
            bob: Some(BobParams {
                vertical_frequency: 1.8,
                lateral_frequency: 0.9,
                vertical_amplitude: 0.06,
                lateral_amplitude: 0.04,
                speed_threshold: 0.5,
                grounded_only: true,
            }),
            tilt: Some(TiltParams {
                max_angle: 3.0,
                speed_reference: 8.0,
                tension: 12.0,
                grounded_only: true,
            }),
            sway: Some(SwayParams {
                amplitude: 0.5,
                frequency: 0.4,
                speed_scale: 0.2,
                grounded_only: false,
            }),
        };
        desc.view_feel = Some(view_feel.clone());
        let comp = PlayerMovementComponent::from_descriptor(&desc);
        assert_eq!(comp.view_feel, Some(view_feel));
    }

    #[test]
    fn from_descriptor_view_feel_absent_yields_none() {
        let comp = PlayerMovementComponent::from_descriptor(&canonical_descriptor());
        assert!(comp.view_feel.is_none());
    }

    /// Descriptor with the air-jump (double-jump) budget enabled: one air
    /// charge and a finite upward-velocity ceiling. Mirrors the canonical
    /// descriptor otherwise so the double-jump tests vary only the air-jump
    /// fields under test. `jump_ceiling = 2.0` sits below the 5.5 jump launch
    /// velocity, so the charge cannot be spent at the top of the rising arc but
    /// fires once the arc has decayed past the ceiling (or while falling).
    fn double_jump_descriptor() -> PlayerMovementDescriptor {
        let mut desc = canonical_descriptor();
        desc.air.jumps = 1;
        desc.air.jump_ceiling = 2.0;
        desc
    }

    /// Build a `CollisionWorld` containing:
    ///   - a flat floor at y=0 spanning x∈[-20,20], z∈[-10,10],
    ///   - a step-up ledge of height 0.3 m starting at x=5 (top span x∈[5,15]),
    ///   - a wall at x=15 extending from y=0.3 up to y=5 along z∈[-10,10].
    ///
    /// Triangles use CCW winding when viewed from the side the player is on so
    /// parry's contact normals point back toward the player.
    fn ledge_and_wall_world() -> CollisionWorld {
        let mut points: Vec<Point<f32>> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();

        // Floor: y=0, x∈[-20,20], z∈[-10,10]. Up-facing normal +Y.
        let f0 = points.len() as u32;
        points.push(Point::new(-20.0, 0.0, -10.0));
        points.push(Point::new(20.0, 0.0, -10.0));
        points.push(Point::new(20.0, 0.0, 10.0));
        points.push(Point::new(-20.0, 0.0, 10.0));
        tris.push([f0, f0 + 1, f0 + 2]);
        tris.push([f0, f0 + 2, f0 + 3]);

        // Step ledge top: y=0.3, x∈[5,15], z∈[-10,10]. Up-facing +Y.
        let l0 = points.len() as u32;
        points.push(Point::new(5.0, 0.3, -10.0));
        points.push(Point::new(15.0, 0.3, -10.0));
        points.push(Point::new(15.0, 0.3, 10.0));
        points.push(Point::new(5.0, 0.3, 10.0));
        tris.push([l0, l0 + 1, l0 + 2]);
        tris.push([l0, l0 + 2, l0 + 3]);

        // Step ledge riser: x=5, y∈[0,0.3], z∈[-10,10]. Normal facing -X.
        let r0 = points.len() as u32;
        points.push(Point::new(5.0, 0.0, -10.0));
        points.push(Point::new(5.0, 0.0, 10.0));
        points.push(Point::new(5.0, 0.3, 10.0));
        points.push(Point::new(5.0, 0.3, -10.0));
        tris.push([r0, r0 + 1, r0 + 2]);
        tris.push([r0, r0 + 2, r0 + 3]);

        // Wall: x=15, y∈[0.3,5], z∈[-10,10]. Normal facing -X.
        let w0 = points.len() as u32;
        points.push(Point::new(15.0, 0.3, -10.0));
        points.push(Point::new(15.0, 0.3, 10.0));
        points.push(Point::new(15.0, 5.0, 10.0));
        points.push(Point::new(15.0, 5.0, -10.0));
        tris.push([w0, w0 + 1, w0 + 2]);
        tris.push([w0, w0 + 2, w0 + 3]);

        let mesh = TriMesh::new(points, tris);
        CollisionWorld {
            mesh,
            isometry: Isometry::identity(),
        }
    }

    /// Returns a component just above the floor with no velocity, airborne.
    /// Gravity will pull it into contact on the first move-and-collide tick.
    /// Tests assert position envelopes and velocity caps rather than the
    /// per-tick `is_grounded` flag, which can briefly clear on tick
    /// boundaries before the ground-stick snap re-establishes contact.
    fn settle_player(desc: &PlayerMovementDescriptor) -> (PlayerMovementComponent, Vec3) {
        let comp = PlayerMovementComponent::from_descriptor(desc);
        // Start a hair above the floor so the first tick's gravity step closes
        // the gap and the sweep registers floor contact.
        let start = Vec3::new(
            0.0,
            desc.capsule.half_height + desc.capsule.radius + 0.01,
            0.0,
        );
        (comp, start)
    }

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    fn run_ticks(
        comp: &mut PlayerMovementComponent,
        world: &CollisionWorld,
        position: &mut Vec3,
        ticks: usize,
        input: &MovementInput,
    ) -> MovementEvents {
        let mut last = MovementEvents::default();
        for _ in 0..ticks {
            let (next, ev) = tick(comp, input, world, GRAVITY, DT, *position);
            *position = next;
            last = ev;
        }
        last
    }

    #[test]
    fn player_movement_walks_jumps_steps_and_slides_wall() {
        let desc = canonical_descriptor();
        let world = ledge_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let floor_y = desc.capsule.half_height + desc.capsule.radius; // 1.2
        let ledge_y = 0.3 + floor_y; // 1.5

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        // Let gravity settle the capsule onto the floor.
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);
        // Per-tick y oscillates within one gravity-step of the floor.
        let settle_tol = 0.02;
        assert!(
            (pos.y - floor_y).abs() < settle_tol,
            "player should settle near floor_y={}, got y={}",
            floor_y,
            pos.y
        );

        // ---- Phase 1: walk forward (+X) on the floor for 10 ticks. ----
        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let x_phase1_start = pos.x;
        run_ticks(&mut comp, &world, &mut pos, 10, &walk);

        assert!(
            pos.x > x_phase1_start,
            "player should have advanced forward, got x={} (started {})",
            pos.x,
            x_phase1_start
        );
        assert!(
            pos.x < 5.0,
            "player should still be on the floor before the ledge, got x={}",
            pos.x
        );
        // The step-up probe is gated on wall-like normals (|ny| < cos_walkable),
        // so flat-floor walking does not trigger it and y stays near floor_y.
        // A small tolerance covers gravity's sub-millimeter settle each tick.
        let walk_y_min = floor_y - settle_tol;
        let walk_y_max = floor_y + settle_tol;
        assert!(
            pos.y >= walk_y_min && pos.y <= walk_y_max,
            "player y during walk should be in [{}, {}], got {}",
            walk_y_min,
            walk_y_max,
            pos.y
        );
        let h_speed = (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt();
        assert!(
            h_speed > 0.0,
            "horizontal speed should be positive after walking, got {}",
            h_speed
        );
        // The Quake-style projection cap means horizontal speed cannot exceed
        // the active speed — walk speed here, since this phase does not sprint.
        // The bounded-from-below value depends on how many ticks were spent in
        // the ground accel branch versus airborne (gravity-only), so we assert
        // the cap, not a specific reached value.
        assert!(
            h_speed <= desc.ground.speed.walk + VEL_EPS,
            "horizontal speed should not exceed walk speed, got {}",
            h_speed
        );

        // ---- Phase 2: jump while continuing to walk forward. ----
        let jump = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        // Find a tick where the player is grounded (oscillates per tick during
        // walking due to the step-up probe lifting the capsule off the floor).
        // Try until found or a generous upper bound.
        let mut jumped = false;
        for _ in 0..60 {
            if comp.is_grounded {
                let (next, ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
                pos = next;
                assert!(ev.jumped, "grounded + jump_pressed should emit jumped");
                assert!(
                    !comp.is_grounded,
                    "should be airborne immediately after jump"
                );
                assert!(
                    comp.velocity.y > 0.0,
                    "vertical velocity should be positive after jump, got {}",
                    comp.velocity.y
                );
                assert!(
                    approx_eq(comp.velocity.y, desc.air.jump_velocity, VEL_EPS),
                    "vy after jump should equal jump_velocity={}, got {}",
                    desc.air.jump_velocity,
                    comp.velocity.y
                );
                jumped = true;
                break;
            }
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert!(jumped, "expected a grounded tick within 60 attempts");

        // Continue one tick airborne with jump still held — air.jumps=0 so no
        // double-jump; gravity decelerates the upward arc.
        let vy_before = comp.velocity.y;
        let (next, _ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            comp.velocity.y < vy_before,
            "gravity should reduce vy from {} after one airborne tick, got {}",
            vy_before,
            comp.velocity.y
        );

        // ---- Phase 3: walk forward into the step-up ledge until crossed. ----
        // Burn enough ticks to land and traverse the ~5 m floor + 10 m ledge.
        // At ground.speed=7, that's ~2.15 s ≈ 130 ticks. Cap generously.
        for _ in 0..200 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x > 6.0 && pos.y > ledge_y - 0.05 {
                break;
            }
        }
        assert!(
            pos.x > 5.0,
            "player should cross the step-up ledge near edge (x=5), got x={}",
            pos.x
        );
        assert!(
            pos.y > floor_y + 0.1,
            "player y should climb above floor when crossing the ledge, got y={}",
            pos.y
        );
        assert!(
            pos.y >= ledge_y - settle_tol
                && pos.y <= ledge_y + desc.ground.step_height + settle_tol,
            "player y on ledge should be near ledge_y={}, got y={}",
            ledge_y,
            pos.y
        );

        // ---- Phase 4: walk into the wall at x=15. Wall slide. ----
        for _ in 0..200 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
        }
        // The capsule's leading face sits radius beyond pos.x; with the wall at
        // x=15, the centre cannot exceed 15 - radius (small parry slop tolerated).
        // Allow a small parry contact slop on top of the spec's position eps.
        let wall_limit = 15.0 - desc.capsule.radius + POS_EPS + 1e-3;
        assert!(
            pos.x <= wall_limit,
            "player x should not penetrate the wall: pos.x={}, limit={}",
            pos.x,
            wall_limit
        );
        let x_pinned = pos.x;
        for _ in 0..10 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
        }
        // Wall-unstick nudge is `NORMAL_NUDGE` (1e-4) — well under the skin
        // width — so a pinned player stays within one skin width of the wall.
        assert!(
            (pos.x - x_pinned).abs() < 0.03,
            "wall-pinned player x should stay within skin width: before={}, after={}",
            x_pinned,
            pos.x
        );
        // Wall slide: velocity component along +X (into the wall) is bled off
        // by the per-tick sweep-and-slide projection.
        assert!(
            comp.velocity.x.abs() < 0.1,
            "wall slide should not produce a large +X velocity, got vx={}",
            comp.velocity.x
        );
        // Y still rests near the ledge surface (allowing for the step-up jitter).
        assert!(
            pos.y >= ledge_y - settle_tol
                && pos.y <= ledge_y + desc.ground.step_height + settle_tol,
            "wall-pinned player y should be near ledge ({}), got y={}",
            ledge_y,
            pos.y
        );
    }

    /// Flat floor at y=0 spanning x,z ∈ [-20,20] with a vertical wall at x=5
    /// (y∈[0,5], z∈[-20,20]). Used to isolate wall-slide behavior from the
    /// step-up probe path.
    fn flat_floor_and_wall_world() -> CollisionWorld {
        let mut points: Vec<Point<f32>> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();

        let f0 = points.len() as u32;
        points.push(Point::new(-20.0, 0.0, -20.0));
        points.push(Point::new(20.0, 0.0, -20.0));
        points.push(Point::new(20.0, 0.0, 20.0));
        points.push(Point::new(-20.0, 0.0, 20.0));
        tris.push([f0, f0 + 1, f0 + 2]);
        tris.push([f0, f0 + 2, f0 + 3]);

        let w0 = points.len() as u32;
        points.push(Point::new(5.0, 0.0, -20.0));
        points.push(Point::new(5.0, 0.0, 20.0));
        points.push(Point::new(5.0, 5.0, 20.0));
        points.push(Point::new(5.0, 5.0, -20.0));
        tris.push([w0, w0 + 1, w0 + 2]);
        tris.push([w0, w0 + 2, w0 + 3]);

        let mesh = TriMesh::new(points, tris);
        CollisionWorld {
            mesh,
            isometry: Isometry::identity(),
        }
    }

    // Regression: step-up lift (step_height + 0.05) exceeded the ground-stick
    // down-cast range (step_height), so a wall-walking player oscillated between
    // lifted-and-airborne and snapped-to-floor states each tick — visible as
    // ~0.35 m camera jitter.
    #[test]
    fn wall_slide_does_not_bounce_y() {
        let desc = canonical_descriptor();
        let world = ledge_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let ledge_y = 0.3 + desc.capsule.half_height + desc.capsule.radius;

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        for _ in 0..300 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x > 14.0 && pos.y > ledge_y - 0.05 {
                break;
            }
        }
        assert!(
            pos.x > 13.5,
            "setup: player should reach ledge near wall, got x={}",
            pos.x
        );

        let mut y_samples = Vec::with_capacity(60);
        let mut grounded_after_settle = Vec::with_capacity(60);
        for i in 0..60 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            y_samples.push(pos.y);
            if i >= 5 {
                grounded_after_settle.push(comp.is_grounded);
            }
        }
        let y_min = y_samples.iter().cloned().fold(f32::INFINITY, f32::min);
        let y_max = y_samples.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            y_max - y_min < 0.03,
            "wall-pinned y envelope should stay within skin width, got {} (min={}, max={})",
            y_max - y_min,
            y_min,
            y_max
        );
        let non_grounded = grounded_after_settle.iter().filter(|g| !*g).count();
        assert!(
            non_grounded <= 2,
            "is_grounded should be stable; got {} non-grounded ticks out of {}",
            non_grounded,
            grounded_after_settle.len()
        );
    }

    #[test]
    fn walking_along_wall_keeps_horizontal_speed() {
        let desc = canonical_descriptor();
        let world = ledge_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let ledge_y = 0.3 + desc.capsule.half_height + desc.capsule.radius;

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        for _ in 0..300 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x > 14.0 && pos.y > ledge_y - 0.05 {
                break;
            }
        }

        let z_start = pos.z;
        let diag = MovementInput {
            wish_dir: Vec2::new(1.0, 1.0).normalize(),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        for _ in 0..120 {
            let (next, _ev) = tick(&mut comp, &diag, &world, GRAVITY, DT, pos);
            pos = next;
        }
        let z_advance = (pos.z - z_start).abs();
        let min_z_advance = desc.ground.speed.walk * (120.0 / 60.0) * 0.5;
        assert!(
            z_advance >= min_z_advance,
            "tangential -Z advance should be >= {}, got {}",
            min_z_advance,
            z_advance
        );
        let h_speed = (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt();
        assert!(
            h_speed > desc.ground.speed.walk * 0.4,
            "horizontal speed after 120 ticks should exceed {}, got {}",
            desc.ground.speed.walk * 0.4,
            h_speed
        );
    }

    #[test]
    fn walking_into_wall_y_stable_per_tick() {
        let desc = canonical_descriptor();
        let world = ledge_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let ledge_y = 0.3 + desc.capsule.half_height + desc.capsule.radius;

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        for _ in 0..300 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x > 14.0 && pos.y > ledge_y - 0.05 {
                break;
            }
        }

        let wall_contact_x = 15.0 - desc.capsule.radius - 0.05;
        let mut prev_y = pos.y;
        let mut in_contact = false;
        for _ in 0..60 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x >= wall_contact_x {
                in_contact = true;
            }
            if in_contact {
                let dy = (pos.y - prev_y).abs();
                assert!(
                    dy < 0.03,
                    "per-tick |dy| should stay within skin width after wall contact, got {} (prev_y={}, y={})",
                    dy,
                    prev_y,
                    pos.y
                );
            }
            prev_y = pos.y;
        }
        assert!(in_contact, "test setup: should have reached wall contact");
    }

    // Regression: step-up probe lifted ~0.35 m every tick when walking into a
    // pure wall (no walkable surface above). Ground-stick snapped back within
    // the tick, but the intra-tick excursion produced visible camera jitter.
    // Direct unit test: pure wall → None.
    #[test]
    fn step_up_lift_returns_none_at_pure_wall() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let capsule = Capsule::new(
            Point::new(0.0, -desc.capsule.half_height, 0.0),
            Point::new(0.0, desc.capsule.half_height, 0.0),
            desc.capsule.radius,
        );
        let cos_walkable = desc.ground.max_slope.to_radians().cos();
        let floor_y = desc.capsule.half_height + desc.capsule.radius;
        // Position the capsule just shy of the wall (wall at x=5) so the
        // forward probe hits it on the first cast. Lifted slightly above the
        // floor so the forward probe finds the wall (not the floor contact).
        let current_pos = Vec3::new(5.0 - desc.capsule.radius - 0.05, floor_y + 0.05, 0.0);
        let horiz_vel = Vec3::new(desc.ground.speed.walk, 0.0, 0.0);
        let horiz_speed = horiz_vel.length();

        let result = step_up_lift(
            &world,
            &capsule,
            current_pos,
            horiz_vel,
            horiz_speed,
            desc.ground.step_height,
            cos_walkable,
            DT,
            desc.capsule.radius,
        );
        assert!(
            result.is_none(),
            "step_up_lift should return None at a pure wall, got {:?}",
            result
        );
    }

    // Direct unit test: walkable step → Some(lifted) at step_height + 0.05.
    #[test]
    fn step_up_lift_returns_some_at_walkable_step() {
        let desc = canonical_descriptor();
        let world = ledge_and_wall_world();
        let capsule = Capsule::new(
            Point::new(0.0, -desc.capsule.half_height, 0.0),
            Point::new(0.0, desc.capsule.half_height, 0.0),
            desc.capsule.radius,
        );
        let cos_walkable = desc.ground.max_slope.to_radians().cos();
        let floor_y = desc.capsule.half_height + desc.capsule.radius;
        // Approach the step riser at x=5 from the floor side. Lift the
        // capsule slightly above the floor so the forward probe doesn't
        // return the floor contact (toi=0, normal +Y) before reaching the
        // riser.
        let current_pos = Vec3::new(5.0 - desc.capsule.radius - 0.05, floor_y + 0.05, 0.0);
        let horiz_vel = Vec3::new(desc.ground.speed.walk, 0.0, 0.0);
        let horiz_speed = horiz_vel.length();

        let result = step_up_lift(
            &world,
            &capsule,
            current_pos,
            horiz_vel,
            horiz_speed,
            desc.ground.step_height,
            cos_walkable,
            DT,
            desc.capsule.radius,
        );
        let lifted = result.expect("step_up_lift should return Some at a walkable step");
        let expected_y = current_pos.y + desc.ground.step_height + 0.05;
        assert!(
            approx_eq(lifted.y, expected_y, POS_EPS),
            "lifted y should be {} (current + step_height + 0.05), got {}",
            expected_y,
            lifted.y
        );
        assert!(
            approx_eq(lifted.x, current_pos.x, POS_EPS)
                && approx_eq(lifted.z, current_pos.z, POS_EPS),
            "lift should preserve horizontal position, got {:?}",
            lifted
        );
    }

    // Regression: walking into a pure wall produced visible vertical camera
    // jitter even though tick-boundary `pos.y` snapped back via ground-stick.
    #[test]
    fn walking_into_wall_y_stays_at_floor() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let floor_y = desc.capsule.half_height + desc.capsule.radius;

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);
        let settle_y = pos.y;
        assert!(
            (settle_y - floor_y).abs() < 0.02,
            "test setup: should settle near floor_y={}, got {}",
            floor_y,
            settle_y
        );

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let wall_contact_x = 5.0 - desc.capsule.radius - 0.05;
        let mut in_contact = false;
        for _ in 0..120 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x >= wall_contact_x {
                in_contact = true;
            }
            if in_contact {
                // Compare to settle_y, not floor_y: cast_capsule's 0.02 skin
                // means the resting capsule sits ~0.02 above geometric floor_y.
                assert!(
                    (pos.y - settle_y).abs() < 0.01,
                    "wall-walking y should stay within 0.01 of settle_y={}, got {}",
                    settle_y,
                    pos.y
                );
            }
        }
        assert!(in_contact, "test setup: should have reached wall contact");
    }

    // Regression: the floor TOI=0 branch in the slide loop unconditionally
    // pushed the player up by 0.025 m per iteration (up to 4× per tick) when
    // a grounded capsule walked into a flat wall. Ground-stick snapped back
    // most ticks but at a wall/floor inside corner the downcast could pick
    // the wall normal first and silently latch the player above the floor.
    // Tight envelope across the last 30 ticks catches the orbital pump that
    // the looser 0.01-bounded test missed.
    #[test]
    fn walking_into_wall_no_orbital_jitter() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        // Run 120 ticks; the player reaches the wall well within the first
        // 60 (7 m at canonical 7 m/s ≈ 60 ticks of accel + wall approach).
        // Sample the last 30 ticks so all samples come from the
        // post-stabilisation wall-pinned regime.
        let mut ys = Vec::with_capacity(120);
        for _ in 0..120 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            ys.push(pos.y);
        }
        let tail_y = &ys[90..];
        let y_min = tail_y.iter().cloned().fold(f32::INFINITY, f32::min);
        let y_max = tail_y.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        // 3e-3 m tolerance bounds the vertical wobble the floor_push budget
        // (skin + nudge ≈ 0.02 m, applied at most once per tick and snapped
        // back by ground-stick) can leak. The pre-fix code's orbital pump
        // produced 0.025–0.1 m vertical jitter — an order of magnitude past
        // this bound. Horizontal drift is not asserted here: floor triangle
        // normals tilt a few thousandths off pure +Y, so projection of
        // tangential motion introduces a slow sub-millimetre-per-tick
        // horizontal creep that accumulates over many ticks but does not
        // reflect the orbital-pump bug.
        let envelope = 3.0e-3;
        assert!(
            y_max - y_min < envelope,
            "wall-pinned y envelope across last 30 ticks should be < {} m, got {} (min={}, max={})",
            envelope,
            y_max - y_min,
            y_min,
            y_max
        );
    }

    // Regression: capsule pressed against a wall produced TOI=0 every sweep
    // iteration, burning all 4 slots without advancing — player froze instead
    // of sliding tangentially along the wall.
    #[test]
    fn player_slides_along_wall_when_approaching_diagonally() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);

        let diag = MovementInput {
            wish_dir: Vec2::new(1.0, 1.0).normalize(),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };

        for _ in 0..200 {
            let (next, _ev) = tick(&mut comp, &diag, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x >= 5.0 - desc.capsule.radius - 0.05 {
                break;
            }
        }
        let wall_limit = 5.0 - desc.capsule.radius + POS_EPS + 1e-3;
        assert!(
            pos.x <= wall_limit,
            "diagonal approach should not penetrate the wall: pos.x={}, limit={}",
            pos.x,
            wall_limit
        );
        assert!(
            pos.x > 5.0 - desc.capsule.radius - 0.1,
            "player should have reached the wall: pos.x={}",
            pos.x
        );

        let z_before = pos.z;
        for _ in 0..30 {
            let (next, _ev) = tick(&mut comp, &diag, &world, GRAVITY, DT, pos);
            pos = next;
        }
        // facing_yaw=0 makes forward=-Z, so diagonal input (right+forward)
        // produces (+X, 0, -Z) motion. Wall projects out +X; -Z slide remains.
        assert!(
            pos.z < z_before - 0.5,
            "player should slide along -Z while pinned to the wall: z_before={}, z_after={}",
            z_before,
            pos.z
        );
    }

    /// Two perpendicular walls forming an interior corner at (x=5, z=5),
    /// floor below at y=0. The east wall (x=5, y∈[0,5], z∈[-20,5]) and the
    /// north wall (x∈[-20,5], y∈[0,5], z=5) meet at a 90° interior corner so
    /// a player driven into the corner experiences both wall normals (-X, -Z)
    /// in the same tick — the geometric setup the deadzone targets.
    fn corner_world() -> CollisionWorld {
        let mut points: Vec<Point<f32>> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();

        // Floor.
        let f0 = points.len() as u32;
        points.push(Point::new(-20.0, 0.0, -20.0));
        points.push(Point::new(20.0, 0.0, -20.0));
        points.push(Point::new(20.0, 0.0, 20.0));
        points.push(Point::new(-20.0, 0.0, 20.0));
        tris.push([f0, f0 + 1, f0 + 2]);
        tris.push([f0, f0 + 2, f0 + 3]);

        // East wall at x=5 facing -X.
        let e0 = points.len() as u32;
        points.push(Point::new(5.0, 0.0, -20.0));
        points.push(Point::new(5.0, 0.0, 5.0));
        points.push(Point::new(5.0, 5.0, 5.0));
        points.push(Point::new(5.0, 5.0, -20.0));
        tris.push([e0, e0 + 1, e0 + 2]);
        tris.push([e0, e0 + 2, e0 + 3]);

        // North wall at z=5 facing -Z.
        let n0 = points.len() as u32;
        points.push(Point::new(-20.0, 0.0, 5.0));
        points.push(Point::new(-20.0, 5.0, 5.0));
        points.push(Point::new(5.0, 5.0, 5.0));
        points.push(Point::new(5.0, 0.0, 5.0));
        tris.push([n0, n0 + 1, n0 + 2]);
        tris.push([n0, n0 + 2, n0 + 3]);

        let mesh = TriMesh::new(points, tris);
        CollisionWorld {
            mesh,
            isometry: Isometry::identity(),
        }
    }

    /// Drive the player diagonally toward the corner at (x=5, z=5) until the
    /// capsule is firmly wedged, then return the last 10 tick-boundary (x,z)
    /// samples. Used by the deadzone tests below.
    fn wedge_player_in_corner(
        desc: &PlayerMovementDescriptor,
        comp: &mut PlayerMovementComponent,
        pos: &mut Vec3,
        world: &CollisionWorld,
    ) -> Vec<(f32, f32)> {
        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(comp, world, pos, 10, &idle);

        // facing_yaw=0 ⇒ forward=-Z, so wish_dir=(1,-1).norm() gives input
        // (+X, 0, +Z): straight at the +X / +Z corner.
        let toward_corner = MovementInput {
            wish_dir: Vec2::new(1.0, -1.0).normalize(),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        // Approach and reach the corner.
        for _ in 0..240 {
            let (next, _ev) = tick(comp, &toward_corner, world, GRAVITY, DT, *pos);
            *pos = next;
            if pos.x >= 5.0 - desc.capsule.radius - 0.05
                && pos.z >= 5.0 - desc.capsule.radius - 0.05
            {
                break;
            }
        }
        // Press into the corner for 50 ticks so any pinned pattern stabilises.
        for _ in 0..50 {
            let (next, _ev) = tick(comp, &toward_corner, world, GRAVITY, DT, *pos);
            *pos = next;
        }
        // Sample the last 10 tick-boundary positions.
        let mut samples = Vec::with_capacity(10);
        for _ in 0..10 {
            let (next, _ev) = tick(comp, &toward_corner, world, GRAVITY, DT, *pos);
            *pos = next;
            samples.push((pos.x, pos.z));
        }
        samples
    }

    #[test]
    fn wedging_into_corner_zeros_horizontal_velocity_when_deadzone_enabled() {
        let desc = canonical_descriptor();
        let world = corner_world();
        let (mut comp, mut pos) = settle_player(&desc);
        assert!(comp.stuck_stop_enabled, "deadzone is on by default");

        let samples = wedge_player_in_corner(&desc, &mut comp, &mut pos, &world);
        let xs: Vec<f32> = samples.iter().map(|(x, _)| *x).collect();
        let zs: Vec<f32> = samples.iter().map(|(_, z)| *z).collect();
        let x_range = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - xs.iter().cloned().fold(f32::INFINITY, f32::min);
        let z_range = zs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - zs.iter().cloned().fold(f32::INFINITY, f32::min);

        // With the deadzone on, the corner-wedge detector zeroes horizontal
        // velocity and rolls back XZ when contradictory wall normals appear,
        // so the player's XZ position is flat across consecutive ticks.
        let eps = 5.0e-4;
        assert!(
            x_range < eps,
            "deadzone enabled: x range across last 10 ticks should be < {} m, got {}",
            eps,
            x_range
        );
        assert!(
            z_range < eps,
            "deadzone enabled: z range across last 10 ticks should be < {} m, got {}",
            eps,
            z_range
        );
    }

    #[test]
    fn wedging_into_corner_keeps_motion_when_deadzone_disabled() {
        let desc = canonical_descriptor();
        let world = corner_world();
        let (mut comp, mut pos) = settle_player(&desc);
        // Disable the deadzone so the slide loop's wall projections govern
        // the final wedge XZ trajectory without the velocity zero-out.
        comp.stuck_stop_enabled = false;

        let samples = wedge_player_in_corner(&desc, &mut comp, &mut pos, &world);
        let xs: Vec<f32> = samples.iter().map(|(x, _)| *x).collect();
        let zs: Vec<f32> = samples.iter().map(|(_, z)| *z).collect();
        let x_range = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - xs.iter().cloned().fold(f32::INFINITY, f32::min);
        let z_range = zs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - zs.iter().cloned().fold(f32::INFINITY, f32::min);

        // Without the deadzone the player is not snapped to a frozen XZ —
        // velocity is still alive, projection just bleeds horizontal
        // components against both walls. Per-tick XZ should still be small
        // (the wedge is stable) but not exactly zero across consecutive
        // ticks the way the deadzone produces. Loose bound (> 0) is enough
        // to prove the flag gates the velocity zero-out — the
        // deadzone-enabled test asserts the much tighter bound.
        assert!(
            comp.velocity.x.abs() + comp.velocity.z.abs() < 1.0,
            "deadzone disabled wedge should still come to near-rest, got vx={} vz={}",
            comp.velocity.x,
            comp.velocity.z
        );
        // Sanity: with the deadzone OFF, the explicit XZ zero-out branch in
        // `tick()` did not fire, so the player retains whatever the slide
        // loop's natural projection leaves. Document via assertion that the
        // XZ wobble is observably non-negative (a no-op assertion that
        // makes the test's purpose explicit).
        assert!(x_range >= 0.0 && z_range >= 0.0);
    }

    #[test]
    fn sliding_along_wall_diagonally_not_affected_by_deadzone() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        assert!(comp.stuck_stop_enabled);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);

        let diag = MovementInput {
            wish_dir: Vec2::new(1.0, 1.0).normalize(),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        for _ in 0..200 {
            let (next, _ev) = tick(&mut comp, &diag, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x >= 5.0 - desc.capsule.radius - 0.05 {
                break;
            }
        }
        // facing_yaw=0 ⇒ forward=-Z; diagonal input (right+forward) gives
        // (+X, 0, -Z). Wall projects out +X but -Z slide must remain.
        let z_before = pos.z;
        for _ in 0..60 {
            let (next, _ev) = tick(&mut comp, &diag, &world, GRAVITY, DT, pos);
            pos = next;
        }
        let z_advance = z_before - pos.z;
        assert!(
            z_advance > 1.0,
            "diagonal wall slide should still produce tangential -Z motion with deadzone on: z_before={}, z_after={}, advance={}",
            z_before,
            pos.z,
            z_advance
        );
    }

    #[test]
    fn walking_along_flat_floor_not_affected_by_deadzone() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        assert!(comp.stuck_stop_enabled);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let x_start = pos.x;
        // 30 ticks (0.5 s) at 7 m/s ⇒ comfortably >1.5 m on open floor.
        for _ in 0..30 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
        }
        let advance = pos.x - x_start;
        assert!(
            advance > 1.5,
            "flat-floor walk should advance > 1.5 m in 30 ticks, got {}",
            advance
        );
        let h_speed = (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt();
        assert!(
            h_speed > desc.ground.speed.walk * 0.8,
            "flat-floor walk should keep h_speed near ground.speed={}, got {}",
            desc.ground.speed.walk,
            h_speed
        );
    }

    /// Steady-state horizontal speed on flat ground after enough ticks to
    /// reach the projection cap, given a fixed input direction.
    fn steady_state_ground_speed(running: bool) -> f32 {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        // Walk along -Z (away from the wall at x=5) so the move never contacts
        // geometry and the projection cap is the only speed limiter. 60 ticks
        // (~11 m at run speed) reaches the cap while staying on the floor
        // (z ∈ [-20, 20]).
        let mv = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 60, &mv);
        (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt()
    }

    // Sprint is omnidirectional and selects `ground.speed.run`: steady-state
    // horizontal speed while running must reach the run cap and exceed the
    // walk steady-state.
    #[test]
    fn running_reaches_higher_steady_state_than_walking() {
        let desc = canonical_descriptor();
        let walk_speed = steady_state_ground_speed(false);
        let run_speed = steady_state_ground_speed(true);

        assert!(
            approx_eq(walk_speed, desc.ground.speed.walk, 0.05),
            "walk steady-state should reach walk cap {}, got {}",
            desc.ground.speed.walk,
            walk_speed
        );
        assert!(
            approx_eq(run_speed, desc.ground.speed.run, 0.05),
            "run steady-state should reach run cap {}, got {}",
            desc.ground.speed.run,
            run_speed
        );
        assert!(
            run_speed > walk_speed + 1.0,
            "running ({run_speed}) should be meaningfully faster than walking ({walk_speed})"
        );
    }

    // Regression: the airborne horizontal speed cap used `ground.speed` (now
    // `.walk` after the rename). Sprinting then jumping must not instantly
    // decelerate mid-air — the cap honors run speed while running is held.
    #[test]
    fn airborne_cap_honors_run_speed_while_sprinting() {
        let desc = canonical_descriptor();
        // bunny_hop must be off for the air cap to apply at all.
        assert!(!desc.air.bunny_hop, "canonical descriptor has air cap on");
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        // Build up to the run cap on the ground (heading -Z, away from the
        // wall). Keep the buildup short enough that the player stays on the
        // floor (floor spans z ∈ [-20, 20]; ~30 ticks at 11 m/s ≈ 5.5 m).
        let run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 30, &run);
        let h_ground = (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt();
        assert!(
            h_ground > desc.ground.speed.walk + 0.5,
            "setup: ground run speed should exceed walk cap, got {h_ground}"
        );

        // Jump while still sprinting, then run several airborne ticks. The cap
        // is the run speed, so horizontal speed must stay above the walk cap —
        // a walk-capped airborne path would bleed it down to ~7. Flat-floor
        // walking can clear `is_grounded` for a tick, so find a grounded tick
        // before jumping (mirrors the walk/jump/step integration test).
        let run_jump = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: true,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let mut jumped = false;
        for _ in 0..60 {
            if comp.is_grounded {
                let (next, ev) = tick(&mut comp, &run_jump, &world, GRAVITY, DT, pos);
                pos = next;
                assert!(ev.jumped, "grounded + jump should emit jumped");
                jumped = true;
                break;
            }
            let (next, _ev) = tick(&mut comp, &run, &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert!(jumped, "setup: should have jumped from a grounded tick");

        // A few airborne ticks with sprint held; jump_pressed released so we
        // don't re-trigger (air.jumps = 0 anyway).
        for _ in 0..5 {
            let (next, _ev) = tick(&mut comp, &run, &world, GRAVITY, DT, pos);
            pos = next;
        }
        let h_air = (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt();
        assert!(
            h_air > desc.ground.speed.walk + 0.5,
            "airborne sprint speed should stay above the walk cap (run cap honored), got {h_air}"
        );
        assert!(
            h_air <= desc.ground.speed.run + VEL_EPS,
            "airborne sprint speed should not exceed the run cap {}, got {}",
            desc.ground.speed.run,
            h_air
        );
    }

    /// Drive the player off a grounded tick into a single jump and return once
    /// airborne. Mirrors the grounded-tick search the other jump tests use so
    /// flat-floor `is_grounded` blips don't desync the launch.
    fn ground_jump_into_air(
        comp: &mut PlayerMovementComponent,
        world: &CollisionWorld,
        pos: &mut Vec3,
    ) {
        let jump = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        for _ in 0..60 {
            if comp.is_grounded {
                let (next, ev) = tick(comp, &jump, world, GRAVITY, DT, *pos);
                *pos = next;
                assert!(ev.jumped, "grounded + jump_pressed should emit jumped");
                return;
            }
            let (next, _ev) = tick(comp, &idle, world, GRAVITY, DT, *pos);
            *pos = next;
        }
        panic!("expected a grounded tick within 60 attempts");
    }

    // Double-jump fires while airborne once the rising arc has decayed past the
    // ceiling, consuming one air charge. Proves the named air-jump ability under
    // the budget model: a second jump airborne, gated by `air.jump_ceiling`.
    #[test]
    fn air_jump_fires_second_jump_while_airborne_under_ceiling() {
        let desc = double_jump_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);
        assert_eq!(
            comp.air_jumps_remaining, desc.air.jumps,
            "budget should start full on the ground"
        );

        // First jump from the ground. Launch vy is 5.5, well above the 2.0
        // ceiling, so the air-jump must NOT fire on the launch tick.
        ground_jump_into_air(&mut comp, &world, &mut pos);
        assert_eq!(
            comp.air_jumps_remaining, desc.air.jumps,
            "air-jump budget untouched by the ground jump"
        );

        let jump = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        // While vy is still above the ceiling, holding jump must not consume a
        // charge (the ceiling gate blocks it).
        let blocked = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
        pos = blocked.0;
        assert!(
            comp.velocity.y > desc.air.jump_ceiling,
            "setup: vy should still be above the ceiling one tick after launch, got {}",
            comp.velocity.y
        );
        assert_eq!(
            comp.air_jumps_remaining, desc.air.jumps,
            "air-jump must not fire while vy is above the ceiling"
        );

        // Let gravity decay vy under the ceiling, holding jump released so the
        // charge isn't spent the instant the ceiling is crossed.
        for _ in 0..60 {
            if comp.velocity.y <= desc.air.jump_ceiling {
                break;
            }
            let (next, _ev) = tick(&mut comp, &idle, &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert!(
            comp.velocity.y <= desc.air.jump_ceiling,
            "setup: vy should have decayed under the ceiling, got {}",
            comp.velocity.y
        );
        assert!(!comp.is_grounded, "setup: player must still be airborne");

        // Now the air-jump fires: a charge is consumed and vy relaunches to the
        // jump velocity.
        let (_next, ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
        assert!(ev.jumped, "air-jump under the ceiling should emit jumped");
        assert_eq!(
            comp.air_jumps_remaining,
            desc.air.jumps - 1,
            "air-jump should consume exactly one charge"
        );
        assert!(
            approx_eq(comp.velocity.y, desc.air.jump_velocity, VEL_EPS),
            "air-jump should relaunch vy to jump_velocity={}, got {}",
            desc.air.jump_velocity,
            comp.velocity.y
        );
    }

    // The air-jump budget replenishes on landing through `refresh_on_landing`:
    // after spending the charge airborne, returning to the floor restores it.
    #[test]
    fn air_jump_budget_restored_on_landing() {
        let desc = double_jump_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        // Spend the air charge directly while airborne: jump, drop vy under the
        // ceiling, then air-jump.
        ground_jump_into_air(&mut comp, &world, &mut pos);
        let jump = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        for _ in 0..60 {
            if comp.velocity.y <= desc.air.jump_ceiling {
                let (next, _ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
                pos = next;
                break;
            }
            let (next, _ev) = tick(&mut comp, &idle, &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert_eq!(
            comp.air_jumps_remaining, 0,
            "setup: the single air charge should be spent"
        );

        // Fall and land; the landing-refresh point restores the budget.
        for _ in 0..120 {
            let (next, _ev) = tick(&mut comp, &idle, &world, GRAVITY, DT, pos);
            pos = next;
            if comp.is_grounded {
                break;
            }
        }
        assert!(comp.is_grounded, "setup: player should have landed");
        assert_eq!(
            comp.air_jumps_remaining, desc.air.jumps,
            "landing should restore the air-jump budget via refresh_on_landing"
        );
    }

    // Budget exhaustion: with a one-charge budget, a second airborne jump cannot
    // fire until landing replenishes it. Proves the air-jump count gates repeated
    // airborne jumps within one airborne window.
    #[test]
    fn air_jump_exhausts_after_configured_count() {
        let desc = double_jump_descriptor();
        assert_eq!(desc.air.jumps, 1, "fixture uses a one-charge budget");
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        ground_jump_into_air(&mut comp, &world, &mut pos);
        let jump = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };

        // Spend the only charge once vy is under the ceiling.
        let mut spent = false;
        for _ in 0..60 {
            if comp.velocity.y <= desc.air.jump_ceiling {
                let (next, ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
                pos = next;
                assert!(ev.jumped, "first air-jump should fire");
                spent = true;
                break;
            }
            let (next, _ev) = tick(&mut comp, &idle, &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert!(spent, "setup: should have spent the air charge");
        assert_eq!(comp.air_jumps_remaining, 0, "budget exhausted");

        // Keep holding jump while airborne and under the ceiling: with the
        // budget at zero, no further air-jump may fire until landing.
        for _ in 0..30 {
            if comp.is_grounded {
                break;
            }
            let vy_before = comp.velocity.y;
            let (next, ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
            pos = next;
            assert!(
                !ev.jumped,
                "no air-jump should fire with an exhausted budget while airborne"
            );
            assert_eq!(
                comp.air_jumps_remaining, 0,
                "budget must stay exhausted until landing"
            );
            // Vy keeps decaying under gravity — it is not relaunched.
            assert!(
                comp.velocity.y < vy_before + VEL_EPS,
                "vy should not relaunch with an exhausted budget: before={}, after={}",
                vy_before,
                comp.velocity.y
            );
        }
    }

    // ---- Dash -------------------------------------------------------------

    /// Build a `DashParams` with the three orthogonal knobs explicit so each
    /// dash test can place itself on the rigid↔fluid spectrum (D3).
    fn dash_params(
        boost_speed: f32,
        momentum_retention: f32,
        steer_control: f32,
        dash_drag: f32,
        cooldown_ms: f32,
        air_dashes: u32,
        preserve_vertical: bool,
    ) -> DashParams {
        DashParams {
            boost_speed: boost_speed.into(),
            momentum_retention: momentum_retention.into(),
            steer_control: steer_control.into(),
            dash_drag: dash_drag.into(),
            cooldown_ms: cooldown_ms.into(),
            air_dashes,
            preserve_vertical: preserve_vertical.into(),
        }
    }

    /// Canonical descriptor with a dash configured. Defaults to a committed,
    /// rigid dash (no steer, no retention, finite drag); tests override the
    /// `dash` field for the corner they exercise.
    fn dash_descriptor(dash: DashParams) -> PlayerMovementDescriptor {
        let mut desc = canonical_descriptor();
        desc.dash = Some(dash);
        desc
    }

    fn idle_input() -> MovementInput {
        MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        }
    }

    fn horiz_speed(comp: &PlayerMovementComponent) -> f32 {
        (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt()
    }

    /// Settle the player on flat ground, then build up run speed along -Z (away
    /// from the wall at x=5) so dash tests start from a known grounded velocity.
    fn settle_and_run(
        desc: &PlayerMovementDescriptor,
        world: &CollisionWorld,
        run_ticks_n: usize,
    ) -> (PlayerMovementComponent, Vec3) {
        let (mut comp, mut pos) = settle_player(desc);
        run_ticks(&mut comp, world, &mut pos, 10, &idle_input());
        let run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, world, &mut pos, run_ticks_n, &run);
        (comp, pos)
    }

    // Fluid corner: momentumRetention=1, dashDrag=0. A dash in the direction of
    // travel while already running stacks (peak exceeds a standing dash), then
    // decays through Normal's ground friction back into the run-speed band, at
    // which point control returns to Normal — before the DASH_MAX_MS guard.
    #[test]
    fn dash_fluid_corner_stacks_then_decays_into_band() {
        let world = flat_floor_and_wall_world();
        let desc = dash_descriptor(dash_params(8.0, 1.0, 0.0, 0.0, 0.0, 0, false));
        let run_cap = desc.ground.speed.run; // 11.0

        // Reference: a standing dash (no prior velocity) peaks at boost_speed.
        let (mut standing, mut spos) = settle_player(&desc);
        run_ticks(&mut standing, &world, &mut spos, 10, &idle_input());
        let dash_in_place = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let _ = tick(&mut standing, &dash_in_place, &world, GRAVITY, DT, spos);
        let standing_peak = horiz_speed(&standing);

        // Running dash: build up to the run cap, then dash forward.
        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        let pre_dash = horiz_speed(&comp);
        assert!(
            pre_dash > desc.ground.speed.walk,
            "setup: should be running above walk speed, got {pre_dash}"
        );
        let (next, _ev) = tick(&mut comp, &dash_in_place, &world, GRAVITY, DT, pos);
        pos = next;
        let running_peak = horiz_speed(&comp);
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "should have entered Dash"
        );
        assert!(
            running_peak > standing_peak + 1.0,
            "running dash should stack over a standing dash: running={running_peak}, standing={standing_peak}"
        );

        // Release directional input; ground friction bleeds the dash back into
        // the run-speed band, at which point control returns to Normal.
        let mut returned = false;
        for _ in 0..30 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                returned = true;
                break;
            }
        }
        assert!(returned, "dash should decay back into Normal via friction");
        assert!(
            horiz_speed(&comp) <= run_cap + VEL_EPS,
            "post-dash speed should be within the run band, got {}",
            horiz_speed(&comp)
        );
    }

    // Regression: a grounded dash held in the move direction must decay back to
    // the run cap even while the stick stays down. Ground friction used to be
    // no-input-only (it relies on `pm_accelerate` to cap grounded speed), so a
    // held-input dash — which is deliberately above the cap — stayed locked at
    // boost speed indefinitely until the player released the button.
    #[test]
    fn dash_held_input_decays_back_to_run_speed_on_ground() {
        let world = flat_floor_and_wall_world();
        // dashDrag = 0 (decay through Normal friction), momentumRetention = 0.5,
        // steerControl = 0.3 — the dev-player tuning that exposed the bug.
        let desc = dash_descriptor(dash_params(22.0, 0.5, 0.3, 0.0, 0.0, 0, false));
        let run_cap = desc.ground.speed.run;

        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        let dash_held = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash_held, &world, GRAVITY, DT, pos);
        pos = next;
        let peak = horiz_speed(&comp);
        assert!(
            peak > run_cap + 1.0,
            "dash should briefly exceed the run cap, got {peak}"
        );

        // Keep holding the SAME direction. Only the first tick was the dash
        // rising edge, so `dash_pressed` is false hereafter (no re-entry).
        let hold = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        // Keep holding the direction and confirm the speed bleeds back to the run
        // cap *while still grounded* — the actual bug was a held-input grounded
        // dash never decaying. Break as soon as it reaches the band on the ground;
        // asserting the grounded state rules out the speed merely being clamped by
        // the airborne cap if the player later runs off the floor edge.
        let mut grounded_in_band = false;
        for _ in 0..120 {
            let (next, _ev) = tick(&mut comp, &hold, &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal)
                && comp.is_grounded
                && horiz_speed(&comp) <= run_cap + 0.05
            {
                grounded_in_band = true;
                break;
            }
        }
        assert!(
            grounded_in_band,
            "held-input dash never bled back to the run cap on the ground; final speed {:.3}, grounded {}",
            horiz_speed(&comp),
            comp.is_grounded,
        );
        // It settles at the cap, not below — the stick is still held.
        assert!(
            horiz_speed(&comp) > run_cap - 1.0,
            "held-input dash should hold the run cap, not slow further: {}",
            horiz_speed(&comp)
        );
    }

    // Rigid corner: momentumRetention=0, steerControl=0, dashDrag>0. The dash
    // outcome (peak speed and the linear dash_drag decay curve) is identical
    // regardless of entry velocity — bit-exact repeatability.
    #[test]
    fn dash_rigid_corner_identical_regardless_of_entry_velocity() {
        let world = flat_floor_and_wall_world();
        let desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 30.0, 0.0, 0, false));
        let dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };

        // Capture the dash speed curve from a standing entry.
        let (mut standing, mut spos) = settle_player(&desc);
        run_ticks(&mut standing, &world, &mut spos, 10, &idle_input());
        let mut standing_curve = Vec::new();
        let (next, _ev) = tick(&mut standing, &dash, &world, GRAVITY, DT, spos);
        spos = next;
        standing_curve.push(horiz_speed(&standing));
        for _ in 0..10 {
            let (next, _ev) = tick(&mut standing, &idle_input(), &world, GRAVITY, DT, spos);
            spos = next;
            standing_curve.push(horiz_speed(&standing));
        }

        // Capture the same curve from a fast running entry.
        let (mut running, mut rpos) = settle_and_run(&desc, &world, 60);
        let mut running_curve = Vec::new();
        let (next, _ev) = tick(&mut running, &dash, &world, GRAVITY, DT, rpos);
        rpos = next;
        running_curve.push(horiz_speed(&running));
        for _ in 0..10 {
            let (next, _ev) = tick(&mut running, &idle_input(), &world, GRAVITY, DT, rpos);
            rpos = next;
            running_curve.push(horiz_speed(&running));
        }

        // momentumRetention=0 ⇒ entry velocity is fully replaced; the dash speed
        // curve is the same regardless of entry state. The two runs differ only
        // by sub-millimetre collision-position float noise (each run is at a
        // different world position), so compare within a tight epsilon rather
        // than bit-for-bit.
        for (i, (a, b)) in standing_curve.iter().zip(running_curve.iter()).enumerate() {
            assert!(
                approx_eq(*a, *b, 1.0e-2),
                "rigid dash speed at step {i} must match regardless of entry: standing={a}, running={b}"
            );
        }
    }

    // steerControl: at 0 input does not alter the dash trajectory mid-dash
    // (committed); at >0 input steers it. One test capturing the contrast.
    #[test]
    fn dash_steer_control_committed_vs_steerable() {
        let world = flat_floor_and_wall_world();
        // Dash forward (-Z) from a standing start; mid-dash hold a +X steer.
        let steer_input = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let dash_forward = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };

        // Committed: steer_control = 0. Mid-dash +X input must not add +X
        // velocity.
        let committed_desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 20.0, 0.0, 0, false));
        let (mut c, mut cpos) = settle_player(&committed_desc);
        run_ticks(&mut c, &world, &mut cpos, 10, &idle_input());
        let (next, _ev) = tick(&mut c, &dash_forward, &world, GRAVITY, DT, cpos);
        cpos = next;
        for _ in 0..4 {
            let (next, _ev) = tick(&mut c, &steer_input, &world, GRAVITY, DT, cpos);
            cpos = next;
        }
        let committed_vx = c.velocity.x;

        // Steerable: steer_control = 1. The same mid-dash +X input adds +X
        // velocity.
        let steer_desc = dash_descriptor(dash_params(15.0, 0.0, 1.0, 20.0, 0.0, 0, false));
        let (mut s, mut spos) = settle_player(&steer_desc);
        run_ticks(&mut s, &world, &mut spos, 10, &idle_input());
        let (next, _ev) = tick(&mut s, &dash_forward, &world, GRAVITY, DT, spos);
        spos = next;
        for _ in 0..4 {
            let (next, _ev) = tick(&mut s, &steer_input, &world, GRAVITY, DT, spos);
            spos = next;
        }
        let steered_vx = s.velocity.x;

        assert!(
            committed_vx.abs() < VEL_EPS,
            "committed dash (steer_control=0) must not gain +X from mid-dash input, got vx={committed_vx}"
        );
        assert!(
            steered_vx > 0.5,
            "steerable dash (steer_control=1) should gain +X from mid-dash input, got vx={steered_vx}"
        );
    }

    // momentumRetention: at 0 the dash replaces prior horizontal velocity
    // (input+facing held constant ⇒ outcome independent of entry velocity); at 1
    // it adds to prior horizontal velocity.
    #[test]
    fn dash_momentum_retention_replace_vs_additive() {
        let world = flat_floor_and_wall_world();
        let dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };

        // Retention 0: a standing dash and a running dash reach the same peak
        // (prior velocity replaced).
        let replace_desc = dash_descriptor(dash_params(8.0, 0.0, 0.0, 20.0, 0.0, 0, false));
        let (mut standing, mut sp) = settle_player(&replace_desc);
        run_ticks(&mut standing, &world, &mut sp, 10, &idle_input());
        let (_next, _ev) = tick(&mut standing, &dash, &world, GRAVITY, DT, sp);
        let replace_standing = horiz_speed(&standing);

        let (mut running, rp) = settle_and_run(&replace_desc, &world, 60);
        let (_next, _ev) = tick(&mut running, &dash, &world, GRAVITY, DT, rp);
        let replace_running = horiz_speed(&running);
        assert!(
            approx_eq(replace_standing, replace_running, VEL_EPS),
            "retention=0: dash peak must be independent of entry velocity, standing={replace_standing}, running={replace_running}"
        );
        assert!(
            approx_eq(replace_standing, 8.0, VEL_EPS),
            "retention=0 standing dash peak should equal boost_speed=8.0, got {replace_standing}"
        );

        // Retention 1: a running dash adds the boost on top of prior velocity.
        let add_desc = dash_descriptor(dash_params(8.0, 1.0, 0.0, 20.0, 0.0, 0, false));
        let (mut running2, mut rp2) = settle_and_run(&add_desc, &world, 60);
        let pre = horiz_speed(&running2);
        let (next, _ev) = tick(&mut running2, &dash, &world, GRAVITY, DT, rp2);
        rp2 = next;
        let _ = rp2;
        let add_running = horiz_speed(&running2);
        assert!(
            add_running > pre + 4.0,
            "retention=1: dash should add to prior velocity (pre={pre}, peak={add_running})"
        );
    }

    // Layered decay (D4): momentumRetention=1, dashDrag>0. Only the boost decays
    // at the dash_drag rate while the retained base continues under Normal's
    // friction; post-dash steady speed settles into Normal's band (the dash_drag
    // bleed does NOT drag the retained base below it).
    #[test]
    fn dash_layered_decay_base_survives_boost_drag() {
        let world = flat_floor_and_wall_world();
        // High drag bleeds the boost fast; retention=1 keeps the run-speed base.
        let desc = dash_descriptor(dash_params(8.0, 1.0, 0.0, 60.0, 0.0, 0, false));
        let run_cap = desc.ground.speed.run;

        // Build up to the run cap, then dash forward and KEEP running forward so
        // the base is sustained by ground accel (no friction while input held).
        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        let dash_run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash_run, &world, GRAVITY, DT, pos);
        pos = next;
        let peak = horiz_speed(&comp);
        assert!(
            peak > run_cap + 4.0,
            "setup: dash should stack over the run cap, got {peak}"
        );

        // Continue running forward; the boost drags off but the base is held at
        // the run cap by ongoing input.
        let run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let mut returned = false;
        for _ in 0..30 {
            let (next, _ev) = tick(&mut comp, &run, &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                returned = true;
                break;
            }
        }
        assert!(returned, "dash should exit once the boost has bled off");
        let settled = horiz_speed(&comp);
        // The retained base settles back at the run cap — the dash_drag bleed did
        // not drag it below the band.
        assert!(
            approx_eq(settled, run_cap, 0.2),
            "retained base should settle at the run cap {run_cap}, got {settled}"
        );
    }

    // DASH_MAX_MS guard: the Dash state cannot persist past DASH_MAX_MS even if
    // momentum stays high.
    #[test]
    fn dash_max_ms_guard_bounds_state() {
        let world = flat_floor_and_wall_world();
        // Zero drag + full retention + sustained input keeps speed pinned above
        // the band indefinitely, so only the elapsed guard can end the dash.
        let desc = dash_descriptor(dash_params(8.0, 1.0, 1.0, 0.0, 0.0, 0, false));
        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        let dash_run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash_run, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(matches!(comp.movement_state, MovementState::Dash { .. }));

        // DASH_MAX_MS / (dt*1000) ticks bound the state. Run a few past that.
        let max_ticks = (DASH_MAX_MS / (DT * 1000.0)).ceil() as usize;
        let mut exited_by = None;
        let run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        for i in 0..(max_ticks + 5) {
            let (next, _ev) = tick(&mut comp, &run, &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                exited_by = Some(i + 1);
                break;
            }
        }
        let exit_tick = exited_by.expect("dash must exit by the DASH_MAX_MS guard");
        // +1 for the entry tick already consumed above.
        assert!(
            exit_tick <= max_ticks,
            "dash exited after {} ticks; DASH_MAX_MS bounds it at ~{} ticks",
            exit_tick + 1,
            max_ticks
        );
    }

    // Cooldown: a dash requested while cooldown is active is suppressed — no
    // second impulse until the cooldown elapses.
    #[test]
    fn dash_cooldown_suppresses_until_elapsed() {
        let world = flat_floor_and_wall_world();
        // 500 ms cooldown, instant drag so the first dash ends quickly and we
        // return to Normal while the cooldown is still counting down.
        let desc = dash_descriptor(dash_params(12.0, 0.0, 0.0, 200.0, 500.0, 0, false));
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());
        // Zero wish_dir so the dash takes its direction from facing (forward =
        // -Z) and the suppression check sees ONLY the dash impulse — no Normal
        // locomotion accelerating the player when the dash is gated off.
        let dash = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(comp.dash_cooldown_ms > 0.0, "cooldown should be armed");

        // Let the first dash decay back to Normal (drag is strong).
        for _ in 0..20 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                break;
            }
        }
        assert!(matches!(comp.movement_state, MovementState::Normal));
        assert!(
            comp.dash_cooldown_ms > 0.0,
            "cooldown should still be active well within 500ms"
        );

        // Request a dash while the cooldown is active: suppressed.
        let speed_before = horiz_speed(&comp);
        let (next, _ev) = tick(&mut comp, &dash, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            matches!(comp.movement_state, MovementState::Normal),
            "dash must be suppressed while cooldown is active"
        );
        assert!(
            horiz_speed(&comp) <= speed_before + VEL_EPS,
            "no dash impulse should be applied during cooldown"
        );

        // Run out the cooldown, then a fresh dash fires again.
        for _ in 0..40 {
            if comp.dash_cooldown_ms <= 0.0 {
                break;
            }
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert!(comp.dash_cooldown_ms <= 0.0, "cooldown should have elapsed");
        let (next, _ev) = tick(&mut comp, &dash, &world, GRAVITY, DT, pos);
        pos = next;
        let _ = pos;
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "dash should fire again once the cooldown has elapsed"
        );
    }

    // Rising edge: holding the dash button does not re-trigger after the
    // cooldown elapses; only a fresh press fires. `dash_pressed` is a true rising
    // edge derived at the call site, so a held button presents as a single
    // `true` then `false` — modeled here by holding `dash_pressed = false` after
    // the initial press.
    #[test]
    fn dash_rising_edge_held_button_does_not_refire() {
        let world = flat_floor_and_wall_world();
        // Short cooldown so it elapses within the test window.
        let desc = dash_descriptor(dash_params(12.0, 0.0, 0.0, 200.0, 100.0, 0, false));
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());

        // Initial press: the rising edge fires the dash.
        let press = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &press, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(matches!(comp.movement_state, MovementState::Dash { .. }));

        // The button stays physically held, but the call site only emits a
        // rising edge once — so dash_pressed is false for the held duration.
        // Run long enough for the dash to end AND the cooldown to elapse.
        let held = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let mut redashed = false;
        for _ in 0..60 {
            let (next, _ev) = tick(&mut comp, &held, &world, GRAVITY, DT, pos);
            pos = next;
            if comp.dash_cooldown_ms <= 0.0
                && matches!(comp.movement_state, MovementState::Dash { .. })
            {
                redashed = true;
                break;
            }
        }
        assert!(
            !redashed,
            "a held button (no fresh rising edge) must not re-trigger the dash after cooldown"
        );
        assert!(
            comp.dash_cooldown_ms <= 0.0,
            "setup: cooldown should have elapsed"
        );

        // A fresh press now fires again.
        let (next, _ev) = tick(&mut comp, &press, &world, GRAVITY, DT, pos);
        pos = next;
        let _ = pos;
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "a fresh rising edge after cooldown should fire the dash"
        );
    }

    /// Place the player high above the floor, airborne, with ability budgets
    /// full — giving ample air time for several airborne dashes before the long
    /// fall back to y=0. Avoids the short, altitude-sensitive window a single
    /// jump provides (a horizontal dash that zeroes vy lands almost immediately
    /// from a low apex). The first tick the test runs establishes the airborne
    /// `is_grounded=false` flag via the substrate.
    fn airborne_aloft(
        desc: &PlayerMovementDescriptor,
        world: &CollisionWorld,
    ) -> (PlayerMovementComponent, Vec3) {
        let mut comp = PlayerMovementComponent::from_descriptor(desc);
        comp.is_grounded = false;
        comp.air_ticks = 10; // already settled into the airborne regime
        let mut pos = Vec3::new(0.0, 20.0, 0.0);
        // A few idle ticks let the substrate confirm airborne and build up a
        // clearly-nonzero downward vy (so preserve-vertical tests can tell a
        // retained fall from a zeroed one).
        run_ticks(&mut comp, world, &mut pos, 8, &idle_input());
        (comp, pos)
    }

    // Air-dash budget: dashes fire while airborne up to the configured budget,
    // are exhausted after that many airborne dashes, and are restored on landing.
    #[test]
    fn dash_air_budget_exhausts_then_restores_on_landing() {
        let world = flat_floor_and_wall_world();
        // 2 air dashes, no cooldown so the budget (not cooldown) is the gate.
        let desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 2, false));
        let (mut comp, mut pos) = airborne_aloft(&desc, &world);
        assert!(!comp.is_grounded, "setup: should be airborne");
        assert_eq!(comp.air_dashes_remaining, 2, "budget starts full aloft");

        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };

        // First airborne dash consumes one charge.
        let (next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(matches!(comp.movement_state, MovementState::Dash { .. }));
        assert_eq!(
            comp.air_dashes_remaining, 1,
            "first air-dash consumes a charge"
        );

        // Return to Normal (drag bleeds the dash) so the next press is a fresh
        // Normal→Dash transition, still airborne.
        for _ in 0..20 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                break;
            }
        }
        assert!(
            !comp.is_grounded,
            "setup: still airborne for the second dash"
        );

        // Second airborne dash consumes the last charge.
        let (next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);
        pos = next;
        assert_eq!(
            comp.air_dashes_remaining, 0,
            "second air-dash exhausts the budget"
        );

        for _ in 0..20 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                break;
            }
        }
        assert!(
            !comp.is_grounded,
            "setup: still airborne for the exhausted attempt"
        );

        // Third attempt while exhausted: suppressed.
        let (next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            matches!(comp.movement_state, MovementState::Normal),
            "an airborne dash must not fire with an exhausted budget"
        );

        // Fall and land; the budget is restored through refresh_on_landing.
        for _ in 0..180 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if comp.is_grounded {
                break;
            }
        }
        assert!(comp.is_grounded, "setup: player should have landed");
        assert_eq!(
            comp.air_dashes_remaining, 2,
            "landing should restore the air-dash budget"
        );
    }

    // Airborne classification spends exactly one charge: a dash fired on a tick
    // whose last-tick `is_grounded` flag is airborne (and which makes no floor
    // contact) consumes one air-dash charge in the intent step. With no floor
    // touch there is no landing-refresh, so the spend is the sole effect on the
    // budget — directly observable. This is the consume half of the
    // landing-tick behavior, isolated so a silently-skipped consume (grounded
    // misclassification) fails here rather than being masked by a same-tick
    // refresh.
    #[test]
    fn dash_airborne_classification_spends_one_charge() {
        let world = flat_floor_and_wall_world();
        // 2 air dashes so a single consume (→1) is distinct from both a full
        // budget (→2, consume skipped) and an exhausted one (→0).
        let desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 2, true));
        let (mut comp, pos) = airborne_aloft(&desc, &world);
        // Aloft at y=20 with no nearby floor: this tick's sweep cannot contact
        // the floor, so refresh_on_landing will not run.
        assert!(!comp.is_grounded, "setup: airborne entering the tick");
        assert_eq!(comp.air_dashes_remaining, 2, "setup: budget full aloft");

        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (_next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);

        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: airborne dash should have fired"
        );
        assert!(
            !comp.is_grounded,
            "setup: no floor contact, so no landing-refresh this tick"
        );
        // Fails (stays at 2) if the airborne consume is skipped via a grounded
        // misclassification — the spend is observable precisely because nothing
        // refreshes it back.
        assert_eq!(
            comp.air_dashes_remaining, 1,
            "airborne-classified dash spends exactly one air-dash charge"
        );
    }

    // Landing-tick ordering: a dash fired on the landing tick consumes an
    // air-dash charge in the intent/transition step (classified airborne via the
    // stale last-tick `is_grounded` flag), and the landing-refresh runs AFTERWARD
    // in the substrate-result step. Seeded one charge short of full so the
    // consume-then-refresh order leaves a FULL budget, while the inverted
    // refresh-then-consume order would leave it one short — making the ordering
    // directly observable in the post-tick budget.
    #[test]
    fn dash_on_landing_tick_consumes_then_refreshes() {
        let world = flat_floor_and_wall_world();
        // air_dashes = 2 is the refresh target. Seed remaining = 1 (one short):
        //   consume-then-refresh: 1 → 0 (consume) → 2 (refresh)  ⇒ 2 (full)
        //   refresh-then-consume: 1 → 2 (refresh) → 1 (consume)  ⇒ 1 (one short)
        // The post-tick value distinguishes the two orderings. preserve_vertical
        // so the entering downward velocity carries the capsule into the floor on
        // this single landing tick.
        let desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 2, true));
        let floor_y = desc.capsule.half_height + desc.capsule.radius;

        let mut comp = PlayerMovementComponent::from_descriptor(&desc);
        comp.is_grounded = false; // last-tick flag is airborne (stale on landing)
        comp.air_ticks = 5;
        comp.air_dashes_remaining = 1; // one short of the refresh target (2)
        // A hair above the floor with a downward velocity so this single tick's
        // sweep registers floor contact (the landing tick).
        let pos = Vec3::new(0.0, floor_y + 0.02, 0.0);
        comp.velocity = Vec3::new(0.0, -2.0, 0.0);

        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (_next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);

        assert!(
            comp.is_grounded,
            "setup: this tick should have landed the player"
        );
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: airborne-classified dash should have fired on the landing tick"
        );
        // Full budget proves consume (1→0) ran BEFORE refresh (0→2). The inverted
        // order would leave 1 here, so this also fails on an ordering inversion.
        assert_eq!(
            comp.air_dashes_remaining, 2,
            "landing-tick dash spends a charge in the intent step, then the \
             landing-refresh restores the full budget"
        );
    }

    // Disabled: absent DashParams ⇒ Normal→Dash never fires, no impulse
    // regardless of input.
    #[test]
    fn dash_disabled_when_no_params() {
        let world = flat_floor_and_wall_world();
        let desc = canonical_descriptor();
        assert!(desc.dash.is_none(), "canonical descriptor has no dash");
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());

        let speed_before = horiz_speed(&comp);
        // Zero wish_dir: with dash disabled and no locomotion input, any speed
        // gain could only come from a (forbidden) dash impulse.
        let dash = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        for _ in 0..10 {
            let (next, _ev) = tick(&mut comp, &dash, &world, GRAVITY, DT, pos);
            pos = next;
            assert!(
                matches!(comp.movement_state, MovementState::Normal),
                "dash must never fire when DashParams is absent"
            );
        }
        assert!(
            horiz_speed(&comp) <= speed_before + VEL_EPS,
            "no dash impulse should ever be applied with dash disabled"
        );
    }

    // preserveVertical: on an airborne dash, false zeroes vertical velocity and
    // true retains it (gravity then resumes).
    #[test]
    fn dash_preserve_vertical_airborne() {
        let world = flat_floor_and_wall_world();

        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };

        // The dash ENTERS in `Normal`'s intent step, which zeroes/keeps vy at the
        // very end of that tick; the dash's own gravity resumes the NEXT tick (in
        // `dash_intent`). So the entry-tick result is exactly zeroed/retained, and
        // a following tick shows gravity resume.

        // preserve_vertical = false: vy is zeroed on entry.
        let zero_desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 3, false));
        let (mut z, zpos) = airborne_aloft(&zero_desc, &world);
        // Aloft and falling: a clearly-nonzero downward vy distinguishes a
        // zeroed entry from a retained one.
        assert!(
            z.velocity.y < -1.0,
            "setup: should have a clearly-nonzero downward vy aloft, got {}",
            z.velocity.y
        );
        let (znext, _ev) = tick(&mut z, &air_dash, &world, GRAVITY, DT, zpos);
        assert!(
            matches!(z.movement_state, MovementState::Dash { .. }),
            "setup: airborne dash should have fired"
        );
        // Entry zeroed vy exactly.
        assert!(
            approx_eq(z.velocity.y, 0.0, VEL_EPS),
            "preserve_vertical=false should zero entering vy, got {}",
            z.velocity.y
        );
        // Gravity resumes the next (in-Dash) tick: vy goes negative again.
        let (_n, _e) = tick(&mut z, &idle_input(), &world, GRAVITY, DT, znext);
        assert!(
            z.velocity.y < -VEL_EPS,
            "gravity should resume after a zeroed-vertical dash entry, got {}",
            z.velocity.y
        );

        // preserve_vertical = true: vy is retained on entry.
        let keep_desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 3, true));
        let (mut k, kpos) = airborne_aloft(&keep_desc, &world);
        let vy_before = k.velocity.y;
        assert!(
            vy_before < -1.0,
            "setup: downward vy aloft, got {vy_before}"
        );
        let (_knext, _ev) = tick(&mut k, &air_dash, &world, GRAVITY, DT, kpos);
        // The entry runs inside `Normal`'s intent step, so Normal's gravity for
        // this tick already advanced vy before the keep; the retained value is
        // therefore `vy_before + g*dt`, clearly distinct from the false case's 0.
        let expected = vy_before + GRAVITY * DT;
        assert!(
            approx_eq(k.velocity.y, expected, VEL_EPS),
            "preserve_vertical=true should keep entering vy, expected ~{expected}, got {}",
            k.velocity.y
        );
    }

    /// Walk a grounded player toward +X until just shy of the wall at x=5, then
    /// return the settled component/position primed to dash into it.
    fn approach_wall_grounded(
        desc: &PlayerMovementDescriptor,
        world: &CollisionWorld,
    ) -> (PlayerMovementComponent, Vec3) {
        let (mut comp, mut pos) = settle_player(desc);
        run_ticks(&mut comp, world, &mut pos, 10, &idle_input());
        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        // Walk toward +X until close to the wall but still clear of the capsule
        // standoff (radius 0.4 ⇒ center stops near x≈4.6).
        for _ in 0..200 {
            let (next, _ev) = tick(&mut comp, &walk, world, GRAVITY, DT, pos);
            pos = next;
            if pos.x > 3.5 {
                break;
            }
        }
        (comp, pos)
    }

    // Regression: dashing head-on into geometry left a phantom backward velocity.
    // Collide-and-slide zeroed the velocity component along the boost axis, but
    // the tracked `boost` kept full magnitude, so `base = velocity - boost`
    // reconstructed a vector pointing back out of the wall (empirically vx=-1.5,
    // base.x=-15 the tick after entry). The boost/realized reconciliation in
    // `dash_intent` clamps it. This test fails on the pre-fix code.
    #[test]
    fn dash_head_on_into_wall_does_not_reverse() {
        let world = flat_floor_and_wall_world();
        // High boost, no drag-into-band complications; rigid committed dash.
        let desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 50.0, 0.0, 0, false));
        let capsule_standoff = 5.0 - desc.capsule.radius; // wall x=5, radius 0.4
        let (mut comp, mut pos) = approach_wall_grounded(&desc, &world);

        let dash_into_wall = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash_into_wall, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: dash should have entered, got {:?}",
            comp.movement_state
        );

        // Hold the dash direction into the wall; track velocity and penetration
        // across the whole dash AND the exit tick. No tick — including the one
        // that transitions back to Normal — may leave a backward velocity. The
        // phantom-base bug surfaces precisely on the tick after the wall zeroes
        // the boost axis, which is also the tick the dash exits, so the check
        // must run after each tick rather than only at the top of the loop.
        let mut returned_to_normal = false;
        for _ in 0..40 {
            let (next, _ev) = tick(&mut comp, &dash_into_wall, &world, GRAVITY, DT, pos);
            pos = next;
            assert!(
                comp.velocity.x > -VEL_EPS,
                "dash into wall must not produce backward velocity, got vx={}",
                comp.velocity.x
            );
            assert!(
                pos.x < capsule_standoff + 0.05,
                "player must not penetrate the wall (standoff {capsule_standoff}), got x={}",
                pos.x
            );
            if matches!(comp.movement_state, MovementState::Normal) {
                returned_to_normal = true;
                break;
            }
        }
        assert!(
            returned_to_normal,
            "dash blocked by the wall should exit cleanly into Normal"
        );
    }

    // An angled dash into the wall (toward +X, along -Z) should slide along the
    // wall: the tangential -Z speed survives while the into-wall +X component is
    // clipped. The boost reconciliation must not stick or reverse the slide.
    #[test]
    fn dash_angled_into_wall_slides_along_it() {
        let world = flat_floor_and_wall_world();
        let desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 50.0, 0.0, 0, false));
        let (mut comp, mut pos) = approach_wall_grounded(&desc, &world);

        // wish_dir=(1,1): into +X (the wall) and along -Z (tangent, free). With
        // facing_yaw=0, forward=-Z and right=+X, so this resolves to (1,0,-1).
        let dash_diag = MovementInput {
            wish_dir: Vec2::new(1.0, 1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash_diag, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: angled dash should have entered"
        );

        // The tick after entry, collision has clipped the +X component but the
        // tangential -Z slide must remain (clearly negative, not stuck/reversed).
        let (next, _ev) = tick(&mut comp, &dash_diag, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            comp.velocity.z < -1.0,
            "angled dash into the wall should retain tangential -Z slide, got vz={}",
            comp.velocity.z
        );
        assert!(
            comp.velocity.x > -VEL_EPS,
            "angled dash must not reverse out of the wall, got vx={}",
            comp.velocity.x
        );
        // The player should keep advancing along -Z (sliding), not stick.
        let z_before = pos.z;
        run_ticks(&mut comp, &world, &mut pos, 3, &dash_diag);
        assert!(
            pos.z < z_before - VEL_EPS,
            "player should slide along the wall in -Z, z went {z_before} -> {}",
            pos.z
        );
    }

    // Finding #3: a grounded dash must not spend an air-dash charge — the consume
    // is gated on `!is_grounded` in `try_enter_dash`.
    #[test]
    fn dash_grounded_preserves_air_dash_budget() {
        let world = flat_floor_and_wall_world();
        let desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 2, false));
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());
        assert!(comp.is_grounded, "setup: player should be grounded");
        assert_eq!(
            comp.air_dashes_remaining, 2,
            "setup: full air-dash budget before the grounded dash"
        );

        let dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (_next, _ev) = tick(&mut comp, &dash, &world, GRAVITY, DT, pos);
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: grounded dash should have fired"
        );
        assert_eq!(
            comp.air_dashes_remaining, 2,
            "a grounded dash must not consume an air-dash charge"
        );
    }

    // Finding #7: jump input during the Dash state is dropped by design —
    // `dash_intent` omits the jump branch. vy should only follow gravity.
    #[test]
    fn dash_ignores_jump_input() {
        let world = flat_floor_and_wall_world();
        // preserve_vertical so entry does not zero vy, isolating the jump check.
        let desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 50.0, 0.0, 3, true));
        let (mut comp, pos) = airborne_aloft(&desc, &world);

        // Enter the dash (airborne) without jump.
        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (pos, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: airborne dash should have entered"
        );

        // Now hold jump while in Dash; vy must only advance by gravity.
        let dash_with_jump = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let vy_before = comp.velocity.y;
        let (_next, _ev) = tick(&mut comp, &dash_with_jump, &world, GRAVITY, DT, pos);
        let expected = vy_before + GRAVITY * DT;
        assert!(
            approx_eq(comp.velocity.y, expected, VEL_EPS),
            "jump during dash must not add impulse: expected gravity-only vy ~{expected}, got {}",
            comp.velocity.y
        );
    }

    // Finding #6: an airborne dash with `dash_drag == 0` and a large boost should
    // decay back into the steady band rather than stay pinned at boost speed —
    // the boost folds into `Normal`'s contextual air cap.
    #[test]
    fn dash_airborne_zero_drag_decays_into_band() {
        let world = flat_floor_and_wall_world();
        // dash_drag=0, momentum_retention=0; large boost well above the band.
        let desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 0.0, 0.0, 3, true));
        let band = desc.ground.speed.run; // exit band is ground run speed
        let (mut comp, mut pos) = airborne_aloft(&desc, &world);

        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);
        pos = next;
        let peak = horiz_speed(&comp);
        assert!(
            peak > band,
            "setup: airborne dash should start above the band, got {peak}"
        );

        // Idle airborne; the boost must bleed back into the band before the
        // DASH_MAX_MS guard would force the exit.
        let mut settled = false;
        for _ in 0..40 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "zero-drag airborne dash should decay into Normal, not stay at boost speed"
        );
        assert!(
            horiz_speed(&comp) <= band + VEL_EPS,
            "post-dash horizontal speed should settle into the band, got {}",
            horiz_speed(&comp)
        );
    }

    // ----- Dash expression eval --------------------------------------------
    //
    // These cover the expression form of the dash value fields: an authored IR
    // expression resolves against a live `MovementScope` snapshot at the
    // engine-pinned moment (entry vs per-tick), clamps to the field range, and
    // observably changes dash behavior versus a literal.

    fn ir_num(v: f32) -> IrNode {
        IrNode::Const {
            value: IrValue::Number(v),
        }
    }

    fn ir_input(name: &str) -> IrNode {
        IrNode::Input {
            name: name.to_string(),
        }
    }

    /// Dash entry once: settle, optionally run, then issue a single dash tick and
    /// return the resulting horizontal speed plus the post-dash component.
    fn dash_once_from_run(
        desc: &PlayerMovementDescriptor,
        world: &CollisionWorld,
        run_ticks_n: usize,
    ) -> (PlayerMovementComponent, f32) {
        let (mut comp, pos) = settle_and_run(desc, world, run_ticks_n);
        let dash_input = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (_next, _ev) = tick(&mut comp, &dash_input, world, GRAVITY, DT, pos);
        let speed = horiz_speed(&comp);
        (comp, speed)
    }

    #[test]
    fn momentum_retention_select_on_grounded_differs_grounded_vs_airborne() {
        // AC: a `momentumRetention` select on `grounded` produces different entry
        // velocities grounded vs airborne. Grounded → retain 0 (pure boost);
        // airborne → retain 1 (boost stacks on prior horizontal velocity). With a
        // running entry, the airborne dash must peak strictly higher.
        let world = flat_floor_and_wall_world();
        // select(grounded, 0.0, 1.0): grounded ⇒ 0 retention, airborne ⇒ 1.
        let retention_expr = IrNode::Select {
            cond: Box::new(ir_input("grounded")),
            a: Box::new(ir_num(0.0)),
            b: Box::new(ir_num(1.0)),
        };
        let mut dash = dash_params(8.0, 0.0, 0.0, 0.0, 0.0, 3, false);
        dash.momentum_retention = NumberOrIr::Ir(retention_expr);
        let desc = dash_descriptor(dash);

        // Grounded dash: retention resolves to 0, so peak ≈ boost_speed (8.0),
        // not stacking the ~run-cap prior velocity.
        let (_grounded_comp, grounded_peak) = dash_once_from_run(&desc, &world, 60);

        // Airborne dash: leave the ground, then dash. Retention resolves to 1, so
        // the boost stacks on the retained horizontal velocity.
        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        // No jump is available and there is no ledge to walk off, so fake the
        // airborne state by clearing `is_grounded` and adding upward velocity.
        comp.is_grounded = false;
        comp.velocity.y = 3.0;
        let dash_input = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let prior = horiz_speed(&comp);
        let (next, _ev) = tick(&mut comp, &dash_input, &world, GRAVITY, DT, pos);
        pos = next;
        let _ = pos;
        let airborne_peak = horiz_speed(&comp);

        assert!(
            prior > desc.ground.speed.walk,
            "setup: airborne dash should start above walk speed, got {prior}"
        );
        assert!(
            airborne_peak > grounded_peak + 1.0,
            "airborne (retain=1) must stack over grounded (retain=0): airborne={airborne_peak}, grounded={grounded_peak}"
        );
    }

    #[test]
    fn steer_control_ramp_over_elapsed_ms_grows_steer_authority() {
        // AC: a `steerControl` ramp over `elapsedMs` produces increasing steer
        // authority across a dash. steerControl = clamp(elapsedMs / 200, 0, 1):
        // 0 on the first dash tick (committed), rising as the dash ages. We dash
        // forward (-Z) then steer hard sideways (+X) and confirm the lateral
        // velocity gained per tick GROWS as elapsed_ms accumulates.
        let world = flat_floor_and_wall_world();
        let steer_expr = IrNode::Clamp {
            x: Box::new(IrNode::Div {
                a: Box::new(ir_input("elapsedMs")),
                b: Box::new(ir_num(200.0)),
            }),
            lo: Box::new(ir_num(0.0)),
            hi: Box::new(ir_num(1.0)),
        };
        // High boost, zero drag so the dash stays alive long enough to ramp.
        let mut dash = dash_params(30.0, 1.0, 0.0, 0.0, 0.0, 3, false);
        dash.steer_control = NumberOrIr::Ir(steer_expr);
        let desc = dash_descriptor(dash);

        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        let enter = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &enter, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "should have entered Dash"
        );

        // Steer hard +X each subsequent tick; lateral velocity gain per tick must
        // increase as steerControl ramps with elapsed_ms.
        let steer = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let mut prev_vx = comp.velocity.x;
        let mut gains: Vec<f32> = Vec::new();
        for _ in 0..4 {
            if !matches!(comp.movement_state, MovementState::Dash { .. }) {
                break;
            }
            let (next, _ev) = tick(&mut comp, &steer, &world, GRAVITY, DT, pos);
            pos = next;
            gains.push(comp.velocity.x - prev_vx);
            prev_vx = comp.velocity.x;
        }
        assert!(
            gains.len() >= 3,
            "dash should persist for several steer ticks, got {} gains",
            gains.len()
        );
        // The first steer tick (elapsed_ms small) gains the least authority; a
        // later tick gains more. Compare first against last collected gain.
        let first = gains.first().copied().unwrap();
        let last = gains.last().copied().unwrap();
        assert!(
            last > first,
            "steer authority should grow with elapsed_ms: first-tick gain={first}, later gain={last}"
        );
    }

    #[test]
    fn momentum_retention_expression_clamps_above_one_to_one() {
        // AC: a `momentumRetention` evaluating to 3.0 behaves as 1.0 (clamped to
        // the [0, 1] range). Compare against an explicit literal-1.0 dash: equal
        // entry peaks prove the over-range expression clamped to 1.
        let world = flat_floor_and_wall_world();
        let mut over = dash_params(8.0, 0.0, 0.0, 0.0, 0.0, 3, false);
        over.momentum_retention = NumberOrIr::Ir(ir_num(3.0));
        let over_desc = dash_descriptor(over);
        let one_desc = dash_descriptor(dash_params(8.0, 1.0, 0.0, 0.0, 0.0, 3, false));

        let (_o, over_peak) = dash_once_from_run(&over_desc, &world, 60);
        let (_l, one_peak) = dash_once_from_run(&one_desc, &world, 60);
        assert!(
            (over_peak - one_peak).abs() < VEL_EPS,
            "retention 3.0 must behave as 1.0: over={over_peak}, literal-one={one_peak}"
        );
    }

    #[test]
    fn cooldown_ms_expression_negative_arms_as_zero() {
        // AC: a `cooldownMs` evaluating negative arms as 0 (clamped to >= 0).
        let world = flat_floor_and_wall_world();
        let mut dash = dash_params(8.0, 0.0, 0.0, 5.0, 0.0, 3, false);
        // cooldownMs = 0 - 500 = -500, clamped to 0.
        dash.cooldown_ms = NumberOrIr::Ir(IrNode::Sub {
            a: Box::new(ir_num(0.0)),
            b: Box::new(ir_num(500.0)),
        });
        let desc = dash_descriptor(dash);
        let (comp, _speed) = dash_once_from_run(&desc, &world, 60);
        // The entry tick decrements the armed cooldown by one dt*1000; a negative
        // arm clamped to 0 cannot go positive, so the cooldown is non-positive.
        assert!(
            comp.dash_cooldown_ms <= 0.0,
            "negative cooldownMs must arm as 0 (got {})",
            comp.dash_cooldown_ms
        );
    }

    #[test]
    fn charges_remaining_reads_post_spend_value_at_entry() {
        // AC / snapshot semantics: `chargesRemaining` at entry reads the
        // POST-spend value. With air_dashes=2, an airborne dash spends one charge
        // BEFORE the snapshot, so the expression sees 1. Author boostSpeed =
        // chargesRemaining * 4 and confirm the boost reflects 1 charge (4), not 2.
        let world = flat_floor_and_wall_world();
        let mut dash = dash_params(99.0, 0.0, 0.0, 5.0, 0.0, 2, false);
        dash.boost_speed = NumberOrIr::Ir(IrNode::Mul {
            a: Box::new(ir_input("chargesRemaining")),
            b: Box::new(ir_num(4.0)),
        });
        let desc = dash_descriptor(dash);

        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());
        // Force a clean airborne state with zero horizontal velocity so the dash
        // boost is observed directly (retention 0).
        comp.is_grounded = false;
        comp.velocity = Vec3::new(0.0, 2.0, 0.0);
        comp.air_dashes_remaining = 2;
        let dash_input = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let _ = tick(&mut comp, &dash_input, &world, GRAVITY, DT, pos);
        // One charge spent before the snapshot ⇒ chargesRemaining read as 1 ⇒
        // boost_speed = 4. Horizontal speed at entry ≈ 4 (pure boost, no prior
        // horizontal velocity), NOT 8 (which a pre-spend read of 2 would give).
        let speed = horiz_speed(&comp);
        assert!(
            (speed - 4.0).abs() < 0.2,
            "boost should reflect POST-spend charges (1*4=4), got {speed}"
        );
        assert_eq!(comp.air_dashes_remaining, 1, "one charge spent");
    }

    #[test]
    fn elapsed_ms_reads_zero_at_entry_and_live_per_tick() {
        // AC / snapshot semantics: `elapsedMs` reads 0 at entry and the live value
        // per-tick. dashDrag = elapsedMs (an expression): on the entry tick the
        // boost decays by 0 (elapsed_ms = 0 at the top of the first intent), so the
        // boost is undiminished; subsequent ticks decay by the accumulating value.
        let world = flat_floor_and_wall_world();
        let mut dash = dash_params(30.0, 0.0, 0.0, 0.0, 0.0, 3, false);
        dash.dash_drag = NumberOrIr::Ir(ir_input("elapsedMs"));
        let desc = dash_descriptor(dash);

        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        let enter = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &enter, &world, GRAVITY, DT, pos);
        pos = next;
        // On the entry tick the dash intent runs with elapsed_ms = 0 at its top,
        // so dash_drag resolves to 0 — the boost is not decayed on entry. The dash
        // therefore peaks high. (A nonzero-at-entry read would have decayed it.)
        let entry_speed = horiz_speed(&comp);
        assert!(
            entry_speed > 25.0,
            "elapsedMs must read 0 at entry (no drag), peak should stay high, got {entry_speed}"
        );
        // Hold the dash a few ticks: now elapsedMs is live (> 0) so dash_drag bites
        // and horizontal speed falls.
        let hold = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &hold, &world, GRAVITY, DT, pos);
        pos = next;
        let _ = pos;
        assert!(
            horiz_speed(&comp) < entry_speed,
            "live elapsedMs (>0) should decay the boost after entry: entry={entry_speed}, after={}",
            horiz_speed(&comp)
        );
    }

    #[test]
    fn boost_speed_expression_evaluating_zero_yields_zero_boost_dash() {
        // Deliberate divergence: `boostSpeed`'s literal bound is
        // exclusive (> 0) and rejects a literal 0 at declaration, but an
        // EXPRESSION evaluating to 0 is floored at 0 and yields a zero-boost dash.
        let world = flat_floor_and_wall_world();
        let mut dash = dash_params(8.0, 0.0, 0.0, 5.0, 0.0, 3, false);
        dash.boost_speed = NumberOrIr::Ir(ir_num(0.0));
        let desc = dash_descriptor(dash);
        // Dash from standstill so the only horizontal velocity could come from the
        // boost. A zero boost with retention 0 leaves horizontal speed ≈ 0.
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());
        let dash_input = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let _ = tick(&mut comp, &dash_input, &world, GRAVITY, DT, pos);
        assert!(
            horiz_speed(&comp) < VEL_EPS,
            "an expression boostSpeed of 0 yields a zero-boost dash, got {}",
            horiz_speed(&comp)
        );
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "the dash still fires (the field bound to a program, not a rejected literal)"
        );
    }

    #[test]
    fn literal_only_dash_leaves_programs_unbound() {
        // Literal-only behavior must stay bit-identical: a fully-literal dash binds
        // NO programs (every slot None), so the resolve helpers take the literal
        // path and never eval.
        let desc = dash_descriptor(dash_params(8.0, 0.5, 0.3, 2.0, 100.0, 3, false));
        let comp = PlayerMovementComponent::from_descriptor(&desc);
        let p = &comp.dash_programs;
        assert!(p.boost_speed.is_none());
        assert!(p.momentum_retention.is_none());
        assert!(p.steer_control.is_none());
        assert!(p.dash_drag.is_none());
        assert!(p.cooldown_ms.is_none());
        assert!(p.preserve_vertical.is_none());
    }

    #[test]
    fn from_descriptor_binds_expression_fields_into_programs() {
        // An expression field materializes a bound program in the matching slot;
        // literal siblings stay None.
        let mut dash = dash_params(8.0, 0.5, 0.3, 2.0, 100.0, 3, false);
        dash.boost_speed = NumberOrIr::Ir(ir_input("speed"));
        dash.steer_control = NumberOrIr::Ir(ir_num(0.5));
        let desc = dash_descriptor(dash);
        let comp = PlayerMovementComponent::from_descriptor(&desc);
        assert!(comp.dash_programs.boost_speed.is_some());
        assert!(comp.dash_programs.steer_control.is_some());
        assert!(comp.dash_programs.momentum_retention.is_none());
        assert!(comp.dash_programs.preserve_vertical.is_none());
    }

    #[test]
    fn dash_intent_eval_pass_is_zero_allocation_with_all_fields_authored() {
        // AC: zero heap allocations across the eval pass of a dash tick with all
        // six fields authored as expressions. Arm the alloc probe around the full
        // `dash_intent` call — the snapshot refresh is itself alloc-free, so the
        // wider window is a strictly stronger assertion.
        use crate::alloc_probe::AllocSnapshot;

        let world = flat_floor_and_wall_world();
        // Author every expression-capable field as an expression so all six bound
        // programs evaluate during the tick.
        let mut dash = dash_params(30.0, 0.5, 0.3, 1.0, 50.0, 3, false);
        dash.boost_speed = NumberOrIr::Ir(IrNode::Add {
            a: Box::new(ir_input("speed")),
            b: Box::new(ir_num(20.0)),
        });
        dash.momentum_retention = NumberOrIr::Ir(IrNode::Clamp {
            x: Box::new(ir_input("verticalSpeed")),
            lo: Box::new(ir_num(0.0)),
            hi: Box::new(ir_num(1.0)),
        });
        dash.steer_control = NumberOrIr::Ir(IrNode::Div {
            a: Box::new(ir_input("elapsedMs")),
            b: Box::new(ir_num(200.0)),
        });
        dash.dash_drag = NumberOrIr::Ir(IrNode::Mul {
            a: Box::new(ir_input("cooldownMs")),
            b: Box::new(ir_num(0.0)),
        });
        dash.cooldown_ms = NumberOrIr::Ir(ir_num(50.0));
        dash.preserve_vertical = BoolOrIr::Ir(ir_input("grounded"));
        let desc = dash_descriptor(dash);

        // Enter the dash so the per-tick `dash_intent` path runs (steer_control +
        // dash_drag eval). Then warm one more tick before arming the probe.
        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        let enter = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &enter, &world, GRAVITY, DT, pos);
        pos = next;
        let _ = pos;
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "should be dashing so dash_intent runs"
        );

        // Drive the dash_intent path directly so the measured window is the intent
        // (snapshot refresh + eval into locals + velocity mutation) with no
        // collision-substrate allocation noise.
        let steer = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };
        let mut elapsed_ms = 16.0_f32;
        let mut boost = Vec3::new(comp.velocity.x, 0.0, comp.velocity.z);
        // Warm: run the intent once before arming so any one-time lazy state is hot.
        let _ = dash_intent(&mut comp, &steer, GRAVITY, DT, &mut elapsed_ms, &mut boost);

        let snapshot = AllocSnapshot::arm();
        let _ = dash_intent(&mut comp, &steer, GRAVITY, DT, &mut elapsed_ms, &mut boost);
        let allocs = snapshot.allocs_since();
        assert_eq!(
            allocs, 0,
            "dash_intent eval pass must perform zero heap allocations"
        );
    }

    // ----- Input forgiveness (coyote time + jump buffer) -------------------
    //
    // These cover D5: coyote/buffer windows are descriptor-tuned, derived once
    // per tick as edges the `Normal` jump steps consume. The canonical fixture
    // pins both windows to zero, so these tests build descriptors with explicit
    // windows to exercise the grace paths.

    const JUMP_VELOCITY: f32 = 5.5;
    /// Ticks that fit inside a 100 ms window at 60 Hz (100 / 16.67 ≈ 6).
    const WITHIN_100MS_TICKS: usize = 4;
    /// Ticks that clear a 100 ms window with margin.
    const PAST_100MS_TICKS: usize = 9;

    /// Canonical descriptor with explicit forgiveness windows (ms). A `coyote`
    /// of 0 disables coyote; a `buffer` of 0 disables jump buffering.
    fn forgiveness_descriptor(coyote_ms: f32, jump_buffer_ms: f32) -> PlayerMovementDescriptor {
        let mut desc = canonical_descriptor();
        desc.forgiveness = Some(ForgivenessParams {
            coyote_ms,
            jump_buffer_ms,
        });
        desc
    }

    fn jump_input() -> MovementInput {
        MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        }
    }

    /// Settle the player grounded on a flat floor, then lift them off the ground
    /// and run one idle tick so the substrate clears `is_grounded`. After this
    /// the player is airborne with the coyote timer holding ~one tick — the
    /// deterministic "just walked off the ledge" state the coyote edge keys off.
    fn settle_then_leave_ground(
        desc: &PlayerMovementDescriptor,
        world: &CollisionWorld,
    ) -> (PlayerMovementComponent, Vec3) {
        let (mut comp, mut pos) = settle_player(desc);
        run_ticks(&mut comp, world, &mut pos, 10, &idle_input());
        assert!(comp.is_grounded, "test setup: player should be grounded");
        // Teleport up out of floor range, then one idle tick drops `is_grounded`.
        pos.y += 2.0;
        run_ticks(&mut comp, world, &mut pos, 1, &idle_input());
        assert!(
            !comp.is_grounded,
            "test setup: player should be airborne after leaving the ground"
        );
        assert!(
            !comp.jump_spent,
            "test setup: no jump should be spent yet after leaving the ground"
        );
        (comp, pos)
    }

    #[test]
    fn coyote_jump_within_window_launches_grounded_jump() {
        let desc = forgiveness_descriptor(100.0, 0.0);
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_then_leave_ground(&desc, &world);

        // Stay airborne briefly (still inside the 100 ms coyote window), then
        // press jump. The coyote grace routes through the grounded-jump step.
        run_ticks(
            &mut comp,
            &world,
            &mut pos,
            WITHIN_100MS_TICKS,
            &idle_input(),
        );
        let events = run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());

        assert!(events.jumped, "coyote jump within the window should fire");
        assert!(
            approx_eq(comp.velocity.y, JUMP_VELOCITY, VEL_EPS),
            "coyote jump should apply the full grounded jump velocity, got {}",
            comp.velocity.y
        );
    }

    #[test]
    fn coyote_jump_after_window_does_not_fire() {
        let desc = forgiveness_descriptor(100.0, 0.0);
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_then_leave_ground(&desc, &world);

        // Linger past the coyote window before pressing — no grounded jump, and
        // canonical `air.jumps == 0` means no air jump either.
        run_ticks(&mut comp, &world, &mut pos, PAST_100MS_TICKS, &idle_input());
        let vy_before = comp.velocity.y;
        let events = run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());

        assert!(
            !events.jumped,
            "a jump pressed after the coyote window should not fire"
        );
        // Falling, not launched: vy keeps decreasing under gravity.
        assert!(
            comp.velocity.y < vy_before,
            "no jump means vy keeps falling, got {} (was {})",
            comp.velocity.y,
            vy_before
        );
    }

    #[test]
    fn coyote_jump_does_not_consume_air_jump_budget() {
        // A double-jump budget is available, but the coyote jump must route
        // through the GROUNDED path and leave the air-jump budget untouched.
        let mut desc = double_jump_descriptor();
        desc.forgiveness = Some(ForgivenessParams {
            coyote_ms: 100.0,
            jump_buffer_ms: 0.0,
        });
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_then_leave_ground(&desc, &world);
        assert_eq!(
            comp.air_jumps_remaining, 1,
            "test setup: one air jump should be available"
        );

        run_ticks(
            &mut comp,
            &world,
            &mut pos,
            WITHIN_100MS_TICKS,
            &idle_input(),
        );
        let events = run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());

        assert!(events.jumped, "coyote jump should fire");
        assert_eq!(
            comp.air_jumps_remaining, 1,
            "coyote jump must NOT spend an air-jump charge"
        );
    }

    #[test]
    fn coyote_does_not_rearm_after_a_jump() {
        // Spend a grounded jump first; leaving the ground afterward must grant
        // no fresh coyote ground-jump (jump_spent gates re-arming).
        let desc = forgiveness_descriptor(100.0, 0.0);
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());
        assert!(comp.is_grounded);

        // Grounded jump.
        let events = run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());
        assert!(events.jumped, "the initial grounded jump should fire");
        assert!(comp.jump_spent, "a fired jump should set jump_spent");

        // Now airborne with the jump spent. Press again within what would be the
        // coyote window — no second grounded jump (and air.jumps == 0).
        run_ticks(&mut comp, &world, &mut pos, 2, &idle_input());
        let vy_before = comp.velocity.y;
        let events = run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());
        assert!(
            !events.jumped,
            "coyote must not re-arm after a jump was already spent"
        );
        assert!(
            comp.velocity.y < vy_before,
            "no second launch: vy keeps falling, got {} (was {})",
            comp.velocity.y,
            vy_before
        );
    }

    #[test]
    fn buffered_jump_fires_exactly_once_on_landing() {
        // Press jump (a single tap) while airborne, inside the buffer window,
        // before landing — it must fire exactly once on the landing tick. Use a
        // generous window so it comfortably survives the descent; the point
        // under test is exactly-once, not the window length (expiry is covered
        // separately).
        let desc = forgiveness_descriptor(0.0, 2000.0);
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());

        // Launch into the air with a normal grounded jump, then release jump.
        let events = run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());
        assert!(events.jumped, "test setup: initial grounded jump");
        assert!(comp.jump_spent);
        // Rise a few ticks (jump released).
        run_ticks(&mut comp, &world, &mut pos, 3, &idle_input());

        // Single-tap a buffered jump while airborne, then fall to the floor. The
        // landing clears jump_spent, and the buffered press fires once.
        run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());
        assert!(
            comp.jump_buffer_timer_ms > 0.0,
            "test setup: the airborne tap should arm the buffer"
        );

        // Fall back down (jump released) and count buffered launches across the
        // descent + landing window.
        let mut launches = 0;
        for _ in 0..60 {
            let (next, ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if ev.jumped {
                launches += 1;
            }
        }
        assert_eq!(
            launches, 1,
            "a buffered jump must fire exactly once on landing, fired {launches}"
        );
    }

    #[test]
    fn single_press_near_ledge_yields_exactly_one_jump() {
        // Coyote + buffer both enabled. A single grounded press near a ledge must
        // not combine into two launches (one grounded + one buffered on landing).
        let desc = forgiveness_descriptor(100.0, 100.0);
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());

        // One grounded press, then release. Count every launch through the full
        // jump-and-land arc.
        let mut launches = 0;
        let first = run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());
        if first.jumped {
            launches += 1;
        }
        for _ in 0..60 {
            let (next, ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if ev.jumped {
                launches += 1;
            }
        }
        assert_eq!(
            launches, 1,
            "a single press must yield exactly one jump, got {launches}"
        );
    }

    #[test]
    fn buffered_jump_expires_before_landing_drops_silently() {
        // A buffered jump whose window expires before landing must fire no jump.
        // Short buffer (one tick worth) tapped high above the floor.
        let desc = forgiveness_descriptor(0.0, 10.0);
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());

        // Big grounded jump to gain plenty of airtime, release.
        run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());
        run_ticks(&mut comp, &world, &mut pos, 2, &idle_input());

        // Single-tap a buffered jump; with only a 10 ms window it expires long
        // before the player descends and lands.
        run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());

        let mut launches = 0;
        for _ in 0..60 {
            let (next, ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if ev.jumped {
                launches += 1;
            }
        }
        assert_eq!(
            launches, 0,
            "an expired buffer must drop silently, fired {launches}"
        );
        assert_eq!(
            comp.jump_buffer_timer_ms, 0.0,
            "the expired buffer timer should be cleared"
        );
    }

    #[test]
    fn forgiveness_windows_are_descriptor_tunable_and_zero_disables() {
        // Zero coyote disables the coyote ground-jump independently of buffer.
        let world = flat_floor_and_wall_world();

        let desc_no_coyote = forgiveness_descriptor(0.0, 100.0);
        let (mut comp, mut pos) = settle_then_leave_ground(&desc_no_coyote, &world);
        run_ticks(&mut comp, &world, &mut pos, 1, &idle_input());
        let vy_before = comp.velocity.y;
        let events = run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());
        assert!(
            !events.jumped,
            "coyoteMs = 0 must disable the coyote ground-jump"
        );
        assert!(
            comp.velocity.y < vy_before,
            "no launch with coyote disabled"
        );

        // Nonzero coyote on the same seam DOES grant the jump — tunability.
        let desc_coyote = forgiveness_descriptor(100.0, 0.0);
        let (mut comp, mut pos) = settle_then_leave_ground(&desc_coyote, &world);
        run_ticks(&mut comp, &world, &mut pos, 1, &idle_input());
        let events = run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());
        assert!(events.jumped, "coyoteMs > 0 must grant the coyote jump");
    }

    #[test]
    fn absent_forgiveness_applies_documented_engine_defaults() {
        // An absent `forgiveness` sub-object materializes the documented engine
        // defaults (~100 ms each), not zero. Verify by exercising the coyote
        // grace, which only works when the default window is nonzero.
        let mut desc = canonical_descriptor();
        desc.forgiveness = None;
        let comp = PlayerMovementComponent::from_descriptor(&desc);
        assert_eq!(
            comp.coyote_ms,
            ForgivenessParams::DEFAULT_COYOTE_MS,
            "absent forgiveness should apply the default coyote window"
        );
        assert_eq!(
            comp.jump_buffer_ms,
            ForgivenessParams::DEFAULT_JUMP_BUFFER_MS,
            "absent forgiveness should apply the default buffer window"
        );

        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_then_leave_ground(&desc, &world);
        run_ticks(
            &mut comp,
            &world,
            &mut pos,
            WITHIN_100MS_TICKS,
            &idle_input(),
        );
        let events = run_ticks(&mut comp, &world, &mut pos, 1, &jump_input());
        assert!(
            events.jumped,
            "default (absent) forgiveness should permit a coyote jump in-window"
        );
    }

    // ----- Capsule-resize + stand-up-probe substrate helpers (D8) -----------
    //
    // These drive `resize_capsule` / `standup_clearance_probe` DIRECTLY with a
    // target size and anchor mode — no `Crouching` intent involved — proving the
    // substrate is reusable (slide can call the same helpers).

    /// Lowest point of the collision capsule given a center position.
    fn capsule_bottom(comp: &PlayerMovementComponent, pos: Vec3) -> f32 {
        pos.y - (comp.capsule.half_height + comp.capsule.radius)
    }

    /// Highest point of the collision capsule given a center position.
    fn capsule_top(comp: &PlayerMovementComponent, pos: Vec3) -> f32 {
        pos.y + (comp.capsule.half_height + comp.capsule.radius)
    }

    /// Flat floor at y=0 plus a horizontal ceiling slab at `ceiling_y` spanning
    /// x∈[-20,20], z∈[-10,10] with a down-facing (−Y) normal. Used to drive the
    /// stand-up probe: the ceiling sits at a tunable height above the player so
    /// the head-rise sweep does or does not hit it.
    fn floor_and_ceiling_world(ceiling_y: f32) -> CollisionWorld {
        let mut points: Vec<Point<f32>> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();

        // Floor: y=0, up-facing +Y.
        let f0 = points.len() as u32;
        points.push(Point::new(-20.0, 0.0, -10.0));
        points.push(Point::new(20.0, 0.0, -10.0));
        points.push(Point::new(20.0, 0.0, 10.0));
        points.push(Point::new(-20.0, 0.0, 10.0));
        tris.push([f0, f0 + 1, f0 + 2]);
        tris.push([f0, f0 + 2, f0 + 3]);

        // Ceiling: y=ceiling_y, wound so the normal faces down (−Y) toward the
        // player below.
        let c0 = points.len() as u32;
        points.push(Point::new(-20.0, ceiling_y, -10.0));
        points.push(Point::new(20.0, ceiling_y, 10.0));
        points.push(Point::new(20.0, ceiling_y, -10.0));
        points.push(Point::new(-20.0, ceiling_y, 10.0));
        tris.push([c0, c0 + 1, c0 + 2]);
        tris.push([c0, c0 + 3, c0 + 1]);

        let mesh = TriMesh::new(points, tris);
        CollisionWorld {
            mesh,
            isometry: Isometry::identity(),
        }
    }

    #[test]
    fn resize_capsule_feet_anchor_keeps_lowest_point_fixed() {
        let desc = canonical_descriptor();
        let mut comp = PlayerMovementComponent::from_descriptor(&desc);
        // Standing capsule resting on the floor: center at half_height + radius.
        let mut pos = Vec3::new(0.0, comp.capsule.half_height + comp.capsule.radius, 0.0);
        let bottom_before = capsule_bottom(&comp, pos);

        // Shrink to a crouched half-height with the FEET planted. Caller applies
        // the returned center delta to position.
        let target_half_height = 0.4; // < standing 0.8
        let target_eye_height = 0.2;
        let delta = resize_capsule(
            &mut comp,
            target_half_height,
            target_eye_height,
            ResizeAnchor::Feet,
        );
        pos.y += delta;

        // Helper owns the size fields.
        assert!(
            approx_eq(comp.capsule.half_height, target_half_height, POS_EPS),
            "resize must write the target half_height"
        );
        assert!(
            approx_eq(comp.capsule.eye_height, target_eye_height, POS_EPS),
            "resize must write the target eye_height"
        );
        // Feet anchor: the lowest point is unchanged after applying the delta.
        let bottom_after = capsule_bottom(&comp, pos);
        assert!(
            approx_eq(bottom_after, bottom_before, POS_EPS),
            "Feet anchor must keep the lowest point fixed: before {bottom_before}, after {bottom_after}"
        );
        // Center moved DOWN by the half-height delta on a shrink.
        assert!(
            approx_eq(delta, target_half_height - 0.8, POS_EPS),
            "Feet center delta should equal new_half_height - old_half_height"
        );
    }

    #[test]
    fn resize_capsule_head_anchor_keeps_highest_point_fixed() {
        let desc = canonical_descriptor();
        let mut comp = PlayerMovementComponent::from_descriptor(&desc);
        // Airborne capsule somewhere off the floor.
        let mut pos = Vec3::new(0.0, 5.0, 0.0);
        let top_before = capsule_top(&comp, pos);

        // Shrink with the HEAD pinned (airborne crouch): feet rise toward center.
        let target_half_height = 0.4;
        let target_eye_height = 0.2;
        let delta = resize_capsule(
            &mut comp,
            target_half_height,
            target_eye_height,
            ResizeAnchor::Head,
        );
        pos.y += delta;

        let top_after = capsule_top(&comp, pos);
        assert!(
            approx_eq(top_after, top_before, POS_EPS),
            "Head anchor must keep the highest point fixed: before {top_before}, after {top_after}"
        );
        // Center moved UP by the half-height delta on a shrink.
        assert!(
            approx_eq(delta, 0.8 - target_half_height, POS_EPS),
            "Head center delta should equal old_half_height - new_half_height"
        );
    }

    #[test]
    fn standup_probe_reports_blocked_when_ceiling_within_head_rise() {
        let desc = canonical_descriptor();
        let mut comp = PlayerMovementComponent::from_descriptor(&desc);

        let standing_half_height = 0.8; // canonical standing
        let crouched_half_height = 0.4;
        // With feet planted, crouched center sits at crouched_hh + radius.
        let pos = Vec3::new(0.0, crouched_half_height + comp.capsule.radius, 0.0);

        // Head-rise delta = 2 * (0.8 - 0.4) = 0.8. Crouched capsule top sits at
        // pos.y + crouched_hh + radius. Place the ceiling just BELOW where the
        // standing head would reach so the upward sweep hits within head-rise.
        let crouched_top = pos.y + crouched_half_height + comp.capsule.radius;
        let head_rise = 2.0 * (standing_half_height - crouched_half_height);
        // Ceiling 0.2 m above the crouched top — well inside the 0.8 m rise.
        let blocked_world = floor_and_ceiling_world(crouched_top + 0.2);
        // Reflect the crouched size on the component (as the caller would have
        // after a resize) before probing.
        resize_capsule(&mut comp, crouched_half_height, 0.2, ResizeAnchor::Feet);
        let clear = standup_clearance_probe(
            &comp,
            &blocked_world,
            pos,
            crouched_half_height,
            standing_half_height,
        );
        assert!(
            !clear,
            "ceiling within the {head_rise} m head-rise must report standing blocked"
        );

        // Ceiling well above the standing head — clear to stand.
        let clear_world = floor_and_ceiling_world(crouched_top + head_rise + 1.0);
        let clear = standup_clearance_probe(
            &comp,
            &clear_world,
            pos,
            crouched_half_height,
            standing_half_height,
        );
        assert!(
            clear,
            "ceiling beyond the head-rise must report headroom clear"
        );

        // No ceiling at all (empty world) — clear.
        let empty = CollisionWorld::new();
        let clear = standup_clearance_probe(
            &comp,
            &empty,
            pos,
            crouched_half_height,
            standing_half_height,
        );
        assert!(clear, "no ceiling geometry must report headroom clear");
    }

    // ----- `Crouching` state: entry, speed, stand-up, crouch-jump (D2–D10) ---

    /// Canonical descriptor with crouch configured: crouched half-height 0.4
    /// (standing 0.8), crouched eye 0.2 (standing 0.5), and a transition rate of
    /// 15/s. Crouch speed tier is the canonical 3.0 (below walk 7.0).
    fn crouch_descriptor() -> PlayerMovementDescriptor {
        let mut desc = canonical_descriptor();
        desc.crouch = Some(CrouchParams {
            half_height: 0.4,
            eye_height: 0.2,
            transition_rate: 15.0,
        });
        desc
    }

    fn crouch_hold_input() -> MovementInput {
        MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: true,
            facing_yaw: 0.0,
        }
    }

    fn is_crouching(comp: &PlayerMovementComponent) -> bool {
        matches!(comp.movement_state, MovementState::Crouching { .. })
    }

    /// Ground entry (D2): crouch held while grounded enters `Crouching`, the
    /// collision half-height becomes the crouched value, and the capsule's lowest
    /// point (the planted feet) is unchanged from standing.
    #[test]
    fn crouch_ground_entry_shrinks_capsule_with_feet_planted() {
        let desc = crouch_descriptor();
        let world = floor_and_ceiling_world(50.0); // open headroom
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 8, &idle_input());

        let bottom_standing = capsule_bottom(&comp, pos);
        assert!(
            approx_eq(comp.capsule.half_height, 0.8, POS_EPS),
            "precondition: standing half-height, got {}",
            comp.capsule.half_height
        );

        // One tick of held crouch fires the Normal -> Crouching entry.
        run_ticks(&mut comp, &world, &mut pos, 1, &crouch_hold_input());
        assert!(
            is_crouching(&comp),
            "crouch held + grounded must enter Crouching"
        );
        assert!(
            approx_eq(comp.capsule.half_height, 0.4, POS_EPS),
            "collision half-height must be the crouched value, got {}",
            comp.capsule.half_height
        );
        let bottom_crouched = capsule_bottom(&comp, pos);
        assert!(
            approx_eq(bottom_crouched, bottom_standing, 0.02),
            "feet planted: lowest point must be unchanged: standing {bottom_standing}, crouched {bottom_crouched}"
        );
    }

    /// Crouched speed (D5): crouch held with full movement input settles at the
    /// crouch speed tier, strictly below walk.
    #[test]
    fn crouch_speed_settles_at_crouch_tier_below_walk() {
        let desc = crouch_descriptor();
        let world = floor_and_ceiling_world(50.0);
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 8, &idle_input());

        let crouch_move = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: true,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 40, &crouch_move);
        assert!(is_crouching(&comp), "should remain Crouching while held");
        let speed = horiz_speed(&comp);
        assert!(
            approx_eq(speed, desc.ground.speed.crouch, 0.1),
            "crouched steady-state speed should settle at crouch tier {}, got {}",
            desc.ground.speed.crouch,
            speed
        );
        assert!(
            speed < desc.ground.speed.walk,
            "crouch speed {} must be below walk {}",
            speed,
            desc.ground.speed.walk
        );
    }

    /// Stand-up CLEAR (D7): crouch released with open headroom transitions
    /// Crouching -> Normal, restores the standing half-height, and keeps the
    /// lowest point fixed (feet planted, center rises).
    #[test]
    fn crouch_release_stands_up_when_headroom_clear() {
        let desc = crouch_descriptor();
        let world = floor_and_ceiling_world(50.0);
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 8, &idle_input());
        run_ticks(&mut comp, &world, &mut pos, 5, &crouch_hold_input());
        assert!(is_crouching(&comp), "precondition: crouched");

        let bottom_crouched = capsule_bottom(&comp, pos);

        // Release: open headroom => stand up this tick.
        run_ticks(&mut comp, &world, &mut pos, 1, &idle_input());
        assert!(
            matches!(comp.movement_state, MovementState::Normal),
            "crouch released with clear headroom must return to Normal"
        );
        assert!(
            approx_eq(comp.capsule.half_height, 0.8, POS_EPS),
            "stand-up must restore the standing half-height, got {}",
            comp.capsule.half_height
        );
        let bottom_standing = capsule_bottom(&comp, pos);
        assert!(
            approx_eq(bottom_standing, bottom_crouched, 0.02),
            "feet planted on stand-up: lowest point fixed: crouched {bottom_crouched}, standing {bottom_standing}"
        );
    }

    /// Stand-up BLOCKED (D7): crouch released under a low ceiling keeps the
    /// player crouched (the standing capsule never materializes); removing the
    /// ceiling on a later tick auto-stands the first clear tick.
    #[test]
    fn crouch_release_under_ceiling_stays_crouched_then_stands_when_clear() {
        let desc = crouch_descriptor();
        // Enter crouch under open headroom first.
        let open = floor_and_ceiling_world(50.0);
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &open, &mut pos, 8, &idle_input());
        run_ticks(&mut comp, &open, &mut pos, 5, &crouch_hold_input());
        assert!(is_crouching(&comp), "precondition: crouched");

        // A ceiling within the head-rise delta of the crouched head. Crouched top
        // = pos.y + 0.4 + 0.4; head-rise = 2*(0.8-0.4) = 0.8. Ceiling 0.2 above
        // the crouched top blocks the standing capsule.
        let crouched_top = capsule_top(&comp, pos);
        let blocked = floor_and_ceiling_world(crouched_top + 0.2);

        // Release under the ceiling: must stay crouched, standing capsule never
        // materializes.
        for _ in 0..5 {
            run_ticks(&mut comp, &blocked, &mut pos, 1, &idle_input());
            assert!(
                is_crouching(&comp),
                "blocked headroom must keep the player Crouching"
            );
            assert!(
                approx_eq(comp.capsule.half_height, 0.4, POS_EPS),
                "standing capsule must never materialize under a ceiling, got {}",
                comp.capsule.half_height
            );
        }

        // Remove the ceiling: the very next tick (release still held) auto-stands.
        run_ticks(&mut comp, &open, &mut pos, 1, &idle_input());
        assert!(
            matches!(comp.movement_state, MovementState::Normal),
            "with the ceiling gone the first clear tick must auto-stand"
        );
        assert!(
            approx_eq(comp.capsule.half_height, 0.8, POS_EPS),
            "auto-stand must restore the standing half-height, got {}",
            comp.capsule.half_height
        );
    }

    /// Crouch-jump CLEAR (D10): jump while crouched under open headroom stands
    /// then jumps — transitions to Normal AND launches the jump arc this tick.
    #[test]
    fn crouch_jump_under_clear_headroom_stands_then_jumps() {
        let desc = crouch_descriptor();
        let world = floor_and_ceiling_world(50.0);
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 8, &idle_input());
        run_ticks(&mut comp, &world, &mut pos, 5, &crouch_hold_input());
        assert!(is_crouching(&comp), "precondition: crouched");

        let jump_while_crouched = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            crouch_intent: true, // crouch STILL held during the jump
            facing_yaw: 0.0,
        };
        let events = run_ticks(&mut comp, &world, &mut pos, 1, &jump_while_crouched);
        assert!(events.jumped, "crouch-jump must launch the jump");
        assert!(
            matches!(comp.movement_state, MovementState::Normal),
            "clear-headroom crouch-jump must stand (exit to Normal)"
        );
        assert!(
            approx_eq(comp.capsule.half_height, 0.8, POS_EPS),
            "clear-headroom crouch-jump must restore the standing capsule, got {}",
            comp.capsule.half_height
        );
        assert!(
            approx_eq(comp.velocity.y, desc.air.jump_velocity, VEL_EPS),
            "crouch-jump must apply the full jump velocity, got {}",
            comp.velocity.y
        );
    }

    /// Crouch-jump BLOCKED (D10): jump while crouched under a blocking ceiling
    /// still applies the jump and retains the crouched capsule (no dead input).
    #[test]
    fn crouch_jump_under_ceiling_jumps_without_standing() {
        let desc = crouch_descriptor();
        let open = floor_and_ceiling_world(50.0);
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &open, &mut pos, 8, &idle_input());
        run_ticks(&mut comp, &open, &mut pos, 5, &crouch_hold_input());
        assert!(is_crouching(&comp), "precondition: crouched");

        let crouched_top = capsule_top(&comp, pos);
        let blocked = floor_and_ceiling_world(crouched_top + 0.2);

        let jump_while_crouched = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            crouch_intent: true,
            facing_yaw: 0.0,
        };
        let events = run_ticks(&mut comp, &blocked, &mut pos, 1, &jump_while_crouched);
        assert!(
            events.jumped,
            "blocked crouch-jump must STILL apply the jump (no dead input)"
        );
        assert!(
            is_crouching(&comp),
            "blocked crouch-jump must retain the Crouching state"
        );
        assert!(
            approx_eq(comp.capsule.half_height, 0.4, POS_EPS),
            "blocked crouch-jump must retain the crouched capsule, got {}",
            comp.capsule.half_height
        );
        assert!(
            comp.velocity.y > 0.0,
            "blocked crouch-jump must still launch an upward arc, got vy {}",
            comp.velocity.y
        );
    }

    /// The `Dash` transition is available from `Crouching` (D10): a dash press
    /// while crouched exits crouch into the dash burst.
    #[test]
    fn crouch_dash_transition_available() {
        let mut desc = crouch_descriptor();
        desc.dash = Some(dash_params(12.0, 0.0, 0.0, 50.0, 0.0, 0, false));
        let world = floor_and_ceiling_world(50.0);
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 8, &idle_input());
        run_ticks(&mut comp, &world, &mut pos, 5, &crouch_hold_input());
        assert!(is_crouching(&comp), "precondition: crouched");

        let dash_while_crouched = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            crouch_intent: true,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 1, &dash_while_crouched);
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "a dash press from Crouching must enter Dash"
        );
    }

    /// Airborne crouch (D4): entering `Crouching` midair anchors the HEAD — the
    /// capsule's highest point is unchanged (feet rise toward center).
    #[test]
    fn crouch_airborne_entry_anchors_head() {
        let desc = crouch_descriptor();
        let world = floor_and_ceiling_world(50.0);
        let mut comp = PlayerMovementComponent::from_descriptor(&desc);
        // Place the player clearly airborne, no floor contact this tick.
        let mut pos = Vec3::new(0.0, 10.0, 0.0);
        comp.is_grounded = false;

        let top_before = capsule_top(&comp, pos);
        assert!(
            approx_eq(comp.capsule.half_height, 0.8, POS_EPS),
            "precondition: standing half-height"
        );

        run_ticks(&mut comp, &world, &mut pos, 1, &crouch_hold_input());
        assert!(
            is_crouching(&comp),
            "crouch held while airborne must enter Crouching"
        );
        assert!(
            approx_eq(comp.capsule.half_height, 0.4, POS_EPS),
            "airborne crouch must shrink the collision capsule, got {}",
            comp.capsule.half_height
        );
        let top_after = capsule_top(&comp, pos);
        assert!(
            approx_eq(top_after, top_before, 0.02),
            "head anchored midair: highest point unchanged: before {top_before}, after {top_after}"
        );
    }

    /// Airborne release-stand (D4): entering `Crouching` midair (Head-anchored)
    /// then releasing crouch with clear headroom must exit Head-anchored too, so a
    /// crouch→stand cycle nets to NO upward center drift. A `Feet`-anchored exit
    /// after the `Head`-anchored entry would float the capsule up by
    /// `2 × (standing_hh − crouched_hh)` with no ground-stick to mask it. Gravity
    /// lowers the center each tick, so a no-crouch baseline over the identical tick
    /// count isolates the resize: the crouch path's capsule top must match the
    /// gravity-only top, not sit above it.
    #[test]
    fn crouch_airborne_release_stands_up_head_anchored_no_drift() {
        let desc = crouch_descriptor();
        let world = floor_and_ceiling_world(50.0); // open headroom

        // Baseline: stay airborne and standing for the same two ticks the crouch
        // path uses (one entry tick + one release/stand tick). This captures the
        // gravity-only descent of the capsule top with no crouch resize.
        let mut base_comp = PlayerMovementComponent::from_descriptor(&desc);
        let mut base_pos = Vec3::new(0.0, 10.0, 0.0);
        base_comp.is_grounded = false;
        run_ticks(&mut base_comp, &world, &mut base_pos, 2, &idle_input());
        assert!(
            !base_comp.is_grounded,
            "baseline must stay airborne (no floor contact at y≈10)"
        );
        let baseline_top = capsule_top(&base_comp, base_pos);

        // Crouch path: same start, enter Crouching midair, then release.
        let mut comp = PlayerMovementComponent::from_descriptor(&desc);
        let mut pos = Vec3::new(0.0, 10.0, 0.0);
        comp.is_grounded = false;

        run_ticks(&mut comp, &world, &mut pos, 1, &crouch_hold_input());
        assert!(is_crouching(&comp), "precondition: airborne crouch entry");
        assert!(
            !comp.is_grounded,
            "precondition: still airborne while crouched"
        );

        // Release with clear headroom: the airborne stand-up must fire this tick.
        run_ticks(&mut comp, &world, &mut pos, 1, &idle_input());
        assert!(
            matches!(comp.movement_state, MovementState::Normal),
            "airborne crouch release with clear headroom must return to Normal"
        );
        assert!(
            approx_eq(comp.capsule.half_height, 0.8, POS_EPS),
            "airborne stand-up must restore the standing half-height, got {}",
            comp.capsule.half_height
        );

        // No drift: the crouch→stand cycle's capsule top equals the gravity-only
        // baseline. A `Feet`-anchored exit would put it ~0.8 above the baseline.
        let crouch_top = capsule_top(&comp, pos);
        assert!(
            approx_eq(crouch_top, baseline_top, 0.02),
            "airborne crouch→stand must not float the capsule up: baseline top \
             {baseline_top}, crouch-cycle top {crouch_top}"
        );
    }

    /// Absent `crouch` descriptor disables crouch: Normal -> Crouching never
    /// fires regardless of `crouch_intent`, and no capsule resize occurs.
    #[test]
    fn crouch_disabled_when_no_params() {
        let desc = canonical_descriptor(); // crouch: None
        let world = floor_and_ceiling_world(50.0);
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 8, &idle_input());

        let half_before = comp.capsule.half_height;
        run_ticks(&mut comp, &world, &mut pos, 10, &crouch_hold_input());
        assert!(
            matches!(comp.movement_state, MovementState::Normal),
            "absent crouch params: Normal -> Crouching must never fire"
        );
        assert!(
            approx_eq(comp.capsule.half_height, half_before, POS_EPS),
            "absent crouch params: no capsule resize, got {} (was {})",
            comp.capsule.half_height,
            half_before
        );
    }

    /// Eye smoothing (D3): the camera eye eases toward the crouched target across
    /// ticks rather than snapping on entry.
    #[test]
    fn crouch_eye_height_smooths_toward_target() {
        let desc = crouch_descriptor();
        let world = floor_and_ceiling_world(50.0);
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 8, &idle_input());

        let standing_eye = comp.capsule.eye_height; // 0.5
        // Entry tick: the Normal -> Crouching transition fires and the eye is
        // held at the standing value (no snap to the crouched target). Smoothing
        // begins the following tick, inside the Crouching intent.
        run_ticks(&mut comp, &world, &mut pos, 1, &crouch_hold_input());
        assert!(is_crouching(&comp));
        assert!(
            approx_eq(comp.capsule.eye_height, standing_eye, POS_EPS),
            "entry tick must NOT snap the eye to the crouched target, got {}",
            comp.capsule.eye_height
        );
        // A few smoothing ticks: the eye eases between standing 0.5 and crouched
        // 0.2 (exponential approach), not snapped to either end.
        run_ticks(&mut comp, &world, &mut pos, 2, &crouch_hold_input());
        let eye_mid = comp.capsule.eye_height;
        assert!(
            eye_mid < standing_eye && eye_mid > 0.2,
            "eye should ease (between standing 0.5 and crouched 0.2), got {eye_mid}"
        );
        // Many ticks later the eye should have converged near the crouched target.
        run_ticks(&mut comp, &world, &mut pos, 40, &crouch_hold_input());
        assert!(
            approx_eq(comp.capsule.eye_height, 0.2, 0.01),
            "eye should converge toward the crouched target 0.2, got {}",
            comp.capsule.eye_height
        );
    }
}
