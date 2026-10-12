// Optional CPU-time accumulators an environment view carries into its queries, for stage timing.
// See: context/lib/rendering_pipeline.md §12

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Accumulates CPU time spent in environment queries made through a timed
/// [`EnvironmentView`](crate::EnvironmentView), by query family. The caller
/// that owns the stage timing builds one per timed span, attaches it with
/// [`EnvironmentView::timed`](crate::EnvironmentView::timed) only while its
/// timing gate is on, and reads it back into its own stages. A view without
/// one (every untimed caller) pays one branch per query and reads no clock.
///
/// Atomic so a timed view stays `Send + Sync` like an untimed one; counts are
/// relaxed, since only the totals read after the span matter.
#[derive(Debug, Default)]
pub struct EnvironmentQueryTimer {
    body_resolve: QueryTotals,
    fluid_crossing: QueryTotals,
}

#[derive(Debug, Default)]
struct QueryTotals {
    calls: AtomicU64,
    nanos: AtomicU64,
}

impl QueryTotals {
    const fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
            nanos: AtomicU64::new(0),
        }
    }

    fn add(&self, start: Instant) {
        let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
    }

    /// Total nanoseconds, or `None` when no query of this family ran, so a
    /// caller reports absent rather than zero.
    fn nanos(&self) -> Option<u64> {
        (self.calls.load(Ordering::Relaxed) > 0).then(|| self.nanos.load(Ordering::Relaxed))
    }
}

impl EnvironmentQueryTimer {
    pub const fn new() -> Self {
        Self {
            body_resolve: QueryTotals::new(),
            fluid_crossing: QueryTotals::new(),
        }
    }

    /// Time in [`resolve_environment`](crate::resolve_environment) and
    /// [`resolve_gravity`](crate::resolve_gravity): per-body environment
    /// resolution. `None` when no body resolved.
    pub fn body_resolve_nanos(&self) -> Option<u64> {
        self.body_resolve.nanos()
    }

    /// Time in [`visit_fluid_crossings`](crate::visit_fluid_crossings),
    /// including the caller's per-crossing visit. `None` when no ray was
    /// queried.
    pub fn fluid_crossing_nanos(&self) -> Option<u64> {
        self.fluid_crossing.nanos()
    }
}

/// Which accumulator a timed query adds to.
#[derive(Debug, Clone, Copy)]
pub(crate) enum QueryFamily {
    BodyResolve,
    FluidCrossing,
}

/// Runs `query`, adding its elapsed time to `timer`'s `family` when a timer is
/// attached. Untimed, it is a direct call.
#[inline]
pub(crate) fn timed<T>(
    timer: Option<&EnvironmentQueryTimer>,
    family: QueryFamily,
    query: impl FnOnce() -> T,
) -> T {
    let Some(timer) = timer else {
        return query();
    };
    let start = Instant::now();
    let result = query();
    match family {
        QueryFamily::BodyResolve => timer.body_resolve.add(start),
        QueryFamily::FluidCrossing => timer.fluid_crossing.add(start),
    }
    result
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::{
        BodyProbe, EnvironmentView, EnvironmentVolumes, resolve_environment, resolve_gravity,
        visit_fluid_crossings,
    };

    fn probe() -> BodyProbe {
        BodyProbe {
            position: Vec3::ZERO,
            half_height: 0.5,
            radius: 0.3,
            eye_height: 0.4,
        }
    }

    #[test]
    fn timed_view_counts_each_query_family_apart() {
        let volumes = EnvironmentVolumes::empty();
        let timer = EnvironmentQueryTimer::new();
        let view = EnvironmentView::new(-9.81, &volumes, &[]).timed(&timer);
        assert_eq!(timer.body_resolve_nanos(), None, "absent before any query");
        assert_eq!(timer.fluid_crossing_nanos(), None);

        resolve_environment(view, &probe());
        resolve_gravity(view, Vec3::ZERO);
        assert_eq!(timer.body_resolve.calls.load(Ordering::Relaxed), 2);
        assert!(timer.body_resolve_nanos().is_some());
        assert_eq!(timer.fluid_crossing_nanos(), None, "no ray queried");

        visit_fluid_crossings(view, Vec3::ZERO, Vec3::X, 10.0, |_| {});
        assert_eq!(timer.fluid_crossing.calls.load(Ordering::Relaxed), 1);
        assert!(timer.fluid_crossing_nanos().is_some());
    }

    #[test]
    fn untimed_view_resolves_identically_and_records_nothing() {
        let volumes = EnvironmentVolumes::empty();
        let timer = EnvironmentQueryTimer::new();
        let untimed = EnvironmentView::new(-3.0, &volumes, &[]);
        let timed = untimed.timed(&timer);
        assert!(untimed.timer.is_none());
        let from_untimed = resolve_environment(untimed, &probe());
        assert_eq!(timer.body_resolve_nanos(), None, "untimed view never adds");
        assert_eq!(from_untimed, resolve_environment(timed, &probe()));
        assert!(timer.body_resolve_nanos().is_some());
    }
}
