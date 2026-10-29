// The broad-face rule: the occluder rule judged on one broad face of an
// own-frame box, sampled on a lattice, and the face measures the cluster
// pass reads from the same probes.
// See: context/plans/in-progress/occluder-first-partition/research.md §Occluder rule

use std::f64::consts::FRAC_1_SQRT_2;

use glam::DVec3;

use super::{
    BrushFrame, FACE_SAMPLE_FRACTIONS, OCCLUDER_ABSOLUTE_LENGTH_M, OCCLUDER_FLOOR_M,
    OCCLUDER_SPAN_FRACTION, OccluderVerdict, OpenSpace, PROBE_OFFSET_M, at_least, see_over_m,
};

/// An axis within 45° of up takes the see-over test; any other takes the
/// horizontal one.
const VERTICAL_AXIS_MIN_COS: f64 = FRAC_1_SQRT_2;

/// The occluder rule on an own-frame box, judged from the faces across each
/// of `normal_axes`: a brush offers its thinnest axes, a cluster section the
/// axis across its slab.
pub(crate) fn judge_box(
    frame: &BrushFrame,
    normal_axes: impl IntoIterator<Item = usize>,
    space: &OpenSpace<'_>,
) -> OccluderVerdict {
    // Exempt verdicts rank by how far the box got; the furthest one wins.
    let mut verdict = OccluderVerdict::UnderFloor;
    for normal_axis in normal_axes {
        if !broad_axes_reach_floor(frame, normal_axis) {
            continue;
        }
        verdict = rank(verdict, OccluderVerdict::NoOpenFace);
        for sign in [1.0, -1.0] {
            let face_verdict = judge_face(frame, normal_axis, sign, space);
            if face_verdict == OccluderVerdict::Occludes {
                return face_verdict;
            }
            verdict = rank(verdict, face_verdict);
        }
    }
    verdict
}

/// The occluder rule on the one broad face of `frame`'s box across
/// `normal_axis` whose outward normal points along `outward` (±1) times
/// that axis: what `judge_box` reads from each face.
pub(crate) fn judge_face(
    frame: &BrushFrame,
    normal_axis: usize,
    outward: f64,
    space: &OpenSpace<'_>,
) -> OccluderVerdict {
    if !broad_axes_reach_floor(frame, normal_axis) {
        return OccluderVerdict::UnderFloor;
    }
    let broad = [(normal_axis + 1) % 3, (normal_axis + 2) % 3];
    BroadFace::of(frame, normal_axis, outward).judge(frame, broad, space)
}

/// True when both broad axes of the faces across `normal_axis` reach the
/// floor, so the rule judges those faces at all.
pub(crate) fn broad_axes_reach_floor(frame: &BrushFrame, normal_axis: usize) -> bool {
    [(normal_axis + 1) % 3, (normal_axis + 2) % 3]
        .iter()
        .all(|&axis| at_least(frame.extents[axis], OCCLUDER_FLOOR_M))
}

fn rank(current: OccluderVerdict, candidate: OccluderVerdict) -> OccluderVerdict {
    fn order(verdict: OccluderVerdict) -> u8 {
        match verdict {
            OccluderVerdict::UnderFloor => 0,
            OccluderVerdict::NoOpenFace => 1,
            OccluderVerdict::ShortSpan => 2,
            OccluderVerdict::LowTop => 3,
            OccluderVerdict::Occludes => 4,
        }
    }
    if order(candidate) > order(current) {
        candidate
    } else {
        current
    }
}

/// Mean open depth in front of the face of `frame`'s box whose outward
/// normal is `outward` along `normal_axis`, over the face lattice's open
/// samples; zero when none is open. The same probes the rule's depth test
/// casts, so it reads only geometry and the first pass.
pub(crate) fn broad_face_open_depth(
    frame: &BrushFrame,
    normal_axis: usize,
    outward: f64,
    space: &OpenSpace<'_>,
) -> f64 {
    let depths = open_sample_depths(frame, normal_axis, outward, space);
    if depths.is_empty() {
        0.0
    } else {
        depths.iter().sum::<f64>() / depths.len() as f64
    }
}

/// Deepest open depth in front of the face of `frame`'s box whose outward
/// normal is `outward` along `normal_axis`, over the face lattice's open
/// samples: where the open space in front of the face ends. `None` when no
/// sample is open. The same probes the rule's depth test casts.
pub(crate) fn broad_face_deepest_open_depth(
    frame: &BrushFrame,
    normal_axis: usize,
    outward: f64,
    space: &OpenSpace<'_>,
) -> Option<f64> {
    open_sample_depths(frame, normal_axis, outward, space)
        .into_iter()
        .reduce(f64::max)
}

