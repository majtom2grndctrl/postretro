// The gate's three per-light memos (static texels, probes, animated texels):
// key pieces, the row payload, and hit/miss logging.
// See: context/lib/build_pipeline.md §Build Cache

use glam::DVec3;

use super::pose::{MoverPoses, Terminus};
use super::tally::RowTally;
use super::texels::CandidateMover;
use crate::cache::{CacheKey, StageCache};
use crate::map_data::MapLight;
use crate::partition::Aabb;

/// Folded into every gate memo key. Bump on any change to the test all three
/// tiers share: posing, the segment and point tests and their tolerances,
/// the influence prefilter, or the hard-ray target a segment ends on.
pub(crate) const GATE_EPOCH: u32 = 2;

/// One gate memo's cache stage. Bump `version` when its payload or its
/// tier's own gate changes; [`GATE_EPOCH`] covers what the tiers share.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MemoStage {
    pub(crate) id: &'static str,
    pub(crate) version: u32,
}

/// Static texel tallies, per layer light, inside the lightmap stage.
pub(crate) const TEXEL_MEMO: MemoStage = MemoStage {
    id: "mover_light_texels",
    version: 1,
};
/// Probe tallies, per analysed light, at the probe gate. Its reach test
/// borrows the SH bakes' `sh_bake::light_reaches_point` and
/// `spot_cone_parameters`, which no key names: a change to either that can
/// move a reached probe must bump this version.
pub(crate) const PROBE_MEMO: MemoStage = MemoStage {
    id: "mover_light_probes",
    version: 1,
};
/// Animated texel tallies, per animated light, after the weight maps.
pub(crate) const ANIMATED_MEMO: MemoStage = MemoStage {
    id: "mover_light_animated",
    version: 1,
};

/// One light's rows in one tier, keyed by mover id rather than position, so
/// a memo survives an edit that renumbers the analysed list but keeps every
/// candidate's id and pose. Ascending by (mover id, terminus).
pub(crate) type LightRows = Vec<(u32, Terminus, RowTally)>;

/// A hasher holding the gate epoch: the start of every gate memo key.
pub(crate) fn gate_key_hasher() -> blake3::Hasher {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&GATE_EPOCH.to_le_bytes());
    hasher
}

/// Fold one light's own key piece: the whole light, as the lightmap layer key
/// folds it (a field the gate never reads only re-gates that one light), then
/// each candidate mover's id and, per terminus, the light's inside tag there,
/// with the posed brushes only when its influence reaches that pose. The gate
/// never reads a pose the influence misses, so moving one terminus re-gates
/// only the lights reaching its old or new pose, and an edit to a mover that
/// is no candidate leaves this key alone.
pub(crate) fn fold_gate_light(
    hasher: &mut blake3::Hasher,
    light: &MapLight,
    poses: &MoverPoses,
    candidates: &[CandidateMover],
) {
    let encoded = postcard::to_allocvec(light).expect("postcard serialize MapLight for gate key");
    hasher.update(&(encoded.len() as u32).to_le_bytes());
    hasher.update(&encoded);
    hasher.update(&(candidates.len() as u32).to_le_bytes());
    for candidate in candidates {
        let mover = &poses.analysed[candidate.mover];
        hasher.update(&mover.mover_id.to_le_bytes());
        for terminus in Terminus::BOTH {
            let inside = candidate.inside[terminus.index()];
            hasher.update(&[match inside {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            }]);
            if inside.is_none() {
                continue;
            }
            let pose = mover.pose(terminus);
            fold_aabb(hasher, &pose.bounds);
            hasher.update(&(pose.brushes.len() as u32).to_le_bytes());
            for brush in &pose.brushes {
                fold_aabb(hasher, &brush.bounds);
                hasher.update(&(brush.planes.len() as u32).to_le_bytes());
                for &(normal, distance) in &brush.planes {
                    fold_dvec3(hasher, normal);
                    hasher.update(&distance.to_bits().to_le_bytes());
                }
            }
        }
    }
}

fn fold_dvec3(hasher: &mut blake3::Hasher, v: DVec3) {
    for component in v.to_array() {
        hasher.update(&component.to_bits().to_le_bytes());
    }
}

