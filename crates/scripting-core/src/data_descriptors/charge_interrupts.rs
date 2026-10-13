// `charge.interruptedBy` shape checks, shared by the QuickJS and Luau weapon
// parsers so both runtimes reject the same values with the same diagnostics.
// See: context/lib/scripting.md §1 (parser twins) · context/lib/entity_model.md §Components

use postretro_foundation::CHARGE_INTERRUPT_SOURCES;

use super::DescriptorError;

/// One `charge.interruptedBy` entry as a VM read it. `key` is `None` for a
/// Luau key that is not a string.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RawChargeInterrupt {
    pub key: Option<String>,
    pub value: RawInterruptValue,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RawInterruptValue {
    /// `undefined` in JS; a Luau table never holds `nil`.
    Absent,
    Bool,
    /// Anything else, `null` included.
    Other,
}

/// Validate the entries of the `interruptedBy` map under `action_path` (such
/// as `components.weapon.primary`). The vocabulary is closed: an unknown
/// source names itself and the accepted ones. Entries are checked in key
/// order, so either VM reports the same first error.
pub(crate) fn validate_charge_interrupts(
    action_path: &str,
    mut entries: Vec<RawChargeInterrupt>,
) -> Result<(), DescriptorError> {
    let path = format!("{action_path}.charge.interruptedBy");
    let expected = CHARGE_INTERRUPT_SOURCES
        .iter()
        .map(|source| format!("`{source}`"))
        .collect::<Vec<_>>()
        .join(", ");
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    for entry in entries {
        let Some(key) = entry.key else {
            return Err(invalid(format!(
                "`{path}` keys must be interrupt source names; expected {expected}"
            )));
        };
        if !CHARGE_INTERRUPT_SOURCES.contains(&key.as_str()) {
            return Err(invalid(format!(
                "`{path}.{key}` is not a charge interrupt source; expected {expected}"
            )));
        }
        if entry.value == RawInterruptValue::Other {
            return Err(invalid(format!("`{path}.{key}` must be a boolean")));
        }
    }
    Ok(())
}

fn invalid(reason: String) -> DescriptorError {
    DescriptorError::InvalidShape { reason }
}
