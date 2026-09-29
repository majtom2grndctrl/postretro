//! Streamed lightmap cell blocks (ids 22/42): mode, manifest, load reads, drain boundary.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

mod boundary;
mod load;
mod manifest;
mod storage;

pub use boundary::{
    LightmapBlockClass, LightmapDrainBatch, LightmapDrainOutcome, LightmapPoolReport,
    LightmapStreamingMode, LightmapTarget, PreparedLightmapBlock,
};
pub(crate) use load::read_lightmap_for_residency;
pub use manifest::{LightmapBlockFileRanges, LightmapStreamManifest};
pub(crate) use storage::LightmapResidencyInputs;
pub use storage::LightmapStorage;

use crate::prl::PrlLoadError;

/// Resolve the developer/test gate for lightmap residency. Unset means
/// `AllResident` until the streaming runtime (controller and renderer
/// streaming) lands; Task 9b of `spatial-residency--lightmap-cell-blocks`
/// flips the unset default to `Stream`. `stream` opts in; a level without a
/// usable id-51 set runs all-resident regardless.
pub fn requested_lightmap_streaming_mode() -> Result<LightmapStreamingMode, PrlLoadError> {
    match std::env::var("POSTRETRO_LIGHTMAP_STREAMING") {
        Err(std::env::VarError::NotPresent) => Ok(LightmapStreamingMode::AllResident),
        Ok(value) if value == "all-resident" => Ok(LightmapStreamingMode::AllResident),
        Ok(value) if value == "stream" => Ok(LightmapStreamingMode::Stream),
        Ok(value) => Err(lightmap_stream_error(format!(
            "POSTRETRO_LIGHTMAP_STREAMING must be `all-resident` or `stream`, got `{value}`"
        ))),
        Err(std::env::VarError::NotUnicode(_)) => Err(lightmap_stream_error(
            "POSTRETRO_LIGHTMAP_STREAMING is not unicode",
        )),
    }
}

pub(crate) fn lightmap_stream_error(message: impl Into<String>) -> PrlLoadError {
    PrlLoadError::SectionValidation {
        section: "lightmap streaming",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
