//! CPU-only drain-boundary types for streamed lightmap cell blocks (ids 22/42).
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

mod boundary;

pub use boundary::{
    LightmapBlockClass, LightmapDrainBatch, LightmapDrainOutcome, LightmapPoolReport,
    LightmapStreamingMode, LightmapTarget, PreparedLightmapBlock,
};

use crate::prl::PrlLoadError;

/// Resolve the developer/test gate for lightmap residency. Unset means
/// `Stream` when the level carries a usable id-51 residency set; a level
/// without one runs all-resident regardless.
pub fn requested_lightmap_streaming_mode() -> Result<LightmapStreamingMode, PrlLoadError> {
    match std::env::var("POSTRETRO_LIGHTMAP_STREAMING") {
        Err(std::env::VarError::NotPresent) => Ok(LightmapStreamingMode::Stream),
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
