//! Quake translation of the `emissive_volume` brush entity: the glow range and
//! tint it gives the static world faces inside it, and one convex hull per brush.
//! See: context/lib/build_pipeline.md §Custom FGD · resource_management.md §4.5

use std::collections::HashMap;

use anyhow::Result;
use glam::DVec3;
use shambler::GeoMap;
use shambler::brush::BrushId;

use postretro_level_format::emissive::{EMISSIVE_VALUE_CEILING, MAX_EMISSIVE_VOLUMES};

use crate::map_data::{BrushVolume, MapEmissiveHull, MapEmissiveVolume};

pub(super) const EMISSIVE_VOLUME: &str = "emissive_volume";

const MIN_KEY: &str = "emissive_min";
const MAX_KEY: &str = "emissive_max";
const COLOR_KEY: &str = "emissive_color";
const DEFAULT_MIN: f32 = 0.0;
const DEFAULT_MAX: f32 = 1.0;
const WHITE: [f32; 3] = [1.0; 3];

/// One `emissive_volume` read during the entity walk, its keys already fixed;
/// its hulls resolve after the walk, in one hull build for every volume.
pub(super) struct PendingEmissiveVolume {
    entity_index: usize,
    label: String,
    min: f32,
    max: f32,
    color: [f32; 3],
    tags: Vec<String>,
    brush_ids: Vec<BrushId>,
}

