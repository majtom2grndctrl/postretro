//! Mover probe channel: bake fidelity `low` against `high` at rest. Each map builds `--no-cache`
//! (interactive, `low`) and `--release` (`high`); at each terminus every probe either mover build
//! stores a bounce state for composes that state or id 34, and the two composes are compared in
//! display levels, with their sign. Direct and depth never depend on fidelity.
//!
//! The gate fails past the owner's bound, [`LOW_AGAINST_HIGH_BOUND_LEVELS`], and prints the
//! largest difference per map, mover and terminus, with its sign and where it falls. `#[ignore]`:
//! it bakes `mover-closet` and `mover-setpiece` cold at both fidelities.
//! See: context/lib/build_pipeline.md §Build Cache (Bake fidelity)

mod mover_bounce_support;
mod prl_build_support;

use std::collections::BTreeSet;

use mover_bounce_support::{
    Cache, Tile, Volume, bounce_states_in, build, channel, dev_map, section, signed_display_levels,
    with_worldspawn_key,
};
use postretro_level_format::SectionId;

/// The owner's bound on `low` against `high` at rest, in display levels either way, set
/// from a first measurement: `low` is an iteration preview, judged in-engine.
const LOW_AGAINST_HIGH_BOUND_LEVELS: f32 = 12.0;

/// The largest signed difference over every compared probe: `low` shown minus `high` shown.
#[derive(Default)]
struct Worst {
    darker: (f32, usize),
    brighter: (f32, usize),
    compared: usize,
    past_one: usize,
    past_three: usize,
}

impl Worst {
    fn add(&mut self, probe: usize, difference: f32) {
        self.compared += 1;
        if difference.abs() > 1.0 {
            self.past_one += 1;
        }
        if difference.abs() > 3.0 {
            self.past_three += 1;
        }
        if difference < self.darker.0 {
            self.darker = (difference, probe);
        }
        if difference > self.brighter.0 {
            self.brighter = (difference, probe);
        }
    }
}

// Low against high at rest, on `mover-closet` and `mover-setpiece`: the largest
// per-probe display-level difference and its sign are reported, and the test fails past the
// owner's bound.
#[test]
#[ignore = "bakes mover-closet and mover-setpiece cold at low and high (about a minute); run \
            `cargo test -p postretro-level-compiler --test mover_probe_channel_fidelity_rest -- \
            --ignored`"]
fn low_against_high_at_rest_stays_within_the_owners_bound_and_reports_the_largest_difference() {
    let mut misses = Vec::new();
    for name in ["mover-closet", "mover-setpiece"] {
        // Uniform L0, so id 34 reads back per probe.
        let map = with_worldspawn_key(&dev_map(&format!("{name}.map")), "_sh_coarsen", "0");
        let low = build(&map, name, Cache::Cold, &[]);
        let high = build(&map, name, Cache::Cold, &["--release"]);
        let volume = Volume::read(&high.prl);
        assert!(
            section(&low.prl, SectionId::OctahedralShVolume)
                == section(&high.prl, SectionId::OctahedralShVolume),
            "{name}: id 34 does not depend on fidelity"
        );
        let (low_channel, high_channel) = (
            channel(&low.prl).expect("low id 57"),
            channel(&high.prl).expect("high id 57"),
        );
        let movers: BTreeSet<u32> = high_channel
            .bounce_entries
            .iter()
            .chain(&low_channel.bounce_entries)
            .map(|entry| entry.mover_id)
            .collect();
        for mover in movers {
            let (low_states, high_states) = (
                bounce_states_in(&low_channel, &volume, mover),
                bounce_states_in(&high_channel, &volume, mover),
            );
            let probes: BTreeSet<usize> = low_states
                .keys()
                .chain(high_states.keys())
                .copied()
                .collect();
            for (terminus, label) in [(0, "t′ 0"), (1, "t′ 1")] {
                let compose = |states: &std::collections::BTreeMap<usize, [Option<Tile>; 2]>,
                               probe: usize| {
                    states
                        .get(&probe)
                        .and_then(|tiles| tiles[terminus].clone())
                        .unwrap_or_else(|| volume.tile(probe))
                };
                let mut worst = Worst::default();
                for &probe in &probes {
                    worst.add(
                        probe,
                        signed_display_levels(
                            &compose(&low_states, probe),
                            &compose(&high_states, probe),
                        ),
                    );
                }
                let at = |probe: usize| {
                    volume
                        .quake_position(probe)
                        .map(|axis| (axis * 10.0).round() / 10.0)
                };
                eprintln!(
                    "{name} mover #{mover} {label}: {} probe(s) compared; low darker by up to \
                     {:.2} levels at probe {} {:?}, brighter by up to {:.2} at probe {} {:?}; {} \
                     past 1 level, {} past 3",
                    worst.compared,
                    worst.darker.0.abs(),
                    worst.darker.1,
                    at(worst.darker.1),
                    worst.brighter.0,
                    worst.brighter.1,
                    at(worst.brighter.1),
                    worst.past_one,
                    worst.past_three,
                );
                assert!(worst.compared > 0, "{name} mover #{mover}: probes compared");
                for (difference, probe) in [worst.darker, worst.brighter] {
                    if difference.abs() > LOW_AGAINST_HIGH_BOUND_LEVELS {
                        misses.push(format!(
                            "{name} mover #{mover} {label}: low {} by {:.2} levels at probe \
                             {probe} {:?}",
                            if difference < 0.0 {
                                "darker"
                            } else {
                                "brighter"
                            },
                            difference.abs(),
                            at(probe),
                        ));
                    }
                }
            }
        }
    }
    assert!(
        misses.is_empty(),
        "low against high at rest past the owner's {LOW_AGAINST_HIGH_BOUND_LEVELS} levels:\n{}",
        misses.join("\n")
    );
}