/// Open depth in front of the face at each lattice sample whose probe starts
/// in open space, in lattice order.
fn open_sample_depths(
    frame: &BrushFrame,
    normal_axis: usize,
    outward: f64,
    space: &OpenSpace<'_>,
) -> Vec<f64> {
    let broad = [(normal_axis + 1) % 3, (normal_axis + 2) % 3];
    let face = BroadFace::of(frame, normal_axis, outward);
    face.sample_offsets(frame, broad)
        .map(|offsets| face.probe(offsets))
        .filter(|&probe| !space.is_blocked(probe))
        .map(|probe| face.open_depth(probe, space))
        .collect()
}

/// The broad axis of `frame`'s box across `normal_axis` that takes the
/// see-over test, within 45° of up; the more upright when both are, the
/// first on a tie. `None` for a slab, whose broad axes are both horizontal.
pub(crate) fn vertical_broad_axis(frame: &BrushFrame, normal_axis: usize) -> Option<usize> {
    let upness = |axis: usize| frame.axes[axis].dot(DVec3::Y).abs();
    [(normal_axis + 1) % 3, (normal_axis + 2) % 3]
        .into_iter()
        .filter(|&axis| upness(axis) >= VERTICAL_AXIS_MIN_COS)
        .reduce(|best, axis| {
            if upness(axis) > upness(best) {
                axis
            } else {
                best
            }
        })
}

/// World height of the lowest floor the see-over test reads in front of the
/// face of `frame`'s box whose outward normal is `outward` along
/// `normal_axis`: the drop from in front of the top along `vertical_axis`,
/// at each lattice sample that fronts open space at least the floor deep.
/// `None` when no sample does. The same drops the rule casts, so it reads
/// only geometry and the first pass.
pub(crate) fn broad_face_floor(
    frame: &BrushFrame,
    normal_axis: usize,
    vertical_axis: usize,
    outward: f64,
    space: &OpenSpace<'_>,
) -> Option<f64> {
    let broad = [(normal_axis + 1) % 3, (normal_axis + 2) % 3];
    let slot = broad.iter().position(|&axis| axis == vertical_axis)?;
    let face = BroadFace::of(frame, normal_axis, outward);
    face.sample_offsets(frame, broad)
        .filter(|&offsets| {
            let probe = face.probe(offsets);
            !space.is_blocked(probe) && face.fronts_floor_deep(probe, space)
        })
        .map(|offsets| {
            let (top, see_over) =
                face.drop_from_top(frame, vertical_axis, offsets[1 - slot], space);
            top.y - see_over
        })
        .reduce(f64::min)
}

/// One broad face of a brush's own-frame box, outward normal and centre.
struct BroadFace {
    normal: DVec3,
    center: DVec3,
}

impl BroadFace {
    /// The face of `frame`'s box across `normal_axis` whose outward normal
    /// points along `sign` (±1) times that axis.
    fn of(frame: &BrushFrame, normal_axis: usize, sign: f64) -> Self {
        let normal = frame.axes[normal_axis] * sign;
        Self {
            normal,
            center: frame.center + normal * (frame.extents[normal_axis] * 0.5),
        }
    }

    /// The lattice's displacements from the face centre along its two broad
    /// axes, one pair per sample.
    fn sample_offsets(
        &self,
        frame: &BrushFrame,
        broad: [usize; 2],
    ) -> impl Iterator<Item = [DVec3; 2]> {
        let offset = move |axis: usize, fraction: f64| {
            frame.axes[axis] * ((fraction - 0.5) * frame.extents[axis])
        };
        FACE_SAMPLE_FRACTIONS.into_iter().flat_map(move |a| {
            FACE_SAMPLE_FRACTIONS
                .into_iter()
                .map(move |b| [offset(broad[0], a), offset(broad[1], b)])
        })
    }

    /// Where a sample's probes start: in front of the face, off its plane.
    fn probe(&self, offsets: [DVec3; 2]) -> DVec3 {
        self.center + offsets[0] + offsets[1] + self.normal * PROBE_OFFSET_M
    }

    /// Open depth in front of the face from `probe`, along its normal.
    fn open_depth(&self, probe: DVec3, space: &OpenSpace<'_>) -> f64 {
        PROBE_OFFSET_M + space.distance_to_blocking(probe, self.normal)
    }