fn fold_aabb(hasher: &mut blake3::Hasher, bounds: &Aabb) {
    fold_dvec3(hasher, bounds.min);
    fold_dvec3(hasher, bounds.max);
}

/// A gate memo over one stage cache.
pub(crate) struct GateMemo<'c> {
    cache: &'c StageCache,
    stage: MemoStage,
}

impl<'c> GateMemo<'c> {
    pub(crate) fn new(cache: &'c StageCache, stage: MemoStage) -> Self {
        Self { cache, stage }
    }

    pub(crate) fn key(&self, hasher: &blake3::Hasher) -> CacheKey {
        CacheKey::new(
            self.stage.id,
            self.stage.version,
            hasher.finalize().as_bytes(),
        )
    }

    /// The rows stored under `key`, or `None` on a miss. An entry naming a
    /// mover outside `candidates`, or out of order, is corrupt: a miss that
    /// re-gates. Runs outside any governor permit.
    pub(crate) fn get(
        &self,
        key: &CacheKey,
        poses: &MoverPoses,
        candidates: &[CandidateMover],
    ) -> Option<LightRows> {
        let bytes = self.cache.get(key)?;
        let rows = decode_rows(&bytes).filter(|rows| {
            rows.iter().all(|(mover_id, _, _)| {
                candidates
                    .iter()
                    .any(|candidate| poses.analysed[candidate.mover].mover_id == *mover_id)
            })
        });
        if rows.is_none() {
            log::warn!("[cache] corrupt {} entry, re-gating", self.stage.id);
        }
        rows
    }

    /// Store `rows`, empty included, so an unchanged light hits next build.
    /// Runs outside any governor permit.
    pub(crate) fn put(&self, key: &CacheKey, rows: &LightRows) {
        self.cache.put(key, &encode_rows(rows));
    }

    /// One line per pass: `hit` when every probed light hit, else `miss`
    /// with the share that re-gates.
    pub(crate) fn log_outcome(&self, probed: usize, hits: usize) {
        if probed == 0 {
            return;
        }
        if hits == probed {
            log::info!("[cache] {} hit ({probed} light(s))", self.stage.id);
        } else {
            log::info!(
                "[cache] {} miss ({} of {probed} light(s) re-gate)",
                self.stage.id,
                probed - hits
            );
        }
    }
}

/// Bytes per row with both bounds absent: mover id, terminus, inside flag,
/// six counts, and a presence byte per bound.
const MIN_ROW_BYTES: usize = 4 + 1 + 1 + 6 * 8 + 2;
/// Bytes a present bound adds: six `f64`.
const BOUNDS_BYTES: usize = 6 * 8;

pub(crate) fn encode_rows(rows: &LightRows) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + rows.len() * (MIN_ROW_BYTES + 2 * BOUNDS_BYTES));
    out.extend_from_slice(&(rows.len() as u32).to_le_bytes());
    for (mover_id, terminus, row) in rows {
        out.extend_from_slice(&mover_id.to_le_bytes());
        out.push(terminus.index() as u8);
        out.push(u8::from(row.light_inside));
        for count in [
            row.gated_texels,
            row.shared_texels,
            row.touched_blocks,
            row.differing_blocks,
            row.gated_probes,
            row.shared_probes,
        ] {
            out.extend_from_slice(&count.to_le_bytes());
        }
        for bounds in [&row.texel_centres, &row.probe_centres] {
            match bounds {
                None => out.push(0),
                Some(bounds) => {
                    out.push(1);
                    for component in bounds
                        .min
                        .to_array()
                        .into_iter()
                        .chain(bounds.max.to_array())
                    {
                        out.extend_from_slice(&component.to_bits().to_le_bytes());
                    }
                }
            }
        }
    }
    out
}

