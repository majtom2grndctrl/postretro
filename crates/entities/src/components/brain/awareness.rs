// Per-candidate awareness meters: an NPC without a target notices each
// candidate it could acquire through a bounded meter, filled and drained at its
// acquisition checks and set by hits. Storage and the meter arithmetic; the AI
// tick owns when checks run and which candidates they visit.
// See: context/lib/entity_model.md §7c

use serde::{Deserialize, Serialize};

use crate::registry::EntityId;
use postretro_foundation::AwarenessDescriptor;

/// How many candidates one NPC meters at once: the nearest few that pass.
pub const AWARENESS_METER_CAPACITY: usize = 8;

/// A meter this close below 1 reads full. Fills summed check by check carry
/// float rounding (fifteen 66.7 ms checks over 1000 ms); the slack is far
/// below one check's gain at any authored rate.
pub const AWARENESS_FULL_EPSILON: f32 = 1.0e-4;

/// One candidate's meter.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AwarenessMeter {
    pub candidate: EntityId,
    /// 0 to 1; 1 notices.
    pub value: f32,
    /// The candidate passed candidacy and line of sight on the check that
    /// last visited it, so a pass on the next check is a second sighting in a
    /// row and fills. A hit or a held target leaves it false: the next
    /// sighting is a first one.
    pub seen: bool,
    /// Milliseconds since the check that last visited the meter, or since
    /// the hit that set it: the time its next fill or drain covers.
    pub since_ms: f32,
    /// A hit set the meter since the brain last ran. The next run ages it
    /// one tick, never the time it spent unscheduled before the hit, so the
    /// meter drains from the hit's tick.
    pub hit_since_run: bool,
    /// The candidate's XZ distance when last visited or hit. Among equal
    /// values, eviction takes the farthest.
    pub distance: f32,
    /// The candidate's life counter when metered: its respawn forgets the
    /// meter, so it is noticed again through a fresh fill. Host-local.
    #[serde(skip)]
    pub life: u32,
}

/// An NPC's meters, beside its attacker ledger. The meters serialize and
/// restore; each meter's `life` stamp does not (it restarts at 0), so a
/// restore keeps what the NPC was noticing only when it is a whole-world
/// restore where the pawns' life counters restart too. Against a pawn whose
/// life counter is above 0 the meter reads as from an earlier life and the
/// brain drops it at its next run. Old brains start with none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct AwarenessMeters {
    #[serde(default)]
    pub slots: [Option<AwarenessMeter>; AWARENESS_METER_CAPACITY],
    /// The candidate the NPC selected on its last check or holds as its
    /// target. A resting NPC, which keeps no target between checks, re-selects
    /// it while its meter stays full instead of taking another full one, so it
    /// never flips between candidates.
    #[serde(default)]
    pub selected: Option<EntityId>,
}

impl AwarenessMeters {
    /// `candidate`'s meter, if it holds one.
    pub fn meter(&self, candidate: EntityId) -> Option<&AwarenessMeter> {
        self.slots
            .iter()
            .flatten()
            .find(|meter| meter.candidate == candidate)
    }

    /// `candidate`'s value: 0 without a meter.
    pub fn value(&self, candidate: EntityId) -> f32 {
        self.meter(candidate).map_or(0.0, |meter| meter.value)
    }

    /// The highest meter, 0 to 1; 0 with none.
    pub fn highest(&self) -> f32 {
        self.slots
            .iter()
            .flatten()
            .map(|meter| meter.value)
            .fold(0.0, f32::max)
            .clamp(0.0, 1.0)
    }

    /// How many candidates hold a meter.
    pub fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Empty every meter: a closed aggro gate. The next sighting of any
    /// candidate is a first sighting.
    pub fn empty(&mut self) {
        *self = Self::default();
    }

    /// Drop `candidate`'s meter, if it holds one.
    pub fn forget(&mut self, candidate: EntityId) {
        self.forget_where(|meter| meter.candidate == candidate);
    }

    /// Drop every meter `forget` names; a dropped candidate is no longer the
    /// last selection.
    pub fn forget_where(&mut self, forget: impl Fn(&AwarenessMeter) -> bool) {
        for slot in self.slots.iter_mut() {
            if let Some(meter) = *slot
                && forget(&meter)
            {
                if self.selected == Some(meter.candidate) {
                    self.selected = None;
                }
                *slot = None;
            }
        }
    }

