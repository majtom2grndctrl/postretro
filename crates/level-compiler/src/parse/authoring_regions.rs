//! Compiler-only brush-region resolution in canonical engine space.
//! See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use glam::DVec3;
use shambler::GeoMap;
use shambler::brush::{BrushId, brush_hulls};
use shambler::face::{face_planes, face_vertices};

use crate::map_data::{MapLightmapScaleRegion, MapStreamingHintRegion, MapStreamingPriorityRegion};

use super::{parse_optional_finite_f32, quake_to_engine, shambler_to_dvec3};

/// Convex brush bounds shared by compiler-only region entities. Positions and
/// plane distances are in engine meters; plane normals are swizzled directions
/// and therefore deliberately do not receive the map unit scale.
pub(super) struct BrushRegionBounds {
    pub(super) min: DVec3,
    pub(super) max: DVec3,
    pub(super) planes: Vec<[f32; 4]>,
    vertices: Vec<DVec3>,
}

/// Resolve one brush entity into its world-space AABB and source-hull planes.
///
/// The caller owns entity-specific plane budgets and KVP validation. Returning
/// `None` for a brush with no usable vertices matches the fog-volume path and
/// lets a malformed invisible region stay out of the static world geometry.
pub(super) fn resolve_brush_region_bounds(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    scale: f64,
    classname: &str,
) -> Result<Option<BrushRegionBounds>> {
    resolve_brush_region_bounds_with_empty_policy(geo_map, brush_ids, scale, classname, true)
}

fn resolve_brush_region_bounds_with_empty_policy(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    scale: f64,
    classname: &str,
    warn_when_empty: bool,
) -> Result<Option<BrushRegionBounds>> {
    let geo_planes = face_planes(&geo_map.face_planes);
    let entity_brush_faces: BTreeMap<BrushId, Vec<shambler::face::FaceId>> = brush_ids
        .iter()
        .filter_map(|brush_id| {
            geo_map
                .brush_faces
                .get(brush_id)
                .map(|faces| (*brush_id, faces.clone()))
        })
        .collect();
    let hulls = brush_hulls(&entity_brush_faces, &geo_planes);
    let (face_verts, _) = face_vertices(&entity_brush_faces, &geo_planes, &hulls);

    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    let mut have_any = false;
    let mut planes = Vec::new();
    let mut vertices = Vec::new();
    for (face_id, verts) in face_verts.iter() {
        let mut face_seen_vertex = false;
        for vertex in verts {
            let point = quake_to_engine(shambler_to_dvec3(vertex)) * scale;
            min = min.min(point);
            max = max.max(point);
            have_any = true;
            face_seen_vertex = true;
            vertices.push(point);
        }
        if !face_seen_vertex {
            continue;
        }
        let Some(plane) = geo_planes.get(face_id) else {
            continue;
        };
        let normal = quake_to_engine(shambler_to_dvec3(plane.normal()));
        let point = quake_to_engine(shambler_to_dvec3(&verts[0])) * scale;
        let distance = normal.dot(point);
        planes.push([
            normal.x as f32,
            normal.y as f32,
            normal.z as f32,
            distance as f32,
        ]);
    }
    if !have_any {
        if warn_when_empty {
            log::warn!("[Compiler] {classname} has no usable brush vertices; skipping");
        }
        return Ok(None);
    }
    if planes.is_empty() {
        anyhow::bail!(
            "{classname}: brush hull yielded zero face planes — region needs a non-degenerate convex hull"
        );
    }

    Ok(Some(BrushRegionBounds {
        min,
        max,
        planes,
        vertices,
    }))
}

