// .map file parsing via shambler: brush classification and face extraction.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result};
use glam::DVec3;
use shambler::GeoMap;
use shambler::brush::{BrushId, brush_hulls};
use shambler::entity::EntityId;
use shambler::face::face_planes;
use shambler::face::{FaceWinding, face_centers, face_indices, face_vertices};

use crate::format::quake_map;
use crate::map_data::{
    BrushPlane, BrushSide, BrushVolume, EntityInfo, MapData, MapEntityRecord, MapFogVolume,
    MapLight, TextureProjection,
};
use crate::map_format::MapFormat;
use postretro_level_format::fog_volumes::MAX_FOG_VOLUMES;

/// Convert a shambler nalgebra Vector3 to glam DVec3.
///
/// This is the **input precision boundary**: shambler stores coordinates as f32
/// (parsed from the .map text), and we widen them to f64 here. All subsequent
/// compile-time geometry is computed in double precision.
fn shambler_to_dvec3(v: &shambler::Vector3) -> DVec3 {
    DVec3::new(v.x as f64, v.y as f64, v.z as f64)
}

/// Swizzle a direction vector from Quake coordinates (right-handed, Z-up) to
/// engine coordinates (right-handed, Y-up). For use on normals and other
/// direction vectors — does NOT apply unit scale.
///
/// Quake: +X forward, +Y left, +Z up
/// Engine: +X right, +Y up, -Z forward
///
/// engine_x = -quake_y, engine_y = quake_z, engine_z = -quake_x
///
/// For positions and plane distances, also multiply by `MapFormat::units_to_meters()`
/// after swizzling. Normals are direction vectors — scale must not be applied
/// to them (only the swizzle).
fn quake_to_engine(v: DVec3) -> DVec3 {
    DVec3::new(-v.y, v.z, -v.x)
}

/// Parse an origin string like "-192 25.6 167.736" into a DVec3.
///
/// Parses directly to f64 — no precision cast from f32.
fn parse_origin(s: &str) -> Option<DVec3> {
    let parts: Vec<f64> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() == 3 {
        Some(DVec3::new(parts[0], parts[1], parts[2]))
    } else {
        None
    }
}

/// Look up a property value by key from shambler's entity properties.
fn get_property(geo_map: &GeoMap, entity_id: &EntityId, key: &str) -> Option<String> {
    let props = geo_map.entity_properties.get(entity_id)?;
    props.iter().find(|p| p.key == key).map(|p| p.value.clone())
}

/// Extract all key-value pairs for an entity as a property bag. Thin
/// wrapper that isolates the shambler dependency from the translator.
fn collect_entity_properties(geo_map: &GeoMap, entity_id: &EntityId) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(props) = geo_map.entity_properties.get(entity_id) {
        for p in props.iter() {
            out.insert(p.key.clone(), p.value.clone());
        }
    }
    out
}