    /// Advance every meter's clock over one run: a meter a hit set since the
    /// last run ages one tick, `tick_dt_ms`; the rest age the run's span,
    /// `span_dt_ms`.
    pub fn age(&mut self, span_dt_ms: f32, tick_dt_ms: f32) {
        for meter in self.slots.iter_mut().flatten() {
            if meter.hit_since_run {
                meter.since_ms = tick_dt_ms;
                meter.hit_since_run = false;
            } else {
                meter.since_ms += span_dt_ms;
            }
        }
    }

    /// The NPC holds `target`: no meter changes while it does. Each run
    /// restarts every clock and sighting, so once the target is lost every
    /// meter drains from where it stands. A meter below full refills only from
    /// a second sighting in a row; the lost target's own meter still stands at
    /// 1 with no sighting, so it stays full through a first sighting and is
    /// re-selected on sight as the previous selection.
    pub fn hold_target(&mut self, target: EntityId) {
        for meter in self.slots.iter_mut().flatten() {
            meter.since_ms = 0.0;
            meter.seen = false;
            meter.hit_since_run = false;
        }
        self.selected = Some(target);
    }

    /// A new acquisition: `target`'s meter at 1, every other cleared.
    pub fn acquire(&mut self, target: EntityId, distance: f32, life: u32) {
        self.slots = Default::default();
        self.slots[0] = Some(AwarenessMeter {
            candidate: target,
            value: 1.0,
            seen: true,
            since_ms: 0.0,
            hit_since_run: false,
            distance,
            life,
        });
        self.selected = Some(target);
    }

    /// A hit on an NPC without a target: its attacker's meter at 1, draining
    /// from this hit. An attacker without a meter takes a slot, evicting the
    /// lowest-valued member, the farthest among equals.
    pub fn record_hit(&mut self, attacker: EntityId, distance: f32, life: u32) {
        let meter = AwarenessMeter {
            candidate: attacker,
            value: 1.0,
            seen: false,
            since_ms: 0.0,
            hit_since_run: true,
            distance,
            life,
        };
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_some_and(|member| member.candidate == attacker))
        {
            *slot = Some(meter);
            return;
        }
        let slot = self
            .slots
            .iter()
            .position(Option::is_none)
            .or_else(|| lowest_farthest(&self.slots, |_| true))
            .expect("a full meter set has a member to evict");
        self.slots[slot] = Some(meter);
    }
}

/// The slot `eligible` admits whose meter is lowest, the farthest among
/// equal values, the earlier slot among equal distances.
fn lowest_farthest(
    slots: &[Option<AwarenessMeter>; AWARENESS_METER_CAPACITY],
    eligible: impl Fn(usize) -> bool,
) -> Option<usize> {
    let mut best: Option<(usize, AwarenessMeter)> = None;
    for (slot, meter) in slots.iter().enumerate() {
        let Some(meter) = *meter else {
            continue;
        };
        if !eligible(slot) {
            continue;
        }
        if best.is_none_or(|(_, current)| {
            meter
                .value
                .total_cmp(&current.value)
                .then_with(|| current.distance.total_cmp(&meter.distance))
                .is_lt()
        }) {
            best = Some((slot, meter));
        }
    }
    best.map(|(slot, _)| slot)
}

/// One acquisition check over an NPC's meters. The AI visits candidates
/// nearest first: members by [`Self::visit`], then passing newcomers by
/// [`Self::admit`]; [`Self::finish`] frees what the check could not visit.
pub struct AwarenessCheck<'a> {
    meters: &'a mut AwarenessMeters,
    rates: AwarenessDescriptor,
    visited: [bool; AWARENESS_METER_CAPACITY],
    passed: [bool; AWARENESS_METER_CAPACITY],
}

impl<'a> AwarenessCheck<'a> {
    pub fn new(meters: &'a mut AwarenessMeters, rates: AwarenessDescriptor) -> Self {
        Self {
            meters,
            rates,
            visited: [false; AWARENESS_METER_CAPACITY],
            passed: [false; AWARENESS_METER_CAPACITY],
        }
    }

    /// The meters as the check has left them so far.
    pub fn meters(&self) -> &AwarenessMeters {
        self.meters
    }

    /// `candidate`'s slot, if it holds a meter.
    pub fn member(&self, candidate: EntityId) -> Option<usize> {
        self.meters
            .slots
            .iter()
            .position(|slot| slot.is_some_and(|meter| meter.candidate == candidate))
    }

    /// The candidate in `slot`, if any.
    pub fn candidate(&self, slot: usize) -> Option<EntityId> {
        self.meters.slots[slot].map(|meter| meter.candidate)
    }

    pub fn visited(&self, slot: usize) -> bool {
        self.visited[slot]
    }

