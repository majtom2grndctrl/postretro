// Quake-family point-entity origins, translated to the canonical point each classname means.
// See: context/lib/build_pipeline.md §Source-format neutrality · context/lib/entity_model.md §7b (Player spawn points)

use glam::DVec3;

/// How far below its authored origin a `player_spawn`'s feet stand, in Quake
/// units. The FGD gives `player_spawn` Quake's player-hull box,
/// `size(-16 -16 -24, 16 16 32)`, so an author who rests the box on a floor
/// leaves the origin 24 units up. The canonical spawn point is the feet: the
/// box bottom. The engine raises it to the pawn's capsule centre.
pub const PLAYER_SPAWN_FEET_BELOW_ORIGIN_UNITS: f64 = 24.0;

/// Translate an authored point-entity origin (Quake units, Z-up, before the
/// axis swizzle and unit scale) to the canonical point its classname means.
/// Only `player_spawn` differs: its origin becomes its feet.
pub fn canonical_entity_origin(classname: &str, origin: DVec3) -> DVec3 {
    if classname == "player_spawn" {
        origin - DVec3::Z * PLAYER_SPAWN_FEET_BELOW_ORIGIN_UNITS
    } else {
        origin
    }
}
