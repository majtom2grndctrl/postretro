//! A residency walk's levers: the lead, each pool's size, the play drain cap
//! and the lightmap in-hand cap.
//! See: context/lib/rendering_pipeline.md §4

use std::num::NonZeroU64;

use postretro_render_cpu::lightmap_pool::{PerPool, PoolKind};

use crate::lightmap_streaming::controller::LightmapResidencyController;
use crate::session::level_streaming::LevelStreaming;

/// The runs to make, in place of the default sweep: see [`parse_pool_layers`].
pub(super) const POOL_LAYERS_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_POOL_LAYERS";

/// `bound` in [`POOL_LAYERS_ENV`]: each pool at its id 58 figure, no margin.
const AT_THE_BOUND: &str = "bound";

/// Every run's play drain cap in place of production: see [`parse_drain_cap`].
pub(crate) const DRAIN_CAP_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_DRAIN_CAP";

/// Every run's lightmap in-hand pair byte total in place of production: see
/// [`parse_in_hand_cap`].
pub(crate) const IN_HAND_CAP_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_IN_HAND_CAP";

#[derive(Debug, Clone, Copy)]
pub(super) struct Levers {
    pub(super) lead_metres: f32,
    /// A pool's first generation in layers, taken as given. `None` sizes that
    /// pool as the runtime does, from the level's id 58.
    pub(super) pool_layers: PerPool<Option<u32>>,
    /// The play drain cap; `None` is production.
    pub(super) drain_cap: Option<WalkDrainCap>,
    /// The lightmap in-hand total, the band taking half; `None` is
    /// production.
    pub(super) in_hand_cap: Option<WalkInHandCap>,
}

/// The shipped levers: the run every lever sweep varies one lever from.
pub(super) const DEFAULT_LEVERS: Levers = Levers {
    lead_metres: 16.0,
    pool_layers: PerPool {
        lightmap: None,
        mask: None,
    },
    drain_cap: None,
    in_hand_cap: None,
};

impl Levers {
    /// The pool sizes this run asked for, lightmap then mask. It names the
    /// request, not the result, so a paired run's label is the same on any bake.
    pub(super) fn pools_label(&self) -> String {
        let figure = |kind: PoolKind| {
            self.pool_layers[kind].map_or_else(|| "id58".to_string(), |layers| layers.to_string())
        };
        format!(
            "pools {}/{}",
            figure(PoolKind::Lightmap),
            figure(PoolKind::Mask)
        )
    }

    /// The run label's drain cap part: empty at production, so an unset run's
    /// label is unchanged.
    pub(super) fn drain_cap_label(&self) -> String {
        self.drain_cap
            .map_or_else(String::new, |cap| format!(" | drain cap {} MiB", cap.mib))
    }

    /// The run label's in-hand cap part: empty at production, so an unset
    /// run's label is unchanged.
    pub(super) fn in_hand_cap_label(&self) -> String {
        self.in_hand_cap
            .map_or_else(String::new, |cap| format!(" | in-hand cap {} MiB", cap.mib))
    }
}

/// A walk's play drain cap: the MiB asked for and the whole bytes it applies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WalkDrainCap {
    pub(crate) mib: f64,
    pub(crate) bytes: NonZeroU64,
}

impl WalkDrainCap {
    /// Sets `level`'s play cap override, the seam the dev-tools slider sets.
    pub(crate) fn apply(self, level: &mut LevelStreaming) {
        level.set_play_drain_cap_override(Some(self.bytes));
    }
}

/// A walk's lightmap in-hand cap: the MiB asked for and the whole bytes it
/// applies to the total; the band takes half, as in production.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WalkInHandCap {
    pub(crate) mib: f64,
    pub(crate) bytes: NonZeroU64,
}

impl WalkInHandCap {
    /// Sets `controller`'s in-hand total and its band half.
    pub(crate) fn apply(self, controller: &mut LightmapResidencyController) {
        controller.set_in_hand_cap_override(Some(self.bytes.get()));
    }
}

