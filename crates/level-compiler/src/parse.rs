// .map file parsing via shambler: brush classification and face extraction.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::collections::{BTreeMap, HashMap, HashSet};
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
    BrushPlane, BrushSide, BrushVolume, EntityInfo, EntityShadowParams, KinematicMoveMode,
    LightType, MapAssembly, MapData, MapEntityRecord, MapFogVolume, MapKinematicMover,
    MapKinematicWaypoint, MapLight, MapLightmapScaleRegion, MapStreamingHintRegion,
    MapStreamingPriorityRegion, MapTriggerVolume, NavParams, TextureProjection,
};
use crate::map_format::MapFormat;
use crate::mover_light_gate::MAX_POSES;
use crate::surface_radiance;
use postretro_level_format::environment_volumes::DEFAULT_WORLD_GRAVITY_MPS2;
use postretro_level_format::fog_volumes::{MAX_FOG_VOLUMES, MAX_PLANES_PER_VOLUME};
use postretro_level_format::kinematic_geometry::KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH;

#[path = "parse/attack_pools.rs"]
mod attack_pools;
#[path = "parse/authoring_regions.rs"]
mod authoring_regions;
#[cfg(test)]
#[path = "parse/environment_volume_tests.rs"]
mod environment_volume_tests;
#[path = "parse/environment_volumes.rs"]
mod environment_volumes;
#[path = "parse/fog_volumes.rs"]
mod fog_volumes;
#[path = "parse/ghost_links.rs"]
mod ghost_links;
#[path = "parse/map_source.rs"]
mod map_source;
#[path = "parse/reflection_capture.rs"]
mod reflection_capture;
#[cfg(test)]
#[path = "parse/reflection_capture_tests.rs"]
mod reflection_capture_tests;
#[cfg(test)]
#[path = "parse/reflection_volume_tests.rs"]
mod reflection_volume_tests;
#[path = "parse/reflection_volumes.rs"]
mod reflection_volumes;
#[path = "parse/streaming_hint_source.rs"]
mod streaming_hint_source;
#[cfg(test)]
#[path = "parse/streaming_hint_tests.rs"]
mod streaming_hint_tests;
#[path = "parse/teleporters.rs"]
mod teleporters;

use authoring_regions::{
    BrushRegionBounds, resolve_brush_region_bounds, resolve_lightmap_scale_region,
    resolve_sh_protect_aabb, resolve_streaming_hint_region, resolve_streaming_priority_region,
};
use fog_volumes::{resolve_fog_ellipsoid, resolve_fog_lamp, resolve_fog_tube, resolve_fog_volume};
use map_source::{normalize_map_source, parse_complete_map};
use streaming_hint_source::reject_invalid_streaming_hint_source_hulls;

/// Convert a shambler nalgebra Vector3 to glam DVec3.
///
/// This is the **input precision boundary**: shambler stores coordinates as f32
/// (parsed from the .map text), and we widen them to f64 here. All subsequent
/// compile-time geometry is computed in double precision.
pub(crate) fn shambler_to_dvec3(v: &shambler::Vector3) -> DVec3 {
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
pub(crate) fn quake_to_engine(v: DVec3) -> DVec3 {
    DVec3::new(-v.y, v.z, -v.x)
}

/// Parse an origin string like "-192 25.6 167.736" into a DVec3.
///
/// Parses directly to f64 — no precision cast from f32.
pub(super) fn parse_origin(s: &str) -> Option<DVec3> {
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

#[derive(Debug)]
struct PendingKinematicMover {
    name: String,
    tags: Vec<String>,
    authored_origin: Option<DVec3>,
    path: String,
    speed: f32,
    wait_ms: f32,
    spin_axis: [f32; 3],
    spin_speed_deg_s: f32,
    spin_accel_deg_s2: f32,
    carry_yaw: bool,
    block_policy: String,
    crush_damage: f32,
    crush_interval_ms: f32,
    auto_close_ms: Option<f32>,
    open_event: Option<String>,
    close_event: Option<String>,
    blocked_event: Option<String>,
    crush_event: Option<String>,
    open_sound: Option<String>,
    close_sound: Option<String>,
    blocked_sound: Option<String>,
    crush_sound: Option<String>,
    crush_damage_type: Option<String>,
    light_ramp_start: f32,
    light_ramp_end: f32,
    light_bounces: Option<u32>,
    light_poses: Option<u32>,
    move_mode: KinematicMoveMode,
    start_on_spawn: bool,
    brush_ids: Vec<BrushId>,
}

/// Transient indices of sibling members recognized by the format adapter.
///
/// Assemblies themselves remain compiler-shaped identity/provenance records;
/// this map exists only while the parser has indices into its typed outputs.
/// A later assembly consumer can use it before those temporary parse vectors
/// are resolved without letting `_tb_group` enter a generic runtime KVP bag.
#[derive(Debug, Default)]
struct AssemblyMembers {
    light_indices: Vec<usize>,
    pending_mover_indices: Vec<usize>,
    map_entity_indices: Vec<usize>,
    trigger_indices: Vec<usize>,
}

impl AssemblyMembers {
    #[cfg(debug_assertions)]
    fn indices_are_in_bounds(
        &self,
        light_count: usize,
        pending_mover_count: usize,
        map_entity_count: usize,
        trigger_count: usize,
    ) -> bool {
        self.light_indices.iter().all(|&index| index < light_count)
            && self
                .pending_mover_indices
                .iter()
                .all(|&index| index < pending_mover_count)
            && self
                .map_entity_indices
                .iter()
                .all(|&index| index < map_entity_count)
            && self
                .trigger_indices
                .iter()
                .all(|&index| index < trigger_count)
    }
}

/// Sentinel standing in for a space inside an encoded brush-face material name.
/// U+0001 (Start of Heading) is a non-printable control byte: it never occurs
/// in a `.map` material token or a real filesystem path, it is space-free (so
/// shalrath's `take_until(" ")` reads the whole name as one token), and it is
/// non-quote and non-newline (so it survives shalrath's tokenizer). A
/// path-illegal sentinel round-trips unambiguously — unlike a printable
/// stand-in such as `%20`, it cannot collide with a literal substring an author
/// put in a real material name.
const SPACE_SENTINEL: char = '\u{1}';

/// String form of [`SPACE_SENTINEL`], for use as a replacement target.
const SPACE_SENTINEL_STR: &str = "\u{1}";

/// Encode spaces inside quoted brush-face material fields so shalrath's
/// brush-plane grammar can tokenize them.
///
/// TrenchBroom (idTech2 output) double-quotes a face's material name only when
/// it contains a space. shalrath has no quote handling, so we rewrite each
/// affected line: the quotes are stripped and interior spaces become
/// [`SPACE_SENTINEL`], yielding a single space-free token that shalrath stores
/// verbatim. The token is decoded back to its original spelling by
/// [`decode_brush_texture`] at the texture-read boundary.
///
/// Only brush-plane lines are touched. A brush-plane line begins with `(` (its
/// first point triple); entity key/value lines begin with `"`. The point
/// triples never contain quotes, so the first `"` on a brush-plane line is
/// always the material field's opening quote. Lines without a quoted material
/// field pass through unchanged (the common case), so this is a no-op for maps
/// that use only space-free names.
fn encode_quoted_brush_textures(map_text: &str) -> String {
    let mut out = String::with_capacity(map_text.len());
    for line in map_text.split_inclusive('\n') {
        // Preserve the trailing newline (if any) when re-emitting the line.
        let (body, newline) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };

        // Brush-plane lines start with the first point's `(` (after optional
        // indentation). Anything else (KVP lines starting with `"`, braces,
        // blank lines) is emitted verbatim.
        let trimmed = body.trim_start();
        let is_brush_plane = trimmed.starts_with('(');
        let has_quote = body.contains('"');
        if !is_brush_plane || !has_quote {
            out.push_str(body);
            out.push_str(newline);
            continue;
        }

        // The first `"` on the line opens the material field — the point triples
        // ahead of it never contain a quote, so this is robust even when the
        // material name itself contains `)`.
        out.push_str(&encode_quoted_run(body));
        out.push_str(newline);
    }
    out
}

/// Rewrite the first double-quoted run in `line` into a space-free, unquoted
/// token: strip the surrounding quotes and replace interior spaces with the
/// space sentinel. Text outside the quoted run is preserved exactly. If there
/// is no complete quoted run, `line` is returned unchanged.
fn encode_quoted_run(line: &str) -> String {
    let open = match line.find('"') {
        Some(i) => i,
        None => return line.to_string(),
    };
    // Find the matching close quote after the open quote.
    let close = match line[open + 1..].find('"') {
        Some(i) => open + 1 + i,
        None => return line.to_string(),
    };

    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..open]);
    out.push_str(&line[open + 1..close].replace(' ', SPACE_SENTINEL_STR));
    out.push_str(&line[close + 1..]);
    out
}

/// Decode a brush-face material name produced by [`encode_quoted_brush_textures`],
/// turning the space sentinel back into a space. A no-op for names that were
/// never encoded (the sentinel does not appear in real material names).
fn decode_brush_texture(name: &str) -> String {
    name.replace(SPACE_SENTINEL, " ")
}

/// Look up a property value by key from shambler's entity properties.
fn get_property(geo_map: &GeoMap, entity_id: &EntityId, key: &str) -> Option<String> {
    let props = geo_map.entity_properties.get(entity_id)?;
    props.iter().find(|p| p.key == key).map(|p| p.value.clone())
}

/// Resolve a positive-float worldspawn KVP, falling back to `default` when
/// the key is absent or its value is non-finite/≤0. Mirrors the
/// `_lightmap_density` parse posture: invalid values warn and fall back rather
/// than halting the build (the key names the offending KVP — worldspawn has no
/// meaningful per-entity origin to name). Shared by the `nav_*` and
/// `entity_shadow_*` worldspawn KVPs.
fn parse_positive_worldspawn_kvp(
    geo_map: &GeoMap,
    worldspawn_id: &EntityId,
    key: &str,
    default: f32,
) -> f32 {
    match get_property(geo_map, worldspawn_id, key) {
        None => default,
        Some(raw) => match raw.trim().parse::<f32>() {
            Ok(parsed) if parsed.is_finite() && parsed > 0.0 => parsed,
            Ok(parsed) => {
                log::warn!(
                    "[Compiler] worldspawn `{key}` value `{parsed}` is non-finite or <= 0; \
                     falling back to default {default}"
                );
                default
            }
            Err(e) => {
                log::warn!(
                    "[Compiler] worldspawn `{key}` value `{raw}` is not a valid float ({e}); \
                     falling back to default {default}"
                );
                default
            }
        },
    }
}

/// `entity_shadow_*` worldspawn KVP parser — thin call-site alias for
/// [`parse_positive_worldspawn_kvp`].
fn parse_entity_shadow_kvp(
    geo_map: &GeoMap,
    worldspawn_id: &EntityId,
    key: &str,
    default: f32,
) -> f32 {
    parse_positive_worldspawn_kvp(geo_map, worldspawn_id, key, default)
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

fn is_runtime_map_entity_key(key: &str) -> bool {
    !quake_map::RESERVED_MAP_ENTITY_KEYS.contains(&key) && !key.starts_with("_tb_")
}

/// Read a `func_group` marker into the canonical identity the rest of the
/// compiler can use. The editor-specific keys stop at this adapter boundary.
fn map_assembly_from_marker(geo_map: &GeoMap, entity_id: &EntityId) -> MapAssembly {
    let group_id = get_property(geo_map, entity_id, "_tb_id")
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    let provenance = get_property(geo_map, entity_id, "_tb_name")
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if group_id.is_empty() {
                "group <unnamed>".to_string()
            } else {
                format!("group {group_id}")
            }
        });
    let linked_group_id = get_property(geo_map, entity_id, "_tb_linked_group_id")
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .strip_prefix('{')
                .and_then(|value| value.strip_suffix('}'))
                .unwrap_or(&value)
                .to_owned()
        });

    MapAssembly {
        provenance,
        group_id,
        linked_group_id,
    }
}

