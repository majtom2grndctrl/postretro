//! Per-drain admission tallies for the drain-budget spike: same-class cuts and the largest drains.
//! See: context/lib/rendering_pipeline.md §4 (shared SH+lightmap drain budget)

use serde_json::{Value, json};

use super::report::{max_and_p95, mib};
use crate::streaming::drain_budget::DrainItem;
use crate::streaming::request::StreamResource;
use crate::streaming::shared_drain::SharedDrain;

/// How many of a run's largest drains the report lists.
const LARGEST_DRAINS: usize = 5;

/// Whether a drain's budget cut admits an item of one resource and leaves
/// behind a same-class item of the other. `items` are in admission order and
/// the first `admitted` installed. Admission is a prefix and orders by class
/// first, so only the class of the first item left behind can hold both
/// sides of the cut: the drain counts once when that class's admitted part
/// holds a resource its left-behind part holds the other of, wherever inside
/// the class the cut falls. A drain with no items, with everything admitted,
/// with one resource only, or cut at a class boundary counts nothing.
pub(super) fn same_class_cut(items: &[DrainItem], admitted: usize) -> bool {
    let Some(first_left) = items.get(admitted) else {
        return false;
    };
    let class = first_left.rank.class();
    let in_class = |item: &&DrainItem| item.rank.class() == class;
    let admitted_side = resources(items[..admitted].iter().rev().take_while(in_class));
    let left_side = resources(items[admitted..].iter().take_while(in_class));
    (admitted_side.sh && left_side.lightmap) || (admitted_side.lightmap && left_side.sh)
}

#[derive(Debug, Default, Clone, Copy)]
struct Resources {
    sh: bool,
    lightmap: bool,
}

fn resources<'a>(items: impl Iterator<Item = &'a DrainItem>) -> Resources {
    let mut held = Resources::default();
    for item in items {
        match item.rank.resource() {
            StreamResource::Sh => held.sh = true,
            StreamResource::LightmapBlock => held.lightmap = true,
        }
    }
    held
}

/// One drain's admission: decoded bytes admitted per resource, items
/// admitted, and whether its cut split a class across resources.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct DrainSample {
    pub(super) sh_bytes: u64,
    pub(super) lightmap_bytes: u64,
    pub(super) items: u32,
    pub(super) same_class_cut: bool,
}

impl DrainSample {
    /// Reads `drain` after its admission.
    pub(super) fn of(drain: &SharedDrain) -> Self {
        let sh = drain.admission(StreamResource::Sh);
        let lightmap = drain.admission(StreamResource::LightmapBlock);
        let (items, admitted) = drain.ordered_items();
        Self {
            sh_bytes: sh.bytes,
            lightmap_bytes: lightmap.bytes,
            items: u32::try_from(admitted).unwrap_or(u32::MAX),
            same_class_cut: same_class_cut(items, admitted),
        }
    }

    pub(super) fn total_bytes(&self) -> u64 {
        self.sh_bytes + self.lightmap_bytes
    }

    fn json(&self) -> Value {
        json!({
            "bytes": self.total_bytes(),
            "sh_bytes": self.sh_bytes,
            "lightmap_bytes": self.lightmap_bytes,
            "items": self.items,
        })
    }
}

/// One step kind's drains: how many ran, how many admitted anything, the
/// same-class cuts, and every admitting drain's sample.
#[derive(Debug, Default, Clone)]
pub(super) struct DrainTally {
    drains: u64,
    same_class_cuts: u64,
    admitting: Vec<DrainSample>,
}

impl DrainTally {
    pub(super) fn add(&mut self, sample: DrainSample) {
        self.drains += 1;
        self.same_class_cuts += u64::from(sample.same_class_cut);
        if sample.items > 0 {
            self.admitting.push(sample);
        }
    }

