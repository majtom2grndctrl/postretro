// Emissive volume membership: which final static world faces each `emissive_volume` holds, as id 60's volume records.
// See: context/lib/build_pipeline.md §Compiler pipeline · resource_management.md §4.5

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use glam::DVec3;
use postretro_level_format::emissive::EmissiveVolumeRecord;
use postretro_level_format::octahedral;

use crate::geometry::GeometryResult;
use crate::map_data::{BrushVolume, MapEmissiveVolume};
use crate::parse::FLUSH_TOLERANCE_METERS;

/// Containment slack, engine meters: a face in the plane of a volume brush's
/// side joins (`n · p - d <= tolerance`). Also how far a final face's
/// vertices may sit off the authored side it is matched to; f32 vertex
/// positions stay well inside it at level coordinates.
const MEMBERSHIP_TOLERANCE_M: f64 = FLUSH_TOLERANCE_METERS;

/// A final face's normal and its authored side's agree within this cosine.
/// The face normal is the vertex's octahedral `u16` encoding, a few 1e-5 rad
/// off, so this only rejects a neighbouring side a sliver face also lies
/// within tolerance of, along their shared edge.
const SIDE_NORMAL_MIN_COS: f64 = 0.99;

/// Mantissa bits the overlap rule's size key keeps, as the environment
/// volumes' (`postretro-collision` `volumes.rs`). A summed brush volume rounds
/// differently where it sits, so identical copies differ in their last bits;
/// at 8 significant bits (a relative step of about 0.4%) they compare equal
/// and map order decides, while "a sign's box inside a room's" still orders by
/// size.
const SIZE_KEY_MANTISSA_BITS: u32 = 8;

/// A monotone, coarse key for a summed brush volume: the f32 rounded to
/// [`SIZE_KEY_MANTISSA_BITS`] significant bits, as its bit pattern.
fn size_key(summed: f64) -> u32 {
    let summed = summed as f32;
    if summed.is_nan() || summed <= 0.0 {
        return 0;
    }
    if summed.is_infinite() {
        return u32::MAX;
    }
    let dropped = f32::MANTISSA_DIGITS - 1 - SIZE_KEY_MANTISSA_BITS;
    let half = 1u32 << (dropped - 1);
    (summed.to_bits() + half) & !((1u32 << dropped) - 1)
}

