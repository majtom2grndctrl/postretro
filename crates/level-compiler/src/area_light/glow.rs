// Area glow stamping: each `light_area`'s emitting render faces and the glow they draw (PRL id 62).
// See: context/lib/build_pipeline.md §PRL section IDs · resource_management.md §4.5

use std::collections::HashMap;

use postretro_level_format::area_glow::{AreaGlowLight, AreaGlowSection, MAX_AREA_GLOW_FACES};
use postretro_level_format::sh_volume::ANIMATED_SLOT_NONE;

use crate::geometry::GeometryResult;
use crate::map_data::{MapAreaLight, MapLight};

/// Per final Geometry face: whether its source brush belongs to an area
/// light. Emissive volume membership skips these faces, so an area light's
/// faces have one glow owner, their light. Empty when the map has none.
pub(crate) fn area_light_faces(
    area_lights: &[MapAreaLight],
    face_brushes: &[Option<u32>],
) -> Vec<bool> {
    if area_lights.is_empty() {
        return Vec::new();
    }
    let owner = brush_owners(area_lights);
    face_brushes
        .iter()
        .map(|brush| brush.is_some_and(|brush| owner.contains_key(&(brush as usize))))
        .collect()
}

/// Brush index → its area light's position in `area_lights`.
fn brush_owners(area_lights: &[MapAreaLight]) -> HashMap<usize, usize> {
    let mut owner = HashMap::new();
    for (index, area) in area_lights.iter().enumerate() {
        for &brush in &area.brush_indices {
            owner.insert(brush, index);
        }
    }
    owner
}

/// Id 62 from the final static world: after the atlas cut and once the light
/// namespaces are fixed, so each light names its own compose descriptor slot
/// (research.md O4). `None` when the map has no area light (O5).
///
/// An emitting face is a final face whose brush belongs to the light and
/// whose texture has an `_e` map; every render face derived from an emitting
/// side is one, however the BSP or the atlas split it, and a frame face is
/// not (O12). Once any face glows, a world face with no triangle fails the
/// build: the runtime reaches a face's vertices only through its BVH leaf,
/// so it cannot stamp glow ids on such a level.
///
/// A light with a compose slot keeps it at any intensity, `light 0`
/// included, so its glow follows its state and curves; a static light's
/// faces draw `_glow × _color` (O16). Past [`MAX_AREA_GLOW_FACES`] emitting
/// faces the build fails, naming the count and the cap (O5). Both checks
/// live here alone: this is id 62's one producer, and it runs before
/// reflection capture, so a failing map fails early.
pub(crate) fn build_area_glow_section(
    area_lights: &[MapAreaLight],
    lights: &[MapLight],
    slot_for_map_light: &[u32],
    geometry: &GeometryResult,
    face_brushes: &[Option<u32>],
    has_emissive_map: &[bool],
) -> anyhow::Result<Option<AreaGlowSection>> {
    if area_lights.is_empty() {
        return Ok(None);
    }
    let owner = brush_owners(area_lights);
    let mut faces_of: Vec<Vec<u32>> = vec![Vec::new(); area_lights.len()];
    for (face, meta) in geometry.geometry.faces.iter().enumerate() {
        let Some(&light) = face_brushes
            .get(face)
            .copied()
            .flatten()
            .and_then(|brush| owner.get(&(brush as usize)))
        else {
            continue;
        };
        let has_triangle = geometry
            .face_index_ranges
            .get(face)
            .is_some_and(|range| range.index_count > 0);
        let glows = has_emissive_map
            .get(meta.texture_index as usize)
            .copied()
            .unwrap_or(false);
        if has_triangle && glows {
            faces_of[light].push(face as u32);
        }
    }

    let total: usize = faces_of.iter().map(Vec::len).sum();
    if total > MAX_AREA_GLOW_FACES {
        anyhow::bail!(
            "area lights have {total} emitting faces, past the glow table's cap of \
             {MAX_AREA_GLOW_FACES}; split the map or use fewer `_e` faces on `light_area` entities"
        );
    }
    if total > 0
        && let Some(face) = geometry
            .face_index_ranges
            .iter()
            .position(|range| range.index_count == 0)
    {
        anyhow::bail!(
            "area lights glow on {total} face(s), but world face {face} has no triangle, and a \
             level whose faces carry glow ids needs a triangle on every world face. Remove or \
             rebuild that degenerate brush"
        );
    }

    let section = AreaGlowSection {
        lights: area_lights
            .iter()
            .zip(faces_of)
            .map(|(area, faces)| {
                let light = &lights[area.light_index];
                AreaGlowLight {
                    glow: area.glow,
                    color: light.color,
                    intensity: light.intensity,
                    compose_slot: slot_for_map_light
                        .get(area.light_index)
                        .copied()
                        .filter(|&slot| slot != ANIMATED_SLOT_NONE),
                    faces,
                }
            })
            .collect(),
    };
    log::info!(
        "[Compiler] Area glow: {} light(s), {total} emitting face(s)",
        section.lights.len()
    );
    Ok(Some(section))
}

#[cfg(test)]
#[path = "glow_tests.rs"]
mod tests;
