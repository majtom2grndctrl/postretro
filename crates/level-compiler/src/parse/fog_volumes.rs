//! Fog volume, lamp and tube resolution from FGD key-values.
//! See: context/lib/build_pipeline.md §Custom FGD

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use glam::DVec3;
use shambler::GeoMap;
use shambler::brush::BrushId;

use super::authoring_regions::{BrushRegionBounds, resolve_brush_region_bounds};
use super::{quake_to_engine, shambler_to_dvec3};
use crate::map_data::MapFogVolume;
use postretro_level_format::fog_volumes::MAX_PLANES_PER_VOLUME;

/// Parse a `tint` color255 string like "255 128 64" into a linear [0,1] float triple.
/// Values outside 0-255 are rejected; missing or malformed values return `None`.
fn parse_fog_tint(s: &str) -> Option<[f32; 3]> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 3 {
        return None;
    }
    let mut out = [0.0f32; 3];
    for (i, p) in parts.iter().enumerate() {
        let v: i32 = p.parse().ok()?;
        if !(0..=255).contains(&v) {
            return None;
        }
        out[i] = v as f32 / 255.0;
    }
    Some(out)
}

/// Clamp `light_range` to a strictly-positive minimum.
///
/// `light_range = 0` is degenerate: the fog shader's
/// `clamp(1.0 - dist / (range * vs_light_range), 0.0, 1.0)` would divide by
/// zero when the range is 0. Authors almost certainly don't intend this.
/// Negative and non-finite values are equally nonsensical here.
fn clamp_light_range(value: f32, classname: &str) -> f32 {
    const MIN: f32 = 0.001;
    if !value.is_finite() || value <= 0.0 {
        log::warn!(
            "[Compiler] {classname}: light_range {value} is non-positive or non-finite; clamping to {MIN}"
        );
        return MIN;
    }
    MIN.max(value)
}

fn scatter_bias_to_anisotropy(value: f32, classname: &str) -> f32 {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        log::warn!(
            "[Compiler] {classname}: scatter_bias {value} is outside 0..100 or non-finite; clamping"
        );
    }
    if value.is_finite() {
        // Maps 0–100 authoring range to HG g ∈ [0, 0.9]; 0.9 matches HG_MAX_G in the
        // shader (avoids singularity at g=1). Out-of-range inputs trigger the warning
        // above and clamp: negatives → 0.0, over-range → 0.9.
        (value / 100.0 * 0.9).clamp(0.0, 0.9)
    } else {
        0.0
    }
}

fn clamp_ambient_scatter(value: f32, classname: &str) -> f32 {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        log::warn!(
            "[Compiler] {classname}: ambient_scatter {value} is outside 0..1 or non-finite; clamping"
        );
    }
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Compute a fog_volume brush entity's world-space AABB and bounding planes from its brush faces and
/// parse its KVP-authored parameters. Returns `None` when the brush set
/// produces no usable vertices (degenerate authoring). Returns `Err` when the
/// brush hull yields zero face planes (degenerate convex hull) or more than 16
/// (exceeds the per-volume plane budget).
pub(super) fn resolve_fog_volume(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    props: &HashMap<String, String>,
    scale: f64,
    classname: &str,
) -> Result<Option<MapFogVolume>> {
    let Some(BrushRegionBounds {
        min, max, planes, ..
    }) = resolve_brush_region_bounds(geo_map, brush_ids, scale, classname)?
    else {
        return Ok(None);
    };
    if planes.len() > MAX_PLANES_PER_VOLUME {
        anyhow::bail!(
            "{classname}: brush hull yielded {} face planes — exceeds the {}-plane limit; simplify the brush",
            planes.len(),
            MAX_PLANES_PER_VOLUME,
        );
    }

    let density = props
        .get("density")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.5);
    let edge_softness = props
        .get("edge_softness")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(1.0);
    let glow = props
        .get("glow")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.6);
    let tint = props
        .get("tint")
        .and_then(|s| parse_fog_tint(s))
        .unwrap_or([1.0, 1.0, 1.0]);
    let saturation = props
        .get("saturation")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(1.0);
    let min_brightness = props
        .get("min_brightness")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.0_f32);
    let light_range = props
        .get("light_range")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| clamp_light_range(v, classname))
        .unwrap_or(1.0_f32);
    let anisotropy = props
        .get("scatter_bias")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| scatter_bias_to_anisotropy(v, classname))
        .unwrap_or(0.0);
    let ambient_scatter = props
        .get("ambient_scatter")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| clamp_ambient_scatter(v, classname))
        .unwrap_or(1.0);
    let tags: Vec<String> = props
        .get("_tags")
        .map(|s| s.split_whitespace().map(|t| t.to_string()).collect())
        .unwrap_or_default();

    log::info!(
        "[Compiler] {classname}: aabb [{:.3}, {:.3}, {:.3}]–[{:.3}, {:.3}, {:.3}], density={density}, planes={}",
        min.x,
        min.y,
        min.z,
        max.x,
        max.y,
        max.z,
        planes.len(),
    );

    Ok(Some(MapFogVolume {
        min: [min.x as f32, min.y as f32, min.z as f32],
        max: [max.x as f32, max.y as f32, max.z as f32],
        density,
        edge_softness,
        glow,
        radial_falloff: 0.0,
        tint,
        saturation,
        min_brightness,
        light_range,
        anisotropy,
        ambient_scatter,
        planes,
        tags,
        is_ellipsoid: false,
    }))
}

