// The Build Summary's mover-lighting block: row totals, side counts, and the
// mask estimate against the same PRL's packed lightmap-family bytes.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use postretro_level_format::SectionId;

use super::lights::GateLights;
use super::pose::MoverPoses;
use super::side_count::SideCount;
use super::tally::GateTally;

/// The sections the estimate is a share of: the lightmap family, ids 22, 25
/// and 42, standing in for the camera-dependent residency mandatory set.
pub(crate) const LIGHTMAP_FAMILY: [SectionId; 3] = [
    SectionId::Lightmap,
    SectionId::AnimatedLightWeightMaps,
    SectionId::ShadowmaskAtlas,
];

/// The block's totals, gathered once every tier's rows are in. Texel and
/// probe totals sum over rows, so a texel gated at both termini counts once
/// per terminus, as the two state masks the estimate prices would store it.
#[derive(Debug, Default)]
pub(crate) struct MoverLightSummary {
    movers_analysed: usize,
    movers_not_analysed: usize,
    lights_not_analysed: usize,
    rows: usize,
    gated_texels: u64,
    shared_texels: u64,
    gated_probes: u64,
    shared_probes: u64,
    carried_lights: usize,
    other_lights: usize,
    every_gated_bytes: u64,
    differing_only_bytes: u64,
}

impl MoverLightSummary {
    pub(crate) fn new(
        poses: &MoverPoses,
        lights: &GateLights<'_>,
        tally: &GateTally,
        side_counts: &[SideCount],
    ) -> Self {
        let mut summary = Self {
            movers_analysed: poses.analysed.len(),
            movers_not_analysed: poses.not_analysed.len(),
            lights_not_analysed: lights.not_analysed.len(),
            ..Self::default()
        };
        for (_, row) in tally.rows() {
            summary.rows += 1;
            summary.gated_texels += row.gated_texels;
            summary.shared_texels += row.shared_texels;
            summary.gated_probes += row.gated_probes;
            summary.shared_probes += row.shared_probes;
            summary.every_gated_bytes += row.every_gated_estimate();
            summary.differing_only_bytes += row.differing_only_estimate();
        }
        for count in side_counts {
            summary.carried_lights += count.carried.len();
            summary.other_lights += count.others.len();
        }
        summary
    }

    /// The block's lines, given the PRL's packed lightmap-family bytes. A
    /// level without movers gets one line.
    pub(crate) fn lines(&self, lightmap_family_bytes: u64) -> Vec<String> {
        if self.movers_analysed + self.movers_not_analysed == 0 {
            return vec!["Mover lighting: 0 movers".to_string()];
        }
        vec![
            format!(
                "Mover lighting: {} mover(s) analysed, {} not analysed; {} row(s); \
                 {} baked light(s) not analysed",
                self.movers_analysed, self.movers_not_analysed, self.rows, self.lights_not_analysed,
            ),
            format!(
                "  gated est.: {} texel(s) ({} shared, {} differing); \
                 {} probe(s) ({} shared, {} differing)",
                self.gated_texels,
                self.shared_texels,
                self.gated_texels - self.shared_texels,
                self.gated_probes,
                self.shared_probes,
                self.gated_probes - self.shared_probes,
            ),
            format!(
                "  side count: {} carried dynamic light(s), {} other",
                self.carried_lights, self.other_lights,
            ),
            format!(
                "  mask est.: {} B every gated ({}), {} B differing only ({}) \
                 of {lightmap_family_bytes} B lightmap family (ids 22, 25, 42)",
                self.every_gated_bytes,
                share(self.every_gated_bytes, lightmap_family_bytes),
                self.differing_only_bytes,
                share(self.differing_only_bytes, lightmap_family_bytes),
            ),
        ]
    }
}

/// `bytes` as a percentage of `total`, or "n/a" when `total` is zero.
fn share(bytes: u64, total: u64) -> String {
    if total == 0 {
        "n/a".to_string()
    } else {
        format!("{:.2}%", bytes as f64 * 100.0 / total as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> MoverLightSummary {
        MoverLightSummary {
            movers_analysed: 2,
            movers_not_analysed: 1,
            lights_not_analysed: 3,
            rows: 4,
            gated_texels: 900,
            shared_texels: 600,
            gated_probes: 50,
            shared_probes: 20,
            carried_lights: 1,
            other_lights: 2,
            every_gated_bytes: 1_000,
            differing_only_bytes: 240,
        }
    }

    #[test]
    fn the_block_states_each_estimate_as_a_share_of_the_lightmap_family() {
        let lines = summary().lines(8_000);
        assert_eq!(
            lines,
            [
                "Mover lighting: 2 mover(s) analysed, 1 not analysed; 4 row(s); \
                 3 baked light(s) not analysed",
                "  gated est.: 900 texel(s) (600 shared, 300 differing); \
                 50 probe(s) (20 shared, 30 differing)",
                "  side count: 1 carried dynamic light(s), 2 other",
                "  mask est.: 1000 B every gated (12.50%), 240 B differing only (3.00%) \
                 of 8000 B lightmap family (ids 22, 25, 42)",
            ]
        );
    }

    #[test]
    fn the_percentage_reads_n_a_when_the_lightmap_family_is_empty() {
        let lines = summary().lines(0);
        assert!(
            lines[3].contains("1000 B every gated (n/a), 240 B differing only (n/a) of 0 B"),
            "{}",
            lines[3]
        );
    }

    #[test]
    fn a_level_without_movers_prints_one_line() {
        assert_eq!(
            MoverLightSummary::default().lines(8_000),
            ["Mover lighting: 0 movers"]
        );
    }
}