/// The rows of a well-formed payload: exact length, known termini, shared
/// counts within gated, and rows strictly ascending by (mover id, terminus).
pub(crate) fn decode_rows(bytes: &[u8]) -> Option<LightRows> {
    let mut reader = Reader(bytes);
    let count = reader.u32()? as usize;
    if count.checked_mul(MIN_ROW_BYTES)? > reader.0.len() {
        return None;
    }
    let mut rows: LightRows = Vec::with_capacity(count);
    for _ in 0..count {
        let mover_id = reader.u32()?;
        let terminus = match reader.u8()? {
            0 => Terminus::First,
            1 => Terminus::Last,
            _ => return None,
        };
        let light_inside = match reader.u8()? {
            0 => false,
            1 => true,
            _ => return None,
        };
        let [
            gated_texels,
            shared_texels,
            touched_blocks,
            differing_blocks,
            gated_probes,
            shared_probes,
        ] = [(); 6].map(|()| reader.u64());
        let mut bounds = [(); 2].map(|()| reader.bounds());
        let row = RowTally {
            gated_texels: gated_texels?,
            shared_texels: shared_texels?,
            light_inside,
            texel_centres: bounds[0].take()?,
            touched_blocks: touched_blocks?,
            differing_blocks: differing_blocks?,
            gated_probes: gated_probes?,
            shared_probes: shared_probes?,
            probe_centres: bounds[1].take()?,
        };
        let ordered = rows
            .last()
            .is_none_or(|(id, at, _)| (*id, *at) < (mover_id, terminus));
        if !ordered
            || row.shared_texels > row.gated_texels
            || row.shared_probes > row.gated_probes
            || row.differing_blocks > row.touched_blocks
        {
            return None;
        }
        rows.push((mover_id, terminus, row));
    }
    reader.0.is_empty().then_some(rows)
}

struct Reader<'b>(&'b [u8]);

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (head, rest) = self.0.split_first_chunk::<N>()?;
        self.0 = rest;
        Some(*head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|[byte]| byte)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_le_bytes)
    }

    fn f64(&mut self) -> Option<f64> {
        self.take()
            .map(|bytes| f64::from_bits(u64::from_le_bytes(bytes)))
    }

    /// `Some(None)` for an absent bound, `None` for a malformed one.
    fn bounds(&mut self) -> Option<Option<Aabb>> {
        match self.u8()? {
            0 => Some(None),
            1 => {
                let mut component = || self.f64();
                let min = DVec3::new(component()?, component()?, component()?);
                let max = DVec3::new(component()?, component()?, component()?);
                Some(Some(Aabb { min, max }))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(gated: u64, shared: u64, centres: Option<Aabb>) -> RowTally {
        RowTally {
            gated_texels: gated,
            shared_texels: shared,
            light_inside: gated.is_multiple_of(2),
            texel_centres: centres,
            touched_blocks: 3,
            differing_blocks: 1,
            gated_probes: 7,
            shared_probes: 2,
            probe_centres: None,
        }
    }

    fn render(rows: &LightRows) -> String {
        format!("{rows:?}")
    }

    #[test]
    fn rows_round_trip_and_reject_malformed_payloads() {
        let bounds = Aabb {
            min: DVec3::new(-1.5, 0.25, 3.0),
            max: DVec3::new(2.0, 0.5, 4.125),
        };
        let rows: LightRows = vec![
            (2, Terminus::First, row(10, 4, Some(bounds))),
            (2, Terminus::Last, row(5, 4, None)),
            (9, Terminus::First, row(0, 0, None)),
        ];
        let bytes = encode_rows(&rows);
        assert_eq!(bytes.len(), 4 + rows.len() * MIN_ROW_BYTES + BOUNDS_BYTES);
        assert_eq!(
            render(&decode_rows(&bytes).expect("round trip")),
            render(&rows)
        );
        assert_eq!(
            render(&decode_rows(&encode_rows(&Vec::new())).unwrap()),
            "[]"
        );

        assert!(decode_rows(&bytes[..bytes.len() - 1]).is_none(), "short");
        assert!(
            decode_rows(&[bytes.clone(), vec![0]].concat()).is_none(),
            "long"
        );
        let mut shared_over = rows.clone();
        shared_over[0].2.shared_texels = 11;
        assert!(decode_rows(&encode_rows(&shared_over)).is_none());
        let mut unordered = rows.clone();
        unordered.swap(0, 1);
        assert!(decode_rows(&encode_rows(&unordered)).is_none());
        let mut terminus = bytes.clone();
        terminus[8] = 2;
        assert!(decode_rows(&terminus).is_none(), "unknown terminus");
    }
}
