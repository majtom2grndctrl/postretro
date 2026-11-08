// Runtime rewrite of one probe's depth-tile reference on an installed level:
// the seam the mover probe channel redirects or clears a probe's tile through.
// See: context/lib/rendering_pipeline.md §4 (Probe depth moments)

use postretro_render_cpu::sh_depth_tiles::{DepthTileReferenceError, ProbeDepthTileReferences};

use super::Renderer;
use super::level_resource_set::LevelResourceSet;
use super::sh_indirection::SH_INDIRECTION_VALID_BIT;
use super::sh_streaming::ShResidencyState;
use super::sh_volume::ShVolumeResources;
use super::uploads::UploadQueue;

/// Why a depth-tile reference rewrite changed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeDepthTileRewriteError {
    /// No level set is published.
    NoLevel,
    /// The dense index is past the level's probe lattice (or the level has
    /// no usable SH, so no lattice).
    ProbeOutsideLattice { probe: u32, probe_count: u32 },
    /// The probe sits in solid or off-grid; no sampler weighs it.
    InvalidProbe { probe: u32 },
    /// The tile is past the level's id-59 tile texture.
    Reference(DepthTileReferenceError),
}

impl From<DepthTileReferenceError> for ProbeDepthTileRewriteError {
    fn from(error: DepthTileReferenceError) -> Self {
        Self::Reference(error)
    }
}

impl std::fmt::Display for ProbeDepthTileRewriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoLevel => write!(f, "no level is installed"),
            Self::ProbeOutsideLattice { probe, probe_count } => write!(
                f,
                "probe {probe} is outside the level's {probe_count}-probe lattice"
            ),
            Self::InvalidProbe { probe } => write!(f, "probe {probe} is not a valid probe"),
            Self::Reference(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ProbeDepthTileRewriteError {}

impl Renderer {
    /// Point dense lattice probe `probe` of the published level at depth tile
    /// `tile`, or clear its reference with `None` so it weighs with its id-34
    /// averaged moments again, bit for bit as if it never had a tile.
    ///
    /// The seam for the mover probe channel
    /// (`context/plans/drafts/mover-light-flow--probe-channel/`): a mover
    /// state redirects a probe to another tile in the level's texture, or
    /// clears it. The next frame samples the change through the same texel
    /// and bindings. Only the published load's table changes, so another
    /// load, of the same map or not, starts from its own id 59.
    pub fn set_probe_depth_tile_reference(
        &mut self,
        probe: u32,
        tile: Option<u16>,
    ) -> Result<(), ProbeDepthTileRewriteError> {
        let Self { queue, full, .. } = self;
        let level = full
            .as_mut()
            .and_then(|full| full.level.as_mut())
            .ok_or(ProbeDepthTileRewriteError::NoLevel)?;
        level.set_probe_depth_tile_reference(queue, probe, tile)
    }
}

impl LevelResourceSet {
    /// [`Renderer::set_probe_depth_tile_reference`] on this set.
    pub(in crate::render) fn set_probe_depth_tile_reference(
        &mut self,
        queue: &UploadQueue,
        probe: u32,
        tile: Option<u16>,
    ) -> Result<(), ProbeDepthTileRewriteError> {
        set_depth_tile_reference(
            &mut self.sh_volume_resources,
            self.sh_streaming.as_mut(),
            queue,
            probe,
            tile,
        )
    }
}

/// Rewrite `probe`'s reference in `sh`'s table, the load's one table. A
/// whole-load level stages the probe's texel on the frame batch now; a
/// streamed one (`streaming`) queues it for the next drain's promotion and
/// eviction batch, so it orders with that drain's own texel writes.
pub(in crate::render) fn set_depth_tile_reference(
    sh: &mut ShVolumeResources,
    streaming: Option<&mut ShResidencyState>,
    queue: &UploadQueue,
    probe: u32,
    tile: Option<u16>,
) -> Result<(), ProbeDepthTileRewriteError> {
    if let Some(state) = streaming {
        state.check_depth_tile_probe(probe)?;
        if rewrite(sh.depth_tiles.references_mut(), probe, tile)? {
            state.queue_depth_tile_texel(probe);
        }
        return Ok(());
    }
    let words = &sh.probe_indirection_words;
    let word =
        *words
            .get(probe as usize)
            .ok_or(ProbeDepthTileRewriteError::ProbeOutsideLattice {
                probe,
                probe_count: words.len() as u32,
            })?;
    if word & SH_INDIRECTION_VALID_BIT == 0 {
        return Err(ProbeDepthTileRewriteError::InvalidProbe { probe });
    }
    if !rewrite(sh.depth_tiles.references_mut(), probe, tile)? {
        return Ok(());
    }
    // A change needs a level with tiles, and a whole-loaded level with tiles
    // keeps its id-34 moments for exactly this repack.
    let texel = sh
        .depth_tiles
        .whole_load_texel(probe, word)
        .expect("a whole-loaded level with tiles keeps its id-34 moments");
    let [width, height, _] = sh.grid_dimensions;
    let bytes: Vec<u8> = texel.iter().flat_map(|value| value.to_le_bytes()).collect();
    queue.write_texture(
        sh.depth_moment_texture(),
        wgpu::Origin3d {
            x: probe % width,
            y: probe / width % height,
            z: probe / (width * height),
        },
        &bytes,
        bytes.len() as u32,
        1,
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}

/// Apply the rewrite to the table; whether the probe's reference changed.
fn rewrite(
    references: &mut ProbeDepthTileReferences,
    probe: u32,
    tile: Option<u16>,
) -> Result<bool, DepthTileReferenceError> {
    let before = references.tile_of(probe);
    references.set_reference(probe, tile)?;
    Ok(before != tile)
}
