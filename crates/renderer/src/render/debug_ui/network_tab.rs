//! Network tab: the dev lag proxy's live impairment, edited in place, plus the
//! session role and measured round trip, read-only.
//! See: context/lib/networking.md §Testing the conditioned link
//!
//! The lever is plain data so the renderer never sees netcode types; the
//! caller builds it from the live endpoint and writes a changed lever back.

/// The networked role the tab reports; read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkSessionRole {
    Host,
    Client,
}

impl NetworkSessionRole {
    const fn label(self) -> &'static str {
        match self {
            Self::Host => "Host",
            Self::Client => "Client",
        }
    }
}

/// The lag proxy's impairment as the dev-tools Network tab edits it. Every
/// value applies to each direction through this process's proxy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NetworkLagLever {
    /// One-way delay in milliseconds.
    pub delay_ms: u32,
    /// Upper bound of the uniform extra delay, in milliseconds.
    pub jitter_ms: u32,
    /// Drop chance per datagram, as a percent.
    pub loss_percent: f64,
    /// Read-only.
    pub role: NetworkSessionRole,
    /// The client's smoothed round trip in milliseconds; read-only. `None` on
    /// a host or before the first time-sync echo.
    pub measured_rtt_ms: Option<f64>,
}

/// Slider bounds. A launch seed past them is kept until a slider is dragged.
const NETWORK_LAG_MAX_DELAY_MS: u32 = 500;
const NETWORK_LAG_MAX_JITTER_MS: u32 = 250;
const NETWORK_LAG_MAX_LOSS_PERCENT: f64 = 50.0;

impl NetworkLagLever {
    /// The automated harness's standard link: 45 ms delay, up to 60 ms jitter,
    /// 5% loss.
    pub fn set_standard_profile(&mut self) {
        self.delay_ms = 45;
        self.jitter_ms = 60;
        self.loss_percent = 5.0;
    }

    pub fn set_off(&mut self) {
        self.delay_ms = 0;
        self.jitter_ms = 0;
        self.loss_percent = 0.0;
    }

    /// Whether an impairment field differs from `before`. Read-only fields are
    /// ignored.
    #[must_use]
    pub fn impairment_changed(&self, before: &Self) -> bool {
        self.delay_ms != before.delay_ms
            || self.jitter_ms != before.jitter_ms
            || self.loss_percent != before.loss_percent
    }
}

pub(super) fn draw_network_tab(ui: &mut egui::Ui, lever: Option<&mut NetworkLagLever>) {
    let Some(lever) = lever else {
        ui.label("Not in a networked session");
        return;
    };
    egui::Grid::new("network_session_grid")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            ui.label("Role");
            ui.label(egui::RichText::new(lever.role.label()).monospace());
            ui.end_row();
            ui.label("Measured RTT");
            ui.label(egui::RichText::new(format_rtt(lever)).monospace());
            ui.end_row();
        });
    ui.separator();
    ui.label("Simulated lag");
    ui.label(
        egui::RichText::new(
            "Dev-tools lever: applies to each direction through this machine's proxy, \
             so RTT grows by about twice the delay. A host applies it to every client.",
        )
        .weak(),
    );
    // `Edits` keeps a launch seed beyond the slider range until it is dragged.
    ui.label("Delay (ms)");
    ui.add(
        egui::Slider::new(&mut lever.delay_ms, 0..=NETWORK_LAG_MAX_DELAY_MS)
            .clamping(egui::SliderClamping::Edits),
    );
    ui.label("Jitter (ms)");
    ui.add(
        egui::Slider::new(&mut lever.jitter_ms, 0..=NETWORK_LAG_MAX_JITTER_MS)
            .clamping(egui::SliderClamping::Edits),
    );
    ui.label("Loss (%)");
    ui.add(
        egui::Slider::new(&mut lever.loss_percent, 0.0..=NETWORK_LAG_MAX_LOSS_PERCENT)
            .step_by(0.5)
            .clamping(egui::SliderClamping::Edits),
    );
    ui.horizontal(|ui| {
        if ui.button("Standard (45 / 60 / 5%)").clicked() {
            lever.set_standard_profile();
        }
        if ui.button("Off").clicked() {
            lever.set_off();
        }
    });
}

fn format_rtt(lever: &NetworkLagLever) -> String {
    match (lever.measured_rtt_ms, lever.role) {
        (Some(rtt), _) => format!("{rtt:.1} ms"),
        (None, NetworkSessionRole::Host) => "n/a on host".to_owned(),
        (None, NetworkSessionRole::Client) => "waiting for first echo".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lever() -> NetworkLagLever {
        NetworkLagLever {
            delay_ms: 0,
            jitter_ms: 0,
            loss_percent: 0.0,
            role: NetworkSessionRole::Client,
            measured_rtt_ms: Some(12.0),
        }
    }

    #[test]
    fn presets_set_the_harness_standard_and_off() {
        let mut lever = lever();
        lever.set_standard_profile();
        assert_eq!(
            (lever.delay_ms, lever.jitter_ms, lever.loss_percent),
            (45, 60, 5.0)
        );
        lever.set_off();
        assert_eq!(
            (lever.delay_ms, lever.jitter_ms, lever.loss_percent),
            (0, 0, 0.0)
        );
    }

    #[test]
    fn only_impairment_fields_count_as_a_change() {
        let before = lever();
        let mut after = before;
        after.measured_rtt_ms = Some(40.0);
        after.role = NetworkSessionRole::Host;
        assert!(!after.impairment_changed(&before));
        for edit in [
            |l: &mut NetworkLagLever| l.delay_ms = 1,
            |l: &mut NetworkLagLever| l.jitter_ms = 1,
            |l: &mut NetworkLagLever| l.loss_percent = 0.5,
        ] {
            let mut after = before;
            edit(&mut after);
            assert!(after.impairment_changed(&before));
        }
    }

    #[test]
    fn rtt_reads_per_role() {
        let mut lever = lever();
        assert_eq!(format_rtt(&lever), "12.0 ms");
        lever.measured_rtt_ms = None;
        assert_eq!(format_rtt(&lever), "waiting for first echo");
        lever.role = NetworkSessionRole::Host;
        assert_eq!(format_rtt(&lever), "n/a on host");
    }
}
