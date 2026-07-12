// Scripting reaction dispatch adapters and compatibility re-exports.
// See: context/lib/scripting.md §10
#![allow(unused_imports)]

use postretro_entities::{DataRegistry, ScriptCtx, SlotTable};
use postretro_scripting_core::reaction_dispatch::fire_named_event_with_sequences;
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_scripting_core::state_crossings::CrossingDetector;

pub(crate) use crate::fx::emitter_reactions::{set_emitter_rate, set_spin_rate};
pub(crate) use crate::fx::fog_reactions::{
    set_fog_animation, set_fog_density, set_fog_edge_softness, set_fog_falloff, set_fog_glow,
    set_fog_params,
};
pub(crate) use crate::health::reactions as apply_damage;
pub(crate) use animation as set_animation_state;

pub(crate) mod animation;
pub(crate) mod registry;
pub(crate) mod system_commands;

#[cfg(test)]
pub(crate) mod log_capture;

pub(crate) use postretro_scripting_core::reaction_registry::ReactionError;

/// Detect settled state crossings and synchronously dispatch their named reactions.
/// App owns the surrounding frame and system-command drains; this owns their shared dispatch seam.
pub(crate) fn dispatch_state_crossings_with_sequences(
    crossing_detector: &mut CrossingDetector,
    slot_table: &SlotTable,
    data_registry: &DataRegistry,
    sequence_registry: &SequencedPrimitiveRegistry,
    reaction_registry: &ReactionPrimitiveRegistry,
    system_registry: &SystemReactionRegistry,
    script_ctx: &ScriptCtx,
) -> Vec<String> {
    let crossing_events = crossing_detector.detect(slot_table);
    for event_name in &crossing_events {
        let _ = fire_named_event_with_sequences(
            event_name,
            data_registry,
            sequence_registry,
            reaction_registry,
            system_registry,
            script_ctx,
        );
    }
    crossing_events
}
