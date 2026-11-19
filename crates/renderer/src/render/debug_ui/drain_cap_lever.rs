//! Streaming tab's play drain cap lever: the shared per-drain install budget
//! for SH and lightmaps, overridden live for the drain-budget spike.
//! See: context/lib/rendering_pipeline.md §4 · context/lib/experimental_spikes.md §Tuning levers
//!
//! The lever is plain data so the renderer never sees streaming types; the
//! caller builds it from the level's streaming owner and writes a changed
//! lever back.

const MIB: f64 = 1024.0 * 1024.0;

/// Slider span relative to production: an eighth of it to four times it,
/// so half and twice production are always in reach.
const SLIDER_MIN_FRACTION: f64 = 1.0 / 8.0;
const SLIDER_MAX_MULTIPLE: f64 = 4.0;
const SLIDER_STEP_MIB: f64 = 0.5;

/// The play drain's byte cap as the dev-tools Streaming tab edits it. A
/// developer lever, never a player setting
/// (`context/lib/experimental_spikes.md` §Tuning levers). It replaces the
/// play cap only: Settling keeps its own budget, and the lightmap in-hand
/// caps keep their production totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamingDrainCapLever {
    /// The override in bytes; `None` admits against the production cap.
    /// Takes effect at the next frame.
    pub cap_bytes: Option<u64>,
    /// The production play cap in bytes; read-only.
    pub production_bytes: u64,
}

impl StreamingDrainCapLever {
    /// The cap the next play drain admits against.
    pub fn effective_bytes(&self) -> u64 {
        self.cap_bytes.unwrap_or(self.production_bytes)
    }

    /// Overrides the cap with `mib` mebibytes, rounded to whole bytes and
    /// never below one byte.
    pub fn set_cap_mib(&mut self, mib: f64) {
        self.cap_bytes = Some(((mib * MIB).round() as u64).max(1));
    }

    /// Returns the cap to production (unset).
    pub fn reset(&mut self) {
        self.cap_bytes = None;
    }

    /// The slider's range in MiB.
    fn slider_range_mib(&self) -> std::ops::RangeInclusive<f64> {
        let production = self.production_bytes as f64 / MIB;
        production * SLIDER_MIN_FRACTION..=production * SLIDER_MAX_MULTIPLE
    }
}

pub(super) fn draw_drain_cap_lever(ui: &mut egui::Ui, lever: &mut StreamingDrainCapLever) {
    ui.label("Play drain cap (MiB)");
    let mut mib = lever.effective_bytes() as f64 / MIB;
    let slider = egui::Slider::new(&mut mib, lever.slider_range_mib())
        .step_by(SLIDER_STEP_MIB)
        .suffix(" MiB");
    if ui.add(slider).changed() {
        lever.set_cap_mib(mib);
    }
    ui.horizontal(|ui| {
        ui.label(status_text(lever));
        if ui
            .add_enabled(
                lever.cap_bytes.is_some(),
                egui::Button::new("Production (unset)"),
            )
            .clicked()
        {
            lever.reset();
        }
    });
    ui.label(
        egui::RichText::new(
            "Dev-tools lever: one play cap for SH and lightmap, next frame. Settling and the \
             lightmap in-hand caps keep production.",
        )
        .weak(),
    );
}

fn status_text(lever: &StreamingDrainCapLever) -> String {
    let production = lever.production_bytes as f64 / MIB;
    match lever.cap_bytes {
        None => format!("Production, {production:.1} MiB"),
        Some(_) => format!("Override (production {production:.1} MiB)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRODUCTION: u64 = 8 * 1024 * 1024;

    fn lever() -> StreamingDrainCapLever {
        StreamingDrainCapLever {
            cap_bytes: None,
            production_bytes: PRODUCTION,
        }
    }

    #[test]
    fn slider_reaches_half_and_twice_production() {
        let range = lever().slider_range_mib();
        assert!(*range.start() <= 4.0 && *range.end() >= 16.0, "{range:?}");
        assert!(*range.start() <= 2.0 && *range.end() >= 32.0, "{range:?}");
        assert!(*range.start() > 0.0, "a zero cap is never offered");
    }

    #[test]
    fn slider_mib_becomes_whole_bytes_and_reset_returns_to_production() {
        let mut lever = lever();
        assert_eq!(lever.effective_bytes(), PRODUCTION);
        lever.set_cap_mib(4.0);
        assert_eq!(lever.cap_bytes, Some(PRODUCTION / 2));
        lever.set_cap_mib(16.0);
        assert_eq!(lever.effective_bytes(), 2 * PRODUCTION);
        lever.set_cap_mib(0.5);
        assert_eq!(lever.cap_bytes, Some(512 * 1024));
        lever.reset();
        assert_eq!(lever.cap_bytes, None);
        assert_eq!(lever.effective_bytes(), PRODUCTION);
    }

    #[test]
    fn status_names_production_or_override() {
        let mut lever = lever();
        assert_eq!(status_text(&lever), "Production, 8.0 MiB");
        lever.set_cap_mib(4.0);
        assert_eq!(status_text(&lever), "Override (production 8.0 MiB)");
    }
}
