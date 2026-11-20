// Emissive volume membership: which final static world faces each `emissive_volume` holds, as id 60's volume records.
// See: context/lib/build_pipeline.md §Compiler pipeline · resource_management.md §4.5

use glam::DVec3;
use postretro_level_format::emissive::EmissiveVolumeRecord;

use crate::geometry::GeometryResult;
use crate::map_data::MapEmissiveVolume;
use crate::parse::FLUSH_TOLERANCE_METERS;

/// Containment slack, engine meters: a face in the plane of a volume brush's
/// side joins (`n · p - d <= tolerance`).
const MEMBERSHIP_TOLERANCE_M: f64 = FLUSH_TOLERANCE_METERS;

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

/// Each face's membership point, taken before the atlas cut: a BSP
/// fragment's own area-weighted centroid, or `None` for a face with no
/// triangle, which never joins a volume. Remapped through the cut
/// ([`crate::geometry::remap_per_face`]), every sub-face of one fragment
/// carries its fragment's point, so the cut never splits a membership.
pub(crate) fn fragment_centroids(geometry: &GeometryResult) -> Vec<Option<DVec3>> {
    (0..geometry.geometry.faces.len())
        .map(|face| face_centroid(face, geometry))
        .collect()
}

/// Id 60's volume records, in map order, from the final static world: after
/// the atlas cut, on the face indices the Geometry section (id 17) stores.
/// Movers and fluids are not in that geometry, so no volume lists them.
///
/// Membership is per BSP fragment: `fragment_centroids[face]` is the centroid
/// of the pre-cut face `face` came from ([`fragment_centroids`], remapped
/// through the cut), and a face joins a volume when that point lies in one of
/// its hulls, inclusive within [`MEMBERSHIP_TOLERANCE_M`]. A glowing brush
/// side the BSP splits across two volumes therefore splits between them,
/// while the atlas's sub-faces of one fragment share its answer. A face with
/// no triangle, or no point, joins nothing.
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
    fragment_centroids: &[Option<DVec3>],
    has_emissive_map: &[bool],
) -> Vec<EmissiveVolumeRecord> {
    if volumes.is_empty() {
        return Vec::new();
    }
    debug_assert_eq!(
        fragment_centroids.len(),
        geometry.geometry.faces.len(),
        "one membership point per final face"
    );
    let size_keys: Vec<u32> = volumes
        .iter()
        .map(|volume| size_key(volume.summed_volume))
        .collect();
    let mut faces_of: Vec<Vec<u32>> = vec![Vec::new(); volumes.len()];
    let mut glowing_members = vec![0usize; volumes.len()];
    let mut glowing_inside = vec![0usize; volumes.len()];

    for face in 0..geometry.geometry.faces.len() {
        // The runtime reaches a face's vertices through its BVH leaf, and a
        // face with no triangle has none: it can never present a volume.
        let has_triangle = geometry
            .face_index_ranges
            .get(face)
            .is_some_and(|range| range.index_count > 0);
        let Some(point) = fragment_centroids
            .get(face)
            .copied()
            .flatten()
            .filter(|_| has_triangle)
        else {
            continue;
        };
        let containing = containing_volumes(volumes, point);
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
            for &index in &containing {
                glowing_inside[index] += 1;
            }
        }
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

/// Fail the build when a volume holds faces and some world face has no
/// triangle. The loader stamps each face's volume id through that face's one
/// BVH leaf, and once any volume holds a face it recovers every face's range
/// that way: a triangle-less face anywhere refuses the level, not only one
/// near a volume. Run as soon as membership resolves, so the build stops
/// before reflection capture; the pack-time check stays as a backstop.
pub(crate) fn refuse_members_beside_a_triangle_less_face(
    volumes: &[MapEmissiveVolume],
    records: &[EmissiveVolumeRecord],
    geometry: &GeometryResult,
    face_brushes: &[Option<u32>],
) -> anyhow::Result<()> {
    let holding: Vec<&MapEmissiveVolume> = volumes
        .iter()
        .zip(records)
        .filter(|(_, record)| !record.faces.is_empty())
        .map(|(volume, _)| volume)
        .collect();
    let Some(&first) = holding.first() else {
        return Ok(());
    };
    let Some(face) = geometry
        .face_index_ranges
        .iter()
        .position(|range| range.index_count == 0)
    else {
        return Ok(());
    };
    let texture = geometry
        .geometry
        .faces
        .get(face)
        .and_then(|meta| {
            geometry
                .texture_names
                .names
                .get(meta.texture_index as usize)
        })
        .map_or("?", String::as_str);
    let brush = match face_brushes.get(face).copied().flatten() {
        Some(brush) => format!("brush {brush}"),
        None => "no source brush".to_owned(),
    };
    let others = match holding.len() - 1 {
        0 => String::new(),
        more => format!(" (and {more} more emissive volume(s))"),
    };
    anyhow::bail!(
        "{}{others} holds faces, but world face {face} (texture '{texture}', {brush}) has no \
         triangle, and a level whose emissive volumes hold faces needs a triangle on every \
         world face. Remove or rebuild that degenerate brush, or remove the emissive volumes",
        describe(first)
    )
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

/// The face's triangle-list indices.
fn face_indices(face: usize, geometry: &GeometryResult) -> Option<&[u32]> {
    let range = geometry.face_index_ranges.get(face)?;
    let start = range.index_offset as usize;
    geometry
        .geometry
        .indices
        .get(start..start + range.index_count as usize)
}

/// A face's area-weighted centroid; its vertex mean when every triangle is
/// degenerate; `None` when it has no triangle.
fn face_centroid(face: usize, geometry: &GeometryResult) -> Option<DVec3> {
    let position = |vertex: u32| {
        DVec3::from(
            geometry.geometry.vertices[vertex as usize]
                .position
                .map(f64::from),
        )
    };
    let triangles = face_indices(face, geometry)?.as_chunks::<3>().0;
    if triangles.is_empty() {
        return None;
    }
    let mut sum = DVec3::ZERO;
    let mut weight = 0.0;
    let mut mean = DVec3::ZERO;
    for triangle in triangles {
        let [a, b, c] = triangle.map(position);
        let area = (b - a).cross(c - a).length();
        sum += (a + b + c) / 3.0 * area;
        weight += area;
        mean += a + b + c;
    }
    Some(if weight > 0.0 {
        sum / weight
    } else {
        mean / (triangles.len() * 3) as f64
    })
}

#[cfg(test)]
#[path = "emissive_membership_tests.rs"]
mod tests;
