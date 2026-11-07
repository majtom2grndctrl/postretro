// NavMesh stage: the whole-section cache lookup and bake.
// See: context/lib/build_pipeline.md §Build Cache

use glam::DVec3;
use postretro_level_format::navmesh::NavMeshSection;

use super::{NAVMESH_STAGE_VERSION, bake_navmesh_with_movers};
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;
use crate::map_data::{BrushVolume, MapKinematicMover, MapKinematicWaypoint, NavParams};

/// The navmesh for these inputs, from the cache when its key hits. `None` when
/// no walkable region survives.
pub fn bake_or_load_navmesh(
    geo: &GeometryResult,
    params: &NavParams,
    movers: &[MapKinematicMover],
    waypoints: &[MapKinematicWaypoint],
    stage_cache: Option<&StageCache>,
) -> Option<NavMeshSection> {
    let key = stage_key(geo, params, movers, waypoints);

    // Cache stores the section's `to_bytes()`; an empty payload is the sentinel
    // for a cached "no walkable region" result (no section).
    match stage_cache.and_then(|cache| cache.get(&key)) {
        Some(bytes) if bytes.is_empty() => {
            log::info!("[cache] navmesh hit (no walkable region)");
            return None;
        }
        Some(bytes) => match NavMeshSection::from_bytes(&bytes) {
            Ok(section) => {
                log::info!("[cache] navmesh hit");
                return Some(section);
            }
            Err(e) => log::warn!("[cache] corrupt navmesh entry, re-baking: {e}"),
        },
        None => log::info!("[cache] navmesh miss"),
    }

    let section = bake_navmesh_with_movers(geo, params, movers, waypoints);
    if let Some(cache) = stage_cache {
        cache.put(
            &key,
            &section.as_ref().map(|s| s.to_bytes()).unwrap_or_default(),
        );
    }
    section
}

/// The stage key over every bake input: geometry, nav parameters, and each
/// mover's identity, origin, path, spin axis and brushes, plus the waypoints
/// its path walks. Only inputs the bake reads go in, so a speed or sound edit
/// still hits.
pub(super) fn stage_key(
    geo: &GeometryResult,
    params: &NavParams,
    movers: &[MapKinematicMover],
    waypoints: &[MapKinematicWaypoint],
) -> CacheKey {
    let mut buf = postcard::to_allocvec(geo).expect("postcard serialize geo_result");
    buf.extend_from_slice(&postcard::to_allocvec(params).expect("postcard serialize nav params"));
    fold_mover_inputs(&mut buf, movers, waypoints);
    let input_hash = blake3::hash(&buf);
    CacheKey::new("navmesh", NAVMESH_STAGE_VERSION, input_hash.as_bytes())
}

fn fold_mover_inputs(
    buf: &mut Vec<u8>,
    movers: &[MapKinematicMover],
    waypoints: &[MapKinematicWaypoint],
) {
    fold_len(buf, movers.len());
    for mover in movers {
        buf.extend_from_slice(&mover.mover_id.to_le_bytes());
        fold_point(buf, mover.origin);
        fold_str(buf, &mover.path);
        for component in mover.spin_axis {
            buf.extend_from_slice(&component.to_le_bytes());
        }
        fold_len(buf, mover.brush_volumes.len());
        for brush in &mover.brush_volumes {
            fold_brush(buf, brush);
        }
    }
    fold_len(buf, waypoints.len());
    for waypoint in waypoints {
        fold_str(buf, &waypoint.name);
        fold_str(buf, &waypoint.next);
        fold_point(buf, waypoint.origin);
    }
}

/// Planes and bounds feed the gate test, sides the link test. Textures stay out:
/// a retexture moves no walkable surface.
fn fold_brush(buf: &mut Vec<u8>, brush: &BrushVolume) {
    fold_len(buf, brush.planes.len());
    for plane in &brush.planes {
        fold_point(buf, plane.normal);
        buf.extend_from_slice(&plane.distance.to_le_bytes());
    }
    fold_point(buf, brush.aabb.min);
    fold_point(buf, brush.aabb.max);
    fold_len(buf, brush.sides.len());
    for side in &brush.sides {
        fold_point(buf, side.normal);
        fold_len(buf, side.vertices.len());
        for &vertex in &side.vertices {
            fold_point(buf, vertex);
        }
    }
}

fn fold_len(buf: &mut Vec<u8>, len: usize) {
    buf.extend_from_slice(&(len as u64).to_le_bytes());
}

fn fold_str(buf: &mut Vec<u8>, text: &str) {
    fold_len(buf, text.len());
    buf.extend_from_slice(text.as_bytes());
}

fn fold_point(buf: &mut Vec<u8>, point: DVec3) {
    for component in point.to_array() {
        buf.extend_from_slice(&component.to_le_bytes());
    }
}
