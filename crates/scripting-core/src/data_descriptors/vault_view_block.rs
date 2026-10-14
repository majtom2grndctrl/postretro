// `movement.viewFeel.vault` assembly and validation, shared by the QuickJS and
// Luau view-feel parsers so both runtimes accept and reject alike, with the
// same diagnostics.
// See: context/lib/movement.md §1 (view feel) · context/lib/scripting.md §1 (parser twins)

use postretro_foundation::data_descriptors::VaultViewParams;

use super::DescriptorError;

/// One `movement.viewFeel.vault` field as a VM read it. Numbers stay `f64` so
/// a range check sees the authored value before f32 narrowing can round it
/// onto a bound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RawViewNumber {
    /// Absent, `null`/`undefined` or `nil`.
    Absent,
    Number(f64),
    /// Present but not a number.
    Other,
}

fn invalid(reason: String) -> DescriptorError {
    DescriptorError::InvalidShape { reason }
}

/// A `movement.viewFeel.vault` value that is present but not an object (a
/// table, in Luau).
pub(crate) fn vault_view_not_an_object() -> DescriptorError {
    invalid("`movement.viewFeel.vault` must be an object".to_string())
}

fn number(
    field: &'static str,
    raw: RawViewNumber,
    default: f32,
    min: f32,
    max: f32,
) -> Result<f64, DescriptorError> {
    let value = match raw {
        RawViewNumber::Absent => return Ok(f64::from(default)),
        RawViewNumber::Number(value) => value,
        RawViewNumber::Other => {
            return Err(invalid(format!(
                "`movement.viewFeel.vault.{field}` must be a number"
            )));
        }
    };
    if !value.is_finite() || !(f64::from(min)..=f64::from(max)).contains(&value) {
        return Err(invalid(format!(
            "`movement.viewFeel.vault.{field}` must be finite in [{min}, {max}], got {value}"
        )));
    }
    Ok(value)
}

/// Read the block's fields through `read` (one VM's field reader) and
/// validate them. Every field is optional: a missing channel holds nothing,
/// and the shares default to [`VaultViewParams::DEFAULT_EASE_IN`] and
/// [`VaultViewParams::DEFAULT_EASE_OUT`]. A bad block rejects the descriptor,
/// as every other `viewFeel` block does.
pub(crate) fn vault_view_params_from_fields(
    mut read: impl FnMut(&'static str) -> Result<RawViewNumber, DescriptorError>,
) -> Result<VaultViewParams, DescriptorError> {
    let angle = VaultViewParams::MAX_ANGLE;
    let fov = VaultViewParams::MAX_FOV;
    let pitch = number("pitch", read("pitch")?, 0.0, -angle, angle)?;
    let roll = number("roll", read("roll")?, 0.0, -angle, angle)?;
    let fov = number("fov", read("fov")?, 0.0, -fov, fov)?;
    let ease_in = number(
        "easeIn",
        read("easeIn")?,
        VaultViewParams::DEFAULT_EASE_IN,
        0.0,
        1.0,
    )?;
    let ease_out = number(
        "easeOut",
        read("easeOut")?,
        VaultViewParams::DEFAULT_EASE_OUT,
        0.0,
        1.0,
    )?;
    // A default share is a widened f32 (0.2 reads 0.2000000003), so allow that
    // much over 1: an authored 0.8 with the default 0.2 still fits.
    if ease_in + ease_out > 1.0 + 1.0e-6 {
        return Err(invalid(format!(
            "`movement.viewFeel.vault.easeIn` plus `movement.viewFeel.vault.easeOut` must be at most 1, got {}",
            ease_in + ease_out
        )));
    }
    Ok(VaultViewParams {
        pitch: pitch as f32,
        roll: roll as f32,
        fov: fov as f32,
        ease_in: ease_in as f32,
        ease_out: ease_out as f32,
    })
}