/// Read and parse a .map file, classify brushes, and extract face geometry.
///
/// The `format` parameter identifies the source map format. Its `units_to_meters()`
/// scale is applied at this boundary alongside the axis swizzle: vertex positions,
/// entity origins, and plane distances are all converted to engine meters here.
/// All downstream stages receive engine-native coordinates and meters.
pub fn parse_map_file(path: &Path, format: MapFormat) -> Result<MapData> {
    let scale = format.units_to_meters();
    let map_text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read map file: {}", path.display()))?;

    let shalrath_map: shambler::shalrath::repr::Map = map_text
        .parse()
        .map_err(|e| anyhow::anyhow!("failed to parse .map syntax: {e}"))?;

    let geo_map = GeoMap::new(shalrath_map);

    // Identify worldspawn entity
    let worldspawn_id = geo_map
        .entities
        .iter()
        .find(|id| get_property(&geo_map, id, "classname").as_deref() == Some("worldspawn"))
        .copied()
        .context("no worldspawn entity found in .map file")?;

    // Read the optional worldspawn `script` KVP. The level compiler resolves
    // this relative to the `.map` file's directory and invokes scripts-build
    // to produce a sibling `.js` artifact before packing.
    let script = get_property(&geo_map, &worldspawn_id, "script").and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    // Read the optional worldspawn `data_script` KVP. The level compiler
    // resolves this relative to the `.map` file's directory, compiles `.ts`
    // sources via scripts-build (Luau passes through), and embeds the bytes as
    // the PRL `DataScript` section. See `context/lib/scripting.md`.
    let data_script = get_property(&geo_map, &worldspawn_id, "data_script").and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    // Classify brushes: world vs entity
    let world_brush_ids: Vec<BrushId> = geo_map
        .entity_brushes
        .get(&worldspawn_id)
        .cloned()
        .unwrap_or_default();

    // Collect entity info and entity brush counts
    let mut entities = Vec::new();
    let mut entity_brushes_summary = Vec::new();
    let mut entity_classnames: Vec<String> = Vec::new();
    // Lights translate to the canonical format as we walk entities — they
    // share the origin/classname extraction but do not participate in BSP
    // construction.
    let mut lights: Vec<MapLight> = Vec::new();
    // Generic map entities for runtime classname dispatch — non-light point
    // entities only. Brush entities (those with brushes attached) are resolved
    // separately by their dedicated subsystems (e.g. `fog_volume`).
    let mut map_entities: Vec<MapEntityRecord> = Vec::new();
    // Resolved fog volume entities (brush `fog_volume` plus point `fog_lamp`
    // and `fog_tube`). Walked alongside the entity pass; brush AABBs come from
    // brush-face vertices, point-entity AABBs from origin + radius/height.
    let mut fog_volumes: Vec<MapFogVolume> = Vec::new();

    // Worldspawn `fog_pixel_scale` (1=full-res, 8=coarsest). Default 4 when
    // unset. `0` is the "unset" sentinel — pass it through as `0` so the
    // engine's `clamp_fog_pixel_scale(0)` returns its own default (4).
    // Values above 8 are author errors and are clamped to 8 silently.
    // Negative values are treated as unset (0).
    let fog_pixel_scale: u32 = get_property(&geo_map, &worldspawn_id, "fog_pixel_scale")
        .and_then(|s| s.trim().parse::<i64>().ok())
        .map(|v| v.clamp(0, 8) as u32)
        .unwrap_or(0);

    for entity_id in geo_map.entities.iter() {
        let classname =
            get_property(&geo_map, entity_id, "classname").unwrap_or_else(|| "unknown".to_string());
        // Swizzle axes then apply unit scale: origin is a position, not a direction.
        let origin = get_property(&geo_map, entity_id, "origin")
            .and_then(|s| parse_origin(&s))
            .map(|v| quake_to_engine(v) * scale);

        entities.push(EntityInfo {
            classname: classname.clone(),
            origin,
        });

        if !entity_classnames.contains(&classname) {
            entity_classnames.push(classname.clone());
        }

        if *entity_id != worldspawn_id {
            let brush_count = geo_map
                .entity_brushes
                .get(entity_id)
                .map(|v| v.len())
                .unwrap_or(0);
            entity_brushes_summary.push((classname.clone(), brush_count));
        }

        // Lights: translate the property bag into a canonical `MapLight`.
        // Errors block compilation; warnings are logged inside the translator.
        if quake_map::is_light_classname(&classname) {
            let props = collect_entity_properties(&geo_map, entity_id);
            let light_origin = origin.ok_or_else(|| {
                anyhow::anyhow!(
                    "light entity '{classname}' missing origin — all light entities must have an origin"
                )
            })?;
            match quake_map::translate_light(&props, light_origin, &classname) {
                Ok(light) => lights.push(light),
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "failed to translate {classname} at {light_origin:?}: {e}"
                    ));
                }
            }
            continue;
        }

        // Worldspawn carries scene-wide settings, not a runtime entity.
        if *entity_id == worldspawn_id {
            continue;
        }

        // Brush entities (e.g. fog_volume, env_reverb_zone) are resolved
        // separately by their dedicated subsystems and do not flow through the
        // generic classname dispatch path.
        let has_brushes = geo_map
            .entity_brushes
            .get(entity_id)
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if has_brushes {
            if classname == "fog_volume" {
                if fog_volumes.len() >= MAX_FOG_VOLUMES {
                    log::warn!(
                        "[Compiler] fog_volume cap reached ({MAX_FOG_VOLUMES}); skipping additional volume"
                    );
                    continue;
                }
                let props = collect_entity_properties(&geo_map, entity_id);
                let brush_ids = geo_map
                    .entity_brushes
                    .get(entity_id)
                    .cloned()
                    .unwrap_or_default();
                if brush_ids.len() > 1 {
                    anyhow::bail!(
                        "fog_volume entity must own exactly one brush (got {}); \
                         multi-brush volumes would silently produce a plane intersection \
                         rather than the union the author likely intended — split into \
                         separate fog_volume entities",
                        brush_ids.len()
                    );
                }
                let volume = resolve_fog_volume(&geo_map, &brush_ids, &props, scale, &classname)?;
                if let Some(v) = volume {
                    fog_volumes.push(v);
                }
            }
            continue;
        }

        if classname == "fog_lamp" || classname == "fog_tube" {
            if fog_volumes.len() >= MAX_FOG_VOLUMES {
                log::warn!(
                    "[Compiler] {classname} cap reached ({MAX_FOG_VOLUMES}); skipping additional volume"
                );
                continue;
            }
            let entity_origin = origin.ok_or_else(|| {
                anyhow::anyhow!(
                    "{classname} missing origin — point fog entities must have an origin"
                )
            })?;
            let props = collect_entity_properties(&geo_map, entity_id);
            let volume = if classname == "fog_lamp" {
                resolve_fog_lamp(&props, entity_origin, &classname)?
            } else {
                resolve_fog_tube(&props, entity_origin, &classname)?
            };
            fog_volumes.push(volume);
            continue;
        }

        // Point entities without an origin can't be placed; skip with a warning.
        let Some(entity_origin) = origin else {
            log::warn!(
                "[Compiler] entity '{classname}' has no origin; skipping (point entities must have an origin)"
            );
            continue;
        };

        let props = collect_entity_properties(&geo_map, entity_id);
        let diagnostic_ref = format!(
            "{classname} @ ({:.3}, {:.3}, {:.3})",
            entity_origin.x, entity_origin.y, entity_origin.z
        );
        let angles = quake_map::quake_to_engine_angles(&props, &diagnostic_ref);
        let tags: Vec<String> = props
            .get("_tags")
            .map(|s| s.split_whitespace().map(|t| t.to_string()).collect())
            .unwrap_or_default();
        let key_values: Vec<(String, String)> = props
            .into_iter()
            .filter(|(k, _)| !quake_map::RESERVED_MAP_ENTITY_KEYS.contains(&k.as_str()))
            .collect();

        map_entities.push(MapEntityRecord {
            classname: classname.clone(),
            origin: entity_origin,
            angles,
            key_values,
            tags,
        });
    }

    // Compute geometry for world brushes only
    let geo_planes = face_planes(&geo_map.face_planes);

    // Build brush_faces subset for world brushes only
    let world_brush_faces: BTreeMap<BrushId, Vec<shambler::face::FaceId>> = world_brush_ids
        .iter()
        .filter_map(|bid| {
            geo_map
                .brush_faces
                .get(bid)
                .map(|faces| (*bid, faces.clone()))
        })
        .collect();

    let brush_hulls = brush_hulls(&world_brush_faces, &geo_planes);
    let (face_verts, _face_vert_planes) =
        face_vertices(&world_brush_faces, &geo_planes, &brush_hulls);
    let face_ctrs = face_centers(&face_verts);
    let face_idx = face_indices(
        &geo_map.face_planes,
        &geo_planes,
        &face_verts,
        &face_ctrs,
        // Shambler's FaceWinding naming is relative to the solid interior of the brush.
        // FaceWinding::Clockwise produces ascending-angle (CCW-from-front) vertex order,
        // which is what wgpu FrontFace::Ccw requires.
        FaceWinding::Clockwise,
    );

    // Extract brush volumes with their textured sides. The BSP builder
    // consumes the bounding planes; brush-side projection consumes the
    // textured polygons. Both come out of the same per-brush walk so the
    // plane and side lists cannot drift apart.
    let mut brush_volumes: Vec<BrushVolume> = Vec::new();
    let mut total_vertex_count: usize = 0;
    let mut total_side_count: usize = 0;

    for brush_id in &world_brush_ids {
        let face_ids = match geo_map.brush_faces.get(brush_id) {
            Some(ids) => ids,
            None => continue,
        };

        let planes: Vec<BrushPlane> = face_ids
            .iter()
            .filter_map(|fid| {
                let plane = geo_planes.get(fid)?;
                // Normal: swizzle only — normals are direction vectors, scale must not be applied.
                // Distance: scaled explicitly. A plane n·x = d in Quake units becomes
                // n·x' = d * scale where x' is in meters. Scale and swizzle are independent
                // for this scalar; the swizzle is already embedded in `normal`.
                Some(BrushPlane {
                    normal: quake_to_engine(shambler_to_dvec3(plane.normal())),
                    distance: plane.distance() as f64 * scale,
                })
            })
            .collect();

        if planes.is_empty() {
            // No bounding planes survived — skip the brush entirely.
            // Brush-volume BSP construction requires a non-empty plane set
            // to define the half-space intersection that bounds the volume.
            continue;
        }

        // Compute AABB from this brush's face vertices (already in engine space).
        let mut aabb = crate::partition::Aabb::empty();
        for fid in face_ids {
            if let Some(verts) = face_verts.get(fid) {
                for v in verts {
                    aabb.expand_point(quake_to_engine(shambler_to_dvec3(v)) * scale);
                }
            }
        }

        let mut sides: Vec<BrushSide> = Vec::with_capacity(face_ids.len());

        for face_id in face_ids {
            let vertices_raw = match face_verts.get(face_id) {
                Some(v) => v,
                None => continue,
            };

            // Skip degenerate faces
            if vertices_raw.len() < 3 {
                continue;
            }

            let indices = match face_idx.get(face_id) {
                Some(i) => i,
                None => continue,
            };

            // Reorder vertices by winding indices; swizzle axes and apply unit scale.
            // Vertices are positions — both the axis swizzle and the meter scale apply.
            let vertices: Vec<DVec3> = indices
                .iter()
                .map(|&i| quake_to_engine(shambler_to_dvec3(&vertices_raw[i])) * scale)
                .collect();

            let plane = &geo_planes[face_id];
            // Normal: swizzle only — direction vector, no unit scale.
            let normal = quake_to_engine(shambler_to_dvec3(plane.normal()));
            // Distance: scaled explicitly. A plane n·x = d in Quake units becomes
            // n·x' = d * scale where x' is in meters.
            let distance = plane.distance() as f64 * scale;

            // Look up texture name
            let texture = geo_map
                .face_textures
                .get(face_id)
                .and_then(|tex_id| geo_map.textures.get(tex_id))
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());

            // Extract texture projection data (Quake space).
            let face_offset = geo_map.face_offsets.get(face_id).copied();
            let face_angle = geo_map.face_angles.get(face_id).copied().unwrap_or(0.0) as f64;
            let face_scale = geo_map.face_scales.get(face_id);

            let (scale_u, scale_v) = face_scale
                .map(|s| (s.x as f64, s.y as f64))
                .unwrap_or((1.0, 1.0));

            let tex_projection = match face_offset {
                Some(shambler::shalrath::repr::TextureOffset::Valve { u, v }) => {
                    TextureProjection::Valve {
                        u_axis: DVec3::new(u.x as f64, u.y as f64, u.z as f64),
                        u_offset: u.d as f64,
                        v_axis: DVec3::new(v.x as f64, v.y as f64, v.z as f64),
                        v_offset: v.d as f64,
                        scale_u,
                        scale_v,
                    }
                }
                Some(shambler::shalrath::repr::TextureOffset::Standard { u, v }) => {
                    TextureProjection::Standard {
                        u_offset: u as f64,
                        v_offset: v as f64,
                        angle: face_angle,
                        scale_u,
                        scale_v,
                    }
                }
                None => TextureProjection::Standard {
                    u_offset: 0.0,
                    v_offset: 0.0,
                    angle: 0.0,
                    scale_u: 1.0,
                    scale_v: 1.0,
                },
            };

            total_vertex_count += vertices.len();
            total_side_count += 1;

            sides.push(BrushSide {
                vertices,
                normal,
                distance,
                texture,
                tex_projection,
            });
        }

        brush_volumes.push(BrushVolume {
            planes,
            sides,
            aabb,
        });
    }

    // Stat logging
    let total_brushes = geo_map.brushes.len();
    let world_brush_count = world_brush_ids.len();
    let entity_brush_count = total_brushes - world_brush_count;

    log::info!("Total brushes: {total_brushes}");
    log::info!("World brushes: {world_brush_count}");
    log::info!("Entity brushes: {entity_brush_count}");
    log::info!("Brush sides: {total_side_count}");
    log::info!("Total vertices: {total_vertex_count}");
    log::info!("Entity classnames: {}", entity_classnames.join(", "));
    log::info!("Lights: {}", lights.len());
    log::info!("Map entities (classname dispatch): {}", map_entities.len());

    Ok(MapData {
        brush_volumes,
        entity_brushes: entity_brushes_summary,
        entities,
        lights,
        script,
        data_script,
        map_entities,
        fog_volumes,
        fog_pixel_scale,
    })
}

