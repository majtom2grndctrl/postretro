// A received life-state profile's `assistable` literals, checked against the
// authoring rules. See: context/lib/networking.md (Tuning payload)

use postretro_foundation::Assistable;

/// The refusal message for the first `assistable` literal outside its range,
/// built from the rule the authoring side reports. Expression fields are
/// carried as data; the client never binds or evaluates them.
pub(super) fn literal_fault_message(assistable: &Assistable) -> Option<String> {
    assistable
        .literal_fault()
        .map(|(key, rule)| format!("assistable {key} {rule}"))
}
