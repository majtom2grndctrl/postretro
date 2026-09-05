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
    MapKinematicWaypoint, MapLight, MapTriggerVolume, NavParams, TextureProjection,
};
use crate::map_format::MapFormat;
use postretro_level_format::fog_volumes::{
    DEFAULT_WORLD_GRAVITY_MPS2, MAX_FOG_VOLUMES, MAX_PLANES_PER_VOLUME,
};
use postretro_level_format::kinematic_geometry::KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH;

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
        move_mode,
        start_on_spawn,
        brush_ids,
    })
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
    let parsed: f32 = raw.trim().parse().map_err(|e| {
        anyhow::anyhow!("{classname} `{name}` `{key}` value `{raw}` is not a valid float: {e}")
    })?;
    if !parsed.is_finite() {
        anyhow::bail!("{classname} `{name}` `{key}` value `{raw}` is not finite");
    }
    Ok(parsed)
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
    let map_text = encode_quoted_brush_textures(&raw_map_text);

    let shalrath_map: shambler::shalrath::repr::Map = map_text
        .parse()
        .map_err(|e| anyhow::anyhow!("failed to parse .map syntax: {e}"))?;

    let geo_map = GeoMap::new(shalrath_map);

    // Recognize editor-group markers before the entity walk. Sibling members
    // may appear before their marker in source order, but static brush
    // flattening remains in the existing walk below so its geometry ordering is
    // unchanged.
    let mut assemblies = Vec::new();
    let mut assembly_by_group_id = HashMap::new();
    for entity_id in geo_map.entities.iter() {
        let classname = get_property(&geo_map, entity_id, "classname");
        if classname.as_deref() != Some("func_group") {
            continue;
        }

        let assembly = map_assembly_from_marker(&geo_map, entity_id);
        let assembly_index = assemblies.len();
        assembly_by_group_id
            .entry(assembly.group_id.clone())
            .or_insert(assembly_index);
        assemblies.push(assembly);
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

    // Collect entity info and entity brush counts
    let mut entities = Vec::new();
    let mut entity_brushes_summary = Vec::new();
    let mut entity_classnames: Vec<String> = Vec::new();
    // Lights translate to the canonical format as we walk entities — they
    // share the origin/classname extraction but do not participate in BSP
    // construction.
    let mut lights: Vec<MapLight> = Vec::new();
    let mut light_start_active_defaults = Vec::new();
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
    let mut trigger_volumes = Vec::new();
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
    // the documented Earth-gravity default; supplied values remain strict so
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
            .map(|v| quake_to_engine(v) * scale);

        entities.push(EntityInfo {
            classname: classname.clone(),
            origin,
        });

        // Sibling members identify a marker through `_tb_group`; capture that
        // source relation before the branches below translate or strip their
        // property bags. A missing marker is ordinary malformed authoring data
        // here: it simply has no assembly association.
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
                let name = props
                    .get("name")
                    .map(|v| v.trim().to_owned())
                    .unwrap_or_default();
                // Same brush-hull → world-AABB union `trigger_volume` uses; a
                // protection volume needs only the enclosing box, none of the
                // trigger's activation/target data.
                let (mut min, mut max) = crate::trigger_volumes::resolve_brush_entity_aabb(
                    &geo_map, &brush_ids, scale, &classname, &name,
                )?;
                // Optional `dilation` margin, expanding the box on all six faces
                // so a probe just outside the authored brushwork is still
                // protected. Default 0.0; negatives are an authoring error (they
                // would shrink the volume). In world units — the same engine
                // space the AABB and the CLI `--sh-protect-aabb` boxes live in.
                let dilation =
                    parse_optional_finite_f32(&props, "dilation", 0.0, &classname, &name)?;
                if dilation < 0.0 {
                    anyhow::bail!(
                        "{classname} `{name}` `dilation` must be non-negative, got {dilation}"
                    );
                }
                let d = dilation as f64;
                min -= DVec3::splat(d);
                max += DVec3::splat(d);
                let min = min.to_array().map(|v| v as f32);
                let max = max.to_array().map(|v| v as f32);
                sh_protect_aabbs.push([min[0], min[1], min[2], max[0], max[1], max[2]]);
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
    let brush_assembly: Vec<Option<usize>> = world_hulls
        .iter()
        .map(|(brush_id, _)| assembly_by_brush_id.get(brush_id).copied())
        .collect();
    let brush_volumes: Vec<BrushVolume> =
        world_hulls.into_iter().map(|(_, volume)| volume).collect();
    debug_assert_eq!(brush_volumes.len(), brush_assembly.len());
    let total_vertex_count: usize = brush_volumes
        .iter()
        .flat_map(|brush| brush.sides.iter())
        .map(|side| side.vertices.len())
        .sum();
    let total_side_count: usize = brush_volumes.iter().map(|brush| brush.sides.len()).sum();

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

    Ok(MapData {
        brush_volumes,
        assemblies,
        brush_assembly,
        entity_brushes: entity_brushes_summary,
        entities,
        lights,
        carried_light_links,
        light_start_active_defaults,
        data_script,
        map_entities,
        kinematic_movers,
        kinematic_waypoints,
        trigger_volumes,
        sh_protect_aabbs,
        uniform_grid_optout,
        fog_volumes,
        fog_pixel_scale,
        initial_gravity,
        lightmap_density,
        sh_density_fidelity,
        nav_params,
        entity_shadow_params,
    })
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
fn resolve_fog_ellipsoid(
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
fn resolve_fog_lamp(
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
fn resolve_fog_tube(
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

/// Re-export `quake_to_engine` for cross-module tests (geometry round-trip).
#[cfg(test)]
pub fn quake_to_engine_for_test(v: DVec3) -> DVec3 {
    quake_to_engine(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use log::Level;
    use postretro_test_log_capture::LogCapture;

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
            .join("content/dev/maps/campaign-test.map")
    }

    fn parse_inline_map(map_text: &str) -> anyhow::Result<MapData> {
        static NEXT_INLINE_MAP_ID: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let id = NEXT_INLINE_MAP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "postretro-inline-map-{}-{id}.map",
            std::process::id(),
        ));
        std::fs::write(&path, format!("{}\n", map_text.trim()))?;
        let result = parse_map_file(&path, MapFormat::IdTech2);
        let _ = std::fs::remove_file(&path);
        result
    }

    fn kinematic_test_map(path_next: &str) -> String {
        kinematic_test_map_with_nexts(path_next, "")
    }

    fn kinematic_map_with_light(
        map: String,
        classname: &str,
        origin: &str,
        carrier: Option<&str>,
        extra_properties: &str,
    ) -> String {
        let carrier_property = carrier
            .map(|carrier| format!("\"carrier\" \"{carrier}\"\n"))
            .unwrap_or_default();
        let light = format!(
            "// entity 4\n{{\n\"classname\" \"{classname}\"\n\"origin\" \"{origin}\"\n\"light\" \"300\"\n\"_color\" \"255 255 255\"\n\"_falloff_range\" \"512\"\n\"style\" \"0\"\n{carrier_property}{extra_properties}}}\n"
        );

        map.replacen("// entity 3", &format!("{light}// entity 3"), 1)
    }

    fn kinematic_map_with_duplicate_named_mover(map: String) -> String {
        let mover_start = map
            .find("// entity 1")
            .expect("kinematic fixture must contain its mover");
        let waypoint_start = map
            .find("// entity 2")
            .expect("kinematic fixture must contain its first waypoint");
        let duplicate = map[mover_start..waypoint_start].replacen("// entity 1", "// entity 5", 1);

        map.replacen("// entity 2", &format!("{duplicate}// entity 2"), 1)
    }

    fn kinematic_test_map_with_nexts(path_next: &str, wp_b_next: &str) -> String {
        format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{{
( 256 0 -32 ) ( 257 0 -32 ) ( 256 1 -32 ) static_tex 0 0 0 1 1
( 256 0  32 ) ( 256 1  32 ) ( 257 0  32 ) static_tex 0 0 0 1 1
( 192 0 0 ) ( 192 1 0 ) ( 192 0 1 ) static_tex 0 0 0 1 1
( 320 0 0 ) ( 320 0 1 ) ( 320 1 0 ) static_tex 0 0 0 1 1
( 256 -64 0 ) ( 256 -64 1 ) ( 257 -64 0 ) static_tex 0 0 0 1 1
( 256  64 0 ) ( 257  64 0 ) ( 256  64 1 ) static_tex 0 0 0 1 1
}}
}}
// entity 1
{{
"classname" "kinematic_mover"
"name" "lift_a"
"path" "wp_a"
"speed" "2.5"
"wait_ms" "50"
"move_mode" "ping_pong"
"start_on_spawn" "1"
"_tags" "platform arena"
{{
( 0 0 -16 ) ( 1 0 -16 ) ( 0 1 -16 ) mover_tex 0 0 0 1 1
( 0 0  16 ) ( 0 1  16 ) ( 1 0  16 ) mover_tex 0 0 0 1 1
( -32 0 0 ) ( -32 1 0 ) ( -32 0 1 ) mover_tex 0 0 0 1 1
(  32 0 0 ) (  32 0 1 ) (  32 1 0 ) mover_tex 0 0 0 1 1
( 0 -32 0 ) ( 0 -32 1 ) ( 1 -32 0 ) mover_tex 0 0 0 1 1
( 0  32 0 ) ( 1  32 0 ) ( 0  32 1 ) mover_tex 0 0 0 1 1
}}
}}
// entity 2
{{
"classname" "kinematic_waypoint"
"name" "wp_a"
"next" "{path_next}"
"origin" "0 0 0"
}}
// entity 3
{{
"classname" "kinematic_waypoint"
"name" "wp_b"
"next" "{wp_b_next}"
"origin" "0 0 64"
}}
"#
        )
    }

    fn spinning_kinematic_test_map(path_next: &str) -> String {
        kinematic_test_map(path_next).replacen(
            "\"start_on_spawn\" \"1\"\n",
            "\"start_on_spawn\" \"1\"\n\"spin_axis\" \"0 0 2\"\n\"spin_speed\" \"90\"\n\"spin_accel\" \"12.5\"\n\"carry_yaw\" \"1\"\n",
            1,
        )
    }

    fn trigger_volume_map(command: &str, command_arg: &str, rearm_ms: &str) -> String {
        kinematic_test_map("wp_b")
            .replacen("\"classname\" \"kinematic_mover\"", "\"classname\" \"trigger_volume\"", 1)
            .replacen("\"name\" \"lift_a\"", &format!("\"name\" \"lift_a\"\n\"command\" \"{command}\"\n\"command_arg\" \"{command_arg}\"\n\"rearm_ms\" \"{rearm_ms}\""), 1)
    }

    #[test]
    fn trigger_volume_is_extracted_as_aabb_and_excluded_from_static_geometry() {
        let map = parse_inline_map(&trigger_volume_map("start", "", "100")).unwrap();
        assert_eq!(map.trigger_volumes.len(), 1);
        assert!(
            map.trigger_volumes[0]
                .tags
                .contains(&"platform".to_string())
        );
        assert!(
            map.brush_volumes
                .iter()
                .flat_map(|b| b.sides.iter())
                .all(|side| side.texture != "mover_tex")
        );
        let result = crate::partition::partition(&map.brush_volumes).unwrap();
        let geometry =
            crate::geometry::extract_geometry(&result.faces, &result.tree, &HashSet::new());
        assert!(
            geometry
                .texture_names
                .names
                .iter()
                .all(|name| name != "mover_tex")
        );
    }

    #[test]
    fn trigger_volume_rejects_invalid_command_argument_and_rearm() {
        assert!(parse_inline_map(&trigger_volume_map("nonsense", "", "0")).is_err());
        assert!(parse_inline_map(&trigger_volume_map("go_to_path_node", "", "0")).is_err());
        assert!(parse_inline_map(&trigger_volume_map("start", "", "-1")).is_err());
    }

    #[test]
    fn trigger_volume_trims_event_names() {
        let map = trigger_volume_map("start", "", "100").replacen(
            "\"rearm_ms\" \"100\"",
            "\"rearm_ms\" \"100\"\n\"on_fire\" \"  open_lift  \"\n\"on_exit\" \"  close_lift  \"",
            1,
        );
        let map = parse_inline_map(&map).unwrap();
        let trigger = &map.trigger_volumes[0];
        assert_eq!(trigger.on_fire, "open_lift");
        assert_eq!(trigger.on_exit, "close_lift");

        let records = crate::trigger_volumes::encode_trigger_volumes_section(&map.trigger_volumes)
            .expect("one trigger produces a PRL section");
        assert_eq!(records.triggers[0].on_fire, "open_lift");
        assert_eq!(records.triggers[0].on_exit, "close_lift");
    }

    #[test]
    fn trigger_volume_accepts_exit_only_event() {
        let map = trigger_volume_map("start", "", "100").replacen(
            "\"rearm_ms\" \"100\"",
            "\"rearm_ms\" \"100\"\n\"on_exit\" \"  close_lift  \"",
            1,
        );
        let map = parse_inline_map(&map).expect("exit-only trigger must compile");
        let trigger = &map.trigger_volumes[0];
        assert!(trigger.target_tag.is_empty());
        assert!(trigger.on_fire.is_empty());
        assert_eq!(trigger.on_exit, "close_lift");

        let records = crate::trigger_volumes::encode_trigger_volumes_section(&map.trigger_volumes)
            .expect("one trigger produces a PRL section");
        assert_eq!(records.triggers[0].on_exit, "close_lift");
    }

    /// A `worldspawn` plus one `sh_protect_volume` brush entity — a 32-unit box
    /// at the Quake origin. `extra_kvps` is spliced onto the entity without a
    /// blank line (a blank line inside an entity block makes the .map parser
    /// silently drop the remaining entities).
    fn sh_protect_volume_map(extra_kvps: &str) -> String {
        format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{{
( 256 0 -32 ) ( 257 0 -32 ) ( 256 1 -32 ) static_tex 0 0 0 1 1
( 256 0  32 ) ( 256 1  32 ) ( 257 0  32 ) static_tex 0 0 0 1 1
( 192 0 0 ) ( 192 1 0 ) ( 192 0 1 ) static_tex 0 0 0 1 1
( 320 0 0 ) ( 320 0 1 ) ( 320 1 0 ) static_tex 0 0 0 1 1
( 256 -64 0 ) ( 256 -64 1 ) ( 257 -64 0 ) static_tex 0 0 0 1 1
( 256  64 0 ) ( 257  64 0 ) ( 256  64 1 ) static_tex 0 0 0 1 1
}}
}}
// entity 1
{{
"classname" "sh_protect_volume"
"name" "vault"{extra_kvps}
{{
( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) protect_tex 0 0 0 1 1
( 32 0 0 ) ( 32 0 1 ) ( 32 1 0 ) protect_tex 0 0 0 1 1
( 0 0 0 ) ( 0 0 1 ) ( 1 0 0 ) protect_tex 0 0 0 1 1
( 0 32 0 ) ( 1 32 0 ) ( 0 32 1 ) protect_tex 0 0 0 1 1
( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) protect_tex 0 0 0 1 1
( 0 0 32 ) ( 0 1 32 ) ( 1 0 32 ) protect_tex 0 0 0 1 1
}}
}}
"#
        )
    }

    #[test]
    fn sh_protect_volume_resolves_to_a_world_aabb() {
        let map = parse_inline_map(&sh_protect_volume_map("")).expect("protect volume compiles");
        assert_eq!(
            map.sh_protect_aabbs.len(),
            1,
            "one protect volume → one AABB"
        );
        // Quake box [0,32]^3 → engine (−y, z, −x) × 0.0254 (inch → m).
        let s = MapFormat::IdTech2.units_to_meters() as f32;
        let expect = [-32.0 * s, 0.0, -32.0 * s, 0.0, 32.0 * s, 0.0];
        let aabb = map.sh_protect_aabbs[0];
        for (got, want) in aabb.iter().zip(expect.iter()) {
            assert!(
                (got - want).abs() < 1e-5,
                "aabb {aabb:?} vs expected {expect:?}"
            );
        }
        // A protection volume is invisible: it becomes neither a trigger nor
        // static world geometry.
        assert!(
            map.trigger_volumes.is_empty(),
            "protect volume is not a trigger"
        );
        assert!(
            map.brush_volumes
                .iter()
                .flat_map(|b| b.sides.iter())
                .all(|side| side.texture != "protect_tex"),
            "protect volume brush must not enter static geometry"
        );
    }

    #[test]
    fn sh_protect_volume_dilation_expands_the_aabb_on_all_faces() {
        let base = parse_inline_map(&sh_protect_volume_map(""))
            .unwrap()
            .sh_protect_aabbs[0];
        let dilated = parse_inline_map(&sh_protect_volume_map("\n\"dilation\" \"0.5\""))
            .expect("dilated protect volume compiles")
            .sh_protect_aabbs[0];
        let d = 0.5f32;
        for i in 0..3 {
            assert!(
                (dilated[i] - (base[i] - d)).abs() < 1e-5,
                "min[{i}] expected {} got {}",
                base[i] - d,
                dilated[i]
            );
            assert!(
                (dilated[i + 3] - (base[i + 3] + d)).abs() < 1e-5,
                "max[{i}] expected {} got {}",
                base[i + 3] + d,
                dilated[i + 3]
            );
        }
    }

    #[test]
    fn sh_protect_volume_rejects_negative_dilation() {
        let err = parse_inline_map(&sh_protect_volume_map("\n\"dilation\" \"-1\""))
            .expect_err("negative dilation must fail the compile");
        assert!(
            err.to_string().contains("`dilation` must be non-negative"),
            "{err}"
        );
    }

    /// A `switch` sharing the `trigger_volume` fixture's brush, so the two can be
    /// compiled side by side and their AABBs compared per axis.
    ///
    /// `extra_kvps` is spliced without a blank line: a blank line inside an entity
    /// block makes the .map parser silently drop the remaining entities.
    fn switch_map(extra_kvps: &str) -> String {
        let name_block = if extra_kvps.is_empty() {
            "\"name\" \"lift_a\"".to_string()
        } else {
            format!("\"name\" \"lift_a\"\n{extra_kvps}")
        };
        kinematic_test_map("wp_b")
            .replacen(
                "\"classname\" \"kinematic_mover\"",
                "\"classname\" \"switch\"",
                1,
            )
            .replacen("\"name\" \"lift_a\"", &name_block, 1)
    }

    /// A Quake-format axis-aligned box brush spanning `min`..`max` (map units),
    /// with each face's point triple wound so its plane normal points out of the box.
    fn box_brush(min: [i32; 3], max: [i32; 3], texture: &str) -> String {
        let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
        format!(
            r#"{{
( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {texture} 0 0 0 1 1
( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1
( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1
( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1
( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1
( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {texture} 0 0 0 1 1
}}"#
        )
    }

    /// A switch console mounted flush on a wall, mirroring
    /// `content/dev/maps/switch-demo.map`: console at Quake y 312..320, 16-unit
    /// wall at y 320..336. Quake +y is engine -x, so the walled face is the
    /// `aabb_min[0]` one; the console's other five faces front open space.
    fn wall_mounted_switch_map() -> String {
        let wall = box_brush([0, 320, 0], [448, 336, 160], "wall_tex");
        let console = box_brush([200, 312, 40], [248, 320, 72], "switch_tex");
        format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{wall}
}}
// entity 1
{{
"classname" "switch"
"name" "door_switch"
"on_fire" "open_door"
{console}
}}
"#
        )
    }

    // Regression: the margin was applied to all six faces unconditionally, so a
    // wall-flush switch was pressable from the room on the far side of the wall —
    // margin plus capsule radius outreached a standard 16-unit wall.
    #[test]
    fn switch_flush_against_a_wall_grows_only_into_open_space() {
        let map =
            parse_inline_map(&wall_mounted_switch_map()).expect("wall-mounted switch must compile");
        assert_eq!(map.trigger_volumes.len(), 1);
        let switch = &map.trigger_volumes[0];

        // Console hull in map units, engine-swizzled (engine x = -quake_y,
        // y = quake_z, z = -quake_x). The wall occupies engine x -336..-320.
        let hull_min = [-320.0, 40.0, -248.0];
        let hull_max = [-312.0, 72.0, -200.0];
        let unit = MapFormat::IdTech2.units_to_meters() as f32;
        let margin = DEFAULT_SWITCH_USE_REACH;

        // The walled face keeps the raw hull — no margin at all.
        assert!(
            (switch.aabb_min[0] - hull_min[0] * unit).abs() < 1e-5,
            "rear face must not grow into the wall, got {}",
            switch.aabb_min[0]
        );
        assert!(
            switch.aabb_min[0] > -336.0 * unit,
            "rear face must stay short of the wall's far side"
        );
        // Every face fronting open room gets the full margin.
        assert!((switch.aabb_max[0] - (hull_max[0] + margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_min[1] - (hull_min[1] - margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_max[1] - (hull_max[1] + margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_min[2] - (hull_min[2] - margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_max[2] - (hull_max[2] + margin) * unit).abs() < 1e-5);
    }

    /// The inside corner of two 16-unit walls with a console mounted flush on one
    /// of them and 4 units clear of the other. Quake: north wall y 320..336 across
    /// x 0..448, west wall x 0..16 across y 0..336, console x 20..68 on y 312..320.
    ///
    /// Engine axes are `(-y, z, -x)`, so the console's `+z` face (engine z -20)
    /// faces the west wall's near side (engine z -16) across a 4-unit gap, its `-x`
    /// face is flush on the north wall, and its other four faces front open room.
    fn corner_mounted_switch_map() -> String {
        let north_wall = box_brush([0, 320, 0], [448, 336, 160], "wall_tex");
        let west_wall = box_brush([0, 0, 0], [16, 336, 160], "wall_tex");
        let console = box_brush([20, 312, 40], [68, 320, 72], "switch_tex");
        format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{north_wall}
{west_wall}
}}
// entity 1
{{
"classname" "switch"
"name" "corner_switch"
"on_fire" "open_door"
{console}
}}
"#
        )
    }

    // Regression: solidity was sampled one map unit outside each face and the face
    // then grew by the full margin, so any solid between the probe and the margin
    // was passed through — this console's +z face grew 24 units across a 4-unit gap
    // and out the far side of a 16-unit wall, pressable from the room beyond.
    #[test]
    fn switch_reach_clamps_each_face_to_the_free_space_in_front_of_it() {
        let map = parse_inline_map(&corner_mounted_switch_map())
            .expect("corner-mounted switch must compile");
        assert_eq!(map.trigger_volumes.len(), 1);
        let switch = &map.trigger_volumes[0];

        // Console hull in map units, engine-swizzled.
        let hull_min = [-320.0, 40.0, -68.0];
        let hull_max = [-312.0, 72.0, -20.0];
        let unit = MapFormat::IdTech2.units_to_meters() as f32;
        let margin = DEFAULT_SWITCH_USE_REACH;
        let clearance = 4.0;

        // Clamped to the gap: 4 units, not the 24-unit margin.
        assert!(
            (switch.aabb_max[2] - (hull_max[2] + clearance) * unit).abs() < 1e-5,
            "+z face must stop at the west wall's near side, got {} (hull {})",
            switch.aabb_max[2],
            hull_max[2] * unit
        );
        // Flush on the north wall: zero free distance, so no growth.
        assert!(
            (switch.aabb_min[0] - hull_min[0] * unit).abs() < 1e-5,
            "flush face must not grow, got {}",
            switch.aabb_min[0]
        );
        // The four faces fronting open room still get the full margin — a blanket
        // clamp fails here as loudly as no clamp fails above.
        assert!((switch.aabb_max[0] - (hull_max[0] + margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_min[1] - (hull_min[1] - margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_max[1] - (hull_max[1] + margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_min[2] - (hull_min[2] - margin) * unit).abs() < 1e-5);
    }

    /// The corner fixture with its west wall authored as a `kinematic_mover` — a
    /// console mounted beside a blast door rather than beside static brushwork.
    /// Geometry, and therefore the expected clamp, is identical to
    /// [`corner_mounted_switch_map`].
    fn mover_occluder_switch_map() -> String {
        let north_wall = box_brush([0, 320, 0], [448, 336, 160], "wall_tex");
        let door = box_brush([0, 0, 0], [16, 336, 160], "door_tex");
        let console = box_brush([20, 312, 40], [68, 320, 72], "switch_tex");
        format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{north_wall}
}}
// entity 1
{{
"classname" "kinematic_mover"
"name" "blast_door"
"path" "wp_a"
"speed" "2.5"
"wait_ms" "50"
"move_mode" "ping_pong"
"start_on_spawn" "1"
{door}
}}
// entity 2
{{
"classname" "switch"
"name" "corner_switch"
"on_fire" "open_door"
{console}
}}
// entity 3
{{
"classname" "kinematic_waypoint"
"name" "wp_a"
"next" "wp_b"
"origin" "0 0 0"
}}
// entity 4
{{
"classname" "kinematic_waypoint"
"name" "wp_b"
"next" ""
"origin" "0 0 64"
}}
"#
        )
    }

    // Mover occlusion is published FGD contract, but deleting the whole mover-hull
    // build and passing an empty slice failed nothing: mover brushes never reach the
    // static world set, so only a fixture with a mover in the reach corridor covers it.
    #[test]
    fn switch_reach_clamps_against_a_kinematic_mover_hull() {
        let map = parse_inline_map(&mover_occluder_switch_map())
            .expect("switch beside a kinematic_mover must compile");
        assert_eq!(map.trigger_volumes.len(), 1);
        let switch = &map.trigger_volumes[0];

        // Console hull in map units, engine-swizzled.
        let hull_max = [-312.0, 72.0, -20.0];
        let unit = MapFormat::IdTech2.units_to_meters() as f32;
        let margin = DEFAULT_SWITCH_USE_REACH;
        let clearance = 4.0;

        assert!(
            (switch.aabb_max[2] - (hull_max[2] + clearance) * unit).abs() < 1e-5,
            "+z face must stop at the door's near side, got {} (hull {})",
            switch.aabb_max[2],
            hull_max[2] * unit
        );
        // Not a blanket clamp: the face pointing away from the door still grows.
        assert!((switch.aabb_max[0] - (hull_max[0] + margin) * unit).abs() < 1e-5);
        // The mover's brushwork stays out of the static world geometry.
        assert!(
            map.brush_volumes
                .iter()
                .flat_map(|brush| brush.sides.iter())
                .all(|side| side.texture != "door_tex"),
            "a kinematic_mover brush must not become world geometry just because it clamped"
        );
    }

    /// A console one map unit clear of the wall in front of it. Quake: wall x 0..19
    /// across y 0..336 and z 0..160, console x 20..68 on y 312..320, z 40..72.
    ///
    /// Engine axes are `(-y, z, -x)`, so the console's `+z` face (engine z -20) faces
    /// the wall's near side (engine z -19) across that single unit, and the console's
    /// other five faces front open space.
    fn one_unit_gap_switch_map() -> String {
        let wall = box_brush([0, 0, 0], [19, 336, 160], "wall_tex");
        let console = box_brush([20, 312, 40], [68, 320, 72], "switch_tex");
        format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{wall}
}}
// entity 1
{{
"classname" "switch"
"name" "tight_switch"
"on_fire" "open_door"
{console}
}}
"#
        )
    }

    // Mutation guard on FLUSH_TOLERANCE_METERS: every other fixture's smallest free
    // gap is 4 map units (0.1016 m) and every cross-section span is >= 8 units, so
    // loosening the tolerance 100x to 1e-1 left the suite green. One map unit of gap
    // is 0.0254 m — under a loosened tolerance the wall stops reading as standing in
    // front of the face and it grows the full margin instead of one unit.
    #[test]
    fn switch_reach_resolves_a_one_unit_gap_at_the_flush_tolerance() {
        let map =
            parse_inline_map(&one_unit_gap_switch_map()).expect("tight-gap switch must compile");
        let switch = &map.trigger_volumes[0];

        let hull_min = [-320.0, 40.0, -68.0];
        let hull_max = [-312.0, 72.0, -20.0];
        let unit = MapFormat::IdTech2.units_to_meters() as f32;
        let margin = DEFAULT_SWITCH_USE_REACH;

        assert!(
            (switch.aabb_max[2] - (hull_max[2] + 1.0) * unit).abs() < 1e-5,
            "+z face must grow exactly the one free unit, got {} (hull {})",
            switch.aabb_max[2],
            hull_max[2] * unit
        );
        // The other five faces front open space and take the full margin.
        assert!((switch.aabb_min[0] - (hull_min[0] - margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_max[0] - (hull_max[0] + margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_min[1] - (hull_min[1] - margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_max[1] - (hull_max[1] + margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_min[2] - (hull_min[2] - margin) * unit).abs() < 1e-5);
    }

    /// A console flush on the north wall with a short post in front of its `+z` face,
    /// covering only the lower half of that face's cross-section. Quake: north wall
    /// y 320..336 across x 0..448, post x 0..16 on y 300..320 and z 40..56, console
    /// x 20..68 on y 312..320 and z 40..72.
    ///
    /// Engine axes are `(-y, z, -x)`, so the post spans engine y 40..56 against the
    /// console's 40..72 — a partial overlap, not containment.
    fn partial_overlap_switch_map() -> String {
        let north_wall = box_brush([0, 320, 0], [448, 336, 160], "wall_tex");
        let post = box_brush([0, 300, 40], [16, 320, 56], "post_tex");
        let console = box_brush([20, 312, 40], [68, 320, 72], "switch_tex");
        format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{north_wall}
{post}
}}
// entity 1
{{
"classname" "switch"
"name" "post_switch"
"on_fire" "open_door"
{console}
}}
"#
        )
    }

    // Mutation guard on the positive-area cross-section test: in both wall fixtures
    // the wall engulfs the console's cross-section, so replacing overlap with
    // containment left the suite green. Half a face's cross-section covered must
    // clamp the whole face — an AABB trigger cannot grow part of one.
    #[test]
    fn switch_reach_clamps_a_face_its_occluder_only_partly_covers() {
        let map = parse_inline_map(&partial_overlap_switch_map())
            .expect("switch with a partly-covering occluder must compile");
        let switch = &map.trigger_volumes[0];

        let hull_min = [-320.0, 40.0, -68.0];
        let hull_max = [-312.0, 72.0, -20.0];
        let unit = MapFormat::IdTech2.units_to_meters() as f32;
        let margin = DEFAULT_SWITCH_USE_REACH;
        let clearance = 4.0;

        assert!(
            (switch.aabb_max[2] - (hull_max[2] + clearance) * unit).abs() < 1e-5,
            "+z face must stop at the post's near side even though the post covers only \
             half its cross-section, got {} (hull {})",
            switch.aabb_max[2],
            hull_max[2] * unit
        );
        // Flush on the north wall.
        assert!((switch.aabb_min[0] - hull_min[0] * unit).abs() < 1e-5);
        // The post sits outside the footprint of every other face.
        assert!((switch.aabb_max[0] - (hull_max[0] + margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_min[1] - (hull_min[1] - margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_max[1] - (hull_max[1] + margin) * unit).abs() < 1e-5);
        assert!((switch.aabb_min[2] - (hull_min[2] - margin) * unit).abs() < 1e-5);
    }

    /// A non-axis-aligned brush: the diagonal slab `800 <= x + y <= 900` bounded by
    /// `x >= 100`, `y >= 100`, `0 <= z <= 300`.
    ///
    /// Its solid is a quadrilateral prism with corners at Quake (100, 700), (100, 800),
    /// (800, 100), (700, 100), so its AABB is x 100..800, y 100..800 — far larger than
    /// the brush and, unlike the brush, straddling a console at x 200..248, y 312..320.
    fn diagonal_slab_brush() -> &'static str {
        r#"{
( 100 0 0 ) ( 100 1 0 ) ( 100 0 1 ) slab_tex 0 0 0 1 1
( 0 100 0 ) ( 0 100 1 ) ( 1 100 0 ) slab_tex 0 0 0 1 1
( 800 0 0 ) ( 0 800 0 ) ( 800 0 1 ) slab_tex 0 0 0 1 1
( 900 0 0 ) ( 900 0 1 ) ( 0 900 0 ) slab_tex 0 0 0 1 1
( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) slab_tex 0 0 0 1 1
( 0 0 300 ) ( 0 1 300 ) ( 1 0 300 ) slab_tex 0 0 0 1 1
}"#
    }

    /// A console in open space with [`diagonal_slab_brush`] as the only world brush.
    /// The slab's nearest face is ~164 map units from the console — 6× the default
    /// reach — while its AABB contains the console outright.
    fn diagonal_occluder_switch_map() -> String {
        let console = box_brush([200, 312, 40], [248, 320, 72], "switch_tex");
        format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{}
}}
// entity 1
{{
"classname" "switch"
"name" "open_switch"
"on_fire" "open_door"
{console}
}}
"#,
            diagonal_slab_brush()
        )
    }

    // Regression: the occluder test read the brush AABB, so a wedge, ramp, chamfer, or
    // diagonal partition 100+ units away could straddle the switch on all three axes,
    // zero all six faces, and warn that the switch was enclosed in solid.
    #[test]
    fn switch_reach_ignores_a_diagonal_brush_its_aabb_straddles_the_switch() {
        let map = parse_inline_map(&diagonal_occluder_switch_map())
            .expect("switch near a diagonal slab must compile");
        let switch = &map.trigger_volumes[0];

        let hull_min = [-320.0, 40.0, -248.0];
        let hull_max = [-312.0, 72.0, -200.0];
        let unit = MapFormat::IdTech2.units_to_meters() as f32;
        let margin = DEFAULT_SWITCH_USE_REACH;

        // The premise, asserted so the test cannot pass vacuously: the slab really
        // reaches the world occluder set, and its AABB really does straddle the
        // console on every axis. Both are what made the AABB proxy zero all six faces.
        let slab = map
            .brush_volumes
            .iter()
            .find(|brush| brush.sides.iter().any(|side| side.texture == "slab_tex"))
            .expect("the diagonal slab must reach the static world brush set");
        for axis in 0..3 {
            let low = f64::from(hull_min[axis]) * f64::from(unit);
            let high = f64::from(hull_max[axis]) * f64::from(unit);
            assert!(
                slab.aabb.min[axis] < low && slab.aabb.max[axis] > high,
                "axis {axis}: the slab AABB must straddle the console, got {:?}..{:?}",
                slab.aabb.min,
                slab.aabb.max
            );
        }

        for axis in 0..3 {
            assert!(
                (switch.aabb_min[axis] - (hull_min[axis] - margin) * unit).abs() < 1e-5,
                "axis {axis} low face must take the full margin, got {}",
                switch.aabb_min[axis]
            );
            assert!(
                (switch.aabb_max[axis] - (hull_max[axis] + margin) * unit).abs() < 1e-5,
                "axis {axis} high face must take the full margin, got {}",
                switch.aabb_max[axis]
            );
        }
    }

    /// A floor-standing console sunk one map unit into the floor and mounted flush
    /// against the wall behind it — routine practice to hide a seam. Quake: floor
    /// z -16..0 across x 0..448 and y 0..448, wall y 320..336 across x 0..448 and
    /// z -16..160, console x 200..248 on y 312..320 and z -1..31.
    fn sunk_console_switch_map() -> String {
        let floor = box_brush([0, 0, -16], [448, 448, 0], "floor_tex");
        let wall = box_brush([0, 320, -16], [448, 336, 160], "wall_tex");
        let console = box_brush([200, 312, -1], [248, 320, 31], "switch_tex");
        format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{floor}
{wall}
}}
// entity 1
{{
"classname" "switch"
"name" "sunk_console"
"on_fire" "open_door"
{console}
}}
"#
        )
    }

    // The observable half of the per-face clamp diagnostic. Zeroing here is correct —
    // the one-unit overlap band means no horizontal face can grow without entering
    // solid — but it used to be silent: the top face still grew, so the
    // every-face-enclosed warning never fired and the switch compiled clean and
    // unpressable. The warning itself is not asserted: the compiler's logger is
    // process-global and not unit-capturable.
    #[test]
    fn switch_sunk_into_its_mount_grows_only_upward() {
        let map = parse_inline_map(&sunk_console_switch_map()).expect("sunk console must compile");
        let switch = &map.trigger_volumes[0];

        // Console hull in map units, engine-swizzled.
        let hull_min = [-320.0, -1.0, -248.0];
        let hull_max = [-312.0, 31.0, -200.0];
        let unit = MapFormat::IdTech2.units_to_meters() as f32;
        let margin = DEFAULT_SWITCH_USE_REACH;

        // Every horizontal face overlaps the floor across the one-unit sink band, so
        // none of them can grow at all.
        for axis in [0, 2] {
            assert!(
                (switch.aabb_min[axis] - hull_min[axis] * unit).abs() < 1e-5,
                "axis {axis} low face must not grow into the floor, got {}",
                switch.aabb_min[axis]
            );
            assert!(
                (switch.aabb_max[axis] - hull_max[axis] * unit).abs() < 1e-5,
                "axis {axis} high face must not grow into the floor, got {}",
                switch.aabb_max[axis]
            );
        }
        // Down is the floor itself; up is the only open direction.
        assert!((switch.aabb_min[1] - hull_min[1] * unit).abs() < 1e-5);
        assert!((switch.aabb_max[1] - (hull_max[1] + margin) * unit).abs() < 1e-5);
    }

    #[test]
    fn switch_folds_brushes_into_world_geometry_and_emits_inflated_use_trigger() {
        // The fixture's only other world brush stands 160 units off the nearest
        // switch face — well past the 24-unit reach corridor — so no face is
        // clamped and all six margins are expected. This is the suite's guard on
        // the `min(margin, gap)` cap in the gap-exceeds-margin direction: shrink
        // that separation below the margin and the expected AABB changes.
        //
        // Reference hull: the same brush authored as a `trigger_volume`, which is
        // not inflated.
        let reference = parse_inline_map(&trigger_volume_map("start", "", "0"))
            .expect("reference trigger_volume must compile");
        let hull = &reference.trigger_volumes[0];

        let map = parse_inline_map(&switch_map("\"on_fire\" \"open_lift\""))
            .expect("switch must compile");
        assert_eq!(map.trigger_volumes.len(), 1, "one switch → one trigger");
        let switch = &map.trigger_volumes[0];
        assert_eq!(switch.activation, 1, "switch activation is forced to `use`");

        // Visible + solid: unlike trigger_volume, the switch brush stays in the
        // static BSP inputs and the extracted draw geometry.
        assert!(
            map.brush_volumes
                .iter()
                .flat_map(|brush| brush.sides.iter())
                .any(|side| side.texture == "mover_tex"),
            "switch brush must feed static brush_volumes"
        );
        let result = crate::partition::partition(&map.brush_volumes).unwrap();
        let geometry =
            crate::geometry::extract_geometry(&result.faces, &result.tree, &HashSet::new());
        assert!(
            geometry
                .texture_names
                .names
                .iter()
                .any(|name| name == "mover_tex"),
            "switch faces must reach the draw geometry"
        );

        // Reachability: inflated by the default use_reach on every axis, scaled
        // from map units into engine meters.
        let margin = DEFAULT_SWITCH_USE_REACH * MapFormat::IdTech2.units_to_meters() as f32;
        for axis in 0..3 {
            assert!(
                switch.aabb_min[axis] < hull.aabb_min[axis],
                "axis {axis}: min must shrink ({} vs {})",
                switch.aabb_min[axis],
                hull.aabb_min[axis]
            );
            assert!(
                switch.aabb_max[axis] > hull.aabb_max[axis],
                "axis {axis}: max must grow ({} vs {})",
                switch.aabb_max[axis],
                hull.aabb_max[axis]
            );
            assert!((switch.aabb_min[axis] - (hull.aabb_min[axis] - margin)).abs() < 1e-5);
            assert!((switch.aabb_max[axis] - (hull.aabb_max[axis] + margin)).abs() < 1e-5);
        }
    }

    #[test]
    fn switch_use_reach_override_widens_the_trigger_beyond_the_default() {
        // Both the authored value and the expected delta derive from the constant:
        // hardcoding them made this test fail whenever the default moved, for a
        // reason unrelated to what it checks.
        let authored = DEFAULT_SWITCH_USE_REACH * 2.0;
        let default_map =
            parse_inline_map(&switch_map("\"on_fire\" \"open_lift\"")).expect("switch compiles");
        let wide_map = parse_inline_map(&switch_map(&format!(
            "\"on_fire\" \"open_lift\"\n\"use_reach\" \"{authored}\""
        )))
        .expect("switch with authored use_reach compiles");

        let default_trigger = &default_map.trigger_volumes[0];
        let wide = &wide_map.trigger_volumes[0];
        let extra =
            (authored - DEFAULT_SWITCH_USE_REACH) * MapFormat::IdTech2.units_to_meters() as f32;
        for axis in 0..3 {
            assert!((wide.aabb_min[axis] - (default_trigger.aabb_min[axis] - extra)).abs() < 1e-5);
            assert!((wide.aabb_max[axis] - (default_trigger.aabb_max[axis] + extra)).abs() < 1e-5);
        }
    }

    #[test]
    fn switch_rejects_use_reach_outside_the_supported_range() {
        // Zero is rejected alongside negatives: it leaves a press volume no larger
        // than the solid brush, pressable only from flush contact, which is a typo
        // rather than intent.
        assert!(parse_inline_map(&switch_map("\"use_reach\" \"0\"")).is_err());
        assert!(parse_inline_map(&switch_map("\"use_reach\" \"-1\"")).is_err());
        // The floor is the flush tolerance, not zero. A margin below it is discarded
        // per face as float dust, so it used to compile clean into an unpressable
        // switch that reported itself as clamped against solid.
        assert!(parse_inline_map(&switch_map("\"use_reach\" \"1e-38\"")).is_err());
        assert!(parse_inline_map(&switch_map("\"use_reach\" \"0.001\"")).is_err());
        // Just above the floor is legal: the tolerance is 1e-3 m and one map unit is
        // 0.0254 m, so the floor sits near 0.0394 map units.
        assert!(
            parse_inline_map(&switch_map("\"use_reach\" \"0.05\"")).is_ok(),
            "a margin above the flush tolerance is small but real"
        );
        assert!(parse_inline_map(&switch_map("\"use_reach\" \"nonsense\"")).is_err());
        // `nan` and `inf` parse as floats and take the `is_finite` branch rather
        // than the parse-error one.
        assert!(parse_inline_map(&switch_map("\"use_reach\" \"nan\"")).is_err());
        assert!(parse_inline_map(&switch_map("\"use_reach\" \"inf\"")).is_err());
        assert!(parse_inline_map(&switch_map("\"use_reach\" \"-inf\"")).is_err());
        // Unbounded reach made the volume swallow the level, firing on every `use`
        // press in it.
        assert!(parse_inline_map(&switch_map("\"use_reach\" \"100000\"")).is_err());
        let over = format!("\"use_reach\" \"{}\"", MAX_SWITCH_USE_REACH + 1.0);
        assert!(parse_inline_map(&switch_map(&over)).is_err());
        // The bound itself is legal — as is any value f32 rounds to it, so the
        // rejected set starts one ULP above, not at the first decimal past it.
        let at_bound = format!("\"use_reach\" \"{MAX_SWITCH_USE_REACH}\"");
        assert!(
            parse_inline_map(&switch_map(&at_bound)).is_ok(),
            "the bound itself is a legal value"
        );
    }

    #[test]
    fn switch_falls_back_to_the_default_use_reach_when_the_field_is_empty() {
        // A TrenchBroom field the author cleared arrives as `""`, and `use_reach` is
        // the one numeric key the FGD invites authors to edit. Rejecting the empty
        // value turned an editor keystroke into a compile error.
        let default_map =
            parse_inline_map(&switch_map("\"on_fire\" \"open_lift\"")).expect("switch compiles");
        let cleared =
            parse_inline_map(&switch_map("\"on_fire\" \"open_lift\"\n\"use_reach\" \"\""))
                .expect("a cleared use_reach must fall back to the default, not fail the compile");

        let expected = &default_map.trigger_volumes[0];
        let actual = &cleared.trigger_volumes[0];
        for axis in 0..3 {
            assert!((actual.aabb_min[axis] - expected.aabb_min[axis]).abs() < 1e-6);
            assert!((actual.aabb_max[axis] - expected.aabb_max[axis]).abs() < 1e-6);
        }
    }

    #[test]
    fn switch_rejects_multiple_brushes_in_one_entity() {
        // `resolve_trigger_volume` unions the entity's brushes into one AABB, so two
        // consoles on facing walls of a room silently produced a room-spanning `use`
        // volume: every face of the union fronts open room, so the clamp leaves it
        // alone and `MAX_SWITCH_USE_REACH` never applied to the hull.
        let first = box_brush([0, 0, 0], [32, 32, 32], "switch_tex");
        let second = box_brush([256, 0, 0], [288, 32, 32], "switch_tex");
        let map = format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{}
}}
// entity 1
{{
"classname" "switch"
"name" "twin_console"
"on_fire" "open_door"
{first}
{second}
}}
"#,
            box_brush([-512, -512, -512], [-448, -448, -448], "static_tex")
        );
        let error = parse_inline_map(&map).expect_err("a multi-brush switch must not compile");
        let message = error.to_string();
        assert!(
            message.contains("one switch per brush"),
            "error must tell the author how to fix it: {message}"
        );
    }

    /// A `trigger_volume` before and between two switches carrying different
    /// `use_reach` values. Every brush is far enough from the others that no clamp
    /// applies, so each switch's growth reports only its own margin.
    fn interleaved_trigger_and_switch_map() -> String {
        let far_world = box_brush([-512, -512, -512], [-448, -448, -448], "static_tex");
        let first_volume = box_brush([0, 0, 0], [32, 32, 32], "gate_a_tex");
        let near_switch = box_brush([512, 0, 0], [544, 32, 32], "switch_near_tex");
        let second_volume = box_brush([1024, 0, 0], [1056, 32, 32], "gate_b_tex");
        let far_switch = box_brush([1536, 0, 0], [1568, 32, 32], "switch_far_tex");
        format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{far_world}
}}
// entity 1
{{
"classname" "trigger_volume"
"name" "gate_a"
"on_fire" "open_gate_a"
{first_volume}
}}
// entity 2
{{
"classname" "switch"
"name" "switch_near"
"on_fire" "open_door"
"use_reach" "32"
{near_switch}
}}
// entity 3
{{
"classname" "trigger_volume"
"name" "gate_b"
"on_fire" "open_gate_b"
{second_volume}
}}
// entity 4
{{
"classname" "switch"
"name" "switch_far"
"on_fire" "open_hatch"
"use_reach" "64"
{far_switch}
}}
"#
        )
    }

    // Regression guard for the deferred index: a switch's `trigger_index` is
    // captured mid-entity-loop and consumed long after, so a reordering of
    // `trigger_volumes` in between would hand each switch the other's margin.
    // Distinct reaches and interleaved plain volumes make that visible.
    #[test]
    fn switch_reach_margins_follow_their_own_volume_when_triggers_interleave() {
        let map = parse_inline_map(&interleaved_trigger_and_switch_map())
            .expect("interleaved trigger volumes and switches must compile");
        assert_eq!(map.trigger_volumes.len(), 4);
        let unit = MapFormat::IdTech2.units_to_meters() as f32;
        let brush_units = 32.0;

        // Emission order is entity order, and only the switches grow.
        for (index, name, growth) in [
            (0, "gate_a", 0.0),
            (1, "switch_near", 32.0),
            (2, "gate_b", 0.0),
            (3, "switch_far", 64.0),
        ] {
            let volume = &map.trigger_volumes[index];
            assert_eq!(volume.name, name, "trigger_volumes[{index}] identity");
            let expected = brush_units + 2.0 * growth;
            for axis in 0..3 {
                let size = (volume.aabb_max[axis] - volume.aabb_min[axis]) / unit;
                assert!(
                    (size - expected).abs() < 1e-3,
                    "`{name}` axis {axis}: expected {expected} map units, got {size}"
                );
            }
        }
    }

    // Drift guard: the FGD's `use_reach` default and its documented ceiling are
    // hand-copied from this file's constants, so they can disagree silently — the
    // author reads the editor's default while the compiler applies another.
    #[test]
    fn fgd_switch_use_reach_matches_the_compiler_constants() {
        let fgd_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../sdk/TrenchBroom/postretro.fgd"
        );
        let fgd = std::fs::read_to_string(fgd_path).expect("read committed postretro.fgd");
        let switch_class = fgd
            .split_once("= switch :")
            .and_then(|(_, rest)| rest.split_once("\n]"))
            .map(|(body, _)| body)
            .expect("postretro.fgd declares a `switch` class with a closed body");
        let attribute = switch_class
            .lines()
            .find(|line| line.trim_start().starts_with("use_reach("))
            .expect("the FGD `switch` class declares `use_reach` on a single line");

        // FGD attribute fields are quoted and positional: display name, default,
        // then optional help text.
        let fields: Vec<&str> = attribute.split('"').skip(1).step_by(2).collect();
        let declared_default = fields
            .get(1)
            .unwrap_or_else(|| panic!("FGD `use_reach` needs a quoted default: {attribute}"));
        let declared_default: f32 = declared_default.trim().parse().unwrap_or_else(|e| {
            panic!("FGD `use_reach` default is not a number ({e}): {attribute}")
        });
        assert!(
            (declared_default - DEFAULT_SWITCH_USE_REACH).abs() < 1e-6,
            "postretro.fgd `switch` `use_reach` default is {declared_default} but \
             DEFAULT_SWITCH_USE_REACH is {DEFAULT_SWITCH_USE_REACH} — one copy drifted"
        );

        // The ceiling lives in the help text as prose, so the guard pins a fixed
        // marker and parses the digit run after it, numerically. A `contains`
        // substring test passed `MAX_SWITCH_USE_REACH = 12.0` against the FGD's
        // "128", and `{:.0}` rounded anything in [127.5, 128.5) to a match.
        const CEILING_MARKER: &str = "no more than ";
        let declared_ceiling = fields
            .iter()
            .find_map(|field| field.split_once(CEILING_MARKER))
            .map(|(_, rest)| {
                rest.chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect::<String>()
            })
            .unwrap_or_else(|| {
                panic!(
                    "postretro.fgd `switch` `use_reach` help text must state the ceiling as \
                     `{CEILING_MARKER}<number>`; it reads {fields:?}"
                )
            });
        let declared_ceiling: f32 = declared_ceiling.parse().unwrap_or_else(|e| {
            panic!("FGD `use_reach` ceiling after `{CEILING_MARKER}` is not a number ({e})")
        });
        assert!(
            (declared_ceiling - MAX_SWITCH_USE_REACH).abs() < 1e-6,
            "postretro.fgd `switch` `use_reach` states a {declared_ceiling}-unit ceiling but \
             MAX_SWITCH_USE_REACH is {MAX_SWITCH_USE_REACH} — one copy drifted"
        );
    }

    #[test]
    fn fgd_kinematic_mover_declares_spin_properties() {
        let fgd_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../sdk/TrenchBroom/postretro.fgd"
        );
        let fgd = std::fs::read_to_string(fgd_path).expect("read committed postretro.fgd");
        let mover_class = fgd
            .split_once("= kinematic_mover :")
            .and_then(|(_, rest)| rest.split_once("\n]"))
            .map(|(body, _)| body)
            .expect("postretro.fgd declares a `kinematic_mover` class with a closed body");

        for property in [
            "spin_axis(string) : \"Local spin axis as x y z\" : \"0 0 0\"",
            "spin_speed(float) : \"Initial spin speed (degrees per second)\" : \"0\"",
            "spin_accel(float) : \"Spin acceleration (degrees per second squared)\" : \"0\"",
            "carry_yaw(choices) : \"Carry world-up player yaw while rotating; never pitch or roll\" : 0 =",
            "block_policy(choices) : \"Host-authoritative response when this mover contacts an entity\" : \"displace\" =",
            "crush_damage(float) : \"Damage per crusher hit\" : \"0\"",
            "crush_interval_ms(float) : \"Milliseconds between crusher hits (0 hits every tick)\" : \"0\"",
            "auto_close_ms(float) : \"Milliseconds before automatic close; blank inherits the mod default, 0 disables\" : \"\"",
            "open_event(string) : \"Named event when the mover reaches its open terminus\" : \"\"",
            "close_event(string) : \"Named event when the mover reaches its closed terminus\" : \"\"",
            "blocked_event(string) : \"Named event when a blocking mover reacts to contact\" : \"\"",
            "crush_event(string) : \"Named event when a crusher deals damage\" : \"\"",
        ] {
            assert!(
                mover_class.contains(property),
                "postretro.fgd kinematic_mover is missing `{property}`"
            );
        }
    }

    #[test]
    fn switch_without_reaction_or_target_compiles_inert() {
        // Parity with trigger_volume: no on_fire/on_exit/target_tag is a warning
        // from the shared resolve_trigger_volume path, not a compile error.
        let map = parse_inline_map(&switch_map("")).expect("inert switch must compile");
        assert_eq!(map.trigger_volumes.len(), 1);
        let switch = &map.trigger_volumes[0];
        assert!(switch.on_fire.is_empty());
        assert!(switch.on_exit.is_empty());
        assert!(switch.target_tag.is_empty());
        assert_eq!(switch.activation, 1);
    }

    /// A `switch` authored as a point entity — every brush deleted, or hand-written
    /// without any.
    fn brushless_switch_map() -> String {
        format!(
            r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{}
}}
// entity 1
{{
"classname" "switch"
"name" "ghost_switch"
"on_fire" "open_door"
"origin" "0 0 0"
}}
"#,
            box_brush([0, 0, 0], [64, 64, 64], "static_tex")
        )
    }

    #[test]
    fn switch_is_not_emitted_as_a_runtime_map_entity() {
        let map = parse_inline_map(&switch_map("\"on_fire\" \"open_lift\""))
            .expect("switch must compile");
        assert!(
            map.map_entities
                .iter()
                .all(|entity| entity.classname != "switch"),
            "switch desugars into geometry + a trigger, never a classname-dispatch entity"
        );

        // A brushless switch used to take the point-entity tail instead, emitting a
        // `switch` MapEntityRecord that the runtime drops at `debug!` as an
        // unregistered classname: no geometry, no trigger, no diagnostic. It is a
        // broken map, so it must fail the compile.
        let error = parse_inline_map(&brushless_switch_map())
            .expect_err("a brushless switch must not compile");
        let message = error.to_string();
        assert!(
            message.contains("switch") && message.contains("no brushes"),
            "error must name the classname and the real problem: {message}"
        );
    }

    #[test]
    fn switch_forwards_shared_trigger_fields_to_the_emitted_volume() {
        let map = parse_inline_map(&switch_map(concat!(
            "\"target_tag\" \"lift_platform\"\n",
            "\"command\" \"go_to_path_node\"\n",
            "\"command_arg\" \"wp_b\"\n",
            "\"fire_mode\" \"multiple\"\n",
            "\"rearm_ms\" \"250\"\n",
            "\"enabled_on_spawn\" \"0\"\n",
            "\"on_fire\" \"open_lift\"\n",
            "\"on_exit\" \"close_lift\"\n",
            "\"activation\" \"touch\"",
        )))
        .expect("fully-populated switch must compile");
        let switch = &map.trigger_volumes[0];
        assert_eq!(switch.name, "lift_a");
        assert_eq!(switch.target_tag, "lift_platform");
        assert_eq!(switch.command, 3, "go_to_path_node");
        assert_eq!(switch.command_arg, "wp_b");
        assert_eq!(switch.fire_mode, 1, "multiple");
        assert!((switch.rearm_ms - 250.0).abs() < 1e-6);
        assert!(!switch.enabled_on_spawn);
        assert_eq!(switch.on_fire, "open_lift");
        assert_eq!(switch.on_exit, "close_lift");
        assert!(switch.tags.contains(&"platform".to_string()));
        // A switch is press-to-activate by definition; an authored `activation`
        // is warned about and discarded rather than honored.
        assert_eq!(switch.activation, 1);

        let records = crate::trigger_volumes::encode_trigger_volumes_section(&map.trigger_volumes)
            .expect("one trigger produces a PRL section");
        assert_eq!(records.triggers[0].activation, 1);
        assert_eq!(records.triggers[0].on_fire, "open_lift");
    }

    fn grouped_brush_test_map() -> &'static str {
        r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
{
( 256 0 -32 ) ( 257 0 -32 ) ( 256 1 -32 ) static_tex 0 0 0 1 1
( 256 0  32 ) ( 256 1  32 ) ( 257 0  32 ) static_tex 0 0 0 1 1
( 192 0 0 ) ( 192 1 0 ) ( 192 0 1 ) static_tex 0 0 0 1 1
( 320 0 0 ) ( 320 0 1 ) ( 320 1 0 ) static_tex 0 0 0 1 1
( 256 -64 0 ) ( 256 -64 1 ) ( 257 -64 0 ) static_tex 0 0 0 1 1
( 256  64 0 ) ( 257  64 0 ) ( 256  64 1 ) static_tex 0 0 0 1 1
}
}
// entity 1
{
"classname" "func_group"
"_tb_type" "_tb_group"
"_tb_name" "grouped_static_brush"
"_tb_id" "1"
{
( 384 0 -32 ) ( 385 0 -32 ) ( 384 1 -32 ) group_tex 0 0 0 1 1
( 384 0  32 ) ( 384 1  32 ) ( 385 0  32 ) group_tex 0 0 0 1 1
( 320 0 0 ) ( 320 1 0 ) ( 320 0 1 ) group_tex 0 0 0 1 1
( 448 0 0 ) ( 448 0 1 ) ( 448 1 0 ) group_tex 0 0 0 1 1
( 384 -64 0 ) ( 384 -64 1 ) ( 385 -64 0 ) group_tex 0 0 0 1 1
( 384  64 0 ) ( 385  64 0 ) ( 384  64 1 ) group_tex 0 0 0 1 1
}
}
"#
    }

    fn empty_editor_group_test_map() -> &'static str {
        r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
{
( 256 0 -32 ) ( 257 0 -32 ) ( 256 1 -32 ) static_tex 0 0 0 1 1
( 256 0  32 ) ( 256 1  32 ) ( 257 0  32 ) static_tex 0 0 0 1 1
( 192 0 0 ) ( 192 1 0 ) ( 192 0 1 ) static_tex 0 0 0 1 1
( 320 0 0 ) ( 320 0 1 ) ( 320 1 0 ) static_tex 0 0 0 1 1
( 256 -64 0 ) ( 256 -64 1 ) ( 257 -64 0 ) static_tex 0 0 0 1 1
( 256  64 0 ) ( 257  64 0 ) ( 256  64 1 ) static_tex 0 0 0 1 1
}
}
// entity 1
{
"classname" "func_group"
"origin" "64 0 0"
"_tb_type" "_tb_group"
"_tb_name" "empty_marker"
"_tb_id" "2"
}
"#
    }

    fn point_entity_with_trenchbroom_metadata_map() -> &'static str {
        r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
}
// entity 1
{
"classname" "prop_mesh"
"origin" "0 0 0"
"model" "models/crate.glb"
"_tb_group" "1"
"_tb_name" "crate prop"
"_tb_id" "3"
"_tb_type" "_tb_entity"
}
"#
    }

    #[test]
    fn parses_test_map() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("campaign-test.map should parse without error");

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
            .expect("campaign-test.map should parse without error");

        let classnames: Vec<&str> = map_data
            .entities
            .iter()
            .map(|e| e.classname.as_str())
            .collect();
        assert!(classnames.contains(&"worldspawn"));
        assert!(classnames.contains(&"player_spawn"));

        // Point entities must have 0 brushes.
        for point_classname in &["player_spawn", "player", "light", "light_spot", "fog_lamp"] {
            let brush_count = map_data
                .entity_brushes
                .iter()
                .find(|(cls, _)| cls == point_classname)
                .map(|(_, count)| *count)
                .unwrap_or(0);
            assert_eq!(
                brush_count, 0,
                "{point_classname} is a point entity and should have 0 brushes"
            );
        }

        // Brush entities must have > 0 brushes.
        let fog_volume_brush_count = map_data
            .entity_brushes
            .iter()
            .find(|(cls, _)| cls == "fog_volume")
            .map(|(_, count)| *count)
            .unwrap_or(0);
        assert!(
            fog_volume_brush_count > 0,
            "fog_volume is a brush entity and should have > 0 brushes"
        );
    }

    #[test]
    fn map_entities_collected_strip_reserved_keys_and_lights() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("campaign-test.map should parse without error");

        assert!(
            !map_data.map_entities.is_empty(),
            "should have at least one collected map entity"
        );

        // player_spawn must be present; check its reserved keys are stripped.
        let me = map_data
            .map_entities
            .iter()
            .find(|e| e.classname == "player_spawn")
            .expect("player_spawn should be in map_entities");
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
        assert!(
            map_data
                .map_entities
                .iter()
                .all(|e| e.classname != "worldspawn"),
            "worldspawn must not appear in map_entities",
        );
    }

    #[test]
    fn kinematic_mover_brush_is_absent_from_static_world_inputs_and_geometry() {
        let map_data = parse_inline_map(&kinematic_test_map("wp_b"))
            .expect("valid kinematic map should parse");

        assert_eq!(map_data.kinematic_movers.len(), 1);
        assert_eq!(map_data.brush_volumes.len(), 1);
        assert!(
            map_data
                .brush_volumes
                .iter()
                .flat_map(|brush| brush.sides.iter())
                .all(|side| side.texture != "mover_tex"),
            "kinematic_mover brush texture leaked into static brush_volumes"
        );

        let result = crate::partition::partition(&map_data.brush_volumes)
            .expect("static world brush should partition");
        let static_geometry =
            crate::geometry::extract_geometry(&result.faces, &result.tree, &HashSet::new());
        assert!(
            static_geometry
                .texture_names
                .names
                .iter()
                .all(|name| name != "mover_tex"),
            "kinematic_mover brush leaked into static GeometrySection"
        );
    }

    #[test]
    fn dynamic_light_carrier_resolves_to_mover_local_offset() {
        let map_text = kinematic_test_map("wp_b").replacen(
            r#""origin" "0 0 0"
}
// entity 3"#,
            r#""origin" "0 0 0"
}
// entity 4
{
"classname" "light_dynamic"
"origin" "0 0 64"
"light" "300"
"_color" "255 255 255"
"_falloff_range" "512"
"carrier" "lift_a"
}
// entity 3"#,
            1,
        );
        let map_data = parse_inline_map(&map_text).expect("carrier map should parse");

        assert_eq!(map_data.lights.len(), 1);
        assert!(map_data.lights[0].is_dynamic);
        assert_eq!(map_data.lights[0].carrier, "lift_a");
        assert_eq!(map_data.kinematic_movers.len(), 1);
        assert_eq!(map_data.kinematic_movers[0].name, "lift_a");
        assert_eq!(map_data.carried_light_links.len(), 1);
        let link = &map_data.carried_light_links[0];
        assert_eq!(link.source_light_index, 0);
        assert_eq!(link.mover_id, map_data.kinematic_movers[0].mover_id);
        assert_eq!(
            link.local_offset,
            (map_data.lights[0].origin - map_data.kinematic_movers[0].origin)
                .to_array()
                .map(|value| value as f32),
        );
    }

    // Regression: a finite compiler offset could narrow to infinity in the V6
    // carried-light record and later poison GPU light and influence positions.
    #[test]
    fn dynamic_light_carrier_unrepresentable_local_offset_warns_and_stays_unbound() {
        let map_text = kinematic_map_with_light(
            kinematic_test_map("wp_b"),
            "light_dynamic",
            "0 0 64",
            Some("lift_a"),
            "",
        );
        let mut map_data = parse_inline_map(&map_text).expect("carrier map should parse");
        map_data.lights[0].origin = DVec3::new(3.0e38, 0.0, 0.0);
        map_data.kinematic_movers[0].origin = DVec3::new(-3.0e38, 0.0, 0.0);
        let derived_offset = map_data.lights[0].origin - map_data.kinematic_movers[0].origin;
        assert!((map_data.lights[0].origin.x as f32).is_finite());
        assert!((map_data.kinematic_movers[0].origin.x as f32).is_finite());
        assert!(derived_offset.is_finite());
        assert!(!(derived_offset.x as f32).is_finite());

        let capture = LogCapture::start();
        let links = resolve_carried_light_links(&mut map_data.lights, &map_data.kinematic_movers);

        capture.assert_logged_once(
            Level::Warn,
            "outside the runtime f32 range; leaving it unbound",
        );
        assert!(links.is_empty());
    }

    #[test]
    fn dynamic_light_carrier_missing_mover_warns_and_leaves_light_unbound() {
        let map_text = kinematic_map_with_light(
            kinematic_test_map("wp_b"),
            "light_dynamic",
            "0 0 64",
            Some("missing_lift"),
            "",
        );
        let capture = LogCapture::start();
        let map_data = parse_inline_map(&map_text).expect("missing carrier must not fail parsing");

        let light = &map_data.lights[0];
        capture.assert_logged_once(
            Level::Warn,
            &format!(
                "dynamic light at {:?} carrier `missing_lift` matches no kinematic_mover",
                light.origin
            ),
        );
        assert!(
            map_data.carried_light_links.is_empty(),
            "a missing carrier must leave the dynamic light unbound"
        );
    }

    #[test]
    fn dynamic_light_carrier_duplicate_movers_warns_and_leaves_light_unbound() {
        let map_text = kinematic_map_with_duplicate_named_mover(kinematic_map_with_light(
            kinematic_test_map("wp_b"),
            "light_dynamic",
            "0 0 64",
            Some("lift_a"),
            "",
        ));
        let capture = LogCapture::start();
        let map_data =
            parse_inline_map(&map_text).expect("duplicate mover names must not fail parsing");

        assert_eq!(
            map_data.kinematic_movers.len(),
            2,
            "the carrier diagnostic must not become a global unique-name validation"
        );
        capture.assert_logged_once(
            Level::Warn,
            &format!(
                "dynamic light at {:?} carrier `lift_a`",
                map_data.lights[0].origin
            ),
        );
        capture.assert_logged_once(
            Level::Warn,
            "carrier `lift_a` matches duplicate kinematic_movers `lift_a` (id 0), `lift_a` (id 1)",
        );
        assert!(
            map_data.carried_light_links.is_empty(),
            "a light cannot bind to more than one mover"
        );
    }

    #[test]
    fn baked_light_carrier_warns_clears_binding_and_preserves_static_light_input() {
        let bound_map = kinematic_map_with_light(
            kinematic_test_map("wp_b"),
            "light",
            "0 0 64",
            Some("lift_a"),
            "",
        );
        let capture = LogCapture::start();
        let bound = parse_inline_map(&bound_map).expect("baked carrier must not fail parsing");

        capture.assert_logged_once(
            Level::Warn,
            &format!(
                "baked light at {:?} ignores carrier `lift_a`; baked lights cannot be carried",
                bound.lights[0].origin
            ),
        );
        assert!(!bound.lights[0].is_dynamic);
        assert!(bound.lights[0].carrier.is_empty());
        assert!(bound.carried_light_links.is_empty());

        capture.clear();
        let unbound_map =
            kinematic_map_with_light(kinematic_test_map("wp_b"), "light", "0 0 64", None, "");
        let unbound = parse_inline_map(&unbound_map).expect("unbound baked light must parse");

        assert_eq!(
            bound.lights[0], unbound.lights[0],
            "clearing a baked carrier must leave the static-bake input unchanged"
        );
    }

    // Regression: dynamic bake-only carrier links silently disappeared during AlphaLights packing.
    #[test]
    fn dynamic_bake_only_light_carrier_warns_and_leaves_light_unbound() {
        let map_text = kinematic_map_with_light(
            kinematic_test_map("wp_b"),
            "light_dynamic",
            "0 0 64",
            Some("lift_a"),
            "\"_bake_only\" \"1\"\n",
        );
        let capture = LogCapture::start();
        let map_data =
            parse_inline_map(&map_text).expect("dynamic bake-only carrier must not fail parsing");

        capture.assert_logged_once(
            Level::Warn,
            &format!(
                "dynamic bake-only light at {:?} ignores carrier `lift_a`; bake-only lights have no runtime presence and cannot be carried",
                map_data.lights[0].origin
            ),
        );
        assert!(map_data.lights[0].is_dynamic);
        assert!(map_data.lights[0].bake_only);
        assert!(map_data.lights[0].carrier.is_empty());
        assert!(
            map_data.carried_light_links.is_empty(),
            "a bake-only light cannot emit a runtime mover link"
        );
    }

    #[test]
    fn dynamic_spot_carrier_spinner_capability_warns_but_retains_position_link() {
        let mover_map = kinematic_test_map("wp_b").replacen(
            "\"start_on_spawn\" \"1\"\n",
            "\"start_on_spawn\" \"1\"\n\"spin_axis\" \"0 0 2\"\n\"spin_speed\" \"0\"\n",
            1,
        );
        let map_text = kinematic_map_with_light(
            mover_map,
            "light_dynamic_spot",
            "0 0 64",
            Some("lift_a"),
            "\"angles\" \"-90 0 0\"\n\"_cone\" \"30\"\n",
        );
        let capture = LogCapture::start();
        let map_data = parse_inline_map(&map_text).expect("spinner-capable spot map must parse");

        let mover = &map_data.kinematic_movers[0];
        assert_ne!(
            mover.spin_axis, [0.0; 3],
            "the warning contract keys spinner capability on its authored axis"
        );
        assert_eq!(
            mover.spin_speed_deg_s, 0.0,
            "the test proves a zero initial spin speed does not suppress the warning"
        );
        capture.assert_logged_once(
            Level::Warn,
            &format!(
                "dynamic spot light at {:?} carrier `lift_a`",
                map_data.lights[0].origin
            ),
        );
        capture.assert_logged_once(
            Level::Warn,
            "spinner-capable kinematic_mover `lift_a` (id 0); cone re-aim under rotation is deferred, carrying position only",
        );
        assert_eq!(
            map_data.carried_light_links.len(),
            1,
            "the spot retains its carried-position relation despite the deferred cone aim"
        );
    }

    #[test]
    fn blank_or_cleared_dynamic_light_carrier_is_silent_and_unbound() {
        let map_text = kinematic_map_with_light(
            kinematic_map_with_light(
                kinematic_test_map("wp_b"),
                "light_dynamic",
                "0 0 64",
                None,
                "",
            ),
            "light_dynamic",
            "0 0 96",
            Some("   "),
            "",
        );
        let capture = LogCapture::start();
        let map_data = parse_inline_map(&map_text).expect("blank carriers must parse normally");

        assert!(map_data.lights.iter().all(|light| light.is_dynamic));
        assert!(map_data.lights.iter().all(|light| light.carrier.is_empty()));
        assert!(map_data.carried_light_links.is_empty());
        assert!(
            capture
                .records()
                .iter()
                .all(|record| record.level != Level::Warn),
            "absent and cleared carriers must not produce a warning"
        );
    }

    #[test]
    fn trenchbroom_func_group_brushes_are_flattened_into_static_world() {
        // Regression: TrenchBroom editor groups are saved as `func_group`
        // brush entities; treating every brush entity as non-static made
        // grouped brushes disappear from the runtime.
        let map_data =
            parse_inline_map(grouped_brush_test_map()).expect("grouped brush map should parse");

        assert_eq!(
            map_data.brush_volumes.len(),
            2,
            "worldspawn and editor-group brushes should both feed static BSP inputs"
        );
        assert!(
            map_data
                .brush_volumes
                .iter()
                .flat_map(|brush| brush.sides.iter())
                .any(|side| side.texture == "group_tex"),
            "grouped brush texture should survive in static brush_volumes"
        );
        assert!(
            map_data
                .map_entities
                .iter()
                .all(|entity| entity.classname != "func_group"),
            "editor group marker must not become a runtime classname-dispatch entity"
        );
        assert_eq!(map_data.assemblies.len(), 1);
        assert_eq!(map_data.assemblies[0].provenance, "grouped_static_brush");
        assert_eq!(map_data.assemblies[0].group_id, "1");
        assert_eq!(map_data.assemblies[0].linked_group_id, None);
        assert_eq!(map_data.brush_assembly, vec![None, Some(0)]);
    }

    #[test]
    fn empty_trenchbroom_func_group_is_not_a_runtime_entity() {
        // Regression: empty editor-group markers with an origin used to flow
        // into generic runtime entity records.
        let map_data = parse_inline_map(empty_editor_group_test_map())
            .expect("empty editor group map should parse");

        assert_eq!(
            map_data.brush_volumes.len(),
            1,
            "empty editor group should not add static brushes"
        );
        assert!(
            map_data
                .map_entities
                .iter()
                .all(|entity| entity.classname != "func_group"),
            "empty editor group marker must not become a runtime classname-dispatch entity"
        );
        assert_eq!(map_data.assemblies.len(), 1);
        assert_eq!(map_data.assemblies[0].provenance, "empty_marker");
        assert_eq!(map_data.assemblies[0].group_id, "2");
        assert!(
            map_data.brush_assembly.iter().all(Option::is_none),
            "an empty marker must not associate any retained brush"
        );
    }

    #[test]
    fn func_group_normalizes_braced_and_bare_linked_group_ids() {
        let braced = grouped_brush_test_map().replacen(
            "\"_tb_id\" \"1\"",
            "\"_tb_id\" \"1\"\n\"_tb_linked_group_id\" \"{linked-guid}\"",
            1,
        );
        let bare = braced.replacen("\"{linked-guid}\"", "\"linked-guid\"", 1);

        let braced = parse_inline_map(&braced).expect("braced linked group should parse");
        let bare = parse_inline_map(&bare).expect("bare linked group should parse");

        assert_eq!(braced.assemblies.len(), 1);
        assert_eq!(bare.assemblies.len(), 1);
        assert_eq!(
            braced.assemblies[0].linked_group_id,
            Some("linked-guid".to_string())
        );
        assert_eq!(
            braced.assemblies[0].linked_group_id, bare.assemblies[0].linked_group_id,
            "the captured GUID is normalized only; no relation is derived from it"
        );
    }

    #[test]
    fn unnamed_func_group_uses_its_stable_id_as_provenance() {
        let map_text = grouped_brush_test_map().replacen(
            "\"_tb_name\" \"grouped_static_brush\"",
            "\"_tb_name\" \"   \"",
            1,
        );
        let map_data = parse_inline_map(&map_text).expect("unnamed group should parse");

        assert_eq!(map_data.assemblies[0].provenance, "group 1");
    }

    #[test]
    fn same_named_func_groups_remain_distinct_by_group_id() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|path| path.parent())
            .expect("workspace root")
            .join("content/dev/maps/kinematic-platform.map");
        let map_text = std::fs::read_to_string(path)
            .expect("kinematic-platform map fixture should be readable")
            .replacen(
                "\"_tb_name\" \"Ambient Room Light\"",
                "\"_tb_name\" \"Spot lights\"",
                1,
            );

        let map_data = parse_inline_map(&map_text).expect("same-named markers should parse");

        assert_eq!(map_data.assemblies.len(), 2);
        assert_eq!(map_data.assemblies[0].provenance, "Spot lights");
        assert_eq!(map_data.assemblies[1].provenance, "Spot lights");
        assert_eq!(map_data.assemblies[0].group_id, "1");
        assert_eq!(map_data.assemblies[1].group_id, "2");
    }

    #[test]
    fn map_entity_key_values_strip_trenchbroom_metadata() {
        let map_data = parse_inline_map(point_entity_with_trenchbroom_metadata_map())
            .expect("point entity with editor metadata should parse");
        let entity = map_data
            .map_entities
            .iter()
            .find(|entity| entity.classname == "prop_mesh")
            .expect("prop_mesh should become a runtime map entity");

        assert!(
            entity
                .key_values
                .iter()
                .any(|(key, value)| { key == "model" && value == "models/crate.glb" }),
            "game-authored KVPs should remain available to runtime dispatch"
        );
        assert!(
            entity
                .key_values
                .iter()
                .all(|(key, _)| !key.starts_with("_tb_")),
            "TrenchBroom editor metadata must not leak into runtime KVPs"
        );
    }

    #[test]
    fn map_entity_key_values_serialize_in_canonical_order() {
        // Regression: HashMap iteration made MapEntity bytes vary across compiler processes.
        let map_with_properties = |properties: &str| {
            format!(
                r#"
// entity 0
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
}}
// entity 1
{{
"classname" "billboard_emitter"
"origin" "0 0 0"
{properties}
}}
"#
            )
        };
        let first = parse_inline_map(&map_with_properties(concat!(
            "\"sprite\" \"campfire\"\n",
            "\"rate\" \"9.5\"\n",
            "\"drag\" \"0.4\"",
        )))
        .expect("first property order should parse");
        let reordered = parse_inline_map(&map_with_properties(concat!(
            "\"drag\" \"0.4\"\n",
            "\"sprite\" \"campfire\"\n",
            "\"rate\" \"9.5\"",
        )))
        .expect("reordered properties should parse");

        let expected = vec![
            ("drag".to_string(), "0.4".to_string()),
            ("rate".to_string(), "9.5".to_string()),
            ("sprite".to_string(), "campfire".to_string()),
        ];
        assert_eq!(first.map_entities[0].key_values, expected);
        assert_eq!(reordered.map_entities[0].key_values, expected);

        let first_bytes = crate::pack::encode_map_entities(&first.map_entities)
            .expect("point entity should produce MapEntity section")
            .to_bytes();
        let reordered_bytes = crate::pack::encode_map_entities(&reordered.map_entities)
            .expect("point entity should produce MapEntity section")
            .to_bytes();
        assert_eq!(first_bytes, reordered_bytes);
    }

    #[test]
    fn kinematic_mover_with_two_waypoints_emits_kinematic_section() {
        let map_data = parse_inline_map(&kinematic_test_map("wp_b"))
            .expect("valid kinematic map should parse");
        let mut texture_names =
            postretro_level_format::texture_names::TextureNamesSection { names: Vec::new() };
        let section = crate::kinematic_geometry::encode_kinematic_geometry_section(
            &map_data.kinematic_movers,
            &map_data.kinematic_waypoints,
            &[],
            &[],
            &mut texture_names,
        )
        .expect("movers should emit kinematic geometry");

        assert_eq!(section.movers.len(), 1);
        assert_eq!(section.waypoints.len(), 2);
        assert_eq!(section.movers[0].path, "wp_a");
        assert_eq!(section.movers[0].tags, ["platform", "arena"]);
        assert!(!section.movers[0].vertices.is_empty());
        assert!(
            section.movers[0]
                .vertices
                .iter()
                .all(|vertex| vertex.lightmap_uv == [0, 0] && vertex.lightmap_layer == 0),
            "mover vertices must not carry static lightmap data"
        );
        assert!(
            texture_names.names.iter().any(|name| name == "mover_tex"),
            "mover texture must be added to shared TextureNames"
        );
    }

    #[test]
    fn kinematic_mover_spin_properties_encode_in_engine_space() {
        let map_data = parse_inline_map(&spinning_kinematic_test_map("wp_b"))
            .expect("spinning kinematic map should parse");
        let mut texture_names =
            postretro_level_format::texture_names::TextureNamesSection { names: Vec::new() };
        let section = crate::kinematic_geometry::encode_kinematic_geometry_section(
            &map_data.kinematic_movers,
            &map_data.kinematic_waypoints,
            &[],
            &[],
            &mut texture_names,
        )
        .expect("spinning mover should emit kinematic geometry");

        let mover = &section.movers[0];
        assert_eq!(mover.spin_axis, [0.0, 1.0, 0.0]);
        assert_eq!(mover.spin_speed_deg_s, 90.0);
        assert_eq!(mover.spin_accel_deg_s2, 12.5);
        assert!(mover.carry_yaw);
    }

    #[test]
    fn kinematic_mover_blocking_properties_parse_and_reach_the_prl_record() {
        let map_text = spinning_kinematic_test_map("wp_b").replacen(
            "\"carry_yaw\" \"1\"\n",
            "\"carry_yaw\" \"1\"\n\"block_policy\" \"stop\"\n\"crush_damage\" \"12.5\"\n\"crush_interval_ms\" \"250\"\n\"auto_close_ms\" \"1750\"\n\"open_event\" \"  lift_open  \"\n\"close_event\" \"lift_closed\"\n\"blocked_event\" \"  lift_blocked\"\n\"crush_event\" \"\"\n",
            1,
        );
        let map_data = parse_inline_map(&map_text).expect("blocking mover authoring must parse");
        let authored = &map_data.kinematic_movers[0];
        assert_eq!(authored.block_policy, "stop");
        assert!((authored.crush_damage - 12.5).abs() < f32::EPSILON);
        assert!((authored.crush_interval_ms - 250.0).abs() < f32::EPSILON);
        assert_eq!(authored.auto_close_ms, Some(1750.0));
        assert_eq!(authored.open_event.as_deref(), Some("lift_open"));
        assert_eq!(authored.close_event.as_deref(), Some("lift_closed"));
        assert_eq!(authored.blocked_event.as_deref(), Some("lift_blocked"));
        assert_eq!(authored.crush_event, None);

        let mut texture_names =
            postretro_level_format::texture_names::TextureNamesSection { names: Vec::new() };
        let section = crate::kinematic_geometry::encode_kinematic_geometry_section(
            &map_data.kinematic_movers,
            &map_data.kinematic_waypoints,
            &[],
            &[],
            &mut texture_names,
        )
        .expect("mover authoring must encode into kinematic geometry");
        let record = &section.movers[0];
        assert_eq!(record.block_policy, "stop");
        assert!((record.crush_damage - 12.5).abs() < f32::EPSILON);
        assert!((record.crush_interval_ms - 250.0).abs() < f32::EPSILON);
        assert_eq!(record.auto_close_ms, Some(1750.0));
        assert_eq!(record.open_event.as_deref(), Some("lift_open"));
        assert_eq!(record.close_event.as_deref(), Some("lift_closed"));
        assert_eq!(record.blocked_event.as_deref(), Some("lift_blocked"));
        assert_eq!(record.crush_event, None);
    }

    // Regression: collapsing both forms to zero made an authored disable
    // unable to override a positive mod-wide default.
    #[test]
    fn kinematic_mover_auto_close_preserves_absent_blank_and_explicit_zero() {
        let absent = parse_inline_map(&kinematic_test_map("wp_b")).unwrap();
        assert_eq!(absent.kinematic_movers[0].auto_close_ms, None);

        let blank_text = kinematic_test_map("wp_b").replacen(
            "\"start_on_spawn\" \"1\"\n",
            "\"start_on_spawn\" \"1\"\n\"auto_close_ms\" \"\"\n",
            1,
        );
        let blank = parse_inline_map(&blank_text).unwrap();
        assert_eq!(blank.kinematic_movers[0].auto_close_ms, None);

        let zero_text = kinematic_test_map("wp_b").replacen(
            "\"start_on_spawn\" \"1\"\n",
            "\"start_on_spawn\" \"1\"\n\"auto_close_ms\" \"0\"\n",
            1,
        );
        let zero = parse_inline_map(&zero_text).unwrap();
        assert_eq!(zero.kinematic_movers[0].auto_close_ms, Some(0.0));
    }

    #[test]
    fn kinematic_mover_rejects_invalid_blocking_properties() {
        for (property, value) in [
            ("block_policy", "pause"),
            ("crush_damage", "-1"),
            ("crush_interval_ms", "-1"),
            ("auto_close_ms", "nan"),
        ] {
            let map_text = kinematic_test_map("wp_b").replacen(
                "\"start_on_spawn\" \"1\"\n",
                &format!("\"start_on_spawn\" \"1\"\n\"{property}\" \"{value}\"\n"),
                1,
            );
            assert!(
                parse_inline_map(&map_text).is_err(),
                "{property}={value} must be rejected"
            );
        }
    }

    #[test]
    fn kinematic_mover_invalid_carry_yaw_reports_supported_values() {
        let map_text = spinning_kinematic_test_map("wp_b")
            .replace("\"carry_yaw\" \"1\"", "\"carry_yaw\" \"maybe\"");
        let err = parse_inline_map(&map_text).expect_err("invalid carry_yaw must reject");

        assert!(
            err.to_string()
                .contains("`carry_yaw` must be `0`, `1`, `false`, `true`, `False`, or `True`"),
            "diagnostic should name every supported carry_yaw value, got: {err}"
        );
    }

    #[test]
    fn kinematic_mover_missing_speed_uses_fgd_default() {
        let map_text = kinematic_test_map("wp_b").replace("\"speed\" \"2.5\"\n", "");
        let map_data =
            parse_inline_map(&map_text).expect("missing speed should use the FGD default");

        assert_eq!(map_data.kinematic_movers.len(), 1);
        assert_eq!(map_data.kinematic_movers[0].speed, 1.0);
    }

    #[test]
    fn kinematic_mover_path_with_fewer_than_two_waypoints_rejects() {
        let err = parse_inline_map(&kinematic_test_map(""))
            .expect_err("single-waypoint mover path must reject");
        assert!(
            err.to_string().contains("fewer than two waypoints"),
            "diagnostic should name the invalid path length, got: {err}"
        );
    }

    #[test]
    fn pure_rotator_with_one_waypoint_is_accepted() {
        let map_data = parse_inline_map(&spinning_kinematic_test_map(""))
            .expect("non-zero spin should permit a one-waypoint pure rotator");

        assert_eq!(map_data.kinematic_movers.len(), 1);
        assert_eq!(map_data.kinematic_movers[0].spin_speed_deg_s, 90.0);
    }

    #[test]
    fn zero_speed_spin_with_nonzero_axis_is_normalized() {
        let map_text = spinning_kinematic_test_map("wp_b")
            .replace("\"spin_axis\" \"0 0 2\"", "\"spin_axis\" \"0 3 0\"")
            .replace("\"spin_speed\" \"90\"", "\"spin_speed\" \"0\"");
        let map_data = parse_inline_map(&map_text)
            .expect("a normalized authored axis should be retained while spin is at rest");

        assert_eq!(map_data.kinematic_movers[0].spin_axis, [-1.0, 0.0, 0.0]);
        assert_eq!(map_data.kinematic_movers[0].spin_speed_deg_s, 0.0);
    }

    #[test]
    fn kinematic_mover_rejects_non_finite_spin_speed() {
        let map_text = spinning_kinematic_test_map("wp_b")
            .replace("\"spin_speed\" \"90\"", "\"spin_speed\" \"nan\"");
        let err = parse_inline_map(&map_text).expect_err("non-finite spin speed must reject");

        assert!(err.to_string().contains("spin_speed") && err.to_string().contains("not finite"));
    }

    // Regression: a degree-domain pure rotator could pass admission but convert to zero radians.
    #[test]
    fn kinematic_mover_rejects_nonzero_spin_speed_that_underflows_in_radians() {
        let map_text = spinning_kinematic_test_map("")
            .replace("\"spin_speed\" \"90\"", "\"spin_speed\" \"1e-45\"");
        let err = parse_inline_map(&map_text)
            .expect_err("a non-zero spin speed must stay non-zero in the runtime unit");

        assert!(
            err.to_string().contains("spin_speed")
                && err.to_string().contains("conversion to radians/sec")
        );
    }

    #[test]
    fn kinematic_mover_rejects_negative_or_non_finite_spin_accel() {
        for value in ["-1", "nan"] {
            let map_text = spinning_kinematic_test_map("wp_b").replace(
                "\"spin_accel\" \"12.5\"",
                &format!("\"spin_accel\" \"{value}\""),
            );
            let err = parse_inline_map(&map_text)
                .expect_err("negative or non-finite spin acceleration must reject");
            assert!(err.to_string().contains("spin_accel"));
        }
    }

    // Regression: positive degree-domain acceleration could convert to zero and snap the ramp.
    #[test]
    fn kinematic_mover_rejects_positive_spin_accel_that_underflows_in_radians() {
        let map_text = spinning_kinematic_test_map("wp_b")
            .replace("\"spin_accel\" \"12.5\"", "\"spin_accel\" \"1e-45\"");
        let err = parse_inline_map(&map_text)
            .expect_err("positive spin acceleration must stay positive in the runtime unit");

        assert!(
            err.to_string().contains("spin_accel")
                && err.to_string().contains("conversion to radians/sec²")
        );
    }

    #[test]
    fn kinematic_mover_rejects_invalid_spin_axis_for_active_spin() {
        for value in ["0 0 0", "nan 0 0"] {
            let map_text = spinning_kinematic_test_map("wp_b").replace(
                "\"spin_axis\" \"0 0 2\"",
                &format!("\"spin_axis\" \"{value}\""),
            );
            let err = parse_inline_map(&map_text)
                .expect_err("active spin requires a finite, non-zero axis");
            assert!(err.to_string().contains("spin_axis"));
        }
    }

    #[test]
    fn kinematic_mover_waypoint_cycle_rejects() {
        let err = parse_inline_map(&kinematic_test_map_with_nexts("wp_b", "wp_a"))
            .expect_err("cyclic mover path must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("waypoint cycle") && msg.contains("wp_a"),
            "diagnostic should name the waypoint cycle, got: {err}"
        );
    }

    #[test]
    fn duplicate_kinematic_waypoint_names_reject() {
        let map_text = kinematic_test_map("wp_b").replace("\"name\" \"wp_b\"", "\"name\" \"wp_a\"");
        let err = parse_inline_map(&map_text)
            .expect_err("duplicate waypoint names must reject before PRL emission");

        assert!(
            err.to_string()
                .contains("duplicate kinematic_waypoint name"),
            "diagnostic should name duplicate waypoint names, got: {err}"
        );
    }

    #[test]
    fn non_finite_kinematic_waypoint_origin_rejects() {
        let map_text =
            kinematic_test_map("wp_b").replace("\"origin\" \"0 0 64\"", "\"origin\" \"0 nan 64\"");
        let err = parse_inline_map(&map_text)
            .expect_err("non-finite waypoint origins must reject before PRL emission");

        assert!(
            err.to_string().contains("origin is non-finite"),
            "diagnostic should name non-finite waypoint origin, got: {err}"
        );
    }

    #[test]
    fn zero_length_kinematic_path_segment_rejects() {
        let map_text =
            kinematic_test_map("wp_b").replace("\"origin\" \"0 0 64\"", "\"origin\" \"0 0 0\"");
        let err = parse_inline_map(&map_text)
            .expect_err("zero-length adjacent waypoint segments must reject");

        assert!(
            err.to_string().contains("zero-length segment"),
            "diagnostic should name zero-length path segment, got: {err}"
        );
    }

    #[test]
    fn near_zero_kinematic_path_segment_rejects() {
        let map_text = kinematic_test_map("wp_b")
            .replace("\"origin\" \"0 0 64\"", "\"origin\" \"0 0 0.000001\"");
        let err = parse_inline_map(&map_text)
            .expect_err("near-zero adjacent waypoint segments must reject");

        assert!(
            err.to_string().contains("zero-length segment"),
            "diagnostic should name near-zero path segment, got: {err}"
        );
    }

    #[test]
    fn brush_sides_have_valid_vertices() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("campaign-test.map should parse without error");

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
            .expect("campaign-test.map should parse without error");

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
            .expect("campaign-test.map should parse without error");

        let player_start = map_data
            .entities
            .iter()
            .find(|e| e.classname == "player_spawn")
            .expect("should have player_spawn");

        let origin = player_start
            .origin
            .expect("player_spawn should have origin");
        assert!(origin.x.is_finite(), "origin x should be finite");
        assert!(origin.y.is_finite(), "origin y should be finite");
        assert!(origin.z.is_finite(), "origin z should be finite");
    }

    // -- Quoted brush-face material encoding --

    /// Extract the material token (the first whitespace-delimited field after
    /// the third point triple) from an encoded brush-plane line, mirroring how
    /// shalrath tokenizes it. Anchors on the third `)` — the close of the point
    /// triples — not the last `)`, since the encoded material token may itself
    /// contain `)` (e.g. `Pack (v1)/metal panel`).
    fn material_token(encoded: &str) -> &str {
        let triples_end = encoded
            .match_indices(')')
            .nth(2)
            .expect("a brush-plane line has three point triples")
            .0;
        let tail = &encoded[triples_end + 1..];
        tail.split_whitespace().next().unwrap()
    }

    #[test]
    fn encode_quoted_texture_makes_brush_plane_tokenizable() {
        // A Standard-format brush-plane line whose material name has spaces is
        // quoted by TrenchBroom. After encoding, the quotes are gone and the
        // interior spaces are the sentinel, so the texture field is one token.
        let line = "( -16 1040 -16 ) ( -16 -16 0 ) ( -16 -16 -16 ) \
             \"Level Eleven Games/Metal-Panel-002\" 0 0 0 1 1\n";
        let encoded = encode_quoted_brush_textures(line);
        assert!(
            !encoded.contains('"'),
            "quotes around the material name must be stripped"
        );
        let token = material_token(&encoded);
        assert!(
            !token.contains(' '),
            "the material field must be a single space-free token: {token:?}"
        );
        assert_eq!(
            decode_brush_texture(token),
            "Level Eleven Games/Metal-Panel-002",
            "decoding the token restores the original spaced name"
        );
        // Trailing projection numbers and the point triples are untouched.
        assert!(encoded.contains("( -16 1040 -16 )"));
        assert!(encoded.trim_end().ends_with(" 0 0 0 1 1"));
    }

    #[test]
    fn encode_handles_material_name_containing_parens() {
        // A quoted material name with a `)` in it broke the old `rfind(')')`
        // anchor: the split landed inside the name. Anchoring on the first `"`
        // avoids that — the name round-trips exactly.
        let original = "Pack (v1)/metal panel";
        let line = format!("( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) \"{original}\" 0 0 0 1 1\n");
        let encoded = encode_quoted_brush_textures(&line);
        assert!(
            !encoded.contains('"'),
            "quotes must be stripped even when the name contains `)`: {encoded}"
        );
        let token = material_token(&encoded);
        assert!(
            !token.contains(' '),
            "the material field must be a single space-free token: {token:?}"
        );
        assert_eq!(decode_brush_texture(token), original);
    }

    #[test]
    fn encode_preserves_literal_value_that_collided_with_old_sentinel() {
        // The old `%20` sentinel corrupted any literal `%20` an author put in a
        // real material name. The path-illegal control-byte sentinel cannot
        // occur in a real name, so a literal `%20` now round-trips losslessly.
        let original = "weird%20name with space";
        let line = format!("( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) \"{original}\" 0 0 0 1 1\n");
        let encoded = encode_quoted_brush_textures(&line);
        let token = material_token(&encoded);
        assert!(
            !token.contains(' '),
            "the material field must be a single space-free token: {token:?}"
        );
        assert_eq!(
            decode_brush_texture(token),
            original,
            "a literal `%20` in the name must survive the round-trip"
        );
    }

    #[test]
    fn encode_handles_valve220_axis_brackets() {
        // Valve 220 faces append `[ x y z off ] [ x y z off ]` UV axes after the
        // material. Only the quoted material is encoded; the brackets and the
        // trailing numbers must survive verbatim.
        let line = "( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) \"Sci Fi Pack/panel\" \
             [ 1 0 0 0 ] [ 0 -1 0 0 ] 0 1 1\n";
        let encoded = encode_quoted_brush_textures(line);
        assert!(!encoded.contains('"'), "quotes must be stripped: {encoded}");
        let token = material_token(&encoded);
        assert_eq!(decode_brush_texture(token), "Sci Fi Pack/panel");
        assert!(
            encoded.contains("[ 1 0 0 0 ] [ 0 -1 0 0 ] 0 1 1"),
            "Valve220 axis brackets and offsets must survive: {encoded}"
        );
    }

    #[test]
    fn encode_leaves_space_free_brush_planes_unchanged() {
        let line = "( -16 1040 -16 ) ( -16 -16 0 ) ( -16 -16 -16 ) \
             concrete_pavement_036 0 0 0 1 1\n";
        assert_eq!(encode_quoted_brush_textures(line), line);
    }

    #[test]
    fn encode_does_not_touch_quoted_entity_kvps() {
        // Entity key/value lines start with `"` and are parsed correctly by
        // shalrath; their quoted values (which legitimately contain spaces)
        // must pass through untouched.
        let kvp = "\"_tags\" \"arena wave 2\"\n";
        assert_eq!(encode_quoted_brush_textures(kvp), kvp);

        let origin = "\"origin\" \"-1000 1464 -24\"\n";
        assert_eq!(encode_quoted_brush_textures(origin), origin);
    }

    #[test]
    fn encode_decode_round_trips_to_original_name() {
        let original = "Level Eleven Games Sci-Fi Texture Pack v1/Metal-Panel-002_Section-001-3";
        let line = format!("( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) \"{original}\" 0 0 0 1 1\n");
        let encoded = encode_quoted_brush_textures(&line);
        assert_eq!(decode_brush_texture(material_token(&encoded)), original);
    }

    #[test]
    fn decode_is_noop_for_unencoded_names() {
        assert_eq!(
            decode_brush_texture("concrete_pavement_036"),
            "concrete_pavement_036"
        );
        assert_eq!(
            decode_brush_texture("collection/metal_panel_01"),
            "collection/metal_panel_01"
        );
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
    /// Vertices appear CCW when viewed from the front, matching the renderer's
    /// front-face convention after upload.
    #[test]
    fn brush_side_winding_aligns_with_side_normal() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("campaign-test.map should parse without error");

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
            .expect("campaign-test.map should parse without error");

        assert!(
            !map_data.brush_volumes.is_empty(),
            "campaign-test.map should produce brush volumes"
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
    fn resolve_fog_lamp_uses_fgd_default_radius_when_not_set() {
        // TrenchBroom does not write FGD default values to the .map file unless
        // the author explicitly sets them. A freshly placed fog_lamp has no
        // `radius` KVP; the compiler falls back to the FGD default of 64 units.
        let props = HashMap::new();
        let v = resolve_fog_lamp(&props, DVec3::ZERO, 1.0, "fog_lamp")
            .expect("missing radius should fall back to FGD default of 64");
        assert!(
            (v.max[0] - 64.0).abs() < 1e-5,
            "default radius should be 64"
        );
    }

    #[test]
    fn resolve_fog_lamp_rejects_non_positive_radius() {
        let mut props = HashMap::new();
        props.insert("radius".to_string(), "0".to_string());
        let err = resolve_fog_lamp(&props, DVec3::ZERO, 1.0, "fog_lamp")
            .expect_err("zero radius must error");
        assert!(format!("{err}").contains("positive"));

        let mut props = HashMap::new();
        props.insert("radius".to_string(), "-1".to_string());
        let err = resolve_fog_lamp(&props, DVec3::ZERO, 1.0, "fog_lamp")
            .expect_err("negative radius must error");
        assert!(format!("{err}").contains("positive"));
    }

    #[test]
    fn resolve_fog_lamp_produces_centered_aabb_and_no_planes() {
        let mut props = HashMap::new();
        props.insert("radius".to_string(), "2.5".to_string());
        let v = resolve_fog_lamp(&props, DVec3::new(1.0, 2.0, 3.0), 1.0, "fog_lamp")
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
    fn resolve_fog_lamp_unit_scales_radius() {
        // IdTech2 inches → meters: radius 64 in maps with scale 0.0254 must
        // yield a 64 * 0.0254 = 1.6256 m sphere centred on the (already
        // unit-scaled) origin.
        let mut props = HashMap::new();
        props.insert("radius".to_string(), "64".to_string());
        let v = resolve_fog_lamp(&props, DVec3::ZERO, 0.0254, "fog_lamp")
            .expect("valid radius should resolve");
        let expected = 64.0_f32 * 0.0254;
        assert!((v.max[0] - expected).abs() < 1e-5);
        assert!((v.min[0] + expected).abs() < 1e-5);
    }

    #[test]
    fn resolve_fog_tube_unit_scales_radius_and_height() {
        // Authored radius 32 inches and height 256 inches with scale 0.0254
        // must produce a 32 * 0.0254-radius capsule whose long axis is
        // 256 * 0.0254 m tip-to-tip.
        let mut props = HashMap::new();
        props.insert("radius".to_string(), "32".to_string());
        props.insert("height".to_string(), "256".to_string());
        let v = resolve_fog_tube(&props, DVec3::ZERO, 0.0254, "fog_tube")
            .expect("valid sizing should resolve");
        let r = 32.0_f32 * 0.0254;
        let h = 256.0_f32 * 0.0254;
        // Default orientation: axis on +Y. AABB spans ±r on X/Z and ±h/2 on Y.
        assert!((v.max[0] - r).abs() < 1e-5);
        assert!((v.max[2] - r).abs() < 1e-5);
        assert!((v.max[1] - h * 0.5).abs() < 1e-5);
        assert!((v.min[1] + h * 0.5).abs() < 1e-5);
    }

    #[test]
    fn resolve_fog_tube_oriented_aabb_inflates_with_pitch_and_yaw() {
        // Capsule: radius 1, height 4. Local axis is +Y; with pitch=0/yaw=0 the
        // axis stays vertical, so the AABB is [-1, -2, -1] – [1, 2, 1].
        let mut props = HashMap::new();
        props.insert("radius".to_string(), "1".to_string());
        props.insert("height".to_string(), "4".to_string());
        let v = resolve_fog_tube(&props, DVec3::ZERO, 1.0, "fog_tube").expect("axis-aligned tube");
        assert_eq!(v.min, [-1.0, -2.0, -1.0]);
        assert_eq!(v.max, [1.0, 2.0, 1.0]);

        // Pitch 90° tilts the axis fully into the horizontal plane (pure -Z).
        // The half-segment now extends along Z, and Y collapses to just the
        // capsule radius. Pitch of 90° with yaw=0 → axis = (0, 0, -1).
        let mut props = HashMap::new();
        props.insert("radius".to_string(), "1".to_string());
        props.insert("height".to_string(), "4".to_string());
        props.insert("pitch".to_string(), "90".to_string());
        let v = resolve_fog_tube(&props, DVec3::ZERO, 1.0, "fog_tube").expect("tilted tube");
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
        let v = resolve_fog_tube(&props, DVec3::ZERO, 1.0, "fog_tube").expect("yawed tube");
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
    fn resolve_fog_tube_uses_fgd_defaults_when_not_set() {
        // TrenchBroom does not write FGD default values to the .map file unless
        // the author explicitly sets them. A freshly placed fog_tube has no
        // `radius` or `height` KVPs; the compiler falls back to the FGD defaults
        // (radius=32, height=128).
        let props = HashMap::new();
        let v = resolve_fog_tube(&props, DVec3::ZERO, 1.0, "fog_tube")
            .expect("missing sizing KVPs should fall back to FGD defaults");
        // Default orientation: axis on +Y. AABB spans ±32 on X/Z, ±64 on Y.
        assert!(
            (v.max[0] - 32.0).abs() < 1e-5,
            "default radius should be 32"
        );
        assert!(
            (v.max[1] - 64.0).abs() < 1e-5,
            "default half-height should be 64"
        );
    }

    // -- fog_volume brush rejection --

    /// Build a GeoMap from inline .map text and return the fog_volume entity's
    /// brush IDs. The caller's map must contain exactly one `fog_volume` entity
    /// with at least one brush.
    fn fog_volume_geo_map_from_str(map_text: &str) -> (GeoMap, Vec<shambler::brush::BrushId>) {
        let shalrath_map: shambler::shalrath::repr::Map = map_text
            .trim()
            .parse()
            .expect("inline map text should parse");
        let geo_map = GeoMap::new(shalrath_map);
        let fog_entity_id = geo_map
            .entities
            .iter()
            .find(|id| get_property(&geo_map, id, "classname").as_deref() == Some("fog_volume"))
            .copied()
            .expect("test map must contain a fog_volume entity");
        let brush_ids = geo_map
            .entity_brushes
            .get(&fog_entity_id)
            .cloned()
            .unwrap_or_default();
        (geo_map, brush_ids)
    }

    fn simple_fog_volume_map() -> (&'static str, f64) {
        (
            r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
}
// entity 1
{
"classname" "fog_volume"
{
( 0 0 -32 ) ( 1 0 -32 ) ( 0 1 -32 ) tex 0 0 0 1 1
( 0 0  32 ) ( 0 1  32 ) ( 1 0  32 ) tex 0 0 0 1 1
( -64 0 0 ) ( -64 1 0 ) ( -64 0 1 ) tex 0 0 0 1 1
(  64 0 0 ) (  64 0 1 ) (  64 1 0 ) tex 0 0 0 1 1
( 0 -64 0 ) ( 0 -64 1 ) ( 1 -64 0 ) tex 0 0 0 1 1
( 0  64 0 ) ( 1  64 0 ) ( 0  64 1 ) tex 0 0 0 1 1
}
}
"#,
            MapFormat::IdTech2.units_to_meters(),
        )
    }

    #[test]
    fn fog_resolvers_default_directional_fields_to_identity_values() {
        let props = HashMap::new();
        let lamp = resolve_fog_lamp(&props, DVec3::ZERO, 1.0, "fog_lamp").expect("lamp resolves");
        assert!((lamp.anisotropy - 0.0).abs() < 1e-6);
        assert!((lamp.ambient_scatter - 1.0).abs() < 1e-6);

        let tube = resolve_fog_tube(&props, DVec3::ZERO, 1.0, "fog_tube").expect("tube resolves");
        assert!((tube.anisotropy - 0.0).abs() < 1e-6);
        assert!((tube.ambient_scatter - 1.0).abs() < 1e-6);

        let (map_text, scale) = simple_fog_volume_map();
        let (geo_map, brush_ids) = fog_volume_geo_map_from_str(map_text);
        let plane_volume = resolve_fog_volume(&geo_map, &brush_ids, &props, scale, "fog_volume")
            .expect("plane volume resolves")
            .expect("box has vertices");
        assert!((plane_volume.anisotropy - 0.0).abs() < 1e-6);
        assert!((plane_volume.ambient_scatter - 1.0).abs() < 1e-6);

        let ellipsoid = resolve_fog_ellipsoid(&geo_map, &brush_ids, &props, scale, "fog_volume")
            .expect("ellipsoid resolves");
        assert!((ellipsoid.anisotropy - 0.0).abs() < 1e-6);
        assert!((ellipsoid.ambient_scatter - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fog_resolvers_translate_scatter_bias_and_clamp_ambient_scatter() {
        let mut props = HashMap::new();
        props.insert("scatter_bias".to_string(), "100".to_string());
        props.insert("ambient_scatter".to_string(), "0".to_string());
        let lamp = resolve_fog_lamp(&props, DVec3::ZERO, 1.0, "fog_lamp").expect("lamp resolves");
        assert!((lamp.anisotropy - 0.9).abs() < 1e-6);
        assert!((lamp.ambient_scatter - 0.0).abs() < 1e-6);

        props.insert("scatter_bias".to_string(), "0".to_string());
        props.insert("ambient_scatter".to_string(), "1".to_string());
        let tube = resolve_fog_tube(&props, DVec3::ZERO, 1.0, "fog_tube").expect("tube resolves");
        assert!((tube.anisotropy - 0.0).abs() < 1e-6);
        assert!((tube.ambient_scatter - 1.0).abs() < 1e-6);

        props.insert("scatter_bias".to_string(), "150".to_string());
        props.insert("ambient_scatter".to_string(), "2".to_string());
        let (map_text, scale) = simple_fog_volume_map();
        let (geo_map, brush_ids) = fog_volume_geo_map_from_str(map_text);
        let plane_volume = resolve_fog_volume(&geo_map, &brush_ids, &props, scale, "fog_volume")
            .expect("plane volume resolves")
            .expect("box has vertices");
        assert!((plane_volume.anisotropy - 0.9).abs() < 1e-6);
        assert!((plane_volume.ambient_scatter - 1.0).abs() < 1e-6);

        props.insert("scatter_bias".to_string(), "-10".to_string());
        props.insert("ambient_scatter".to_string(), "-0.5".to_string());
        let ellipsoid = resolve_fog_ellipsoid(&geo_map, &brush_ids, &props, scale, "fog_volume")
            .expect("ellipsoid resolves");
        assert!((ellipsoid.anisotropy - 0.0).abs() < 1e-6);
        assert!((ellipsoid.ambient_scatter - 0.0).abs() < 1e-6);
    }

    #[test]
    fn resolve_fog_volume_rejects_brush_with_more_than_16_planes() {
        // A 15-sided prism (15 rectangular side faces + top cap + bottom cap = 17
        // face planes) exceeds the per-volume budget of 16.  The brush is a valid
        // convex polyhedron — shambler will compute all 17 planes in the hull.
        //
        // Points are in Quake coordinates (right-handed, Z-up).  Shambler
        // converts the triangle (p0,p1,p2) to a Plane3d whose outward normal is
        // (p2-p0) × (p1-p0), and then tests hull containment as n·v ≤ d.  The
        // side-face planes are specified with p0 on the cylinder surface, p1 one
        // unit above p0 (+Z), and p2 one step along the tangent, giving an
        // outward normal that points away from the prism axis.
        let map_text = r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
}
// entity 1
{
"classname" "fog_volume"
{
( 0 0 32 ) ( 0 1 32 ) ( 1 0 32 ) tex 0 0 0 1 1
( 0 0 -32 ) ( 1 0 -32 ) ( 0 1 -32 ) tex 0 0 0 1 1
( 64.0000 0.0000 0 ) ( 64.0000 0.0000 1 ) ( 64.0000 1.0000 0 ) tex 0 0 0 1 1
( 58.4669 26.0311 0 ) ( 58.4669 26.0311 1 ) ( 58.0602 26.9447 0 ) tex 0 0 0 1 1
( 42.8244 47.5613 0 ) ( 42.8244 47.5613 1 ) ( 42.0812 48.2304 0 ) tex 0 0 0 1 1
( 19.7771 60.8676 0 ) ( 19.7771 60.8676 1 ) ( 18.8260 61.1766 0 ) tex 0 0 0 1 1
( -6.6898 63.6494 0 ) ( -6.6898 63.6494 1 ) ( -7.6843 63.5449 0 ) tex 0 0 0 1 1
( -32.0000 55.4256 0 ) ( -32.0000 55.4256 1 ) ( -32.8660 54.9256 0 ) tex 0 0 0 1 1
( -51.7771 37.6183 0 ) ( -51.7771 37.6183 1 ) ( -52.3649 36.8092 0 ) tex 0 0 0 1 1
( -62.6014 13.3063 0 ) ( -62.6014 13.3063 1 ) ( -62.8094 12.3282 0 ) tex 0 0 0 1 1
( -62.6014 -13.3063 0 ) ( -62.6014 -13.3063 1 ) ( -62.3935 -14.2845 0 ) tex 0 0 0 1 1
( -51.7771 -37.6183 0 ) ( -51.7771 -37.6183 1 ) ( -51.1893 -38.4273 0 ) tex 0 0 0 1 1
( -32.0000 -55.4256 0 ) ( -32.0000 -55.4256 1 ) ( -31.1340 -55.9256 0 ) tex 0 0 0 1 1
( -6.6898 -63.6494 0 ) ( -6.6898 -63.6494 1 ) ( -5.6953 -63.7539 0 ) tex 0 0 0 1 1
( 19.7771 -60.8676 0 ) ( 19.7771 -60.8676 1 ) ( 20.7281 -60.5586 0 ) tex 0 0 0 1 1
( 42.8244 -47.5613 0 ) ( 42.8244 -47.5613 1 ) ( 43.5675 -46.8921 0 ) tex 0 0 0 1 1
( 58.4669 -26.0311 0 ) ( 58.4669 -26.0311 1 ) ( 58.8736 -25.1176 0 ) tex 0 0 0 1 1
}
}
"#;
        let (geo_map, brush_ids) = fog_volume_geo_map_from_str(map_text);
        let props = HashMap::new();
        let scale = MapFormat::IdTech2.units_to_meters();
        let err = resolve_fog_volume(&geo_map, &brush_ids, &props, scale, "fog_volume")
            .expect_err("17-plane brush must exceed the 16-plane budget");
        let msg = format!("{err}");
        assert!(
            msg.contains("16") || msg.contains("simplify"),
            "error should mention the plane limit or instruct simplification: {msg}"
        );
    }

    #[test]
    fn resolve_fog_volume_box_brush_emits_planes_with_inside_when_dot_le_d() {
        // 6-face axis-aligned box fog_volume brush in Quake coords (Z-up,
        // inches). Outward face normals must satisfy `n·p ≤ d` for any
        // interior point — this is the load-bearing invariant at the
        // compiler↔shader seam: the inlined per-step volume test in
        // `fog_volume.wgsl::cs_main` evaluates
        // `dot(pos, plane.xyz) <= plane.w` to test membership and uses
        // `plane.w - dot(pos, plane.xyz)` as the signed distance for the
        // edge_softness fade. If the convention drifts (e.g. inward normals
        // or sign flip on `d`), every primitive volume goes black on the
        // wrong side of every face.
        let map_text = r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
}
// entity 1
{
"classname" "fog_volume"
{
( 0 0 -32 ) ( 1 0 -32 ) ( 0 1 -32 ) tex 0 0 0 1 1
( 0 0  32 ) ( 0 1  32 ) ( 1 0  32 ) tex 0 0 0 1 1
( -64 0 0 ) ( -64 1 0 ) ( -64 0 1 ) tex 0 0 0 1 1
(  64 0 0 ) (  64 0 1 ) (  64 1 0 ) tex 0 0 0 1 1
( 0 -64 0 ) ( 0 -64 1 ) ( 1 -64 0 ) tex 0 0 0 1 1
( 0  64 0 ) ( 1  64 0 ) ( 0  64 1 ) tex 0 0 0 1 1
}
}
"#;
        let (geo_map, brush_ids) = fog_volume_geo_map_from_str(map_text);
        let props = HashMap::new();
        let scale = MapFormat::IdTech2.units_to_meters();
        let volume = resolve_fog_volume(&geo_map, &brush_ids, &props, scale, "fog_volume")
            .expect("6-face axis-aligned box must resolve")
            .expect("brush has vertices, must produce a volume");

        assert_eq!(
            volume.planes.len(),
            6,
            "6-face axis-aligned box must yield 6 face planes"
        );

        // Interior point: world AABB centre. The brush spans (-64,-64,-32)..(64,64,32)
        // in Quake; after `quake_to_engine` swizzle and unit scale, the centre
        // remains (0,0,0). Any reasonable interior point in engine space works
        // for the convention check; (0,0,0) has the merit of being unambiguously
        // inside an origin-centred box.
        let interior = glam::Vec3::new(0.0, 0.0, 0.0);
        for (i, p) in volume.planes.iter().enumerate() {
            let n = glam::Vec3::new(p[0], p[1], p[2]);
            let d = p[3];
            let dot = n.dot(interior);
            // Strict inequality (not <=): centre is strictly interior, so any
            // outward-normal plane through a face must produce dot < d. Equality
            // would indicate the centre lies on the plane, which would be a
            // degenerate brush.
            assert!(
                dot <= d,
                "plane {i} ({p:?}) violates inside-when-dot<=d at interior point {interior:?}: dot={dot} d={d}"
            );
        }

        // Sanity: an exterior point must violate at least one plane.
        let exterior = glam::Vec3::new(100.0, 0.0, 0.0);
        let any_violated = volume.planes.iter().any(|p| {
            let n = glam::Vec3::new(p[0], p[1], p[2]);
            n.dot(exterior) > p[3]
        });
        assert!(
            any_violated,
            "exterior point {exterior:?} must violate at least one face plane (n·p > d)"
        );
    }

    // NOTE: the zero-plane rejection path in `resolve_fog_volume`
    // (`planes.is_empty()` while `have_any == true`) is a defensive guard that
    // is not reachable via honest GeoMap construction: the FaceIds present in
    // `face_verts` are a subset of those registered in `geo_map.face_planes`,
    // which is the same BTreeMap used to build `geo_planes`.  Therefore
    // `geo_planes.get(face_id)` cannot return `None` for any face that produced
    // vertices.  Testing this path would require either (a) manually constructing
    // an inconsistent GeoMap with a BrushId → FaceId mapping that refers to a
    // FaceId absent from `face_planes`, or (b) refactoring the guard into a
    // testable helper.  Both are out of scope for the structurally-unreachable
    // case.

    // -- fog_volume axis-aligned (ellipsoid) resolution --

    #[test]
    fn resolve_fog_ellipsoid_box_brush_emits_aabb_and_no_planes() {
        let map_text = r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
}
// entity 1
{
"classname" "fog_volume"
"falloff" "3.0"
"density" "0.25"
"glow" "0.7"
{
( 0 0 -32 ) ( 1 0 -32 ) ( 0 1 -32 ) tex 0 0 0 1 1
( 0 0  32 ) ( 0 1  32 ) ( 1 0  32 ) tex 0 0 0 1 1
( -64 0 0 ) ( -64 1 0 ) ( -64 0 1 ) tex 0 0 0 1 1
(  64 0 0 ) (  64 0 1 ) (  64 1 0 ) tex 0 0 0 1 1
( 0 -64 0 ) ( 0 -64 1 ) ( 1 -64 0 ) tex 0 0 0 1 1
( 0  64 0 ) ( 1  64 0 ) ( 0  64 1 ) tex 0 0 0 1 1
}
}
"#;
        let (geo_map, brush_ids) = fog_volume_geo_map_from_str(map_text);
        let mut props = HashMap::new();
        props.insert("falloff".to_string(), "3.0".to_string());
        props.insert("density".to_string(), "0.25".to_string());
        props.insert("glow".to_string(), "0.7".to_string());
        let scale = MapFormat::IdTech2.units_to_meters();
        let v = resolve_fog_ellipsoid(&geo_map, &brush_ids, &props, scale, "fog_volume")
            .expect("box brush must resolve");

        assert!(
            v.is_ellipsoid,
            "axis-aligned fog_volume resolver must set is_ellipsoid"
        );
        assert!(
            v.planes.is_empty(),
            "axis-aligned fog_volume emits no planes; got {}",
            v.planes.len()
        );
        assert_eq!(v.edge_softness, 0.0);
        assert!((v.radial_falloff - 3.0).abs() < 1e-6);
        assert!((v.density - 0.25).abs() < 1e-6);
        assert!((v.glow - 0.7).abs() < 1e-6);
        for i in 0..3 {
            assert!(
                v.max[i] > v.min[i],
                "axis {i} must have positive extent: min={} max={}",
                v.min[i],
                v.max[i]
            );
        }
    }

    #[test]
    fn resolve_fog_ellipsoid_rejects_zero_extent_brush() {
        // A brush whose top and bottom Z faces sit at the same plane has zero
        // thickness on the Z axis. shambler computes hull vertices from face
        // planes, so a zero-thickness slab typically produces no vertices at
        // all — exercising the "no usable vertices" defensive rejection that
        // sits in front of the explicit zero-extent check. Either path is an
        // actionable rejection of a degenerate volume, and naming this test
        // for the zero-extent symptom keeps the acceptance criterion legible.
        let map_text = r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
}
// entity 1
{
"classname" "fog_volume"
{
( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) tex 0 0 0 1 1
( 0 0 0 ) ( 0 1 0 ) ( 1 0 0 ) tex 0 0 0 1 1
( -64 0 0 ) ( -64 1 0 ) ( -64 0 1 ) tex 0 0 0 1 1
(  64 0 0 ) (  64 0 1 ) (  64 1 0 ) tex 0 0 0 1 1
( 0 -64 0 ) ( 0 -64 1 ) ( 1 -64 0 ) tex 0 0 0 1 1
( 0  64 0 ) ( 1  64 0 ) ( 0  64 1 ) tex 0 0 0 1 1
}
}
"#;
        let (geo_map, brush_ids) = fog_volume_geo_map_from_str(map_text);
        let props = HashMap::new();
        let scale = MapFormat::IdTech2.units_to_meters();
        let err = resolve_fog_ellipsoid(&geo_map, &brush_ids, &props, scale, "fog_volume")
            .expect_err("zero-thickness brush must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("fog_volume")
                && (msg.contains("no usable vertices") || msg.contains("zero extent")),
            "error message must name fog_volume and the degenerate condition; got: {msg}"
        );
    }

    #[test]
    fn resolve_fog_ellipsoid_inv_half_ext_matches_aabb_half_extents() {
        // The acceptance criterion is `inv_half_ext[i] = 1 / ((max[i] - min[i]) * 0.5)`.
        // The resolver produces `min`/`max`; pack.rs derives `inv_half_ext` from
        // those. Compose both stages so this test locks the contract end-to-end.
        //
        // Quake brush spans (-64..64, -64..64, -32..32). After `quake_to_engine`
        // (x=-y, y=z, z=-x) and the IdTech2 scale, expected engine extents are:
        //   x: ±64 * 0.0254, y: ±32 * 0.0254, z: ±64 * 0.0254.
        let map_text = r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
}
// entity 1
{
"classname" "fog_volume"
{
( 0 0 -32 ) ( 1 0 -32 ) ( 0 1 -32 ) tex 0 0 0 1 1
( 0 0  32 ) ( 0 1  32 ) ( 1 0  32 ) tex 0 0 0 1 1
( -64 0 0 ) ( -64 1 0 ) ( -64 0 1 ) tex 0 0 0 1 1
(  64 0 0 ) (  64 0 1 ) (  64 1 0 ) tex 0 0 0 1 1
( 0 -64 0 ) ( 0 -64 1 ) ( 1 -64 0 ) tex 0 0 0 1 1
( 0  64 0 ) ( 1  64 0 ) ( 0  64 1 ) tex 0 0 0 1 1
}
}
"#;
        let (geo_map, brush_ids) = fog_volume_geo_map_from_str(map_text);
        let props = HashMap::new();
        let scale = MapFormat::IdTech2.units_to_meters();
        let v = resolve_fog_ellipsoid(&geo_map, &brush_ids, &props, scale, "fog_volume")
            .expect("box brush must resolve");

        let s = scale as f32;
        let expected_min = [-64.0 * s, -32.0 * s, -64.0 * s];
        let expected_max = [64.0 * s, 32.0 * s, 64.0 * s];
        let eps = 1e-5;
        for i in 0..3 {
            assert!(
                (v.min[i] - expected_min[i]).abs() < eps,
                "min[{i}] = {} expected {}",
                v.min[i],
                expected_min[i]
            );
            assert!(
                (v.max[i] - expected_max[i]).abs() < eps,
                "max[{i}] = {} expected {}",
                v.max[i],
                expected_max[i]
            );
        }

        // Compose with pack.rs to confirm `inv_half_ext` lands at
        // `1 / ((max - min) * 0.5)`. This locks the end-to-end contract that
        // the ellipsoid shader path depends on.
        let section = crate::pack::encode_fog_volumes(std::slice::from_ref(&v), 1, -9.81);
        assert_eq!(section.volumes.len(), 1);
        let rec = &section.volumes[0];
        for i in 0..3 {
            let expected = 1.0 / ((v.max[i] - v.min[i]) * 0.5);
            assert!(
                (rec.inv_half_ext[i] - expected).abs() < eps,
                "inv_half_ext[{i}] = {} expected {}",
                rec.inv_half_ext[i],
                expected
            );
        }
    }

    #[test]
    fn resolve_fog_ellipsoid_uses_fgd_default_falloff_when_not_set() {
        let map_text = r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
}
// entity 1
{
"classname" "fog_volume"
{
( 0 0 -32 ) ( 1 0 -32 ) ( 0 1 -32 ) tex 0 0 0 1 1
( 0 0  32 ) ( 0 1  32 ) ( 1 0  32 ) tex 0 0 0 1 1
( -64 0 0 ) ( -64 1 0 ) ( -64 0 1 ) tex 0 0 0 1 1
(  64 0 0 ) (  64 0 1 ) (  64 1 0 ) tex 0 0 0 1 1
( 0 -64 0 ) ( 0 -64 1 ) ( 1 -64 0 ) tex 0 0 0 1 1
( 0  64 0 ) ( 1  64 0 ) ( 0  64 1 ) tex 0 0 0 1 1
}
}
"#;
        let (geo_map, brush_ids) = fog_volume_geo_map_from_str(map_text);
        let props = HashMap::new();
        let scale = MapFormat::IdTech2.units_to_meters();
        let v = resolve_fog_ellipsoid(&geo_map, &brush_ids, &props, scale, "fog_volume")
            .expect("box brush must resolve");
        assert!(
            (v.radial_falloff - 2.0).abs() < 1e-6,
            "default falloff should be 2.0, got {}",
            v.radial_falloff
        );
    }

    // -- fog_volume geometry detection (axis-aligned → ellipsoid path) --

    #[test]
    fn axis_aligned_box_brush_detected_as_axis_aligned() {
        let map_text = r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
}
// entity 1
{
"classname" "fog_volume"
{
( 0 0 -32 ) ( 1 0 -32 ) ( 0 1 -32 ) tex 0 0 0 1 1
( 0 0  32 ) ( 0 1  32 ) ( 1 0  32 ) tex 0 0 0 1 1
( -64 0 0 ) ( -64 1 0 ) ( -64 0 1 ) tex 0 0 0 1 1
(  64 0 0 ) (  64 0 1 ) (  64 1 0 ) tex 0 0 0 1 1
( 0 -64 0 ) ( 0 -64 1 ) ( 1 -64 0 ) tex 0 0 0 1 1
( 0  64 0 ) ( 1  64 0 ) ( 0  64 1 ) tex 0 0 0 1 1
}
}
"#;
        let (geo_map, brush_ids) = fog_volume_geo_map_from_str(map_text);
        assert!(
            is_axis_aligned_brush_set(&geo_map, &brush_ids),
            "an axis-aligned 6-face box must be detected as axis-aligned"
        );
    }

    #[test]
    fn slanted_face_brush_not_detected_as_axis_aligned() {
        // Cube with one face replaced by a 45° wedge plane (normal off-cardinal).
        let map_text = r#"
// entity 0
{
"classname" "worldspawn"
"initialGravity" "-9.81"
}
// entity 1
{
"classname" "fog_volume"
{
( 0 0 -32 ) ( 1 0 -32 ) ( 0 1 -32 ) tex 0 0 0 1 1
( 0 0  32 ) ( 0 1  32 ) ( 1 0  32 ) tex 0 0 0 1 1
( -64 0 0 ) ( -64 1 0 ) ( -64 0 1 ) tex 0 0 0 1 1
( 0 -64 0 ) ( 0 -64 1 ) ( 1 -64 0 ) tex 0 0 0 1 1
( 0  64 0 ) ( 1  64 0 ) ( 0  64 1 ) tex 0 0 0 1 1
( 64 0 0 ) ( 0 64 0 ) ( 64 0 1 ) tex 0 0 0 1 1
}
}
"#;
        let (geo_map, brush_ids) = fog_volume_geo_map_from_str(map_text);
        assert!(
            !is_axis_aligned_brush_set(&geo_map, &brush_ids),
            "a brush with a slanted face must not be detected as axis-aligned"
        );
    }

    #[test]
    fn parse_map_file_reads_initial_gravity_from_worldspawn() {
        let map_text = "\
// entity 0
{
\"classname\" \"worldspawn\"
\"initialGravity\" \"-15.0\"
{
( -16 -16 -16 ) ( -16 -16 16 ) ( -16 16 -16 ) tex 0 0 0 1 1
( -16 -16 -16 ) ( -16 16 -16 ) ( 16 -16 -16 ) tex 0 0 0 1 1
( -16 -16 -16 ) ( 16 -16 -16 ) ( -16 -16 16 ) tex 0 0 0 1 1
( 16 16 16 ) ( 16 -16 16 ) ( 16 16 -16 ) tex 0 0 0 1 1
( 16 16 16 ) ( 16 16 -16 ) ( -16 16 16 ) tex 0 0 0 1 1
( 16 16 16 ) ( -16 16 16 ) ( 16 -16 16 ) tex 0 0 0 1 1
}
}
";
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let tmp = std::env::temp_dir().join(format!("postretro_initial_gravity_{unique}.map"));
        std::fs::write(&tmp, map_text).unwrap();
        let map_data = parse_map_file(&tmp, MapFormat::IdTech2)
            .expect("inline gravity fixture should parse without error");
        let _ = std::fs::remove_file(&tmp);
        assert!(
            (map_data.initial_gravity - -15.0).abs() < 1e-5,
            "expected -15.0, got {}",
            map_data.initial_gravity,
        );
    }

    #[test]
    fn parse_map_file_defaults_missing_initial_gravity() {
        // A map can omit `initialGravity`; the parser seeds the runtime
        // register with the canonical Earth-gravity default.
        let map_text = "\
// entity 0
{
\"classname\" \"worldspawn\"
{
( -16 -16 -16 ) ( -16 -16 16 ) ( -16 16 -16 ) tex 0 0 0 1 1
( -16 -16 -16 ) ( -16 16 -16 ) ( 16 -16 -16 ) tex 0 0 0 1 1
( -16 -16 -16 ) ( 16 -16 -16 ) ( -16 -16 16 ) tex 0 0 0 1 1
( 16 16 16 ) ( 16 -16 16 ) ( 16 16 -16 ) tex 0 0 0 1 1
( 16 16 16 ) ( 16 16 -16 ) ( -16 16 16 ) tex 0 0 0 1 1
( 16 16 16 ) ( -16 16 16 ) ( 16 -16 16 ) tex 0 0 0 1 1
}
}
";
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let tmp =
            std::env::temp_dir().join(format!("postretro_default_initial_gravity_{unique}.map"));
        std::fs::write(&tmp, map_text).unwrap();
        let map_data = parse_map_file(&tmp, MapFormat::IdTech2)
            .expect("missing `initialGravity` should use the default");
        let _ = std::fs::remove_file(&tmp);
        assert!(
            (map_data.initial_gravity - DEFAULT_WORLD_GRAVITY_MPS2).abs() < f32::EPSILON,
            "missing `initialGravity` should default to {DEFAULT_WORLD_GRAVITY_MPS2}, got {}",
            map_data.initial_gravity,
        );
    }

    #[test]
    fn parse_map_file_rejects_malformed_or_non_finite_initial_gravity() {
        for value in ["not-a-number", "NaN", "inf"] {
            let map_data = format!(
                "\\
// entity 0
{{
\"classname\" \"worldspawn\"
\"initialGravity\" \"{value}\"
{{
( -16 -16 -16 ) ( -16 -16 16 ) ( -16 16 -16 ) tex 0 0 0 1 1
( -16 -16 -16 ) ( -16 16 -16 ) ( 16 -16 -16 ) tex 0 0 0 1 1
( -16 -16 -16 ) ( 16 -16 -16 ) ( -16 -16 16 ) tex 0 0 0 1 1
( 16 16 16 ) ( 16 -16 16 ) ( 16 16 -16 ) tex 0 0 0 1 1
( 16 16 16 ) ( 16 16 -16 ) ( -16 16 16 ) tex 0 0 0 1 1
( 16 16 16 ) ( -16 16 16 ) ( 16 -16 16 ) tex 0 0 0 1 1
}}
}}
"
            );
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let tmp = std::env::temp_dir().join(format!(
                "postretro_invalid_initial_gravity_{value}_{unique}.map"
            ));
            std::fs::write(&tmp, map_data).unwrap();
            let err = parse_map_file(&tmp, MapFormat::IdTech2)
                .expect_err("supplied invalid `initialGravity` should fail");
            let _ = std::fs::remove_file(&tmp);
            assert!(
                err.to_string().contains("initialGravity"),
                "error should reference `initialGravity`, got: {err}",
            );
        }
    }

    /// Write a worldspawn-only .map with the supplied extra KVP block (raw map
    /// syntax, e.g. `"_lightmap_density" "0.02"`) and parse it. Shared by the
    /// `_lightmap_density` round-trip tests below to avoid duplicating the
    /// brush fixture six ways.
    fn parse_worldspawn_with_kvp(extra_kvp: &str) -> MapData {
        // Inline `extra_kvp` only when non-empty; an empty line between KVPs
        // and the first brush trips shalrath's "no worldspawn entity found"
        // path on some platforms.
        let kvp_line = if extra_kvp.is_empty() {
            String::new()
        } else {
            format!("{extra_kvp}\n")
        };
        let map_text = format!(
            "\
// entity 0
{{
\"classname\" \"worldspawn\"
\"initialGravity\" \"-9.81\"
{kvp_line}{{
( -16 -16 -16 ) ( -16 -16 16 ) ( -16 16 -16 ) tex 0 0 0 1 1
( -16 -16 -16 ) ( -16 16 -16 ) ( 16 -16 -16 ) tex 0 0 0 1 1
( -16 -16 -16 ) ( 16 -16 -16 ) ( -16 -16 16 ) tex 0 0 0 1 1
( 16 16 16 ) ( 16 -16 16 ) ( 16 16 -16 ) tex 0 0 0 1 1
( 16 16 16 ) ( 16 16 -16 ) ( -16 16 16 ) tex 0 0 0 1 1
( 16 16 16 ) ( -16 16 16 ) ( 16 -16 16 ) tex 0 0 0 1 1
}}
}}
"
        );
        // Disambiguate the temp filename per-call: parallel test threads can
        // collide on `subsec_nanos` alone, leading to one test stomping
        // another's fixture mid-parse.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tid = std::thread::current().id();
        let tmp =
            std::env::temp_dir().join(format!("postretro_lightmap_density_{nanos}_{tid:?}.map"));
        std::fs::write(&tmp, map_text).unwrap();
        let result = parse_map_file(&tmp, MapFormat::IdTech2);
        let _ = std::fs::remove_file(&tmp);
        result.expect("fixture should parse without error")
    }

    #[test]
    fn parse_map_file_reads_lightmap_density_from_worldspawn() {
        let map_data = parse_worldspawn_with_kvp("\"_lightmap_density\" \"0.02\"");
        assert_eq!(
            map_data.lightmap_density,
            Some(0.02),
            "authored `_lightmap_density` must round-trip into MapData",
        );
    }

    #[test]
    fn parse_map_file_lightmap_density_absent_is_none() {
        let map_data = parse_worldspawn_with_kvp("");
        assert_eq!(
            map_data.lightmap_density, None,
            "absent `_lightmap_density` must surface as None so the compiler falls back to default",
        );
    }

    #[test]
    fn parse_map_file_lightmap_density_rejects_zero() {
        let map_data = parse_worldspawn_with_kvp("\"_lightmap_density\" \"0\"");
        assert_eq!(
            map_data.lightmap_density, None,
            "zero `_lightmap_density` must warn and fall back to default",
        );
    }

    #[test]
    fn parse_map_file_lightmap_density_rejects_negative() {
        let map_data = parse_worldspawn_with_kvp("\"_lightmap_density\" \"-0.04\"");
        assert_eq!(
            map_data.lightmap_density, None,
            "negative `_lightmap_density` must warn and fall back to default",
        );
    }

    #[test]
    fn parse_map_file_lightmap_density_rejects_nan() {
        let map_data = parse_worldspawn_with_kvp("\"_lightmap_density\" \"nan\"");
        assert_eq!(
            map_data.lightmap_density, None,
            "NaN `_lightmap_density` must warn and fall back to default",
        );
    }

    #[test]
    fn parse_map_file_lightmap_density_rejects_unparseable() {
        let map_data = parse_worldspawn_with_kvp("\"_lightmap_density\" \"not-a-number\"");
        assert_eq!(
            map_data.lightmap_density, None,
            "non-float `_lightmap_density` must warn and fall back to default",
        );
    }

    #[test]
    fn parse_map_file_reads_sh_density_fidelity_from_worldspawn() {
        let map_data = parse_worldspawn_with_kvp("\"_sh_density_fidelity\" \"0.5\"");
        assert_eq!(map_data.sh_density_fidelity, Some(0.5));
    }

    #[test]
    fn parse_map_file_sh_density_fidelity_absent_or_invalid_uses_default_path() {
        assert_eq!(
            parse_worldspawn_with_kvp("").sh_density_fidelity,
            None,
            "absence must defer to the compiler default"
        );
        for value in ["0", "-1", "nan", "not-a-number"] {
            let map_data =
                parse_worldspawn_with_kvp(&format!("\"_sh_density_fidelity\" \"{value}\""));
            assert_eq!(
                map_data.sh_density_fidelity, None,
                "{value} must be discarded"
            );
        }
    }

    #[test]
    fn parse_map_file_sh_coarsen_absent_keeps_default_enabled() {
        let map_data = parse_worldspawn_with_kvp("");
        assert!(!map_data.uniform_grid_optout);
    }

    #[test]
    fn parse_map_file_sh_coarsen_zero_selects_uniform_grid() {
        let map_data = parse_worldspawn_with_kvp("\"_sh_coarsen\" \"0\"");
        assert!(map_data.uniform_grid_optout);
    }

    #[test]
    fn parse_map_file_sh_coarsen_nonzero_keeps_default_enabled() {
        let map_data = parse_worldspawn_with_kvp("\"_sh_coarsen\" \"1\"");
        assert!(!map_data.uniform_grid_optout);
    }

    #[test]
    fn parse_map_file_reads_entity_shadow_thresholds_from_worldspawn() {
        let map_data = parse_worldspawn_with_kvp(
            "\"entity_shadow_min_intensity_ratio\" \"0.6\"\n\"entity_shadow_min_range\" \"6.5\"",
        );

        assert_eq!(map_data.entity_shadow_params.min_intensity_ratio, 0.6);
        assert_eq!(map_data.entity_shadow_params.min_range, 6.5);
    }

    #[test]
    fn parse_map_file_entity_shadow_thresholds_default_when_absent() {
        let map_data = parse_worldspawn_with_kvp("");

        assert_eq!(map_data.entity_shadow_params, EntityShadowParams::default());
    }

    #[test]
    fn empty_brush_set_not_detected_as_axis_aligned() {
        // No brushes → fall through to the plane-bounded path so the resolver
        // surfaces the empty-brush error instead of producing a silent ellipsoid.
        let map_text = r#"
{
"classname" "worldspawn"
"initialGravity" "-9.81"
}
"#;
        let shalrath_map: shambler::shalrath::repr::Map = map_text
            .trim()
            .parse()
            .expect("inline map text should parse");
        let geo_map = GeoMap::new(shalrath_map);
        assert!(!is_axis_aligned_brush_set(&geo_map, &[]));
    }

    // -- nav_* worldspawn KVP parsing --
    //
    // Each test exercises one of the five `nav_*` keys that feed `NavParams` via
    // `parse_positive_worldspawn_kvp`. The fixture helper `parse_worldspawn_with_kvp`
    // is reused from the `_lightmap_density` tests above.

    #[test]
    fn nav_agent_radius_kvp_overrides_agent_radius_field() {
        let map_data = parse_worldspawn_with_kvp("\"nav_agent_radius\" \"0.6\"");
        let eps = 1e-5_f32;
        assert!(
            (map_data.nav_params.agent_radius - 0.6).abs() < eps,
            "nav_agent_radius 0.6 must override NavParams::agent_radius, got {}",
            map_data.nav_params.agent_radius,
        );
    }

    #[test]
    fn nav_agent_height_kvp_overrides_agent_height_field() {
        let map_data = parse_worldspawn_with_kvp("\"nav_agent_height\" \"0.6\"");
        let eps = 1e-5_f32;
        assert!(
            (map_data.nav_params.agent_height - 0.6).abs() < eps,
            "nav_agent_height 0.6 must override NavParams::agent_height, got {}",
            map_data.nav_params.agent_height,
        );
    }

    #[test]
    fn nav_step_height_kvp_overrides_step_height_field() {
        let map_data = parse_worldspawn_with_kvp("\"nav_step_height\" \"0.6\"");
        let eps = 1e-5_f32;
        assert!(
            (map_data.nav_params.step_height - 0.6).abs() < eps,
            "nav_step_height 0.6 must override NavParams::step_height, got {}",
            map_data.nav_params.step_height,
        );
    }

    #[test]
    fn nav_max_slope_kvp_overrides_max_slope_deg_field() {
        let map_data = parse_worldspawn_with_kvp("\"nav_max_slope\" \"0.6\"");
        let eps = 1e-5_f32;
        assert!(
            (map_data.nav_params.max_slope_deg - 0.6).abs() < eps,
            "nav_max_slope 0.6 must override NavParams::max_slope_deg, got {}",
            map_data.nav_params.max_slope_deg,
        );
    }

    #[test]
    fn nav_cell_size_kvp_overrides_cell_size_field() {
        let map_data = parse_worldspawn_with_kvp("\"nav_cell_size\" \"0.6\"");
        let eps = 1e-5_f32;
        assert!(
            (map_data.nav_params.cell_size - 0.6).abs() < eps,
            "nav_cell_size 0.6 must override NavParams::cell_size, got {}",
            map_data.nav_params.cell_size,
        );
    }

    #[test]
    fn nav_params_absent_keys_fall_back_to_defaults() {
        // No nav_* KVPs authored — every field must equal NavParams::default().
        let map_data = parse_worldspawn_with_kvp("");
        let defaults = NavParams::default();
        let eps = 1e-5_f32;
        assert!(
            (map_data.nav_params.agent_radius - defaults.agent_radius).abs() < eps,
            "absent nav_agent_radius must fall back to default {}, got {}",
            defaults.agent_radius,
            map_data.nav_params.agent_radius,
        );
        assert!(
            (map_data.nav_params.agent_height - defaults.agent_height).abs() < eps,
            "absent nav_agent_height must fall back to default {}, got {}",
            defaults.agent_height,
            map_data.nav_params.agent_height,
        );
        assert!(
            (map_data.nav_params.step_height - defaults.step_height).abs() < eps,
            "absent nav_step_height must fall back to default {}, got {}",
            defaults.step_height,
            map_data.nav_params.step_height,
        );
        assert!(
            (map_data.nav_params.max_slope_deg - defaults.max_slope_deg).abs() < eps,
            "absent nav_max_slope must fall back to default {}, got {}",
            defaults.max_slope_deg,
            map_data.nav_params.max_slope_deg,
        );
        assert!(
            (map_data.nav_params.cell_size - defaults.cell_size).abs() < eps,
            "absent nav_cell_size must fall back to default {}, got {}",
            defaults.cell_size,
            map_data.nav_params.cell_size,
        );
    }

    #[test]
    fn nav_agent_radius_invalid_zero_falls_back_to_default() {
        let map_data = parse_worldspawn_with_kvp("\"nav_agent_radius\" \"0\"");
        let eps = 1e-5_f32;
        let default = NavParams::default().agent_radius;
        assert!(
            (map_data.nav_params.agent_radius - default).abs() < eps,
            "zero nav_agent_radius must warn and fall back to default {}, got {}",
            default,
            map_data.nav_params.agent_radius,
        );
    }

    #[test]
    fn nav_agent_height_invalid_negative_falls_back_to_default() {
        let map_data = parse_worldspawn_with_kvp("\"nav_agent_height\" \"-1.8\"");
        let eps = 1e-5_f32;
        let default = NavParams::default().agent_height;
        assert!(
            (map_data.nav_params.agent_height - default).abs() < eps,
            "negative nav_agent_height must warn and fall back to default {}, got {}",
            default,
            map_data.nav_params.agent_height,
        );
    }

    #[test]
    fn nav_step_height_invalid_nan_falls_back_to_default() {
        let map_data = parse_worldspawn_with_kvp("\"nav_step_height\" \"nan\"");
        let eps = 1e-5_f32;
        let default = NavParams::default().step_height;
        assert!(
            (map_data.nav_params.step_height - default).abs() < eps,
            "NaN nav_step_height must warn and fall back to default {}, got {}",
            default,
            map_data.nav_params.step_height,
        );
    }

    #[test]
    fn nav_max_slope_invalid_unparseable_falls_back_to_default() {
        let map_data = parse_worldspawn_with_kvp("\"nav_max_slope\" \"not-a-number\"");
        let eps = 1e-5_f32;
        let default = NavParams::default().max_slope_deg;
        assert!(
            (map_data.nav_params.max_slope_deg - default).abs() < eps,
            "non-float nav_max_slope must warn and fall back to default {}, got {}",
            default,
            map_data.nav_params.max_slope_deg,
        );
    }

    #[test]
    fn nav_cell_size_invalid_zero_falls_back_to_default() {
        let map_data = parse_worldspawn_with_kvp("\"nav_cell_size\" \"0\"");
        let eps = 1e-5_f32;
        let default = NavParams::default().cell_size;
        assert!(
            (map_data.nav_params.cell_size - default).abs() < eps,
            "zero nav_cell_size must warn and fall back to default {}, got {}",
            default,
            map_data.nav_params.cell_size,
        );
    }
}
