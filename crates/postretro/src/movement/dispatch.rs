// Player movement state dispatch and transition carry glue.
// See: context/lib/movement.md §4

use glam::Vec3;

use crate::collision::CollisionWorld;
use crate::movement::carry::{CarryRule, apply_boost, apply_horizontal};
use crate::movement::intents::{crouching_intent, dash_intent, normal_intent};
use crate::movement::substrate::{JumpEdges, ResizeAnchor, resize_capsule};
use crate::movement::{MovementEvents, MovementInput, Transition};
use postretro_foundation::{MovementState, PlayerMovementComponent};

/// Resize the live capsule from the crouched size back to the configured
/// STANDING reference (`standing_half_height` / `standing_eye_height`), keeping
/// the `anchor`-selected capsule extreme fixed, and apply the helper-returned
/// center delta to `position`. Used by both stand-up paths (release and
/// crouch-jump). The eye snaps to the standing target here — the smoothing window
/// is the descent into crouch; standing back up restores the standing eye
/// directly (the camera-follow read picks it up next tick). Returns the applied
/// center delta.
///
/// The `anchor` MUST mirror the crouch-ENTRY anchor so an entry→exit cycle is a
/// net no-op on the capsule center (D4). Crouch entry is grounded-vs-airborne
/// anchored — `Feet` when grounded (feet planted, center drops), `Head` when
/// airborne (head pinned, feet rise). Stand-up inverts the SAME axis: grounded ⇒
/// `Feet` (feet planted, center rises back); airborne ⇒ `Head` (head pinned, feet
/// drop back). A mismatched exit anchor double-applies the half-height delta to
/// the center — an airborne `Feet` exit after a `Head` entry floats the player up
/// by `2 × (standing_hh − crouched_hh)` with no ground-stick to mask it. The
/// caller selects `anchor` from grounded-AT-TICK-START (snapshotted before the
/// jump branch may clear `is_grounded`), so a ground-origin crouch-jump still
/// anchors the feet and launches from the planted position.
pub(super) fn stand_up_resize(
    component: &mut PlayerMovementComponent,
    position: &mut Vec3,
    anchor: ResizeAnchor,
) -> f32 {
    let target_half_height = component.standing_half_height;
    let target_eye_height = component.standing_eye_height;
    let delta = resize_capsule(component, target_half_height, target_eye_height, anchor);
    position.y += delta;
    delta
}

/// The `Crouching` → `Normal` transition: `KEEP_ALL` carry. Crouch preserves
/// momentum across the edge — it is a capsule resize, not a velocity reset — so
/// the dispatch-applied carry is the §6 parity no-op.
pub(super) fn stand_up_transition() -> Transition {
    Transition {
        next: MovementState::Normal,
        carry: CarryRule::KEEP_ALL,
    }
}

/// The outgoing horizontal boost vector a state carries as it leaves (the D4
/// additive layer). `Dash` carries its live `boost`; `Normal` carries none, so
/// `keepBoost`/drop operate on a zero boost — a no-op. Read at the transition
/// edge so the carry sees the boost as it leaves, before the new state is written.
fn outgoing_boost(state: &MovementState) -> Vec3 {
    match state {
        MovementState::Normal => Vec3::ZERO,
        MovementState::Dash { boost, .. } => *boost,
        // `Crouching` carries no boost layer — it is a resize, not a velocity
        // impulse. `keepBoost`/drop operate on a zero boost (a no-op), matching
        // `Normal`.
        MovementState::Crouching { .. } => Vec3::ZERO,
    }
}

