//! Per-phase wall-clock marks for one level-geometry install.
//! See: context/lib/boot_sequence.md §3

use std::time::{Duration, Instant};

/// Wall time between consecutive named points inside
/// `Renderer::install_level_geometry`, logged as one line when the install
/// ends. The renderer reports its own breakdown; the app's `geometry_upload`
/// stage only sees the total.
pub(super) struct GeometryInstallMarks {
    start: Instant,
    last: Instant,
    phases: Vec<(&'static str, Duration)>,
}

impl GeometryInstallMarks {
    pub(super) fn start() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            last: now,
            phases: Vec::new(),
        }
    }

    /// Close the phase running since the previous mark under `phase`.
    pub(super) fn mark(&mut self, phase: &'static str) {
        let now = Instant::now();
        self.phases.push((phase, now.duration_since(self.last)));
        self.last = now;
    }

    /// One line: the total, each marked phase, and `other` for the time
    /// between marks and the end of the install. `has_geometry` is false for
    /// the empty install that releases a level's resources.
    pub(super) fn log(&self, has_geometry: bool) {
        let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
        let total = self.start.elapsed();
        let marked: Duration = self.phases.iter().map(|(_, duration)| *duration).sum();
        let mut line = format!("total={:.1}ms", ms(total));
        for (phase, duration) in &self.phases {
            line.push_str(&format!(", {phase}={:.1}ms", ms(*duration)));
        }
        line.push_str(&format!(
            ", other={:.1}ms",
            ms(total.saturating_sub(marked))
        ));
        log::info!("[Renderer] Geometry install timing: has_geometry={has_geometry}, {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_keep_mark_order_and_never_exceed_the_total() {
        let mut marks = GeometryInstallMarks::start();
        marks.mark("first");
        marks.mark("second");
        let names: Vec<_> = marks.phases.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["first", "second"]);
        let marked: Duration = marks.phases.iter().map(|(_, duration)| *duration).sum();
        assert!(marked <= marks.start.elapsed());
    }
}
