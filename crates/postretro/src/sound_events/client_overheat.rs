// Connected-client overheat cue, from the rising edge of the replicated
// owner-private overheat latch. Presentation only; it lags the host by one round trip.
// See: context/lib/audio.md §4 · context/lib/networking.md §Combat authority

use postretro_entities::EntityId;
use postretro_sim::weapon::ReplicatedWeaponProjection;

/// What this client holds in the host wieldable slot the replicated latch
/// names: a weapon that runs heat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ProjectedHeatWeapon {
    pub(crate) weapon: EntityId,
    /// Whether that slot is the client's own active slot.
    pub(crate) wielded: bool,
}

/// One frame's replicated heat state, as the tracker reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum OverheatReading {
    Sample(OverheatSample),
    /// Nothing is replicated yet, or the named slot holds no heat weapon,
    /// here or on the host.
    Absent,
    /// The heat values name different host slots. State records arrive per
    /// slot, not atomically, so such a frame is held rather than read.
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OverheatSample {
    pub(crate) weapon: EntityId,
    pub(crate) wielded: bool,
    /// `player.overheated`.
    pub(crate) overheated: bool,
}

impl OverheatReading {
    /// Read the projection against the weapon its latch's host slot holds.
    /// The heat value, and the threshold once replicated, must describe that
    /// slot too.
    pub(crate) fn from_projection(
        projection: &ReplicatedWeaponProjection,
        weapon_at: impl FnOnce(usize) -> Option<ProjectedHeatWeapon>,
    ) -> Self {
        let Some(latch) = projection.overheated else {
            return Self::Absent;
        };
        let slot = latch.slot;
        let Some(projected) = weapon_at(slot) else {
            return Self::Absent;
        };
        let Some(heat) = projection.heat else {
            return Self::Absent;
        };
        if heat.slot != slot
            || projection
                .overheat_at
                .is_some_and(|overheat_at| overheat_at.slot != slot)
        {
            return Self::Mixed;
        }
        // The host names this slot's weapon heat-less.
        if heat.value.is_none() {
            return Self::Absent;
        }
        Self::Sample(OverheatSample {
            weapon: projected.weapon,
            wielded: projected.wielded,
            overheated: latch.value,
        })
    }
}

/// Turns successive replicated heat samples into the host's `overheat` cue:
/// the latch rising on the weapon the client wields. The host raises it on the
/// shot that crosses the threshold, and the latch holds until heat reaches 0,
/// so one rise is one overheat.
///
/// Samples follow the weapon the host projects. A change of projected weapon
/// resets the baseline without a cue, so a switch back to a weapon still
/// locked out reads as a held latch. A rise while the client wields another
/// slot plays nothing; its baseline still advances.
#[derive(Debug, Default)]
pub(crate) struct ClientOverheatEdge {
    previous: Option<OverheatSample>,
}

