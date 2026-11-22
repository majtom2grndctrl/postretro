// Connected-client mover correction glide: how fast a smoothed correction
// converges, advanced once per frame.
// See: context/lib/networking.md §Snapshot apply ordering

use glam::Vec3;
use postretro_entities::presentation_correction::PoseCorrection;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{GroundRef, PlayerMovementComponent};

use crate::MoverCorrection;
use crate::collision::moving::MoverCollider;
use crate::prediction::TELEPORT_CORRECTION_MIN_M;

/// Time constant of a mover correction's glide, in seconds. Faster than the
/// pawn offset's glide (0.7 per 60 Hz tick, about 47 ms), so a deck settles
/// under a rider before the rider's own residual does.
pub(crate) const MOVER_GLIDE_TAU_SECS: f32 = 0.025;

/// A glide within this many metres and radians is done.
pub(crate) const MOVER_GLIDE_SETTLED: f32 = 1.0e-5;

/// Advance every mover correction glide by `frame_dt` seconds of presented
/// time, at a rate independent of frame rate. Call once per frame, after the
/// fixed ticks and before any presentation consumer reads a mover's pose. A
/// glide seeded this frame is drawn whole first. No-op without glides.
pub fn advance_mover_correction_glides(registry: &mut EntityRegistry, frame_dt: f32) {
    if !registry.has_presentation_corrections() {
        return;
    }
    let remaining = if frame_dt.is_finite() && frame_dt > 0.0 {
        (-frame_dt / MOVER_GLIDE_TAU_SECS).exp()
    } else {
        1.0
    };
    registry.advance_presentation_corrections(remaining, MOVER_GLIDE_SETTLED);
}

/// Snap every mover correction this frame's apply seeded whose glide would
/// move any point of the mover by the pawn reconcile's teleport-class floor:
/// the deck lands whole on the corrected pose instead. Judged on the composed
/// glide, so several corrections in one frame snap together, and on the
/// mover's reach about its origin, so a rotation-only correction of a large
/// mover snaps too (pin P7). A mover with no collider is judged by its origin.
/// Returns the snapped movers' ids; allocates only when one snaps.
pub fn settle_mover_correction_snaps(
    registry: &mut EntityRegistry,
    corrections: &[MoverCorrection],
    colliders: &[MoverCollider],
) -> Vec<u32> {
    let mut snapped = Vec::new();
    for correction in corrections {
        let mover = correction.authoritative_state.entity;
        let Some(glide) = registry.presentation_correction(mover) else {
            continue;
        };
        let reach = colliders
            .iter()
            .find(|collider| collider.mover_id() == correction.mover_id)
            .map_or(0.0, MoverCollider::reach);
        if glide_moves_mover_by(&glide, reach) >= TELEPORT_CORRECTION_MIN_M {
            registry.clear_presentation_correction(mover);
            snapped.push(correction.mover_id);
        }
    }
    snapped
}

/// The local pawn's position and ground just before a reconcile, for
/// [`exclude_mover_share_from_offset`].
pub(crate) fn pawn_before_reconcile(
    registry: &EntityRegistry,
    pawn: EntityId,
) -> Option<(Vec3, GroundRef)> {
    let position = registry.get_component::<Transform>(pawn).ok()?.position;
    let ground = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .ok()?
        .ground;
    Some((position, ground))
}

/// Keep a mover correction's share of a standing pawn's reconcile out of the
/// world offset: the eye rides the drawn deck, which glides that share in
/// already, so counting it in the offset too would carry it twice. The share
/// is the correction's displacement of the pawn's pre-correction point, taken
/// when the pawn's ground named the corrected mover before the reconcile (the
/// deck its eye rides), and only while the reconcile left an offset in flight
/// (a cut clears it). A pawn the snapshot also moves off that deck keeps the
/// share out too: the eye's next push detaches and hands the deck's remaining
/// glide into the offset exactly once (pin P5).
pub(crate) fn exclude_mover_share_from_offset(
    prediction: &mut crate::prediction::ClientPrediction,
    before: Option<(Vec3, GroundRef)>,
    corrections: &[MoverCorrection],
) {
    let Some((point, GroundRef::Mover(mover_id))) = before else {
        return;
    };
    if prediction.presentation_offset() == Vec3::ZERO {
        return;
    }
    let share = corrections
        .iter()
        .filter(|correction| correction.mover_id == mover_id)
        .filter_map(|correction| mover_share_at(point, correction))
        .fold(Vec3::ZERO, |sum, share| sum + share);
    prediction.shift_presentation_offset(share);
}

