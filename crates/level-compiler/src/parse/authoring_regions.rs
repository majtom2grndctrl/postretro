//! Compiler-only brush-region resolution in canonical engine space.
//! See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use glam::DVec3;
use shambler::GeoMap;
use shambler::brush::{BrushId, brush_hulls};
use shambler::face::{face_planes, face_vertices};

use crate::map_data::MapLightmapScaleRegion;

use super::{parse_optional_finite_f32, quake_to_engine, shambler_to_dvec3};

/// Convex brush bounds shared by compiler-only region entities. Positions and
/// plane distances are in engine meters; plane normals are swizzled directions
/// and therefore deliberately do not receive the map unit scale.
pub(super) struct BrushRegionBounds {
    pub(super) min: DVec3,
    pub(super) max: DVec3,
    pub(super) planes: Vec<[f32; 4]>,
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
    for (face_id, verts) in face_verts.iter() {
        let mut face_seen_vertex = false;
        for vertex in verts {
            let point = quake_to_engine(shambler_to_dvec3(vertex)) * scale;
            min = min.min(point);
            max = max.max(point);
            have_any = true;
            face_seen_vertex = true;
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
        log::warn!("[Compiler] {classname} has no usable brush vertices; skipping");
        return Ok(None);
    }
    if planes.is_empty() {
        anyhow::bail!(
            "{classname}: brush hull yielded zero face planes — region needs a non-degenerate convex hull"
        );
    }

    Ok(Some(BrushRegionBounds { min, max, planes }))
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
