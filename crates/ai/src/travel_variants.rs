// Directional travel clips: under target facing, activity animation `X` plays
// as `X_left`, `X_right` or `X_backward` by travel relative to facing.
// See: context/lib/entity_model.md §7c (Engaged motion and facing)

use glam::Vec3;
use postretro_entities::components::brain::TravelVariant;
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::FacingPolicy;

use super::LocomotionIntent;

/// Travel within this angle of facing plays the base clip.
const FORWARD_SECTOR_RADIANS: f32 = std::f32::consts::FRAC_PI_4;
/// Travel at least this far from facing plays the backward clip; between the
/// two sectors plays the side clip.
const BACKWARD_SECTOR_RADIANS: f32 = 3.0 * std::f32::consts::FRAC_PI_4;
/// How far past its sector edge travel must swing before a side or backward
/// clip on screen gives way. Each switch restarts the clip under a crossfade,
/// and a strafe can run near an edge: the limitator's runs about 48.5°.
const HOLD_BAND_RADIANS: f32 = 10.0_f32.to_radians();

/// The variant this tick's travel asks for. Only a target-facing mover whose
/// steering moves reads its travel against facing: forward within 45°,
/// backward from 135°, and the side between, left being counter-clockwise
/// seen from above (+yaw), the mesh's own left. Travel facing plays `X`.
///
/// `presented` is the variant on screen. A side or backward variant holds
/// until travel leaves its sector widened by [`HOLD_BAND_RADIANS`]. The base
/// clip holds nothing: every stop, travel facing and missing clip presents it,
/// so leaving it reads the plain sectors.
pub(super) fn classify(
    policy: Option<FacingPolicy>,
    steering_velocity: Option<Vec3>,
    facing_yaw: f32,
    presented: TravelVariant,
) -> TravelVariant {
    let Some(velocity) = steering_velocity.filter(|velocity| {
        policy == Some(FacingPolicy::Target) && LocomotionIntent::from_velocity(*velocity).moving
    }) else {
        return TravelVariant::Base;
    };
    let travel_yaw = velocity.x.atan2(velocity.z);
    let relative = (travel_yaw - facing_yaw + std::f32::consts::PI)
        .rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    variant_at(relative, presented)
}

/// The variant travel `relative` radians from facing (counter-clockwise
/// positive) presents while `presented` is on screen.
fn variant_at(relative: f32, presented: TravelVariant) -> TravelVariant {
    if !relative.is_finite() {
        return TravelVariant::Base;
    }
    let wanted = sector(relative);
    if wanted != presented && holds(presented, relative) {
        presented
    } else {
        wanted
    }
}

/// The plain sector `relative` falls in: forward within 45° inclusive,
/// backward from 135° inclusive, the side between.
fn sector(relative: f32) -> TravelVariant {
    let off = relative.abs();
    if off <= FORWARD_SECTOR_RADIANS {
        TravelVariant::Base
    } else if off >= BACKWARD_SECTOR_RADIANS {
        TravelVariant::Backward
    } else if relative > 0.0 {
        TravelVariant::Left
    } else {
        TravelVariant::Right
    }
}

/// Whether `presented` still holds at `relative`: travel lies within its
/// sector widened by [`HOLD_BAND_RADIANS`] at each edge.
fn holds(presented: TravelVariant, relative: f32) -> bool {
    let side = |off: f32| {
        off > FORWARD_SECTOR_RADIANS - HOLD_BAND_RADIANS
            && off < BACKWARD_SECTOR_RADIANS + HOLD_BAND_RADIANS
    };
    match presented {
        TravelVariant::Base => false,
        TravelVariant::Left => side(relative),
        TravelVariant::Right => side(-relative),
        TravelVariant::Backward => relative.abs() >= BACKWARD_SECTOR_RADIANS - HOLD_BAND_RADIANS,
    }
}