/// How far `correction` carries a point fixed to its mover at `point`.
fn mover_share_at(point: Vec3, correction: &MoverCorrection) -> Option<Vec3> {
    let applied = correction.applied?;
    let origin = correction.pre_correction.position;
    Some(origin + applied.translation + applied.rotation * (point - origin) - point)
}

/// The farthest any point within `reach` of the mover origin moves under
/// `glide`: its translation plus the chord its rotation sweeps at that radius.
fn glide_moves_mover_by(glide: &PoseCorrection, reach: f32) -> f32 {
    glide.translation.length() + 2.0 * reach * (glide.angle() * 0.5).sin()
}

#[cfg(test)]
mod tests {
    use glam::{Quat, Vec3};
    use postretro_entities::{EntityRegistry, Transform};

    use super::advance_mover_correction_glides;

    fn glide_after(frame_hz: f32, elapsed_secs: f32) -> f32 {
        let mut registry = EntityRegistry::new();
        let mover = registry.spawn(Transform::default());
        registry
            .correct_transform(
                mover,
                Transform {
                    position: Vec3::new(0.16, 0.0, 0.0),
                    rotation: Quat::from_rotation_y(0.1),
                    ..Transform::default()
                },
                true,
            )
            .unwrap();
        let dt = 1.0 / frame_hz;
        // The correction frame draws it whole; each later frame shrinks it.
        advance_mover_correction_glides(&mut registry, dt);
        let frames = (elapsed_secs * frame_hz).round() as u32;
        for _ in 0..frames {
            advance_mover_correction_glides(&mut registry, dt);
        }
        registry
            .presentation_correction(mover)
            .map_or(0.0, |correction| correction.translation.length())
    }

    #[test]
    fn mover_glide_converges_in_the_same_presented_time_at_any_frame_rate() {
        for elapsed in [0.1, 0.2] {
            let at_60 = glide_after(60.0, elapsed);
            assert!(at_60 > 0.0);
            for hz in [30.0, 240.0] {
                let other = glide_after(hz, elapsed);
                assert!(
                    (other - at_60).abs() <= at_60 * 1.0e-4,
                    "{hz} Hz after {elapsed} s: {other} vs 60 Hz {at_60}"
                );
            }
        }
        // Converged and dropped well inside half a second.
        assert_eq!(glide_after(60.0, 0.5), 0.0);
    }
}

#[cfg(test)]
mod snap_tests {
    use glam::{Quat, Vec3};
    use postretro_entities::{EntityRegistry, Transform};

    use super::settle_mover_correction_snaps;
    use crate::MoverCorrection;
    use crate::collision::moving::MoverCollider;
    use crate::kinematic_mover::MoverTickState;
    use postretro_net::wire::NetworkId;

    const MOVER: u32 = 9;

    /// A flat deck reaching `half` metres from its origin along X and Z.
    fn deck(half: f32) -> MoverCollider {
        MoverCollider::from_local_triangles(
            MOVER,
            &[
                Vec3::new(-half, 0.0, -half),
                Vec3::new(half, 0.0, -half),
                Vec3::new(half, 0.0, half),
                Vec3::new(-half, 0.0, half),
            ],
            &[[0, 1, 2], [0, 2, 3]],
        )
        .unwrap()
    }

