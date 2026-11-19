//! Per-drain estimated install time and admitted item sizes, per resource, for the drain time-budget sweep.
//! See: context/lib/rendering_pipeline.md §4 (shared drain budget)

// Every figure is over admitted items, priced by `estimated_install_us`, and is
// reported for every run, byte budget or time budget, so a byte-cap baseline
// carries the same keys. A lone item is a drain whose admitted set is exactly
// one item; it is counted when its estimate passes the run's reference.

use serde_json::{Value, json};

use super::levers::Levers;
use super::report::{max_and_p95, mib, nearest_rank};
use crate::streaming::drain_budget::DrainItem;
use crate::streaming::request::StreamResource;

/// The time a byte-budget run's lone items are judged against: the draft
/// brief's starting play budget, a quarter of a 60 Hz frame.
pub(super) const BYTE_RUN_REFERENCE_US: u64 = 4_000;

/// Each resource with its result-key prefix and prose name.
const RESOURCES: [(StreamResource, &str, &str); StreamResource::COUNT] = [
    (StreamResource::Sh, "sh", "SH"),
    (StreamResource::LightmapBlock, "lightmap", "lightmap"),
];

/// The estimated time a run's lone items are judged against: its play time
/// budget, or [`BYTE_RUN_REFERENCE_US`] in a byte-budget run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LoneItemReference {
    micros: u64,
    time_budget: bool,
}

impl LoneItemReference {
    pub(super) fn of(levers: &Levers) -> Self {
        match levers.drain_time_us() {
            Some(micros) => Self {
                micros,
                time_budget: true,
            },
            None => Self {
                micros: BYTE_RUN_REFERENCE_US,
                time_budget: false,
            },
        }
    }

    fn label(self) -> String {
        if self.time_budget {
            format!("the play time budget ({} µs)", self.micros)
        } else {
            format!(
                "a {} µs reference (byte-budget run: the brief's starting time budget, not this \
                 run's budget)",
                self.micros
            )
        }
    }

    /// The reference's result keys, once per run.
    pub(super) fn json(self, into: &mut serde_json::Map<String, Value>) {
        into.insert("lone_reference_us".to_string(), json!(self.micros));
        into.insert(
            "lone_reference".to_string(),
            json!(if self.time_budget {
                "time_budget"
            } else {
                "byte_run_4000us"
            }),
        );
    }
}

/// One resource's admitted work.
#[derive(Debug, Default, Clone)]
struct ResourceTally {
    /// This resource's estimated µs in each drain that admitted any of it.
    drain_estimates: Vec<u64>,
    /// Every admitted item's decoded bytes.
    item_bytes: Vec<u64>,
    /// The estimate of each drain that admitted this resource's item alone.
    lone_estimates: Vec<u64>,
}

impl ResourceTally {
    /// Item count, then p50, p95 and max decoded bytes; `None` without items.
    fn item_sizes(&self) -> (usize, Option<[u64; 3]>) {
        let mut sorted = self.item_bytes.clone();
        sorted.sort_unstable();
        let quantiles = (!sorted.is_empty()).then(|| {
            [
                nearest_rank(&sorted, 0.5),
                nearest_rank(&sorted, 0.95),
                *sorted.last().expect("non-empty"),
            ]
        });
        (sorted.len(), quantiles)
    }

    /// Lone items over `reference`, and the largest such estimate.
    fn lone_over(&self, reference: LoneItemReference) -> (usize, Option<u64>) {
        let over = self
            .lone_estimates
            .iter()
            .copied()
            .filter(|&estimate| estimate > reference.micros);
        (over.clone().count(), over.max())
    }
}

/// One step kind's admitted drains, priced by the install estimate.
#[derive(Debug, Default, Clone)]
pub(super) struct DrainEstimateTally {
    /// Each admitting drain's estimated µs, both resources together.
    drain_estimates: Vec<u64>,
    resources: [ResourceTally; StreamResource::COUNT],
}

impl DrainEstimateTally {
    /// One drain's admitted items, in admission order.
    pub(super) fn add(&mut self, admitted: &[DrainItem]) {
        if admitted.is_empty() {
            return;
        }
        let mut estimates = [None::<u64>; StreamResource::COUNT];
        for item in admitted {
            let index = item.rank.resource().index();
            let estimate = estimates[index].get_or_insert(0);
            *estimate = estimate.saturating_add(item.estimated_install_us());
            self.resources[index].item_bytes.push(item.bytes);
        }
        let mut total = 0u64;
        for (resource, estimate) in self.resources.iter_mut().zip(estimates) {
            if let Some(estimate) = estimate {
                resource.drain_estimates.push(estimate);
                total = total.saturating_add(estimate);
            }
        }
        self.drain_estimates.push(total);
        if let [lone] = admitted {
            self.resources[lone.rank.resource().index()]
                .lone_estimates
                .push(lone.estimated_install_us());
        }
    }