fn parse_kinematic_mover(
    props: &HashMap<String, String>,
    authored_origin: Option<DVec3>,
    brush_ids: Vec<BrushId>,
) -> anyhow::Result<PendingKinematicMover> {
    let name = props
        .get("name")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let path = props
        .get("path")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if path.is_empty() {
        anyhow::bail!("kinematic_mover `{name}` missing required `path` waypoint name");
    }

    let speed = parse_optional_finite_f32(props, "speed", 1.0, "kinematic_mover", &name)?;
    if speed <= 0.0 {
        anyhow::bail!("kinematic_mover `{name}` `speed` must be finite and positive, got {speed}");
    }

    let wait_ms = parse_optional_finite_f32(props, "wait_ms", 0.0, "kinematic_mover", &name)?;
    if wait_ms < 0.0 {
        anyhow::bail!(
            "kinematic_mover `{name}` `wait_ms` must be finite and non-negative, got {wait_ms}"
        );
    }

    let spin_speed_deg_s =
        parse_optional_finite_f32(props, "spin_speed", 0.0, "kinematic_mover", &name)?;
    let spin_accel_deg_s2 =
        parse_optional_finite_f32(props, "spin_accel", 0.0, "kinematic_mover", &name)?;
    if spin_speed_deg_s != 0.0 && spin_speed_deg_s.to_radians() == 0.0 {
        anyhow::bail!(
            "kinematic_mover `{name}` non-zero `spin_speed` becomes zero after conversion to radians/sec"
        );
    }
    if spin_accel_deg_s2 < 0.0 {
        anyhow::bail!(
            "kinematic_mover `{name}` `spin_accel` must be finite and non-negative, got {spin_accel_deg_s2}"
        );
    }
    if spin_accel_deg_s2 > 0.0 && spin_accel_deg_s2.to_radians() == 0.0 {
        anyhow::bail!(
            "kinematic_mover `{name}` positive `spin_accel` becomes zero after conversion to radians/sec²"
        );
    }
    let spin_axis = parse_kinematic_spin_axis(props, &name, spin_speed_deg_s)?;

    let carry_yaw = match props
        .get("carry_yaw")
        .map(|value| value.trim())
        .unwrap_or("0")
    {
        "1" | "true" | "True" => true,
        "0" | "false" | "False" => false,
        other => {
            anyhow::bail!(
                "kinematic_mover `{name}` `carry_yaw` must be `0`, `1`, `false`, `true`, `False`, or `True`, got `{other}`"
            );
        }
    };

    let block_policy = match props
        .get("block_policy")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("displace")
    {
        policy @ ("displace" | "reverse" | "stop" | "crush") => policy.to_string(),
        other => {
            anyhow::bail!(
                "kinematic_mover `{name}` `block_policy` must be `displace`, `reverse`, `stop`, or `crush`, got `{other}`"
            );
        }
    };
    let crush_damage =
        parse_optional_finite_f32(props, "crush_damage", 0.0, "kinematic_mover", &name)?;
    if crush_damage < 0.0 {
        anyhow::bail!(
            "kinematic_mover `{name}` `crush_damage` must be finite and non-negative, got {crush_damage}"
        );
    }
    let crush_interval_ms =
        parse_optional_finite_f32(props, "crush_interval_ms", 0.0, "kinematic_mover", &name)?;
    if crush_interval_ms < 0.0 {
        anyhow::bail!(
            "kinematic_mover `{name}` `crush_interval_ms` must be finite and non-negative, got {crush_interval_ms}"
        );
    }
    let auto_close_ms = props
        .get("auto_close_ms")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| {
            value.parse::<f32>().map_err(|_| {
                anyhow::anyhow!(
                    "kinematic_mover `{name}` `auto_close_ms` must be a finite number, got `{value}`"
                )
            })
        })
        .transpose()?;
    if auto_close_ms.is_some_and(|value| !value.is_finite() || value < 0.0) {
        anyhow::bail!(
            "kinematic_mover `{name}` `auto_close_ms` must be finite and non-negative, got {}",
            auto_close_ms.expect("checked as present")
        );
    }
    // Events and sound keys share one rule: trimmed, and blank reads as absent.
    let optional_event = |key: &str| {
        props
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let open_event = optional_event("open_event");
    let close_event = optional_event("close_event");
    let blocked_event = optional_event("blocked_event");
    let crush_event = optional_event("crush_event");
    let open_sound = optional_event("open_sound");
    let close_sound = optional_event("close_sound");
    let blocked_sound = optional_event("blocked_sound");
    let crush_sound = optional_event("crush_sound");
    // A damage-type name; the engine resolves it against the mod's registered
    // types at level load and warns on an unknown one.
    let crush_damage_type = optional_event("crush_damage_type");
    let (light_ramp_start, light_ramp_end) = parse_kinematic_light_ramp(props, &name)?;
    let light_bounces = parse_kinematic_light_bounces(props, &name)?;
    let light_poses = parse_kinematic_light_poses(props, &name)?;

    let move_mode = match props
        .get("move_mode")
        .map(|value| value.trim())
        .unwrap_or("once")
    {
        "once" | "0" => KinematicMoveMode::Once,
        "ping_pong" | "1" => KinematicMoveMode::PingPong,
        other => {
            anyhow::bail!(
                "kinematic_mover `{name}` `move_mode` must be `once` or `ping_pong`, got `{other}`"
            );
        }
    };

    let start_on_spawn = match props
        .get("start_on_spawn")
        .map(|value| value.trim())
        .unwrap_or("1")
    {
        "1" | "true" | "True" => true,
        "0" | "false" | "False" => false,
        other => {
            anyhow::bail!("kinematic_mover `{name}` `start_on_spawn` must be 0/1, got `{other}`");
        }
    };

    let tags = props
        .get("_tags")
        .map(|s| s.split_whitespace().map(|tag| tag.to_string()).collect())
        .unwrap_or_default();

    Ok(PendingKinematicMover {
        name,
        tags,
        authored_origin,
        path,
        speed,
        wait_ms,
        spin_axis,
        spin_speed_deg_s,
        spin_accel_deg_s2,
        carry_yaw,
        block_policy,
        crush_damage,
        crush_interval_ms,
        auto_close_ms,
        open_event,
        close_event,
        blocked_event,
        crush_event,
        open_sound,
        close_sound,
        blocked_sound,
        crush_sound,
        crush_damage_type,
        light_ramp_start,
        light_ramp_end,
        light_bounces,
        light_poses,
        move_mode,
        start_on_spawn,
        brush_ids,
    })
}

/// The mover's light ramp keys, `light_ramp_start` (default 0) and
/// `light_ramp_end` (default 1): the travel window its light state ramps
/// across. Each must be a finite number, with `0 <= start < end <= 1`; a
/// blank key is absent. A start of `-0` reads as 0.
fn parse_kinematic_light_ramp(
    props: &HashMap<String, String>,
    name: &str,
) -> anyhow::Result<(f32, f32)> {
    let key = |key: &str, default: f32| {
        non_blank(props, key).map_or(Ok(default), |raw| {
            parse_finite_f32(raw, key, "kinematic_mover", name)
        })
    };
    // `-0` parses as −0.0; adding +0.0 turns it into +0.0 and leaves every
    // other value as it is, so one ramp has one encoding in id 57.
    let start = key("light_ramp_start", 0.0)? + 0.0;
    let end = key("light_ramp_end", 1.0)?;
    if !(0.0..end).contains(&start) || end > 1.0 {
        anyhow::bail!(
            "kinematic_mover `{name}` `light_ramp_start` and `light_ramp_end` must satisfy \
             0 <= light_ramp_start < light_ramp_end <= 1, got {start} and {end}"
        );
    }
    Ok((start, end))
}

/// The mover's `light_bounces` key: how many static bounces its poses
/// re-bake, an integer of at least 0. Absent or blank re-bakes every bounce
/// the SH base bakes. The integer reads as the map's other integer keys do
/// (`priority`, `_stream_priority`): an optional sign, so `+2` is 2 and `-0`
/// is 0. A negative, fractional or non-numeric value fails the build, naming
/// the mover, as does one past `u32::MAX`, which says it is too large.
fn parse_kinematic_light_bounces(
    props: &HashMap<String, String>,
    name: &str,
) -> anyhow::Result<Option<u32>> {
    let Some(raw) = non_blank(props, "light_bounces") else {
        return Ok(None);
    };
    let too_large = || {
        anyhow::anyhow!(
            "kinematic_mover `{name}` `light_bounces` `{raw}` is too large: it must be at most \
             {}, and any count past the level's bounces bakes them all",
            u32::MAX
        )
    };
    let not_a_count = || {
        anyhow::anyhow!(
            "kinematic_mover `{name}` `light_bounces` must be an integer of at least 0, \
             got `{raw}`"
        )
    };
    match raw.parse::<i64>() {
        Ok(value) if value < 0 => Err(not_a_count()),
        Ok(value) => u32::try_from(value).map(Some).map_err(|_| too_large()),
        Err(error) if *error.kind() == std::num::IntErrorKind::PosOverflow => Err(too_large()),
        Err(_) => Err(not_a_count()),
    }
}

/// The mover's `light_poses` key: how many even poses the mover probe
/// channel bakes along its path, termini included, an integer of at least 2,
/// and no geometry poses. Absent or blank places them automatically: even
/// poses about one probe spacing apart, up to a cap, plus geometry poses
/// where the mover's brushes start or stop enclosing a probe. Reads
/// as `light_bounces` does, with an optional sign; anything else fails the
/// build naming the mover, as does a count past the most poses a path can
/// place ([`MAX_POSES`]), which says it is too large. Placement refuses that
/// count again before it allocates.
fn parse_kinematic_light_poses(
    props: &HashMap<String, String>,
    name: &str,
) -> anyhow::Result<Option<u32>> {
    let Some(raw) = non_blank(props, "light_poses") else {
        return Ok(None);
    };
    let too_large = || {
        anyhow::anyhow!(
            "kinematic_mover `{name}` `light_poses` `{raw}` is too large: it must be at most \
             {MAX_POSES}, the most poses a path can place; past it, neighbouring poses fall on \
             one point of travel"
        )
    };
    let not_a_count = || {
        anyhow::anyhow!(
            "kinematic_mover `{name}` `light_poses` must be an integer of at least 2 (2 bakes \
             only the path's two ends), got `{raw}`"
        )
    };
    match raw.parse::<i64>() {
        Ok(value) if value < 2 => Err(not_a_count()),
        Ok(value) => u32::try_from(value)
            .ok()
            .filter(|&count| count <= MAX_POSES)
            .map(Some)
            .ok_or_else(too_large),
        Err(error) if *error.kind() == std::num::IntErrorKind::PosOverflow => Err(too_large()),
        Err(_) => Err(not_a_count()),
    }
}

fn parse_kinematic_spin_axis(
    props: &HashMap<String, String>,
    name: &str,
    spin_speed_deg_s: f32,
) -> anyhow::Result<[f32; 3]> {
    let Some(raw) = props.get("spin_axis") else {
        if spin_speed_deg_s != 0.0 {
            anyhow::bail!(
                "kinematic_mover `{name}` `spin_axis` must be finite and non-zero when `spin_speed` is non-zero"
            );
        }
        return Ok([0.0; 3]);
    };

    let components: Vec<&str> = raw.split_whitespace().collect();
    if components.len() != 3 {
        anyhow::bail!(
            "kinematic_mover `{name}` `spin_axis` must contain exactly three components, got `{raw}`"
        );
    }
    let mut axis = [0.0f32; 3];
    for (index, component) in components.iter().enumerate() {
        let parsed: f32 = component.parse().map_err(|error| {
            anyhow::anyhow!(
                "kinematic_mover `{name}` `spin_axis` component {index} `{component}` is not a valid float: {error}"
            )
        })?;
        if !parsed.is_finite() {
            anyhow::bail!(
                "kinematic_mover `{name}` `spin_axis` component {index} is not finite: `{component}`"
            );
        }
        axis[index] = parsed;
    }

    let axis = quake_to_engine(DVec3::new(
        f64::from(axis[0]),
        f64::from(axis[1]),
        f64::from(axis[2]),
    ));
    let length = axis.length();
    if length == 0.0 {
        if spin_speed_deg_s != 0.0 {
            anyhow::bail!(
                "kinematic_mover `{name}` `spin_axis` must be finite and non-zero when `spin_speed` is non-zero"
            );
        }
        return Ok([0.0; 3]);
    }

    let axis = axis / length;
    Ok([axis.x as f32, axis.y as f32, axis.z as f32])
}