/// Compute a fog_volume brush entity's world-space AABB and bounding planes from its brush faces and
/// parse its KVP-authored parameters. Returns `None` when the brush set
/// produces no usable vertices (degenerate authoring). Returns `Err` when the
/// brush hull yields zero face planes (degenerate convex hull) or more than 16
/// (exceeds the per-volume plane budget).
fn resolve_fog_volume(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    props: &HashMap<String, String>,
    scale: f64,
    classname: &str,
) -> Result<Option<MapFogVolume>> {
    use shambler::brush::brush_hulls;
    use shambler::face::{face_planes, face_vertices};

    // Run shambler's face-vertex pipeline on the entity's brushes only — keeps
    // the worldspawn computation undisturbed.
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
    let mut have_any = false;
    let mut planes: Vec<[f32; 4]> = Vec::new();
    for (face_id, verts) in face_verts.iter() {
        let mut face_seen_vertex = false;
        for v in verts {
            let p = quake_to_engine(shambler_to_dvec3(v)) * scale;
            min = min.min(p);
            max = max.max(p);
            have_any = true;
            face_seen_vertex = true;
        }
        if !face_seen_vertex {
            continue;
        }
        let plane = match geo_planes.get(face_id) {
            Some(p) => p,
            None => continue,
        };
        let n = quake_to_engine(shambler_to_dvec3(plane.normal()));
        let any_vertex = quake_to_engine(shambler_to_dvec3(&verts[0])) * scale;
        let d = n.dot(any_vertex);
        planes.push([n.x as f32, n.y as f32, n.z as f32, d as f32]);
    }
    if !have_any {
        log::warn!("[Compiler] {classname} has no usable brush vertices; skipping");
        return Ok(None);
    }
    if planes.is_empty() {
        anyhow::bail!(
            "{classname}: brush hull yielded zero face planes — fog volume needs a non-degenerate convex hull"
        );
    }
    if planes.len() > 16 {
        anyhow::bail!(
            "{classname}: brush hull yielded {} face planes (max 16); simplify the brush",
            planes.len()
        );
    }

    // Colour authored as "R G B" 0–255; divide by 255 (no sRGB curve), to match
    // the convention used across the FGD ecosystem.
    let color = props
        .get("color")
        .and_then(|s| parse_color255_local(s))
        .unwrap_or([1.0, 1.0, 1.0]);
    let density = props
        .get("density")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.5);
    let edge_softness = props
        .get("edge_softness")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(1.0);
    let scatter = props
        .get("scatter")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.6);
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
        color,
        density,
        edge_softness,
        scatter,
        radial_falloff: 0.0,
        planes,
        tags,
    }))
}

