// Ray crossings of fluid-volume boundaries, the geometric fact under hitscan splash.
// See: context/lib/entity_model.md §7 (Environment Volumes — Hitscan splash facts)

use glam::Vec3;

use crate::leaf::{ContentsMask, ConvexLeaf};
use crate::line::{BrushInterval, Run, clip_line, for_each_run};
use crate::query_timer::{QueryFamily, timed};
use crate::resolve::EnvironmentView;
use crate::volumes::EnvironmentVolumes;

/// A crossing closer to the ray origin than this, in metres along the ray, is
/// not reported: a ray starting on (or within float noise of) a fluid face never
/// splashes there, whether it leaves or enters through it.
pub const CROSSING_START_EPSILON: f32 = 1.0e-3;

/// Brushes of one fluid volume whose intervals along the ray are separated by a
/// gap whose midpoint lies within this distance of one of the volume's brushes,
/// in metres ([`ConvexLeaf`] bounds and plane depth), count as continuous.
/// Shared faces between brushes of one entity are compiled per brush, so their
/// planes can disagree by float noise; without this a ray across the seam would
/// see a hairline gap and report an exit and an entry inside the pool. Measured
/// across the seam rather than along the ray, so a ray grazing a tilted seam,
/// whose gap along the ray is the noise divided by the grazing angle's sine,
/// still joins it. The eye depth ([`crate::fluid_volume_signed_depth`]) joins
/// seams by the same distance over the union of every volume of one fluid.
pub const CROSSING_SEAM_EPSILON: f32 = 1.0e-4;

/// Brush intervals one volume's measurement keeps inline, so its runs are found
/// from the clips already made. A ray meeting more of one volume's brushes falls
/// back to re-clipping that volume's brush list on each pass; both paths measure
/// the same runs.
const INLINE_VOLUME_INTERVALS: usize = 16;

/// One crossing of a fluid volume's boundary along a ray.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidCrossing {
    /// Ray parameter: the crossing is at `origin + dir * t`, in units of `dir`.
    pub t: f32,
    pub point: Vec3,
    /// Unit outward normal of the crossed brush face: the face entered for an
    /// entering crossing, the face exited for an exiting one.
    pub outward_normal: Vec3,
    /// The crossed volume's own fluid; overlapping fluids are not ranked here.
    pub fluid: u16,
    /// The crossed volume's map-order index.
    pub volume: usize,
    /// True when the ray passes from outside the volume to inside it.
    pub entering: bool,
}

/// Visit every crossing of a fluid volume's boundary by the ray
/// `origin + t * dir` with `CROSSING_START_EPSILON / |dir| < t <= max_t`.
///
/// The boundary is that of the union of each fluid volume's brushes: a face
/// shared by two brushes of one volume is no crossing, while
/// two adjacent fluid volumes give an exit from one and an entry into the other
/// at their shared face. Each volume is measured on its own, carrying its own
/// fluid, never an overlap winner. A ray only touching a brush at a point (an
/// edge or corner) crosses nothing; a ray lying in a face's plane is inside, as
/// a point on a face is. The caller handles a blocking hit that
/// coincides with a crossing: a face buried in the blocking surface never
/// splashes.
///
/// Order: increasing `t` within one volume; across volumes unspecified (the
/// same on every peer, since the tree build is deterministic). Allocation-free.
pub fn visit_fluid_crossings(
    view: EnvironmentView<'_>,
    origin: Vec3,
    dir: Vec3,
    max_t: f32,
    visit: impl FnMut(FluidCrossing),
) {
    timed(view.timer, QueryFamily::FluidCrossing, || {
        visit_fluid_crossings_untimed(view, origin, dir, max_t, visit)
    })
}

