// Player spawn points: first-player order, joinable spawns, standing position.
// See: context/lib/entity_model.md §7b (Player spawn points)

use glam::Vec3;
use postretro_entities::provenance::parse_bool;
use postretro_foundation::NavAgentParams;
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

use super::MapEntity;
use super::data_archetype::{find_descriptor, spawn_origin_shift};

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

/// The order the first player tries spawn points in, each paired with whether
/// it is [`COOP_ONLY_KEY`]: every open spawn in map order, then the co-op-only
/// ones as a fallback. Reads each spawn's key once, so a malformed value warns
/// once.
pub fn first_player_spawn_order(spawn_points: &[MapEntity]) -> Vec<(usize, bool)> {
    let mut order: Vec<(usize, bool)> = spawn_points
        .iter()
        .enumerate()
        .map(|(index, spawn)| (index, is_coop_only(spawn)))
        .collect();
    // Stable: map order holds within the open and the co-op-only spawns.
    order.sort_by_key(|&(_, coop_only)| coop_only);
    order
}

/// The descriptor a spawn's `entity_class` (default `"player"`) names, if it is
/// registered.
pub fn spawn_descriptor<'a>(
    spawn: &MapEntity,
    descriptors: &'a [EntityTypeDescriptor],
) -> Option<&'a EntityTypeDescriptor> {
    let entity_class = spawn
        .key_values
        .get("entity_class")
        .map(String::as_str)
        .unwrap_or("player");
    find_descriptor(descriptors, entity_class)
}

/// The spawns a co-op joiner may be given, in map order: every spawn whose
/// `entity_class` resolves. A spawn that cannot materialize a pawn is never
/// assigned, so a joiner is not closed for the map's authoring mistake.
pub fn joinable_spawns(
    spawn_points: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
) -> Vec<usize> {
    spawn_points
        .iter()
        .enumerate()
        .filter(|(_, spawn)| spawn_descriptor(spawn, descriptors).is_some())
        .map(|(index, _)| index)
        .collect()
}

/// Where a pawn spawned at `spawn` stands: its origin plus the
/// [`spawn_origin_shift`] of the descriptor its `entity_class` names. An
/// unregistered class stands at the origin.
pub fn standing_position(
    spawn: &MapEntity,
    descriptors: &[EntityTypeDescriptor],
    agent_params: Option<NavAgentParams>,
) -> Vec3 {
    spawn.origin
        + spawn_descriptor(spawn, descriptors).map_or(Vec3::ZERO, |descriptor| {
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
        assert_eq!(
            first_player_spawn_order(&points),
            vec![(1, false), (2, false), (0, true)]
        );
    }

    #[test]
    fn every_spawn_coop_only_falls_back_to_map_order() {
        let points = [
            spawn(&[(COOP_ONLY_KEY, "1")]),
            spawn(&[(COOP_ONLY_KEY, "1")]),
        ];
        assert_eq!(
            first_player_spawn_order(&points),
            vec![(0, true), (1, true)]
        );
    }

    #[test]
    fn malformed_coop_only_counts_as_open() {
        assert!(!is_coop_only(&spawn(&[(COOP_ONLY_KEY, "yes")])));
        assert!(is_coop_only(&spawn(&[(COOP_ONLY_KEY, " true ")])));
    }
}
