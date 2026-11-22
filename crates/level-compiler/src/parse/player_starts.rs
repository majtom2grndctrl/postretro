//! Player-start warnings: Quake player-start classnames and a map with no `player_spawn`.
//! See: context/lib/entity_model.md §7b (Player spawn points)

use crate::format::quake_map;
use crate::map_data::MapEntityRecord;

/// The classname a level start places players at.
pub(super) const PLAYER_SPAWN_CLASSNAME: &str = "player_spawn";

/// Warn when entity `index` uses one of Quake's player-start classnames,
/// which the engine does not read.
pub(super) fn warn_quake_player_start(index: usize, classname: &str) {
    if !quake_map::is_quake_player_start_classname(classname) {
        return;
    }
    // A Quake co-op start maps to a spawn the first player never takes.
    let coop_hint = if classname == "info_player_coop" {
        " with `coop_only` 1"
    } else {
        ""
    };
    log::warn!(
        "[Compiler] entity {index} '{classname}': Quake's player start is not read; \
         the engine starts players only at a `{PLAYER_SPAWN_CLASSNAME}` entity, so \
         change this classname to `{PLAYER_SPAWN_CLASSNAME}`{coop_hint}"
    );
}

/// Whether the map holds a `player_spawn` the engine can start a player at.
pub(super) fn has_player_spawn(map_entities: &[MapEntityRecord]) -> bool {
    map_entities
        .iter()
        .any(|entity| entity.classname == PLAYER_SPAWN_CLASSNAME)
}

/// Warn once when the map holds no `player_spawn` at all.
pub(super) fn warn_without_player_spawn(map_entities: &[MapEntityRecord]) {
    if has_player_spawn(map_entities) {
        return;
    }
    log::warn!(
        "[Compiler] map has no `{PLAYER_SPAWN_CLASSNAME}`; the player will start in the \
         free-fly camera and can fly through walls"
    );
}

#[cfg(test)]
#[path = "player_starts/tests.rs"]
mod tests;