    /// The largest admitting drains by total admitted bytes, largest first;
    /// ties keep run order.
    fn largest(&self) -> Vec<DrainSample> {
        let mut sorted = self.admitting.clone();
        sorted.sort_by_key(|sample| std::cmp::Reverse(sample.total_bytes()));
        sorted.truncate(LARGEST_DRAINS);
        sorted
    }

    /// Max and p95 of admitted bytes over the drains that admitted anything.
    fn max_and_p95(&self) -> (u64, u64) {
        max_and_p95(
            self.admitting
                .iter()
                .map(DrainSample::total_bytes)
                .collect(),
        )
    }

    pub(super) fn line(&self) -> String {
        let (max, p95) = self.max_and_p95();
        let largest: Vec<String> = self
            .largest()
            .iter()
            .map(|sample| {
                format!(
                    "{:.2} (SH {:.2}, lightmap {:.2}; {} items)",
                    mib(sample.total_bytes()),
                    mib(sample.sh_bytes),
                    mib(sample.lightmap_bytes),
                    sample.items,
                )
            })
            .collect();
        format!(
            "{} drains ({} admitting); same-class cuts {}; admitted MiB per admitting drain p95 \
             {:.2}, max {:.2}; largest MiB: {}",
            self.drains,
            self.admitting.len(),
            self.same_class_cuts,
            mib(p95),
            mib(max),
            if largest.is_empty() {
                "none".to_string()
            } else {
                largest.join(", ")
            },
        )
    }

