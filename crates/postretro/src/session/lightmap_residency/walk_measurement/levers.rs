//! A residency walk's levers: the lead, each pool's size, the play drain
//! budget (a byte cap or a time budget) and the lightmap in-hand cap.
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

/// Every run's play drain time budget in µs in place of production: see
/// [`parse_drain_time`]. Exclusive with [`DRAIN_CAP_ENV`].
pub(crate) const DRAIN_TIME_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_DRAIN_TIME_US";

/// Every run's lightmap in-hand pair byte total in place of production: see
/// [`parse_in_hand_cap`].
pub(crate) const IN_HAND_CAP_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_IN_HAND_CAP";

const MIB: f64 = 1024.0 * 1024.0;

#[derive(Debug, Clone, Copy)]
pub(super) struct Levers {
    pub(super) lead_metres: f32,
    /// A pool's first generation in layers, taken as given. `None` sizes that
    /// pool as the runtime does, from the level's id 58.
    pub(super) pool_layers: PerPool<Option<u32>>,
    /// The play drain budget; `None` is production.
    pub(super) drain_budget: Option<WalkDrainBudget>,
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
    drain_budget: None,
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

    /// The run label's drain budget part: empty at production, so an unset
    /// run's label is unchanged.
    pub(super) fn drain_budget_label(&self) -> String {
        match self.drain_budget {
            None => String::new(),
            Some(WalkDrainBudget::Cap(cap)) => format!(" | drain cap {} MiB", cap.mib),
            Some(WalkDrainBudget::Time(time)) => format!(" | drain time {} µs", time.micros),
        }
    }

    /// The byte cap's MiB, when the run set one.
    pub(super) fn drain_cap_mib(&self) -> Option<f64> {
        match self.drain_budget {
            Some(WalkDrainBudget::Cap(cap)) => Some(cap.mib),
            _ => None,
        }
    }

    /// The time budget's µs, when the run set one.
    pub(super) fn drain_time_us(&self) -> Option<u64> {
        match self.drain_budget {
            Some(WalkDrainBudget::Time(time)) => Some(time.micros.get()),
            _ => None,
        }
    }

    /// The run label's in-hand cap part: empty at production, so an unset
    /// run's label is unchanged.
    pub(super) fn in_hand_cap_label(&self) -> String {
        self.in_hand_cap
            .map_or_else(String::new, |cap| format!(" | in-hand cap {} MiB", cap.mib))
    }
}

/// A walk's play drain budget: a decoded-byte cap or an estimated
/// install-time budget, never both.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WalkDrainBudget {
    Cap(WalkDrainCap),
    Time(WalkDrainTime),
}

impl WalkDrainBudget {
    /// Sets `level`'s play budget override.
    pub(crate) fn apply(self, level: &mut LevelStreaming) {
        match self {
            Self::Cap(cap) => cap.apply(level),
            Self::Time(time) => time.apply(level),
        }
    }
}

/// A walk's play drain time budget in whole µs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WalkDrainTime {
    pub(crate) micros: NonZeroU64,
}

