// Capture's streamed-lightmap preload, overrides, and residency summary.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency), §7.8

use anyhow::{Context, Result, bail};
use postretro_level_format::lightmap_pool::{PerPool, PoolDemand};
use postretro_level_loader::{LevelWorld, LightmapStreamingMode};
use postretro_renderer::LightmapStreamingLiveDiagnostics;

use super::scene::CaptureScene;
use crate::render::Renderer;
use crate::render_preparation::VisibleRenderPreparation;
use crate::session::lightmap_residency::{LightmapLevelView, LightmapStreamingSession};
use crate::streaming::cluster_hints::ClusterHints;

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
    /// Usable layers of each streamed pool; `None` unless streaming.
    pub(super) pool_layers: Option<PerPool<u32>>,
    /// The streaming counters after the preload, the captured view's visible
    /// misses counted; `None` unless streaming.
    pub(super) counters: Option<LightmapStreamingLiveDiagnostics>,
}

/// Reject lightmap overrides the loaded level cannot honour, before any GPU
/// work: each override needs a streamed lightmap, `mask_pool_layers` needs a
/// mask pool, and a forced block must exist.
pub(super) fn validate_lightmap_overrides(scene: &CaptureScene, world: &LevelWorld) -> Result<()> {
    let demand = world.lightmap_stream_manifest().map(|manifest| {
        PoolDemand::of_indices(manifest.lightmap_index(), manifest.shadowmask_index())
    });
    check_lightmap_overrides(scene, demand.as_ref())
}

/// [`validate_lightmap_overrides`] over the streamed level's pool demand,
/// `None` when the level does not stream its lightmap.
fn check_lightmap_overrides(scene: &CaptureScene, streamed: Option<&PoolDemand>) -> Result<()> {
    let overrides = scene.lightmap_first_generation();
    let wants_streaming = !scene.force_missing_lightmap_blocks.is_empty()
        || overrides.lightmap.is_some()
        || overrides.mask.is_some();
    let Some(demand) = streamed else {
        if wants_streaming {
            bail!(
                "force_missing_lightmap_blocks, lightmap_pool_layers and mask_pool_layers need \
                 a streamed lightmap: a level with an id-51 residency set and portals, loaded \
                 with POSTRETRO_LIGHTMAP_STREAMING unset or `stream`"
            );
        }
        return Ok(());
    };
    let block_count = demand.block_count() as u32;
    // No id 42, or an id 42 whose every plane is absent: the mask pool has
    // no layers, so a figure for it would be silently 0.
    if overrides.mask.is_some() && demand.plane_count() == 0 {
        bail!("mask_pool_layers needs a level with a shadowmask: this level has no mask pool");
    }
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
/// captured frame, as capture preloads SH. `drawn_cells` is every cell the
/// capture draws: the main view's, plus the planar mirror's drawn cells.
/// Every block the view draws is targeted visible, whatever its visibility
/// path, since capture renders the full view. Reads are synchronous through
/// the level's positional reader and install in one renderer drain, so no
/// async timing can reach a capture; after this the pool never changes,
/// since capture runs no further lightmap drain. Forced-missing blocks stay
/// targeted and unread. A capture fails rather than render a partial set.
/// `hints` is the level's id 49, decoded once for SH and lightmaps.
pub(super) fn preload_capture_lightmap(
    world: &LevelWorld,
    renderer: &mut Renderer,
    visible_render: &VisibleRenderPreparation,
    drawn_cells: &postretro_visibility::VisibleCells,
    scene: &CaptureScene,
    hints: Option<&ClusterHints>,
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
            counters: None,
        });
    };

    let mut session = LightmapStreamingSession::new(view, hints)?;
    // Capture never sets L: its view demands at the level's default lead.
    session.update_capture_demand(
        crate::streaming::cell_demand::CellDemand::new(view.residency_set.max_lead).frame(
            view.residency_set,
            visible_render.stats.camera_cell,
            visible_render.stats.path,
            drawn_cells,
        ),
    );
    let summary = session
        .preload(&scene.force_missing_lightmap_blocks, |batch| {
            renderer.drain_lightmap_residency(batch)
        })
        .context("[Capture] lightmap preload drain")?;
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
        counters: Some(live),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::scene::parse_scene;

    /// A capture scene that sets only the mask pool's first generation.
    fn mask_override_scene() -> CaptureScene {
        parse_scene(
            r#"
            {
              "map": "content/dev/maps/test.prl",
              "camera": { "position": [1.0, 2.0, 3.0], "yaw_deg": 45.0, "pitch_deg": -10.0 },
              "resolution": [1280, 720],
              "output": "capture.png",
              "mask_pool_layers": 2
            }
        "#,
        )
        .expect("mask override scene must parse")
    }

    fn refusal(streamed: &PoolDemand) -> String {
        check_lightmap_overrides(&mask_override_scene(), Some(streamed))
            .expect_err("a mask pool override on a level with no mask pool is refused")
            .to_string()
    }

    #[test]
    fn mask_pool_override_refuses_a_streamed_level_with_no_shadowmask() {
        let no_id_42 = PoolDemand::lightmap_only(4, vec![(8, 4), (8, 4)]);
        assert!(refusal(&no_id_42).contains("this level has no mask pool"));
    }

    #[test]
    fn mask_pool_override_refuses_a_streamed_level_whose_shadowmask_stores_no_plane() {
        let no_plane = PoolDemand::new(4, vec![(8, 4), (8, 4)], Some(vec![[false; 2]; 2]));
        assert!(refusal(&no_plane).contains("this level has no mask pool"));

        // One stored plane gives the mask pool a layer, so the figure stands.
        let one_plane = PoolDemand::new(
            4,
            vec![(8, 4), (8, 4)],
            Some(vec![[false; 2], [false, true]]),
        );
        check_lightmap_overrides(&mask_override_scene(), Some(&one_plane))
            .expect("a level with a stored plane honours a mask pool override");
    }
}