    /// A member visited this check: a pass after a pass gains the time
    /// between the two checks over `fillMs`, a first sighting gains nothing;
    /// a fail drains the time since its last check, or its hit, over
    /// `drainMs`. A duration of 0 acts as one check.
    pub fn visit(&mut self, slot: usize, distance: f32, passes: bool) {
        let Some(meter) = self.meters.slots[slot].as_mut() else {
            return;
        };
        if passes {
            if meter.seen {
                let gain = if self.rates.fill_ms > 0.0 {
                    meter.since_ms / self.rates.fill_ms
                } else {
                    1.0
                };
                meter.value = (meter.value + gain).min(1.0);
            }
            if meter.value >= 1.0 - AWARENESS_FULL_EPSILON {
                meter.value = 1.0;
            }
        } else {
            let drain = if self.rates.drain_ms > 0.0 {
                meter.since_ms / self.rates.drain_ms
            } else {
                1.0
            };
            meter.value = (meter.value - drain).max(0.0);
        }
        meter.seen = passes;
        meter.since_ms = 0.0;
        meter.hit_since_run = false;
        meter.distance = distance;
        self.visited[slot] = true;
        self.passed[slot] = passes;
    }

    /// The member in `slot` passed this check with a full meter.
    pub fn full(&self, slot: usize) -> bool {
        self.passed[slot] && self.meters.slots[slot].is_some_and(|meter| meter.value >= 1.0)
    }

    /// A candidate without a meter that passed this check takes a slot at 0:
    /// a free one, else a member this check has not visited (departed; `finish`
    /// frees its slot anyway, so it goes before any live member), else a
    /// visited member that failed (lowest value first, the farthest among
    /// equals), else a passing member farther than it. Nearer failing
    /// candidates never keep a passing one unmetered.
    pub fn admit(&mut self, candidate: EntityId, distance: f32, life: u32) -> bool {
        let slot = self
            .meters
            .slots
            .iter()
            .position(Option::is_none)
            .or_else(|| lowest_farthest(&self.meters.slots, |slot| !self.visited[slot]))
            .or_else(|| lowest_farthest(&self.meters.slots, |slot| !self.passed[slot]))
            .or_else(|| {
                lowest_farthest(&self.meters.slots, |slot| {
                    self.meters.slots[slot].is_some_and(|meter| meter.distance > distance)
                })
            });
        let Some(slot) = slot else {
            return false;
        };
        self.meters.slots[slot] = Some(AwarenessMeter {
            candidate,
            value: 0.0,
            seen: true,
            since_ms: 0.0,
            hit_since_run: false,
            distance,
            life,
        });
        self.visited[slot] = true;
        self.passed[slot] = true;
        true
    }

    /// A new acquisition on this check: see [`AwarenessMeters::acquire`].
    pub fn acquire(self, target: EntityId, distance: f32, life: u32) {
        self.meters.acquire(target, distance, life);
    }

    /// End the check with `selected` as its selection. A member the check
    /// could not visit (departed, no longer a pawn) frees its slot and its
    /// value; one drained to 0 frees it too.
    pub fn finish(self, selected: Option<EntityId>) {
        for (slot, entry) in self.meters.slots.iter_mut().enumerate() {
            let keep = self.visited[slot]
                && entry.is_some_and(|meter| self.passed[slot] || meter.value > 0.0);
            if !keep {
                *entry = None;
            }
        }
        self.meters.selected = selected;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u32) -> EntityId {
        EntityId::from_raw(index)
    }

    const RATES: AwarenessDescriptor = AwarenessDescriptor {
        fill_ms: 1_000.0,
        drain_ms: 2_000.0,
    };

    fn check(meters: &mut AwarenessMeters, sightings: &[(u32, f32, bool)], dt_ms: f32) {
        meters.age(dt_ms, dt_ms);
        let mut check = AwarenessCheck::new(meters, RATES);
        let mut newcomers = Vec::new();
        for &(candidate, distance, passes) in sightings {
            match check.member(id(candidate)) {
                Some(slot) => check.visit(slot, distance, passes),
                None if passes => newcomers.push((candidate, distance)),
                None => {}
            }
        }
        for (candidate, distance) in newcomers {
            check.admit(id(candidate), distance, 0);
        }
        check.finish(None);
    }

