// Player spawn points: which `player_spawn` the first player takes, and where a
// pawn stands relative to a spawn point's origin. A spawn point is a place a
// player can be put, never a pawn by itself.
// See: context/lib/entity_model.md §7b (Player spawn points)

use glam::Vec3;
use postretro_entities::provenance::parse_bool;
use postretro_foundation::NavAgentParams;
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

use super::MapEntity;
use super::data_archetype::find_descriptor;
use super::host_replicated::ai_capsule_center_from_feet_offset;

/// `player_spawn` key: `1` keeps the spawn for co-op joiners only, so the first
/// player (single player, or a co-op host) never starts there. Default `0`.
pub const COOP_ONLY_KEY: &str = "coop_only";

/// Whether this spawn point is reserved for co-op joiners. A value other than
/// `0`/`1` (or `false`/`true`) warns and counts as `0`.
pub fn is_coop_only(spawn: &MapEntity) -> bool {
    let Some(raw) = spawn.key_values.get(COOP_ONLY_KEY) else {
        return false;
    };
    parse_bool(raw).unwrap_or_else(|| {
        log::warn!(
            "[Loader] {origin}: `{COOP_ONLY_KEY}` value `{raw}` is not 0 or 1; treating as 0",
            origin = spawn.diagnostic_origin(),
        );
        false
    })
}

/// The order the first player tries spawn points in: every spawn not marked
/// [`COOP_ONLY_KEY`], in map order, then the co-op-only ones as a fallback.
/// Joiners take whichever spawns are free afterwards, in map order, through
/// the host's seat table.
pub fn first_player_spawn_order(spawn_points: &[MapEntity]) -> Vec<usize> {
    let (open, coop): (Vec<usize>, Vec<usize>) =
        (0..spawn_points.len()).partition(|&index| !is_coop_only(&spawn_points[index]));
    open.into_iter().chain(coop).collect()
}

/// Offset from a placement's canonical origin to the spawned entity's
/// transform. A spawn point's origin is the feet (the compiler translates the
/// authoring box), so a movement pawn rises by its standing capsule's half
/// extent to put the capsule's centre at its transform. An AI body rises to its
/// agent capsule's centre. Anything else spawns at the origin.
pub fn spawn_origin_shift(
    descriptor: &EntityTypeDescriptor,
    agent_params: Option<NavAgentParams>,
) -> Vec3 {
    if let Some(movement) = descriptor.movement.as_ref() {
        return Vec3::Y * (movement.capsule.half_height + movement.capsule.radius);
    }
    ai_capsule_center_from_feet_offset(descriptor, agent_params)
}

/// Where a pawn spawned at `spawn` stands: its origin plus the
/// [`spawn_origin_shift`] of the descriptor its `entity_class` (default
/// `"player"`) names. An unregistered class stands at the origin.
pub fn standing_position(
    spawn: &MapEntity,
    descriptors: &[EntityTypeDescriptor],
    agent_params: Option<NavAgentParams>,
) -> Vec3 {
    let entity_class = spawn
        .key_values
        .get("entity_class")
        .map(String::as_str)
        .unwrap_or("player");
    spawn.origin
        + find_descriptor(descriptors, entity_class).map_or(Vec3::ZERO, |descriptor| {
            spawn_origin_shift(descriptor, agent_params)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn(keys: &[(&str, &str)]) -> MapEntity {
        let mut entity = MapEntity {
            map_ordinal: None,
            classname: "player_spawn".to_string(),
            origin: Vec3::ZERO,
            angles: Vec3::ZERO,
            key_values: Default::default(),
            tags: Vec::new(),
        };
        for (key, value) in keys {
            entity
                .key_values
                .insert((*key).to_string(), (*value).to_string());
        }
        entity
    }

    #[test]
    fn first_player_skips_coop_only_spawns() {
        let points = [
            spawn(&[(COOP_ONLY_KEY, "1")]),
            spawn(&[]),
            spawn(&[(COOP_ONLY_KEY, "0")]),
        ];
        assert_eq!(first_player_spawn_order(&points), vec![1, 2, 0]);
    }

    #[test]
    fn every_spawn_coop_only_falls_back_to_map_order() {
        let points = [
            spawn(&[(COOP_ONLY_KEY, "1")]),
            spawn(&[(COOP_ONLY_KEY, "1")]),
        ];
        assert_eq!(first_player_spawn_order(&points), vec![0, 1]);
    }

    #[test]
    fn malformed_coop_only_counts_as_open() {
        assert!(!is_coop_only(&spawn(&[(COOP_ONLY_KEY, "yes")])));
        assert!(is_coop_only(&spawn(&[(COOP_ONLY_KEY, " true ")])));
    }
}