/// Apply a transition's carry-rule to `component.velocity` at the transition
/// edge — the DISPATCH-layer velocity transform (D6), never inside a state
/// intent. The outgoing velocity decomposes into the D4 layers: a horizontal
/// base (`velocity - outgoing_boost`, vertical kept) and the outgoing `boost`.
/// The `horizontal` rule transforms the base layer; the `boost` rule transforms
/// the boost layer; the two recombine into the new resolved velocity.
///
/// `KEEP_ALL` is exactly a no-op: the base is kept and the kept boost is added
/// back, reconstructing the outgoing velocity unchanged — the `Normal`↔`Dash`
/// parity guarantee. Vertical velocity is preserved throughout (the carry
/// vocabulary governs horizontal momentum only; the boost layer is horizontal).
fn apply_carry(rule: CarryRule, velocity: &mut Vec3, boost: Vec3) {
    let mut base = *velocity - boost;
    apply_horizontal(rule.horizontal, &mut base);
    let carried_boost = apply_boost(rule.boost, boost);
    *velocity = base + carried_boost;
}

/// Run the active state's velocity intent, resolving the
/// component-vs-active-state borrow ONCE so each state owns its live data in
/// place behind one uniform convention.
///
/// The active state is moved out of the component (replaced with the `Normal`
/// placeholder) so the matched arm holds a `&mut` to the state's own live data
/// while the intent also borrows `&mut component`. After the intent runs, the
/// (in-place-mutated) state is written back.
///
/// The CARRY STEP runs here, at the transition edge — NOT in `tick`: when an
/// intent returns a `Transition`, its carry-rule is applied to
/// `component.velocity` reading the OUTGOING state's live boost (the `Dash`
/// boost) BEFORE the outgoing `state` local is dropped — so the carry sees the
/// boost as it leaves. The returned `Option<MovementState>` is ONLY the next
/// state for `tick` to write after the substrate resolves collision; by the time
/// `tick` sees it, carry is already committed. Searching `tick` for carry
/// application will come up empty by design — it lives here. Velocity carry is
/// owned by this dispatch point, never by a state intent (D6).
///
/// `jump_edges` are the forgiveness-derived jump edges `tick` computed once
/// before dispatch; both `normal_intent` and `crouching_intent` consume them
/// (the `Dash` state drops jump input by design).
///
/// Adding a new state means adding one arm here with its own `&mut` live data;
/// `tick`'s structure and existing intent signatures are untouched.
// Threads the substrate handles (`collision_world`, the mutable `position`) the
// `Crouching` intent needs for its stand-up probe and resize; grouping them into
// a struct would add an abstraction with one caller and no reuse.
#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_state_intent(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    jump_edges: JumpEdges,
    gravity: f32,
    dt: f32,
    collision_world: &CollisionWorld,
    position: &mut Vec3,
    events: &mut MovementEvents,
) -> Option<MovementState> {
    // Resolve the borrow once: move the live state into a local so its data can
    // be borrowed `&mut` independently of `&mut component`. The `Normal`
    // placeholder left behind is overwritten below (write-back or transition).
    let mut state = std::mem::replace(&mut component.movement_state, MovementState::Normal);
    let transition = match &mut state {
        MovementState::Normal => {
            normal_intent(component, input, jump_edges, gravity, dt, position, events)
        }
        MovementState::Dash { elapsed_ms, boost } => {
            dash_intent(component, input, gravity, dt, elapsed_ms, boost)
        }
        MovementState::Crouching { eye_current } => crouching_intent(
            component,
            input,
            jump_edges,
            gravity,
            dt,
            collision_world,
            position,
            events,
            eye_current,
        ),
    };
    match transition {
        Some(Transition { next, carry }) => {
            // Carry step at the transition edge: apply the rule to the resolved
            // OUTGOING velocity, reading the outgoing state's live boost before
            // `state` is dropped. The new state is then handed to `tick`.
            apply_carry(carry, &mut component.velocity, outgoing_boost(&state));
            Some(next)
        }
        None => {
            // No transition: write the (mutated) live state back so an unchanged
            // state's in-place data updates (the `Dash` boost/elapsed) carry
            // forward.
            component.movement_state = state;
            None
        }
    }
}