    /// Glide `mover` onto `corrected` and settle it against `colliders`;
    /// whether a glide is still in flight afterwards.
    fn glides(corrected: Transform, colliders: &[MoverCollider]) -> bool {
        let mut registry = EntityRegistry::new();
        let mover = registry.spawn(Transform::default());
        registry.correct_transform(mover, corrected, true).unwrap();
        let correction = MoverCorrection {
            pre_correction: Default::default(),
            applied: None,
            network_id: NetworkId(1),
            mover_id: MOVER,
            magnitude: corrected.position.length(),
            authoritative_state: MoverTickState {
                entity: mover,
                transform: corrected,
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::ZERO,
                tick_rotation_delta: Quat::IDENTITY,
                carry_yaw: false,
                tick_dt: 1.0 / 60.0,
            },
        };
        settle_mover_correction_snaps(&mut registry, &[correction], colliders);
        registry.presentation_correction(mover).is_some()
    }

    fn moved(x: f32) -> Transform {
        Transform {
            position: Vec3::new(x, 0.0, 0.0),
            ..Transform::default()
        }
    }

    fn turned(yaw: f32) -> Transform {
        Transform {
            rotation: Quat::from_rotation_y(yaw),
            ..Transform::default()
        }
    }

    #[test]
    fn mover_correction_glides_below_the_teleport_floor_and_snaps_at_it() {
        assert!(glides(moved(2.9), &[deck(1.0)]));
        assert!(!glides(moved(3.0), &[deck(1.0)]));
    }

    // Pin P7: a rotation-only correction that carries the deck's far corner by
    // the floor snaps, though the mover's origin does not move.
    #[test]
    fn rotation_only_correction_snaps_on_the_deck_it_moves() {
        // A 10 m-half deck reaches 14.1 m; 0.2 rad sweeps its corner 2.8 m.
        let large = [deck(10.0)];
        assert!(glides(turned(0.2), &large));
        assert!(!glides(turned(0.25), &large));
        // The same turn of a small deck glides.
        assert!(glides(turned(0.25), &[deck(1.0)]));
    }
}

#[cfg(test)]
mod zero_tick_step_tests {
    use glam::{Quat, Vec3};
    use postretro_entities::{EntityRegistry, Transform};

    const CORNER: Vec3 = Vec3::new(1.75, 0.0, 1.75);

    fn corner(transform: &Transform) -> Vec3 {
        transform.position + transform.rotation * CORNER
    }

    /// Worst drawn step at the deck corner when a 0.16 m, 0.05 rad correction
    /// lands on a frame that runs no fixed tick, over alphas.
    fn worst_step(smoothed: bool) -> f32 {
        let mut worst = 0.0_f32;
        for alpha in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let mut registry = EntityRegistry::new();
            let mover = registry.spawn(Transform::default());
            registry.snapshot_transforms();
            let current = Transform {
                position: Vec3::new(0.05, 0.0, 0.0),
                rotation: Quat::from_rotation_y(0.02),
                ..Transform::default()
            };
            registry.set_component(mover, current).unwrap();
            let before = registry.interpolated_transform(mover, alpha).unwrap();
            let corrected = Transform {
                position: current.position + Vec3::new(0.16, 0.0, 0.0),
                rotation: Quat::from_rotation_y(0.05) * current.rotation,
                ..current
            };
            if smoothed {
                registry.correct_transform(mover, corrected, true).unwrap();
            } else {
                registry.set_component(mover, corrected).unwrap();
            }
            let after = registry.interpolated_transform(mover, alpha).unwrap();
            worst = worst.max((corner(&after) - corner(&before)).length());
        }
        worst
    }

    // Pin P1: before smoothing, a correction on a zero-tick frame drew the
    // corrected current against the stale previous, stepping by alpha times it.
    #[test]
    fn zero_tick_mover_correction_step_before_and_after() {
        let (before, after) = (worst_step(false), worst_step(true));
        eprintln!(
            "[riders] synthetic zero-tick correction, worst drawn step: before {before:.4} m, \
             after {after:.6} m"
        );
        assert!(before > 0.1);
        assert!(after <= 1.0e-5, "{after}");
    }
}