/// Produces a `MapFogVolume` with `is_ellipsoid: true` and no bounding planes.
/// AABB derived by walking all vertices across all brushes; multi-brush entities are
/// accepted and their AABBs unioned. Zero-extent AABB on any axis is an error.
pub(super) fn resolve_fog_ellipsoid(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    props: &HashMap<String, String>,
    scale: f64,
    classname: &str,
) -> Result<MapFogVolume> {
    use shambler::brush::brush_hulls;
    use shambler::face::{face_planes, face_vertices};

    let geo_planes = face_planes(&geo_map.face_planes);
    let entity_brush_faces: BTreeMap<BrushId, Vec<shambler::face::FaceId>> = brush_ids
        .iter()
        .filter_map(|bid| {
            geo_map
                .brush_faces
                .get(bid)
                .map(|faces| (*bid, faces.clone()))
        })
        .collect();
    let hulls = brush_hulls(&entity_brush_faces, &geo_planes);
    let (face_verts, _) = face_vertices(&entity_brush_faces, &geo_planes, &hulls);

    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for verts in face_verts.values() {
        for v in verts {
            let p = quake_to_engine(shambler_to_dvec3(v)) * scale;
            min = min.min(p);
            max = max.max(p);
        }
    }
    if min.x == f64::INFINITY {
        anyhow::bail!(
            "{classname}: brushes produced no usable vertices — axis-aligned fog_volume needs a non-degenerate brush"
        );
    }

    let extent = max - min;
    if extent.x <= 0.0 || extent.y <= 0.0 || extent.z <= 0.0 {
        anyhow::bail!(
            "{classname}: brush AABB has zero extent on at least one axis ({:.6}, {:.6}, {:.6}) — axis-aligned fog_volume needs a non-degenerate volume on all three axes",
            extent.x,
            extent.y,
            extent.z,
        );
    }

    let density = props
        .get("density")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.5);
    let glow = props
        .get("glow")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.6);
    let radial_falloff = props
        .get("falloff")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(2.0);
    let tint = props
        .get("tint")
        .and_then(|s| parse_fog_tint(s))
        .unwrap_or([1.0, 1.0, 1.0]);
    let saturation = props
        .get("saturation")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(1.0);
    let min_brightness = props
        .get("min_brightness")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.0_f32);
    let light_range = props
        .get("light_range")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| clamp_light_range(v, classname))
        .unwrap_or(1.0_f32);
    let anisotropy = props
        .get("scatter_bias")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| scatter_bias_to_anisotropy(v, classname))
        .unwrap_or(0.0);
    let ambient_scatter = props
        .get("ambient_scatter")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| clamp_ambient_scatter(v, classname))
        .unwrap_or(1.0);
    let tags: Vec<String> = props
        .get("_tags")
        .map(|s| s.split_whitespace().map(|t| t.to_string()).collect())
        .unwrap_or_default();

    log::info!(
        "[Compiler] {classname}: aabb [{:.3}, {:.3}, {:.3}]–[{:.3}, {:.3}, {:.3}], density={density}, falloff={radial_falloff}",
        min.x,
        min.y,
        min.z,
        max.x,
        max.y,
        max.z,
    );

    Ok(MapFogVolume {
        min: [min.x as f32, min.y as f32, min.z as f32],
        max: [max.x as f32, max.y as f32, max.z as f32],
        density,
        edge_softness: 0.0,
        glow,
        radial_falloff,
        tint,
        saturation,
        min_brightness,
        light_range,
        anisotropy,
        ambient_scatter,
        planes: Vec::new(),
        tags,
        is_ellipsoid: true,
    })
}

