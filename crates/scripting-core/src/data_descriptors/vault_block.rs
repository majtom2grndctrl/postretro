// `movement.vault` assembly and its warn-and-drop policy, shared by the QuickJS
// and Luau movement parsers so both runtimes keep or drop a block identically,
// with the same diagnostics.
// See: context/lib/movement.md §2 (author surface) · context/lib/scripting.md §1 (parser twins)

use postretro_foundation::data_descriptors::{VaultEntry, VaultParams, VaultTrigger};

use super::DescriptorError;

/// One value of a `movement.vault` field as a VM read it, reduced to the
/// shapes the block distinguishes. An object holds one level of entries; a
/// nested object, an array, a function and a Luau table with any non-string
/// key all read as [`RawVaultValue::Other`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RawVaultValue {
    /// Absent, `null`/`undefined` or `nil`.
    Absent,
    Number(f32),
    Bool(bool),
    Str(String),
    Object(Vec<(String, RawVaultValue)>),
    Other,
}

fn invalid(reason: String) -> DescriptorError {
    DescriptorError::InvalidShape { reason }
}

/// A `movement.vault` value that is neither absent nor an object (a table, in
/// Luau).
pub(crate) fn vault_block_not_an_object() -> DescriptorError {
    invalid("`movement.vault` must be an object".to_string())
}

fn required_number(field: &'static str, value: RawVaultValue) -> Result<f32, DescriptorError> {
    match value {
        RawVaultValue::Number(number) => Ok(number),
        RawVaultValue::Absent => Err(invalid(format!("`movement.vault.{field}` is required"))),
        _ => Err(invalid(format!(
            "`movement.vault.{field}` must be a number"
        ))),
    }
}

fn trigger(place: &str, value: RawVaultValue) -> Result<Option<VaultTrigger>, DescriptorError> {
    match value {
        RawVaultValue::Absent => Ok(None),
        RawVaultValue::Str(name) if name == "auto" => Ok(Some(VaultTrigger::Auto)),
        RawVaultValue::Str(name) if name == "jump" => Ok(Some(VaultTrigger::Jump)),
        _ => Err(invalid(format!(
            "`movement.vault.from.{place}` must be \"auto\" or \"jump\""
        ))),
    }
}

/// A present `from` replaces the default whole: a place it leaves out cannot
/// vault. Only the closed places `air`, `ground` and `swim` exist.
fn entry(value: RawVaultValue) -> Result<VaultEntry, DescriptorError> {
    let mut entries = match value {
        RawVaultValue::Absent => return Ok(VaultEntry::default()),
        RawVaultValue::Object(entries) => entries,
        _ => {
            return Err(invalid(
                "`movement.vault.from` must be an object".to_string(),
            ));
        }
    };
    // Luau table order is unspecified: sort so both runtimes report the same
    // first bad entry.
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut from = VaultEntry {
        air: None,
        ground: None,
        swim: None,
    };
    for (place, value) in entries {
        let slot = match place.as_str() {
            "air" => &mut from.air,
            "ground" => &mut from.ground,
            "swim" => &mut from.swim,
            _ => {
                return Err(invalid(format!(
                    "`movement.vault.from.{place}` is not a vault context; expected air, ground or swim"
                )));
            }
        };
        *slot = trigger(&place, value)?;
    }
    Ok(from)
}

/// Read the block's fields in authored order through `read` (one VM's field
/// reader), then range-check them with [`VaultParams::validated`] against the
/// descriptor's `ground.step_height`.
pub(crate) fn vault_params_from_fields(
    mut read: impl FnMut(&'static str) -> Result<RawVaultValue, DescriptorError>,
    step_height: f32,
) -> Result<VaultParams, DescriptorError> {
    let max_height = required_number("maxHeight", read("maxHeight")?)?;
    let swim_max_height = required_number("swimMaxHeight", read("swimMaxHeight")?)?;
    let duration_ms = required_number("durationMs", read("durationMs")?)?;
    let from = entry(read("from")?)?;
    let allow_fire = match read("allowFire")? {
        RawVaultValue::Absent => false,
        RawVaultValue::Bool(allow) => allow,
        _ => {
            return Err(invalid(
                "`movement.vault.allowFire` must be a boolean".to_string(),
            ));
        }
    };
    VaultParams {
        max_height,
        swim_max_height,
        duration_ms,
        from,
        allow_fire,
    }
    .validated(step_height)
}

/// Keep a parsed vault block, or drop a malformed one with a warning naming
/// the field. A dropped block leaves the descriptor without vault: the player
/// cannot vault, and the rest of the descriptor and mod load.
pub(crate) fn keep_or_drop_vault_block(
    parsed: Result<VaultParams, DescriptorError>,
) -> Option<VaultParams> {
    match parsed {
        Ok(vault) => Some(vault),
        Err(error) => {
            let detail = match error {
                DescriptorError::InvalidShape { reason } => reason,
                other => other.to_string(),
            };
            log::warn!(
                "[Scripting] {detail}; the movement.vault block was dropped, so the player cannot vault"
            );
            None
        }
    }
}