    // A first sighting gains nothing; each later pass gains the time since
    // the previous check; a fail drains it over `drainMs`.
    #[test]
    fn a_meter_fills_from_the_second_sighting_and_drains_on_a_miss() {
        let mut meters = AwarenessMeters::default();
        check(&mut meters, &[(1, 5.0, true)], 100.0);
        assert_eq!(meters.value(id(1)), 0.0);
        check(&mut meters, &[(1, 5.0, true)], 250.0);
        assert!((meters.value(id(1)) - 0.25).abs() < 1e-6);
        check(&mut meters, &[(1, 5.0, false)], 200.0);
        assert!((meters.value(id(1)) - 0.15).abs() < 1e-6);
        check(&mut meters, &[(1, 5.0, true)], 100.0);
        assert!(
            (meters.value(id(1)) - 0.15).abs() < 1e-6,
            "a sighting after a miss is a first one"
        );
        check(&mut meters, &[], 100.0);
        assert!(meters.is_empty(), "an unvisited member frees its slot");
    }

    // P43: a hit always takes a slot at 1, evicting the lowest member, the
    // farthest among equals; it drains from the hit, aged one tick by the
    // run that follows it.
    #[test]
    fn a_hit_takes_a_slot_at_one_and_drains_from_the_hit() {
        let mut meters = AwarenessMeters::default();
        let sightings: Vec<_> = (0..8).map(|i| (i, 1.0 + i as f32, true)).collect();
        check(&mut meters, &sightings, 16.0);
        assert_eq!(meters.len(), 8);
        meters.record_hit(id(9), 30.0, 0);
        assert_eq!(meters.value(id(9)), 1.0);
        assert!(
            meters.meter(id(7)).is_none(),
            "the farthest of the equal members is evicted"
        );
        meters.age(500.0, 1_000.0 / 60.0);
        let since = meters.meter(id(9)).unwrap().since_ms;
        assert!((since - 1_000.0 / 60.0).abs() < 1e-4, "{since}");
    }

    // P33: passing newcomers evict failing members first, lowest first; with
    // eight passing members, only one nearer than a passing member gets in.
    #[test]
    fn passing_newcomers_evict_failing_members_before_passing_ones() {
        let mut meters = AwarenessMeters::default();
        let sightings: Vec<_> = (0..8).map(|i| (i, 10.0 + i as f32, true)).collect();
        check(&mut meters, &sightings, 16.0);
        check(&mut meters, &sightings, 500.0);
        let mut failing = sightings.clone();
        failing[3].2 = false;
        failing.push((20, 30.0, true));
        check(&mut meters, &failing, 16.0);
        assert!(
            meters.meter(id(3)).is_none(),
            "the failing member is evicted"
        );
        assert_eq!(meters.value(id(20)), 0.0);
        let mut all: Vec<_> = (0..8).map(|i| (i, 10.0 + i as f32, true)).collect();
        all.retain(|(i, _, _)| *i != 3);
        all.push((20, 30.0, true));
        all.push((21, 40.0, true));
        check(&mut meters, &all, 16.0);
        assert!(
            meters.meter(id(21)).is_none(),
            "eight nearer passing members keep a farther newcomer out"
        );
    }

    // P32: a member the check never visits (departed) frees its slot at
    // `finish`, so a passing newcomer takes it before evicting a failing
    // member, even one with a lower value.
    #[test]
    fn admit_takes_a_departed_slot_before_evicting_a_failing_member() {
        let mut meters = AwarenessMeters::default();
        let sightings: Vec<_> = (0..8).map(|i| (i, 10.0 + i as f32, true)).collect();
        check(&mut meters, &sightings, 16.0);
        check(&mut meters, &sightings, 500.0);
        assert_eq!(meters.len(), 8);
        // Member 3 fails but keeps some value; member 5 departs with a higher
        // one, so the old lowest-value fallback would have evicted member 3.
        let slot_of = |meters: &AwarenessMeters, who: u32| {
            meters
                .slots
                .iter()
                .position(|slot| slot.is_some_and(|meter| meter.candidate == id(who)))
                .unwrap()
        };
        let failing = slot_of(&meters, 3);
        let departed = slot_of(&meters, 5);
        meters.slots[failing].as_mut().unwrap().value = 0.4;
        meters.slots[departed].as_mut().unwrap().value = 0.9;
        meters.age(100.0, 100.0);
        let mut check = AwarenessCheck::new(&mut meters, RATES);
        for who in 0..8u32 {
            if who == 5 {
                continue;
            }
            let slot = check.member(id(who)).unwrap();
            check.visit(slot, 10.0 + who as f32, who != 3);
        }
        assert!(check.admit(id(20), 30.0, 0));
        check.finish(None);
        assert!(meters.meter(id(5)).is_none(), "the departed slot is taken");
        assert!(
            meters.meter(id(3)).is_some(),
            "the failing member keeps its slot"
        );
        assert_eq!(meters.value(id(20)), 0.0);
    }
}
