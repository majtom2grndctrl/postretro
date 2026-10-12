// Line-versus-convex-leaf clipping and allocation-free unions of the resulting intervals.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md

use glam::Vec3;

use crate::leaf::LeafPlane;

/// The closed parameter range where `origin + t * dir` lies inside a convex leaf,
/// within the range the clip was asked for. `enter_plane` / `exit_plane` index
/// the leaf plane that bounds each end, or are `None` where the requested range
/// bounds it instead (the line was already inside there).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LineInterval {
    pub t_enter: f32,
    pub t_exit: f32,
    pub enter_plane: Option<u32>,
    pub exit_plane: Option<u32>,
}

impl LineInterval {
    /// Positive length: a line that only touches the leaf at a point (an edge or
    /// corner) covers nothing and crosses nothing.
    pub fn has_length(&self) -> bool {
        self.t_exit > self.t_enter
    }
}

/// Clip the segment `origin + t * dir`, `t` in `[t_min, t_max]`, against the
/// inner half-spaces of `planes` (`dot(p, n) - d <= 0`, the same closed rule as
/// `ConvexLeaf::contains_point`). Returns `None` when nothing remains. A plane the
/// line runs parallel to either keeps the whole line or none of it. Ties between
/// planes bounding the same end keep the lower plane index.
pub(crate) fn clip_line(
    planes: &[LeafPlane],
    origin: Vec3,
    dir: Vec3,
    t_min: f32,
    t_max: f32,
) -> Option<LineInterval> {
    #[cfg(test)]
    CLIP_LINE_CALLS.with(|calls| calls.set(calls.get() + 1));
    let mut interval = LineInterval {
        t_enter: t_min,
        t_exit: t_max,
        enter_plane: None,
        exit_plane: None,
    };
    for (index, plane) in planes.iter().enumerate() {
        let start_side = plane.side(origin);
        let rate = plane.normal.dot(dir);
        if rate == 0.0 {
            if start_side > 0.0 {
                return None;
            }
            continue;
        }
        let t = -start_side / rate;
        if rate > 0.0 {
            if t < interval.t_exit {
                interval.t_exit = t;
                interval.exit_plane = Some(index as u32);
            }
        } else if t > interval.t_enter {
            interval.t_enter = t;
            interval.enter_plane = Some(index as u32);
        }
        if interval.t_enter > interval.t_exit {
            return None;
        }
    }
    Some(interval)
}

// Test hook: clips made on this thread, so a test can bound a query's cost.
#[cfg(test)]
thread_local! {
    pub(crate) static CLIP_LINE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One leaf's interval along a line, tagged with the leaf's index in its volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BrushInterval {
    pub brush: u32,
    pub interval: LineInterval,
}

/// Which brush face bounds one end of a run: `(brush index, plane index)`. The
/// brush index is within its own volume, so a face names a unique brush only in
/// a union of one volume's brushes.
pub(crate) type FaceRef = (u32, u32);

/// One maximal covered run of a union of intervals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Run {
    pub start: f32,
    /// The face the run starts on; `None` where the clip range bounds it.
    pub start_face: Option<FaceRef>,
    pub end: f32,
    /// The face the run ends on; `None` where the clip range bounds it.
    pub end_face: Option<FaceRef>,
}

/// Emit, in increasing order, the maximal runs of the union of the positive-length
/// intervals `source` yields. Overlapping or touching intervals join one run; a
/// run also continues across a gap `(end, next_start)` when `joins` accepts it
/// (a seam of float noise between two brushes of one union). Allocation-free:
/// `source` is called once per extension step and must yield the same intervals
/// every call (any order). A run's start face is the lowest brush index among
/// intervals starting there, its end face the lowest among those ending there.
/// A run's bounds never depend on yield order; its faces may when the union
/// spans several volumes, whose brush indices repeat. Only crossings read
/// faces, and they measure one volume at a time.
pub(crate) fn for_each_run(
    source: impl Fn(&mut dyn FnMut(BrushInterval)),
    joins: impl Fn(f32, f32) -> bool,
    mut emit: impl FnMut(Run),
) {
    let mut previous_end: Option<f32> = None;
    loop {
        // Every interval starting at or before the previous run's end lies
        // inside it, so the next run starts at the earliest one past that.
        let Some(first) = earliest_start_after(&source, previous_end) else {
            return;
        };

        // Extend while any interval reaching the run ends later, or ends at the
        // same point from a lower brush index: the end face is then the lowest
        // brush among the run's intervals ending there, whatever the pass order.
        // With nothing reaching, step over a gap `joins` accepts to the next
        // interval and keep extending from there.
        let mut last = first;
        loop {
            let end = last.interval.t_exit;
            let mut extension: Option<BrushInterval> = None;
            source(&mut |candidate| {
                let interval = candidate.interval;
                if interval.has_length()
                    && interval.t_enter <= end
                    && ends_after(candidate, last)
                    && extension.is_none_or(|best| ends_after(candidate, best))
                {
                    extension = Some(candidate);
                }
            });
            if let Some(extension) = extension {
                last = extension;
                continue;
            }
            match earliest_start_after(&source, Some(end)) {
                Some(next) if joins(end, next.interval.t_enter) => last = next,
                _ => break,
            }
        }

        let end = last.interval.t_exit;
        emit(Run {
            start: first.interval.t_enter,
            start_face: face(first.brush, first.interval.enter_plane),
            end,
            end_face: face(last.brush, last.interval.exit_plane),
        });
        previous_end = Some(end);
    }
}

