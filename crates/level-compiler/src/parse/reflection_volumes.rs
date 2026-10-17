//! Quake translation of the `reflection_volume` brush entity: an authored
//! reflection-probe box and its optional horizontal capture offset.
//! See: context/lib/build_pipeline.md §Source-format neutrality · resource_management.md §5.1

use std::collections::HashMap;

use anyhow::Result;
use glam::DVec3;
use shambler::GeoMap;
use shambler::brush::BrushId;

use crate::map_data::MapReflectionVolume;

use super::quake_to_engine;

pub(super) const REFLECTION_VOLUME: &str = "reflection_volume";

/// Mapper-facing reminder of the offset's shape, appended to its errors.
const OFFSET_SHAPE: &str = "`_capture_offset` is two numbers on the map's horizontal axes, in \
                            map units (\"32 -16\"); the capture point's height is fixed";

/// Translate one `reflection_volume` into the canonical record. Its AABB
/// spans every brush; the offset converts to engine axes and meters. Every
/// failure names the entity by its map entity index and classname.
pub(super) fn resolve_reflection_volume(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    props: &HashMap<String, String>,
    scale: f64,
    entity_index: usize,
) -> Result<MapReflectionVolume> {
    let source_label = format!("entity {entity_index} '{REFLECTION_VOLUME}'");
    let source_entity = u32::try_from(entity_index)
        .map_err(|_| anyhow::anyhow!("{source_label} index exceeds the PRL u32 entity field"))?;
    if brush_ids.is_empty() {
        anyhow::bail!(
            "{source_label} has no brushes — a reflection volume is brushwork; its brushes' bounds \
             are the probe's box"
        );
    }
    let capture_offset = parse_capture_offset(props, &source_label, scale)?;
    let (min, max) = crate::trigger_volumes::resolve_brush_entity_aabb(
        geo_map,
        brush_ids,
        scale,
        REFLECTION_VOLUME,
        &source_label,
    )?;
    let min = min.as_vec3().to_array();
    let max = max.as_vec3().to_array();
    if min.iter().chain(&max).any(|value| !value.is_finite()) {
        anyhow::bail!("{source_label} brush bounds exceed engine precision");
    }
    Ok(MapReflectionVolume {
        source_entity,
        source_label,
        min,
        max,
        capture_offset,
    })
}

/// Optional; blank is unset. Two finite numbers on the map's horizontal axes
/// (Quake x, y), a length in map units. A third number would be vertical: the
/// capture point stays 2 mm above the water, so that is an error too.
fn parse_capture_offset(
    props: &HashMap<String, String>,
    entity: &str,
    scale: f64,
) -> Result<Option<[f32; 2]>> {
    let Some(raw) = props
        .get("_capture_offset")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let numbers = raw
        .split_whitespace()
        .map(|token| match token.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(value),
            _ => Err(anyhow::anyhow!(
                "{entity} `_capture_offset` \"{raw}\" holds `{token}`, which is not a finite \
                 number; {OFFSET_SHAPE}"
            )),
        })
        .collect::<Result<Vec<f64>>>()?;
    let [x, y] = numbers.as_slice() else {
        if numbers.len() == 3 {
            anyhow::bail!(
                "{entity} `_capture_offset` \"{raw}\" has a third, vertical number; {OFFSET_SHAPE}"
            );
        }
        anyhow::bail!(
            "{entity} `_capture_offset` \"{raw}\" has {} number(s); {OFFSET_SHAPE}",
            numbers.len()
        );
    };
    // A length: swizzle the axes, then scale to meters.
    let engine = quake_to_engine(DVec3::new(*x, *y, 0.0)) * scale;
    let offset = [engine.x as f32, engine.z as f32];
    if offset.iter().any(|value| !value.is_finite()) {
        anyhow::bail!("{entity} `_capture_offset` \"{raw}\" exceeds engine precision");
    }
    Ok(Some(offset))
}