    /// The prose lines, each labelled with `step`.
    pub(super) fn lines(&self, step: &str, reference: LoneItemReference) -> [String; 3] {
        let (max, p95) = max_and_p95(self.drain_estimates.clone());
        let mut estimates = format!(
            "drain install estimate {step} (µs per admitting drain, warm CPU model): all p95 \
             {p95}, max {max}"
        );
        let mut sizes = format!("admitted item sizes {step} (decoded MiB):");
        let mut lone = format!("lone admitted items over {}, {step}:", reference.label());
        for (kind, _, name) in RESOURCES {
            let resource = &self.resources[kind.index()];
            let (max, p95) = max_and_p95(resource.drain_estimates.clone());
            estimates.push_str(&format!(
                "; {name} p95 {p95}, max {max} over {} drains",
                resource.drain_estimates.len()
            ));
            let (count, quantiles) = resource.item_sizes();
            sizes.push_str(&match quantiles {
                Some([p50, p95, max]) => format!(
                    " {name} {count} items p50 {:.3}, p95 {:.3}, max {:.3};",
                    mib(p50),
                    mib(p95),
                    mib(max)
                ),
                None => format!(" {name} none;"),
            });
            let (over, largest) = resource.lone_over(reference);
            lone.push_str(&match largest {
                Some(largest) => format!(" {name} {over} (largest {largest} µs);"),
                None => format!(" {name} 0;"),
            });
        }
        [estimates, sizes, lone].map(|line| line.trim_end_matches(';').to_string())
    }