/// Id 60's volume records, in map order, from the final static world: after
/// the atlas cut, on the face indices the Geometry section (id 17) stores.
/// Movers and fluids are not in that geometry, so no volume lists them.
///
/// A face's membership follows its authored brush side: `face_brushes`
/// (remapped through the cut) names its source brush in `brush_volumes`, the
/// side it lies on is found by plane, normal and texture, and that side's
/// whole-polygon centroid is tested. Every BSP fragment and atlas sub-face of
/// one authored face therefore shares one membership. A face with no brush
/// or no matching side tests its own centroid (see the report below).
///
/// Overlaps: the smallest containing volume by summed brush volume wins, and
/// an exact tie goes to the later in map order. Environment volumes and
/// `reflection_volume` tie to the earlier; this rule is emissive's own, so a
/// volume placed later over an equal one takes its faces.
///
/// `has_emissive_map[texture]` says whether a texture has an `_e` map. A volume
/// left with no member face on one warns once, including one another volume
/// empties; it keeps its record, index and origin.
pub(crate) fn resolve_emissive_volume_records(
    volumes: &[MapEmissiveVolume],
    geometry: &GeometryResult,
    face_brushes: &[Option<u32>],
    brush_volumes: &[BrushVolume],
    has_emissive_map: &[bool],
) -> Vec<EmissiveVolumeRecord> {
    if volumes.is_empty() {
        return Vec::new();
    }
    let size_keys: Vec<u32> = volumes
        .iter()
        .map(|volume| size_key(volume.summed_volume))
        .collect();
    let mut faces_of: Vec<Vec<u32>> = vec![Vec::new(); volumes.len()];
    let mut glowing_members = vec![0usize; volumes.len()];
    let mut glowing_inside = vec![0usize; volumes.len()];
    // One containment test per authored side: its fragments share the answer.
    let mut by_side: HashMap<(u32, usize), Vec<usize>> = HashMap::new();
    let mut unmatched = 0usize;
    // A face matched to no side tests its own centroid; its answer lives here.
    let mut unmatched_containing;

    for face in 0..geometry.geometry.faces.len() {
        let containing: &[usize] = match authored_side(face, geometry, face_brushes, brush_volumes)
        {
            Some((brush, side)) => match by_side.entry((brush, side)) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let point =
                        polygon_centroid(&brush_volumes[brush as usize].sides[side].vertices);
                    entry.insert(containing_volumes(volumes, point))
                }
            },
            None => {
                unmatched += 1;
                unmatched_containing = containing_volumes(volumes, face_centroid(face, geometry));
                &unmatched_containing
            }
        };
        // Smallest wins; on an equal size key the later (higher index) wins.
        let Some(&winner) = containing
            .iter()
            .min_by_key(|&&index| (size_keys[index], std::cmp::Reverse(index)))
        else {
            continue;
        };
        faces_of[winner].push(face as u32);
        let texture = geometry.geometry.faces[face].texture_index as usize;
        if has_emissive_map.get(texture).copied().unwrap_or(false) {
            glowing_members[winner] += 1;
            for &index in containing {
                glowing_inside[index] += 1;
            }
        }
    }

    if unmatched > 0 {
        log::warn!(
            "[Compiler] emissive volumes: {unmatched} static world face(s) matched no authored \
             brush side; each tests its own centroid, so its BSP or atlas pieces may differ in \
             membership"
        );
    }
    for (index, volume) in volumes.iter().enumerate() {
        if glowing_members[index] > 0 {
            continue;
        }
        let name = describe(volume);
        if glowing_inside[index] > 0 {
            log::warn!(
                "[Compiler] {name} holds no face with an `_e` map: smaller or later emissive \
                 volumes take all {} glowing face(s) inside it; it glows nothing",
                glowing_inside[index]
            );
        } else {
            log::warn!("[Compiler] {name} holds no face with an `_e` map; it glows nothing");
        }
    }
    let member_faces: usize = faces_of.iter().map(Vec::len).sum();
    log::info!(
        "[Compiler] Emissive volumes: {} volume(s), {member_faces} member face(s)",
        volumes.len()
    );

    volumes
        .iter()
        .zip(faces_of)
        .map(|(volume, faces)| EmissiveVolumeRecord {
            min: volume.min,
            max: volume.max,
            color: volume.color,
            origin: volume.origin,
            faces,
            tags: volume.tags.clone(),
        })
        .collect()
}

/// The volume's label, tags and origin, so a mapper can find it.
fn describe(volume: &MapEmissiveVolume) -> String {
    let [x, y, z] = volume.origin;
    let tags = if volume.tags.is_empty() {
        String::new()
    } else {
        format!(" (tags: {})", volume.tags.join(" "))
    };
    format!(
        "{}{tags} at ({x:.3}, {y:.3}, {z:.3}) m",
        volume.source_label
    )
}

/// Indices of the volumes one of whose hulls holds `point`, ascending.
fn containing_volumes(volumes: &[MapEmissiveVolume], point: DVec3) -> Vec<usize> {
    let slack = DVec3::splat(MEMBERSHIP_TOLERANCE_M);
    volumes
        .iter()
        .enumerate()
        .filter(|(_, volume)| {
            volume.hulls.iter().any(|hull| {
                if point.cmplt(hull.min - slack).any() || point.cmpgt(hull.max + slack).any() {
                    return false;
                }
                hull.planes.iter().all(|&(normal, distance)| {
                    normal.dot(point) - distance <= MEMBERSHIP_TOLERANCE_M
                })
            })
        })
        .map(|(index, _)| index)
        .collect()
}

