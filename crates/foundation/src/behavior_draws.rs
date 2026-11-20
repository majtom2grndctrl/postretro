// Host-only enemy behavior draws: the per-instance seed and the counter-based
// streams the `activityRoll` fact and a stance hold's hidden release time draw
// from, and the mapping of a release draw onto its hold's `releaseAfter` range.
// See: context/lib/networking.md (engine randomness, second carved exception)
//      context/lib/entity_model.md §7c (Stances and attacks)

// A draw is a pure function of `(seed, stream, counter)`, so a brain stores no
// RNG state beyond its seed and one counter per stream, and replay reproduces
// every draw. The seed salts on placement identity, never on `EntityId`, so a
// reversed spawn order of same-descriptor enemies draws the same values by
// placement. It lives here, below `entities`, because the brain component draws
// on entry and `entities` cannot reach `postretro-sim`'s `SplitMix64`; the mix
// below is the same SplitMix64 finalizer and increment.
//
// The roll and the release time draw from separate streams so that a guard
// reading `activityRoll` can never predict when a hold releases.

use serde::{Deserialize, Serialize};

use crate::data_descriptors::BehaviorGraphEnvelope;

/// SplitMix64's golden-ratio increment.
const GOLDEN_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// Separates the stream keys of one seed.
const STREAM_KEY: u64 = 0xD1B5_4A32_D192_ED03;

/// The SplitMix64 output finalizer: a bijective 64-bit avalanche.
const fn splitmix64_finalize(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The independent draw streams of one behavior seed. Each stream keeps its
/// own counter, so a draw on one never shifts the values another reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BehaviorDrawStream {
    /// `@brain.activityRoll`, one draw per activity-level entry.
    ActivityRoll = 0,
    /// A stance hold's hidden release time, one draw per hold entry, a
    /// pose-mate's included. Its own stream, never the roll's: guards must
    /// not be able to predict a release from the roll they read.
    HoldRelease = 1,
}

/// How many [`BehaviorDrawStream`]s one seed carries.
pub const BEHAVIOR_DRAW_STREAMS: usize = 2;

/// A brain's draw state: its per-instance seed and one draw counter per
/// stream. Host-only simulation state; it never crosses the wire, and clients
/// never draw.
///
/// Every field defaults, so a brain serialized before draws existed restores
/// with seed `0` and draws on its next entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehaviorDraws {
    #[serde(default)]
    pub seed: u64,
    /// Draws taken so far, indexed by [`BehaviorDrawStream`].
    #[serde(default)]
    pub counters: [u64; BEHAVIOR_DRAW_STREAMS],
}

impl BehaviorDraws {
    /// Restart every stream under `seed`.
    pub fn reseed(&mut self, seed: u64) {
        *self = Self {
            seed,
            counters: [0; BEHAVIOR_DRAW_STREAMS],
        };
    }

    /// The next draw of `stream`, uniform in `[0, 1)`.
    pub fn next_unit(&mut self, stream: BehaviorDrawStream) -> f32 {
        let counter = &mut self.counters[stream as usize];
        let draw = behavior_draw_unit(self.seed, stream, *counter);
        *counter = counter.wrapping_add(1);
        draw
    }
}

/// A brain's per-instance identity for its draws: replay-stable and
/// independent of the order enemies are spawned in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BehaviorDrawSalt {
    /// A map-placed instance: its authored entity index (PRL list ordinal).
    Placement(u32),
    /// The `count`-th enemy a map-placed spawner has released this level, by
    /// that spawner's authored entity index.
    Spawned { spawner: u32, count: u32 },
    /// No authored identity reaches this spawn (a synthetic placement or a
    /// spawner with no map ordinal): the entity's own registry index, which is
    /// replay-stable only while spawn order is.
    Entity(u32),
}