/// Resolve a `fog_lamp` point entity into a spherical fog volume.
pub(super) fn resolve_fog_lamp(
    props: &HashMap<String, String>,
    origin: DVec3,
    scale: f64,
    classname: &str,
) -> Result<MapFogVolume> {
    // TrenchBroom only writes KVPs that the author explicitly sets; FGD defaults
    // are shown in the UI but not saved to the .map. Fall back to the FGD default
    // (64 map units) so a freshly placed fog_lamp compiles without manual edits.
    let radius_raw = props
        .get("radius")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(64.0);
    if !(radius_raw.is_finite() && radius_raw > 0.0) {
        anyhow::bail!("{classname}: `radius` must be a finite positive number, got {radius_raw}");
    }
    // `radius` is authored in map units; convert to engine meters so it agrees
    // with `origin`, which has already been unit-scaled by the caller.
    let radius = (radius_raw as f64 * scale) as f32;

    let density = props
        .get("density")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.5);
    let glow = props
        .get("glow")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.6);
    let radial_falloff = props
        .get("radial_falloff")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(2.0);
    let tint = props
        .get("tint")
        .and_then(|s| parse_fog_tint(s))
        .unwrap_or([1.0, 1.0, 1.0]);
    let saturation = props
        .get("saturation")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(1.0);
    let min_brightness = props
        .get("min_brightness")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.0_f32);
    let light_range = props
        .get("light_range")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| clamp_light_range(v, classname))
        .unwrap_or(1.0_f32);
    let anisotropy = props
        .get("scatter_bias")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| scatter_bias_to_anisotropy(v, classname))
        .unwrap_or(0.0);
    let ambient_scatter = props
        .get("ambient_scatter")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| clamp_ambient_scatter(v, classname))
        .unwrap_or(1.0);
    let tags: Vec<String> = props
        .get("_tags")
        .map(|s| s.split_whitespace().map(|t| t.to_string()).collect())
        .unwrap_or_default();

    let ox = origin.x as f32;
    let oy = origin.y as f32;
    let oz = origin.z as f32;
    let min = [ox - radius, oy - radius, oz - radius];
    let max = [ox + radius, oy + radius, oz + radius];

    log::info!(
        "[Compiler] {classname}: origin ({ox:.3}, {oy:.3}, {oz:.3}) radius={radius}, density={density}",
    );

    Ok(MapFogVolume {
        min,
        max,
        density,
        // Semantic point entities use `radial_falloff`; the primitive-only
        // edge softness slot is unused.
        edge_softness: 0.0,
        glow,
        radial_falloff,
        tint,
        saturation,
        min_brightness,
        light_range,
        anisotropy,
        ambient_scatter,
        planes: Vec::new(),
        tags,
        is_ellipsoid: false,
    })
}

