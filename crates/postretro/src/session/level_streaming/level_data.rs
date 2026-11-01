//! The streaming sessions' level-data checks, decoded once per load: the SH
//! mode gate, the id-49 hints, SH's planner topology and the lightmap's block
//! map. Pure CPU and `Send`: load prep runs it off the main thread, and the
//! oracle install runs the same function before it creates the sessions.
//! See: context/lib/rendering_pipeline.md §4 · context/plans/done/level-load-pipeline/index.md (The commit cannot fail on level data)

use std::sync::Arc;

use anyhow::{Context, Result};
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_loader::{
    LevelWorld, LightmapStreamManifest, ShStreamManifest, ShStreamingMode,
};

use crate::lightmap_streaming::LightmapResidencyError;
use crate::lightmap_streaming::block_map::LevelBlockMap;
use crate::lightmap_streaming::source::ManifestBlockSource;
use crate::session::lightmap_residency::LightmapLevelView;
use crate::session::sh_residency::require_loaded_streaming_mode;
use crate::sh_streaming::controller::ShLevelTopology;
use crate::streaming::cluster_hints::{ClusterHints, decode_level_hints};

/// What a level's streaming sessions are built from, every level-data check
/// already passed. Creating the sessions from it can fail only on what is not
/// level data: the renderer's residency snapshot and the budget it yields,
/// and the generation clocks.
#[derive(Debug, Default)]
pub(crate) struct StreamingLevelData {
    /// The level's id-49 hints; `None` without id 49, or when nothing
    /// streams.
    pub(crate) hints: Option<Arc<ClusterHints>>,
    pub(crate) sh: Option<ShLevelData>,
    pub(crate) lightmap: Option<LightmapLevelData>,
}

/// A streamed-SH level's mode and planner topology.
#[derive(Debug)]
pub(crate) struct ShLevelData {
    pub(crate) manifest: Arc<ShStreamManifest>,
    pub(crate) mode: ShStreamingMode,
    pub(crate) topology: ShLevelTopology,
}

/// A streamed-lightmap level's block map.
#[derive(Debug)]
pub(crate) struct LightmapLevelData {
    pub(crate) manifest: Arc<LightmapStreamManifest>,
    pub(crate) blocks: LevelBlockMap,
}

// Compile-time guard: load prep decodes this on a pool thread and the commit
// takes it on the main thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<StreamingLevelData>();
};

impl StreamingLevelData {
    /// A summary for equivalence tests: every decoded table, and each
    /// manifest by its content tag.
    #[cfg(test)]
    pub(crate) fn summary(&self) -> String {
        format!(
            "{:?}",
            (
                &self.hints,
                self.sh.as_ref().map(|sh| (
                    sh.manifest.content_tag(),
                    sh.mode,
                    sh.topology.summary()
                )),
                self.lightmap
                    .as_ref()
                    .map(|lightmap| (lightmap.manifest.content_tag(), &lightmap.blocks)),
            )
        )
    }
}

/// The level parts the checks read, all from one load.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamingLevelParts<'a> {
    pub(crate) sh_manifest: Option<&'a Arc<ShStreamManifest>>,
    pub(crate) lightmap: Option<LightmapLevelView<'a>>,
    pub(crate) cluster_directory: Option<&'a ClusterDirectorySection>,
}

impl<'a> StreamingLevelParts<'a> {
    pub(crate) fn of(world: &'a LevelWorld) -> Self {
        Self {
            sh_manifest: world.sh_stream_manifest(),
            lightmap: LightmapLevelView::of(world),
            cluster_directory: world.cluster_directory(),
        }
    }
}

/// Every check the streaming sessions make on the level's own data, in the
/// order session creation makes them, with its errors. `sh_mode` reads the
/// `POSTRETRO_SH_STREAMING` mode, and only for a streamed-SH level: the oracle
/// reads the environment, prep its dispatch snapshot.
///
/// A level that streams nothing decodes nothing, not even its hints, as the
/// sessions never did.
pub(crate) fn decode_streaming_level_data(
    level: StreamingLevelParts<'_>,
    sh_mode: impl FnOnce() -> Result<ShStreamingMode>,
) -> Result<StreamingLevelData> {
    let sh = match level.sh_manifest {
        Some(manifest) => {
            let mode = sh_mode()?;
            require_loaded_streaming_mode(mode)?;
            Some((manifest, mode))
        }
        None => None,
    };
    if sh.is_none() && level.lightmap.is_none() {
        return Ok(StreamingLevelData::default());
    }
    // Tagged for the shared layer: the hints serve both resources.
    let hints =
        decode_level_hints(level.cluster_directory).context("[Streaming] id-49 cluster hints")?;
    let sh = match sh {
        Some((manifest, mode)) => {
            let level_hints = hints
                .clone()
                .context("[SH streaming] a streamed level carries no id 49")?;
            Some(ShLevelData {
                manifest: Arc::clone(manifest),
                mode,
                topology: ShLevelTopology::decode(manifest, level_hints)?,
            })
        }
        None => None,
    };
    let lightmap = match level.lightmap {
        Some(view) => Some(LightmapLevelData {
            manifest: Arc::clone(view.manifest),
            blocks: build_block_map(view, hints.as_deref())?,
        }),
        None => None,
    };
    Ok(StreamingLevelData {
        hints,
        sh,
        lightmap,
    })
}

fn build_block_map(
    view: LightmapLevelView<'_>,
    hints: Option<&ClusterHints>,
) -> Result<LevelBlockMap, LightmapResidencyError> {
    LevelBlockMap::build(
        &ManifestBlockSource::new(Arc::clone(view.manifest)),
        view.residency_set.camera_cell_count(),
        hints,
    )
}