/// Read one entity's keys. Bad values never fail the compile: each key is
/// fixed on its own first (unparseable or non-finite → its default, negative
/// → 0, past the ceiling → the ceiling), each fix warning once by key name;
/// then a max below min swaps, with a warning. Fixing first keeps the stored
/// min at or below max: swapping `(2, NaN)` first would store `(2, 1)`.
pub(super) fn read_emissive_volume(
    props: &HashMap<String, String>,
    entity_index: usize,
    brush_ids: Vec<BrushId>,
) -> Result<PendingEmissiveVolume> {
    let label = format!("entity {entity_index} '{EMISSIVE_VOLUME}'");
    if brush_ids.is_empty() {
        anyhow::bail!(
            "{label} has no brushes — an emissive volume is brushwork; the faces inside its \
             brushes take its glow"
        );
    }
    let mut min = read_multiplier(props, MIN_KEY, DEFAULT_MIN, &label);
    let mut max = read_multiplier(props, MAX_KEY, DEFAULT_MAX, &label);
    if max < min {
        if authored(props, MAX_KEY).is_some() {
            log::warn!(
                "[Compiler] {label} `{MAX_KEY}` {max} is below `{MIN_KEY}` {min}; the two are \
                 swapped"
            );
        } else {
            log::warn!(
                "[Compiler] {label} `{MAX_KEY}` is not authored, so its default {max} is below \
                 the authored `{MIN_KEY}` {min}; the two are swapped"
            );
        }
        std::mem::swap(&mut min, &mut max);
    }
    let color = read_color(props, &label);
    let tags = props
        .get("_tags")
        .map(|value| value.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    Ok(PendingEmissiveVolume {
        entity_index,
        label,
        min,
        max,
        color,
        tags,
        brush_ids,
    })
}

/// A cleared TrenchBroom field arrives as `""`: unset, like an absent key.
fn authored<'a>(props: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    props
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

fn read_multiplier(props: &HashMap<String, String>, key: &str, default: f32, label: &str) -> f32 {
    let Some(raw) = authored(props, key) else {
        return default;
    };
    // f64, so a finite number past f32's range still reads as past the ceiling.
    let Ok(mut value) = raw.parse::<f64>() else {
        log::warn!(
            "[Compiler] {label} `{key}` \"{raw}\" is not a number; it reads its default {default}"
        );
        return default;
    };
    // A decimal that overflows f64 is finite and past the ceiling; only the
    // `inf` keyword is non-finite.
    if value.is_infinite() && raw.bytes().any(|byte| byte.is_ascii_digit()) {
        value = value.signum() * f64::MAX;
    }
    fix_multiplier(value, key, default, label)
}

/// One key's fix, in the order the Decision gives: non-finite → default,
/// negative → 0, past the ceiling → the ceiling.
fn fix_multiplier(value: f64, key: &str, default: f32, label: &str) -> f32 {
    if !value.is_finite() {
        log::warn!(
            "[Compiler] {label} `{key}` {value} is not finite; it reads its default {default}"
        );
        return default;
    }
    if value < 0.0 {
        log::warn!("[Compiler] {label} `{key}` {value} is negative; it reads 0");
        return 0.0;
    }
    if value > f64::from(EMISSIVE_VALUE_CEILING) {
        log::warn!(
            "[Compiler] {label} `{key}` {value} is past the ceiling; it reads \
             {EMISSIVE_VALUE_CEILING}"
        );
        return EMISSIVE_VALUE_CEILING;
    }
    // `-0` reads as 0, so both spellings write the same bits.
    if value == 0.0 { 0.0 } else { value as f32 }
}

/// Read as every FGD colour key is: three integers 0..=255, each ÷ 255, used
/// as linear. Malformed warns and reads white.
fn read_color(props: &HashMap<String, String>, label: &str) -> [f32; 3] {
    let Some(raw) = authored(props, COLOR_KEY) else {
        return WHITE;
    };
    super::parse_fog_tint(raw).unwrap_or_else(|| {
        log::warn!(
            "[Compiler] {label} `{COLOR_KEY}` \"{raw}\" is not three integers 0 to 255; it reads \
             white"
        );
        WHITE
    })
}

/// Give every volume its hulls, from one hull build over all their brushes
/// (the build walks the whole map's face planes, so a call per brush would
/// repeat that walk). A brush with no usable hull fails the build, as an
/// environment volume's does: an invisible region the compiler cannot
/// resolve must not silently vanish. More than [`MAX_EMISSIVE_VOLUMES`]
/// fails too, naming the authored count.
pub(super) fn resolve_emissive_volumes(
    geo_map: &GeoMap,
    pending: Vec<PendingEmissiveVolume>,
    scale: f64,
) -> Result<Vec<MapEmissiveVolume>> {
    if pending.len() > MAX_EMISSIVE_VOLUMES {
        anyhow::bail!(
            "the map has {} {EMISSIVE_VOLUME} entities; a map holds at most \
             {MAX_EMISSIVE_VOLUMES} — merge volumes that share a range and colour",
            pending.len()
        );
    }
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let brush_ids: Vec<BrushId> = pending
        .iter()
        .flat_map(|volume| volume.brush_ids.iter().copied())
        .collect();
    let mut hulls: HashMap<BrushId, BrushVolume> =
        super::build_brush_volumes_with_ids(geo_map, &brush_ids, scale)
            .into_iter()
            .collect();
    pending
        .into_iter()
        .map(|volume| {
            let source_entity = u32::try_from(volume.entity_index).map_err(|_| {
                anyhow::anyhow!("{} index exceeds the PRL u32 entity field", volume.label)
            })?;
            let mut resolved = Vec::with_capacity(volume.brush_ids.len());
            let mut summed_volume = 0.0;
            let (mut lo, mut hi) = (DVec3::INFINITY, DVec3::NEG_INFINITY);
            for (brush_number, brush_id) in volume.brush_ids.iter().enumerate() {
                let brush = format!("{} brush {brush_number}", volume.label);
                let hull = hulls
                    .remove(brush_id)
                    .ok_or_else(|| anyhow::anyhow!("{brush} has no usable hull"))?;
                let size = hull_volume(&hull);
                if !size.is_finite() || size <= 0.0 {
                    anyhow::bail!("{brush} has zero volume");
                }
                summed_volume += size;
                let (min, max) = (hull.aabb.min, hull.aabb.max);
                lo = lo.min(min);
                hi = hi.max(max);
                resolved.push(MapEmissiveHull {
                    min,
                    max,
                    planes: hull
                        .planes
                        .iter()
                        .map(|plane| (plane.normal, plane.distance))
                        .collect(),
                });
            }
            let origin = ((lo + hi) * 0.5).as_vec3().to_array();
            if origin.iter().any(|value| !value.is_finite()) {
                anyhow::bail!("{} brush bounds exceed engine precision", volume.label);
            }
            Ok(MapEmissiveVolume {
                source_entity,
                source_label: volume.label,
                min: volume.min,
                max: volume.max,
                color: volume.color,
                hulls: resolved,
                summed_volume,
                origin,
                tags: volume.tags,
            })
        })
        .collect()
}

/// A convex brush's volume, m³: each side's polygon area times its
/// distance from an interior point, over three.
fn hull_volume(hull: &BrushVolume) -> f64 {
    let vertex_count: usize = hull.sides.iter().map(|side| side.vertices.len()).sum();
    if vertex_count == 0 {
        return 0.0;
    }
    let inside = hull
        .sides
        .iter()
        .flat_map(|side| side.vertices.iter().copied())
        .sum::<DVec3>()
        / vertex_count as f64;
    hull.sides
        .iter()
        .map(|side| {
            let Some(&first) = side.vertices.first() else {
                return 0.0;
            };
            let twice_area: DVec3 = side
                .vertices
                .windows(2)
                .skip(1)
                .map(|pair| (pair[0] - first).cross(pair[1] - first))
                .sum();
            let area = twice_area.dot(side.normal).abs() * 0.5;
            let height = side.distance - side.normal.dot(inside);
            area * height / 3.0
        })
        .sum()
}

#[cfg(test)]
#[path = "emissive_volume_tests.rs"]
mod tests;