impl ClientOverheatEdge {
    /// Observe this frame's reading and return whether it completes an
    /// overheat. A mixed reading leaves the tracker as it was.
    pub(crate) fn observe_reading(&mut self, reading: OverheatReading) -> bool {
        match reading {
            OverheatReading::Sample(current) => {
                let previous = self.previous.replace(current);
                current.wielded
                    && current.overheated
                    && previous.is_some_and(|previous| {
                        previous.weapon == current.weapon && !previous.overheated
                    })
            }
            OverheatReading::Absent => {
                self.previous = None;
                false
            }
            OverheatReading::Mixed => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_sim::weapon::SlotSample;

    fn rifle() -> EntityId {
        EntityId::from_raw(1)
    }

    fn pistol() -> EntityId {
        EntityId::from_raw(2)
    }

    fn reading(weapon: EntityId, overheated: bool) -> OverheatReading {
        OverheatReading::Sample(OverheatSample {
            weapon,
            wielded: true,
            overheated,
        })
    }

    /// Feed each reading for two frames, as a frame loop between snapshots
    /// does, and count the cues.
    fn cues(readings: &[OverheatReading]) -> usize {
        let mut edge = ClientOverheatEdge::default();
        readings
            .iter()
            .flat_map(|reading| [*reading, *reading])
            .filter(|reading| edge.observe_reading(*reading))
            .count()
    }

    #[test]
    fn a_rising_latch_plays_one_overheat_cue() {
        assert_eq!(
            cues(&[
                reading(rifle(), false),
                reading(rifle(), true),
                reading(rifle(), true),
                reading(rifle(), false),
                reading(rifle(), true),
            ]),
            2
        );
    }

    #[test]
    fn a_held_latch_plays_nothing() {
        // First seen already latched: a join, a reset, or a switch back.
        assert_eq!(cues(&[reading(rifle(), true), reading(rifle(), true)]), 0);
        assert_eq!(
            cues(&[
                reading(rifle(), true),
                OverheatReading::Absent,
                reading(rifle(), true),
            ]),
            0,
            "an absent frame resets the baseline without a cue"
        );
    }

    #[test]
    fn a_switch_to_a_latched_weapon_plays_nothing() {
        assert_eq!(
            cues(&[reading(pistol(), false), reading(rifle(), true)]),
            0,
            "the latch belongs to another weapon than the last sample's"
        );
        assert_eq!(
            cues(&[
                reading(rifle(), false),
                reading(pistol(), false),
                reading(rifle(), true),
            ]),
            0,
            "a switch away and back seen between samples"
        );
    }

    #[test]
    fn a_mixed_frame_plays_nothing_and_holds_the_baseline() {
        assert_eq!(
            cues(&[
                reading(rifle(), false),
                OverheatReading::Mixed,
                reading(rifle(), true),
            ]),
            1,
            "the held unlatched sample still frames the rise"
        );
        assert_eq!(cues(&[OverheatReading::Mixed]), 0);
    }

    #[test]
    fn a_rise_on_a_weapon_the_client_no_longer_wields_plays_nothing() {
        let holstered = |overheated| {
            OverheatReading::Sample(OverheatSample {
                weapon: rifle(),
                wielded: false,
                overheated,
            })
        };
        assert_eq!(cues(&[holstered(false), holstered(true)]), 0);
        assert_eq!(
            cues(&[holstered(false), holstered(true), reading(rifle(), true)]),
            0,
            "returning to it later reads a held latch"
        );
    }

    fn slot_sample<T>(slot: usize, value: T) -> Option<SlotSample<T>> {
        Some(SlotSample { slot, value })
    }

    fn held_in(slot: usize) -> Option<ProjectedHeatWeapon> {
        (slot == 1).then_some(ProjectedHeatWeapon {
            weapon: rifle(),
            wielded: true,
        })
    }

    #[test]
    fn a_reading_attributes_every_value_to_the_latchs_slot() {
        let latched_on = |slot| ReplicatedWeaponProjection {
            heat: slot_sample(slot, Some(80.0)),
            overheat_at: slot_sample(slot, Some(80.0)),
            overheated: slot_sample(slot, true),
            ..ReplicatedWeaponProjection::default()
        };
        assert_eq!(
            OverheatReading::from_projection(&latched_on(1), held_in),
            reading(rifle(), true)
        );
        assert_eq!(
            OverheatReading::from_projection(&latched_on(0), held_in),
            OverheatReading::Absent,
            "the client holds no heat weapon in the named slot"
        );
        for mixed in [
            ReplicatedWeaponProjection {
                heat: slot_sample(0, None),
                ..latched_on(1)
            },
            ReplicatedWeaponProjection {
                overheat_at: slot_sample(0, None),
                ..latched_on(1)
            },
        ] {
            assert_eq!(
                OverheatReading::from_projection(&mixed, held_in),
                OverheatReading::Mixed
            );
        }
        assert_eq!(
            OverheatReading::from_projection(
                &ReplicatedWeaponProjection {
                    heat: slot_sample(1, None),
                    overheat_at: None,
                    ..latched_on(1)
                },
                held_in
            ),
            OverheatReading::Absent,
            "the host names this slot's weapon heat-less"
        );
        assert_eq!(
            OverheatReading::from_projection(
                &ReplicatedWeaponProjection {
                    heat: None,
                    ..latched_on(1)
                },
                held_in
            ),
            OverheatReading::Absent,
            "heat not replicated yet"
        );
        assert_eq!(
            OverheatReading::from_projection(&ReplicatedWeaponProjection::default(), held_in),
            OverheatReading::Absent
        );
    }
}
