// The lights a mover gates only at a mid pose: the gate's one count beside
// its per-terminus rows. The rows stay terminus-only, since they size the
// world-surface channel, which picks its own state spacing; the mover probe
// channel's direct half is what bakes a mid pose's q, so it supplies these.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::collections::BTreeMap;

/// Per mover, the baked lights the probe channel gates at some mid pose and
/// at neither terminus: q below 255 at a mid pose for some probe, and 255 at
/// both termini for every probe. Movers ascend by id, and each mover's
/// lights by source index (authored entity order), so the report's text is
/// the same however the bake ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MidPoseOnlyLights {
    by_mover: BTreeMap<u32, Vec<usize>>,
}

impl MidPoseOnlyLights {
    /// From `(mover id, light source index)` pairs, in any order; a repeated
    /// pair counts once.
    pub(crate) fn from_pairs(pairs: impl IntoIterator<Item = (u32, usize)>) -> Self {
        let mut by_mover: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (mover_id, source_index) in pairs {
            by_mover.entry(mover_id).or_default().push(source_index);
        }
        for lights in by_mover.values_mut() {
            lights.sort_unstable();
            lights.dedup();
        }
        Self { by_mover }
    }

    /// Each mover with such a light, ascending by id, with its lights.
    pub(crate) fn movers(&self) -> impl Iterator<Item = (u32, &[usize])> {
        self.by_mover
            .iter()
            .map(|(&mover_id, lights)| (mover_id, lights.as_slice()))
    }

    /// Mover `mover_id`'s count.
    #[cfg(test)]
    pub(crate) fn count(&self, mover_id: u32) -> usize {
        self.by_mover.get(&mover_id).map_or(0, Vec::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_group_by_mover_in_id_order_with_each_light_once() {
        let lights = MidPoseOnlyLights::from_pairs([(7, 4), (2, 9), (7, 1), (7, 4)]);
        let movers: Vec<(u32, Vec<usize>)> = lights
            .movers()
            .map(|(mover, lights)| (mover, lights.to_vec()))
            .collect();
        assert_eq!(movers, [(2, vec![9]), (7, vec![1, 4])]);
        assert_eq!((lights.count(7), lights.count(3)), (2, 0));
    }
}
