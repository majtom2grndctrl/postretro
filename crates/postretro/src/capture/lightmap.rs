// Capture's streamed-lightmap preload, overrides, and residency summary.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency), §7.8

use anyhow::{Context, Result, bail};
use postretro_level_loader::{LevelWorld, LightmapStreamingMode};
use postretro_renderer::LightmapStreamingLiveDiagnostics;

use super::scene::CaptureScene;
use crate::lightmap_streaming::demand::DemandFrame;
use crate::render::Renderer;
use crate::render_preparation::VisibleRenderPreparation;
use crate::session::lightmap_residency::{LightmapLevelView, LightmapStreamingSession};

/// How the captured level owned its lightmap blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CaptureLightmapMode {
    /// No id 22, or zero blocks: the neutral placeholder.
    Placeholder,
    AllResident,
    Stream,
}

impl CaptureLightmapMode {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Placeholder => "placeholder",
            Self::AllResident => "all-resident",
            Self::Stream => "stream",
        }
    }
}

/// The lightmap residency a capture rendered with, for its report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CaptureLightmapResidency {
    pub(super) mode: CaptureLightmapMode,
    pub(super) block_count: u32,
    pub(super) resident_blocks: u32,
    pub(super) forced_missing_blocks: u32,
    /// Usable layers of the streamed pool; `None` unless streaming.
    pub(super) pool_layers: Option<u32>,
    /// The cap every streamed drain carried; `None` unless streaming.
    pub(super) pool_cap_layers: Option<u32>,
    /// The streaming counters after the preload, the captured view's visible
    /// misses counted; `None` unless streaming.
    pub(super) counters: Option<LightmapStreamingLiveDiagnostics>,
}

/// Reject lightmap overrides the loaded level cannot honour, before any GPU
/// work: both need a streamed lightmap, and a forced block must exist.
pub(super) fn validate_lightmap_overrides(scene: &CaptureScene, world: &LevelWorld) -> Result<()> {
    let wants_streaming =
        !scene.force_missing_lightmap_blocks.is_empty() || scene.lightmap_pool_cap_layers.is_some();
    let Some(manifest) = world.lightmap_stream_manifest() else {
        if wants_streaming {
            bail!(
                "force_missing_lightmap_blocks and lightmap_pool_cap_layers need a streamed \
                 lightmap: a level with an id-51 residency set and portals, loaded with \
                 POSTRETRO_LIGHTMAP_STREAMING unset or `stream`"
            );
        }
        return Ok(());
    };
    let block_count = manifest.block_count();
    if let Some(&block) = scene
        .force_missing_lightmap_blocks
        .iter()
        .find(|&&block| block >= block_count)
    {
        bail!("force_missing_lightmap_blocks names block {block}, past the level's {block_count}");
    }
    Ok(())
}

/// Make the view's mandatory and visible lightmap blocks resident before any
/// captured frame, as capture preloads SH. Reads are synchronous through the
/// level's positional reader and install in one renderer drain, so no async
/// timing can reach a capture; after this the pool never changes, since
/// capture runs no further lightmap drain. Forced-missing blocks stay
/// targeted and unread. A capture fails rather than render a partial set.
pub(super) fn preload_capture_lightmap(
    world: &LevelWorld,
    renderer: &mut Renderer,
    visible_render: &VisibleRenderPreparation,
    scene: &CaptureScene,
) -> Result<CaptureLightmapResidency> {
    let block_count = world
        .lightmap
        .as_ref()
        .map_or(0, |index| index.records.len() as u32);
    let Some(view) = LightmapLevelView::of(world) else {
        let mode = match (world.lightmap_storage().mode(), block_count) {
            (_, 0) => CaptureLightmapMode::Placeholder,
            (LightmapStreamingMode::AllResident, _) => CaptureLightmapMode::AllResident,
            // A streamed manifest without id 51 cannot reach here: the loader
            // streams only with a usable residency set.
            (LightmapStreamingMode::Stream, _) => {
                bail!("capture level streams its lightmap without a residency set")
            }
        };
        return Ok(CaptureLightmapResidency {
            mode,
            block_count,
            resident_blocks: block_count,
            forced_missing_blocks: 0,
            pool_layers: None,
            pool_cap_layers: None,
            counters: None,
        });
    };

    let mut session = LightmapStreamingSession::new(view)?;
    if let Some(cap) = scene.lightmap_pool_cap_layers {
        session.levers_mut().set_pool_cap_layers(cap);
    }
    session.update_demand(DemandFrame {
        residency_set: view.residency_set,
        camera_cell: visible_render.stats.camera_cell,
        path: visible_render.stats.path,
        visible_cells: &visible_render.visible_cells,
    });
    let summary = session.preload(&scene.force_missing_lightmap_blocks, |batch| {
        renderer
            .drain_lightmap_residency(batch)
            .context("[Capture] lightmap preload drain")
    })?;
    if summary.reads.failed != 0
        || summary.failed_installs != 0
        || summary.deferred != 0
        || summary.installed != summary.reads.pairs
    {
        bail!(
            "capture lightmap preload left its view partly non-resident: {} pair(s) read, {} \
             installed, {} read failure(s), {} install failure(s), {} deferred",
            summary.reads.pairs,
            summary.installed,
            summary.reads.failed,
            summary.failed_installs,
            summary.deferred,
        );
    }
    let counters = renderer.lightmap_stream_counters();
    // The captured frame draws after this one drain: count its misses now.
    session.refresh_diagnostics(counters.as_ref());
    let live = *session.live_diagnostics();
    let mut forced = scene.force_missing_lightmap_blocks.clone();
    forced.sort_unstable();
    forced.dedup();
    Ok(CaptureLightmapResidency {
        mode: CaptureLightmapMode::Stream,
        block_count,
        resident_blocks: summary.installed,
        forced_missing_blocks: forced.len() as u32,
        pool_layers: counters.map(|counters| counters.pool_layers),
        pool_cap_layers: Some(live.pool_cap_layers),
        counters: Some(live),
    })
}
