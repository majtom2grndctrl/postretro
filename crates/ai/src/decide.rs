// Decide ticks: the ticks an NPC runs target acquisition, its guards, its move
// and action selectors and combat-slot choice. The ticks between act on what
// the last decide chose. Phases are staggered by each NPC's identity, never
// by spawn order or frame split.
// See: context/lib/entity_model.md §7c (decide ticks)

#[cfg(test)]
use postretro_sim::npc_tiers::phase;
pub(crate) use postretro_sim::npc_tiers::{DECIDE_INTERVAL_TICKS, decides_on};

/// A schedule identity whose decides fall on AI ticks `tick`, `tick + 4`,
/// and so on at production cadence. A brain also decides on its first run.
#[cfg(test)]
pub(crate) fn identity_deciding_on(tick: u64) -> u64 {
    (0..)
        .find(|&identity| decides_on(tick, identity, DECIDE_INTERVAL_TICKS))
        .expect("some identity takes every phase")
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
                    // from 128 (AC 33, owner ruling).
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

    // P15, dormant decides: a dormant NPC decides on one of its four dormant
    // runs in each 2 s cycle, so its decide falls on one of 120 slots
    // (its run phase plus 30 times its decide run). The busiest slot carries
    // at most mean + 2√mean + 1 from 128 NPCs, the dormant cycle's own floor.
    #[test]
    fn dormant_decides_balance_statistically_for_every_identity_set() {
        use postretro_sim::npc_tiers::{
            DORMANT_INTERVAL_TICKS, DORMANT_RUNS_PER_DECIDE, dormant_decides_on,
        };
        let cycle = DORMANT_INTERVAL_TICKS * DORMANT_RUNS_PER_DECIDE;
        for count in [128_u32, 256, 500, 1000] {
            for (label, identities) in identity_sets(count) {
                let mut load = vec![0_u32; cycle as usize];
                for identity in &identities {
                    let slots: Vec<u64> = (0..cycle)
                        .filter(|&tick| dormant_decides_on(tick, *identity))
                        .collect();
                    assert_eq!(slots.len(), 1, "{label}: one dormant decide per cycle");
                    load[slots[0] as usize] += 1;
                }
                let mean = identities.len() as f64 / cycle as f64;
                let ceiling = mean + 2.0 * mean.sqrt() + 1.0;
                let busiest = *load.iter().max().unwrap();
                assert!(
                    f64::from(busiest) <= ceiling,
                    "{label}, {count} over {cycle} dormant decide slots: busiest {busiest} > \
                     {ceiling:.1}"
                );
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