/// Resolve a `fog_lamp` point entity into a spherical fog volume.
fn resolve_fog_lamp(
    props: &HashMap<String, String>,
    origin: DVec3,
    classname: &str,
) -> Result<MapFogVolume> {
    let radius = props
        .get("radius")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .ok_or_else(|| {
            anyhow::anyhow!("{classname}: missing or invalid `radius` (required, > 0)")
        })?;
    if !(radius.is_finite() && radius > 0.0) {
        anyhow::bail!("{classname}: `radius` must be a finite positive number, got {radius}");
    }

    let color = props
        .get("color")
        .and_then(|s| parse_color255_local(s))
        .unwrap_or([1.0, 0.85, 0.6]);
    let density = props
        .get("density")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.5);
    let scatter = props
        .get("scatter")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.6);
    let radial_falloff = props
        .get("radial_falloff")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(2.0);
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
        color,
        density,
        // Semantic point entities use `radial_falloff`; the primitive-only
        // edge softness slot is unused.
        edge_softness: 0.0,
        scatter,
        radial_falloff,
        planes: Vec::new(),
        tags,
    })
}

/// Resolve a `fog_tube` point entity into a capsule-shaped fog volume.
///
/// Yaw rotates around +Y first; pitch then rotates around the resulting +X
/// (intrinsic Y-X). The capsule axis starts as +Y in local space.
fn resolve_fog_tube(
    props: &HashMap<String, String>,
    origin: DVec3,
    classname: &str,
) -> Result<MapFogVolume> {
    let radius = props
        .get("radius")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .ok_or_else(|| {
            anyhow::anyhow!("{classname}: missing or invalid `radius` (required, > 0)")
        })?;
    if !(radius.is_finite() && radius > 0.0) {
        anyhow::bail!("{classname}: `radius` must be a finite positive number, got {radius}");
    }
    let height = props
        .get("height")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .ok_or_else(|| {
            anyhow::anyhow!("{classname}: missing or invalid `height` (required, > 0)")
        })?;
    if !(height.is_finite() && height > 0.0) {
        anyhow::bail!("{classname}: `height` must be a finite positive number, got {height}");
    }

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

    // Local capsule axis is +Y in post-swizzle engine space; yaw rotates around
    // +Y (no-op on +Y), then pitch around the resulting +X tilts the axis into
    // the YZ plane. Engine space is Y-up after Quake-to-engine swizzle.
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

    let color = props
        .get("color")
        .and_then(|s| parse_color255_local(s))
        .unwrap_or([0.6, 0.85, 1.0]);
    let density = props
        .get("density")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.3);
    let scatter = props
        .get("scatter")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(0.6);
    let radial_falloff = props
        .get("radial_falloff")
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(1.5);
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
        color,
        density,
        // Semantic point entities use `radial_falloff`; the primitive-only
        // edge softness slot is unused.
        edge_softness: 0.0,
        scatter,
        radial_falloff,
        planes: Vec::new(),
        tags,
    })
}