/// Resolve a streaming-hint brush with the stricter authoring contract used by
/// seam, resident, and priority hints. Unlike lightmap regions, malformed
/// streaming hints must stop the bake because later clustering cannot safely
/// infer the author's intended volume.
pub(super) fn resolve_streaming_hint_region(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    scale: f64,
    classname: &str,
    authored_origin: Option<DVec3>,
) -> Result<MapStreamingHintRegion> {
    let location = authoring_location(authored_origin);
    let bounds =
        resolve_brush_region_bounds_with_empty_policy(geo_map, brush_ids, scale, classname, false)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{classname} {location} has an invalid brush hull: no usable vertices"
                )
            })?;

    if !bounds.min.is_finite()
        || !bounds.max.is_finite()
        || bounds.vertices.iter().any(|vertex| !vertex.is_finite())
        || bounds
            .planes
            .iter()
            .any(|plane| plane.iter().any(|value| !value.is_finite()))
    {
        anyhow::bail!("{classname} {location} has a non-finite brush hull");
    }

    let extent = bounds.max - bounds.min;
    if extent.x <= 0.0
        || extent.y <= 0.0
        || extent.z <= 0.0
        || !spans_positive_volume(&bounds.vertices)
    {
        anyhow::bail!("{classname} {location} has a zero-volume brush hull");
    }
    if bounds.planes.len() < 4 {
        anyhow::bail!(
            "{classname} {location} has an invalid brush hull: expected at least four bounding planes"
        );
    }
    let (min, max) = cast_streaming_hint_aabb(bounds.min, bounds.max, classname, &location)?;

    let source_location = authored_origin.unwrap_or((bounds.min + bounds.max) * 0.5);
    if !source_location.is_finite() {
        anyhow::bail!("{classname} {location} has a non-finite source location");
    }
    let source_location = source_location.to_array().map(|value| value as f32);
    if source_location.iter().any(|value| !value.is_finite()) {
        anyhow::bail!("{classname} {location} source location exceeds engine precision");
    }

    Ok(MapStreamingHintRegion {
        min,
        max,
        planes: bounds.planes,
        source_location,
    })
}

/// Preserve the compiler's f32 map-data contract: a finite f64 hull cannot
/// silently become an infinite canonical AABB at this storage boundary.
fn cast_streaming_hint_aabb(
    min: DVec3,
    max: DVec3,
    classname: &str,
    location: &str,
) -> Result<([f32; 3], [f32; 3])> {
    let min = min.to_array().map(|value| value as f32);
    let max = max.to_array().map(|value| value as f32);
    if min.iter().any(|value| !value.is_finite()) || max.iter().any(|value| !value.is_finite()) {
        anyhow::bail!("{classname} {location} brush hull exceeds engine precision");
    }
    Ok((min, max))
}

/// Parse the optional authoring priority. An empty TrenchBroom field is the
/// same as absence, while every supplied non-integer or out-of-range value is
/// a hard author error.
pub(super) fn parse_stream_priority(
    props: &HashMap<String, String>,
    classname: &str,
    source_location: [f32; 3],
) -> Result<u8> {
    let Some(value) = props
        .get("_stream_priority")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(0);
    };
    let priority = value.parse::<i64>().map_err(|error| {
        anyhow::anyhow!(
            "{classname} at ({:.3}, {:.3}, {:.3}) m `_stream_priority` value `{value}` must be an integer in 0..=3 ({error})",
            source_location[0], source_location[1], source_location[2],
        )
    })?;
    if !(0..=3).contains(&priority) {
        anyhow::bail!(
            "{classname} at ({:.3}, {:.3}, {:.3}) m `_stream_priority` {priority} must be in 0..=3",
            source_location[0],
            source_location[1],
            source_location[2],
        );
    }
    Ok(priority as u8)
}

fn authoring_location(origin: Option<DVec3>) -> String {
    match origin {
        Some(origin) if origin.is_finite() => {
            format!("at ({:.3}, {:.3}, {:.3}) m", origin.x, origin.y, origin.z)
        }
        Some(_) => "at a non-finite authored origin".to_string(),
        None => "at an entity without an authored origin".to_string(),
    }
}

