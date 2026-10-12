// Push volumes in player movement: lift-off, the protected drift layer, its medium decay and terminal speed along the push.
// See: context/plans/in-progress/E24--water-and-environment-volumes/plan.md (Owner ruling 2026-10-11)

use glam::{Vec2, Vec3};

use postretro_collision::ResolvedEnvironment;
use postretro_foundation::{MovementState, PlayerMovementComponent};

use super::intents::GROUND_STOP_FRICTION;

/// Below this squared speed a decaying drift stops outright, as damped knockback
/// does, so a landed drift reaches exactly zero instead of trailing denormals.
const DRIFT_STOP_SPEED_SQ: f32 = 1.0e-6;

/// Lift-off: a grounded body leaves the ground when the push points up
/// harder than gravity pulls down. Only gravity's downward part resists, so
/// upward gravity never holds a body down; with no push this is always false,
/// and upward gravity alone never lifts a grounded body. `tick` treats a true
/// result like a knockback launch: it clears grounded before the intent and
/// tells the substrate to skip the upward clamp and ground-stick.
///
/// Never for a pawn swimming at tick start (`tick` asks before the swim seam,
/// since the result gates this tick's jump edges): what holds a swimmer down is
/// its sink and the current, not one tick of gravity, so its one lift-off rule
/// is [`release_rising_swimmer`], which reads the total vertical motion after
/// the drift accrues.
pub(super) fn lifts_off(
    component: &PlayerMovementComponent,
    environment: &ResolvedEnvironment,
) -> bool {
    let push_y = environment.push.y;
    !is_swimming(component) && push_y > 0.0 && push_y + environment.gravity.min(0.0) > 0.0
}

/// After the intent. Called while `velocity` holds locomotion only. The medium
/// first decays the drift built up so far, then the tick's push accrues:
/// - **Swimming:** the whole push, all three axes, accrues in the protected
///   drift layer (`push_velocity`), so a current settles at push / viscosity in
///   every direction (never past terminal speed, [`cap_swim_drift`]) and swim
///   steering never fights it. A swimmer resting on the
///   pool floor takes vertical push too: the layer rejoins before collision,
///   which projects its horizontal part like knockback and leaves its vertical
///   part to the fluid ([`project_drift`]), so a downward current presses into
///   the floor and an upward one lifts off it ([`release_rising_swimmer`]).
/// - **Otherwise:** horizontal push accrues in the drift layer, which intent caps
///   and steering never see. Vertical push joins locomotion directly; no intent
///   caps vertical speed, and gravity, lift-off and the terminal cap govern it.
///   While the body stays grounded the floor bears the vertical part, exactly as
///   every intent gates gravity on airborne; a push strong enough to lift has
///   already cleared grounded through [`lifts_off`].
pub(super) fn apply(
    component: &mut PlayerMovementComponent,
    environment: &ResolvedEnvironment,
    dt: f32,
) {
    let push = environment.push;
    decay_drift(component, environment.viscosity, dt);
    if is_swimming(component) {
        component.push_velocity += push * dt;
        cap_swim_drift(component);
        release_rising_swimmer(component);
        return;
    }
    component.push_velocity += Vec3::new(push.x, 0.0, push.z) * dt;
    if !component.is_grounded() {
        component.velocity.y += push.y * dt;
    }
}

/// While swimming, cap the drift layer's speed at the descriptor's fall
/// terminal velocity — by magnitude, so its part along the push (or any
/// direction) is bounded too. Runs every swimming tick, push or none. The
/// total-velocity caps cannot bound the layer: a floor or ceiling never absorbs
/// its vertical part (`project_drift`), so while the contact lasts locomotion
/// holds the opposite and the total stays near zero; at low viscosity a
/// vertical current would accrue without limit, then spill out once the
/// swimmer leaves the contact. A mid-water current settling below terminal
/// speed is untouched.
fn cap_swim_drift(component: &mut PlayerMovementComponent) {
    let terminal = component.fall.terminal_velocity;
    component.push_velocity = component.push_velocity.clamp_length_max(terminal);
}

/// A swimmer resting on the pool floor leaves it once its total vertical
/// motion points up: locomotion (swimming up), knockback and the drift layer
/// (an upward current) together. It runs after the drift accrues, which is the
/// first point the tick knows the total; `tick` then reads the cleared ground
/// as a launch and tells the substrate neither to clamp the rise nor to stick
/// the swimmer back down. A downward current outweighing an upward swim keeps
/// it on the floor.
fn release_rising_swimmer(component: &mut PlayerMovementComponent) {
    let rising = component.velocity.y + component.knockback_velocity.y + component.push_velocity.y;
    if component.is_grounded() && rising > 0.0 {
        component.set_grounded(false);
    }
}

/// Leaving `Swim`: the drift layer's vertical part joins locomotion
/// `velocity.y` and the layer is horizontal again, so outside water vertical
/// push stays in locomotion. Dispatch calls it on every edge out of `Swim`,
/// while `velocity` holds locomotion only, so total velocity is unchanged.
pub(super) fn fold_vertical_drift(component: &mut PlayerMovementComponent) {
    component.velocity.y += component.push_velocity.y;
    component.push_velocity.y = 0.0;
}