/// The variant the mesh can play: `wanted` when it declares a usable
/// `{base}{suffix}` state, else the base clip. A missing variant is ordinary
/// content, so it never warns.
pub(super) fn presentable(
    registry: &EntityRegistry,
    id: EntityId,
    base: Option<&str>,
    wanted: TravelVariant,
) -> TravelVariant {
    let Some(base) = base.filter(|_| wanted != TravelVariant::Base) else {
        return TravelVariant::Base;
    };
    let suffix = wanted.suffix();
    // Compare the name piecewise rather than composing it: this runs every
    // tick a target-facing mover travels, and the composed name is needed
    // only on the switch edge.
    let usable = registry
        .get_component::<MeshComponent>(id)
        .ok()
        .and_then(|mesh| mesh.animation.as_ref())
        .is_some_and(|animation| {
            animation.states.iter().any(|(name, state)| {
                state.clip_index.is_some()
                    && name.len() == base.len() + suffix.len()
                    && name.starts_with(base)
                    && name.ends_with(suffix)
            })
        });
    if usable { wanted } else { TravelVariant::Base }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEG: f32 = std::f32::consts::PI / 180.0;

    /// Travel `degrees` counter-clockwise from a facing yaw of zero (`+Z`).
    fn travel(degrees: f32) -> Vec3 {
        let yaw = degrees * DEG;
        Vec3::new(yaw.sin(), 0.0, yaw.cos()) * 3.0
    }

    #[test]
    fn exact_sector_edges_classify_as_the_plan_states_from_base() {
        let base = TravelVariant::Base;
        assert_eq!(
            variant_at(FORWARD_SECTOR_RADIANS, base),
            TravelVariant::Base
        );
        assert_eq!(
            variant_at(-FORWARD_SECTOR_RADIANS, base),
            TravelVariant::Base
        );
        assert_eq!(
            variant_at(BACKWARD_SECTOR_RADIANS, base),
            TravelVariant::Backward
        );
        assert_eq!(
            variant_at(-BACKWARD_SECTOR_RADIANS, base),
            TravelVariant::Backward
        );
        assert_eq!(variant_at(46.0 * DEG, base), TravelVariant::Left);
        assert_eq!(variant_at(-46.0 * DEG, base), TravelVariant::Right);
        assert_eq!(variant_at(134.0 * DEG, base), TravelVariant::Left);
    }

    #[test]
    fn travel_oscillating_inside_the_band_keeps_the_presented_variant() {
        // Around the 45° edge, as a strafe running near it does.
        let mut presented = variant_at(48.5 * DEG, TravelVariant::Base);
        assert_eq!(presented, TravelVariant::Left);
        for degrees in [41.0, 48.5, 36.0, 54.0, 40.0, 48.5] {
            presented = variant_at(degrees * DEG, presented);
            assert_eq!(
                presented,
                TravelVariant::Left,
                "{degrees}° keeps the side clip"
            );
        }
        // Around the 135° edge, from either side.
        for (start, angles) in [
            (TravelVariant::Left, [140.0, 130.0, 144.0]),
            (TravelVariant::Backward, [130.0, 140.0, 126.0]),
            (TravelVariant::Right, [-140.0, -130.0, -144.0]),
        ] {
            for degrees in angles {
                assert_eq!(
                    variant_at(degrees * DEG, start),
                    start,
                    "{degrees}° holds {start:?}"
                );
            }
        }
    }

    #[test]
    fn travel_past_the_band_leaves_the_presented_variant() {
        assert_eq!(
            variant_at(34.0 * DEG, TravelVariant::Left),
            TravelVariant::Base
        );
        assert_eq!(
            variant_at(146.0 * DEG, TravelVariant::Left),
            TravelVariant::Backward
        );
        assert_eq!(
            variant_at(124.0 * DEG, TravelVariant::Backward),
            TravelVariant::Left
        );
        assert_eq!(
            variant_at(-124.0 * DEG, TravelVariant::Backward),
            TravelVariant::Right
        );
        // Left and right never border each other, so no band joins them.
        assert_eq!(
            variant_at(-60.0 * DEG, TravelVariant::Left),
            TravelVariant::Right
        );
    }

    #[test]
    fn only_target_facing_travel_reads_or_holds_a_variant() {
        let sideways = Some(travel(90.0));
        assert_eq!(
            classify(
                Some(FacingPolicy::Target),
                sideways,
                0.0,
                TravelVariant::Base
            ),
            TravelVariant::Left
        );
        for (policy, velocity) in [
            (Some(FacingPolicy::Travel), sideways),
            (None, sideways),
            (Some(FacingPolicy::Target), Some(Vec3::ZERO)),
            (Some(FacingPolicy::Target), None),
        ] {
            assert_eq!(
                classify(policy, velocity, 0.0, TravelVariant::Left),
                TravelVariant::Base,
                "{policy:?} at {velocity:?} drops the held variant"
            );
        }
        assert_eq!(
            classify(
                Some(FacingPolicy::Target),
                sideways,
                f32::NAN,
                TravelVariant::Left
            ),
            TravelVariant::Base
        );
    }
}
