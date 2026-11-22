// Area-light emitters from the partition's surviving faces: exposure, side merge, sliver floor.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

use std::collections::{BTreeMap, HashSet};

use super::{AreaEmitter, EmitterPolygon, derived_reach};
use crate::map_data::{BrushVolume, Face, MapData};
use crate::partition::BspTree;

/// Emitting pieces smaller than this (1 mm², as for fluid surface parts) are
/// slivers: dropped before the light's polygons are fixed (research.md O8).
pub(crate) const EMITTER_PIECE_AREA_FLOOR_M2: f64 = 1.0e-6;

/// A side whose surviving pieces sum to its unsplit area within this
/// relative tolerance was only split, never trimmed, so it emits as its one
/// unsplit winding (research.md O12).
const UNSPLIT_SIDE_RELATIVE_TOLERANCE: f64 = 1.0e-6;

/// Resolves every area light's emitter from `faces`, the BSP's surviving
/// world faces, as `tree`'s interior leaves draw them. They already exclude
/// what lies inside another solid and hold a coplanar overlap once
/// (research.md O7); a face only solid or `exterior_leaves` hold, such as a
/// hull side facing the void, is dropped as geometry drops it, so emission
/// follows what renders. An emitting face is one whose brush belongs to the
/// light and whose texture has an `_e` map. Sets each light's `area`, its
/// `origin` to the area-weighted centroid and its `falloff_range` to the
/// authored override or the derived reach (O22), and appends the centroid to
/// its diagnostic label. Fails, naming the light, when one is left with no
/// emitting polygon, saying apart a light whose `_e` faces all lie in
/// exterior leaves from one with no `_e` face. On a leaking map the flood
/// reaches every empty leaf, so geometry draws nothing and every area light
/// fails the first way.
pub(crate) fn resolve_area_lights(
    map_data: &mut MapData,
    faces: &[Face],
    tree: &BspTree,
    exterior_leaves: &HashSet<usize>,
    has_emissive_map: impl Fn(&str) -> bool,
) -> anyhow::Result<()> {
    let drawn = drawn_faces(faces.len(), tree, exterior_leaves);
    for area in &map_data.area_lights {
        let polygons = emitting_polygons(
            &map_data.brush_volumes,
            faces
                .iter()
                .zip(&drawn)
                .filter(|&(_, &is_drawn)| is_drawn)
                .map(|(face, _)| face),
            &area.brush_indices,
            &has_emissive_map,
        );
        let label = &mut map_data.light_source_labels[area.light_index];
        let Some(emitter) = AreaEmitter::new(polygons) else {
            if e_faces_all_culled(faces, &drawn, &area.brush_indices, &has_emissive_map) {
                anyhow::bail!(
                    "{label} has `_e` faces, but all are culled as exterior: \
                     they face the void, or the map leaks"
                );
            }
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

/// Per face index, whether an interior leaf of `tree` draws the face: one
/// that is neither solid nor in `exterior_leaves`. Geometry extraction emits
/// exactly these.
fn drawn_faces(face_count: usize, tree: &BspTree, exterior_leaves: &HashSet<usize>) -> Vec<bool> {
    let mut drawn = vec![false; face_count];
    for (index, leaf) in tree.leaves.iter().enumerate() {
        if leaf.is_solid || exterior_leaves.contains(&index) {
            continue;
        }
        for &face in &leaf.face_indices {
            if let Some(drawn) = drawn.get_mut(face) {
                *drawn = true;
            }
        }
    }
    drawn
}

/// Whether the light owns `_e` faces and `drawn` holds none of them. A light
/// with a drawn `_e` face that still has no emitter lost it to the sliver
/// floor, and fails as having no `_e` face (research.md O8).
fn e_faces_all_culled(
    faces: &[Face],
    drawn: &[bool],
    brush_indices: &[usize],
    has_emissive_map: &impl Fn(&str) -> bool,
) -> bool {
    let mut own_e_faces = faces
        .iter()
        .zip(drawn)
        .filter(|(face, _)| {
            brush_indices.contains(&face.brush_index) && has_emissive_map(&face.texture)
        })
        .peekable();
    own_e_faces.peek().is_some() && own_e_faces.all(|(_, &is_drawn)| !is_drawn)
}

/// The light's emitting polygons: per source side, its unsplit winding when
/// every piece survived, else its surviving pieces; slivers dropped.
fn emitting_polygons<'a>(
    brushes: &[BrushVolume],
    faces: impl IntoIterator<Item = &'a Face>,
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
