//! Quake translation of the purpose-named environment-volume brush entities.
//! See: context/lib/build_pipeline.md §Source-format neutrality

use std::collections::HashMap;

use anyhow::Result;
use glam::{DVec3, Vec3};
use shambler::GeoMap;
use shambler::brush::BrushId;

use crate::map_data::{BrushSide, MapEnvironmentBrush, MapEnvironmentVolume};

use super::authoring_regions::resolve_brush_region_bounds_with_empty_policy;
use super::quake_to_engine;

/// The authored field each purpose-named entity sets. The split ends here:
/// all three become one canonical [`MapEnvironmentVolume`].
const FLUID_VOLUME: &str = "fluid_volume";
const GRAVITY_VOLUME: &str = "gravity_volume";
const PUSH_VOLUME: &str = "push_volume";

/// Mapper-facing reminder of each field's shape, appended to shape errors.
const GRAVITY_SHAPE: &str = "`gravity` is one signed vertical number in m/s², like worldspawn \
                             `initialGravity` (\"-3\" is low gravity, \"0\" is weightless)";
const PUSH_SHAPE: &str = "`push` is three numbers, an acceleration in map axes in m/s² \
                          (\"0 0 15\" is an updraft, \"8 0 0\" a wind)";

pub(super) fn is_environment_volume_classname(classname: &str) -> bool {
    matches!(classname, FLUID_VOLUME | GRAVITY_VOLUME | PUSH_VOLUME)
}