/// The play drain cap from [`DRAIN_CAP_ENV`], when set.
pub(super) fn drain_cap_from_env() -> Result<Option<WalkDrainCap>, String> {
    mib_from_env(DRAIN_CAP_ENV, parse_drain_cap)
}

/// The lightmap in-hand cap from [`IN_HAND_CAP_ENV`], when set.
pub(super) fn in_hand_cap_from_env() -> Result<Option<WalkInHandCap>, String> {
    mib_from_env(IN_HAND_CAP_ENV, parse_in_hand_cap)
}

/// `env` through `parse` when set; `None` when unset.
fn mib_from_env<T>(env: &str, parse: fn(&str) -> Result<T, String>) -> Result<Option<T>, String> {
    match std::env::var(env) {
        Ok(value) => parse(&value).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(why) => Err(format!("{env}: {why}")),
    }
}

/// A play drain cap: see [`parse_mib`].
pub(crate) fn parse_drain_cap(value: &str) -> Result<WalkDrainCap, String> {
    let (mib, bytes) = parse_mib(
        DRAIN_CAP_ENV,
        value,
        "a zero cap admits only each drain's first item; unset it for production",
    )?;
    Ok(WalkDrainCap { mib, bytes })
}

/// A lightmap in-hand total: see [`parse_mib`].
pub(crate) fn parse_in_hand_cap(value: &str) -> Result<WalkInHandCap, String> {
    let (mib, bytes) = parse_mib(
        IN_HAND_CAP_ENV,
        value,
        "a zero cap holds only one read in hand; unset it for production",
    )?;
    Ok(WalkInHandCap { mib, bytes })
}

/// `env`'s value through the slider seed's parser: see
/// [`parse_cap_mib`](crate::session::level_streaming::parse_cap_mib).
fn parse_mib(env: &str, value: &str, zero_why: &str) -> Result<(f64, NonZeroU64), String> {
    crate::session::level_streaming::parse_cap_mib(env, value, zero_why)
}

/// Each run's pool sizes from a comma-separated list. A run is `LIGHTMAP/MASK`
/// in layers: `34/15`. Either figure may be left out to take that pool's size
/// from id 58, so `30` and `30/` set the lightmap pool alone and `/15` the mask
/// pool alone. `bound` is both pools at their stored `bound`, with no margin.
pub(super) fn parse_pool_layers(
    list: &str,
    bound: PerPool<u32>,
) -> Result<Vec<PerPool<Option<u32>>>, String> {
    list.split(',')
        .map(|run| {
            let run = run.trim();
            if run == AT_THE_BOUND {
                return Ok(bound.map(Some));
            }
            let (lightmap, mask) = run.split_once('/').unwrap_or((run, ""));
            Ok(PerPool {
                lightmap: pool_figure(lightmap)
                    .map_err(|why| format!("{POOL_LAYERS_ENV} run {run:?}: {why}"))?,
                mask: pool_figure(mask)
                    .map_err(|why| format!("{POOL_LAYERS_ENV} run {run:?}: {why}"))?,
            })
        })
        .collect()
}

