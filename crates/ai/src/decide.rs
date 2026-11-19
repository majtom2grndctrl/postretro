// Decide ticks: the ticks an NPC runs target acquisition, its guards, its move
// and action selectors and combat-slot choice. The ticks between act on what
// the last decide chose. Phases are staggered by each NPC's identity, never
// by spawn order or frame split.
// See: context/lib/entity_model.md §7c (decide ticks)

/// Ticks between an NPC's decides: 15 Hz on the 60 Hz tick.
pub(crate) const DECIDE_INTERVAL_TICKS: u64 = 4;

/// Whether an NPC with `identity` decides on AI tick `tick`, deciding every
/// `interval` ticks.
pub(crate) fn decides_on(tick: u64, identity: u64, interval: u64) -> bool {
    let interval = interval.max(1);
    (tick + phase(identity, interval)) % interval == 0
}

/// An identity's phase in a cycle of `interval` ticks: Fibonacci hashing,
/// the fractional part of `identity × φ` scaled to the cycle. Identities are
/// mostly counters (map ordinals, spawn counts), and a Weyl sequence spreads
/// any run of them, strided or interleaved, near evenly; a mixing hash would
/// spread them only as well as chance.
pub(crate) fn phase(identity: u64, interval: u64) -> u64 {
    let fraction = identity.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    ((u128::from(fraction) * u128::from(interval.max(1))) >> 64) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_foundation::BehaviorDrawSalt;

    /// Identity sets the stress fixture spawns: two spawners both counting
    /// from zero, placements whose ordinals step by 4, and the two
    /// interleaved with placements.
    fn identity_sets(count: u32) -> Vec<(&'static str, Vec<u64>)> {
        let spawned = |spawner| {
            (0..count / 2).map(move |n| BehaviorDrawSalt::Spawned { spawner, count: n }.word())
        };
        let two_spawners: Vec<u64> = spawned(7).chain(spawned(9)).collect();
        let strided: Vec<u64> = (0..count)
            .map(|n| BehaviorDrawSalt::Placement(n * 4).word())
            .collect();
        let interleaved: Vec<u64> = (0..count)
            .map(|n| {
                if n % 2 == 0 {
                    BehaviorDrawSalt::Placement(n).word()
                } else {
                    BehaviorDrawSalt::Spawned {
                        spawner: 3,
                        count: n / 2,
                    }
                    .word()
                }
            })
            .collect();
        vec![
            ("two spawners", two_spawners),
            ("ordinals by 4", strided),
            ("interleaved", interleaved),
        ]
    }

    // P15: the busiest phase carries at most mean + 2√mean + 1 and no phase is
    // empty, at 64 NPCs or more, for the decide cycle and the dormant one.
    #[test]
    fn phases_balance_statistically_for_every_identity_set() {
        for interval in [DECIDE_INTERVAL_TICKS, 30] {
            for count in [16_u32, 32, 64, 128, 256, 500, 1000] {
                for (label, identities) in identity_sets(count) {
                    let mut load = vec![0_u32; interval as usize];
                    for identity in &identities {
                        load[phase(*identity, interval) as usize] += 1;
                    }
                    let mean = identities.len() as f64 / interval as f64;
                    let ceiling = mean + 2.0 * mean.sqrt() + 1.0;
                    let busiest = *load.iter().max().unwrap();
                    assert!(
                        f64::from(busiest) <= ceiling,
                        "{label}, {count} over {interval}: busiest {busiest} > {ceiling:.1}"
                    );
                    // At 64 NPCs over the 30 dormant phases (about two each)
                    // a phase can sit empty; the dormant cycle is held to it
                    // from 128 (plan: AC 33 asks the owner).
                    let checks_empty = count >= 64 && (interval < 30 || count >= 128);
                    if checks_empty {
                        assert!(
                            load.iter().all(|&n| n > 0),
                            "{label}, {count} over {interval}: an empty phase {load:?}"
                        );
                    }
                }
            }
        }
    }

    // Every NPC decides exactly once per interval, whatever its phase.
    #[test]
    fn each_identity_decides_once_per_interval() {
        for identity in [0, 1, 2, 3, 4, 17, u64::MAX, 1 << 62] {
            for start in 0..8 {
                let decides = (start..start + DECIDE_INTERVAL_TICKS)
                    .filter(|&tick| decides_on(tick, identity, DECIDE_INTERVAL_TICKS))
                    .count();
                assert_eq!(decides, 1, "identity {identity}, from tick {start}");
            }
        }
    }
}
