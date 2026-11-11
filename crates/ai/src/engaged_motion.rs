// Engaged `strafe` and `reposition`: entry memory, the rotated combat ring
// each proposes to the slot batch, and the steering each takes from its claim.
// See: context/lib/entity_model.md §7c (Engaged motion and facing)

use glam::Vec3;
use postretro_entities::components::brain::{
    BrainComponent, EngagedMotionMemory, RepositionMemory, StrafeMemory,
};
use postretro_foundation::{MotionVerb, RepositionParams};

use super::EnemyOutcome;
use super::engine_floor::SteeringIntent;
use super::graph_eval::ResolvedMotion;
use super::targeting::TargetPawn;
use crate::combat_positioning::{
    CombatCandidate, KeepAway, RING_SLOT_COUNT, SlotRing, arc_slot_mask, slot_spacing,
};

/// The strafer's next slot counter-clockwise seen from above (+yaw), and
/// clockwise.
const CCW_NEIGHBOR: u8 = 1;
const CW_NEIGHBOR: u8 = RING_SLOT_COUNT - 1;
/// Ring index 0 lies on the ring's bearing: for `reposition`, straight in or
/// out along the entry bearing, which never qualifies.
const RADIAL_SLOT: u8 = 0;
/// Below this XZ offset there is no bearing: the enemy stands directly above
/// (or below) its target.
const MIN_BEARING_DISTANCE: f32 = 1.0e-3;

/// Record or keep this tick's entry memory and build the ring the path's
/// engaged slot verb proposes. `None` when the motion is no such verb or there
/// is no target; the memory clears then, so the next engaged tick enters
/// fresh.
///
/// Entry is an activity entry at or above the depth that supplies the motion,
/// a change of verb or target, or no memory at all (a restored brain).
pub(super) fn plan(
    brain: &mut BrainComponent,
    motion: Option<ResolvedMotion>,
    entry_depth: Option<usize>,
    target: Option<TargetPawn>,
    position: Vec3,
    standoff: f32,
) -> Option<SlotRing> {
    let (Some(motion), Some(target)) = (motion, target) else {
        brain.engaged_motion = EngagedMotionMemory::None;
        return None;
    };
    let entered = entry_depth.is_some_and(|depth| depth <= motion.depth);
    match motion.verb {
        MotionVerb::Strafe => {
            let direction = match brain.engaged_motion {
                EngagedMotionMemory::Strafe(memory)
                    if !entered && memory.target == target.entity =>
                {
                    memory.direction
                }
                _ => 0,
            };
            brain.engaged_motion = EngagedMotionMemory::Strafe(StrafeMemory {
                target: target.entity,
                direction,
            });
            // Rotated to the current bearing every tick, so a strafe steps
            // 45° from wherever the enemy is, never from the shared ring.
            let bearing = horizontal_bearing(target.position, position)
                .or_else(|| {
                    brain
                        .combat_slot
                        .and_then(|slot| horizontal_bearing(target.position, slot))
                })
                .unwrap_or(Vec3::X);
            Some(match direction {
                // First entry: the better-scored neighbor; an exact tie goes
                // to the counter-clockwise one, the lower ring index.
                0 => SlotRing::by_score(bearing, (1 << CCW_NEIGHBOR) | (1 << CW_NEIGHBOR)),
                // Onward first, then the reverse.
                direction if direction > 0 => {
                    SlotRing::listed(bearing, &[CCW_NEIGHBOR, CW_NEIGHBOR])
                }
                _ => SlotRing::listed(bearing, &[CW_NEIGHBOR, CCW_NEIGHBOR]),
            })
        }
        MotionVerb::Reposition => {
            let memory = match brain.engaged_motion {
                EngagedMotionMemory::Reposition(memory)
                    if !entered && memory.target == target.entity =>
                {
                    memory
                }
                _ => reposition_entry(
                    target,
                    position,
                    brain.combat_slot,
                    motion.reposition,
                    standoff,
                ),
            };
            brain.engaged_motion = EngagedMotionMemory::Reposition(memory);
            let ring = SlotRing::by_score(memory.ring_bearing, memory.ring_slots).with_keep_away(
                KeepAway {
                    point: memory.entry_position,
                    distance: memory.min_travel,
                },
            );
            Some(match memory.entry_slot {
                Some(slot) => ring.with_keep_away(KeepAway {
                    point: slot,
                    distance: memory.slot_spacing,
                }),
                None => ring,
            })
        }
        _ => {
            brain.engaged_motion = EngagedMotionMemory::None;
            None
        }
    }
}

/// Fix a `reposition`'s bounds at entry (pin P16). The arc is measured from
/// the bearing from the target to the enemy; with none, from the bearing to
/// the held slot, and with no held slot either the arc is ignored.
fn reposition_entry(
    target: TargetPawn,
    position: Vec3,
    held_slot: Option<Vec3>,
    params: RepositionParams,
    standoff: f32,
) -> RepositionMemory {
    let spacing = slot_spacing(standoff);
    let (ring_bearing, ring_slots) =
        if let Some(bearing) = horizontal_bearing(target.position, position) {
            // Straight in or out along the entry bearing is a back-off.
            let mask = arc_slot_mask(params.arc_deg) & !(1 << RADIAL_SLOT);
            (bearing, mask)
        } else if let Some(bearing) =
            held_slot.and_then(|slot| horizontal_bearing(target.position, slot))
        {
            (bearing, arc_slot_mask(params.arc_deg))
        } else {
            (Vec3::X, u8::MAX)
        };
    RepositionMemory {
        target: target.entity,
        ring_bearing,
        ring_slots,
        entry_position: position,
        min_travel: params.min_travel_m.unwrap_or(spacing),
        entry_slot: held_slot,
        slot_spacing: spacing,
    }
}

/// Steer an engaged slot verb from its batch claim: walk to the claimed slot,
/// or stand when it has none. A strafer's direction becomes the side it was
/// granted, so a batch loser reverses once (pin P9); with no claim it keeps
/// its direction, counter-clockwise when it never picked one.
pub(super) fn settle(outcome: &mut EnemyOutcome, claim: Option<CombatCandidate>) {
    if let EngagedMotionMemory::Strafe(memory) = &mut outcome.brain.engaged_motion {
        memory.direction = match claim.map(|claim| claim.generation_index) {
            Some(index) if index == usize::from(CCW_NEIGHBOR) => 1,
            Some(index) if index == usize::from(CW_NEIGHBOR) => -1,
            _ if memory.direction == 0 => 1,
            _ => memory.direction,
        };
    }
    outcome.steering = match claim {
        Some(claim) => SteeringIntent::MoveTo(claim.position),
        None => SteeringIntent::Clear,
    };
}

fn horizontal_bearing(from: Vec3, to: Vec3) -> Option<Vec3> {
    let offset = Vec3::new(to.x - from.x, 0.0, to.z - from.z);
    let length = offset.length();
    (length.is_finite() && length > MIN_BEARING_DISTANCE).then(|| offset / length)
}
