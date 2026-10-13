// Vault entry at the pre-intent stage, and the mantle path the vault intent follows.
// See: context/lib/movement.md §4

use glam::{Quat, Vec3};

use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::MovementInput;
use crate::movement::ledge_probe::{ReachBand, probe_ledge};
use crate::movement::mover_carry::apply_mover_carry;
use crate::movement::substrate::{step_up_limit, wish_dir_from_input};
use postretro_collision::ResolvedEnvironment;
use postretro_foundation::{
    GroundRef, MovementState, PlayerMovementComponent, VaultParams, VaultTrigger,
};

/// Least `y` of a ledge mover's up axis, in world space, at which a vault onto
/// it is allowed: about 0.8° of tilt. The path splits rise from forward along
/// the mover's own axes, but the probe cleared a world-vertical rise and a
/// world-horizontal move, so on a tilted mover the path would leave the cleared
/// corridor (a 10° roll passes it about 4 cm under the lip), and its local
/// horizontal offset would no longer be the world one the wire contract checks.
/// Yaw-only movers keep their up axis vertical and vault as static ledges do.
const UPRIGHT_MIN_UP_Y: f32 = 0.9999;

/// The vault state this tick's pre-intent stage enters, if any: an enabled
/// context, a trigger that fires, horizontal wish, and a ledge the probe finds.
/// Any entry clears the jump buffer, so a jump pressed before the vault never
/// fires on its exit landing; a jump trigger also spends the jump, as a jump
/// would. A pawn the caller barred (`vault_barred`: dead) never enters.
///
/// Contexts: airborne or grounded `Normal` and `Dash` read `from.air` or
/// `from.ground`; `Swim` reads `from.swim` once the tick eye is out of the
/// fluid. Crouch and slide never vault. Land reach is measured above the feet,
/// past the step-up limit; fluid reach above the fluid surface. A ledge on a
/// tilted mover is refused ([`UPRIGHT_MIN_UP_Y`]).
///
/// Called while `velocity` holds the base alone, the protected layers peeled.
/// `entry_speed` measures what the pawn was really moving at: the horizontal
/// speed of base plus knockback plus drift. Dispatch then empties those layers.
/// The start offset is taken where mover carry (`carry_ground`) puts the pawn
/// this tick, so the path starts where the pawn really is in the target's frame.
pub(super) fn vault_entry(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    resolved: &ResolvedEnvironment,
    collision: &CombinedCollisionWorld<'_>,
    position: Vec3,
    carry_ground: GroundRef,
    dt: f32,
) -> Option<MovementState> {
    if component.vault_barred {
        return None;
    }
    let vault = component.vault.as_ref()?;
    let swimming = matches!(component.movement_state, MovementState::Swim { .. });
    let trigger = match component.movement_state {
        MovementState::Normal | MovementState::Dash { .. } => {
            if component.is_grounded() {
                vault.from.ground
            } else {
                vault.from.air
            }
        }
        MovementState::Swim { .. } if !resolved.tick_eye_in_fluid => vault.from.swim,
        _ => None,
    }?;
    let wish = wish_dir_from_input(input.wish_dir, input.facing_yaw);
    if wish == Vec3::ZERO {
        return None;
    }
    let jump_ready = input.jump_pressed || component.jump_buffer_timer_ms > 0.0;
    if trigger == VaultTrigger::Jump && !jump_ready {
        return None;
    }

    let feet_offset = component.capsule.half_height + component.capsule.radius;
    let feet_y = position.y - feet_offset;
    let band = reach_band(component, vault, swimming, resolved, feet_y);
    let moving = component.velocity + component.knockback_velocity + component.push_velocity;
    let travel = moving.dot(wish) * dt;
    let ledge = probe_ledge(component, collision, position, wish, travel, band)?;

    let entry_feet = apply_mover_carry(position, carry_ground, collision) - Vec3::Y * feet_offset;
    let entry_speed = Vec3::new(moving.x, 0.0, moving.z).length();
    let frame = VaultFrame::resolve(ledge.mover, collision)?;
    if !frame.is_upright() {
        return None;
    }
    let target = frame.to_local(ledge.target_feet);
    let start_offset = frame.to_local(entry_feet) - target;

    component.jump_buffer_timer_ms = 0.0;
    if trigger == VaultTrigger::Jump {
        component.jump_spent = true;
    }
    // Leaving water by a mantle latches swim off, as a fluid jump does, so swim
    // cannot re-capture the rise before the pawn is clear of the water.
    if swimming {
        component.swim_reentry_latched = true;
    }
    Some(MovementState::Vault {
        elapsed_ms: 0.0,
        target,
        start_offset,
        mover: ledge.mover,
        entry_speed,
    })
}

fn reach_band(
    component: &PlayerMovementComponent,
    vault: &VaultParams,
    swimming: bool,
    resolved: &ResolvedEnvironment,
    feet_y: f32,
) -> ReachBand {
    if swimming {
        let segment = 2.0 * (component.capsule.half_height + component.capsule.radius);
        let surface_y = feet_y + resolved.immersion * segment;
        ReachBand {
            min_top_y: surface_y,
            max_top_y: surface_y + vault.swim_max_height,
        }
    } else {
        ReachBand {
            min_top_y: feet_y + step_up_limit(component.ground_params.step_height),
            max_top_y: feet_y + vault.max_height,
        }
    }
}

/// The frame a vault's target and start offset live in: the carrying mover's
/// pose this tick, or the world.
#[derive(Debug, Clone, Copy)]
pub(super) struct VaultFrame {
    origin: Vec3,
    rotation: Quat,
}

impl VaultFrame {
    /// `None` when the mover has no pose this tick (it despawned).
    pub(super) fn resolve(
        mover: Option<u32>,
        collision: &CombinedCollisionWorld<'_>,
    ) -> Option<Self> {
        match mover {
            None => Some(Self {
                origin: Vec3::ZERO,
                rotation: Quat::IDENTITY,
            }),
            Some(id) => collision.poses.pose(id).map(|pose| Self {
                origin: pose.transform.position,
                rotation: pose.transform.rotation.normalize(),
            }),
        }
    }

    /// Whether the frame's up axis is vertical within [`UPRIGHT_MIN_UP_Y`].
    fn is_upright(self) -> bool {
        (self.rotation * Vec3::Y).y >= UPRIGHT_MIN_UP_Y
    }

    fn to_local(self, world: Vec3) -> Vec3 {
        self.rotation.inverse() * (world - self.origin)
    }

    pub(super) fn to_world(self, local: Vec3) -> Vec3 {
        self.origin + self.rotation * local
    }

    pub(super) fn rotate(self, local: Vec3) -> Vec3 {
        self.rotation * local
    }
}