#[cfg(test)]
mod share_tests {
    use glam::{Quat, Vec3};
    use postretro_entities::presentation_correction::PoseCorrection;
    use postretro_entities::{EntityRegistry, Transform};
    use postretro_foundation::{GroundRef, PlayerMovementComponent};
    use postretro_net::wire::NetworkId;

    use super::{exclude_mover_share_from_offset, pawn_before_reconcile};
    use crate::MoverCorrection;
    use crate::kinematic_mover::MoverTickState;
    use crate::prediction::ClientPrediction;

    const MOVER: u32 = 6;

    fn correction(
        applied: PoseCorrection,
        entity: postretro_entities::EntityId,
    ) -> MoverCorrection {
        MoverCorrection {
            pre_correction: Transform {
                position: Vec3::new(1.0, 0.0, 0.0),
                ..Transform::default()
            },
            applied: Some(applied),
            network_id: NetworkId(2),
            mover_id: MOVER,
            magnitude: applied.translation.length(),
            authoritative_state: MoverTickState {
                entity,
                transform: Transform::default(),
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::ZERO,
                tick_rotation_delta: Quat::IDENTITY,
                carry_yaw: false,
                tick_dt: 1.0 / 60.0,
            },
        }
    }

    /// The offset a reconcile leaves after excluding the mover's share, for a
    /// pawn at (3, 1, 0) whose ground goes from `before` to `after`.
    fn offset_after(before: GroundRef, after: GroundRef, mispredict: Vec3) -> (Vec3, Vec3) {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform {
            position: Vec3::new(3.0, 1.0, 0.0),
            ..Transform::default()
        });
        let mut movement: PlayerMovementComponent =
            crate::predict_reconcile_harness_test_fixtures::component();
        movement.ground = before;
        registry.set_component(pawn, movement.clone()).unwrap();
        let pawn_before = pawn_before_reconcile(&registry, pawn);
        let applied = PoseCorrection {
            translation: Vec3::new(0.0, 0.0, -0.1),
            rotation: Quat::from_rotation_y(0.05),
        };
        // The pawn's point carried by the mover correction about its origin.
        let point = Vec3::new(3.0, 1.0, 0.0);
        let origin = Vec3::new(1.0, 0.0, 0.0);
        let share = origin + applied.translation + applied.rotation * (point - origin) - point;
        // Reconcile seeds predicted minus reconciled: minus the share and the
        // mispredict.
        let mut prediction = ClientPrediction::default();
        prediction.shift_presentation_offset(-share - mispredict);
        movement.ground = after;
        registry.set_component(pawn, movement).unwrap();
        exclude_mover_share_from_offset(&mut prediction, pawn_before, &[correction(applied, pawn)]);
        (prediction.presentation_offset(), share)
    }

    // A standing pawn's correction reaches the eye once: the mover's share
    // through the drawn deck, the mispredict alone through the world offset.
    #[test]
    fn standing_pawn_keeps_only_the_mispredict_in_its_world_offset() {
        let mispredict = Vec3::new(0.01, 0.0, -0.02);
        let on = GroundRef::Mover(MOVER);
        let (offset, share) = offset_after(on, on, mispredict);
        assert!(share.length() > 0.1);
        assert!((offset + mispredict).length() < 1.0e-6, "{offset}");
    }

    // Pin P5: leaving the corrected deck in the same snapshot still keeps its
    // share out (the eye's detach hands the deck's remaining glide in once);
    // landing on it keeps the whole correction, which the eye's attach cancels.
    #[test]
    fn ground_change_excludes_the_share_only_from_the_deck_the_eye_rode() {
        let mispredict = Vec3::new(0.01, 0.0, -0.02);
        let on = GroundRef::Mover(MOVER);
        let (offset, _) = offset_after(on, GroundRef::World, mispredict);
        assert!((offset + mispredict).length() < 1.0e-6, "leaving: {offset}");
        let (offset, share) = offset_after(GroundRef::Airborne, on, mispredict);
        assert!(
            (offset + share + mispredict).length() < 1.0e-6,
            "landing: {offset}"
        );
    }
}