    /// The rule's face test at one sample: the open space in front of the
    /// face from `probe` is at least the floor deep.
    fn fronts_floor_deep(&self, probe: DVec3, space: &OpenSpace<'_>) -> bool {
        at_least(self.open_depth(probe, space), OCCLUDER_FLOOR_M)
    }

    /// The see-over drop: from just in front of the face's top along the
    /// broad `axis`, displaced `across` along the other broad axis, down
    /// through any ceiling or roof the top meets or sinks into, to the floor
    /// below. No one sees over a top that solid closes, so the measure runs
    /// from the top itself. Returns the top and the drop's length.
    fn drop_from_top(
        &self,
        frame: &BrushFrame,
        axis: usize,
        across: DVec3,
        space: &OpenSpace<'_>,
    ) -> (DVec3, f64) {
        let direction = frame.axes[axis];
        let up = direction * direction.dot(DVec3::Y).signum();
        let top =
            self.center + across + up * (frame.extents[axis] * 0.5) + self.normal * PROBE_OFFSET_M;
        // Start just under the top, so the drop never starts on its plane,
        // and clear of the box: a face that leans back stands farther out
        // below its top, so a drop from just in front of the top would meet
        // the face itself and read it as the floor.
        let start = self.clear_of_box(frame, top) - DVec3::Y * PROBE_OFFSET_M;
        let see_over = PROBE_OFFSET_M + space.distance_through_to_blocking(start, DVec3::NEG_Y);
        (top, see_over)
    }

    /// `point` moved out along the face's horizontal outward direction until
    /// it lies the probe offset beyond every point of `frame`'s box. Unmoved
    /// in front of an upright face, whose top is already that far out.
    fn clear_of_box(&self, frame: &BrushFrame, point: DVec3) -> DVec3 {
        let Some(out) = (self.normal - DVec3::Y * self.normal.y).try_normalize() else {
            return point;
        };
        let reach = frame.center.dot(out)
            + (0..3)
                .map(|axis| frame.axes[axis].dot(out).abs() * frame.extents[axis] * 0.5)
                .sum::<f64>();
        point + out * (reach + PROBE_OFFSET_M - point.dot(out)).max(0.0)
    }

    /// Judge the face at each open sample of its lattice; it passes when
    /// most of them pass. A sample whose probe starts in blocked space reads
    /// the brush touching the face there, not the space in front, so it is
    /// skipped.
    fn judge(
        &self,
        frame: &BrushFrame,
        broad: [usize; 2],
        space: &OpenSpace<'_>,
    ) -> OccluderVerdict {
        let (mut open, mut passing) = (0_usize, 0_usize);
        let mut failing = OccluderVerdict::NoOpenFace;
        for offsets in self.sample_offsets(frame, broad) {
            let probe = self.probe(offsets);
            if space.is_blocked(probe) {
                continue;
            }
            open += 1;
            match self.judge_sample(probe, offsets, frame, broad, space) {
                OccluderVerdict::Occludes => passing += 1,
                verdict => failing = rank(failing, verdict),
            }
        }
        if open > 0 && 2 * passing > open {
            OccluderVerdict::Occludes
        } else {
            failing
        }
    }

    /// The rule at one sample: `probe` lies in front of the face, displaced
    /// from its centre by `offsets` along the two broad axes.
    fn judge_sample(
        &self,
        probe: DVec3,
        offsets: [DVec3; 2],
        frame: &BrushFrame,
        broad: [usize; 2],
        space: &OpenSpace<'_>,
    ) -> OccluderVerdict {
        if !self.fronts_floor_deep(probe, space) {
            return OccluderVerdict::NoOpenFace;
        }
        for (slot, axis) in broad.into_iter().enumerate() {
            let direction = frame.axes[axis];
            let length = frame.extents[axis];
            let upness = direction.dot(DVec3::Y);
            if upness.abs() >= VERTICAL_AXIS_MIN_COS {
                let (_, see_over) = self.drop_from_top(frame, axis, offsets[1 - slot], space);
                if !at_least(see_over, see_over_m()) {
                    return OccluderVerdict::LowTop;
                }
            } else {
                let across = (direction - DVec3::Y * upness).normalize();
                let span = space.distance_to_blocking(probe, across)
                    + space.distance_to_blocking(probe, -across);
                if !at_least(length, OCCLUDER_ABSOLUTE_LENGTH_M)
                    && !at_least(length, OCCLUDER_SPAN_FRACTION * span)
                {
                    return OccluderVerdict::ShortSpan;
                }
            }
        }
        OccluderVerdict::Occludes
    }
}
