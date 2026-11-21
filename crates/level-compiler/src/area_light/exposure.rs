// Area-light emitters from the partition's surviving faces: exposure, side merge, sliver floor.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

use std::collections::{BTreeMap, HashSet};

use super::{AreaEmitter, EmitterPolygon, derived_reach};
use crate::map_data::{BrushVolume, Face, MapData};

/// Emitting pieces smaller than this (1 mm², as for fluid surface parts) are
/// slivers: dropped before the light's polygons are fixed (research.md O8).
pub(crate) const EMITTER_PIECE_AREA_FLOOR_M2: f64 = 1.0e-6;

/// A side whose surviving pieces sum to its unsplit area within this
/// relative tolerance was only split, never trimmed, so it emits as its one
/// unsplit winding (research.md O12).
const UNSPLIT_SIDE_RELATIVE_TOLERANCE: f64 = 1.0e-6;

/// Resolves every area light's emitter from `faces`, the BSP's surviving
/// world faces. They already exclude what lies inside another solid and hold
/// a coplanar overlap once (research.md O7). An emitting face is one whose
/// brush belongs to the light and whose texture has an `_e` map. Sets each
/// light's `area`, its `origin` to the area-weighted centroid and its
/// `falloff_range` to the authored override or the derived reach (O22), and
/// appends the centroid to its diagnostic label. Fails, naming the light,
/// when one is left with no emitting polygon.
pub(crate) fn resolve_area_lights(
    map_data: &mut MapData,
    faces: &[Face],
    has_emissive_map: impl Fn(&str) -> bool,
) -> anyhow::Result<()> {
    for area in &map_data.area_lights {
        let polygons = emitting_polygons(
            &map_data.brush_volumes,
            faces,
            &area.brush_indices,
            &has_emissive_map,
        );
        let label = &mut map_data.light_source_labels[area.light_index];
        let Some(emitter) = AreaEmitter::new(polygons) else {
            anyhow::bail!("{label} has no face with an `_e` map");
        };
        let light = &mut map_data.lights[area.light_index];
        let reach = area.reach_override.unwrap_or_else(|| {
            derived_reach(f64::from(light.intensity), emitter.total_area) as f32
        });
        let centroid = emitter.centroid;
        light.origin = centroid;
        light.falloff_range = reach;
        light.area = Some(emitter);
        label.push_str(&format!(
            " (emitter centroid {:.3} {:.3} {:.3} m)",
            centroid.x, centroid.y, centroid.z
        ));
    }
    Ok(())
}

/// The light's emitting polygons: per source side, its unsplit winding when
/// every piece survived, else its surviving pieces; slivers dropped.
fn emitting_polygons(
    brushes: &[BrushVolume],
    faces: &[Face],
    brush_indices: &[usize],
    has_emissive_map: &impl Fn(&str) -> bool,
) -> Vec<EmitterPolygon> {
    let own: HashSet<usize> = brush_indices.iter().copied().collect();
    // Faces carry their brush but no side index. A brush is convex, so its
    // plane and texture name one side; faces copy both from it exactly.
    // Keyed in brush and side order, so the polygon order is stable.
    let mut sides: BTreeMap<(usize, usize), Vec<&Face>> = BTreeMap::new();
    let mut unmatched: Vec<&Face> = Vec::new();
    for face in faces {
        if !own.contains(&face.brush_index) || !has_emissive_map(&face.texture) {
            continue;
        }
        match source_side(brushes, face) {
            Some(side) => sides
                .entry((face.brush_index, side))
                .or_default()
                .push(face),
            None => unmatched.push(face),
        }
    }

    let mut polygons = Vec::new();
    for ((brush, side), pieces) in sides {
        let side = &brushes[brush].sides[side];
        let unsplit = EmitterPolygon::new(side.vertices.clone(), side.normal);
        let piece_polygons: Vec<EmitterPolygon> = pieces
            .iter()
            .map(|face| EmitterPolygon::new(face.vertices.clone(), face.normal))
            .collect();
        let pieces_area: f64 = piece_polygons.iter().map(|poly| poly.area).sum();
        let whole =
            (pieces_area - unsplit.area).abs() <= UNSPLIT_SIDE_RELATIVE_TOLERANCE * unsplit.area;
        if whole {
            polygons.push(unsplit);
        } else {
            polygons.extend(piece_polygons);
        }
    }
    polygons.extend(
        unmatched
            .into_iter()
            .map(|face| EmitterPolygon::new(face.vertices.clone(), face.normal)),
    );
    polygons.retain(|poly| poly.vertices.len() >= 3 && poly.area >= EMITTER_PIECE_AREA_FLOOR_M2);
    polygons
}

fn source_side(brushes: &[BrushVolume], face: &Face) -> Option<usize> {
    brushes
        .get(face.brush_index)?
        .sides
        .iter()
        .position(|side| {
            side.normal == face.normal
                && side.distance == face.distance
                && side.texture == face.texture
        })
}

#[cfg(test)]
#[path = "exposure_tests.rs"]
mod tests;