    /// This tally's result keys, each ending in `suffix`.
    pub(super) fn json(
        &self,
        suffix: &str,
        reference: LoneItemReference,
        into: &mut serde_json::Map<String, Value>,
    ) {
        let mut insert = |key: String, value: Value| {
            into.insert(format!("{key}{suffix}"), value);
        };
        let (max, p95) = max_and_p95(self.drain_estimates.clone());
        insert("drain_estimate_us_p95".to_string(), json!(p95));
        insert("drain_estimate_us_max".to_string(), json!(max));
        for (kind, prefix, _) in RESOURCES {
            let resource = &self.resources[kind.index()];
            let (max, p95) = max_and_p95(resource.drain_estimates.clone());
            let (count, quantiles) = resource.item_sizes();
            let (over, largest) = resource.lone_over(reference);
            for (key, value) in [
                ("drains_admitting", json!(resource.drain_estimates.len())),
                ("drain_estimate_us_p95", json!(p95)),
                ("drain_estimate_us_max", json!(max)),
                ("items_admitted", json!(count)),
                ("item_bytes_p50", json!(quantiles.map(|q| q[0]))),
                ("item_bytes_p95", json!(quantiles.map(|q| q[1]))),
                ("item_bytes_max", json!(quantiles.map(|q| q[2]))),
                ("lone_over_reference", json!(over)),
                ("lone_over_reference_max_us", json!(largest)),
            ] {
                insert(format!("{prefix}_{key}"), value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::levers::{DEFAULT_LEVERS, drain_budget_from};
    use super::*;
    use crate::streaming::drain_budget::{DrainClass, DrainRank, estimated_install_us};

    const MIB: u64 = 1024 * 1024;

    fn sh(cluster_id: u32, bytes: u64) -> DrainItem {
        DrainItem {
            rank: DrainRank::sh(DrainClass::Visible, 0, cluster_id),
            bytes,
        }
    }

    fn pair(block_id: u32, bytes: u64) -> DrainItem {
        DrainItem {
            rank: DrainRank::lightmap_block(DrainClass::Visible, 0, 0, block_id),
            bytes,
        }
    }

    fn keys(
        tally: &DrainEstimateTally,
        reference: LoneItemReference,
    ) -> serde_json::Map<String, Value> {
        let mut keys = serde_json::Map::new();
        tally.json("", reference, &mut keys);
        keys
    }

    // A byte-cap run judges lone items against the 4 ms reference; a time run
    // against its own budget.
    #[test]
    fn the_lone_reference_is_the_time_budget_or_4_ms_in_a_byte_run() {
        let byte_run = LoneItemReference::of(&DEFAULT_LEVERS);
        assert_eq!(byte_run.micros, BYTE_RUN_REFERENCE_US);
        assert!(byte_run.label().contains("byte-budget run"));
        let capped = Levers {
            drain_budget: drain_budget_from(Some("16"), None).unwrap(),
            ..DEFAULT_LEVERS
        };
        assert_eq!(LoneItemReference::of(&capped), byte_run);
        let timed = Levers {
            drain_budget: drain_budget_from(None, Some("6000")).unwrap(),
            ..DEFAULT_LEVERS
        };
        let time_run = LoneItemReference::of(&timed);
        assert_eq!(time_run.micros, 6_000);
        assert!(time_run.label().contains("play time budget"));
    }

    #[test]
    fn drain_estimates_split_by_resource_and_lone_items_count_over_the_reference() {
        let mut tally = DrainEstimateTally::default();
        tally.add(&[]);
        // A mixed drain: two SH clusters and one pair.
        tally.add(&[sh(0, MIB), sh(1, 2 * MIB), pair(7, 4 * MIB)]);
        // Lone items: a 4 MiB SH cluster (4,388 µs) passes 4 ms; a 14 MiB
        // pair (2,745 µs) does not; a 3 MiB SH cluster (3,308 µs) does not.
        tally.add(&[sh(2, 4 * MIB)]);
        tally.add(&[pair(8, 14 * MIB)]);
        tally.add(&[sh(3, 3 * MIB)]);

        let reference = LoneItemReference::of(&DEFAULT_LEVERS);
        let found = keys(&tally, reference);
        let mixed_sh = estimated_install_us(StreamResource::Sh, MIB)
            + estimated_install_us(StreamResource::Sh, 2 * MIB);
        let mixed = mixed_sh + estimated_install_us(StreamResource::LightmapBlock, 4 * MIB);
        assert_eq!(mixed_sh, 68 + 1_080 + 68 + 2 * 1_080);
        assert_eq!(mixed, mixed_sh + 15 + 4 * 195);
        // The lone 4 MiB cluster (4,388 µs) outcosts the mixed drain (4,171).
        assert_eq!(found["drain_estimate_us_max"], json!(68 + 4 * 1_080));
        assert!(mixed < 68 + 4 * 1_080);
        assert_eq!(found["sh_drains_admitting"], json!(3));
        assert_eq!(found["sh_drain_estimate_us_max"], json!(68 + 4 * 1_080));
        assert_eq!(found["lightmap_drains_admitting"], json!(2));
        assert_eq!(
            found["lightmap_drain_estimate_us_max"],
            json!(15 + 14 * 195)
        );

        assert_eq!(found["sh_items_admitted"], json!(4));
        assert_eq!(found["sh_item_bytes_p50"], json!(2 * MIB));
        assert_eq!(found["sh_item_bytes_max"], json!(4 * MIB));
        assert_eq!(found["lightmap_items_admitted"], json!(2));
        assert_eq!(found["lightmap_item_bytes_p50"], json!(4 * MIB));

        assert_eq!(found["sh_lone_over_reference"], json!(1));
        assert_eq!(
            found["sh_lone_over_reference_max_us"],
            json!(68 + 4 * 1_080)
        );
        assert_eq!(found["lightmap_lone_over_reference"], json!(0));
        assert_eq!(found["lightmap_lone_over_reference_max_us"], Value::Null);

        // At a 2.5 ms time budget, the 14 MiB pair and the 3 MiB cluster
        // pass too.
        let timed = Levers {
            drain_budget: drain_budget_from(None, Some("2500")).unwrap(),
            ..DEFAULT_LEVERS
        };
        let found = keys(&tally, LoneItemReference::of(&timed));
        assert_eq!(found["sh_lone_over_reference"], json!(2));
        assert_eq!(found["lightmap_lone_over_reference"], json!(1));
    }

    #[test]
    fn an_empty_tally_reports_zero_drains_and_null_sizes() {
        let found = keys(
            &DrainEstimateTally::default(),
            LoneItemReference::of(&DEFAULT_LEVERS),
        );
        assert_eq!(found["drain_estimate_us_max"], json!(0));
        assert_eq!(found["sh_items_admitted"], json!(0));
        assert_eq!(found["sh_item_bytes_p95"], Value::Null);
        assert_eq!(found["lightmap_lone_over_reference"], json!(0));
    }
}
