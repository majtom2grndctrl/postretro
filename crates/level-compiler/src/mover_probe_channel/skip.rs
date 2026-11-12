// Redundant mid-pose skipping: a mid pose stores no bounce or depth state
// for a probe where compose, reading the mover's knots through its author
// ramp and the states left after earlier skips, already lands within that
// probe's bars at the pose's t′. A skipped pose blends across.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)
//
// IP design rules (ip-372 §Design levers 2, 7; ip-check §Piece 7): skipping
// is storage sparsity. A skipped pose composes as the blend of its
// neighbouring stored or base-reading states; it never zeroes a weight,
// flags or excludes a probe. Direct q is never skipped.

use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::mover_probe_channel::{
    DepthSource, Knot, MOVER_IRRADIANCE_TILE_F16_COUNT, PoseInterval, PoseMark, depth_segment,
    pose_interval,
};
use postretro_level_format::probe_depth_tiles::{PROBE_DEPTH_TILE_TEXELS, f16_bits_to_f32};
use postretro_level_format::sh_volume::{OctahedralAtlasTexel, OctahedralShVolumeSection};
use rayon::prelude::*;

use super::depth::{DEPTH_SIGNIFICANCE_FLOOR, MoverAbsentDepth};
use super::domain::{clears_bounce_bars, compact_tile, round_trip_error};
use super::states::{BounceProbe, DepthProbe, MoverBounceStates};

/// What the skip test reads beside a mover's own states: id 34 (each
/// probe's bounce base and its round-trip bar) and the mover-absent depth
/// (each probe's depth base and the significance score).
pub(crate) struct SkipBars<'a> {
    base: &'a OctahedralShVolumeSection,
    /// Dense probe → compact slot in `base`.
    slots: Vec<u32>,
    uncompressed: bool,
    depth: MoverAbsentDepth<'a>,
}

impl<'a> SkipBars<'a> {
    /// `depth` is id 59 as the base bake selected it, before mover tiles
    /// reserve their range: the base the significance floor reads.
    pub(crate) fn new(
        base: &'a OctahedralShVolumeSection,
        depth: MoverAbsentDepth<'a>,
        uncompressed: bool,
    ) -> Self {
        let mut slot = 0u32;
        let slots = base
            .probes
            .iter()
            .map(|probe| {
                let at = slot;
                slot += u32::from(probe.validity != 0);
                at
            })
            .collect();
        Self {
            base,
            slots,
            uncompressed,
            depth,
        }
    }
}

/// (probe, mid pose) states one mover keeps and skips, per block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SkipCounts {
    pub(crate) bounce_stored: usize,
    pub(crate) bounce_skipped: usize,
    pub(crate) depth_stored: usize,
    pub(crate) depth_skipped: usize,
    /// Of the skipped: states at poses the ramp squeezes out, never read.
    pub(crate) squeezed: usize,
}

impl SkipCounts {
    fn add(&mut self, other: Self) {
        self.bounce_stored += other.bounce_stored;
        self.bounce_skipped += other.bounce_skipped;
        self.depth_stored += other.depth_stored;
        self.depth_skipped += other.depth_skipped;
        self.squeezed += other.squeezed;
    }
}

/// Skip what compose can do without in one probe's `marks` (one per pose),
/// read through `knots` (the mover's effective knots, after its ramp).
///
/// A stored mid pose the ramp squeezes out is never read, so it skips.
/// Then, from knot 0, each interval reaches as far as it can: its far end is
/// the farthest later knot, up to the next one that must stay defined (a
/// terminus, or a mid pose reading the base), such that with every stored
/// pose between them blended across, compose at each of those poses' t′
/// lands within that pose's bars. `within(pose, span)` judges compose
/// blending `span` at `pose`'s t′ against `pose`'s own state. The stored
/// poses between are skipped, and the next interval starts at the far end,
/// which stays defined. So every skipped pose is checked against the very
/// interval compose reads once every skip is applied, and a run of adjacent
/// skips never adds up past the bars. Termini and poses that read the base
/// are never skipped. Returns how many stored poses the ramp squeezed out.
fn skip_marks(
    marks: &mut [PoseMark],
    knots: &[Knot],
    within: impl Fn(u32, PoseInterval) -> bool,
) -> usize {
    let count = marks.len();
    let mut squeezed = 0;
    let mut read = vec![false; count];
    for knot in knots {
        read[knot.pose as usize] = true;
    }
    for pose in 1..count.saturating_sub(1) {
        if !read[pose] && marks[pose] == PoseMark::Stored {
            marks[pose] = PoseMark::Unmarked;
            squeezed += 1;
        }
    }
    let last = knots.len() - 1;
    let mark = |marks: &[PoseMark], k: usize| marks[knots[k].pose as usize];
    let mut lo = 0;
    while lo < last {
        // Knots up to the next that must stay defined; a stored one may end
        // the interval or be skipped, an unmarked one is blended across
        // already.
        let reach = (lo + 1..=last)
            .find(|&k| k == last || mark(marks, k) == PoseMark::Base)
            .expect("the last knot ends every reach");
        let mut best = None;
        for hi in lo + 1..=reach {
            if hi < reach && mark(marks, hi) != PoseMark::Stored {
                continue;
            }
            let between: Vec<usize> = (lo + 1..hi)
                .filter(|&k| mark(marks, k) == PoseMark::Stored)
                .collect();
            for &k in &between {
                marks[knots[k].pose as usize] = PoseMark::Unmarked;
            }
            let holds = between.iter().all(|&k| {
                let span = pose_interval(knots, knots[k].t, |pose| {
                    marks[pose as usize] != PoseMark::Unmarked
                });
                within(knots[k].pose, span)
            });
            for &k in &between {
                marks[knots[k].pose as usize] = PoseMark::Stored;
            }
            if holds {
                best = Some((hi, between));
            }
        }
        let (hi, between) = best.expect("the next defined knot always holds");
        for k in between {
            marks[knots[k].pose as usize] = PoseMark::Unmarked;
        }
        lo = hi;
    }
    squeezed
}