impl BehaviorDrawSalt {
    /// One 64-bit word per salt. The top two bits keep the three kinds apart,
    /// so a spawner ordinal keeps its low 30 bits (below them); a level holds
    /// far fewer entities than that. Also the NPC's schedule identity, which
    /// staggers its decide and dormant phases.
    pub const fn word(self) -> u64 {
        match self {
            BehaviorDrawSalt::Placement(ordinal) => ordinal as u64,
            BehaviorDrawSalt::Spawned { spawner, count } => {
                (1 << 62) | (((spawner & SPAWNER_ORDINAL_MASK) as u64) << 32) | count as u64
            }
            BehaviorDrawSalt::Entity(index) => (2 << 62) | index as u64,
        }
    }
}

/// The spawner-ordinal bits a [`BehaviorDrawSalt::Spawned`] word keeps, below
/// its kind tag.
const SPAWNER_ORDINAL_MASK: u32 = (1 << 30) - 1;

/// The behavior draw seed for one enemy: its descriptor name mixed with its
/// placement salt through SplitMix64. Two same-descriptor enemies differ by
/// salt; two descriptors at one salt differ by name.
pub fn behavior_draw_seed(descriptor: &str, salt: BehaviorDrawSalt) -> u64 {
    // FNV-1a folds the name into one word before the avalanche.
    let mut name = 0xCBF2_9CE4_8422_2325_u64;
    for byte in descriptor.bytes() {
        name ^= u64::from(byte);
        name = name.wrapping_mul(0x0000_0100_0000_01B3);
    }
    splitmix64_finalize(
        splitmix64_finalize(name).wrapping_add(salt.word().wrapping_mul(GOLDEN_GAMMA)),
    )
}

/// The `counter`-th draw of `stream` under `seed`, uniform in `[0, 1)`.
///
/// Counter-based: equal to the `counter`-th output of a SplitMix64 generator
/// keyed by `(seed, stream)`, without storing generator state.
pub fn behavior_draw_unit(seed: u64, stream: BehaviorDrawStream, counter: u64) -> f32 {
    let key = splitmix64_finalize(seed ^ (stream as u64 + 1).wrapping_mul(STREAM_KEY));
    let bits =
        splitmix64_finalize(key.wrapping_add(counter.wrapping_add(1).wrapping_mul(GOLDEN_GAMMA)));
    // The top 24 bits fill an f32 mantissa exactly, so the result never
    // rounds up to 1.0.
    (bits >> 40) as f32 / (1u64 << 24) as f32
}

/// The `releaseAfter` range of the row releasing `name` in `envelope`, if the
/// activity is a stance hold. Validation allows at most one per activity.
pub fn hold_release_range(envelope: &BehaviorGraphEnvelope, name: &str) -> Option<[f32; 2]> {
    envelope
        .transitions
        .get(name)?
        .iter()
        .find_map(|row| row.release_after)
}

