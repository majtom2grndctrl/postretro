// VM-agnostic parsing of `movement.sounds`, shared by the QuickJS and Luau
// movement parsers so both runtimes accept and reject identically.
// See: context/lib/audio.md §4 · context/lib/scripting.md §1

use postretro_foundation::data_descriptors::validate_sound_key;

use super::{DescriptorError, MovementSounds};

/// Parse an authored `movement.sounds` object. Unknown keys are rejected so a
/// misspelled event is loud; every present key must be a valid sound key.
pub(crate) fn movement_sounds_from_json(
    json: serde_json::Value,
) -> Result<MovementSounds, DescriptorError> {
    let sounds: MovementSounds =
        serde_json::from_value(json).map_err(|e| DescriptorError::InvalidShape {
            reason: format!("`movement.sounds` invalid: {e}"),
        })?;
    for (field, key) in sounds.keys() {
        validate_sound_key(&format!("movement.sounds.{field}"), key)?;
    }
    Ok(sounds)
}