/// Resolve a `fog_tube` point entity into a capsule-shaped fog volume.
///
/// Yaw rotates around +Y first; pitch then rotates around the resulting +X
/// (intrinsic Y-X). The capsule axis starts as +Y in local space.
pub(super) fn resolve_fog_tube(
    props: &HashMap<String, String>,
    origin: DVec3,
    scale: f64,
    classname: &str,
) -> Result<MapFogVolume> {
    // TrenchBroom only writes KVPs that the author explicitly sets; FGD defaults
    // are shown in the UI but not saved to the .map. Fall back to FGD defaults
    // so a freshly placed fog_tube compiles without manual edits.
    let radius_raw = props
        .get("radius")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(32.0);
    if !(radius_raw.is_finite() && radius_raw > 0.0) {
        anyhow::bail!("{classname}: `radius` must be a finite positive number, got {radius_raw}");
    }
    let height_raw = props
        .get("height")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(128.0);
    if !(height_raw.is_finite() && height_raw > 0.0) {
        anyhow::bail!("{classname}: `height` must be a finite positive number, got {height_raw}");
    }
    // `radius` and `height` are authored in map units; convert to engine meters
    // so they agree with `origin`, which has already been unit-scaled.
    let radius = (radius_raw as f64 * scale) as f32;
    let height = (height_raw as f64 * scale) as f32;

    let pitch_deg = props
        .get("pitch")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.0);
    let yaw_deg = props
        .get("yaw")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.0);
    let pitch = pitch_deg.to_radians();
    let yaw = yaw_deg.to_radians();

    // Intrinsic Y-X Euler: yaw rotates around +Y first, then pitch around the
    // resulting +X. This is the same convention encoded in the `model()` `angles`
    // expression in `postretro.fgd` — both must agree so the editor display model
    // and the runtime AABB rotate identically when an author changes pitch or yaw.
    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    let ax = -sp * sy;
    let ay = cp;
    let az = -sp * cy;
    let len = (ax * ax + ay * ay + az * az).sqrt().max(1.0e-6);
    let a = [ax / len, ay / len, az / len];

    let half_segment = (height * 0.5 - radius).max(0.0);
    let half_extent = [
        a[0].abs() * half_segment + radius,
        a[1].abs() * half_segment + radius,
        a[2].abs() * half_segment + radius,
    ];

    let density = props
        .get("density")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.3);
    let glow = props
        .get("glow")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.6);
    let radial_falloff = props
        .get("radial_falloff")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(1.5);
    let tint = props
        .get("tint")
        .and_then(|s| parse_fog_tint(s))
        .unwrap_or([1.0, 1.0, 1.0]);
    let saturation = props
        .get("saturation")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(1.0);
    let min_brightness = props
        .get("min_brightness")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.0_f32);
    let light_range = props
        .get("light_range")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| clamp_light_range(v, classname))
        .unwrap_or(1.0_f32);
    let anisotropy = props
        .get("scatter_bias")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| scatter_bias_to_anisotropy(v, classname))
        .unwrap_or(0.0);
    let ambient_scatter = props
        .get("ambient_scatter")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| clamp_ambient_scatter(v, classname))
        .unwrap_or(1.0);
    let tags: Vec<String> = props
        .get("_tags")
        .map(|s| s.split_whitespace().map(|t| t.to_string()).collect())
        .unwrap_or_default();

    let ox = origin.x as f32;
    let oy = origin.y as f32;
    let oz = origin.z as f32;
    let min = [
        ox - half_extent[0],
        oy - half_extent[1],
        oz - half_extent[2],
    ];
    let max = [
        ox + half_extent[0],
        oy + half_extent[1],
        oz + half_extent[2],
    ];

    log::info!(
        "[Compiler] {classname}: origin ({ox:.3}, {oy:.3}, {oz:.3}) radius={radius} height={height} pitch={pitch_deg} yaw={yaw_deg}",
    );

    Ok(MapFogVolume {
        min,
        max,
        density,
        // Semantic point entities use `radial_falloff`; the primitive-only
        // edge softness slot is unused.
        edge_softness: 0.0,
        glow,
        radial_falloff,
        tint,
        saturation,
        min_brightness,
        light_range,
        anisotropy,
        ambient_scatter,
        planes: Vec::new(),
        tags,
        is_ellipsoid: false,
    })
}
