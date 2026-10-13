// Vault entry at the pre-intent stage, and the mantle path the vault intent follows.
// See: context/lib/movement.md §4

use glam::{Quat, Vec3};

use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::MovementInput;
use crate::movement::ledge_probe::{ReachBand, probe_ledge};
use crate::movement::substrate::{step_up_limit, wish_dir_from_input};
use postretro_collision::ResolvedEnvironment;
use postretro_foundation::{MovementState, PlayerMovementComponent, VaultParams, VaultTrigger};

/// Share of the vault spent rising; the rest moves forward over the lip. The
/// pawn rises fully before it moves forward, along the path the probe cleared.
const RISE_SHARE: f32 = 0.6;

/// The vault state this tick's pre-intent stage enters, if any: an enabled
/// context, a trigger that fires, horizontal wish, and a ledge the probe finds.
/// A jump trigger consumes the jump buffer and spends the jump, as a jump would.
///
/// Contexts: airborne or grounded `Normal` and `Dash` read `from.air` or
/// `from.ground`; `Swim` reads `from.swim` once the tick eye is out of the
/// fluid. Crouch and slide never vault. Land reach is measured above the feet,
/// past the step-up limit; fluid reach above the fluid surface.
pub(super) fn vault_entry(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    resolved: &ResolvedEnvironment,
    collision: &CombinedCollisionWorld<'_>,
    position: Vec3,
    dt: f32,
) -> Option<MovementState> {
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
    let travel = component.velocity.dot(wish) * dt;
    let ledge = probe_ledge(component, collision, position, wish, travel, band)?;

    let entry_feet = position - Vec3::Y * feet_offset;
    let entry_speed = Vec3::new(component.velocity.x, 0.0, component.velocity.z).length();
    let frame = VaultFrame::resolve(ledge.mover, collision)?;
    let target = frame.to_local(ledge.target_feet);
    let start_offset = frame.to_local(entry_feet) - target;

    if trigger == VaultTrigger::Jump {
        component.jump_buffer_timer_ms = 0.0;
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

/// Feet position, in the vault's frame, at progress `s` (0..=1): the vertical
/// offset closes over the rise share, then the horizontal offset over the rest,
/// each on a smoothstep.
pub(super) fn path_feet(target: Vec3, start_offset: Vec3, s: f32) -> Vec3 {
    let rise = smoothstep(0.0, RISE_SHARE, s);
    let forward = smoothstep(RISE_SHARE, 1.0, s);
    let vertical = Vec3::new(0.0, start_offset.y, 0.0);
    let horizontal = Vec3::new(start_offset.x, 0.0, start_offset.z);
    target + vertical * (1.0 - rise) + horizontal * (1.0 - forward)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