/// Each pose's stored tile among `tiles` (stored poses' tiles in pose
/// order, `stride` values each), `None` where the pose stores none.
fn stored_tiles<'t, T>(marks: &[PoseMark], tiles: &'t [T], stride: usize) -> Vec<Option<&'t [T]>> {
    let mut chunks = tiles.chunks_exact(stride);
    marks
        .iter()
        .map(|&mark| {
            (mark == PoseMark::Stored).then(|| chunks.next().expect("a tile per stored pose"))
        })
        .collect()
}

/// The tiles of the poses `marks` still stores, in pose order.
fn kept_tiles<T: Copy>(marks: &[PoseMark], by_pose: &[Option<&[T]>]) -> Vec<T> {
    marks
        .iter()
        .zip(by_pose)
        .filter(|(mark, _)| **mark == PoseMark::Stored)
        .flat_map(|(_, tile)| tile.expect("a stored pose kept its tile").iter().copied())
        .collect()
}

/// Mid poses `marks` stores a state at (a mover of more than two poses).
fn mid_stored(marks: &[PoseMark]) -> usize {
    marks[1..marks.len() - 1]
        .iter()
        .filter(|&&mark| mark == PoseMark::Stored)
        .count()
}

/// One compact RGB tile (id 27's format) as texels.
fn texels(halves: &[u16]) -> Vec<OctahedralAtlasTexel> {
    halves
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&[r, g, b]| OctahedralAtlasTexel {
            rgba: [r, g, b, f32_to_f16_bits(1.0)],
        })
        .collect()
}

/// Compose's bounce blend of two tiles (`mb_entry_blend`, then WGSL `mix`
/// per texel and channel), as f16 texels.
fn mix_tiles(
    a: &[OctahedralAtlasTexel],
    b: &[OctahedralAtlasTexel],
    weight: f32,
) -> Vec<OctahedralAtlasTexel> {
    let value = |texel: &OctahedralAtlasTexel, c: usize| f16_bits_to_f32(texel.rgba[c]);
    a.iter()
        .zip(b)
        .map(|(a, b)| OctahedralAtlasTexel {
            rgba: [0, 1, 2, 3].map(|c| {
                if c == 3 {
                    f32_to_f16_bits(1.0)
                } else {
                    f32_to_f16_bits(value(a, c) * (1.0 - weight) + value(b, c) * weight)
                }
            }),
        })
        .collect()
}

/// One affected probe after skipping, or `None` when it stores no state
/// any more: it then reads id 34 at every pose, which every skipped pose's
/// test already held to its bars.
fn skip_bounce(
    mut probe: BounceProbe,
    knots: &[Knot],
    bars: &SkipBars<'_>,
) -> (Option<BounceProbe>, SkipCounts) {
    let before = mid_stored(&probe.marks);
    if before == 0 {
        return (Some(probe), SkipCounts::default());
    }
    let tiles = std::mem::take(&mut probe.tiles);
    let by_pose = stored_tiles(&probe.marks, &tiles, MOVER_IRRADIANCE_TILE_F16_COUNT);
    let states: Vec<Option<Vec<OctahedralAtlasTexel>>> =
        by_pose.iter().map(|tile| tile.map(texels)).collect();
    let base = compact_tile(bars.base, bars.slots[probe.probe as usize] as usize);
    let bar = round_trip_error(&base, bars.base.tile_dimension as usize, bars.uncompressed);
    let state = |pose: u32| states[pose as usize].as_deref().unwrap_or(&base);
    let squeezed = skip_marks(&mut probe.marks, knots, |pose, span| {
        let composed = mix_tiles(state(span.lo), state(span.hi), span.weight);
        !clears_bounce_bars(&composed, state(pose), bar)
    });
    probe.tiles = kept_tiles(&probe.marks, &by_pose);
    let after = mid_stored(&probe.marks);
    let counts = SkipCounts {
        bounce_stored: after,
        bounce_skipped: before - after,
        squeezed,
        ..SkipCounts::default()
    };
    let stores = probe.marks.contains(&PoseMark::Stored);
    (stores.then_some(probe), counts)
}

