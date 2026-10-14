// `movement.swim` assembly and its warn-and-drop policy, shared by the QuickJS
// and Luau movement parsers so both runtimes keep or drop a block identically,
// with the same diagnostics.
// See: context/lib/movement.md §2 (author surface) · context/lib/scripting.md §1 (parser twins)

use postretro_foundation::data_descriptors::SwimParams;

use super::DescriptorError;

/// A swim field that is absent, `null`/`undefined` or `nil`.
pub(crate) fn swim_field_missing(field: &str) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: format!("`movement.swim.{field}` is required"),
    }
}

/// A swim field holding something other than a number.
pub(crate) fn swim_field_not_a_number(field: &str) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: format!("`movement.swim.{field}` must be a number"),
    }
}

/// A `movement.swim` value that is neither absent nor an object (a table, in
/// Luau).
pub(crate) fn swim_block_not_an_object() -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: "`movement.swim` must be an object".to_string(),
    }
}

/// Read the six fields in authored order through `read` (one VM's field
/// reader, `None` for an absent or null field), then range-check them with
/// [`SwimParams::validated`]. `fluidJumpVelocity` is optional; the rest are
/// required.
pub(crate) fn swim_params_from_fields(
    mut read: impl FnMut(&'static str) -> Result<Option<f32>, DescriptorError>,
) -> Result<SwimParams, DescriptorError> {
    let mut required = |field| read(field)?.ok_or_else(|| swim_field_missing(field));
    let speed = required("speed")?;
    let accel = required("accel")?;
    let sink_speed = required("sinkSpeed")?;
    let enter_depth = required("enterDepth")?;
    let exit_depth = required("exitDepth")?;
    SwimParams {
        speed,
        accel,
        sink_speed,
        enter_depth,
        exit_depth,
        fluid_jump_velocity: read("fluidJumpVelocity")?,
    }
    .validated()
}

/// Keep a parsed swim block, or drop a malformed one with a warning naming the
/// field. A dropped block leaves the descriptor without swim: the player wades,
/// and the rest of the descriptor and mod load. Swim is optional tuning, so a
/// bad block costs only itself.
pub(crate) fn keep_or_drop_swim_block(
    parsed: Result<SwimParams, DescriptorError>,
) -> Option<SwimParams> {
    match parsed {
        Ok(swim) => Some(swim),
        Err(error) => {
            let detail = match error {
                DescriptorError::InvalidShape { reason } => reason,
                other => other.to_string(),
            };
            log::warn!(
                "[Scripting] {detail}; the movement.swim block was dropped, so the player wades and cannot swim"
            );
            None
        }
    }
}