impl WalkDrainTime {
    /// Sets `level`'s play time override.
    pub(crate) fn apply(self, level: &mut LevelStreaming) {
        level.set_play_drain_time_override(Some(self.micros));
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

/// The play drain budget from [`DRAIN_CAP_ENV`] or [`DRAIN_TIME_ENV`], when
/// one is set: see [`drain_budget_from`].
pub(super) fn drain_budget_from_env() -> Result<Option<WalkDrainBudget>, String> {
    let raw = |env: &str| lever_from_env(env, |value| Ok(value.to_string()));
    drain_budget_from(
        raw(DRAIN_CAP_ENV)?.as_deref(),
        raw(DRAIN_TIME_ENV)?.as_deref(),
    )
}

/// The play drain budget from the cap's and the time budget's raw values.
/// Both set is refused naming both, since one run admits in one unit.
pub(super) fn drain_budget_from(
    cap: Option<&str>,
    time: Option<&str>,
) -> Result<Option<WalkDrainBudget>, String> {
    match (cap, time) {
        (Some(_), Some(_)) => Err(format!(
            "{DRAIN_CAP_ENV} and {DRAIN_TIME_ENV} are both set: a run admits against a byte cap \
             or a time budget, not both; unset one"
        )),
        (Some(cap), None) => parse_drain_cap(cap).map(|cap| Some(WalkDrainBudget::Cap(cap))),
        (None, Some(time)) => parse_drain_time(time).map(|time| Some(WalkDrainBudget::Time(time))),
        (None, None) => Ok(None),
    }
}

/// The lightmap in-hand cap from [`IN_HAND_CAP_ENV`], when set.
pub(super) fn in_hand_cap_from_env() -> Result<Option<WalkInHandCap>, String> {
    lever_from_env(IN_HAND_CAP_ENV, parse_in_hand_cap)
}

/// `env` through `parse` when set; `None` when unset.
fn lever_from_env<T>(
    env: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<Option<T>, String> {
    match std::env::var(env) {
        Ok(value) => parse(&value).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(why) => Err(format!("{env}: {why}")),
    }
}

/// A play drain time budget: a positive whole number of µs (`4000`). Zero,
/// a negative, a fraction, a non-number and a value past `u64` are refused
/// by name, never clamped.
pub(crate) fn parse_drain_time(value: &str) -> Result<WalkDrainTime, String> {
    let refuse = |why: &str| format!("{DRAIN_TIME_ENV}={value:?}: {why}");
    let micros: u64 = value
        .trim()
        .parse()
        .map_err(|_| refuse("not a whole number of µs"))?;
    let micros = NonZeroU64::new(micros).ok_or_else(|| {
        refuse("a zero budget admits only each drain's first item; unset it for production")
    })?;
    Ok(WalkDrainTime { micros })
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

/// `env`'s value as a positive decimal count of MiB (`4`, `16`, `0.5`) and
/// its whole bytes. Zero (refused with `zero_why`), a negative, a non-number,
/// a value past `u64` bytes and one rounding to zero bytes are refused by
/// name, never clamped.
fn parse_mib(env: &str, value: &str, zero_why: &str) -> Result<(f64, NonZeroU64), String> {
    let refuse = |why: &str| format!("{env}={value:?}: {why}");
    let mib: f64 = value
        .trim()
        .parse()
        .ok()
        .filter(|mib: &f64| mib.is_finite())
        .ok_or_else(|| refuse("not a number of MiB"))?;
    if mib < 0.0 {
        return Err(refuse("a cap is a positive number of MiB"));
    }
    if mib == 0.0 {
        return Err(refuse(zero_why));
    }
    let bytes = (mib * MIB).round();
    if bytes >= u64::MAX as f64 {
        return Err(refuse("too large to represent in bytes"));
    }
    let bytes = NonZeroU64::new(bytes as u64).ok_or_else(|| refuse("rounds to zero bytes"))?;
    Ok((mib, bytes))
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
    fn run_label_names_the_drain_budget_only_when_set() {
        assert_eq!(DEFAULT_LEVERS.drain_budget_label(), "");
        let capped = Levers {
            drain_budget: drain_budget_from(Some("16"), None).unwrap(),
            ..DEFAULT_LEVERS
        };
        assert_eq!(capped.drain_budget_label(), " | drain cap 16 MiB");
        let timed = Levers {
            drain_budget: drain_budget_from(None, Some("4000")).unwrap(),
            ..DEFAULT_LEVERS
        };
        assert_eq!(timed.drain_budget_label(), " | drain time 4000 µs");
    }

    #[test]
    fn drain_time_accepts_positive_whole_micros() {
        for (value, micros) in [("4000", 4_000), (" 12000 ", 12_000), ("1", 1)] {
            assert_eq!(parse_drain_time(value).unwrap().micros.get(), micros);
        }
    }

    #[test]
    fn drain_time_refuses_zero_negative_fractions_and_non_numbers_by_name() {
        for (value, why) in [
            ("0", "zero budget"),
            ("-4000", "not a whole number"),
            ("4.5", "not a whole number"),
            ("4ms", "not a whole number"),
            ("", "not a whole number"),
            ("18446744073709551616", "not a whole number"),
        ] {
            let refusal = parse_drain_time(value).unwrap_err();
            assert!(
                refusal.contains(DRAIN_TIME_ENV) && refusal.contains(why),
                "{value:?}: {refusal}"
            );
        }
    }

    // One run admits in one unit: the cap and the time budget together are
    // refused naming both; either alone, or neither, is taken.
    #[test]
    fn drain_cap_and_drain_time_together_are_refused_naming_both() {
        let refusal = drain_budget_from(Some("8"), Some("4000")).unwrap_err();
        assert!(
            refusal.contains(DRAIN_CAP_ENV) && refusal.contains(DRAIN_TIME_ENV),
            "{refusal}"
        );
        assert_eq!(drain_budget_from(None, None).unwrap(), None);
        assert_eq!(
            drain_budget_from(Some("8"), None).unwrap(),
            Some(WalkDrainBudget::Cap(parse_drain_cap("8").unwrap()))
        );
        assert_eq!(
            drain_budget_from(None, Some("4000")).unwrap(),
            Some(WalkDrainBudget::Time(parse_drain_time("4000").unwrap()))
        );
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