/// One probe's depth after skipping, or `None` when it stores no tile any
/// more (every stored pose was squeezed out by the ramp): its live tile then
/// reads its base at every t′, so it carries no depth.
fn skip_depth(
    mut probe: DepthProbe,
    knots: &[Knot],
    bars: &SkipBars<'_>,
) -> (Option<DepthProbe>, SkipCounts) {
    let before = mid_stored(&probe.marks);
    if before == 0 {
        return (Some(probe), SkipCounts::default());
    }
    let tiles = std::mem::take(&mut probe.tiles);
    let by_pose = stored_tiles(&probe.marks, &tiles, PROBE_DEPTH_TILE_TEXELS);
    // Each tile as the shared blend reads it: its texels' means, then their
    // second moments.
    let split = |pairs: &mut dyn Iterator<Item = [f32; 2]>| -> Vec<f32> {
        let pairs: Vec<[f32; 2]> = pairs.collect();
        pairs
            .iter()
            .map(|m| m[0])
            .chain(pairs.iter().map(|m| m[1]))
            .collect()
    };
    let base = split(&mut bars.depth.base_moments(probe.probe).into_iter());
    let moments: Vec<Option<Vec<f32>>> = by_pose
        .iter()
        .map(|tile| {
            tile.map(|tile| {
                split(
                    &mut tile
                        .iter()
                        .map(|&[mean, mean_sq]| [f16_bits_to_f32(mean), f16_bits_to_f32(mean_sq)]),
                )
            })
        })
        .collect();
    let state = |pose: u32| moments[pose as usize].as_deref().unwrap_or(&base);
    let source = |source: DepthSource| match source {
        DepthSource::Pose(pose) => state(pose),
        DepthSource::Base => &base,
    };
    let probe_index = probe.probe;
    let pose_count = probe.marks.len();
    let squeezed = skip_marks(&mut probe.marks, knots, |pose, span| {
        // The renderer's live-tile blend of this interval, by the format's
        // depth rule, packed as the live tile packs it.
        let segment = depth_segment(span, pose_count);
        let mut blended = vec![0.0f32; 2 * PROBE_DEPTH_TILE_TEXELS];
        segment.tile(source(segment.from), source(segment.to), &mut blended);
        let (means, second) = blended.split_at(PROBE_DEPTH_TILE_TEXELS);
        let composed: Vec<[u16; 2]> = means
            .iter()
            .zip(second)
            .map(|(&mean, &mean_sq)| [mean, mean_sq].map(f32_to_f16_bits))
            .collect();
        let own = by_pose[pose as usize].expect("a skipped pose stored a tile");
        bars.depth.score_against(probe_index, &composed, own) < DEPTH_SIGNIFICANCE_FLOOR
    });
    probe.tiles = kept_tiles(&probe.marks, &by_pose);
    let after = mid_stored(&probe.marks);
    let counts = SkipCounts {
        depth_stored: after,
        depth_skipped: before - after,
        squeezed,
        ..SkipCounts::default()
    };
    let stores = probe.marks.contains(&PoseMark::Stored);
    (stores.then_some(probe), counts)
}

/// Skip one mover's redundant mid-pose states against `knots`, its
/// effective knots after its ramp. Runs after every pose of the mover has
/// resolved, from its memo or its bake: the memo holds every pose's states
/// unskipped and never keys on the ramp, so a ramp edit re-decides skips
/// over the same states and warm equals cold. Enclosed and outside-domain
/// mid poses already blend across, and their across marks are untouched.
pub(crate) fn skip_redundant_mid_poses(
    states: &mut MoverBounceStates,
    knots: &[Knot],
    bars: &SkipBars<'_>,
) -> SkipCounts {
    let mut counts = SkipCounts::default();
    if states.pose_count <= 2 {
        return counts;
    }
    let bounce: Vec<_> = std::mem::take(&mut states.bounce)
        .into_par_iter()
        .map(|probe| skip_bounce(probe, knots, bars))
        .collect();
    let depth: Vec<_> = std::mem::take(&mut states.depth)
        .into_par_iter()
        .map(|probe| skip_depth(probe, knots, bars))
        .collect();
    let mut kept_bounce = Vec::with_capacity(bounce.len());
    for (probe, probe_counts) in bounce {
        counts.add(probe_counts);
        kept_bounce.extend(probe);
    }
    let mut kept_depth = Vec::with_capacity(depth.len());
    let mut dropped = 0;
    for (probe, probe_counts) in depth {
        counts.add(probe_counts);
        match probe {
            Some(probe) => kept_depth.push(probe),
            None => dropped += 1,
        }
    }
    states.replace(kept_bounce, kept_depth, dropped);
    counts
}

#[cfg(test)]
#[path = "skip_tests.rs"]
mod tests;