/// Return whether the resolved vertices affinely span three dimensions. This
/// detects coplanar or collapsed hulls even when their diagonal AABB happens
/// to have non-zero extent on all three axes.
fn spans_positive_volume(vertices: &[DVec3]) -> bool {
    for &origin in vertices {
        for &edge_end in vertices {
            let edge = edge_end - origin;
            if edge.length_squared() == 0.0 {
                continue;
            }
            for &plane_point in vertices {
                let area_normal = edge.cross(plane_point - origin);
                if area_normal.length_squared() == 0.0 {
                    continue;
                }
                if vertices
                    .iter()
                    .any(|&volume_point| area_normal.dot(volume_point - origin) != 0.0)
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Bind one parsed convex region to its optional-work priority.
pub(super) fn resolve_streaming_priority_region(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    props: &HashMap<String, String>,
    scale: f64,
    classname: &str,
    authored_origin: Option<DVec3>,
) -> Result<MapStreamingPriorityRegion> {
    let region =
        resolve_streaming_hint_region(geo_map, brush_ids, scale, classname, authored_origin)?;
    let priority = parse_stream_priority(props, classname, region.source_location)?;
    Ok(MapStreamingPriorityRegion { region, priority })
}

/// Resolve a brush-defined lightmap-density override. The AABB is the explicit
/// chart-origin membership classifier; source planes remain parsed alongside it
/// so this compiler-only brush entity follows the region-entity contract.
pub(super) fn resolve_lightmap_scale_region(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    props: &HashMap<String, String>,
    scale: f64,
    classname: &str,
) -> Result<Option<MapLightmapScaleRegion>> {
    let Some(bounds) = resolve_brush_region_bounds(geo_map, brush_ids, scale, classname)? else {
        return Ok(None);
    };
    let lightmap_scale = props
        .get("_lightmap_scale")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| {
            value.parse::<f32>().map_err(|error| {
                anyhow::anyhow!(
                    "{classname} `_lightmap_scale` value `{value}` is not a valid float ({error})"
                )
            })
        })
        .transpose()?
        .unwrap_or(1.0);
    if !lightmap_scale.is_finite() || lightmap_scale <= 0.0 {
        anyhow::bail!(
            "{classname} `_lightmap_scale` must be a finite positive float, got {lightmap_scale}"
        );
    }

    Ok(Some(MapLightmapScaleRegion {
        min: bounds.min.to_array().map(|value| value as f32),
        max: bounds.max.to_array().map(|value| value as f32),
        planes: bounds.planes,
        scale: lightmap_scale,
    }))
}

/// Resolve the mapper-authored SH coarsening-protection region. The AABB uses
/// the same hull union as trigger volumes; only its compiler-only dilation
/// policy differs.
pub(super) fn resolve_sh_protect_aabb(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    props: &HashMap<String, String>,
    scale: f64,
    classname: &str,
) -> Result<[f32; 6]> {
    let name = props
        .get("name")
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    let (mut min, mut max) = crate::trigger_volumes::resolve_brush_entity_aabb(
        geo_map, brush_ids, scale, classname, &name,
    )?;
    let dilation = parse_optional_finite_f32(props, "dilation", 0.0, classname, &name)?;
    if dilation < 0.0 {
        anyhow::bail!("{classname} `{name}` `dilation` must be non-negative, got {dilation}");
    }
    let dilation = DVec3::splat(dilation as f64);
    min -= dilation;
    max += dilation;
    let min = min.to_array().map(|value| value as f32);
    let max = max.to_array().map(|value| value as f32);
    Ok([min[0], min[1], min[2], max[0], max[1], max[2]])
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::cast_streaming_hint_aabb;

    #[test]
    fn streaming_hint_aabb_rejects_finite_values_that_overflow_engine_f32() {
        let error = cast_streaming_hint_aabb(
            DVec3::new(-f64::MAX, 0.0, 0.0),
            DVec3::new(f64::MAX, 1.0, 1.0),
            "streaming_seam_volume",
            "at (0.000, 0.000, 0.000) m",
        )
        .expect_err("finite source bounds that overflow f32 must fail");
        assert!(
            error.to_string().contains("streaming_seam_volume")
                && error.to_string().contains("at (")
                && error.to_string().contains("engine precision"),
            "cast failure must identify the hint and source location: {error}"
        );
    }
}
