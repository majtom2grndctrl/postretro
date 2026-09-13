//! Descriptor-derived player-health schema setup shared by host and client.

use crate::scripting::map_entity::MapEntity;
use postretro_entities::{EntityTypeDescriptor, NumericRange, SlotTable};

/// Attach the descriptor-authored `player.health` validation range before either
/// network role builds its replicated-state schema.
///
/// The selected descriptor mirrors `spawn_from_player_starts`: map placements
/// are visited in order, `entity_class` defaults to `player`, and the first
/// movement descriptor is authoritative. It reads shared authoring data rather
/// than a pawn, so a connected client can construct the same schema before the
/// host baseline materializes its local player.
pub(crate) fn install_descriptor_player_health_range(
    slot_table: &mut SlotTable,
    spawn_points: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
) {
    for spawn in spawn_points {
        let entity_class = spawn
            .key_values
            .get("entity_class")
            .map(String::as_str)
            .unwrap_or("player");
        let Some(descriptor) = descriptors
            .iter()
            .find(|descriptor| descriptor.canonical_name.as_deref() == Some(entity_class))
        else {
            continue;
        };
        if descriptor.movement.is_none() {
            continue;
        }
        let Some(health) = descriptor.health.as_ref() else {
            return;
        };
        if let Err(err) = slot_table.set_engine_numeric_range(
            "player.health",
            NumericRange {
                min: 0.0,
                max: health.max,
            },
        ) {
            log::warn!("[Loader] failed to set player.health range: {err}");
        }
        return;
    }
}
