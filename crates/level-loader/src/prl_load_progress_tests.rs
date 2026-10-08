// Proofs that a PRL load's progress rises monotonically to done == total on
// both container backings, counting only what the load reads.
// See: context/lib/testing_guide.md · context/lib/boot_sequence.md §2

use std::path::PathBuf;
use std::sync::Arc;

use postretro_level_format::cell_residency_set::{CellResidencySetSection, ResidencyEntry};
use postretro_level_format::lightmap::{
    DIRECTION_TEXEL_BYTES, IRRADIANCE_FORMAT_RGBA16F, IRRADIANCE_TEXEL_BYTES, LightmapBlock,
    LightmapMode, LightmapSection,
};
use postretro_level_format::{SectionBlob, SectionId};

use crate::LoadProgress;
use crate::lightmap_stream::LightmapStreamingMode;
use crate::prl_load_test_fixtures::write_portal_prl_load_fixture;
use crate::prl_streaming::load_prl_with_modes_reporting_for_test;
use crate::sh_stream::ShStreamingMode;

/// A two-cell portal level with a residency set and a lightmap large enough to
/// dominate the file, so a backing that streams it and one that reads it whole
/// plan visibly different totals.
struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Self {
        let block = |cell_id: u32| {
            let (width, height) = (64u16, 64u16);
            let texels = usize::from(width) * usize::from(height);
            LightmapBlock {
                cell_id,
                width,
                height,
                irradiance: vec![cell_id as u8 + 1; texels * IRRADIANCE_TEXEL_BYTES],
                direction: vec![7; texels / 4 * DIRECTION_TEXEL_BYTES],
            }
        };
        let lightmap = LightmapSection {
            direction_texel_scale: 2,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            mode: LightmapMode::Shadowed,
            blocks: vec![block(0), block(1)],
        };
        let entry = |cell_id| ResidencyEntry { lead: 0, cell_id };
        let residency = CellResidencySetSection {
            max_lead: 32 * 1024,
            offsets: vec![0, 2, 4],
            entries: vec![entry(0), entry(1), entry(0), entry(1)],
        };
        let blob = |section_id: SectionId, data: Vec<u8>| SectionBlob {
            section_id: section_id as u32,
            version: 1,
            data,
        };
        Self(write_portal_prl_load_fixture(
            [
                blob(SectionId::CellResidencySet, residency.to_bytes()),
                blob(SectionId::Lightmap, lightmap.to_bytes()),
            ],
            name,
        ))
    }

    fn file_len(&self) -> u64 {
        std::fs::metadata(&self.0).unwrap().len()
    }

    /// Load under `lightmap` with SH off and return the counter's history.
    fn load(&self, lightmap: LightmapStreamingMode) -> (crate::LevelWorld, Vec<(u64, u64)>) {
        let progress = Arc::new(LoadProgress::new());
        let world = load_prl_with_modes_reporting_for_test(
            self.0.to_str().unwrap(),
            ShStreamingMode::Off,
            lightmap,
            &progress,
        )
        .unwrap_or_else(|error| panic!("{}: load failed: {error}", self.0.display()));
        assert_eq!(progress.fraction(), 1.0, "a returned load reads complete");
        (world, progress.history())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

/// Every step's `done / total` is at least the previous one's, and the last
/// step has `done == total`. Cross-multiplied so no float rounding hides a dip.
fn assert_monotonic_to_completion(history: &[(u64, u64)]) {
    assert!(
        history.len() > 2,
        "the load reported its steps: {history:?}"
    );
    for pair in history.windows(2) {
        let [(done_a, total_a), (done_b, total_b)] = [pair[0], pair[1]];
        assert!(
            u128::from(done_b) * u128::from(total_a.max(1))
                >= u128::from(done_a) * u128::from(total_b.max(1)),
            "progress fell from {done_a}/{total_a} to {done_b}/{total_b}: {history:?}"
        );
        assert!(done_b <= total_b, "done passed total: {history:?}");
    }
    let (done, total) = *history.last().unwrap();
    assert_eq!(done, total, "a successful load ends at done == total");
}

/// The fraction just before `finish` settles the plan: how much of the bar
/// the load had already earned when it returned.
fn fraction_before_finish(history: &[(u64, u64)]) -> f64 {
    let (done, total) = history[history.len() - 2];
    done as f64 / total.max(1) as f64
}

#[test]
fn whole_file_backing_progress_rises_through_the_image_read_and_ends_at_total() {
    let fixture = Fixture::new("postretro_test_progress_whole_file.prl");
    let (world, history) = fixture.load(LightmapStreamingMode::AllResident);
    assert!(!world.lightmap_storage().is_streaming());

    assert_monotonic_to_completion(&history);
    // The whole-file plan counts the image read itself on top of the sections.
    let (_, planned) = history[0];
    assert!(
        planned > fixture.file_len(),
        "plan {planned} counts the image read"
    );
    assert!(
        fraction_before_finish(&history) > 0.95,
        "the bar earns its progress during the load, not at the return: {history:?}"
    );
}

#[test]
fn positional_backing_progress_skips_streamed_lightmap_blocks_and_ends_at_total() {
    let fixture = Fixture::new("postretro_test_progress_positional.prl");
    let (world, history) = fixture.load(LightmapStreamingMode::Stream);
    assert!(world.lightmap_storage().is_streaming());

    assert_monotonic_to_completion(&history);
    // Only the lightmap's index prefix is read; its blocks stay on disk and
    // leave the plan, so the load counts far less than the file holds.
    let (_, total) = *history.last().unwrap();
    assert!(
        total * 4 < fixture.file_len(),
        "streamed blocks are not counted: {total} of {} bytes",
        fixture.file_len()
    );
    assert!(
        fraction_before_finish(&history) > 0.95,
        "the bar earns its progress during the load, not at the return: {history:?}"
    );
}