fn is_swimming(component: &PlayerMovementComponent) -> bool {
    matches!(component.movement_state, MovementState::Swim { .. })
}

/// The medium a body's drift decays in this tick.
enum DriftMedium {
    /// Swimming: the fluid's viscosity drags the drift, so a current settles
    /// near push / viscosity. A swimmer touching the pool floor still counts
    /// as swimming here: the fluid, not floor friction, is the medium.
    Fluid { viscosity: f32 },
    /// Grounded: friction bleeds the drift.
    Ground,
    /// Airborne: nothing resists, so drift carries out of a wind until landing.
    Air,
}

/// Read after the intent, so a jump or a launch this tick counts as airborne,
/// and a fluid jump this tick (which left swim) counts as airborne too.
/// `viscosity` is the tick's resolved fluid drag.
fn drift_medium(component: &PlayerMovementComponent, viscosity: f32) -> DriftMedium {
    if is_swimming(component) {
        DriftMedium::Fluid { viscosity }
    } else if component.is_grounded() {
        DriftMedium::Ground
    } else {
        DriftMedium::Air
    }
}

/// The medium's decay of the drift layer, applied every tick before the tick's
/// push accrues. Ground friction is the intents' stop-friction model on
/// locomotion, applied whether or not input is held because drift is never
/// voluntary; with a steady push the drift settles at push /
/// `GROUND_STOP_FRICTION`. A fluid removes the same linear fraction per tick,
/// on all three axes, as the swim intent's drag on locomotion
/// (`max(0, 1 - viscosity * dt)`).
fn decay_drift(component: &mut PlayerMovementComponent, viscosity: f32, dt: f32) {
    let medium = drift_medium(component, viscosity);
    let drift = &mut component.push_velocity;
    match medium {
        DriftMedium::Fluid { viscosity } => {
            *drift *= (1.0 - viscosity * dt).max(0.0);
            if drift.length_squared() < DRIFT_STOP_SPEED_SQ {
                *drift = Vec3::ZERO;
            }
        }
        DriftMedium::Ground => {
            let speed = Vec2::new(drift.x, drift.z).length();
            if speed > 0.0 {
                let drop = speed * GROUND_STOP_FRICTION * dt;
                let scale = (speed - drop).max(0.0) / speed;
                drift.x *= scale;
                drift.z *= scale;
            }
            if drift.length_squared() < DRIFT_STOP_SPEED_SQ {
                *drift = Vec3::ZERO;
            }
        }
        DriftMedium::Air => {}
    }
}

/// Project the drift layer against a contact normal exactly as collision
/// projects total velocity and knockback, so a wall absorbs the drift into it.
///
/// Outside `Swim` (`three_d` false) the layer stays horizontal: any vertical
/// part the projection produces stays in total velocity, where the next tick
/// reads it as locomotion.
///
/// While swimming the layer's vertical part is the fluid's, not the contact's:
/// a floor or ceiling never consumes it. The contact still stops total
/// velocity, so locomotion absorbs the difference, and the current builds
/// toward its settled push / viscosity against the floor as in open water
/// (bounded by `cap_swim_drift`). That keeps lift-off tied to the current, not
/// the tick: a swimmer resting on the pool floor rises once its total vertical
/// motion, current included, points up ([`release_rising_swimmer`]), so a
/// current strong enough to outrun the swimmer's sink lifts it as the drift
/// builds, rather than only when one tick's push outweighs one tick's sink
/// acceleration, which is what consuming the layer on every floor contact would
/// test.
pub(super) fn project_drift(drift: &mut Vec3, normal: Vec3, three_d: bool) {
    let vertical = drift.y;
    *drift -= normal * drift.dot(normal);
    drift.y = if three_d { vertical } else { 0.0 };
}

/// Terminal speed: cap the total speed along the push direction at the
/// descriptor's fall terminal velocity, so an updraft or a wind does not
/// accelerate a body forever. Only the component along the push is touched.
/// Runs while `velocity` holds only locomotion. The excess splits into its
/// horizontal and vertical parts, each taken from the protected layers first —
/// the drift layer, then knockback, then locomotion — as
/// `knockback::clamp_fall_speed` does, so a decaying layer never leaves
/// locomotion pushing against the push. Outside `Swim` the drift layer has no
/// vertical part, so vertical excess comes from knockback, then locomotion.
pub(super) fn clamp_speed_along_push(component: &mut PlayerMovementComponent, push: Vec3) {
    let Some(direction) = push.try_normalize() else {
        return;
    };
    let total = component.velocity + component.knockback_velocity + component.push_velocity;
    let excess = (total.dot(direction) - component.fall.terminal_velocity).max(0.0);
    if excess == 0.0 {
        return;
    }
    let excess = direction * excess;

    for part in [
        Vec3::new(excess.x, 0.0, excess.z),
        Vec3::new(0.0, excess.y, 0.0),
    ] {
        let Some(along) = part.try_normalize() else {
            continue;
        };
        let mut remaining = part.length();
        for layer in [
            &mut component.push_velocity,
            &mut component.knockback_velocity,
        ] {
            let cut = remaining.min(layer.dot(along).max(0.0));
            *layer -= along * cut;
            remaining -= cut;
        }
        component.velocity -= along * remaining;
    }
}
