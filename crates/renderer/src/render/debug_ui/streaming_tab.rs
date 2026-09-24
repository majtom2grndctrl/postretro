//! Streaming tab: SH cluster residency gauges and cumulative counters.
//!
//! Rows are built as plain label/value strings first so the grouping and
//! formatting are testable without an egui context.

use super::super::ShStreamingLiveDiagnostics;

pub(super) fn draw_streaming_tab(
    ui: &mut egui::Ui,
    diagnostics: Option<&ShStreamingLiveDiagnostics>,
) {
    let Some(diagnostics) = diagnostics else {
        ui.label("SH streaming inactive");
        return;
    };
    for section in streaming_sections(diagnostics) {
        egui::CollapsingHeader::new(section.title)
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new(("sh_streaming_grid", section.title))
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (label, value) in &section.rows {
                            ui.label(*label);
                            ui.label(egui::RichText::new(value).monospace());
                            ui.end_row();
                        }
                    });
            });
    }
}

struct StreamingSection {
    title: &'static str,
    rows: Vec<(&'static str, String)>,
}

fn streaming_sections(d: &ShStreamingLiveDiagnostics) -> [StreamingSection; 4] {
    [
        StreamingSection {
            title: "Residency",
            rows: vec![
                ("Target clusters", d.target_clusters.to_string()),
                ("Warm clusters", d.warm_clusters.to_string()),
                ("Sampleable clusters", d.sampleable_clusters.to_string()),
                ("Queued clusters", d.queued_clusters.to_string()),
                ("Ready clusters", d.ready_clusters.to_string()),
                ("Permits in use", d.permits_in_use.to_string()),
                ("Pool capacity", format_bytes(d.active_capacity_bytes)),
                (
                    "Logical occupancy",
                    format!(
                        "{} ({})",
                        format_bytes(d.logical_occupancy_bytes),
                        format_percent(d.logical_occupancy_bytes, d.active_capacity_bytes),
                    ),
                ),
            ],
        },
        StreamingSection {
            title: "I/O",
            rows: vec![
                ("Physical reads", d.reads_issued.to_string()),
                (
                    "Coalesced reads",
                    format!(
                        "{} ({} of reads)",
                        d.coalesced_reads,
                        format_percent(d.coalesced_reads, d.reads_issued),
                    ),
                ),
                ("Read bytes", format_bytes(d.read_bytes)),
                (
                    "Gap bytes",
                    format!(
                        "{} ({} of read)",
                        format_bytes(d.gap_bytes),
                        format_percent(d.gap_bytes, d.read_bytes),
                    ),
                ),
                (
                    "Read latency p50/p95/max",
                    format!(
                        "{:.2} / {:.2} / {:.2} ms",
                        d.read_latency_p50_ms, d.read_latency_p95_ms, d.read_latency_max_ms,
                    ),
                ),
                (
                    "Decode latency max",
                    format!("{:.2} ms", d.decode_latency_max_ms),
                ),
                (
                    "Discarded reads",
                    format!(
                        "{} ({})",
                        d.discarded_reads,
                        format_bytes(d.discarded_read_bytes),
                    ),
                ),
                ("Cancelled requests", d.cancelled_requests.to_string()),
            ],
        },
        StreamingSection {
            title: "Installs",
            rows: vec![
                ("Installs", d.installs.to_string()),
                ("Evictions", d.evictions.to_string()),
                ("Misses", d.misses.to_string()),
                ("Retries", d.retries.to_string()),
                ("Decoded installed", format_bytes(d.decoded_bytes_installed)),
                (
                    "Decoded last/max drain",
                    format!(
                        "{} / {}",
                        format_bytes(d.last_drain_decoded_bytes),
                        format_bytes(d.max_drain_decoded_bytes),
                    ),
                ),
                ("Budget-limited drains", d.budget_limited_drains.to_string()),
                (
                    "Install CPU last/max drain",
                    format!(
                        "{} / {}",
                        format_micros(d.install_cpu_last_drain_micros),
                        format_micros(d.install_cpu_max_drain_micros),
                    ),
                ),
                (
                    "Install CPU total",
                    format_micros(d.install_cpu_total_micros),
                ),
            ],
        },
        StreamingSection {
            title: "Pool growth",
            rows: vec![
                ("Growth events", d.pool_growth_events.to_string()),
                ("Capacity added", format_bytes(d.pool_growth_bytes)),
            ],
        },
    ]
}

/// Binary units, one decimal place above bytes.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = UNITS[0];
    for next in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    format!("{value:.1} {unit}")
}

fn format_micros(micros: u64) -> String {
    if micros < 1_000 {
        format!("{micros} µs")
    } else if micros < 1_000_000 {
        format!("{:.2} ms", micros as f64 / 1_000.0)
    } else {
        format!("{:.2} s", micros as f64 / 1_000_000.0)
    }
}

/// `part` as a whole percentage of `whole`; a dash when `whole` is zero.
fn format_percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "-".to_string();
    }
    format!("{:.0}%", part as f64 * 100.0 / whole as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_format_in_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(8 * 1024 * 1024), "8.0 MiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GiB");
    }

    #[test]
    fn install_times_scale_from_micros_to_seconds() {
        assert_eq!(format_micros(850), "850 µs");
        assert_eq!(format_micros(12_345), "12.35 ms");
        assert_eq!(format_micros(3_210_000), "3.21 s");
    }

    #[test]
    fn percentages_tolerate_an_empty_denominator() {
        assert_eq!(format_percent(5, 0), "-");
        assert_eq!(format_percent(1, 4), "25%");
    }

    #[test]
    fn sections_group_gauges_io_installs_and_growth() {
        let diagnostics = ShStreamingLiveDiagnostics {
            reads_issued: 8,
            coalesced_reads: 2,
            active_capacity_bytes: 4 * 1024 * 1024,
            logical_occupancy_bytes: 1024 * 1024,
            pool_growth_events: 3,
            pool_growth_bytes: 2048,
            install_cpu_last_drain_micros: 420,
            install_cpu_max_drain_micros: 1_500,
            ..ShStreamingLiveDiagnostics::default()
        };
        let sections = streaming_sections(&diagnostics);
        let titles: Vec<_> = sections.iter().map(|section| section.title).collect();
        assert_eq!(titles, ["Residency", "I/O", "Installs", "Pool growth"]);

        let value = |title: &str, label: &str| {
            sections
                .iter()
                .find(|section| section.title == title)
                .and_then(|section| section.rows.iter().find(|(row, _)| *row == label))
                .map(|(_, value)| value.clone())
                .unwrap()
        };
        assert_eq!(value("Residency", "Logical occupancy"), "1.0 MiB (25%)");
        assert_eq!(value("I/O", "Coalesced reads"), "2 (25% of reads)");
        assert_eq!(
            value("Installs", "Install CPU last/max drain"),
            "420 µs / 1.50 ms"
        );
        assert_eq!(value("Pool growth", "Growth events"), "3");
        assert_eq!(value("Pool growth", "Capacity added"), "2.0 KiB");
    }
}
