// Retired renderer upload threads: handed off at suspend, joined only once finished, polled every redraw.
// See: context/lib/boot_sequence.md §4, §5 · context/plans/in-progress/level-load-pipeline/index.md (Cancellation)

use postretro_renderer::UploadRetirement;

use crate::App;

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
            self.upload_retirements.push(retirement);
        }
    }

    /// Top of every redraw, in every boot state: join each retired upload
    /// thread that has finished and keep the rest. Never waits.
    pub(crate) fn poll_upload_retirements(&mut self) {
        poll_retirements(&mut self.upload_retirements);
    }
}

fn poll_retirements(retirements: &mut Vec<UploadRetirement>) {
    retirements.retain_mut(|retirement| !retirement.try_finish());
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
        let mut retirements = vec![
            UploadRetirement::from_thread(finished),
            UploadRetirement::from_thread(running),
        ];

        poll_retirements(&mut retirements);
        assert_eq!(
            retirements.len(),
            1,
            "the finished thread is joined and dropped; the running one stays"
        );

        drop(hold);
        while !retirements.is_empty() {
            assert!(
                Instant::now() < deadline,
                "the released thread is joined by a later poll"
            );
            poll_retirements(&mut retirements);
            std::thread::yield_now();
        }
    }
}