fn parse_optional_finite_f32(
    props: &HashMap<String, String>,
    key: &str,
    default: f32,
    classname: &str,
    name: &str,
) -> anyhow::Result<f32> {
    let Some(raw) = props.get(key) else {
        return Ok(default);
    };
    parse_finite_f32(raw, key, classname, name)
}

fn parse_finite_f32(raw: &str, key: &str, classname: &str, name: &str) -> anyhow::Result<f32> {
    let parsed: f32 = raw.trim().parse().map_err(|e| {
        anyhow::anyhow!("{classname} `{name}` `{key}` value `{raw}` is not a valid float: {e}")
    })?;
    if !parsed.is_finite() {
        anyhow::bail!("{classname} `{name}` `{key}` value `{raw}` is not finite");
    }
    Ok(parsed)
}

/// `key`'s trimmed value, `None` when absent or blank: a blank key is
/// absent, so it takes the automatic default.
fn non_blank<'a>(props: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    props
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// Default `switch` `use_reach` margin, in map units (~0.61 m at 1 unit = 1 inch).
///
/// Must match the `use_reach` default in `sdk/TrenchBroom/postretro.fgd` — two
/// hand-maintained copies of one number, held together by
/// `fgd_switch_use_reach_matches_the_compiler_constants`.
/// Deliberately a literal rather than the runtime player capsule radius: that
/// radius is an authored descriptor field the level compiler cannot reach, and
/// this value is chosen to exceed it. The first-party player capsule is 0.2 m
/// (`content/dev/scripts/player.ts`), about 7.9 map units.
const DEFAULT_SWITCH_USE_REACH: f32 = 24.0;

/// Largest accepted `switch` `use_reach`, in map units (~3.25 m).
///
/// Reach only has to bridge the gap between the switch face and the player
/// capsule, so every legitimate value sits near the 24-unit default; a console
/// deep in an alcove might want a few times that. A bound a little over 5× the
/// default catches the authoring typo this exists for — a stray digit (`240`) or a
/// unit mix-up — before it becomes a trigger volume that swallows the room and
/// fires on every `use` press in it.
const MAX_SWITCH_USE_REACH: f32 = 128.0;

/// Distance below which two coincident planes are treated as one, in engine meters.
///
/// Two coincident planes do not compare equal in this file: a trigger AABB is `f32`
/// widened back to f64, while brush hulls are native f64, so a flush mount's shared
/// plane lands a few hundred nanometres apart. The value sits between two bounds —
/// above f32 representation error at level coordinates (~1e-6 m near the origin,
/// approaching 1e-4 m out at 800 m) and 25× below one map unit (0.0254 m), so no
/// authored dimension can hide inside it.
///
/// Used by every comparison in [`apply_switch_use_reach`] that could straddle a
/// contact plane, and as the floor on `use_reach`: a margin the gate would discard
/// as float dust is a no-op, not a small reach.
const FLUSH_TOLERANCE_METERS: f64 = 1e-3;

/// A `switch` trigger awaiting its per-face reach margins.
///
/// Deferred out of the entity loop because the clamp reads the finished static
/// world brush set, which the loop is still collecting.
#[derive(Debug)]
struct PendingSwitchReach {
    /// Position of this switch's volume in the `trigger_volumes` list.
    trigger_index: usize,
    name: String,
    /// Reach margin in engine meters.
    margin: f32,
    /// The switch's own brushes, excluded from its occluder set — they are folded
    /// into the static world set and would otherwise clamp the switch against
    /// itself.
    brush_ids: Vec<BrushId>,
}

