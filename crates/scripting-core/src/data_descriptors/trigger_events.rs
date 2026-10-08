// Trigger-event wire forms and where each belongs: a level script keys trigger
// events by volume (`{ trigger, event, fire }`, a trigger member's `t.on`); the
// mod manifest keys them by tag (`{ tag, event, fire, levels? }`,
// `defineTriggerEvent`). Each VM parses an entry into [`TriggerEventEntry`];
// the routing here rejects the form that belongs elsewhere, so both runtimes
// log the same diagnostic.
// See: context/lib/scripting.md §12 (Per-member sources)

use super::*;
use postretro_entities::data_descriptors::VolumeTriggerEventDescriptor;

/// One parsed `triggerEvents` entry, before the manifest it arrived in decides
/// whether it belongs there.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TriggerEventEntry {
    /// `{ trigger, event, fire }` — a level member event.
    Volume(VolumeTriggerEventDescriptor),
    /// `{ tag, event, fire, levels? }` — a mod-global standing rule.
    Tag(TriggerEventDescriptor),
}

/// Validate the key shape shared by both runtimes: exactly one of `trigger`
/// (level) and `tag` (mod manifest).
pub(crate) fn trigger_event_key_shape(
    has_trigger: bool,
    has_tag: bool,
) -> Result<(), DescriptorError> {
    match (has_trigger, has_tag) {
        (true, true) => Err(DescriptorError::InvalidShape {
            reason: "trigger-event entry carries both `trigger` and `tag`; a level script keys by `trigger`, the mod manifest by `tag`".into(),
        }),
        (false, false) => Err(DescriptorError::InvalidShape {
            reason: "trigger-event entry needs `trigger` (level script) or `tag` (mod manifest)"
                .into(),
        }),
        _ => Ok(()),
    }
}

/// Keep the level form from a level script; warn and drop a tag-keyed entry,
/// naming the level script. Sibling volume-keyed entries are unaffected.
pub(crate) fn level_trigger_event(
    entry: TriggerEventEntry,
    index: impl std::fmt::Display,
    scope: &str,
) -> Option<VolumeTriggerEventDescriptor> {
    match entry {
        TriggerEventEntry::Volume(descriptor) => Some(descriptor),
        TriggerEventEntry::Tag(descriptor) => {
            log::warn!(
                "[Scripting] level script `{scope}`: triggerEvents[{index}] is keyed by tag `{}`; a level script binds trigger events per volume with a trigger member's `on`, and tag-keyed rules belong in the mod manifest (`defineTriggerEvent`). Skipped",
                descriptor.tag,
            );
            None
        }
    }
}

/// Keep the tag form from the mod manifest; warn and drop a volume-keyed entry,
/// naming the manifest. Sibling tag-keyed entries are unaffected.
pub(crate) fn mod_trigger_event(
    entry: TriggerEventEntry,
    index: impl std::fmt::Display,
    scope: &str,
) -> Option<TriggerEventDescriptor> {
    match entry {
        TriggerEventEntry::Tag(descriptor) => Some(descriptor),
        TriggerEventEntry::Volume(descriptor) => {
            log::warn!(
                "[Scripting] mod manifest ({scope}): triggerEvents[{index}] is keyed by trigger volume {}; no level exists when the mod manifest is read, so mod-global trigger events are keyed by tag (`defineTriggerEvent`), and volume-keyed events belong in a level script. Skipped",
                descriptor.trigger,
            );
            None
        }
    }
}