    /// This tally's result keys, each ending in `suffix`.
    pub(super) fn json(&self, suffix: &str, into: &mut serde_json::Map<String, Value>) {
        let (max, p95) = self.max_and_p95();
        let largest: Vec<Value> = self.largest().iter().map(DrainSample::json).collect();
        for (key, value) in [
            ("drains", json!(self.drains)),
            ("drains_admitting", json!(self.admitting.len())),
            ("same_class_cuts", json!(self.same_class_cuts)),
            ("drain_admitted_bytes_p95", json!(p95)),
            ("drain_admitted_bytes_max", json!(max)),
            ("largest_drains", Value::Array(largest)),
        ] {
            into.insert(format!("{key}{suffix}"), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::drain_budget::{
        DrainClass, DrainRank, MAX_INSTALL_DECODED_BYTES_PER_DRAIN, admit_drain,
    };

    const MIB: u64 = 1024 * 1024;

    fn sh(class: DrainClass, cluster_id: u32, bytes: u64) -> DrainItem {
        DrainItem {
            rank: DrainRank::sh(class, 0, cluster_id),
            bytes,
        }
    }

    fn pair(class: DrainClass, block_id: u32, bytes: u64) -> DrainItem {
        DrainItem {
            rank: DrainRank::lightmap_block(class, 0, 0, block_id),
            bytes,
        }
    }

    /// Admits `items` at the production play cap and reports the cut.
    fn cut(mut items: Vec<DrainItem>) -> (usize, bool) {
        let admitted = admit_drain(&mut items, MAX_INSTALL_DECODED_BYTES_PER_DRAIN)
            .unwrap()
            .admitted;
        (admitted, same_class_cut(&items, admitted))
    }

    // P1: the cut falls between two SH Visible items, and a lightmap Visible
    // pair waits behind it: an SH item of that class installed while a
    // lightmap item of it waits.
    #[test]
    fn a_cut_inside_a_class_between_two_sh_items_counts_once() {
        let visible = DrainClass::Visible;
        let (admitted, counted) = cut(vec![
            pair(visible, 7, MIB),
            sh(visible, 0, 3 * MIB),
            sh(visible, 1, 3 * MIB),
            sh(visible, 2, 3 * MIB),
        ]);
        assert_eq!(admitted, 2, "the cut falls after the second SH chunk");
        assert!(counted);
    }

    // P2: the first-item rule admits an oversized SH chunk alone, ahead of a
    // same-class lightmap pair: counted. An oversized lightmap pair with no
    // same-class SH item: not counted.
    #[test]
    fn an_oversized_first_item_counts_only_with_the_other_resource_in_its_class() {
        let visible = DrainClass::Visible;
        let (admitted, counted) = cut(vec![sh(visible, 0, 9 * MIB), pair(visible, 7, MIB)]);
        assert_eq!(admitted, 1);
        assert!(counted, "SH installs while the lightmap pair waits");

        let (admitted, counted) = cut(vec![
            pair(visible, 7, 9 * MIB),
            pair(visible, 8, MIB),
            sh(DrainClass::Lead, 0, MIB),
        ]);
        assert_eq!(admitted, 1);
        assert!(!counted, "no same-class SH item waits");
    }

    // P3: nothing offered, one resource only, a cut at a class boundary, and
    // everything admitted all count nothing.
    #[test]
    fn empty_single_resource_class_boundary_and_whole_drains_count_nothing() {
        assert_eq!(cut(Vec::new()), (0, false));

        let (admitted, counted) = cut(vec![
            sh(DrainClass::Visible, 0, 5 * MIB),
            sh(DrainClass::Visible, 1, 5 * MIB),
        ]);
        assert_eq!(admitted, 1);
        assert!(!counted, "SH alone, cut inside its class");

        let (admitted, counted) = cut(vec![
            sh(DrainClass::Visible, 0, 5 * MIB),
            pair(DrainClass::Lead, 7, 5 * MIB),
        ]);
        assert_eq!(admitted, 1);
        assert!(!counted, "the cut falls between Visible and Lead");

        let (admitted, counted) = cut(vec![
            sh(DrainClass::Visible, 0, MIB),
            pair(DrainClass::Visible, 7, MIB),
        ]);
        assert_eq!(admitted, 2);
        assert!(!counted, "everything admitted");
    }

    // P4: the renderer still holds the previous lightmap batch, so the
    // lightmap side offers nothing; SH cut inside a class is one resource.
    #[test]
    fn a_drain_the_lightmap_side_sat_out_counts_nothing() {
        let mut drain = SharedDrain::default();
        drain.begin();
        for cluster_id in 0..3 {
            drain.offer(sh(DrainClass::Visible, cluster_id, 3 * MIB));
        }
        drain.admit().unwrap();
        let sample = DrainSample::of(&drain);
        assert_eq!(sample.items, 2);
        assert!(!sample.same_class_cut);
    }

    #[test]
    fn a_drain_sample_splits_admitted_bytes_by_resource() {
        let mut drain = SharedDrain::default();
        drain.begin();
        drain.offer(pair(DrainClass::Visible, 7, 2 * MIB));
        drain.offer(sh(DrainClass::Visible, 0, 3 * MIB));
        drain.offer(sh(DrainClass::Visible, 1, 4 * MIB));
        drain.admit().unwrap();
        assert_eq!(
            DrainSample::of(&drain),
            DrainSample {
                sh_bytes: 7 * MIB,
                lightmap_bytes: 0,
                items: 2,
                same_class_cut: true,
            }
        );
    }

    #[test]
    fn the_tally_keeps_admitting_drains_and_lists_the_largest_first() {
        let mut tally = DrainTally::default();
        tally.add(DrainSample::default());
        for mib_bytes in 1..=7 {
            tally.add(DrainSample {
                sh_bytes: mib_bytes * MIB,
                lightmap_bytes: MIB,
                items: 2,
                same_class_cut: mib_bytes == 3,
            });
        }
        let mut keys = serde_json::Map::new();
        tally.json("_spawn", &mut keys);
        assert_eq!(keys["drains_spawn"], json!(8));
        assert_eq!(keys["drains_admitting_spawn"], json!(7));
        assert_eq!(keys["same_class_cuts_spawn"], json!(1));
        assert_eq!(keys["drain_admitted_bytes_max_spawn"], json!(8 * MIB));
        let largest: Vec<u64> = tally
            .largest()
            .iter()
            .map(DrainSample::total_bytes)
            .collect();
        assert_eq!(largest, [8, 7, 6, 5, 4].map(|m| m * MIB));
    }
}
