// Logs a body's tick overlap report: once per level load, naming what a mapper can act on.
// See: context/lib/movement.md §1

use postretro_entities::{EntityId, EntityRegistry};

use crate::collision::{CollisionWorld, StaticOverlap};

/// Warn about a body whose own movement ended a tick inside static geometry,
/// if this level load has not warned yet. Movement reports the breach without
/// entity identity; this resolves the body's descriptor here, and the texture
/// and TrenchBroom position from the report. Every body shares the one warning.
pub fn report_static_overlap(
    world: &CollisionWorld,
    registry: &EntityRegistry,
    body: EntityId,
    overlap: &StaticOverlap,
) {
    if !world.claim_overlap_warning() {
        return;
    }
    let descriptor = crate::emission::descriptor_name(registry, body)
        .unwrap_or_else(|| String::from("<no descriptor>"));
    let at = overlap.map_position();
    let texture = overlap
        .texture
        .and_then(|texture| world.texture_name(texture));
    let surface = texture.map_or_else(String::new, |name| format!(" against texture `{name}`"));
    log::warn!(
        "[Movement] `{descriptor}` ended a tick inside static geometry{surface} at TrenchBroom \
         ({:.0} {:.0} {:.0}); further overlaps this level stay silent",
        at.x,
        at.y,
        at.z,
    );
}