fn pool_figure(figure: &str) -> Result<Option<u32>, String> {
    let figure = figure.trim();
    if figure.is_empty() {
        return Ok(None);
    }
    match figure.parse() {
        Ok(0) => Err("a pool holds at least one layer".to_string()),
        Ok(layers) => Ok(Some(layers)),
        Err(_) => Err(format!("{figure:?} is not a layer count")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUND: PerPool<u32> = PerPool {
        lightmap: 27,
        mask: 12,
    };

    fn pools(lightmap: Option<u32>, mask: Option<u32>) -> PerPool<Option<u32>> {
        PerPool { lightmap, mask }
    }

    // A sweep can name each pool apart, and one pool alone.
    #[test]
    fn pool_layers_list_sets_each_pool_apart_and_leaves_the_rest_to_id_58() {
        assert_eq!(
            parse_pool_layers("34/15, 30, 28/, /15, bound", BOUND).unwrap(),
            [
                pools(Some(34), Some(15)),
                pools(Some(30), None),
                pools(Some(28), None),
                pools(None, Some(15)),
                pools(Some(27), Some(12)),
            ]
        );
    }

    #[test]
    fn pool_layers_list_refuses_a_zero_or_unreadable_figure_by_name() {
        for list in ["0/15", "34/0", "many", "34/15/2"] {
            let why = parse_pool_layers(list, BOUND).unwrap_err();
            assert!(why.contains(POOL_LAYERS_ENV), "{why}");
        }
    }

    #[test]
    fn run_label_names_what_each_pool_asked_for() {
        assert_eq!(DEFAULT_LEVERS.pools_label(), "pools id58/id58");
        let levers = Levers {
            pool_layers: pools(Some(30), None),
            ..DEFAULT_LEVERS
        };
        assert_eq!(levers.pools_label(), "pools 30/id58");
    }

    #[test]
    fn drain_cap_accepts_positive_mib_as_whole_bytes() {
        for (value, bytes) in [
            ("8", 8 << 20),
            ("4", 4 << 20),
            ("16", 16 << 20),
            (" 0.5 ", 512 << 10),
            ("2.25", (2 << 20) + (256 << 10)),
        ] {
            let cap = parse_drain_cap(value).unwrap();
            assert_eq!(cap.bytes.get(), bytes, "{value}");
        }
        assert_eq!(parse_drain_cap("16").unwrap().mib, 16.0);
    }

    /// Every value either cap refuses, with the reason its refusal names.
    const REFUSED: [(&str, &str); 12] = [
        ("0", "zero cap"),
        ("0.0", "zero cap"),
        ("-0", "zero cap"),
        ("-4", "positive"),
        ("", "not a number"),
        ("eight", "not a number"),
        ("8MiB", "not a number"),
        ("inf", "not a number"),
        ("NaN", "not a number"),
        ("1e300", "too large"),
        ("17592186044416", "too large"),
        ("1e-12", "rounds to zero"),
    ];

    #[test]
    fn drain_cap_refuses_zero_negative_non_numbers_and_overflow_by_name() {
        for (value, why) in REFUSED {
            let refusal = parse_drain_cap(value).unwrap_err();
            assert!(
                refusal.contains(DRAIN_CAP_ENV) && refusal.contains(why),
                "{value:?}: {refusal}"
            );
        }
    }

    #[test]
    fn run_label_names_a_drain_cap_only_when_set() {
        assert_eq!(DEFAULT_LEVERS.drain_cap_label(), "");
        let levers = Levers {
            drain_cap: Some(parse_drain_cap("16").unwrap()),
            ..DEFAULT_LEVERS
        };
        assert_eq!(levers.drain_cap_label(), " | drain cap 16 MiB");
    }

    #[test]
    fn in_hand_cap_accepts_positive_mib_as_whole_bytes() {
        for (value, bytes) in [
            ("16", 16 << 20),
            ("64", 64 << 20),
            ("128", 128 << 20),
            (" 0.5 ", 512 << 10),
            ("2.25", (2 << 20) + (256 << 10)),
        ] {
            let cap = parse_in_hand_cap(value).unwrap();
            assert_eq!(cap.bytes.get(), bytes, "{value}");
        }
        assert_eq!(parse_in_hand_cap("64").unwrap().mib, 64.0);
    }

    #[test]
    fn in_hand_cap_refuses_zero_negative_non_numbers_and_overflow_by_name() {
        for (value, why) in REFUSED {
            let refusal = parse_in_hand_cap(value).unwrap_err();
            assert!(
                refusal.contains(IN_HAND_CAP_ENV) && refusal.contains(why),
                "{value:?}: {refusal}"
            );
        }
    }

    #[test]
    fn run_label_names_an_in_hand_cap_only_when_set() {
        assert_eq!(DEFAULT_LEVERS.in_hand_cap_label(), "");
        let levers = Levers {
            in_hand_cap: Some(parse_in_hand_cap("16").unwrap()),
            ..DEFAULT_LEVERS
        };
        assert_eq!(levers.in_hand_cap_label(), " | in-hand cap 16 MiB");
    }
}