fn visit_fluid_crossings_untimed(
    view: EnvironmentView<'_>,
    origin: Vec3,
    dir: Vec3,
    max_t: f32,
    mut visit: impl FnMut(FluidCrossing),
) {
    let volumes = view.volumes;
    let dir_length = dir.length();
    if volumes.is_empty()
        || !origin.is_finite()
        || !(dir_length > 0.0 && dir_length.is_finite())
        || max_t.is_nan()
        || max_t <= 0.0
    {
        return;
    }
    let start_t = CROSSING_START_EPSILON / dir_length;
    let seam_t = CROSSING_SEAM_EPSILON / dir_length;
    // Clip past `max_t` so a brush continuing the volume just beyond it still
    // joins the run, and an end at `max_t` is not mistaken for an exit.
    let clip_end = max_t + 4.0 * seam_t;
    // Every clip is behind the tree's own bounds test, so scanning a volume's
    // brush list costs one slab test per brush that misses the ray, and only
    // brushes the ray comes near are clipped.
    let clip_leaf = |leaf: &ConvexLeaf| {
        if !leaf.line_may_meet(origin, dir, 0.0, clip_end) {
            return None;
        }
        clip_line(&leaf.planes, origin, dir, 0.0, clip_end).filter(|interval| interval.has_length())
    };
    let hits = |leaf: &ConvexLeaf| clip_leaf(leaf).is_some();

    volumes.visit_brushes_on_line(
        origin,
        dir,
        0.0,
        clip_end,
        ContentsMask::FLUID,
        |index, brush, leaf| {
            let Some(volume) = volumes.volume(index) else {
                return;
            };
            let Some(fluid) = volume.fluid else {
                return;
            };
            // Measure each volume once, from the lowest-index brush the ray
            // hits, scanning its own brush list rather than walking the tree.
            let first = brush as usize;
            if !hits(leaf) || volume.brushes[..first].iter().any(hits) {
                return;
            }
            let clip = |brush: usize, leaf: &ConvexLeaf| {
                clip_leaf(leaf).map(|interval| BrushInterval {
                    brush: brush as u32,
                    interval,
                })
            };
            // Brushes before the first hit miss, so the gather starts there.
            let mut inline = [None::<BrushInterval>; INLINE_VOLUME_INTERVALS];
            let mut kept = 0;
            let mut overflowed = false;
            for (brush, leaf) in volume.brushes.iter().enumerate().skip(first) {
                let Some(interval) = clip(brush, leaf) else {
                    continue;
                };
                if kept == INLINE_VOLUME_INTERVALS {
                    overflowed = true;
                    break;
                }
                inline[kept] = Some(interval);
                kept += 1;
            }
            for_each_run(
                |sink| {
                    if overflowed {
                        for (brush, leaf) in volume.brushes.iter().enumerate().skip(first) {
                            if let Some(interval) = clip(brush, leaf) {
                                sink(interval);
                            }
                        }
                    } else {
                        inline[..kept]
                            .iter()
                            .flatten()
                            .for_each(|interval| sink(*interval));
                    }
                },
                |gap_start, gap_end| {
                    let midpoint = origin + dir * (0.5 * (gap_start + gap_end));
                    volume
                        .brushes
                        .iter()
                        .any(|leaf| leaf.is_near(midpoint, CROSSING_SEAM_EPSILON))
                },
                |run| {
                    report_run(
                        volumes, index, fluid, origin, dir, start_t, max_t, run, &mut visit,
                    )
                },
            );
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn report_run(
    volumes: &EnvironmentVolumes,
    volume: usize,
    fluid: u16,
    origin: Vec3,
    dir: Vec3,
    start_t: f32,
    max_t: f32,
    run: Run,
    visit: &mut impl FnMut(FluidCrossing),
) {
    let ends = [
        (run.start, run.start_face, true),
        (run.end, run.end_face, false),
    ];
    for (t, face, entering) in ends {
        // A run end the clip range bounds (no face) is not a boundary crossing.
        let Some((brush, plane)) = face else {
            continue;
        };
        if !(t > start_t && t <= max_t) {
            continue;
        }
        let normal = volumes
            .volume(volume)
            .and_then(|v| v.brushes.get(brush as usize))
            .and_then(|b| b.planes.get(plane as usize))
            .map(|p| p.normal.normalize_or_zero())
            .unwrap_or(Vec3::ZERO);
        visit(FluidCrossing {
            t,
            point: origin + dir * t,
            outward_normal: normal,
            fluid,
            volume,
            entering,
        });
    }
}