/// The authored side `face` lies on: `(brush, side)` indexing
/// `brush_volumes[brush].sides[side]`. Every vertex lies within tolerance of
/// the side's plane, the normals agree and the textures match; of several
/// (a sliver along a shared edge), the best-aligned normal wins.
fn authored_side(
    face: usize,
    geometry: &GeometryResult,
    face_brushes: &[Option<u32>],
    brush_volumes: &[BrushVolume],
) -> Option<(u32, usize)> {
    let brush = face_brushes.get(face).copied().flatten()?;
    let sides = &brush_volumes.get(brush as usize)?.sides;
    let meta = geometry.geometry.faces.get(face)?;
    let texture = geometry
        .texture_names
        .names
        .get(meta.texture_index as usize)?;
    let indices = face_indices(face, geometry)?;
    let first = *indices.first()?;
    let normal = DVec3::from(
        octahedral::decode(geometry.geometry.vertices[first as usize].normal_oct).map(f64::from),
    )
    .normalize_or_zero();
    sides
        .iter()
        .enumerate()
        .filter(|(_, side)| side.texture == *texture)
        .map(|(index, side)| (index, side.normal.dot(normal), side))
        .filter(|&(_, cos, side)| {
            cos >= SIDE_NORMAL_MIN_COS
                && indices.iter().all(|&vertex| {
                    let position = DVec3::from(
                        geometry.geometry.vertices[vertex as usize]
                            .position
                            .map(f64::from),
                    );
                    (side.normal.dot(position) - side.distance).abs() <= MEMBERSHIP_TOLERANCE_M
                })
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _, _)| (brush, index))
}

/// The face's triangle-list indices.
fn face_indices(face: usize, geometry: &GeometryResult) -> Option<&[u32]> {
    let range = geometry.face_index_ranges.get(face)?;
    let start = range.index_offset as usize;
    geometry
        .geometry
        .indices
        .get(start..start + range.index_count as usize)
}

/// A final face's own area-weighted centroid: the fallback when it matches
/// no authored side.
fn face_centroid(face: usize, geometry: &GeometryResult) -> DVec3 {
    let position = |vertex: u32| {
        DVec3::from(
            geometry.geometry.vertices[vertex as usize]
                .position
                .map(f64::from),
        )
    };
    let triangles: Vec<[DVec3; 3]> = face_indices(face, geometry)
        .unwrap_or_default()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|triangle| triangle.map(position))
        .collect();
    weighted_centroid(&triangles)
}

/// A convex polygon's area-weighted centroid, its vertices in winding order.
fn polygon_centroid(vertices: &[DVec3]) -> DVec3 {
    let Some(&first) = vertices.first() else {
        return DVec3::ZERO;
    };
    let triangles: Vec<[DVec3; 3]> = vertices
        .windows(2)
        .skip(1)
        .map(|pair| [first, pair[0], pair[1]])
        .collect();
    if triangles.is_empty() {
        return vertices.iter().copied().sum::<DVec3>() / vertices.len() as f64;
    }
    weighted_centroid(&triangles)
}

/// Area-weighted centroid of `triangles`; their vertex mean when every one is
/// degenerate.
fn weighted_centroid(triangles: &[[DVec3; 3]]) -> DVec3 {
    let mut sum = DVec3::ZERO;
    let mut weight = 0.0;
    for [a, b, c] in triangles {
        let area = (*b - *a).cross(*c - *a).length();
        sum += (*a + *b + *c) / 3.0 * area;
        weight += area;
    }
    if weight > 0.0 {
        return sum / weight;
    }
    let count = triangles.len() * 3;
    if count == 0 {
        return DVec3::ZERO;
    }
    triangles.iter().flatten().copied().sum::<DVec3>() / count as f64
}

#[cfg(test)]
#[path = "emissive_membership_tests.rs"]
mod tests;