/// A hold's release time in `[min, max]` from a [`BehaviorDrawStream::HoldRelease`]
/// draw `unit` in `[0, 1)`, mapped uniformly; `[x, x]`, or an inverted range,
/// is `min`.
pub fn release_draw([min, max]: [f32; 2], unit: f32) -> f32 {
    if max <= min {
        min
    } else {
        min + unit * (max - min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_span_the_unit_interval_without_degenerating() {
        let seed = behavior_draw_seed("brute", BehaviorDrawSalt::Placement(7));
        let draws: Vec<f32> = (0..4096)
            .map(|counter| behavior_draw_unit(seed, BehaviorDrawStream::ActivityRoll, counter))
            .collect();
        assert!(draws.iter().all(|draw| (0.0..1.0).contains(draw)));
        let below_half = draws.iter().filter(|draw| **draw < 0.5).count();
        assert!(
            (1800..2300).contains(&below_half),
            "draws straddle the midpoint: {below_half} of 4096 below 0.5"
        );
        let mut buckets = [0_u32; 10];
        for draw in &draws {
            buckets[(draw * 10.0) as usize] += 1;
        }
        assert!(
            buckets.iter().all(|count| *count > 300),
            "every tenth of [0, 1) is reached: {buckets:?}"
        );
    }

    #[test]
    fn streams_of_one_seed_and_same_descriptor_salts_draw_differently() {
        let seed = behavior_draw_seed("brute", BehaviorDrawSalt::Placement(7));
        let roll =
            |seed, counter| behavior_draw_unit(seed, BehaviorDrawStream::ActivityRoll, counter);
        let release = |counter| behavior_draw_unit(seed, BehaviorDrawStream::HoldRelease, counter);
        assert!((0..64).any(|counter| roll(seed, counter) != release(counter)));
        assert_eq!(roll(seed, 3), roll(seed, 3), "a draw is a pure function");

        let salts = [
            BehaviorDrawSalt::Placement(7),
            BehaviorDrawSalt::Placement(8),
            BehaviorDrawSalt::Spawned {
                spawner: 7,
                count: 0,
            },
            BehaviorDrawSalt::Spawned {
                spawner: 7,
                count: 1,
            },
            BehaviorDrawSalt::Entity(7),
        ];
        let firsts: Vec<f32> = salts
            .iter()
            .map(|salt| roll(behavior_draw_seed("brute", *salt), 0))
            .collect();
        for (index, first) in firsts.iter().enumerate() {
            assert!(
                !firsts[index + 1..].contains(first),
                "salt {:?} shares its first roll with a later salt",
                salts[index]
            );
        }
        assert_ne!(
            roll(
                behavior_draw_seed("brute", BehaviorDrawSalt::Placement(7)),
                0
            ),
            roll(
                behavior_draw_seed("trooper", BehaviorDrawSalt::Placement(7)),
                0
            ),
            "the descriptor name is part of the seed"
        );
    }

    // The kind tag stays apart from a spawner ordinal too wide for its field:
    // the word keeps the ordinal's low 30 bits.
    #[test]
    fn a_wide_spawner_ordinal_never_reaches_the_kind_tag() {
        for spawner in [0, 7, SPAWNER_ORDINAL_MASK, 1 << 30, u32::MAX] {
            let word = BehaviorDrawSalt::Spawned { spawner, count: 3 }.word();
            assert_eq!(word >> 62, 1, "spawner {spawner} keeps the spawned tag");
            assert_eq!(
                word & u64::from(u32::MAX),
                3,
                "the count fills the low word"
            );
        }
        assert_eq!(
            BehaviorDrawSalt::Spawned {
                spawner: 7,
                count: 0
            }
            .word(),
            (1 << 62) | (7 << 32),
            "an ordinal in range is unchanged"
        );
    }

    // A release draw maps the unit interval onto its range; a point range is
    // its one value.
    #[test]
    fn a_release_draw_spans_its_range() {
        assert_eq!(release_draw([500.0, 1200.0], 0.0), 500.0);
        let mid = release_draw([500.0, 1200.0], 0.5);
        assert!((mid - 850.0).abs() < 1e-3, "{mid}");
        assert!(release_draw([500.0, 1200.0], 0.999_999) < 1200.0);
        assert_eq!(release_draw([400.0, 400.0], 0.7), 400.0);
        assert_eq!(release_draw([900.0, 400.0], 0.7), 900.0);
    }

    // Only the row carrying `releaseAfter` names a hold's range.
    #[test]
    fn a_hold_release_range_comes_from_its_release_row() {
        let envelope: BehaviorGraphEnvelope = serde_json::from_value(serde_json::json!({
            "initial": "overhead",
            "activities": {
                "overhead": { "animation": "idle", "motion": "hold", "pose": "overhead" },
                "slam_overhead": { "animation": "idle", "motion": "hold", "pose": "overhead" },
                "slam_release": { "animation": "attack", "motion": "hold" }
            },
            "transitions": {
                "overhead": [{ "to": "slam_overhead" }],
                "slam_overhead": [
                    { "to": "overhead" },
                    { "to": "slam_release", "releaseAfter": [500.0, 1200.0] }
                ]
            }
        }))
        .expect("the lowered envelope deserializes");
        assert_eq!(
            hold_release_range(&envelope, "slam_overhead"),
            Some([500.0, 1200.0])
        );
        assert_eq!(hold_release_range(&envelope, "overhead"), None);
        assert_eq!(hold_release_range(&envelope, "slam_release"), None);
    }
}