/// The positive-length interval starting first after `after` (any, for `None`),
/// lowest brush index on a tie.
fn earliest_start_after(
    source: &impl Fn(&mut dyn FnMut(BrushInterval)),
    after: Option<f32>,
) -> Option<BrushInterval> {
    let mut first: Option<BrushInterval> = None;
    source(&mut |candidate| {
        if !candidate.interval.has_length()
            || after.is_some_and(|end| candidate.interval.t_enter <= end)
        {
            return;
        }
        if first.is_none_or(|best| starts_before(candidate, best)) {
            first = Some(candidate);
        }
    });
    first
}

/// Total length covered by the union of the positive-length intervals `source`
/// yields. Exact: overlapping intervals count once, and runs sum in increasing
/// order so the float result never depends on yield order.
pub(crate) fn union_length(source: impl Fn(&mut dyn FnMut(BrushInterval))) -> f32 {
    let mut total = 0.0;
    for_each_run(source, |_, _| false, |run| total += run.end - run.start);
    total
}

fn face(brush: u32, plane: Option<u32>) -> Option<FaceRef> {
    plane.map(|plane| (brush, plane))
}

fn starts_before(a: BrushInterval, b: BrushInterval) -> bool {
    a.interval.t_enter < b.interval.t_enter
        || (a.interval.t_enter == b.interval.t_enter && a.brush < b.brush)
}

fn ends_after(a: BrushInterval, b: BrushInterval) -> bool {
    a.interval.t_exit > b.interval.t_exit
        || (a.interval.t_exit == b.interval.t_exit && a.brush < b.brush)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::axis_box;

    const EPS: f32 = 1.0e-6;

    fn interval(brush: u32, t_enter: f32, t_exit: f32) -> BrushInterval {
        BrushInterval {
            brush,
            interval: LineInterval {
                t_enter,
                t_exit,
                enter_plane: Some(0),
                exit_plane: Some(1),
            },
        }
    }

    /// Runs of `intervals`, joining any gap no wider than `seam`.
    fn runs(intervals: &[BrushInterval], seam: f32) -> Vec<(f32, f32)> {
        let mut out = Vec::new();
        for_each_run(
            |sink| intervals.iter().for_each(|i| sink(*i)),
            |gap_start, gap_end| gap_end - gap_start <= seam,
            |run| out.push((run.start, run.end)),
        );
        out
    }

    #[test]
    fn clip_line_through_box_reports_entry_and_exit_faces() {
        let leaf = axis_box(Vec3::ZERO, Vec3::ONE);
        let clipped = clip_line(&leaf.planes, Vec3::new(-1.0, 0.5, 0.5), Vec3::X, 0.0, 10.0)
            .expect("the line crosses the box");
        assert!((clipped.t_enter - 1.0).abs() < EPS);
        assert!((clipped.t_exit - 2.0).abs() < EPS);
        // axis_box plane order: +X, -X, +Y, -Y, +Z, -Z.
        assert_eq!(clipped.enter_plane, Some(1));
        assert_eq!(clipped.exit_plane, Some(0));

        let inside = clip_line(&leaf.planes, Vec3::splat(0.5), Vec3::X, 0.0, 10.0).unwrap();
        assert_eq!(
            inside.enter_plane, None,
            "the clip range bounds a start inside"
        );
        assert!(clip_line(&leaf.planes, Vec3::new(-1.0, 2.0, 0.5), Vec3::X, 0.0, 10.0).is_none());
    }

    #[test]
    fn interval_union_counts_overlap_once_and_joins_touching_intervals() {
        let set = [
            interval(2, 3.0, 5.0),
            interval(0, 0.0, 1.0),
            interval(1, 1.0, 2.0),
            interval(3, 4.0, 4.5),
        ];
        assert_eq!(runs(&set, 0.0), vec![(0.0, 2.0), (3.0, 5.0)]);
        let total = union_length(|sink| set.iter().for_each(|i| sink(*i)));
        assert!((total - 4.0).abs() < EPS);
        // A gap the predicate accepts joins the runs; one it rejects does not.
        assert_eq!(runs(&set, 1.0), vec![(0.0, 5.0)]);
        assert_eq!(runs(&set, 0.99), vec![(0.0, 2.0), (3.0, 5.0)]);
    }

    #[test]
    fn interval_union_ignores_zero_length_touches() {
        let set = [interval(0, 1.0, 1.0), interval(1, 2.0, 3.0)];
        assert_eq!(runs(&set, 0.0), vec![(2.0, 3.0)]);
    }
}
