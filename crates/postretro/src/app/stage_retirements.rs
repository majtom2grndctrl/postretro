// The one list of stages still winding down: cancelled or ended level loads, and the upload threads of renderers torn down at suspend. Polled every redraw, in every boot state; nothing here waits.
// See: context/lib/boot_sequence.md §4, §5 · context/plans/in-progress/level-load-pipeline/index.md (Cancellation)

use postretro_renderer::UploadRetirement;

use crate::App;
use crate::level_load::LoadRetirement;

/// Every retiring stage the App owns. A load beside a running level (task
/// 12's) retires here too, so one poll covers Loading, Settling, Running and
/// Frontend frames alike, and Booting and Splash frames before a resumed
/// renderer exists.
#[derive(Debug, Default)]
pub(crate) struct StageRetirements {
    entries: Vec<Retiring>,
}

#[derive(Debug)]
enum Retiring {
    /// A torn-down renderer's upload thread.
    Upload(UploadRetirement),
    /// A level load's prep tasks and set build, by the load's generation.
    Load {
        generation: u64,
        retirement: LoadRetirement,
    },
}

impl Retiring {
    fn try_finish(&mut self) -> bool {
        match self {
            Retiring::Upload(retirement) => retirement.try_finish(),
            Retiring::Load { retirement, .. } => retirement.try_finish(),
        }
    }
}

impl StageRetirements {
    pub(crate) fn push_upload(&mut self, retirement: UploadRetirement) {
        self.entries.push(Retiring::Upload(retirement));
    }

    pub(crate) fn push_load(&mut self, generation: u64, retirement: LoadRetirement) {
        self.entries.push(Retiring::Load {
            generation,
            retirement,
        });
    }

    /// Join each finished entry and keep the rest. Never waits.
    pub(crate) fn poll(&mut self) {
        self.entries.retain_mut(|entry| !entry.try_finish());
    }

    /// Nothing is still winding down: a new load's stages may start without
    /// overlapping an older generation's.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The generations of the loads still retiring, oldest first.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn retiring_loads(&self) -> Vec<u64> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                Retiring::Load { generation, .. } => Some(*generation),
                Retiring::Upload(_) => None,
            })
            .collect()
    }

    /// How many retiring upload threads remain.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn retiring_uploads(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| matches!(entry, Retiring::Upload(_)))
            .count()
    }
}

impl App {
    /// Take the renderer's upload thread before the renderer drops (suspend),
    /// so a job still running on the old device (an unfinished pipeline
    /// build, say) is joined later by the redraw poll, never here.
    pub(crate) fn retire_renderer_upload_thread(&mut self) {
        if let Some(retirement) = self
            .renderer
            .as_mut()
            .and_then(|renderer| renderer.take_upload_retirement())
        {
            self.stage_retirements.push_upload(retirement);
        }
    }

    /// Top of every redraw, in every boot state: join each retired stage that
    /// has finished and keep the rest. Never waits.
    pub(crate) fn poll_stage_retirements(&mut self) {
        self.stage_retirements.poll();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn redraw_poll_drops_a_finished_retirement_and_keeps_an_unfinished_one() {
        let finished = std::thread::spawn(|| {});
        let (hold, held) = mpsc::channel::<()>();
        let running = std::thread::spawn(move || {
            let _ = held.recv();
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        while !finished.is_finished() {
            assert!(Instant::now() < deadline, "the finished thread ends");
            std::thread::yield_now();
        }
        let mut retirements = StageRetirements::default();
        retirements.push_upload(UploadRetirement::from_thread(finished));
        retirements.push_upload(UploadRetirement::from_thread(running));

        retirements.poll();
        assert_eq!(
            retirements.retiring_uploads(),
            1,
            "the finished thread is joined and dropped; the running one stays"
        );
        assert!(!retirements.is_empty());

        drop(hold);
        while !retirements.is_empty() {
            assert!(
                Instant::now() < deadline,
                "the released thread is joined by a later poll"
            );
            retirements.poll();
            std::thread::yield_now();
        }
    }
}