/// Local "R G B" 0–255 to linear 0–1 parser. Mirrors `format::quake_map::parse_color255`
/// without crossing module visibility boundaries.
fn parse_color255_local(s: &str) -> Option<[f32; 3]> {
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

/// Re-export `quake_to_engine` for cross-module tests (geometry round-trip).
#[cfg(test)]
pub fn quake_to_engine_for_test(v: DVec3) -> DVec3 {
    quake_to_engine(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -- Coordinate transform (axis swizzle only) --
    // These tests verify the swizzle in isolation; they do not include the unit
    // scale because `quake_to_engine` is a direction-vector transform used for
    // normals as well as positions. Positions are scaled separately via
    // `MapFormat::units_to_meters()`.

    #[test]
    fn quake_to_engine_z_up_maps_to_y_up() {
        // Quake Z-up → engine Y-up (swizzle only)
        let result = quake_to_engine(DVec3::new(0.0, 0.0, 1.0));
        assert_eq!(result, DVec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn quake_to_engine_x_forward_maps_to_negative_z_forward() {
        // Quake +X forward → engine -Z forward (swizzle only)
        let result = quake_to_engine(DVec3::new(1.0, 0.0, 0.0));
        assert_eq!(result, DVec3::new(0.0, 0.0, -1.0));
    }

    #[test]
    fn quake_to_engine_y_left_maps_to_negative_x() {
        // Quake +Y left → engine -X (swizzle only)
        let result = quake_to_engine(DVec3::new(0.0, 1.0, 0.0));
        assert_eq!(result, DVec3::new(-1.0, 0.0, 0.0));
    }

    // -- Unit scale (position transform = swizzle + scale) --

    #[test]
    fn position_transform_z_up_scales_to_meters() {
        // A point at Quake Z=1 (1 inch up) → engine Y = 0.0254 m
        let scale = MapFormat::IdTech2.units_to_meters();
        let result = quake_to_engine(DVec3::new(0.0, 0.0, 1.0)) * scale;
        assert!(
            (result.y - 0.0254).abs() < 1e-6,
            "expected y=0.0254, got {}",
            result.y
        );
        assert!(result.x.abs() < 1e-6);
        assert!(result.z.abs() < 1e-6);
    }

    #[test]
    fn plane_distance_scales_to_meters() {
        // A face plane with Quake distance 64.0 → engine distance 1.6256 m (64 × 0.0254)
        let scale = MapFormat::IdTech2.units_to_meters();
        let quake_distance: f64 = 64.0;
        let engine_distance = quake_distance * scale;
        assert!(
            (engine_distance - 1.6256).abs() < 1e-5,
            "expected 1.6256, got {engine_distance}"
        );
    }

    // -- Map parsing --

    fn test_map_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("content/tests/maps/test.map")
    }

    #[test]
    fn parses_test_map() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        assert!(
            !map_data.brush_volumes.is_empty(),
            "should have brush volumes"
        );
        let total_sides: usize = map_data.brush_volumes.iter().map(|b| b.sides.len()).sum();
        assert!(total_sides > 0, "should have at least one brush side");
    }

    #[test]
    fn classifies_brushes_correctly() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        // info_player_start has 0 brushes
        assert!(
            map_data.entity_brushes.iter().all(|(_, count)| *count == 0),
            "info_player_start should have 0 brushes"
        );

        // Should have worldspawn + info_player_start
        let classnames: Vec<&str> = map_data
            .entities
            .iter()
            .map(|e| e.classname.as_str())
            .collect();
        assert!(classnames.contains(&"worldspawn"));
        assert!(classnames.contains(&"info_player_start"));
    }

    #[test]
    fn map_entities_collected_strip_reserved_keys_and_lights() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        // info_player_start is the only non-light, non-worldspawn point entity
        // in test.map.
        assert_eq!(
            map_data.map_entities.len(),
            1,
            "expected exactly one collected map entity, got {:?}",
            map_data
                .map_entities
                .iter()
                .map(|e| e.classname.as_str())
                .collect::<Vec<_>>()
        );

        let me = &map_data.map_entities[0];
        assert_eq!(me.classname, "info_player_start");
        // Reserved keys (`classname`, `origin`, `angle`/`angles`/`mangle`,
        // `_tags`) must not appear in the residual KVP bag.
        for (k, _) in &me.key_values {
            assert!(
                !["classname", "origin", "_tags", "angle", "angles", "mangle"]
                    .contains(&k.as_str()),
                "reserved key `{k}` leaked into key_values bag"
            );
        }
        // Lights must NOT appear in map_entities.
        assert!(
            map_data
                .map_entities
                .iter()
                .all(|e| !crate::format::quake_map::is_light_classname(&e.classname)),
            "light classname leaked into map_entities"
        );
    }

    #[test]
    fn brush_sides_have_valid_vertices() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        for (bi, brush) in map_data.brush_volumes.iter().enumerate() {
            for (si, side) in brush.sides.iter().enumerate() {
                assert!(
                    side.vertices.len() >= 3,
                    "brush {bi} side {si} should have at least 3 vertices, got {}",
                    side.vertices.len()
                );
            }
        }
    }

    #[test]
    fn brush_sides_have_unit_normals() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        for (bi, brush) in map_data.brush_volumes.iter().enumerate() {
            for (si, side) in brush.sides.iter().enumerate() {
                let len = side.normal.length();
                assert!(
                    (len - 1.0).abs() < 0.01,
                    "brush {bi} side {si} normal should be unit length, got {len}"
                );
            }
        }
    }

    #[test]
    fn extracts_player_start_origin() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        let player_start = map_data
            .entities
            .iter()
            .find(|e| e.classname == "info_player_start")
            .expect("should have info_player_start");

        let origin = player_start
            .origin
            .expect("info_player_start should have origin");
        assert!(origin.x.is_finite(), "origin x should be finite");
        assert!(origin.y.is_finite(), "origin y should be finite");
        assert!(origin.z.is_finite(), "origin z should be finite");
    }

    #[test]
    fn missing_file_returns_error() {
        let result = parse_map_file(Path::new("nonexistent.map"), MapFormat::IdTech2);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("failed to read"),
            "error should mention file reading, got: {msg}"
        );
    }

    /// Vertex winding contract: the first triangle's geometric normal (cross
    /// of the first two edges) must align with the stored side normal.
    /// Vertices appear CCW when viewed from the front, which is what
    /// `wgpu::FrontFace::Ccw` requires after upload.
    #[test]
    fn brush_side_winding_aligns_with_side_normal() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        let mut checked = 0usize;
        for (bi, brush) in map_data.brush_volumes.iter().enumerate() {
            for (si, side) in brush.sides.iter().enumerate() {
                if side.vertices.len() < 3 {
                    continue;
                }
                let v0 = side.vertices[0];
                let v1 = side.vertices[1];
                let v2 = side.vertices[2];
                let geometric_normal = (v1 - v0).cross(v2 - v0);

                if geometric_normal.length_squared() < 1e-10 {
                    continue;
                }

                let dot = geometric_normal.dot(side.normal);
                assert!(
                    dot > 0.0,
                    "brush {bi} side {si}: geometric normal {geometric_normal:?} \
                     is opposite to stored normal {:?} (dot={dot:.4}); winding is backwards",
                    side.normal
                );
                checked += 1;
            }
        }

        assert!(checked > 0, "no sides were checked — test is vacuous");
    }

    #[test]
    fn every_brush_volume_has_brush_sides() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        assert!(
            !map_data.brush_volumes.is_empty(),
            "test.map should produce brush volumes"
        );

        for (i, brush) in map_data.brush_volumes.iter().enumerate() {
            assert!(
                !brush.sides.is_empty(),
                "brush {i} has no sides; parser should emit a textured polygon per bounding plane"
            );
        }
    }

    // -- fog_lamp / fog_tube resolution --

    #[test]
    fn resolve_fog_lamp_requires_radius() {
        let props = HashMap::new();
        let err = resolve_fog_lamp(&props, DVec3::ZERO, "fog_lamp")
            .expect_err("missing radius must error");
        let msg = format!("{err}");
        assert!(msg.contains("radius"), "error should mention radius: {msg}");
    }

    #[test]
    fn resolve_fog_lamp_rejects_non_positive_radius() {
        let mut props = HashMap::new();
        props.insert("radius".to_string(), "0".to_string());
        let err =
            resolve_fog_lamp(&props, DVec3::ZERO, "fog_lamp").expect_err("zero radius must error");
        assert!(format!("{err}").contains("positive"));

        let mut props = HashMap::new();
        props.insert("radius".to_string(), "-1".to_string());
        let err = resolve_fog_lamp(&props, DVec3::ZERO, "fog_lamp")
            .expect_err("negative radius must error");
        assert!(format!("{err}").contains("positive"));
    }

    #[test]
    fn resolve_fog_lamp_produces_centered_aabb_and_no_planes() {
        let mut props = HashMap::new();
        props.insert("radius".to_string(), "2.5".to_string());
        let v = resolve_fog_lamp(&props, DVec3::new(1.0, 2.0, 3.0), "fog_lamp")
            .expect("valid radius should resolve");
        assert_eq!(v.min, [-1.5, -0.5, 0.5]);
        assert_eq!(v.max, [3.5, 4.5, 5.5]);
        assert!(
            v.planes.is_empty(),
            "fog_lamp is a semantic AABB; no planes"
        );
        assert_eq!(v.edge_softness, 0.0, "semantic entity uses radial_falloff");
    }

    #[test]
    fn resolve_fog_tube_oriented_aabb_inflates_with_pitch_and_yaw() {
        // Capsule: radius 1, height 4. Local axis is +Y; with pitch=0/yaw=0 the
        // axis stays vertical, so the AABB is [-1, -2, -1] – [1, 2, 1].
        let mut props = HashMap::new();
        props.insert("radius".to_string(), "1".to_string());
        props.insert("height".to_string(), "4".to_string());
        let v = resolve_fog_tube(&props, DVec3::ZERO, "fog_tube").expect("axis-aligned tube");
        assert_eq!(v.min, [-1.0, -2.0, -1.0]);
        assert_eq!(v.max, [1.0, 2.0, 1.0]);

        // Pitch 90° tilts the axis fully into the horizontal plane (pure -Z).
        // The half-segment now extends along Z, and Y collapses to just the
        // capsule radius. Pitch of 90° with yaw=0 → axis = (0, 0, -1).
        let mut props = HashMap::new();
        props.insert("radius".to_string(), "1".to_string());
        props.insert("height".to_string(), "4".to_string());
        props.insert("pitch".to_string(), "90".to_string());
        let v = resolve_fog_tube(&props, DVec3::ZERO, "fog_tube").expect("tilted tube");
        // half_segment = max(2 - 1, 0) = 1; axis ≈ (0, 0, -1).
        // half_extent_x = 0*1 + 1 = 1; y = 0*1 + 1 = 1; z = 1*1 + 1 = 2.
        assert!((v.min[0] - -1.0).abs() < 1e-5);
        assert!((v.min[1] - -1.0).abs() < 1e-5);
        assert!((v.min[2] - -2.0).abs() < 1e-5);
        assert!((v.max[0] - 1.0).abs() < 1e-5);
        assert!((v.max[1] - 1.0).abs() < 1e-5);
        assert!((v.max[2] - 2.0).abs() < 1e-5);

        // Yaw 90° rotates the (already pitched) axis around Y; with pitch=90 yaw=90
        // the axis becomes (-1, 0, 0) so the long extent moves to X.
        let mut props = HashMap::new();
        props.insert("radius".to_string(), "1".to_string());
        props.insert("height".to_string(), "4".to_string());
        props.insert("pitch".to_string(), "90".to_string());
        props.insert("yaw".to_string(), "90".to_string());
        let v = resolve_fog_tube(&props, DVec3::ZERO, "fog_tube").expect("yawed tube");
        assert!((v.min[0] - -2.0).abs() < 1e-5);
        assert!((v.min[1] - -1.0).abs() < 1e-5);
        assert!((v.min[2] - -1.0).abs() < 1e-5);
        assert!((v.max[0] - 2.0).abs() < 1e-5);
        assert!((v.max[1] - 1.0).abs() < 1e-5);
        assert!((v.max[2] - 1.0).abs() < 1e-5);

        assert!(
            v.planes.is_empty(),
            "fog_tube is a semantic AABB; no planes"
        );
    }

    #[test]
    fn resolve_fog_tube_requires_radius_and_height() {
        let mut props = HashMap::new();
        props.insert("height".to_string(), "4".to_string());
        let err = resolve_fog_tube(&props, DVec3::ZERO, "fog_tube")
            .expect_err("missing radius must error");
        assert!(format!("{err}").contains("radius"));

        let mut props = HashMap::new();
        props.insert("radius".to_string(), "1".to_string());
        let err = resolve_fog_tube(&props, DVec3::ZERO, "fog_tube")
            .expect_err("missing height must error");
        assert!(format!("{err}").contains("height"));
    }
}