/// Read and parse a .map file, classify brushes, and extract face geometry.
///
/// The `format` parameter identifies the source map format. Its `units_to_meters()`
/// scale is applied at this boundary alongside the axis swizzle: vertex positions,
/// entity origins, and plane distances are all converted to engine meters here.
/// All downstream stages receive engine-native coordinates and meters.
pub fn parse_map_file(path: &Path, format: MapFormat) -> Result<MapData> {
    let scale = format.units_to_meters();
    let raw_map_text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read map file: {}", path.display()))?;

    // TrenchBroom wraps a brush face's material name in double quotes whenever
    // that name contains a space (e.g. a collection directory named
    // `Level Eleven Games Sci-Fi Texture Pack v1`). shalrath's brush-plane
    // grammar tokenizes the material field with `take_until(" ")` and has no
    // quote handling, so a quoted, space-containing name shatters the face
    // grammar — the brush fails, its worldspawn entity fails, and the compiler
    // reports a misleading "no worldspawn" error. Encode the spaces inside
    // quoted material fields to a sentinel here so shalrath sees one token; the
    // name is decoded back to its space-containing form at the texture-read
    // boundary below. See `context/lib/build_pipeline.md` §Texture name resolution.
    //
    // Layout shalrath's grammar cannot take (blank, indented and comment lines,
    // CRLF) is normalized away first, because at the top level it ends the
    // entity list silently. The quote encoding rewrites line for line, so the
    // normalized line numbers still map back to the source for parse errors.
    let source = normalize_map_source(&raw_map_text);
    let map_text = encode_quoted_brush_textures(&source.text);
    reject_invalid_streaming_hint_source_hulls(&map_text, scale)?;

    let shalrath_map = parse_complete_map(&source, &map_text)?;

    let geo_map = GeoMap::new(shalrath_map);

    // Recognize editor-group markers before the entity walk. Sibling members
    // may appear before their marker in source order, but static brush
    // flattening remains in the existing walk below so its geometry ordering is
    // unchanged.
    let mut assemblies = Vec::new();
    // Association is safe only when a marker identity is non-empty and unique.
    // Keep every marker as an append-only assembly for diagnostics and future
    // consumers, but do not guess which marker owns members in malformed input.
    // Only TrenchBroom groups and layers need an id: an authored `func_group`
    // brush entity has none and is not counted.
    let mut empty_marker_count = 0usize;
    for entity_id in geo_map.entities.iter() {
        let classname = get_property(&geo_map, entity_id, "classname");
        if classname.as_deref() != Some("func_group") {
            continue;
        }

        let assembly = map_assembly_from_marker(&geo_map, entity_id);
        if assembly.group_id.is_empty()
            && quake_map::is_trenchbroom_container(
                get_property(&geo_map, entity_id, "_tb_type").as_deref(),
            )
        {
            empty_marker_count += 1;
        }
        assemblies.push(assembly);
    }

    if empty_marker_count > 0 {
        log::warn!(
            "[Compiler] {empty_marker_count} func_group marker(s) have an empty `_tb_id`; brace brushes and sibling members from those markers will remain unassociated; assign every TrenchBroom group and layer a unique non-empty `_tb_id`"
        );
    }

    let mut marker_count_by_group_id = HashMap::new();
    for assembly in &assemblies {
        if !assembly.group_id.is_empty() {
            *marker_count_by_group_id
                .entry(assembly.group_id.clone())
                .or_insert(0usize) += 1;
        }
    }

    let mut assembly_by_group_id = HashMap::new();
    let mut warned_duplicate_group_ids = HashSet::new();
    for (assembly_index, assembly) in assemblies.iter().enumerate() {
        match marker_count_by_group_id.get(&assembly.group_id) {
            Some(&1) => {
                assembly_by_group_id.insert(assembly.group_id.clone(), assembly_index);
            }
            Some(_) if warned_duplicate_group_ids.insert(assembly.group_id.clone()) => {
                log::warn!(
                    "[Compiler] duplicate func_group `_tb_id` `{}`; brace brushes and sibling members for every marker with this id will remain unassociated; assign every TrenchBroom group and layer a unique non-empty `_tb_id`",
                    assembly.group_id,
                );
            }
            _ => {}
        }
    }
    let mut assembly_members: Vec<AssemblyMembers> = (0..assemblies.len())
        .map(|_| AssemblyMembers::default())
        .collect();

    // Identify worldspawn entity
    let worldspawn_id = geo_map
        .entities
        .iter()
        .find(|id| get_property(&geo_map, id, "classname").as_deref() == Some("worldspawn"))
        .copied();
    let worldspawn_id = match worldspawn_id {
        Some(id) => id,
        // If shalrath parsed entities but none is worldspawn, the likely cause
        // is a brush-face line that failed to tokenize (commonly a material
        // name with characters shalrath's face grammar can't handle), which
        // drops the enclosing entity. Point at the material names rather than
        // at a literally-missing worldspawn.
        None if !geo_map.entities.is_empty() => {
            anyhow::bail!(
                "no worldspawn entity found, though {} entit{} parsed — this often means a \
                 malformed brush-face line (e.g. an unsupported character in a material name); \
                 check the map's material names",
                geo_map.entities.len(),
                if geo_map.entities.len() == 1 {
                    "y was"
                } else {
                    "ies were"
                }
            );
        }
        None => anyhow::bail!("no worldspawn entity found in .map file"),
    };

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
    let mut world_brush_ids: Vec<BrushId> = geo_map
        .entity_brushes
        .get(&worldspawn_id)
        .cloned()
        .unwrap_or_default();
    // Associate only brushes flattened through the `func_group` branch below.
    // Other paths such as `switch` deliberately retain their current ungrouped
    // diagnostic behavior until their own provenance work lands.
    let mut assembly_by_brush_id: HashMap<BrushId, usize> = HashMap::new();
    let mut reflection_capture_overrides =
        reflection_capture::ReflectionCaptureOverrides::default();

    // Collect entity info and entity brush counts
    let mut entities = Vec::new();
    let mut entity_brushes_summary = Vec::new();
    let mut entity_classnames: Vec<String> = Vec::new();
    // Lights translate to the canonical format as we walk entities — they
    // share the origin/classname extraction but do not participate in BSP
    // construction.
    let mut lights: Vec<MapLight> = Vec::new();
    let mut light_start_active_defaults = Vec::new();
    let mut light_source_labels: Vec<String> = Vec::new();
    // Generic map entities for runtime classname dispatch — non-light point
    // entities only. Brush entities (those with brushes attached) are resolved
    // separately by their dedicated subsystems (e.g. `fog_volume`).
    let mut map_entities: Vec<MapEntityRecord> = Vec::new();
    let mut pending_kinematic_movers: Vec<PendingKinematicMover> = Vec::new();
    let mut kinematic_waypoints: Vec<MapKinematicWaypoint> = Vec::new();
    // Resolved fog volume entities (brush `fog_volume` plus point `fog_lamp`
    // and `fog_tube`). Walked alongside the entity pass; brush AABBs come from
    // brush-face vertices, point-entity AABBs from origin + radius/height.
    let mut fog_volumes: Vec<MapFogVolume> = Vec::new();
    // Brush-defined scale overrides are preserved in source entity order. Chart
    // planning uses that order to make overlapping regions last-defined-wins.
    let mut lightmap_scale_regions: Vec<MapLightmapScaleRegion> = Vec::new();
    // Compiler-only authored SH streaming metadata. These brushes are peeled
    // before static BSP inputs; later tasks resolve the retained canonical hulls
    // against generated portals and runtime cells.
    let mut streaming_seam_regions: Vec<MapStreamingHintRegion> = Vec::new();
    let mut stream_resident_regions: Vec<MapStreamingHintRegion> = Vec::new();
    let mut stream_priority_regions: Vec<MapStreamingPriorityRegion> = Vec::new();
    let mut trigger_volumes = Vec::new();
    let mut environment_volumes = Vec::new();
    let mut teleporters = teleporters::TeleporterParser::default();
    let mut ghost_links = ghost_links::GhostLinkParser::default();
    let mut attack_pools = attack_pools::AttackPoolChecker::default();
    // Parallel to `environment_volumes`: each volume's authored brush ids, kept
    // until the fluid volumes' textured sides are built in one pass.
    let mut environment_volume_brush_ids: Vec<Vec<BrushId>> = Vec::new();
    // Authored reflection-probe boxes, in map entity order.
    let mut reflection_volumes = Vec::new();
    let mut pending_switch_reach: Vec<PendingSwitchReach> = Vec::new();
    // Mapper-authored SH probe-coarsening protection volumes, each a world-space
    // AABB `[minx,miny,minz,maxx,maxy,maxz]` in engine meters. Unioned with the
    // `--sh-protect-aabb` CLI stand-in at the coarsening classifier so both
    // sources force intersecting 4×4×4 bricks to stay L0 (dense).
    let mut sh_protect_aabbs: Vec<[f32; 6]> = Vec::new();

    // Worldspawn `fog_pixel_scale` (1=full-res, 8=coarsest). Default 4 when
    // unset. `0` is the "unset" sentinel — pass it through as `0` so the
    // engine's `clamp_fog_pixel_scale(0)` returns its own default (4).
    // Values above 8 are author errors and are clamped to 8 silently.
    // Negative values are treated as unset (0).
    let fog_pixel_scale: u32 = get_property(&geo_map, &worldspawn_id, "fog_pixel_scale")
        .and_then(|s| s.trim().parse::<i64>().ok())
        .map(|v| v.clamp(0, 8) as u32)
        .unwrap_or(0);

    // Worldspawn `_lightmap_density` (meters per texel, > 0). Optional opt-in
    // for per-map lightmap fidelity. Absent → falls through to the documented
    // `DEFAULT_TEXEL_DENSITY_METERS` default in the compiler. Non-finite/≤0
    // values are warned-and-discarded per `build_pipeline.md` §Built-in
    // Classname Routing (worldspawn has no meaningful per-entity origin to
    // name; the warning names the key). The `--lightmap-density` CLI flag
    // overrides any KVP value (and keeps its hard-reject posture).
    let lightmap_density: Option<f32> = match get_property(
        &geo_map,
        &worldspawn_id,
        "_lightmap_density",
    ) {
        None => None,
        Some(raw) => match raw.trim().parse::<f32>() {
            Ok(parsed) if parsed.is_finite() && parsed > 0.0 => Some(parsed),
            Ok(parsed) => {
                log::warn!(
                    "[Compiler] worldspawn `_lightmap_density` value `{parsed}` is non-finite or \
                         <= 0; falling back to default"
                );
                None
            }
            Err(e) => {
                log::warn!(
                    "[Compiler] worldspawn `_lightmap_density` value `{raw}` is not a valid \
                         float ({e}); falling back to default"
                );
                None
            }
        },
    };

    // Worldspawn `_sh_density_fidelity` is the base-SH classifier's relative
    // error multiplier. It deliberately follows `_lightmap_density`'s optional
    // authoring posture: invalid map input warns and falls back to the compiler
    // default, while an explicit CLI override remains a hard argument error.
    let sh_density_fidelity: Option<f32> = match get_property(
        &geo_map,
        &worldspawn_id,
        "_sh_density_fidelity",
    ) {
        None => None,
        Some(raw) => match raw.trim().parse::<f32>() {
            Ok(parsed) if parsed.is_finite() && parsed > 0.0 => Some(parsed),
            Ok(parsed) => {
                log::warn!(
                    "[Compiler] worldspawn `_sh_density_fidelity` value `{parsed}` is non-finite or \
                     <= 0; falling back to default"
                );
                None
            }
            Err(error) => {
                log::warn!(
                    "[Compiler] worldspawn `_sh_density_fidelity` value `{raw}` is not a valid \
                     float ({error}); falling back to default"
                );
                None
            }
        },
    };

    // Worldspawn `_bounce_fill` weights the SH base's later bounces: the one
    // author-facing bounce control, a map key and never a compiler flag. An
    // invalid value warns here, once per compile, and bakes the default.
    let bounce_fill = match get_property(&geo_map, &worldspawn_id, "_bounce_fill") {
        None => surface_radiance::DEFAULT_BOUNCE_FILL,
        Some(raw) => match raw.trim().parse::<f32>() {
            Ok(parsed) if surface_radiance::is_valid_bounce_fill(parsed) => parsed,
            Ok(parsed) => {
                log::warn!(
                    "[Compiler] worldspawn `_bounce_fill` value `{parsed}` is non-finite or \
                     outside [0, 1]; falling back to default {}",
                    surface_radiance::DEFAULT_BOUNCE_FILL
                );
                surface_radiance::DEFAULT_BOUNCE_FILL
            }
            Err(error) => {
                log::warn!(
                    "[Compiler] worldspawn `_bounce_fill` value `{raw}` is not a valid float \
                     ({error}); falling back to default {}",
                    surface_radiance::DEFAULT_BOUNCE_FILL
                );
                surface_radiance::DEFAULT_BOUNCE_FILL
            }
        },
    };

    // Production coarsening is default-on for direct SH deltas. Only the
    // canonical literal `"0"` opts a map into the byte-identical uniform-L0
    // path; absent and all other values leave the default enabled.
    let uniform_grid_optout = get_property(&geo_map, &worldspawn_id, "_sh_coarsen")
        .is_some_and(|value| value.trim() == "0");

    // Worldspawn `nav_*` navigation-bake parameters (meters, or degrees for
    // slope). Optional per-map overrides of the engine defaults in
    // `NavParams::default`; mirrors the `_lightmap_density` precedent above —
    // each absent or invalid (non-finite/≤0) value falls back to its default
    // with a warning. The keys lead without an underscore, following the
    // `fog_pixel_scale` form (the majority for engine-authored worldspawn KVPs).
    let nav_defaults = NavParams::default();
    let nav_params = NavParams {
        agent_radius: parse_positive_worldspawn_kvp(
            &geo_map,
            &worldspawn_id,
            "nav_agent_radius",
            nav_defaults.agent_radius,
        ),
        agent_height: parse_positive_worldspawn_kvp(
            &geo_map,
            &worldspawn_id,
            "nav_agent_height",
            nav_defaults.agent_height,
        ),
        step_height: parse_positive_worldspawn_kvp(
            &geo_map,
            &worldspawn_id,
            "nav_step_height",
            nav_defaults.step_height,
        ),
        max_slope_deg: parse_positive_worldspawn_kvp(
            &geo_map,
            &worldspawn_id,
            "nav_max_slope",
            nav_defaults.max_slope_deg,
        ),
        cell_size: parse_positive_worldspawn_kvp(
            &geo_map,
            &worldspawn_id,
            "nav_cell_size",
            nav_defaults.cell_size,
        ),
    };

    let entity_shadow_defaults = EntityShadowParams::default();
    let entity_shadow_params = EntityShadowParams {
        min_intensity_ratio: parse_entity_shadow_kvp(
            &geo_map,
            &worldspawn_id,
            "entity_shadow_min_intensity_ratio",
            entity_shadow_defaults.min_intensity_ratio,
        ),
        min_range: parse_entity_shadow_kvp(
            &geo_map,
            &worldspawn_id,
            "entity_shadow_min_range",
            entity_shadow_defaults.min_range,
        ),
    };

    // Worldspawn `initialGravity` (m/s², negative = downward). Absence uses
    // the documented default gravity; supplied values remain strict so
    // malformed map data cannot reach the runtime gravity register.
    let initial_gravity: f32 = {
        if let Some(raw) = get_property(&geo_map, &worldspawn_id, "initialGravity") {
            let parsed: f32 = raw.trim().parse().map_err(|e| {
                anyhow::anyhow!(
                    "worldspawn `initialGravity` value `{raw}` is not a valid float: {e}"
                )
            })?;
            if !parsed.is_finite() {
                anyhow::bail!("worldspawn `initialGravity` value `{raw}` is not a finite number");
            }
            parsed
        } else {
            DEFAULT_WORLD_GRAVITY_MPS2
        }
    };

    for entity_id in geo_map.entities.iter() {
        let classname =
            get_property(&geo_map, entity_id, "classname").unwrap_or_else(|| "unknown".to_string());
        // Swizzle axes then apply unit scale: origin is a position, not a direction.
        let origin = get_property(&geo_map, entity_id, "origin")
            .and_then(|s| parse_origin(&s))
            .map(|v| quake_map::canonical_entity_origin(&classname, v))
            .map(|v| quake_to_engine(v) * scale);

        entities.push(EntityInfo {
            classname: classname.clone(),
            origin,
        });

        // Sibling members identify a marker through `_tb_group`; capture that
        // source relation before the branches below translate or strip their
        // property bags. A missing or ambiguous marker identity is malformed
        // authoring data here: it simply has no assembly association.
        let sibling_assembly_index = get_property(&geo_map, entity_id, "_tb_group")
            .map(|value| value.trim().to_owned())
            .and_then(|group_id| assembly_by_group_id.get(&group_id).copied());

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
                Ok(light) => {
                    let light_index = lights.len();
                    light_start_active_defaults.push(
                        quake_map::authored_light_start_active(&props).map_err(|error| {
                            anyhow::anyhow!(
                                "failed to translate {classname} at {light_origin:?}: {error}"
                            )
                        })?,
                    );
                    lights.push(light);
                    light_source_labels.push(quake_map::light_source_label(
                        entities.len() - 1,
                        &classname,
                        &get_property(&geo_map, entity_id, "origin").unwrap_or_default(),
                    ));
                    if let Some(assembly_index) = sibling_assembly_index {
                        assembly_members[assembly_index]
                            .light_indices
                            .push(light_index);
                    }
                }
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

        let brush_ids = geo_map
            .entity_brushes
            .get(entity_id)
            .cloned()
            .unwrap_or_default();

        if quake_map::is_editor_group_classname(&classname) {
            reflection_capture_overrides.record_func_group(
                &collect_entity_properties(&geo_map, entity_id),
                &format!("entity {} '{classname}'", entities.len() - 1),
                &brush_ids,
            );
            let group_id = get_property(&geo_map, entity_id, "_tb_id")
                .map(|value| value.trim().to_owned())
                .unwrap_or_default();
            if let Some(&assembly_index) = assembly_by_group_id.get(&group_id) {
                for brush_id in &brush_ids {
                    assembly_by_brush_id.insert(*brush_id, assembly_index);
                }
            }
            world_brush_ids.extend(brush_ids);
            continue;
        }

        // Semantic brush entities are resolved by dedicated compiler/runtime
        // paths. Editor groups are already flattened into static world brushes.
        let has_brushes = !brush_ids.is_empty();

        if matches!(
            classname.as_str(),
            "trigger_teleport" | "teleport_destination"
        ) {
            let props = collect_entity_properties(&geo_map, entity_id);
            teleporters.extract(
                &geo_map,
                &brush_ids,
                &props,
                scale,
                &classname,
                entities.len() - 1,
                &mut map_entities,
            )?;
            continue;
        }

        if classname == postretro_level_format::ghost_links::GHOST_LINK_CLASSNAME {
            let props = collect_entity_properties(&geo_map, entity_id);
            ghost_links.extract(
                &geo_map,
                &brush_ids,
                &props,
                scale,
                entities.len() - 1,
                &mut map_entities,
            )?;
            continue;
        }

        // Environment volumes are non-solid: their brushes never join the
        // static world set, so they seal no cell and reach no bake.
        if environment_volumes::is_environment_volume_classname(&classname) {
            let props = collect_entity_properties(&geo_map, entity_id);
            environment_volumes.push(environment_volumes::resolve_environment_volume(
                &geo_map,
                &brush_ids,
                &props,
                scale,
                &classname,
                entities.len() - 1,
            )?);
            environment_volume_brush_ids.push(brush_ids);
            continue;
        }

        // Reflection volumes are compiler-only boxes for the reflection-probe
        // stage. They stay out of `is_environment_volume_classname` so id 52
        // never sees them, and out of the world set so no geometry, BVH or SH
        // input changes when one is added.
        if classname == reflection_volumes::REFLECTION_VOLUME {
            let props = collect_entity_properties(&geo_map, entity_id);
            reflection_volumes.push(reflection_volumes::resolve_reflection_volume(
                &geo_map,
                &brush_ids,
                &props,
                scale,
                entities.len() - 1,
            )?);
            continue;
        }

        // Streaming hints are compiler-only brush entities. Reject a point
        // entity before the generic `has_brushes` branch so it cannot fall
        // through to runtime classname dispatch as inert, invisible content.
        if matches!(
            classname.as_str(),
            "stream_split_volume" | "stream_resident_volume" | "stream_priority_region"
        ) && !has_brushes
        {
            let location = match origin {
                Some(origin) if origin.is_finite() => {
                    format!("at ({:.3}, {:.3}, {:.3}) m", origin.x, origin.y, origin.z)
                }
                Some(_) => "at a non-finite authored origin".to_string(),
                None => "at an entity without an authored origin".to_string(),
            };
            anyhow::bail!(
                "{classname} {location} has no brushes — streaming hints require exactly one convex brush"
            );
        }

        // A brushless `switch` has nothing to desugar: no geometry to fold, no
        // hull to grow. Falling through to the point-entity tail below would emit
        // a `MapEntityRecord` that the runtime drops at `debug!` as an
        // unregistered classname — no geometry, no trigger, no compile
        // diagnostic. Point-entity switches are out of scope, so this is a broken
        // map and the author has to hear about it here.
        if classname == "switch" && !has_brushes {
            anyhow::bail!(
                "switch `{}` {} has no brushes — a switch is brush sugar: its brushwork becomes \
                 both the visible solid and the press volume, so a point-entity switch \
                 compiles to nothing. Author it as brushwork, or delete it",
                get_property(&geo_map, entity_id, "name")
                    .map(|v| v.trim().to_owned())
                    .unwrap_or_default(),
                // `name` is optional in the FGD, so an unnamed offender would print
                // as ``switch `` `` with nothing for the author to search on.
                match origin {
                    Some(o) => format!("at ({:.3}, {:.3}, {:.3}) m", o.x, o.y, o.z),
                    None => "(no `origin` authored either)".to_string(),
                }
            );
        }

        if has_brushes {
            if matches!(
                classname.as_str(),
                "stream_split_volume" | "stream_resident_volume" | "stream_priority_region"
            ) {
                if brush_ids.len() != 1 {
                    let location = match origin {
                        Some(origin) if origin.is_finite() => {
                            format!("at ({:.3}, {:.3}, {:.3}) m", origin.x, origin.y, origin.z)
                        }
                        Some(_) => "at a non-finite authored origin".to_string(),
                        None => "at an entity without an authored origin".to_string(),
                    };
                    anyhow::bail!(
                        "{classname} {location} owns {} brushes — streaming hints require exactly one convex brush",
                        brush_ids.len()
                    );
                }

                let props = collect_entity_properties(&geo_map, entity_id);
                match classname.as_str() {
                    "stream_split_volume" => {
                        streaming_seam_regions.push(resolve_streaming_hint_region(
                            &geo_map, &brush_ids, scale, &classname, origin,
                        )?)
                    }
                    "stream_resident_volume" => {
                        stream_resident_regions.push(resolve_streaming_hint_region(
                            &geo_map, &brush_ids, scale, &classname, origin,
                        )?)
                    }
                    "stream_priority_region" => {
                        stream_priority_regions.push(resolve_streaming_priority_region(
                            &geo_map, &brush_ids, &props, scale, &classname, origin,
                        )?)
                    }
                    _ => unreachable!("streaming hint classname was pre-filtered"),
                }
                continue;
            }
            if classname == "trigger_volume" {
                let props = collect_entity_properties(&geo_map, entity_id);
                trigger_volumes.push(crate::trigger_volumes::resolve_trigger_volume(
                    &geo_map, &brush_ids, &props, scale, &classname,
                )?);
                continue;
            }
            // A brush volume marking world space where SH probe coarsening must
            // be suppressed: intersecting 4×4×4 bricks stay L0 (dense). The
            // mapper-authored counterpart to the `--sh-protect-aabb` CLI stand-in
            // — both feed the same id-41 coarsening protection input (unioned
            // in `pipeline.rs::apply_coarsen_classification`).
            //
            if classname == "sh_protect_volume" {
                let props = collect_entity_properties(&geo_map, entity_id);
                sh_protect_aabbs.push(resolve_sh_protect_aabb(
                    &geo_map, &brush_ids, &props, scale, &classname,
                )?);
                continue;
            }
            // A `switch` is authoring sugar that desugars into two shipped
            // mechanisms driven by the same brushes: static world geometry (so
            // the switch is visible and solid) and a `use` trigger volume (so
            // it is pressable). No runtime type is involved.
            if classname == "switch" {
                let mut props = collect_entity_properties(&geo_map, entity_id);
                let switch_name = props
                    .get("name")
                    .map(|v| v.trim().to_owned())
                    .unwrap_or_default();
                // `name` is optional in the FGD, so every bail below carries the
                // brushwork's position too — an unnamed offender otherwise reads as
                // ``switch `` `` with nothing for the author to search on. Computed
                // on demand: the hull build is error-path only.
                let switch_location = || {
                    let mut bounds = crate::partition::Aabb::empty();
                    for volume in build_brush_volumes(&geo_map, &brush_ids, scale) {
                        bounds.expand_aabb(&volume.aabb);
                    }
                    let centre = bounds.centroid();
                    format!("({:.3}, {:.3}, {:.3}) m", centre.x, centre.y, centre.z)
                };
                // One brush per switch. The press volume is the union of the
                // entity's brushes as a single AABB, so two consoles on facing
                // walls of a room produce a room-spanning volume that fires on any
                // `use` press inside it — and the per-face clamp cannot catch it,
                // because every face of that union fronts open room.
                // `MAX_SWITCH_USE_REACH` bounds the margin, not the hull it is
                // added to. Same reasoning as `fog_volume` below.
                if brush_ids.len() > 1 {
                    anyhow::bail!(
                        "switch `{switch_name}` at {} owns {} brushes; the press volume is their \
                         union as one AABB, which spans the space between them and fires on any \
                         `use` press inside it — split into one switch per brush",
                        switch_location(),
                        brush_ids.len()
                    );
                }
                // A TrenchBroom field the author cleared arrives as `""`, not
                // absent. `use_reach` is the one numeric key the FGD invites
                // authors to edit, so an empty value falls back to the default
                // rather than failing the compile — the `_lightmap_density`
                // posture, reached by dropping the key before the parser sees it.
                if props
                    .get("use_reach")
                    .is_some_and(|value| value.trim().is_empty())
                {
                    props.remove("use_reach");
                }
                // Press-to-activate is what distinguishes `switch` from
                // `trigger_volume`, so any authored `activation` is discarded.
                // Warned rather than dropped silently: the realistic source is a
                // `trigger_volume` converted by editing its classname, and
                // TrenchBroom cannot surface the leftover key because `switch`
                // does not declare it. A *cleared* field leaves `""` behind rather
                // than an absent key, same as `use_reach` above — nothing was
                // authored, so warning would print an empty value back at the author.
                if let Some(authored) = props.insert("activation".to_string(), "use".to_string()) {
                    let authored = authored.trim();
                    if !authored.is_empty() {
                        log::warn!(
                            "[Compiler] switch `{switch_name}` ignores authored `activation` \
                             `{authored}`; a switch is always `use`-activated"
                        );
                    }
                }
                let use_reach = parse_optional_finite_f32(
                    &props,
                    "use_reach",
                    DEFAULT_SWITCH_USE_REACH,
                    &classname,
                    &switch_name,
                )?;
                // The floor is the flush tolerance, not zero. `apply_switch_use_reach`
                // discards per-face growth at or under that tolerance as float dust
                // from a contact plane, so any smaller margin grows nothing at all:
                // `use_reach "1e-38"` used to pass every check here and then compile
                // into a volume no larger than the solid brush, reported by the
                // diagnostic as clamped against geometry that was never there.
                // Derived from the tolerance and scale-correct — the tolerance is a
                // distance in engine meters, `use_reach` is authored in map units.
                //
                // Such a switch is not strictly unpressable: the runtime press test
                // measures from the capsule *axis*, so flush contact still reaches it
                // (the first-party capsule radius is 0.2 m ≈ 7.9 map units,
                // `content/dev/scripts/player.ts`). Pressable only by standing against
                // it is a typo or a cleared-to-zero field, not intent.
                let min_use_reach = FLUSH_TOLERANCE_METERS / scale;
                if (use_reach as f64) <= min_use_reach {
                    anyhow::bail!(
                        "switch `{switch_name}` at {} `use_reach` {use_reach} is at or below the \
                         {min_use_reach:.4} map-unit floor — a margin under the compiler's flush \
                         tolerance grows no face, leaving a press volume no larger than the solid \
                         brush, pressable only from flush contact",
                        switch_location()
                    );
                }
                if use_reach > MAX_SWITCH_USE_REACH {
                    anyhow::bail!(
                        "switch `{switch_name}` at {} `use_reach` {use_reach} exceeds the maximum \
                         {MAX_SWITCH_USE_REACH} map units — a press volume that large swallows \
                         the room around the switch",
                        switch_location()
                    );
                }
                let trigger = crate::trigger_volumes::resolve_trigger_volume(
                    &geo_map, &brush_ids, &props, scale, &classname,
                )?;
                // The reach margins are applied after this loop: how far a face
                // may grow depends on the static world brush set, which is still
                // being collected here.
                pending_switch_reach.push(PendingSwitchReach {
                    trigger_index: trigger_volumes.len(),
                    name: switch_name,
                    // `use_reach` is authored in map units; the AABB is already
                    // in engine meters, hence the scale.
                    margin: use_reach * scale as f32,
                    brush_ids: brush_ids.clone(),
                });
                let trigger_index = trigger_volumes.len();
                trigger_volumes.push(trigger);
                if let Some(assembly_index) = sibling_assembly_index {
                    assembly_members[assembly_index]
                        .trigger_indices
                        .push(trigger_index);
                }
                reflection_capture_overrides.record_brush_entity(
                    &props,
                    &format!("entity {} '{classname}'", entities.len() - 1),
                    &brush_ids,
                );
                world_brush_ids.extend(brush_ids.iter().copied());
                continue;
            }
            if classname == "kinematic_mover" {
                let props = collect_entity_properties(&geo_map, entity_id);
                let pending_mover_index = pending_kinematic_movers.len();
                pending_kinematic_movers.push(parse_kinematic_mover(&props, origin, brush_ids)?);
                if let Some(assembly_index) = sibling_assembly_index {
                    assembly_members[assembly_index]
                        .pending_mover_indices
                        .push(pending_mover_index);
                }
                continue;
            }
            if classname == "fog_volume" {
                if fog_volumes.len() >= MAX_FOG_VOLUMES {
                    log::warn!(
                        "[Compiler] {classname} cap reached ({MAX_FOG_VOLUMES}); skipping additional volume"
                    );
                    continue;
                }
                let props = collect_entity_properties(&geo_map, entity_id);
                if is_axis_aligned_brush_set(&geo_map, &brush_ids) {
                    let volume =
                        resolve_fog_ellipsoid(&geo_map, &brush_ids, &props, scale, &classname)?;
                    fog_volumes.push(volume);
                } else {
                    if brush_ids.len() > 1 {
                        anyhow::bail!(
                            "fog_volume entity must own exactly one brush (got {}); \
                             multi-brush volumes would silently produce a plane intersection \
                             rather than the union the author likely intended — split into \
                             separate fog_volume entities",
                            brush_ids.len()
                        );
                    }
                    let volume =
                        resolve_fog_volume(&geo_map, &brush_ids, &props, scale, &classname)?;
                    if let Some(v) = volume {
                        fog_volumes.push(v);
                    }
                }
            }
            if classname == "lightmap_scale_region" {
                if brush_ids.len() > 1 {
                    anyhow::bail!(
                        "lightmap_scale_region entity must own exactly one brush (got {}); \
                         multi-brush regions would silently produce a plane intersection rather \
                         than the union the author likely intended — split into separate regions",
                        brush_ids.len()
                    );
                }
                let props = collect_entity_properties(&geo_map, entity_id);
                let region =
                    resolve_lightmap_scale_region(&geo_map, &brush_ids, &props, scale, &classname)?;
                if let Some(region) = region {
                    lightmap_scale_regions.push(region);
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
                resolve_fog_lamp(&props, entity_origin, scale, &classname)?
            } else {
                resolve_fog_tube(&props, entity_origin, scale, &classname)?
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
        if classname == "kinematic_waypoint" {
            let name = props
                .get("name")
                .map(|value| value.trim().to_string())
                .unwrap_or_default();
            if name.is_empty() {
                anyhow::bail!(
                    "kinematic_waypoint at ({:.3}, {:.3}, {:.3}) missing required `name`",
                    entity_origin.x,
                    entity_origin.y,
                    entity_origin.z
                );
            }
            let next = props
                .get("next")
                .map(|value| value.trim().to_string())
                .unwrap_or_default();
            kinematic_waypoints.push(MapKinematicWaypoint {
                name,
                next,
                origin: entity_origin,
            });
            continue;
        }
        let diagnostic_ref = format!(
            "{classname} @ ({:.3}, {:.3}, {:.3})",
            entity_origin.x, entity_origin.y, entity_origin.z
        );
        let angles = quake_map::quake_to_engine_angles(&props, &diagnostic_ref);
        let tags: Vec<String> = props
            .get("_tags")
            .map(|s| s.split_whitespace().map(|t| t.to_string()).collect())
            .unwrap_or_default();
        if classname == attack_pools::ATTACK_POOL_CLASSNAME {
            attack_pools.record(entities.len() - 1, &props, &tags)?;
        }
        let mut key_values: Vec<(String, String)> = props
            .into_iter()
            .filter(|(k, _)| is_runtime_map_entity_key(k))
            .collect();
        // MapEntity persists this sequence; HashMap iteration order is randomized.
        key_values.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

        let map_entity_index = map_entities.len();
        map_entities.push(MapEntityRecord {
            classname: classname.clone(),
            origin: entity_origin,
            angles,
            key_values,
            tags,
        });
        if let Some(assembly_index) = sibling_assembly_index {
            assembly_members[assembly_index]
                .map_entity_indices
                .push(map_entity_index);
        }
    }

    #[cfg(debug_assertions)]
    debug_assert!(
        assembly_members
            .iter()
            .all(|members| members.indices_are_in_bounds(
                lights.len(),
                pending_kinematic_movers.len(),
                map_entities.len(),
                trigger_volumes.len(),
            ))
    );

    environment_volumes::attach_fluid_surface_sides(
        &geo_map,
        &mut environment_volumes,
        &environment_volume_brush_ids,
        scale,
    );
    let world_hulls = build_brush_volumes_with_ids(&geo_map, &world_brush_ids, scale);
    // Kinematic mover brushes never reach the world set, but a switch mounted on a
    // door or lift must still clamp against its mount. Their hulls exist only for
    // that clamp, so skip the extra hull build on maps without switches.
    let mover_hulls = if pending_switch_reach.is_empty() {
        Vec::new()
    } else {
        let mover_brush_ids: Vec<BrushId> = pending_kinematic_movers
            .iter()
            .flat_map(|mover| mover.brush_ids.iter().copied())
            .collect();
        build_brush_volumes_with_ids(&geo_map, &mover_brush_ids, scale)
    };
    apply_switch_use_reach(
        &mut trigger_volumes,
        &pending_switch_reach,
        &world_hulls,
        &mover_hulls,
    );
    // `build_brush_volumes_with_ids` drops degenerate brushes, so derive the
    // provenance table from its retained ids rather than from the authored
    // brush list position. `Face::brush_index` is this resulting vector's
    // enumerate index.
    let brush_assembly = brush_assembly_for_retained_hulls(&world_hulls, &assembly_by_brush_id);
    let brush_capture_override = reflection_capture_overrides.for_retained_hulls(&world_hulls);
    let brush_volumes: Vec<BrushVolume> =
        world_hulls.into_iter().map(|(_, volume)| volume).collect();
    debug_assert_eq!(brush_volumes.len(), brush_assembly.len());
    debug_assert_eq!(brush_volumes.len(), brush_capture_override.len());
    let total_vertex_count: usize = brush_volumes
        .iter()
        .flat_map(|brush| brush.sides.iter())
        .map(|side| side.vertices.len())
        .sum();
    let total_side_count: usize = brush_volumes.iter().map(|brush| brush.sides.len()).sum();

    synthesize_grouped_light_carriers(
        &assemblies,
        &assembly_members,
        &pending_kinematic_movers,
        &mut lights,
    );
    let kinematic_movers = resolve_kinematic_movers(
        &geo_map,
        pending_kinematic_movers,
        &kinematic_waypoints,
        scale,
    )?;
    let carried_light_links = resolve_carried_light_links(&mut lights, &kinematic_movers);

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
    log::info!("Environment volumes: {}", environment_volumes.len());
    log::info!("Reflection volumes: {}", reflection_volumes.len());
    let ghost_links = ghost_links.finish(teleporters.destinations())?;

    Ok(MapData {
        brush_volumes,
        assemblies,
        brush_assembly,
        brush_capture_override,
        entity_brushes: entity_brushes_summary,
        entities,
        lights,
        carried_light_links,
        light_start_active_defaults,
        light_source_labels,
        data_script,
        map_entities,
        kinematic_movers,
        kinematic_waypoints,
        trigger_volumes,
        environment_volumes,
        reflection_volumes,
        teleporters: teleporters.finish()?,
        ghost_links,
        sh_protect_aabbs,
        uniform_grid_optout,
        fog_volumes,
        lightmap_scale_regions,
        streaming_seam_regions,
        stream_resident_regions,
        stream_priority_regions,
        fog_pixel_scale,
        initial_gravity,
        lightmap_density,
        sh_density_fidelity,
        bounce_fill,
        nav_params,
        entity_shadow_params,
    })
}

/// Fill eligible dynamic-light carrier names from unambiguous editor assemblies.
///
/// This deliberately stops at the canonical carrier name. The existing resolver
/// below remains the only place that derives offsets, applies degradation, and
/// constructs carried-light links.
fn synthesize_grouped_light_carriers(
    assemblies: &[MapAssembly],
    assembly_members: &[AssemblyMembers],
    pending_movers: &[PendingKinematicMover],
    lights: &mut [MapLight],
) {
    debug_assert_eq!(assemblies.len(), assembly_members.len());

    for (assembly, members) in assemblies.iter().zip(assembly_members) {
        let eligible_light_indices: Vec<usize> = members
            .light_indices
            .iter()
            .copied()
            .filter(|&index| {
                let light = &lights[index];
                light.is_dynamic && !light.bake_only && light.carrier.is_empty()
            })
            .collect();

        match members.pending_mover_indices.as_slice() {
            [] => {}
            [mover_index] => {
                let mover = &pending_movers[*mover_index];
                if mover.name.is_empty() {
                    if !eligible_light_indices.is_empty() {
                        log::warn!(
                            "[Compiler] group `{}` (id `{}`) contains an eligible dynamic light but its only kinematic_mover has no name; grouped carry needs a named mover",
                            assembly.provenance,
                            assembly.group_id,
                        );
                    }
                    continue;
                }

                for &light_index in &members.light_indices {
                    let light = &lights[light_index];
                    let names_another_mover = light.is_dynamic
                        && !light.bake_only
                        && !light.carrier.is_empty()
                        && light.carrier != mover.name
                        && pending_movers
                            .iter()
                            .any(|other| other.name == light.carrier);
                    if names_another_mover {
                        log::warn!(
                            "[Compiler] group `{}` (id `{}`) contains a dynamic light with explicit carrier `{}` instead of its grouped kinematic_mover `{}`; explicit carrier wins",
                            assembly.provenance,
                            assembly.group_id,
                            light.carrier,
                            mover.name,
                        );
                    }
                }

                for light_index in eligible_light_indices {
                    lights[light_index].carrier.clone_from(&mover.name);
                }
            }
            mover_indices => {
                if eligible_light_indices.is_empty() {
                    continue;
                }

                let movers = mover_indices
                    .iter()
                    .map(|&index| {
                        let mover = &pending_movers[index];
                        format!("`{}` (member {index})", mover.name)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                log::warn!(
                    "[Compiler] group `{}` (id `{}`) contains eligible dynamic lights but multiple kinematic_movers {movers}; grouped carry is ambiguous",
                    assembly.provenance,
                    assembly.group_id,
                );
            }
        }
    }
}

/// Resolve dynamic-light carrier names after all movers are available.
///
/// Carrier bindings are presentation-only authoring, so malformed bindings warn
/// and leave the light unbound rather than failing the map build. Baked lights
/// cannot carry because their contribution is already fixed in the bake.
fn resolve_carried_light_links(
    lights: &mut [MapLight],
    movers: &[MapKinematicMover],
) -> Vec<crate::map_data::CarriedLightLink> {
    let mut links = Vec::new();

    for (source_light_index, light) in lights.iter_mut().enumerate() {
        if light.carrier.is_empty() {
            continue;
        }

        if light.is_dynamic && light.bake_only {
            log::warn!(
                "[Compiler] dynamic bake-only light at {:?} ignores carrier `{}`; bake-only lights have no runtime presence and cannot be carried",
                light.origin,
                light.carrier,
            );
            light.carrier.clear();
            continue;
        }

        if !light.is_dynamic {
            log::warn!(
                "[Compiler] baked light at {:?} ignores carrier `{}`; baked lights cannot be carried",
                light.origin,
                light.carrier,
            );
            light.carrier.clear();
            continue;
        }

        let matching_movers: Vec<_> = movers
            .iter()
            .filter(|mover| mover.name == light.carrier)
            .collect();
        let mover = match matching_movers.as_slice() {
            [] => {
                log::warn!(
                    "[Compiler] dynamic light at {:?} carrier `{}` matches no kinematic_mover; leaving it unbound",
                    light.origin,
                    light.carrier,
                );
                continue;
            }
            [mover] => *mover,
            movers => {
                let duplicate_movers = movers
                    .iter()
                    .map(|mover| format!("`{}` (id {})", mover.name, mover.mover_id))
                    .collect::<Vec<_>>()
                    .join(", ");
                log::warn!(
                    "[Compiler] dynamic light at {:?} carrier `{}` matches duplicate kinematic_movers {duplicate_movers}; leaving it unbound",
                    light.origin,
                    light.carrier,
                );
                continue;
            }
        };

        if light.light_type == LightType::Spot && mover.spin_axis != [0.0; 3] {
            log::warn!(
                "[Compiler] dynamic spot light at {:?} carrier `{}` targets spinner-capable kinematic_mover `{}` (id {}); cone re-aim under rotation is deferred, carrying position only",
                light.origin,
                light.carrier,
                mover.name,
                mover.mover_id,
            );
        }

        let derived_offset = light.origin - mover.origin;
        let local_offset = derived_offset.to_array().map(|value| value as f32);
        if !local_offset.iter().all(|component| component.is_finite()) {
            log::warn!(
                "[Compiler] dynamic light at {:?} carrier `{}` produces local offset {derived_offset:?} outside the runtime f32 range; leaving it unbound",
                light.origin,
                light.carrier,
            );
            continue;
        }

        links.push(crate::map_data::CarriedLightLink {
            source_light_index,
            mover_id: mover.mover_id,
            local_offset,
        });
    }

    links
}

/// Brush hulls for `brush_ids`, with the ids dropped.
/// See [`build_brush_volumes_with_ids`].
fn build_brush_volumes(geo_map: &GeoMap, brush_ids: &[BrushId], scale: f64) -> Vec<BrushVolume> {
    build_brush_volumes_with_ids(geo_map, brush_ids, scale)
        .into_iter()
        .map(|(_, volume)| volume)
        .collect()
}

/// Face geometry, planes, and AABB per brush, each paired with its brush id.
///
/// The pairing is not positional: a brush with no faces or no planes yields no
/// volume, so the output can be shorter than the input. A caller that has to
/// identify one specific brush's hull — switch reach clamping, excluding the
/// switch's own brushes — needs the id, not an index into the world brush list.
fn build_brush_volumes_with_ids(
    geo_map: &GeoMap,
    brush_ids: &[BrushId],
    scale: f64,
) -> Vec<(BrushId, BrushVolume)> {
    let geo_planes = face_planes(&geo_map.face_planes);
    let brush_faces: BTreeMap<BrushId, Vec<shambler::face::FaceId>> = brush_ids
        .iter()
        .filter_map(|bid| {
            geo_map
                .brush_faces
                .get(bid)
                .map(|faces| (*bid, faces.clone()))
        })
        .collect();
    let brush_hulls = brush_hulls(&brush_faces, &geo_planes);
    let (face_verts, _face_vert_planes) = face_vertices(&brush_faces, &geo_planes, &brush_hulls);
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

    let mut brush_volumes = Vec::new();
    for brush_id in brush_ids {
        let face_ids = match geo_map.brush_faces.get(brush_id) {
            Some(ids) => ids,
            None => continue,
        };

        let planes: Vec<BrushPlane> = face_ids
            .iter()
            .filter_map(|fid| {
                let plane = geo_planes.get(fid)?;
                Some(BrushPlane {
                    normal: quake_to_engine(shambler_to_dvec3(plane.normal())),
                    distance: plane.distance() as f64 * scale,
                })
            })
            .collect();

        if planes.is_empty() {
            continue;
        }

        let mut aabb = crate::partition::Aabb::empty();
        for fid in face_ids {
            if let Some(verts) = face_verts.get(fid) {
                for v in verts {
                    aabb.expand_point(quake_to_engine(shambler_to_dvec3(v)) * scale);
                }
            }
        }

        let mut sides = Vec::with_capacity(face_ids.len());
        for face_id in face_ids {
            let vertices_raw = match face_verts.get(face_id) {
                Some(v) => v,
                None => continue,
            };
            if vertices_raw.len() < 3 {
                continue;
            }
            let indices = match face_idx.get(face_id) {
                Some(i) => i,
                None => continue,
            };
            let vertices: Vec<DVec3> = indices
                .iter()
                .map(|&i| quake_to_engine(shambler_to_dvec3(&vertices_raw[i])) * scale)
                .collect();

            let plane = &geo_planes[face_id];
            let normal = quake_to_engine(shambler_to_dvec3(plane.normal()));
            let distance = plane.distance() as f64 * scale;
            let texture = geo_map
                .face_textures
                .get(face_id)
                .and_then(|tex_id| geo_map.textures.get(tex_id))
                .map(|name| decode_brush_texture(name))
                .unwrap_or_else(|| "unknown".to_string());

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
                None => TextureProjection::default(),
            };

            sides.push(BrushSide {
                vertices,
                normal,
                distance,
                texture,
                tex_projection,
            });
        }

        brush_volumes.push((
            *brush_id,
            BrushVolume {
                planes,
                sides,
                aabb,
            },
        ));
    }

    brush_volumes
}

/// Translate authored brush membership only for hulls that survived parsing.
/// Keeping this at the retained-`BrushId` seam prevents an omitted degenerate
/// brush from shifting diagnostic provenance onto a later brush.
fn brush_assembly_for_retained_hulls(
    retained_hulls: &[(BrushId, BrushVolume)],
    assembly_by_brush_id: &HashMap<BrushId, usize>,
) -> Vec<Option<usize>> {
    retained_hulls
        .iter()
        .map(|(brush_id, _)| assembly_by_brush_id.get(brush_id).copied())
        .collect()
}

/// Whether `hull` may intrude into the axis-aligned box `box_min..box_max`.
///
/// One-sided convex test: `false` only when a single one of the brush's own outward
/// planes puts the whole box strictly in front of it. Same separating-plane form as
/// `partition::region_polytope`'s `all_vertices_behind`, evaluated on the box's
/// support point rather than eight enumerated corners.
///
/// **Conservative, not exact.** Only the brush's own planes are tried, so a box that
/// survives all of them may still miss the brush — a separating plane can be an
/// edge-cross axis this test never forms. The error is one-directional by design: a
/// false `true` costs a switch face some reach, a false `false` would put a pressable
/// volume inside solid. A brush with no planes yields `true`.
fn hull_may_reach_box(hull: &BrushVolume, box_min: DVec3, box_max: DVec3) -> bool {
    !hull.planes.iter().any(|plane| {
        // Box corner nearest the brush interior along this plane's normal. Planes
        // face outward, so the interior is `v · n <= d`.
        let nearest = DVec3::new(
            if plane.normal.x >= 0.0 {
                box_min.x
            } else {
                box_max.x
            },
            if plane.normal.y >= 0.0 {
                box_min.y
            } else {
                box_max.y
            },
            if plane.normal.z >= 0.0 {
                box_min.z
            } else {
                box_max.z
            },
        );
        nearest.dot(plane.normal) - plane.distance > FLUSH_TOLERANCE_METERS
    })
}

/// Grow each switch's trigger AABB toward its reach margin, clamping every face to
/// the free space in front of it.
///
/// Use-activation is a capsule-vs-AABB intersection test and the switch brush is
/// solid, so the volume has to reach past the switch face into the space the
/// player stands in. Growing a face blindly reaches *through* whatever stands
/// behind it: `capsule_overlaps_aabb` measures from the capsule axis, so a face's
/// effective reach is `margin + capsule_radius` — the default margin plus the
/// first-party 0.2 m capsule (`content/dev/scripts/player.ts`, ~7.9 map units)
/// clears a standard 16-unit wall with room to spare. Hence the invariant this
/// function holds:
///
/// > A grown face never extends past the near side of any occluder the compiler
/// > could not rule out of the corridor that face grows into.
///
/// Each face grows by `min(margin, free_distance)`, so a flush-mounted face (free
/// distance zero) does not grow at all and a face 4 units off its mount grows 4.
///
/// Occluders are the static world brushes plus the kinematic mover brushes, minus
/// the switch's own. A hull qualifies for one face when all three of these hold:
///
/// 1. Its AABB overlaps that face's cross-section by **positive area**. Edge-on
///    contact is not overlap — a flush mount touches four of a console's faces that
///    way, and those four front open room.
/// 2. Its AABB stands past the face on the growth axis, by more than
///    [`FLUSH_TOLERANCE_METERS`]. A mount whose own plane is coplanar with the face
///    has nothing in front of it, and which way the trigger's `f32` bounds rounded
///    must not decide that.
/// 3. No single one of the brush's planes separates the **growth prism** — the
///    face's cross-section extended along the growth axis out to the full margin.
///
/// The first two are cheap AABB rejects; the third is [`hull_may_reach_box`]. An
/// AABB alone is not a usable proxy for its brush: a diagonal partition, wedge,
/// ramp, one-brush staircase, or 45° chamfer 100 units away can have an AABB that
/// straddles the switch on every axis and overlaps every cross-section, which zeroed
/// all six faces against geometry that was never in front of them. The plane test is
/// conservative (see [`hull_may_reach_box`]), so the result is *not* exact — it can
/// still keep an occluder a finer test would drop, which costs reach rather than
/// leaking one.
///
/// Partial overlap of a face's cross-section clamps as readily as full overlap:
/// under-growing is an authoring annoyance, over-growing is a press-through-wall
/// bug. With an AABB trigger there is no way to grow *part* of a face, so a console
/// sunk one unit into its mount cannot grow horizontally at all — zeroing is the
/// correct answer, but a silent one. The author hears about it when **no horizontal
/// face grew**: a player presses from a standing position and reaches sideways, so a
/// volume that grew only upward is reachable from flush contact or directly above
/// and nowhere a player stands. A flush wall mount zeroes exactly one horizontal
/// face and stays silent under that rule, which is why the rule is not "any face was
/// zeroed". Reported as one aggregate `warn!` plus a bounded number of per-switch
/// detail lines.
///
/// What the invariant does **not** cover:
/// - Mover hulls are taken at their **authored (compile-time)** position. A
///   `kinematic_mover` authored *clear* of the switch that later moves *into* the
///   corridor — a blast door authored open and closed by this very switch — is no
///   occluder at compile time, so the face grows fully and at runtime the volume
///   sits behind a closed solid. The opposite case (authored across the corridor,
///   later moves away) only costs reach, which is the safe direction.
/// - Faces are clamped against the *ungrown* cross-section, one axis at a time. An
///   occluder that overlaps no face's cross-section but does sit in the grown box's
///   corner (diagonally adjacent geometry) is not accounted for.
/// - A brush that produced no usable vertices has an empty AABB and clamps
///   nothing; it is not treated as an occluder.
fn apply_switch_use_reach(
    triggers: &mut [MapTriggerVolume],
    pending: &[PendingSwitchReach],
    static_hulls: &[(BrushId, BrushVolume)],
    mover_hulls: &[(BrushId, BrushVolume)],
) {
    // Face indices into the per-face arrays below, which are `[face][axis]`.
    const LOW: usize = 0;
    const HIGH: usize = 1;
    // Engine space is Y-up, so a player standing on the floor reaches along X and Z.
    const HORIZONTAL_AXES: [usize; 2] = [0, 2];
    const AXIS_LABELS: [char; 3] = ['X', 'Y', 'Z'];
    // Detail lines are capped the same way the animated-light chunk cap is: one
    // aggregate line always, per-switch detail only until the log would drown.
    const MAX_DETAIL_LOG_LINES: usize = 8;

    let mut blocked_switches = 0usize;
    let mut detail_lines = 0usize;

    for switch in pending {
        let trigger = &mut triggers[switch.trigger_index];
        let min = DVec3::from_array(trigger.aabb_min.map(|v| v as f64));
        let max = DVec3::from_array(trigger.aabb_max.map(|v| v as f64));
        let margin = switch.margin as f64;
        // Per-face growth allowance, starting at the authored margin and clamped
        // down by each occluder standing in front of that face, plus the occluder
        // plane that last reduced it — the diagnostic below needs to name it.
        let mut growth = [[margin; 3]; 2];
        let mut blocked_at: [[Option<f64>; 3]; 2] = [[None; 3]; 2];

        for (brush_id, hull) in static_hulls.iter().chain(mover_hulls.iter()) {
            if switch.brush_ids.contains(brush_id) {
                continue;
            }
            let occluder = &hull.aabb;
            for axis in 0..3 {
                // Overlap of the two axes the face spans, by positive area: an
                // occluder merely abutting the switch's side sits outside this
                // face's footprint and cannot be reached through by growing it.
                // That is the flush-mount case — the wall touches four of the
                // console's faces edge-on — so counting contact as overlap would
                // zero the margin on all four.
                let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
                let overlaps_cross_section = occluder.max[b] > min[b] + FLUSH_TOLERANCE_METERS
                    && occluder.min[b] < max[b] - FLUSH_TOLERANCE_METERS
                    && occluder.max[c] > min[c] + FLUSH_TOLERANCE_METERS
                    && occluder.min[c] < max[c] - FLUSH_TOLERANCE_METERS;
                if !overlaps_cross_section {
                    continue;
                }

                for face in [LOW, HIGH] {
                    // Does the occluder stand past this face, and if so how much
                    // free gap is left? A negative gap means it already overlaps the
                    // hull on this axis: clamp to zero rather than shrink the volume.
                    // The tolerance is the same one the cross-section test uses, and
                    // for the same reason — a coplanar mount must not read as
                    // standing past the face because f32 rounded up.
                    let (stands_past, near_side, allowance) = if face == LOW {
                        (
                            occluder.min[axis] < min[axis] - FLUSH_TOLERANCE_METERS,
                            occluder.max[axis],
                            (min[axis] - occluder.max[axis]).max(0.0),
                        )
                    } else {
                        (
                            occluder.max[axis] > max[axis] + FLUSH_TOLERANCE_METERS,
                            occluder.min[axis],
                            (occluder.min[axis] - max[axis]).max(0.0),
                        )
                    };
                    if !stands_past || allowance >= growth[face][axis] {
                        continue;
                    }
                    // The AABB says "maybe"; the brush's own planes get the last
                    // word, against the prism this face wants to grow into.
                    let mut prism_min = min;
                    let mut prism_max = max;
                    if face == LOW {
                        prism_min[axis] = min[axis] - margin;
                        prism_max[axis] = min[axis];
                    } else {
                        prism_min[axis] = max[axis];
                        prism_max[axis] = max[axis] + margin;
                    }
                    if !hull_may_reach_box(hull, prism_min, prism_max) {
                        continue;
                    }
                    growth[face][axis] = allowance;
                    blocked_at[face][axis] = Some(near_side);
                }
            }
        }

        // Growth under the flush tolerance is float dust from a contact plane, not
        // clearance; dropping it keeps the diagnostics below honest.
        let mut grew = [[false; 3]; 2];
        for axis in 0..3 {
            if growth[LOW][axis] > FLUSH_TOLERANCE_METERS {
                trigger.aabb_min[axis] -= growth[LOW][axis] as f32;
                grew[LOW][axis] = true;
            }
            if growth[HIGH][axis] > FLUSH_TOLERANCE_METERS {
                trigger.aabb_max[axis] += growth[HIGH][axis] as f32;
                grew[HIGH][axis] = true;
            }
        }

        if HORIZONTAL_AXES
            .iter()
            .any(|&axis| grew[LOW][axis] || grew[HIGH][axis])
        {
            continue;
        }
        blocked_switches += 1;
        if detail_lines >= MAX_DETAIL_LOG_LINES {
            continue;
        }
        detail_lines += 1;
        // Report what the compiler concluded, not what the room contains: the plane
        // test is conservative, so "clamped against geometry in front of it" is
        // defensible where "enclosed in solid" would not be.
        let clamped: Vec<String> = HORIZONTAL_AXES
            .iter()
            .flat_map(|&axis| [(LOW, axis), (HIGH, axis)])
            .map(|(face, axis)| {
                let sign = if face == LOW { '-' } else { '+' };
                let label = AXIS_LABELS[axis];
                match blocked_at[face][axis] {
                    Some(plane) => format!("{sign}{label} against solid at {plane:.3} m"),
                    None => format!("{sign}{label} (margin under the flush tolerance)"),
                }
            })
            .collect();
        let centre = (DVec3::from_array(trigger.aabb_min.map(|v| v as f64))
            + DVec3::from_array(trigger.aabb_max.map(|v| v as f64)))
            * 0.5;
        log::warn!(
            "[Compiler] switch `{}` at ({:.3}, {:.3}, {:.3}) m {}; horizontal faces clamped: {}",
            switch.name,
            centre.x,
            centre.y,
            centre.z,
            if grew.iter().flatten().any(|&g| g) {
                "grew only vertically, so it is pressable from flush contact or directly above \
                 and nowhere a player can stand"
            } else {
                "grew no face at all, so it is pressable only from flush contact"
            },
            clamped.join(", "),
        );
    }

    if blocked_switches > 0 {
        log::warn!(
            "[Compiler] {blocked_switches} of {} switches grew no horizontal press margin — every \
             horizontal face was clamped against geometry standing in front of it. A switch sunk \
             into its mount cannot grow sideways at all: an AABB trigger has no way to grow part \
             of a face, so clear the switch of its mount on at least one horizontal side",
            pending.len(),
        );
    }
}

fn resolve_kinematic_movers(
    geo_map: &GeoMap,
    pending_movers: Vec<PendingKinematicMover>,
    waypoints: &[MapKinematicWaypoint],
    scale: f64,
) -> anyhow::Result<Vec<MapKinematicMover>> {
    validate_kinematic_waypoints(waypoints)?;
    let waypoint_by_name: HashMap<&str, &MapKinematicWaypoint> = waypoints
        .iter()
        .map(|waypoint| (waypoint.name.as_str(), waypoint))
        .collect();
    let mut reached_waypoints = HashSet::new();
    let mut movers = Vec::with_capacity(pending_movers.len());

    for pending in pending_movers {
        let path = resolve_kinematic_path(&pending, &waypoint_by_name)?;
        for waypoint_name in &path {
            reached_waypoints.insert(waypoint_name.clone());
        }
        let first = waypoint_by_name[pending.path.as_str()];
        if let Some(authored_origin) = pending.authored_origin {
            let delta = authored_origin - first.origin;
            if delta.length() > 0.001 {
                log::warn!(
                    "[Compiler] kinematic_mover `{}` authored origin {:?} differs from first waypoint `{}` at {:?}",
                    pending.name,
                    authored_origin,
                    first.name,
                    first.origin
                );
            }
        }

        let brush_volumes = build_brush_volumes(geo_map, &pending.brush_ids, scale);
        movers.push(MapKinematicMover {
            mover_id: movers.len() as u32,
            name: pending.name,
            tags: pending.tags,
            origin: first.origin,
            authored_origin: pending.authored_origin,
            path: pending.path,
            speed: pending.speed,
            wait_ms: pending.wait_ms,
            spin_axis: pending.spin_axis,
            spin_speed_deg_s: pending.spin_speed_deg_s,
            spin_accel_deg_s2: pending.spin_accel_deg_s2,
            carry_yaw: pending.carry_yaw,
            block_policy: pending.block_policy,
            crush_damage: pending.crush_damage,
            crush_interval_ms: pending.crush_interval_ms,
            auto_close_ms: pending.auto_close_ms,
            open_event: pending.open_event,
            close_event: pending.close_event,
            blocked_event: pending.blocked_event,
            crush_event: pending.crush_event,
            open_sound: pending.open_sound,
            close_sound: pending.close_sound,
            blocked_sound: pending.blocked_sound,
            crush_sound: pending.crush_sound,
            crush_damage_type: pending.crush_damage_type,
            light_ramp_start: pending.light_ramp_start,
            light_ramp_end: pending.light_ramp_end,
            light_bounces: pending.light_bounces,
            light_poses: pending.light_poses,
            move_mode: pending.move_mode,
            start_on_spawn: pending.start_on_spawn,
            brush_volumes,
        });
    }

    for waypoint in waypoints {
        if !reached_waypoints.contains(&waypoint.name) {
            log::warn!(
                "[Compiler] kinematic_waypoint `{}` is not reached by any kinematic_mover path",
                waypoint.name
            );
        }
    }

    Ok(movers)
}

fn validate_kinematic_waypoints(waypoints: &[MapKinematicWaypoint]) -> anyhow::Result<()> {
    let mut names = HashSet::new();
    for waypoint in waypoints {
        if !waypoint.origin.is_finite() {
            anyhow::bail!(
                "kinematic_waypoint `{}` origin is non-finite: {:?}",
                waypoint.name,
                waypoint.origin
            );
        }
        if !names.insert(waypoint.name.as_str()) {
            anyhow::bail!("duplicate kinematic_waypoint name `{}`", waypoint.name);
        }
    }
    Ok(())
}

fn resolve_kinematic_path(
    mover: &PendingKinematicMover,
    waypoint_by_name: &HashMap<&str, &MapKinematicWaypoint>,
) -> anyhow::Result<Vec<String>> {
    let mut path: Vec<String> = Vec::new();
    let mut current = mover.path.as_str();
    let mut seen = HashSet::new();

    loop {
        if !seen.insert(current.to_string()) {
            anyhow::bail!(
                "kinematic_mover `{}` path `{}` contains waypoint cycle at `{current}`",
                mover.name,
                mover.path
            );
        }
        let Some(waypoint) = waypoint_by_name.get(current) else {
            anyhow::bail!(
                "kinematic_mover `{}` path references unknown waypoint `{current}`",
                mover.name
            );
        };
        if let Some(previous_name) = path.last() {
            let previous = waypoint_by_name[previous_name.as_str()];
            if (previous.origin - waypoint.origin).length()
                <= KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH as f64
            {
                anyhow::bail!(
                    "kinematic_mover `{}` path has zero-length segment between waypoints `{}` and `{}`",
                    mover.name,
                    previous.name,
                    waypoint.name
                );
            }
        }
        path.push(waypoint.name.clone());
        if waypoint.next.trim().is_empty() {
            break;
        }
        current = waypoint.next.as_str();
    }

    if path.len() < 2 && mover.spin_speed_deg_s == 0.0 {
        anyhow::bail!(
            "kinematic_mover `{}` path `{}` resolves to fewer than two waypoints without non-zero spin_speed",
            mover.name,
            mover.path
        );
    }

    Ok(path)
}

/// True iff every face plane in `brush_ids` has a normal within `EPS` of a
/// cardinal axis (±X, ±Y, ±Z) in Quake space — equivalent to engine space for
/// this test, since the swizzle permutes and sign-flips axes but preserves
/// the "axis-aligned" property. Empty brush sets return `false` so callers
/// fall through to the plane-bounded path (which surfaces the empty-brush
/// error from the resolver).
fn is_axis_aligned_brush_set(geo_map: &GeoMap, brush_ids: &[shambler::brush::BrushId]) -> bool {
    use shambler::face::face_planes;
    if brush_ids.is_empty() {
        return false;
    }
    // 1° of slop (cos ≈ 0.99985). Tighter than typical authoring drift; loose
    // enough to forgive grid-snapped brushes whose plane math accumulated a
    // sub-degree error in shambler's f32 path.
    const EPS: f64 = 1.0e-3;
    let geo_planes = face_planes(&geo_map.face_planes);
    let mut saw_face = false;
    for bid in brush_ids {
        let face_ids = match geo_map.brush_faces.get(bid) {
            Some(ids) => ids,
            None => continue,
        };
        for fid in face_ids {
            let plane = match geo_planes.get(fid) {
                Some(p) => p,
                None => continue,
            };
            saw_face = true;
            let n = shambler_to_dvec3(plane.normal());
            let ax = n.x.abs();
            let ay = n.y.abs();
            let az = n.z.abs();
            let on_x = (ax - 1.0).abs() < EPS && ay < EPS && az < EPS;
            let on_y = (ay - 1.0).abs() < EPS && ax < EPS && az < EPS;
            let on_z = (az - 1.0).abs() < EPS && ax < EPS && ay < EPS;
            if !(on_x || on_y || on_z) {
                return false;
            }
        }
    }
    saw_face
}

/// Re-export `quake_to_engine` for cross-module tests (geometry round-trip).
#[cfg(test)]
pub fn quake_to_engine_for_test(v: DVec3) -> DVec3 {
    quake_to_engine(v)
}

#[cfg(test)]
#[path = "parse/tests.rs"]
mod tests;