/// Translate one `fluid_volume`, `gravity_volume` or `push_volume` into the
/// canonical record. Every failure names the entity by its map entity index
/// and classname, the label buried-light diagnostics use.
pub(super) fn resolve_environment_volume(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    props: &HashMap<String, String>,
    scale: f64,
    classname: &str,
    entity_index: usize,
) -> Result<MapEnvironmentVolume> {
    let entity = format!("entity {entity_index} '{classname}'");
    let source_entity = u32::try_from(entity_index)
        .map_err(|_| anyhow::anyhow!("{entity} index exceeds the PRL u32 entity field"))?;

    let (gravity, push, fluid) = match classname {
        GRAVITY_VOLUME => (Some(parse_gravity(props, &entity)?), None, None),
        PUSH_VOLUME => (None, Some(parse_push(props, &entity)?), None),
        FLUID_VOLUME => (None, None, Some(parse_fluid(props, &entity)?)),
        _ => anyhow::bail!("{entity} is not an environment volume"),
    };
    let priority = parse_priority(props, &entity)?;

    if brush_ids.is_empty() {
        anyhow::bail!(
            "{entity} has no brushes — an environment volume is brushwork; author it as one or \
             more brushes"
        );
    }
    let brushes = brush_ids
        .iter()
        .enumerate()
        .map(|(brush_number, brush_id)| {
            resolve_leaf(geo_map, *brush_id, scale, classname, &entity, brush_number)
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(MapEnvironmentVolume {
        source_entity,
        priority,
        gravity,
        push,
        fluid,
        brushes,
    })
}

/// An authored value, or `None` when the key is missing or blank. A required
/// field fails on either: TrenchBroom writes `""` for a cleared field, and a
/// volume whose one field is unset has nothing to say.
fn required<'a>(props: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    props
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// Gravity is a signed vertical scalar. Quake +Z and engine +Y are both up,
/// so the authored sign carries over with no axis conversion.
fn parse_gravity(props: &HashMap<String, String>, entity: &str) -> Result<f32> {
    let raw = required(props, "gravity")
        .ok_or_else(|| anyhow::anyhow!("{entity} needs a non-blank `gravity`; {GRAVITY_SHAPE}"))?;
    let numbers = parse_numbers(raw, "gravity", entity, GRAVITY_SHAPE)?;
    match numbers.as_slice() {
        [gravity] => Ok(*gravity),
        _ => anyhow::bail!(
            "{entity} `gravity` \"{raw}\" has {}; {GRAVITY_SHAPE}",
            number_count(numbers.len())
        ),
    }
}

/// Push is an acceleration authored in map axes, m/s². Only the axis basis
/// changes on the way to engine axes: the map-unit scale converts authored
/// lengths to meters, and a push is already in meters per second squared, so
/// scaling it would shrink an 8 m/s² wind to 0.2 m/s². It converts like a
/// plane normal — `quake_to_engine` alone.
fn parse_push(props: &HashMap<String, String>, entity: &str) -> Result<Vec3> {
    let raw = required(props, "push")
        .ok_or_else(|| anyhow::anyhow!("{entity} needs a non-blank `push`; {PUSH_SHAPE}"))?;
    let numbers = parse_numbers(raw, "push", entity, PUSH_SHAPE)?;
    let [x, y, z] = numbers.as_slice() else {
        anyhow::bail!(
            "{entity} `push` \"{raw}\" has {}; {PUSH_SHAPE}",
            number_count(numbers.len())
        );
    };
    let map_axes = DVec3::new(f64::from(*x), f64::from(*y), f64::from(*z));
    Ok(quake_to_engine(map_axes).as_vec3())
}

fn parse_fluid(props: &HashMap<String, String>, entity: &str) -> Result<String> {
    required(props, "fluid").map(str::to_owned).ok_or_else(|| {
        anyhow::anyhow!(
            "{entity} needs a non-blank `fluid` naming a fluid the mod manifest declares"
        )
    })
}

/// Optional integer, default 0. A cleared field is unset, like every other
/// optional key.
fn parse_priority(props: &HashMap<String, String>, entity: &str) -> Result<i32> {
    let Some(raw) = required(props, "priority") else {
        return Ok(0);
    };
    raw.parse::<i32>().map_err(|error| {
        anyhow::anyhow!("{entity} `priority` \"{raw}\" is not an integer ({error})")
    })
}

/// "1 number", "3 numbers".
fn number_count(count: usize) -> String {
    if count == 1 {
        "1 number".to_string()
    } else {
        format!("{count} numbers")
    }
}

fn parse_numbers(raw: &str, key: &str, entity: &str, shape: &str) -> Result<Vec<f32>> {
    raw.split_whitespace()
        .map(|token| match token.parse::<f32>() {
            Ok(value) if value.is_finite() => Ok(value),
            _ => Err(anyhow::anyhow!(
                "{entity} `{key}` \"{raw}\" holds `{token}`, which is not a finite number; {shape}"
            )),
        })
        .collect()
}

/// One authored brush as one convex leaf. A brush with no usable hull fails
/// the build, following `trigger_volume`'s posture: an invisible region the
/// compiler cannot resolve must not silently vanish.
fn resolve_leaf(
    geo_map: &GeoMap,
    brush_id: BrushId,
    scale: f64,
    classname: &str,
    entity: &str,
    brush_number: usize,
) -> Result<MapEnvironmentBrush> {
    let brush = format!("{entity} brush {brush_number}");
    let bounds = resolve_brush_region_bounds_with_empty_policy(
        geo_map,
        &[brush_id],
        scale,
        classname,
        false,
    )?
    .ok_or_else(|| anyhow::anyhow!("{brush} has no usable vertices"))?;
    if (bounds.max - bounds.min).min_element() <= 0.0 {
        anyhow::bail!("{brush} has zero volume");
    }
    if bounds.planes.len() < 4 {
        anyhow::bail!(
            "{brush} has {} bounding planes; a convex leaf needs at least four",
            bounds.planes.len()
        );
    }
    let min = bounds.min.as_vec3().to_array();
    let max = bounds.max.as_vec3().to_array();
    let finite = min.iter().chain(&max).all(|value| value.is_finite())
        && bounds
            .planes
            .iter()
            .flatten()
            .all(|value| value.is_finite());
    if !finite {
        anyhow::bail!("{brush} hull exceeds engine precision");
    }
    Ok(MapEnvironmentBrush {
        min,
        max,
        planes: bounds.planes,
        sides: Vec::new(),
    })
}

/// Give every fluid volume's brushes their textured sides, the surface
/// stage's input. `brush_ids[i]` holds volume `i`'s authored brushes, in the
/// order its leaves were resolved. One hull build covers every fluid brush:
/// the build walks the whole map's face planes, so a call per entity would
/// repeat that walk once per pool.
pub(super) fn attach_fluid_surface_sides(
    geo_map: &GeoMap,
    volumes: &mut [MapEnvironmentVolume],
    brush_ids: &[Vec<BrushId>],
    scale: f64,
) {
    debug_assert_eq!(volumes.len(), brush_ids.len());
    let fluid_brush_ids: Vec<BrushId> = volumes
        .iter()
        .zip(brush_ids)
        .filter(|(volume, _)| volume.fluid.is_some())
        .flat_map(|(_, ids)| ids.iter().copied())
        .collect();
    if fluid_brush_ids.is_empty() {
        return;
    }
    let mut sides_by_brush: HashMap<BrushId, Vec<BrushSide>> =
        super::build_brush_volumes_with_ids(geo_map, &fluid_brush_ids, scale)
            .into_iter()
            .map(|(brush_id, hull)| (brush_id, hull.sides))
            .collect();
    for (volume, ids) in volumes.iter_mut().zip(brush_ids) {
        if volume.fluid.is_none() {
            continue;
        }
        // `resolve_leaf` already failed the build on a brush without a hull,
        // so every id has sides here.
        for (leaf, brush_id) in volume.brushes.iter_mut().zip(ids) {
            leaf.sides = sides_by_brush.remove(brush_id).unwrap_or_default();
        }
    }
}
