// glTF → engine skinned-model loader (CPU-only; no wgpu).
// See: context/lib/rendering_pipeline.md §9 · context/lib/build_pipeline.md §Baked texture mips · context/lib/entity_model.md §7

use std::collections::HashMap;
use std::path::Path;

use glam::{Mat4, Quat, Vec3};
use gltf::accessor::{DataType, Dimensions};
use postretro_level_format::gltf_resolve::resolve_material_base_color_path;
use postretro_level_format::octahedral;
use serde::Deserialize;
use thiserror::Error;

use super::mesh::{SkinnedMesh, SkinnedVertex};
use super::skeleton::{
    AnimationClip, Interp, Joint, JointTracks, RestLocal, Skeleton, SkeletonBuildError, Track,
};

/// One drawable run of the merged mesh: the triangles of a single primitive,
/// paired with the material that draws them.
///
/// The material is referenced by a **content-hash cache key** (`blake3` of the
/// base-color PNG, hex-encoded — the same recipe the level compiler bakes `.prm`
/// filenames with) rather than an owned `Material`, so the renderer resolves and
/// de-duplicates textures through the shared cache at upload time. `indices` is a
/// `start..end` range into the **merged** index buffer (what `draw_indexed`
/// consumes directly), so the renderer draws this submesh by slicing that range.
#[derive(Debug, Clone)]
pub struct Submesh {
    /// 64-char hex `blake3` of the base-color PNG (the baked `.prm` cache key),
    /// or the all-zero sentinel when the material has no resolvable PNG.
    pub material_key: String,
    /// `start..end` into the merged index buffer — the half-open range of indices
    /// this primitive contributed, as `draw_indexed` expects.
    pub indices: std::ops::Range<u32>,
}

/// A skeletal hit zone authored on a joint node's per-node `extras`. Read at
/// load time and carried parallel to [`Skeleton::joints`] (see
/// [`LoadedModel::joint_zones`]). A radius is carried only when authored as a
/// positive finite meter value; absent or invalid radii degrade to `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct JointZone {
    /// Author-supplied zone tag (e.g. "head", "torso").
    pub tag: String,
    /// Optional positive finite zone radius in meters. `None` when the joint
    /// node omits `hitZoneRadius` or authors an invalid radius; the consumer
    /// applies its own default.
    pub radius: Option<f32>,
}

/// A model loaded from glTF: one skinned mesh, its skeleton, its animation
/// clips, the per-primitive submeshes (material key + index range), and the
/// author-supplied entity tags read from the document's top-level `extras`.
#[derive(Debug, Clone, Default)]
pub struct LoadedModel {
    /// The skinned geometry. A model with multiple primitives merges them into
    /// one interleaved stream; `submeshes` carries the per-primitive split.
    pub mesh: SkinnedMesh,
    /// The joint hierarchy the mesh binds against, stored parent-before-child
    /// (topological) so the sampler composes world matrices in one forward
    /// sweep. Static/no-skin models use one identity joint so rigid vertices
    /// bound to joint 0 always have a palette entry.
    pub skeleton: Skeleton,
    /// Animation clips parsed from the glTF document, in authored order. All clips
    /// load; addressed by name or index.
    pub clips: Vec<AnimationClip>,
    /// One submesh per mesh primitive, in primitive order: the material cache
    /// key and the index range it occupies in the merged buffer. The renderer
    /// resolves each key against the shared material/texture cache and draws each
    /// range against its (possibly shared) material.
    pub submeshes: Vec<Submesh>,
    /// Tags parsed from the document's top-level `extras` (`{ "tags": [..] }`).
    /// They are returned but currently unused; map placement tags are separate.
    /// Empty when `extras` or `tags` is absent or malformed.
    pub tags: Vec<String>,
    /// Per-joint skeletal hit zones, parallel to [`Skeleton::joints`] (entry `i`
    /// describes joint `i`). `None` when the joint node carries no zone `extras`
    /// or the value is malformed — a garbled zone degrades to no zone, never a
    /// load error. Static/no-skin models carry one identity-joint entry, using
    /// the first authored static node zone when present.
    pub joint_zones: Vec<Option<JointZone>>,
}

/// Errors surfaced while loading required glTF model structure. Optional authored
/// data, such as malformed animation channels or metadata extras, may warn and
/// degrade instead; the loader never panics.
#[derive(Debug, Error)]
pub enum ModelLoadError {
    /// The glTF document or a referenced external buffer could not be read or parsed.
    #[error("glTF import failed for {path}: {source}")]
    Import {
        path: String,
        #[source]
        source: gltf::Error,
    },
    /// The document has no mesh to load (this path loads exactly one mesh).
    #[error("glTF has no mesh: {0}")]
    NoMesh(String),
    /// A primitive is missing a required vertex attribute (e.g. POSITION).
    #[error("glTF primitive missing required attribute '{attribute}': {path}")]
    MissingAttribute { path: String, attribute: String },
    /// A primitive uses an unsupported topology (only triangle lists load).
    #[error("glTF primitive uses unsupported topology {mode:?} (only triangles): {path}")]
    UnsupportedTopology { path: String, mode: String },
    /// A present accessor has a component type or dimensionality the loader cannot read.
    #[error("glTF accessor '{attribute}' has shape {actual}, expected {expected}: {path}")]
    InvalidAccessorShape {
        path: String,
        attribute: String,
        expected: String,
        actual: String,
    },
    /// A present primitive vertex stream does not match the POSITION vertex count.
    #[error(
        "glTF primitive attribute '{attribute}' has {actual} elements, expected {expected}: {path}"
    )]
    AttributeCountMismatch {
        path: String,
        attribute: String,
        expected: usize,
        actual: usize,
    },
    /// A primitive index points outside the primitive-local vertex range.
    #[error("glTF primitive index {index} is out of range for {vertex_count} vertices: {path}")]
    IndexOutOfRange {
        path: String,
        index: u32,
        vertex_count: usize,
    },
    /// A JOINTS_0 value does not map to a topologically-ordered skeleton joint.
    #[error("glTF primitive JOINTS_0 value {joint} has no matching skin joint: {path}")]
    InvalidJointIndex { path: String, joint: u16 },
    /// A primitive authored only one half of the skinning attribute pair.
    #[error(
        "glTF primitive has {present} without {missing}; JOINTS_0 and WEIGHTS_0 must be authored together: {path}"
    )]
    SkinningAttributePairMismatch {
        path: String,
        present: String,
        missing: String,
    },
    /// The skin references more joints than the `u8` joint index can address.
    #[error("glTF skin has {count} joints, exceeding the {max}-joint ceiling: {path}")]
    TooManyJoints {
        path: String,
        count: usize,
        max: usize,
    },
    /// The skin's inverse bind matrix accessor is present but not parallel to its joints.
    #[error(
        "glTF skin inverseBindMatrices has {actual} matrices, expected {expected} joints: {path}"
    )]
    InverseBindCountMismatch {
        path: String,
        expected: usize,
        actual: usize,
    },
    /// The skin's joint hierarchy cannot satisfy the sampler's parent-before-child contract.
    #[error("glTF skin has invalid skeleton hierarchy: {reason}: {path}")]
    InvalidSkeleton { path: String, reason: String },
}

/// Quantize a normalized UV component-pair (clamped to `[0, 1]`) to `u16 x 2`.
/// Mirrors the `WorldVertex` lightmap-UV convention (`* 65535 + 0.5`), so the
/// skinned and world vertex streams share one UV encoding.
fn quantize_uv(uv: [f32; 2]) -> [u16; 2] {
    [
        (uv[0].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
        (uv[1].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
    ]
}

/// Pack a tangent `[x, y, z, w]` (w = bitangent sign, ±1) into the
/// `tangent_packed` scheme `WorldVertex`/`Vertex` use: octahedral `u16` u in
/// `[0]`; 15-bit octahedral v in bits 0..14 of `[1]` with the bitangent sign in
/// bit 15. Mirrors `postretro_level_format::geometry::Vertex::new` exactly so the
/// vertex shader's tangent decode is identical for world and skinned meshes.
fn pack_tangent(tangent: [f32; 4]) -> [u16; 2] {
    let oct = octahedral::encode(tangent[0], tangent[1], tangent[2]);
    let v_15bit = (oct[1] as u32 * 32767 / 65535) as u16;
    let sign_bit: u16 = if tangent[3] >= 0.0 { 0x8000 } else { 0 };
    [oct[0], v_15bit | sign_bit]
}

fn validate_attribute_count(
    path_str: &str,
    attribute: &str,
    expected: usize,
    actual: usize,
) -> Result<(), ModelLoadError> {
    if actual == expected {
        return Ok(());
    }
    Err(ModelLoadError::AttributeCountMismatch {
        path: path_str.to_string(),
        attribute: attribute.to_string(),
        expected,
        actual,
    })
}

fn validate_accessor_shape(
    accessor: &gltf::Accessor,
    path_str: &str,
    attribute: &str,
    expected: &[(DataType, Dimensions)],
) -> Result<(), ModelLoadError> {
    let actual = (accessor.data_type(), accessor.dimensions());
    if expected.contains(&actual) {
        return Ok(());
    }
    Err(ModelLoadError::InvalidAccessorShape {
        path: path_str.to_string(),
        attribute: attribute.to_string(),
        expected: format_accessor_shapes(expected),
        actual: format_accessor_shape(actual),
    })
}

fn format_accessor_shapes(shapes: &[(DataType, Dimensions)]) -> String {
    shapes
        .iter()
        .map(|&shape| format_accessor_shape(shape))
        .collect::<Vec<_>>()
        .join(" or ")
}

fn format_accessor_shape(shape: (DataType, Dimensions)) -> String {
    format!("{:?} {:?}", shape.0, shape.1)
}

fn validate_primitive_index(
    path_str: &str,
    index: u32,
    vertex_count: usize,
) -> Result<(), ModelLoadError> {
    if (index as usize) < vertex_count {
        return Ok(());
    }
    Err(ModelLoadError::IndexOutOfRange {
        path: path_str.to_string(),
        index,
        vertex_count,
    })
}

fn validate_skinning_attribute_pair(
    primitive: &gltf::Primitive,
    path_str: &str,
) -> Result<bool, ModelLoadError> {
    let has_joints = primitive.get(&gltf::mesh::Semantic::Joints(0)).is_some();
    let has_weights = primitive.get(&gltf::mesh::Semantic::Weights(0)).is_some();
    validate_skinning_attribute_presence(has_joints, has_weights, path_str)
}

fn validate_skinning_attribute_presence(
    has_joints: bool,
    has_weights: bool,
    path_str: &str,
) -> Result<bool, ModelLoadError> {
    match (has_joints, has_weights) {
        (true, true) => Ok(true),
        (false, false) => Ok(false),
        (true, false) => Err(ModelLoadError::SkinningAttributePairMismatch {
            path: path_str.to_string(),
            present: "JOINTS_0".to_string(),
            missing: "WEIGHTS_0".to_string(),
        }),
        (false, true) => Err(ModelLoadError::SkinningAttributePairMismatch {
            path: path_str.to_string(),
            present: "WEIGHTS_0".to_string(),
            missing: "JOINTS_0".to_string(),
        }),
    }
}

struct SelectedModel<'a> {
    mesh: gltf::Mesh<'a>,
    skin: Option<gltf::Skin<'a>>,
}

fn select_model<'a>(
    document: &'a gltf::Document,
    path_str: &str,
) -> Result<SelectedModel<'a>, ModelLoadError> {
    for node in document.nodes() {
        if let (Some(mesh), Some(skin)) = (node.mesh(), node.skin()) {
            return Ok(SelectedModel {
                mesh,
                skin: Some(skin),
            });
        }
    }

    let mesh = document
        .meshes()
        .next()
        .ok_or_else(|| ModelLoadError::NoMesh(path_str.to_string()))?;
    Ok(SelectedModel { mesh, skin: None })
}

/// The all-zero cache key, hex-encoded (64 chars). The shared `.prm` convention
/// treats it as the "no source PNG" sentinel and binds a silent placeholder.
///
/// World `TextureCacheKeys` stores zero entries for unresolved textures. Model
/// compilation stores no key; this runtime loader produces the same sentinel
/// when a material has no resolvable base-color PNG.
fn zero_material_key() -> String {
    "0".repeat(64)
}

/// Resolve a material's base-color texture to its baked `.prm` cache key by
/// content-hashing the source PNG.
///
/// Recipe (mirrors the level compiler's `filename_key_for` for a
/// diffuse-present texture, per `build_pipeline.md` §Baked texture mips):
/// `blake3(baseColor PNG bytes)`, hex-encoded — the same key the offline baker
/// uses for `<key>.prm`. Runtime model materials use that same address. We
/// reproduce the recipe inline because `filename_key_for` is private to the
/// level-compiler crate.
///
/// Degrades to the zero sentinel (never errors/panics) when the base-color image
/// has no URI, is embedded in a buffer view (`Source::View`), or its file is
/// missing/unreadable — an unresolvable material renders a placeholder rather
/// than failing the whole load.
fn content_hash_material_key(material: &gltf::Material, parent_dir: &Path) -> String {
    let Some(png_path) = resolve_material_base_color_path(material, parent_dir) else {
        return zero_material_key();
    };

    match std::fs::read(&png_path) {
        Ok(png_bytes) => {
            let key = *blake3::hash(&png_bytes).as_bytes();
            hex_encode(&key)
        }
        Err(_) => zero_material_key(),
    }
}

/// Lowercase-hex encode a 32-byte cache key into the shared 64-char `.prm` key
/// convention. `blake3::Hash::to_hex()` returns a fixed-size `HexString`, not an
/// owned `String`; this produces the owned `String` the key pipeline expects.
fn hex_encode(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The shape of the document's top-level `extras` this loader cares about.
/// Unknown keys are ignored (no `deny_unknown_fields`) so authors can stash
/// arbitrary metadata alongside `tags`; `tags` defaults to empty when absent.
#[derive(Debug, Deserialize)]
struct ModelExtras {
    #[serde(default)]
    tags: Vec<String>,
}

/// Read the entity tags off the document's top-level `extras`.
///
/// The `extras` feature surfaces the raw JSON as `&Option<Box<RawValue>>`. Absent
/// `extras` → no tags. Present `extras` deserializes into [`ModelExtras`]; any
/// deserialize failure (wrong shape, non-array `tags`, etc.) also yields no tags.
/// Tags are author metadata, not load-critical data — a garbled `extras` must not
/// fail the load, so every error arm collapses to an empty list.
fn read_model_tags(extras: &gltf::json::Extras) -> Vec<String> {
    let Some(raw) = extras.as_ref() else {
        return Vec::new();
    };
    match serde_json::from_str::<ModelExtras>(raw.get()) {
        Ok(parsed) => parsed.tags,
        Err(_) => Vec::new(),
    }
}

/// The shape of a joint node's per-node `extras` this loader cares about.
/// Unknown keys are ignored so authors can stash arbitrary metadata; the zone
/// is meaningful only when `hitZone` is present (see [`read_joint_zone`]).
#[derive(Debug, Deserialize)]
struct JointZoneExtras {
    #[serde(rename = "hitZone")]
    hit_zone: Option<String>,
    /// Radius in meters. Invalid optional values degrade to no authored radius.
    #[serde(rename = "hitZoneRadius")]
    hit_zone_radius: Option<serde_json::Value>,
}

/// Read a single joint node's hit zone off its per-node `extras`
/// (`gltf::Node::extras()` — NOT the document-level extras).
///
/// Absent `extras`, a deserialize failure (wrong shape), or a missing `hitZone`
/// tag all yield `None` — a zone is author metadata, not load-critical data, so
/// a garbled value degrades to no zone for that joint rather than failing the
/// load. The radius is carried only when positive and finite; otherwise the
/// zone keeps its tag and degrades to no authored radius.
fn read_joint_zone(extras: &gltf::json::Extras) -> Option<JointZone> {
    let raw = extras.as_ref()?;
    let parsed = serde_json::from_str::<JointZoneExtras>(raw.get()).ok()?;
    let tag = parsed.hit_zone?;
    Some(JointZone {
        tag,
        radius: valid_hit_zone_radius(parsed.hit_zone_radius.as_ref()),
    })
}

fn valid_hit_zone_radius(value: Option<&serde_json::Value>) -> Option<f32> {
    let radius = value?.as_f64()? as f32;
    (radius.is_finite() && radius > 0.0).then_some(radius)
}

fn identity_skeleton() -> Skeleton {
    Skeleton {
        joints: vec![Joint {
            parent: None,
            inverse_bind: Mat4::IDENTITY.to_cols_array_2d(),
            rest_local: RestLocal::default(),
        }],
    }
}

fn static_identity_joint_zones(
    document: &gltf::Document,
    mesh_index: usize,
) -> Vec<Option<JointZone>> {
    vec![identity_joint_zone(
        document
            .nodes()
            .filter(|node| {
                node.mesh()
                    .map(|mesh| mesh.index() == mesh_index)
                    .unwrap_or(false)
            })
            .map(|node| read_joint_zone(node.extras())),
    )]
}

fn identity_joint_zone(zones: impl IntoIterator<Item = Option<JointZone>>) -> Option<JointZone> {
    zones.into_iter().flatten().next()
}

fn skeleton_build_error(path_str: &str, error: SkeletonBuildError) -> ModelLoadError {
    ModelLoadError::InvalidSkeleton {
        path: path_str.to_string(),
        reason: error.to_string(),
    }
}

/// Load a skinned model from a glTF file at `path`.
///
/// Parses one mesh (all primitives merged into a single interleaved stream), its
/// skeleton (joints stored parent-before-child with inverse-bind matrices), and
/// its animation clips. Material textures resolve to their baked `.prm` cache key
/// by content-hashing the source PNG at runtime. Returns an error (never panics)
/// on malformed or unsupported required model input. Malformed optional authored
/// animation channels warn and are skipped, so the joint holds its rest pose.
pub fn load_model(path: &Path) -> Result<LoadedModel, ModelLoadError> {
    let path_str = path.display().to_string();
    // Open the document and resolve only geometry/skin/animation buffers — no
    // image data. `gltf::import` would decode every referenced PNG (returning an
    // `_images` we then discard); `Gltf::open` + `import_buffers` skips that
    // decode entirely. The `.glb` binary-blob arg is `None` (external-`.gltf`
    // only — embedded-image / `.glb` support is out of scope).
    let parent_dir = path.parent().unwrap_or_else(|| Path::new(""));
    let document = gltf::Gltf::open(path)
        .map_err(|source| ModelLoadError::Import {
            path: path_str.clone(),
            source,
        })?
        .document;
    let buffers = gltf::import_buffers(&document, Some(parent_dir), None).map_err(|source| {
        ModelLoadError::Import {
            path: path_str.clone(),
            source,
        }
    })?;

    // The `utils` accessor readers each take a buffer-data closure that indexes
    // the imported buffer blobs (the external `.bin` resolved by
    // `import_buffers`); each helper builds its own closure locally so the
    // closure and the borrowed glTF entity share one lifetime.

    // Load exactly one mesh: prefer the first node that binds both mesh and skin
    // so geometry and skeleton are selected as a pair. A document with no
    // skinned mesh node falls back to the first mesh as a static model.
    let SelectedModel { mesh, skin } = select_model(&document, &path_str)?;

    // --- Skeleton ---------------------------------------------------------
    // Use the selected skinned node's skin. A static/no-skin model still
    // receives one identity joint so rigid vertices bound to joint 0 have a
    // matching palette entry; any authored static node hit zone maps to that
    // identity joint.
    //
    // `skin_joint_to_topo` reindexes mesh `JOINTS_0` (skin-joint indices) into
    // topo order; `node_to_topo` reindexes animation channel targets (node
    // indices) into the same topo order.
    let (skeleton, joint_zones, skin_joint_to_topo, node_to_topo) = match skin.as_ref() {
        Some(skin) => build_skeleton(skin, &buffers, &path_str)?,
        None => (
            identity_skeleton(),
            static_identity_joint_zones(&document, mesh.index()),
            HashMap::new(),
            HashMap::new(),
        ),
    };

    // --- Mesh -------------------------------------------------------------
    let (skinned_mesh, submeshes) =
        load_mesh(&mesh, &buffers, &skin_joint_to_topo, parent_dir, &path_str)?;

    // --- Animation clips --------------------------------------------------
    let clips = document
        .animations()
        .map(|anim| load_clip(&anim, &buffers, &node_to_topo, skeleton.joints.len()))
        .collect();

    // --- Entity tags (top-level extras) -----------------------------------
    // Author metadata; a missing/garbled `extras` yields no tags, not an error.
    // Top-level `extras` lives on the underlying `json::Root`, surfaced as a
    // `serde_json` `RawValue` by the `extras` feature.
    let tags = read_model_tags(&document.as_json().extras);

    Ok(LoadedModel {
        mesh: skinned_mesh,
        skeleton,
        clips,
        submeshes,
        tags,
        joint_zones,
    })
}

/// Build the topologically-sorted skeleton and the skin-joint-index → topo-index
/// remap. glTF `JOINTS_0` attributes and animation targets reference joints by
/// their **skin-joint index** (position in `skin.joints()`); the renderer wants
/// joints parent-before-child, so we re-order and carry the remap.
type SkeletonMaps = (
    Skeleton,
    Vec<Option<JointZone>>,
    HashMap<usize, usize>,
    HashMap<usize, usize>,
);

fn build_skeleton(
    skin: &gltf::Skin,
    buffers: &[gltf::buffer::Data],
    path_str: &str,
) -> Result<SkeletonMaps, ModelLoadError> {
    let buffer_data = |buffer: gltf::Buffer| buffers.get(buffer.index()).map(|d| &d.0[..]);

    // skin-joint-index → glTF node index, and node index → skin-joint-index.
    let joint_nodes: Vec<usize> = skin.joints().map(|n| n.index()).collect();
    if joint_nodes.len() > super::mesh::MAX_JOINTS {
        return Err(ModelLoadError::TooManyJoints {
            path: path_str.to_string(),
            count: joint_nodes.len(),
            max: super::mesh::MAX_JOINTS,
        });
    }
    let node_to_skin_joint: HashMap<usize, usize> = joint_nodes
        .iter()
        .enumerate()
        .map(|(skin_idx, &node)| (node, skin_idx))
        .collect();

    // Rest-pose local TRS per skin joint, captured from the glTF node's default
    // transform (decomposed to TRS so a matrix-form node still yields TRS). The
    // sampler holds these for any channel the clip omits (the shipped clip has
    // no scale channels). Indexed by skin-joint index.
    let rest_locals: Vec<RestLocal> = skin
        .joints()
        .map(|node| {
            let (t, r, s) = node.transform().decomposed();
            RestLocal {
                translation: Vec3::from(t),
                rotation: Quat::from_array(r),
                scale: Vec3::from(s),
            }
        })
        .collect();

    // Hit zone per skin joint, read from each joint NODE's per-node `extras`
    // (NOT the document-level extras). Indexed by skin-joint index, exactly like
    // `rest_locals`, so the same `topo_order` remap below realigns it with the
    // final topo-ordered joints. A missing/malformed value is `None` (no zone).
    let rest_zones: Vec<Option<JointZone>> = skin
        .joints()
        .map(|node| read_joint_zone(node.extras()))
        .collect();

    // Parent map among joint nodes: walk every joint node's children; any child
    // that is itself a joint records this node as its parent. Joints with no
    // joint-parent are roots.
    let mut node_parent: HashMap<usize, usize> = HashMap::new();
    for node in skin.joints() {
        for child in node.children() {
            if node_to_skin_joint.contains_key(&child.index()) {
                node_parent.insert(child.index(), node.index());
            }
        }
    }

    // Inverse-bind matrices, one per skin joint (column-major, glam order). The
    // glTF default (absent accessor) is identity per joint.
    let inverse_binds: Vec<[[f32; 4]; 4]> = match skin.inverse_bind_matrices() {
        Some(accessor) => {
            validate_accessor_shape(
                &accessor,
                path_str,
                "inverseBindMatrices",
                &[(DataType::F32, Dimensions::Mat4)],
            )?;
            if accessor.count() != joint_nodes.len() {
                return Err(ModelLoadError::InverseBindCountMismatch {
                    path: path_str.to_string(),
                    expected: joint_nodes.len(),
                    actual: accessor.count(),
                });
            }
            skin.reader(buffer_data)
                .read_inverse_bind_matrices()
                .ok_or_else(|| ModelLoadError::MissingAttribute {
                    path: path_str.to_string(),
                    attribute: "inverseBindMatrices".to_string(),
                })?
                .collect()
        }
        None => vec![Mat4::IDENTITY.to_cols_array_2d(); joint_nodes.len()],
    };

    // Topological order: a child must follow its parent. Iteratively emit any
    // not-yet-emitted joint whose parent is already emitted (or is a root). The
    // glTF spec already lists joints with parents before children in practice,
    // but we don't rely on that — this is robust to any ordering and to forests.
    let mut topo_order: Vec<usize> = Vec::with_capacity(joint_nodes.len()); // skin-joint indices
    let mut emitted = vec![false; joint_nodes.len()];
    loop {
        let mut progressed = false;
        for (skin_idx, &node) in joint_nodes.iter().enumerate() {
            if emitted[skin_idx] {
                continue;
            }
            let parent_ready = match node_parent.get(&node) {
                None => true,
                Some(parent_node) => node_to_skin_joint
                    .get(parent_node)
                    .map(|&p| emitted[p])
                    .unwrap_or(true),
            };
            if parent_ready {
                emitted[skin_idx] = true;
                topo_order.push(skin_idx);
                progressed = true;
            }
        }
        if topo_order.len() == joint_nodes.len() {
            break;
        }
        if !progressed {
            return Err(ModelLoadError::InvalidSkeleton {
                path: path_str.to_string(),
                reason: "joint hierarchy contains a cycle or unresolved joint parent".to_string(),
            });
        }
    }

    // skin-joint-index → topo-index (the order joints land in the skeleton).
    let skin_joint_to_topo: HashMap<usize, usize> = topo_order
        .iter()
        .enumerate()
        .map(|(topo_idx, &skin_idx)| (skin_idx, topo_idx))
        .collect();

    // node-index → topo-index, for remapping animation channel targets (which
    // address joints by node, not by skin-joint index).
    let node_to_topo: HashMap<usize, usize> = joint_nodes
        .iter()
        .enumerate()
        .filter_map(|(skin_idx, &node)| skin_joint_to_topo.get(&skin_idx).map(|&t| (node, t)))
        .collect();

    // Emit joints in topo order; parent links re-expressed in topo indices.
    let joints: Vec<Joint> = topo_order
        .iter()
        .map(|&skin_idx| {
            let node = joint_nodes[skin_idx];
            let parent = node_parent
                .get(&node)
                .and_then(|parent_node| node_to_skin_joint.get(parent_node))
                .map(|&parent_skin_idx| skin_joint_to_topo[&parent_skin_idx]);
            Joint {
                parent,
                inverse_bind: inverse_binds
                    .get(skin_idx)
                    .copied()
                    .unwrap_or_else(|| Mat4::IDENTITY.to_cols_array_2d()),
                rest_local: rest_locals.get(skin_idx).copied().unwrap_or_default(),
            }
        })
        .collect();

    // Reindex the zone table through the SAME remap `joints` use, so entry `i`
    // describes topo-joint `i`. A skin joint with no zone stays `None`.
    let joint_zones: Vec<Option<JointZone>> = topo_order
        .iter()
        .map(|&skin_idx| rest_zones.get(skin_idx).cloned().flatten())
        .collect();

    let skeleton = Skeleton::new(joints).map_err(|error| skeleton_build_error(path_str, error))?;

    Ok((skeleton, joint_zones, skin_joint_to_topo, node_to_topo))
}

/// Load every primitive of `mesh` into one merged interleaved stream, remapping
/// skin-joint indices to topo order. Returns the mesh and one [`Submesh`] per
/// primitive (in primitive order): each carries its material content-hash key
/// and the `start..end` range it occupies in the merged index buffer — the
/// per-primitive split the merged stream otherwise loses.
fn load_mesh(
    mesh: &gltf::Mesh,
    buffers: &[gltf::buffer::Data],
    skin_joint_to_topo: &HashMap<usize, usize>,
    parent_dir: &Path,
    path_str: &str,
) -> Result<(SkinnedMesh, Vec<Submesh>), ModelLoadError> {
    let buffer_data = |buffer: gltf::Buffer| buffers.get(buffer.index()).map(|d| &d.0[..]);
    let mut out = SkinnedMesh::default();
    let mut submeshes: Vec<Submesh> = Vec::new();

    for primitive in mesh.primitives() {
        if primitive.mode() != gltf::mesh::Mode::Triangles {
            return Err(ModelLoadError::UnsupportedTopology {
                path: path_str.to_string(),
                mode: format!("{:?}", primitive.mode()),
            });
        }

        let reader = primitive.reader(buffer_data);
        let has_skinning_attributes = validate_skinning_attribute_pair(&primitive, path_str)?;

        let position_accessor =
            primitive
                .get(&gltf::mesh::Semantic::Positions)
                .ok_or_else(|| ModelLoadError::MissingAttribute {
                    path: path_str.to_string(),
                    attribute: "POSITION".to_string(),
                })?;
        validate_accessor_shape(
            &position_accessor,
            path_str,
            "POSITION",
            &[(DataType::F32, Dimensions::Vec3)],
        )?;
        let positions: Vec<[f32; 3]> = reader
            .read_positions()
            .ok_or_else(|| ModelLoadError::MissingAttribute {
                path: path_str.to_string(),
                attribute: "POSITION".to_string(),
            })?
            .collect();
        let vertex_count = positions.len();

        // Optional attributes default to neutral values when a primitive omits
        // them (a rigid mesh without skinning, or an untextured primitive).
        let uvs: Vec<[u16; 2]> = match primitive.get(&gltf::mesh::Semantic::TexCoords(0)) {
            Some(accessor) => {
                validate_accessor_shape(
                    &accessor,
                    path_str,
                    "TEXCOORD_0",
                    &[
                        (DataType::U8, Dimensions::Vec2),
                        (DataType::U16, Dimensions::Vec2),
                        (DataType::F32, Dimensions::Vec2),
                    ],
                )?;
                let values: Vec<[u16; 2]> = reader
                    .read_tex_coords(0)
                    .ok_or_else(|| ModelLoadError::MissingAttribute {
                        path: path_str.to_string(),
                        attribute: "TEXCOORD_0".to_string(),
                    })?
                    .into_f32()
                    .map(quantize_uv)
                    .collect();
                validate_attribute_count(path_str, "TEXCOORD_0", vertex_count, values.len())?;
                values
            }
            None => vec![[0, 0]; vertex_count],
        };
        let normals: Vec<[u16; 2]> = match primitive.get(&gltf::mesh::Semantic::Normals) {
            Some(accessor) => {
                validate_accessor_shape(
                    &accessor,
                    path_str,
                    "NORMAL",
                    &[(DataType::F32, Dimensions::Vec3)],
                )?;
                let values: Vec<[u16; 2]> = reader
                    .read_normals()
                    .ok_or_else(|| ModelLoadError::MissingAttribute {
                        path: path_str.to_string(),
                        attribute: "NORMAL".to_string(),
                    })?
                    .map(|nn| octahedral::encode(nn[0], nn[1], nn[2]))
                    .collect();
                validate_attribute_count(path_str, "NORMAL", vertex_count, values.len())?;
                values
            }
            // Default to +Z (octahedral center) when normals are absent.
            None => vec![octahedral::encode(0.0, 0.0, 1.0); vertex_count],
        };
        let tangents: Vec<[u16; 2]> = match primitive.get(&gltf::mesh::Semantic::Tangents) {
            Some(accessor) => {
                validate_accessor_shape(
                    &accessor,
                    path_str,
                    "TANGENT",
                    &[(DataType::F32, Dimensions::Vec4)],
                )?;
                let values: Vec<[u16; 2]> = reader
                    .read_tangents()
                    .ok_or_else(|| ModelLoadError::MissingAttribute {
                        path: path_str.to_string(),
                        attribute: "TANGENT".to_string(),
                    })?
                    .map(pack_tangent)
                    .collect();
                validate_attribute_count(path_str, "TANGENT", vertex_count, values.len())?;
                values
            }
            // Default tangent: +X with positive bitangent sign.
            None => vec![pack_tangent([1.0, 0.0, 0.0, 1.0]); vertex_count],
        };
        // Joints reference skin-joint indices; remap to topo order. `into_u16`
        // normalizes the (u8|u16) storage; values exceed u8 only for skeletons
        // above the MAX_JOINTS ceiling, already rejected in build_skeleton.
        let joints: Vec<[u8; 4]> = if has_skinning_attributes {
            let accessor = primitive
                .get(&gltf::mesh::Semantic::Joints(0))
                .ok_or_else(|| ModelLoadError::MissingAttribute {
                    path: path_str.to_string(),
                    attribute: "JOINTS_0".to_string(),
                })?;
            validate_accessor_shape(
                &accessor,
                path_str,
                "JOINTS_0",
                &[
                    (DataType::U8, Dimensions::Vec4),
                    (DataType::U16, Dimensions::Vec4),
                ],
            )?;
            let raw: Vec<[u16; 4]> = reader
                .read_joints(0)
                .ok_or_else(|| ModelLoadError::MissingAttribute {
                    path: path_str.to_string(),
                    attribute: "JOINTS_0".to_string(),
                })?
                .into_u16()
                .collect();
            validate_attribute_count(path_str, "JOINTS_0", vertex_count, raw.len())?;
            raw.into_iter()
                .map(|quad| remap_joint_quad(quad, skin_joint_to_topo, path_str))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            vec![[0, 0, 0, 0]; vertex_count]
        };
        let weights: Vec<[u8; 4]> = if has_skinning_attributes {
            let accessor = primitive
                .get(&gltf::mesh::Semantic::Weights(0))
                .ok_or_else(|| ModelLoadError::MissingAttribute {
                    path: path_str.to_string(),
                    attribute: "WEIGHTS_0".to_string(),
                })?;
            validate_accessor_shape(
                &accessor,
                path_str,
                "WEIGHTS_0",
                &[
                    (DataType::U8, Dimensions::Vec4),
                    (DataType::U16, Dimensions::Vec4),
                    (DataType::F32, Dimensions::Vec4),
                ],
            )?;
            let values: Vec<[u8; 4]> = reader
                .read_weights(0)
                .ok_or_else(|| ModelLoadError::MissingAttribute {
                    path: path_str.to_string(),
                    attribute: "WEIGHTS_0".to_string(),
                })?
                .into_u8()
                .map(normalize_weights)
                .collect();
            validate_attribute_count(path_str, "WEIGHTS_0", vertex_count, values.len())?;
            values
        } else {
            vec![[255, 0, 0, 0]; vertex_count]
        };

        let base_vertex = out.vertices.len() as u32;
        for i in 0..vertex_count {
            out.vertices.push(SkinnedVertex {
                position: positions[i],
                base_uv: uvs[i],
                normal_oct: normals[i],
                tangent_packed: tangents[i],
                joints: joints[i],
                weights: weights[i],
            });
        }

        // Indices, offset into the merged stream. A primitive without an index
        // buffer is a sequential triangle list. `start..end` bracket this
        // primitive's run in the merged index buffer — exactly the range
        // `draw_indexed` consumes for this submesh.
        let start = out.indices.len() as u32;
        if let Some(accessor) = primitive.indices() {
            validate_accessor_shape(
                &accessor,
                path_str,
                "indices",
                &[
                    (DataType::U8, Dimensions::Scalar),
                    (DataType::U16, Dimensions::Scalar),
                    (DataType::U32, Dimensions::Scalar),
                ],
            )?;
        }
        match reader.read_indices() {
            Some(idx) => {
                for i in idx.into_u32() {
                    validate_primitive_index(path_str, i, vertex_count)?;
                    out.indices.push(base_vertex + i);
                }
            }
            None => {
                for i in 0..vertex_count as u32 {
                    out.indices.push(base_vertex + i);
                }
            }
        }
        let end = out.indices.len() as u32;

        submeshes.push(Submesh {
            material_key: content_hash_material_key(&primitive.material(), parent_dir),
            indices: start..end,
        });
    }

    // Tight local-space bound over the merged vertex positions. The current
    // per-light caster cull transforms this bound by the instance transform.
    out.compute_bounds();

    Ok((out, submeshes))
}

/// Remap one `[u16; 4]` joint quad (skin-joint indices) to topo-order `[u8; 4]`.
/// Every authored joint index must resolve through the skin's topo remap; an
/// unmapped index is malformed required mesh data, not a silent joint-0 bind.
fn remap_joint_quad(
    quad: [u16; 4],
    skin_joint_to_topo: &HashMap<usize, usize>,
    path_str: &str,
) -> Result<[u8; 4], ModelLoadError> {
    let mut out = [0u8; 4];
    for (i, &j) in quad.iter().enumerate() {
        out[i] = skin_joint_to_topo
            .get(&(j as usize))
            .copied()
            .ok_or_else(|| ModelLoadError::InvalidJointIndex {
                path: path_str.to_string(),
                joint: j,
            })?
            .min(u8::MAX as usize) as u8;
    }
    Ok(out)
}

/// Normalize four `u8` weights so they sum to 255 (a fully-weighted vertex). The
/// largest weight absorbs the rounding remainder so the sum is exact for any
/// non-degenerate input (non-zero sum). An all-zero quad (unweighted) becomes
/// full weight on slot 0.
fn normalize_weights(weights: [u8; 4]) -> [u8; 4] {
    let sum: u32 = weights.iter().map(|&w| w as u32).sum();
    if sum == 0 {
        return [255, 0, 0, 0];
    }
    let mut out = [0u8; 4];
    let mut running = 0u32;
    for i in 0..4 {
        out[i] = ((weights[i] as u32 * 255 + sum / 2) / sum) as u8;
        running += out[i] as u32;
    }
    // Push the rounding remainder onto the heaviest slot so the sum is 255.
    // The clamp is a defensive fallback; for valid inputs the correction fits in [0, 255].
    let heaviest = (0..4).max_by_key(|&i| weights[i]).unwrap();
    let corrected = out[heaviest] as i32 + (255 - running as i32);
    out[heaviest] = corrected.clamp(0, 255) as u8;
    debug_assert_eq!(
        out.iter().map(|&w| w as u32).sum::<u32>(),
        255,
        "normalize_weights: post-condition violated for input {weights:?}"
    );
    out
}

/// Resolve a channel's raw output elements + sampler interpolation into the
/// keyframe `values` and engine [`Interp`] mode the track stores. Returns `None`
/// when the channel must be skipped (its joint then holds its rest pose, the
/// same as an absent channel).
///
/// - LINEAR / STEP map directly to [`Interp::Linear`] / [`Interp::Step`]; the raw
///   outputs are the per-keyframe values (one element per input time). If that
///   one-to-one shape does not hold, the channel is malformed, so we warn and
///   skip it to preserve the parallel-length invariant `Track` requires.
/// - CUBICSPLINE stores three elements per keyframe — `[in-tangent, value,
///   out-tangent]` — so the value of keyframe `k` is `raw[3k + 1]`. We extract
///   those, discard the tangents, store the track as `Linear` (degrading cubic to
///   linear; tangent storage and hermite blending are not implemented), and warn.
///   If the triple shape does not hold (`raw.len() != 3 * key_count`) the channel
///   is malformed for cubic, so we warn and skip it — this also guards the
///   parallel-length invariant `Track` requires (otherwise 3N values would be
///   paired with N times).
fn resolve_keyframes<T: Copy>(
    raw: Vec<T>,
    key_count: usize,
    interpolation: gltf::animation::Interpolation,
    clip_name: &str,
    channel_kind: &str,
) -> Option<(Vec<T>, Interp)> {
    use gltf::animation::Interpolation;
    match interpolation {
        Interpolation::Linear => {
            if raw.len() != key_count {
                log::warn!(
                    "clip '{clip_name}' {channel_kind} channel: LINEAR output count {} \
                     does not match its {key_count} keyframes; skipping channel (joint holds rest pose)",
                    raw.len()
                );
                return None;
            }
            Some((raw, Interp::Linear))
        }
        Interpolation::Step => {
            if raw.len() != key_count {
                log::warn!(
                    "clip '{clip_name}' {channel_kind} channel: STEP output count {} \
                     does not match its {key_count} keyframes; skipping channel (joint holds rest pose)",
                    raw.len()
                );
                return None;
            }
            Some((raw, Interp::Step))
        }
        Interpolation::CubicSpline => {
            if raw.len() != key_count * 3 {
                log::warn!(
                    "clip '{clip_name}' {channel_kind} channel: CUBICSPLINE output count {} \
                     is not 3x its {key_count} keyframes; skipping channel (joint holds rest pose)",
                    raw.len()
                );
                return None;
            }
            log::warn!(
                "clip '{clip_name}' {channel_kind} channel: CUBICSPLINE not supported; \
                 extracting keyframe values and degrading to LINEAR (tangents discarded)"
            );
            // Each triple is [in-tangent, value, out-tangent]; keep the value.
            let values: Vec<T> = (0..key_count).map(|k| raw[3 * k + 1]).collect();
            Some((values, Interp::Linear))
        }
    }
}

fn validate_key_times(times: &[f32], clip_name: &str) -> bool {
    for (i, &time) in times.iter().enumerate() {
        if !time.is_finite() {
            log::warn!(
                "clip '{clip_name}' animation channel: input time at key {i} is not finite; \
                 skipping channel (joint holds rest pose)"
            );
            return false;
        }
    }
    for (i, pair) in times.windows(2).enumerate() {
        if pair[0] >= pair[1] {
            log::warn!(
                "clip '{clip_name}' animation channel: input times are not strictly \
                 ascending at keys {i} and {}; skipping channel (joint holds rest pose)",
                i + 1
            );
            return false;
        }
    }
    true
}

fn vec3_is_finite(v: Vec3) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.z.is_finite()
}

fn validate_vec3_keyframes(values: &[Vec3], clip_name: &str, channel_kind: &str) -> bool {
    for (i, &value) in values.iter().enumerate() {
        if !vec3_is_finite(value) {
            log::warn!(
                "clip '{clip_name}' {channel_kind} channel: output value at key {i} \
                 is not finite; skipping channel (joint holds rest pose)"
            );
            return false;
        }
    }
    true
}

fn quat_length_squared(q: Quat) -> f32 {
    q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w
}

fn validate_rotation_keyframes(values: &[Quat], clip_name: &str) -> bool {
    for (i, &value) in values.iter().enumerate() {
        let len_sq = quat_length_squared(value);
        if !len_sq.is_finite() {
            log::warn!(
                "clip '{clip_name}' rotation channel: output quaternion at key {i} \
                 is not finite; skipping channel (joint holds rest pose)"
            );
            return false;
        }
        if len_sq <= 0.0 {
            log::warn!(
                "clip '{clip_name}' rotation channel: output quaternion at key {i} \
                 has zero length; skipping channel (joint holds rest pose)"
            );
            return false;
        }
    }
    true
}

fn build_track<T>(
    times: Vec<f32>,
    values: Vec<T>,
    mode: Interp,
    clip_name: &str,
    channel_kind: &str,
) -> Option<Track<T>> {
    match Track::new(times, values, mode) {
        Ok(track) => Some(track),
        Err(error) => {
            log::warn!(
                "clip '{clip_name}' {channel_kind} channel: invalid keyframe track \
                 ({error:?}); skipping channel (joint holds rest pose)"
            );
            None
        }
    }
}

/// Load one animation clip into per-joint TRS tracks (in topo joint order).
/// Channels targeting non-joint nodes (or joints outside the skin) are skipped.
fn load_clip(
    anim: &gltf::Animation,
    buffers: &[gltf::buffer::Data],
    node_to_topo: &HashMap<usize, usize>,
    joint_count: usize,
) -> AnimationClip {
    let buffer_data = |buffer: gltf::Buffer| buffers.get(buffer.index()).map(|d| &d.0[..]);
    let name = anim.name().unwrap_or("").to_string();
    let mut joints = vec![JointTracks::default(); joint_count];
    let mut duration = 0.0f32;

    for channel in anim.channels() {
        let target_node = channel.target().node().index();
        // Animation channels target joints by node index; remap to topo order.
        let Some(&topo_idx) = node_to_topo.get(&target_node) else {
            continue;
        };
        if topo_idx >= joints.len() {
            continue;
        }

        // The sampler's interpolation algorithm drives how the runtime blends
        // keyframes: LINEAR/STEP map straight to [`Interp`], while CUBICSPLINE is
        // degraded to LINEAR by extracting each key's value element (tangents
        // discarded). True cubic evaluation is out of scope: tangent storage and
        // hermite blending are not implemented.
        let interpolation = channel.sampler().interpolation();

        let reader = channel.reader(buffer_data);
        let Some(inputs) = reader.read_inputs() else {
            continue;
        };
        let times: Vec<f32> = inputs.collect();
        if !validate_key_times(&times, &name) {
            continue;
        }
        if let Some(&last) = times.last() {
            duration = duration.max(last);
        }
        let Some(outputs) = reader.read_outputs() else {
            continue;
        };
        match outputs {
            gltf::animation::util::ReadOutputs::Translations(it) => {
                let raw: Vec<Vec3> = it.map(Vec3::from).collect();
                if let Some((values, mode)) =
                    resolve_keyframes(raw, times.len(), interpolation, &name, "translation")
                        .filter(|(values, _)| validate_vec3_keyframes(values, &name, "translation"))
                {
                    if let Some(track) = build_track(times, values, mode, &name, "translation") {
                        joints[topo_idx].translation = track;
                    }
                }
            }
            gltf::animation::util::ReadOutputs::Rotations(it) => {
                let raw: Vec<Quat> = it
                    .into_f32()
                    .map(|q| Quat::from_xyzw(q[0], q[1], q[2], q[3]))
                    .collect();
                if let Some((values, mode)) =
                    resolve_keyframes(raw, times.len(), interpolation, &name, "rotation")
                        .filter(|(values, _)| validate_rotation_keyframes(values, &name))
                {
                    if let Some(track) = build_track(times, values, mode, &name, "rotation") {
                        joints[topo_idx].rotation = track;
                    }
                }
            }
            gltf::animation::util::ReadOutputs::Scales(it) => {
                let raw: Vec<Vec3> = it.map(Vec3::from).collect();
                if let Some((values, mode)) =
                    resolve_keyframes(raw, times.len(), interpolation, &name, "scale")
                        .filter(|(values, _)| validate_vec3_keyframes(values, &name, "scale"))
                {
                    if let Some(track) = build_track(times, values, mode, &name, "scale") {
                        joints[topo_idx].scale = track;
                    }
                }
            }
            gltf::animation::util::ReadOutputs::MorphTargetWeights(_) => {
                // Morph targets are out of scope for the skinned slice.
            }
        }
    }

    AnimationClip {
        name,
        duration,
        joints,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // --- Pure mapping helpers (the seam: glTF values → engine encodings) ---

    #[test]
    fn quantize_uv_maps_unit_range_to_full_u16() {
        assert_eq!(quantize_uv([0.0, 0.0]), [0, 0]);
        assert_eq!(quantize_uv([1.0, 1.0]), [65535, 65535]);
        // Clamps out-of-range UVs rather than wrapping/overflowing.
        assert_eq!(quantize_uv([-0.5, 2.0]), [0, 65535]);
        // Round-trips a midpoint within quantization precision.
        let q = quantize_uv([0.5, 0.25]);
        assert!((q[0] as f32 / 65535.0 - 0.5).abs() < 1.0e-4);
        assert!((q[1] as f32 / 65535.0 - 0.25).abs() < 1.0e-4);
    }

    #[test]
    fn pack_tangent_matches_world_vertex_scheme() {
        // The packed tangent must decode identically to the world-vertex path,
        // so the shared vertex-shader tangent decode works for skinned meshes.
        // Mirror postretro_level_format::geometry::Vertex's encode + decode.
        let tangent = [1.0f32, 0.0, 0.0, 1.0];
        let packed = pack_tangent(tangent);

        // Bitangent sign lives in bit 15 of [1]; positive sign → bit set.
        assert_eq!(packed[1] & 0x8000, 0x8000, "positive bitangent sign bit");

        // Decode the 15-bit v back and confirm the octahedral vector is ~+X.
        let v_15bit = packed[1] & 0x7FFF;
        let v_16bit = (v_15bit as u32 * 65535 / 32767) as u16;
        let decoded = octahedral::decode([packed[0], v_16bit]);
        assert!(
            (decoded[0] - 1.0).abs() < 1.0e-3,
            "decoded ~+X, got {decoded:?}"
        );

        // Negative bitangent sign clears bit 15.
        let neg = pack_tangent([0.0, 1.0, 0.0, -1.0]);
        assert_eq!(neg[1] & 0x8000, 0, "negative bitangent sign clears bit 15");
    }

    #[test]
    fn normalize_weights_sums_to_255() {
        // A fully-weighted-but-imprecise quad is renormalized to sum exactly 255.
        for quad in [
            [127u8, 127, 0, 0],
            [85, 85, 85, 0],
            [200, 30, 20, 5],
            [1, 1, 1, 1],
        ] {
            let n = normalize_weights(quad);
            let sum: u32 = n.iter().map(|&w| w as u32).sum();
            assert_eq!(sum, 255, "weights {quad:?} normalized to {n:?} (sum {sum})");
        }
    }

    #[test]
    fn normalize_weights_unweighted_becomes_rigid_joint_zero() {
        // An all-zero weight quad is the degenerate/unweighted case: full weight
        // on slot 0 (matches SkinnedVertex::rigid).
        assert_eq!(normalize_weights([0, 0, 0, 0]), [255, 0, 0, 0]);
    }

    #[test]
    fn validate_attribute_count_accepts_only_position_sized_streams() {
        assert!(
            validate_attribute_count("model.gltf", "NORMAL", 3, 3).is_ok(),
            "matching optional stream length is accepted"
        );

        let err = validate_attribute_count("model.gltf", "TEXCOORD_0", 3, 2)
            .expect_err("short present stream is malformed");
        assert!(
            matches!(
                &err,
                ModelLoadError::AttributeCountMismatch {
                    path,
                    attribute,
                    expected: 3,
                    actual: 2,
                } if path == "model.gltf" && attribute == "TEXCOORD_0"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn validate_primitive_index_rejects_values_outside_vertex_range() {
        assert!(
            validate_primitive_index("model.gltf", 2, 3).is_ok(),
            "last in-range local index is accepted"
        );

        let err = validate_primitive_index("model.gltf", 3, 3)
            .expect_err("index equal to vertex_count is out of range");
        assert!(
            matches!(
                &err,
                ModelLoadError::IndexOutOfRange {
                    path,
                    index: 3,
                    vertex_count: 3,
                } if path == "model.gltf"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn validate_skinning_attribute_pair_rejects_half_authored_skinning() {
        assert_eq!(
            validate_skinning_attribute_presence(false, false, "model.gltf")
                .expect("absence of both attributes is rigid"),
            false,
            "absence of both JOINTS_0 and WEIGHTS_0 is the rigid path",
        );
        assert_eq!(
            validate_skinning_attribute_presence(true, true, "model.gltf")
                .expect("both attributes present is skinned"),
            true,
            "presence of both JOINTS_0 and WEIGHTS_0 is the skinned path",
        );

        let err = validate_skinning_attribute_presence(true, false, "model.gltf")
            .expect_err("JOINTS_0 without WEIGHTS_0 is malformed");
        assert!(
            matches!(
                &err,
                ModelLoadError::SkinningAttributePairMismatch {
                    path,
                    present,
                    missing,
                } if path == "model.gltf" && present == "JOINTS_0" && missing == "WEIGHTS_0"
            ),
            "got {err:?}",
        );
        let err = validate_skinning_attribute_presence(false, true, "model.gltf")
            .expect_err("WEIGHTS_0 without JOINTS_0 is malformed");
        assert!(
            matches!(
                &err,
                ModelLoadError::SkinningAttributePairMismatch {
                    path,
                    present,
                    missing,
                } if path == "model.gltf" && present == "WEIGHTS_0" && missing == "JOINTS_0"
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn remap_joint_quad_reindexes_all_mapped_joints() {
        let mut map = HashMap::new();
        map.insert(5usize, 2usize); // skin-joint 5 → topo 2
        map.insert(9usize, 0usize); // skin-joint 9 → topo 0
        let out = remap_joint_quad([5, 9, 5, 5], &map, "model.gltf")
            .expect("all authored joint indices have topo remaps");
        assert_eq!(out, [2, 0, 2, 2]);
    }

    #[test]
    fn remap_joint_quad_rejects_unmapped_joint_indices() {
        // Regression: invalid JOINTS_0 values used to silently remap to joint 0,
        // corrupting skinning instead of reporting malformed required mesh data.
        let mut map = HashMap::new();
        map.insert(5usize, 2usize);

        let err = remap_joint_quad([5, 7, 5, 5], &map, "model.gltf")
            .expect_err("unmapped skin-joint index is malformed");
        assert!(
            matches!(
                &err,
                ModelLoadError::InvalidJointIndex {
                    path,
                    joint: 7,
                } if path == "model.gltf"
            ),
            "got {err:?}"
        );
    }

    // --- Interpolation mode mapping + CUBICSPLINE value extraction ----------

    use gltf::animation::Interpolation;

    #[test]
    fn resolve_keyframes_maps_linear_and_step_directly() {
        let raw = vec![Vec3::X, Vec3::Y, Vec3::Z];
        let (vals, mode) =
            resolve_keyframes(raw.clone(), 3, Interpolation::Linear, "clip", "translation")
                .expect("linear keeps all values");
        assert_eq!(vals, raw, "LINEAR passes raw values through");
        assert_eq!(mode, Interp::Linear);

        let (vals, mode) =
            resolve_keyframes(raw.clone(), 3, Interpolation::Step, "clip", "translation")
                .expect("step keeps all values");
        assert_eq!(vals, raw, "STEP passes raw values through");
        assert_eq!(mode, Interp::Step);
    }

    #[test]
    fn resolve_keyframes_linear_and_step_wrong_shape_skips_channel() {
        // Regression: malformed LINEAR/STEP output counts could create tracks
        // where values were shorter/longer than times, then panic during sampling.
        let short_linear = vec![Vec3::ZERO; 2];
        let result = resolve_keyframes(
            short_linear,
            3,
            Interpolation::Linear,
            "clip",
            "translation",
        );
        assert!(
            result.is_none(),
            "malformed linear (outputs != keys) skips the channel"
        );

        let long_step = vec![Vec3::ZERO; 4];
        let result = resolve_keyframes(long_step, 3, Interpolation::Step, "clip", "scale");
        assert!(
            result.is_none(),
            "malformed step (outputs != keys) skips the channel"
        );
    }

    #[test]
    fn resolve_keyframes_cubicspline_extracts_value_element_as_linear() {
        // Two keyframes, each a [in-tangent, value, out-tangent] triple. The
        // extracted track must be the VALUE element of each triple (index 3k+1),
        // which here differs from the surrounding tangent elements — pinning that
        // we keep the value, not a tangent.
        let in_t0 = Vec3::new(-1.0, -1.0, -1.0);
        let val0 = Vec3::new(5.0, 5.0, 5.0);
        let out_t0 = Vec3::new(9.0, 9.0, 9.0);
        let in_t1 = Vec3::new(-2.0, -2.0, -2.0);
        let val1 = Vec3::new(7.0, 7.0, 7.0);
        let out_t1 = Vec3::new(8.0, 8.0, 8.0);
        let raw = vec![in_t0, val0, out_t0, in_t1, val1, out_t1];

        let (vals, mode) =
            resolve_keyframes(raw, 2, Interpolation::CubicSpline, "clip", "rotation")
                .expect("well-formed cubic triple extracts values");
        assert_eq!(
            vals,
            vec![val0, val1],
            "extracts the value element of each triple"
        );
        assert_ne!(vals[0], in_t0, "extracted value is not the in-tangent");
        assert_ne!(vals[0], out_t0, "extracted value is not the out-tangent");
        assert_eq!(mode, Interp::Linear, "cubic degrades to LINEAR");
    }

    #[test]
    fn resolve_keyframes_cubicspline_wrong_shape_skips_channel() {
        // 5 outputs against 2 keyframes is not a valid 3x cubic triple shape:
        // skip the channel (None) rather than storing a length-mismatched track.
        let raw = vec![Vec3::ZERO; 5];
        let result = resolve_keyframes(raw, 2, Interpolation::CubicSpline, "clip", "scale");
        assert!(
            result.is_none(),
            "malformed cubic (outputs != 3x keys) skips the channel"
        );
    }

    #[test]
    fn validate_key_times_rejects_nan_and_duplicate_keys() {
        // Regression: NaN input times reached locate_span and could underflow
        // during sampling. Loader validation must skip the channel instead.
        assert!(
            !validate_key_times(&[f32::NAN, 1.0], "clip"),
            "NaN input time skips the channel"
        );
        assert!(
            !validate_key_times(&[0.0, 0.0], "clip"),
            "duplicate input times skip the channel"
        );
        assert!(
            !validate_key_times(&[1.0, 0.5], "clip"),
            "descending input times skip the channel"
        );
        assert!(
            validate_key_times(&[0.0, 0.5, 1.0], "clip"),
            "finite strictly ascending times are accepted"
        );
    }

    #[test]
    fn validate_vec3_keyframes_rejects_non_finite_outputs() {
        assert!(
            !validate_vec3_keyframes(&[Vec3::new(0.0, f32::INFINITY, 0.0)], "clip", "translation",),
            "non-finite translation skips the channel"
        );
        assert!(
            !validate_vec3_keyframes(&[Vec3::new(1.0, 2.0, f32::NAN)], "clip", "scale"),
            "non-finite scale skips the channel"
        );
        assert!(
            validate_vec3_keyframes(&[Vec3::ZERO, Vec3::ONE], "clip", "translation"),
            "finite vec3 outputs are accepted"
        );
    }

    #[test]
    fn validate_rotation_keyframes_rejects_non_finite_and_zero_quats() {
        assert!(
            !validate_rotation_keyframes(&[Quat::from_xyzw(0.0, 0.0, 0.0, 0.0)], "clip"),
            "zero quaternion skips the channel"
        );
        assert!(
            !validate_rotation_keyframes(&[Quat::from_xyzw(0.0, f32::NAN, 0.0, 1.0)], "clip"),
            "non-finite quaternion skips the channel"
        );
        assert!(
            validate_rotation_keyframes(&[Quat::IDENTITY], "clip"),
            "finite non-zero quaternion outputs are accepted"
        );
    }

    // --- Error handling (automated AC: malformed input returns Err, no panic) ---

    #[test]
    fn missing_file_returns_err_not_panic() {
        let result = load_model(Path::new("/nonexistent/does-not-exist.gltf"));
        assert!(matches!(result, Err(ModelLoadError::Import { .. })));
    }

    #[test]
    fn malformed_gltf_bytes_return_err_not_panic() {
        // Write garbage to a temp .gltf and confirm the loader returns an Import
        // error (the gltf crate rejects the bytes) rather than panicking.
        let tmp = std::env::temp_dir().join("postretro_malformed_model.gltf");
        std::fs::write(&tmp, b"this is not valid glTF JSON {{{").unwrap();
        let result = load_model(&tmp);
        let _ = std::fs::remove_file(&tmp);
        assert!(
            matches!(result, Err(ModelLoadError::Import { .. })),
            "malformed glTF should be an Import error, got {result:?}"
        );
    }

    fn fixture_json(path: &Path) -> serde_json::Value {
        let json = std::fs::read_to_string(path).expect("fixture JSON reads");
        serde_json::from_str(&json).expect("fixture JSON parses")
    }

    fn write_temp_fixture(name: &str, json: &serde_json::Value) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro_{name}_{}_{}.gltf",
            std::process::id(),
            unique
        ));
        std::fs::write(
            &path,
            serde_json::to_string(json).expect("mutated fixture serializes"),
        )
        .expect("mutated fixture writes");
        path
    }

    #[test]
    fn malformed_position_accessor_shape_returns_err_not_panic() {
        // Regression: glTF typed readers can debug-panic on mismatched accessor
        // shape. Loader validates the POSITION accessor before read_positions().
        let mut json = fixture_json(&multi_primitive_fixture_path());
        json["accessors"][0]["componentType"] = serde_json::json!(5123);
        let path = write_temp_fixture("bad_position_shape", &json);

        let result = std::panic::catch_unwind(|| load_model(&path));
        let _ = std::fs::remove_file(&path);
        let load_result = result.expect("malformed POSITION shape must not panic");
        assert!(
            matches!(
                &load_result,
                Err(ModelLoadError::InvalidAccessorShape {
                    attribute,
                    actual,
                    expected,
                    ..
                }) if attribute == "POSITION"
                    && actual == "U16 Vec3"
                    && expected == "F32 Vec3"
            ),
            "malformed POSITION shape should be typed loader error, got {load_result:?}",
        );
    }

    #[test]
    fn malformed_joints_accessor_shape_returns_err_not_panic() {
        // Regression: JOINTS_0 with an unsupported component type reached
        // read_joints(), whose shape match panics on impossible variants.
        let mut json = fixture_json(&multi_clip_fixture_path());
        json["accessors"][1]["componentType"] = serde_json::json!(5126);
        let path = write_temp_fixture("bad_joints_shape", &json);

        let result = std::panic::catch_unwind(|| load_model(&path));
        let _ = std::fs::remove_file(&path);
        let load_result = result.expect("malformed JOINTS_0 shape must not panic");
        assert!(
            matches!(
                &load_result,
                Err(ModelLoadError::InvalidAccessorShape {
                    attribute,
                    actual,
                    expected,
                    ..
                }) if attribute == "JOINTS_0"
                    && actual == "F32 Vec4"
                    && expected == "U8 Vec4 or U16 Vec4"
            ),
            "malformed JOINTS_0 shape should be typed loader error, got {load_result:?}",
        );
    }

    #[test]
    fn selected_skin_comes_from_selected_mesh_node() {
        // Regression: loader paired document.meshes().next() with
        // document.skins().next(), which could bind the first skin to an
        // unrelated first mesh. Prefer the node that carries both mesh and skin.
        let mut json = fixture_json(&multi_clip_fixture_path());
        json["meshes"]
            .as_array_mut()
            .expect("fixture has meshes")
            .insert(
                0,
                serde_json::json!({
                    "primitives": [
                        {
                            "attributes": { "POSITION": 0 },
                            "indices": 3,
                            "mode": 4
                        }
                    ]
                }),
            );
        json["nodes"][0]["mesh"] = serde_json::json!(1);
        let path = write_temp_fixture("skinned_node_second_mesh", &json);

        let selection = std::panic::catch_unwind(|| {
            let document = gltf::Gltf::open(&path)
                .expect("mutated fixture opens")
                .document;
            let selected = select_model(&document, &path.display().to_string())
                .expect("mutated fixture has a mesh");
            (
                selected.mesh.index(),
                selected.skin.as_ref().map(|skin| skin.index()),
            )
        });
        let _ = std::fs::remove_file(&path);
        let (mesh_index, skin_index) =
            selection.expect("selecting the mutated fixture's model must not panic");

        assert_eq!(
            mesh_index, 1,
            "selected mesh comes from the skinned node, not first mesh"
        );
        assert_eq!(
            skin_index,
            Some(0),
            "selected skin comes from the same node as the mesh"
        );
    }

    #[test]
    fn partial_inverse_bind_matrices_are_rejected() {
        // Regression: present-but-short inverseBindMatrices silently used
        // identity for missing joints, corrupting the skeleton.
        let mut json = fixture_json(&multi_clip_fixture_path());
        json["accessors"][4]["count"] = serde_json::json!(1);
        let path = write_temp_fixture("partial_inverse_binds", &json);

        let result = std::panic::catch_unwind(|| load_model(&path));
        let _ = std::fs::remove_file(&path);
        let load_result = result.expect("loading the mutated fixture must not panic");

        assert!(
            matches!(
                &load_result,
                Err(ModelLoadError::InverseBindCountMismatch {
                    expected: 2,
                    actual: 1,
                    ..
                })
            ),
            "partial inverseBindMatrices should be rejected, got {load_result:?}",
        );
    }

    // --- Real-model load (gated on the asset existing; no GPU) ---

    fn model_path() -> PathBuf {
        // CARGO_MANIFEST_DIR is crates/model; ../../content resolves via repo root.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/dev/models/decraniated_low_poly_retro_pixel/scene.gltf")
    }

    #[test]
    fn real_model_loads_skeleton_mesh_and_clip() {
        let path = model_path();
        if !path.exists() {
            eprintln!("skipping: model asset not present at {}", path.display());
            return;
        }
        let model = load_model(&path).expect("decraniated model loads");

        // One skinned mesh with geometry and indices.
        assert!(!model.mesh.vertices.is_empty(), "mesh has vertices");
        assert!(!model.mesh.indices.is_empty(), "mesh has indices");
        assert_eq!(
            model.mesh.indices.len() % 3,
            0,
            "triangle list index count divisible by 3"
        );
        // Every index addresses a real vertex.
        let vcount = model.mesh.vertices.len() as u32;
        assert!(
            model.mesh.indices.iter().all(|&i| i < vcount),
            "all indices in range"
        );

        // Skeleton: 26 joints, parent-before-child, exactly the joints that have
        // a parent reference it by an earlier index.
        assert_eq!(model.skeleton.joints.len(), 26, "skin has 26 joints");
        for (i, joint) in model.skeleton.joints.iter().enumerate() {
            if let Some(parent) = joint.parent {
                assert!(
                    parent < i,
                    "joint {i} parent {parent} must precede it (topological order)"
                );
            }
        }
        let roots = model
            .skeleton
            .joints
            .iter()
            .filter(|j| j.parent.is_none())
            .count();
        assert!(roots >= 1, "at least one root joint");

        // Joint indices on vertices stay within the joint count.
        let jcount = model.skeleton.joints.len() as u8;
        for v in &model.mesh.vertices {
            for &j in &v.joints {
                assert!(j < jcount, "vertex joint {j} < {jcount}");
            }
            // Weights sum to ~255 (a fully-weighted skinned vertex).
            let sum: u32 = v.weights.iter().map(|&w| w as u32).sum();
            assert_eq!(sum, 255, "vertex weights sum to 255");
        }

        // One clip named "mixamo.com" with non-zero duration and per-joint tracks
        // sized to the skeleton.
        assert_eq!(model.clips.len(), 1, "one animation clip");
        let clip = &model.clips[0];
        assert_eq!(clip.name, "mixamo.com");
        assert!(clip.duration > 0.0, "clip has a positive duration");
        assert_eq!(
            clip.joints.len(),
            model.skeleton.joints.len(),
            "clip tracks parallel to skeleton joints"
        );
        // At least one joint carries rotation keyframes (mixamo animates rotation).
        assert!(
            clip.joints.iter().any(|t| !t.rotation.is_empty()),
            "clip animates at least one joint's rotation"
        );

        // Material: one submesh per primitive; the single primitive resolves to
        // the baseColor key by content-hashing the dev PNG beside the glTF (not
        // a hardcoded table, not the zero placeholder).
        assert_eq!(model.submeshes.len(), 1, "one submesh per primitive");
        assert_eq!(
            model.submeshes[0].material_key,
            "581e80bb91c2d2e6fbed2aca5ba8fc0252aa7485579ea21376eeb294e972f0f1",
            "primitive resolves to the content-hashed baseColor cache key"
        );
        // The single submesh covers the whole merged index buffer.
        assert_eq!(
            model.submeshes[0].indices,
            0..model.mesh.indices.len() as u32,
            "single submesh spans the entire merged index buffer"
        );
    }

    fn reference_enemy_model_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/dev/models/reference_enemy_kaykit_knight/scene.gltf")
    }

    #[test]
    fn reference_enemy_model_loads_four_clips_used_by_the_brain() {
        // The CC0 KayKit Knight is packaged as the map-placeable reference enemy.
        // It must load through the same external-glTF path the engine uses at
        // level load, expose a skinned mesh + skeleton, and carry exactly the
        // four animation clips the `mesh`/`ai` state maps name. Guards the asset
        // and the GLB->glTF conversion (clip prune + body-part mesh merge)
        // against regression. Gated on the asset existing so a stripped checkout
        // skips rather than fails.
        let path = reference_enemy_model_path();
        if !path.exists() {
            eprintln!("skipping: model asset not present at {}", path.display());
            return;
        }
        let model = load_model(&path).expect("reference enemy (KayKit Knight) model loads");

        assert!(!model.mesh.vertices.is_empty(), "mesh has vertices");
        assert!(!model.mesh.indices.is_empty(), "mesh has indices");
        assert_eq!(
            model.mesh.indices.len() % 3,
            0,
            "triangle list index count divisible by 3"
        );
        let vcount = model.mesh.vertices.len() as u32;
        assert!(
            model.mesh.indices.iter().all(|&i| i < vcount),
            "all indices in range"
        );

        // Skinned skeleton, parent-before-child topo order.
        assert!(
            !model.skeleton.joints.is_empty(),
            "Knight skin contributes joints"
        );
        for (i, joint) in model.skeleton.joints.iter().enumerate() {
            if let Some(parent) = joint.parent {
                assert!(parent < i, "joint {i} parent {parent} precedes it");
            }
        }

        // The six body-part primitives were merged into one mesh; each is a
        // submesh of the single loaded mesh.
        assert_eq!(
            model.submeshes.len(),
            6,
            "six merged body-part primitives -> six submeshes"
        );

        // Exactly the four clips named by the reference enemy's mesh/ai maps.
        let mut clip_names: Vec<&str> = model.clips.iter().map(|c| c.name.as_str()).collect();
        clip_names.sort_unstable();
        assert_eq!(
            clip_names,
            vec![
                "1H_Melee_Attack_Slice_Horizontal",
                "Death_A",
                "Idle",
                "Walking_A",
            ],
            "pruned to the four clips the brain/mesh state maps reference"
        );
        for clip in &model.clips {
            assert!(clip.duration > 0.0, "clip `{}` has duration", clip.name);
            assert_eq!(
                clip.joints.len(),
                model.skeleton.joints.len(),
                "clip `{}` tracks parallel to skeleton joints",
                clip.name
            );
        }
    }

    // --- Submesh range partition (multi-primitive split bookkeeping) -------

    #[test]
    fn submesh_ranges_partition_merged_index_buffer() {
        // A synthetic multi-material glTF fixture (two primitives, two
        // materials) exercises the per-primitive submesh split without a GPU.
        // The submesh ranges must tile the merged index buffer end-to-end with
        // no gap or overlap, and every index must address a real merged vertex.
        //
        // Both primitives' base-color PNGs are absent at runtime, so both
        // submesh keys collapse to the SAME zero sentinel. This test only proves
        // range partitioning; distinct-key dedup is proven separately by the
        // `plan_*` tests in `render/mod.rs`.
        let fixture = multi_primitive_fixture_path();
        let model = load_model(&fixture).expect("synthetic multi-primitive fixture loads");

        assert!(
            model.submeshes.len() >= 2,
            "fixture has multiple primitives → multiple submeshes, got {}",
            model.submeshes.len()
        );

        // Ranges partition [0, indices.len()): each starts where the previous
        // ended, the first at 0, the last at the buffer end. No gaps, no overlap.
        let total = model.mesh.indices.len() as u32;
        let mut cursor = 0u32;
        for (i, sub) in model.submeshes.iter().enumerate() {
            assert_eq!(
                sub.indices.start, cursor,
                "submesh {i} starts at {} but previous ended at {cursor} (gap/overlap)",
                sub.indices.start
            );
            assert!(
                sub.indices.end >= sub.indices.start,
                "submesh {i} range is not well-formed: {:?}",
                sub.indices
            );
            cursor = sub.indices.end;
        }
        assert_eq!(
            cursor, total,
            "submesh ranges must cover the whole merged index buffer (end {cursor} vs {total})"
        );

        // Every index any submesh draws addresses a real merged vertex.
        let vcount = model.mesh.vertices.len() as u32;
        for sub in &model.submeshes {
            for &idx in &model.mesh.indices[sub.indices.start as usize..sub.indices.end as usize] {
                assert!(
                    idx < vcount,
                    "submesh index {idx} out of vertex range (0..{vcount})"
                );
            }
        }
    }

    fn multi_primitive_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/multi_primitive/multi_primitive.gltf")
    }

    #[test]
    fn loaded_mesh_bounds_enclose_every_vertex_position() {
        // The loader populates the local-space AABB from the merged stream.
        // Every vertex position must fall inside the computed bound (the
        // per-light caster cull relies on it being a true enclosing box), and
        // the box must be well-formed (min <= max on every axis).
        let model = load_model(&multi_primitive_fixture_path())
            .expect("synthetic multi-primitive fixture loads");
        let b = model.mesh.bounds();
        assert!(
            b.min.x <= b.max.x && b.min.y <= b.max.y && b.min.z <= b.max.z,
            "bounds must be well-formed (min <= max), got {b:?}"
        );
        for v in &model.mesh.vertices {
            let p = glam::Vec3::from_array(v.position);
            assert!(
                p.cmpge(b.min).all() && p.cmple(b.max).all(),
                "vertex {p:?} lies outside computed bounds {b:?}"
            );
        }
        // A real (non-empty) mesh must not collapse to the zero box.
        assert_ne!(b.min, b.max, "a non-degenerate mesh has a non-zero bound");
    }

    // --- Top-level extras → entity tags ------------------------------------

    fn extras_tags_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/extras_tags/extras_tags.gltf")
    }

    #[test]
    fn extras_tags_populate_loaded_model_tags() {
        // A glTF whose top-level `extras` carries `{ "tags": ["a","b"], .. }`
        // (plus an unknown key the loader ignores) loads its tags onto the model.
        let model = load_model(&extras_tags_fixture_path())
            .expect("extras-tags fixture loads (extras is metadata, never a load error)");
        assert_eq!(
            model.tags,
            vec!["a".to_string(), "b".to_string()],
            "top-level extras tags populate LoadedModel.tags",
        );
    }

    #[test]
    fn absent_extras_yields_empty_tags_without_error() {
        // The multi-primitive fixture carries no top-level `extras`; that is not
        // a load error — the model simply loads with no tags.
        let model = load_model(&multi_primitive_fixture_path())
            .expect("a glTF with no extras loads without error");
        assert!(
            model.tags.is_empty(),
            "absent extras yields no tags, got {:?}",
            model.tags,
        );
    }

    fn malformed_extras_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/malformed_extras/malformed_extras.gltf")
    }

    #[test]
    fn malformed_extras_loads_model_with_empty_tags() {
        // End-to-end: a glTF whose top-level `extras` carries a malformed `tags`
        // (a string, not an array) must still load — author metadata degrades to
        // no tags rather than failing the whole load.
        let model = load_model(&malformed_extras_fixture_path())
            .expect("malformed extras is metadata, never a load error");
        assert!(
            model.tags.is_empty(),
            "malformed extras yields no tags, got {:?}",
            model.tags,
        );
    }

    #[test]
    fn malformed_extras_yields_empty_tags_without_error() {
        // `extras` of the wrong shape (a non-array `tags`, an unrelated object)
        // must NOT fail the load — author metadata degrades to no tags. Drive
        // `read_model_tags` directly with several malformed raw-JSON shapes.
        for raw in [
            r#"{ "tags": "not-an-array" }"#,
            r#"{ "tags": [1, 2, 3] }"#,
            r#"{ "unrelated": true }"#,
            r#"[1, 2, 3]"#,
            r#""a bare string""#,
        ] {
            let boxed: Box<serde_json::value::RawValue> =
                serde_json::from_str(raw).expect("test raw JSON parses");
            let extras: gltf::json::Extras = Some(boxed);
            assert!(
                read_model_tags(&extras).is_empty(),
                "malformed extras {raw} must yield no tags",
            );
        }
    }

    #[test]
    fn absent_extras_helper_yields_empty_tags() {
        // The `None` arm (no `extras` block at all) yields no tags.
        let extras: gltf::json::Extras = None;
        assert!(read_model_tags(&extras).is_empty());
    }

    // --- Per-node extras → joint hit zones ---------------------------------

    /// Build a `gltf::json::Extras` from a raw-JSON string (the same shape the
    /// `extras` feature surfaces off a node), for driving `read_joint_zone`
    /// directly. Mirrors the malformed-tags helper test's construction.
    fn extras_from_raw(raw: &str) -> gltf::json::Extras {
        let boxed: Box<serde_json::value::RawValue> =
            serde_json::from_str(raw).expect("test raw JSON parses");
        Some(boxed)
    }

    #[test]
    fn read_joint_zone_reads_tag_and_radius() {
        // A well-formed per-node `extras` with both fields yields a zone whose
        // tag and positive finite radius are carried (no default substituted).
        let extras = extras_from_raw(r#"{ "hitZone": "head", "hitZoneRadius": 0.2 }"#);
        let zone = read_joint_zone(&extras).expect("present hitZone yields a zone");
        assert_eq!(zone.tag, "head");
        assert_eq!(zone.radius, Some(0.2));
    }

    #[test]
    fn read_joint_zone_radius_absent_stays_none_no_default() {
        // `hitZone` present but `hitZoneRadius` omitted → tag carried, radius
        // stays `None`. The 0.12 m default is the downstream consumer's job,
        // never applied here at load time.
        let extras = extras_from_raw(r#"{ "hitZone": "torso" }"#);
        let zone = read_joint_zone(&extras).expect("present hitZone yields a zone");
        assert_eq!(zone.tag, "torso");
        assert_eq!(zone.radius, None, "no default radius applied at load");
    }

    #[test]
    fn read_joint_zone_invalid_radius_degrades_to_none() {
        for raw in [
            r#"{ "hitZone": "head", "hitZoneRadius": -1.0 }"#,
            r#"{ "hitZone": "head", "hitZoneRadius": 0.0 }"#,
            r#"{ "hitZone": "head", "hitZoneRadius": "big" }"#,
        ] {
            let extras = extras_from_raw(raw);
            let zone = read_joint_zone(&extras).expect("valid hitZone tag is preserved");
            assert_eq!(zone.tag, "head");
            assert_eq!(
                zone.radius, None,
                "invalid optional radius in {raw} degrades to no authored radius",
            );
        }
    }

    #[test]
    fn read_joint_zone_untagged_or_malformed_yields_none() {
        // Every absent/untagged/malformed shape degrades to no zone (never a
        // load error). Mirrors the malformed-tags helper test: a missing
        // `hitZone`, a wrong-typed tag, an unrelated object, and non-object JSON
        // all collapse to `None`.
        for raw in [
            r#"{ "someOtherTool": "metadata" }"#, // no hitZone tag
            r#"{ "hitZone": 42 }"#,               // hitZone wrong type
            r#"[1, 2, 3]"#,                       // not an object
            r#""a bare string""#,                 // scalar
        ] {
            let extras = extras_from_raw(raw);
            assert!(
                read_joint_zone(&extras).is_none(),
                "malformed/untagged per-node extras {raw} must yield no zone",
            );
        }
    }

    #[test]
    fn read_joint_zone_absent_extras_yields_none() {
        // The `None` arm (a joint node with no `extras` block at all) → no zone.
        let extras: gltf::json::Extras = None;
        assert!(read_joint_zone(&extras).is_none());
    }

    #[test]
    fn identity_joint_zone_preserves_first_static_zone() {
        let zones = vec![
            None,
            Some(JointZone {
                tag: "crate".to_string(),
                radius: Some(0.75),
            }),
            Some(JointZone {
                tag: "unused".to_string(),
                radius: None,
            }),
        ];
        assert_eq!(
            identity_joint_zone(zones),
            Some(JointZone {
                tag: "crate".to_string(),
                radius: Some(0.75),
            }),
            "a static/no-skin model maps the first authored node zone to identity joint 0",
        );
    }

    #[test]
    fn skeleton_build_error_maps_to_model_load_error() {
        let err = skeleton_build_error(
            "model.gltf",
            SkeletonBuildError::ParentNotBeforeChild {
                joint: 0,
                parent: 1,
            },
        );
        assert!(
            matches!(
                &err,
                ModelLoadError::InvalidSkeleton {
                    path,
                    reason,
                } if path == "model.gltf"
                    && reason.contains("joint 0")
                    && reason.contains("parent 1")
            ),
            "got {err:?}",
        );
    }

    fn joint_zones_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/joint_zones/joint_zones.gltf")
    }

    #[test]
    fn joint_zones_populate_indexed_to_topo_order() {
        // End-to-end: a skin whose joint nodes carry per-node `extras` loads its
        // zones onto `LoadedModel.joint_zones`, INDEXED TO THE TOPO-ORDERED
        // joints — not to `skin.joints()` order. The fixture lists its skin
        // joints child-first (`[head_leaf, root, untagged_leaf]`) so a naive
        // skin-order read would be mis-indexed; the topo remap must reorder so
        // the root (parent) lands at topo 0.
        let model = load_model(&joint_zones_fixture_path()).expect("joint-zones fixture loads");

        // Topo order: root precedes its children (parent-before-child).
        assert_eq!(model.skeleton.joints.len(), 3, "three joints");
        assert_eq!(model.skeleton.joints[0].parent, None, "topo 0 is the root");
        assert_eq!(
            model.skeleton.joints[1].parent,
            Some(0),
            "topo 1 is a child"
        );
        assert_eq!(
            model.skeleton.joints[2].parent,
            Some(0),
            "topo 2 is a child"
        );

        // The zone table is parallel to the topo-ordered joints.
        assert_eq!(
            model.joint_zones.len(),
            model.skeleton.joints.len(),
            "joint_zones parallel to skeleton.joints",
        );

        // Topo 0 (the root, skin-joint index 1): tagged "torso" with radius 0.5
        // carried as-authored. If the read had used skin-joint order this slot
        // would instead hold the head leaf's zone.
        assert_eq!(
            model.joint_zones[0],
            Some(JointZone {
                tag: "torso".to_string(),
                radius: Some(0.5),
            }),
            "root joint's zone, correctly reindexed to topo 0",
        );

        // The two leaf joints (topo 1 and topo 2, in whatever order the topo
        // sort settles them) cover the tagged-leaf and untagged-leaf cases:
        //   - one leaf is tagged "head" with NO authored radius → its radius
        //     stays `None` (the default is applied downstream, not here);
        //   - one leaf carries `extras` with no `hitZone` tag → no zone at all,
        //     never a load failure.
        let leaf_zones = [&model.joint_zones[1], &model.joint_zones[2]];
        assert!(
            leaf_zones.contains(&&Some(JointZone {
                tag: "head".to_string(),
                radius: None,
            })),
            "a tagged leaf joint keeps tag 'head' with radius None, got {leaf_zones:?}",
        );
        assert!(
            leaf_zones.contains(&&None),
            "the untagged leaf joint (extras without hitZone) has no zone, got {leaf_zones:?}",
        );
    }

    #[test]
    fn static_fixture_loads_with_identity_skeleton_for_rigid_joint_zero() {
        let model = load_model(&multi_primitive_fixture_path())
            .expect("synthetic static multi-primitive fixture loads");
        assert_eq!(
            model.skeleton.joints.len(),
            1,
            "static/no-skin models still expose one identity palette joint",
        );
        let joint = model.skeleton.joints[0];
        assert_eq!(joint.parent, None);
        assert_eq!(joint.inverse_bind, Mat4::IDENTITY.to_cols_array_2d());
        assert_eq!(joint.rest_local, RestLocal::default());
        assert_eq!(
            model.joint_zones,
            vec![Some(JointZone {
                tag: "crate".to_string(),
                radius: Some(0.75),
            })],
            "static node hitZone extras map to the identity joint",
        );
        assert!(
            model
                .mesh
                .vertices
                .iter()
                .all(|v| v.joints == [0, 0, 0, 0] && v.weights == [255, 0, 0, 0]),
            "rigid vertices bind to identity joint 0",
        );
    }

    // --- Multi-clip fixture: full load path ----------------------------------
    //
    // A hand-authored two-joint glTF with two named clips of distinct durations,
    // exercising the LINEAR / STEP / CUBICSPLINE channel paths end-to-end. The
    // buffer is a base64 data-URI (no sidecar `.bin`), resolved by the loader's
    // `import_buffers` data-URI entry. Authored layout (verified byte-for-byte
    // against the encoded buffer):
    //   joints: skin = [node1 (root), node2 (child of node1)] → topo [0, 1].
    //   clip "idle"  (dur 1.0): LINEAR rotation on the root joint, keys at
    //                t=0,1 = identity, then Z+90°.
    //   clip "walk"  (dur 2.0): STEP translation on the root joint, keys at
    //                t=0,1,2 = (0,0,0),(10,0,0),(20,0,0); plus a CUBICSPLINE
    //                translation on the child joint, keys at t=0,1 with value
    //                elements (5,5,5),(7,7,7) and tangents that differ from
    //                those values (in=-1/-2, out=9/8) so the value-vs-tangent
    //                extraction is observable.

    use crate::anim::sample_clip;
    use glam::Mat4;

    const SAMPLE_EPS: f32 = 1.0e-4;

    fn multi_clip_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi_clip/multi_clip.gltf")
    }

    /// Sample a clip against the model's skeleton at `time` and return joint
    /// `joint`'s composed local translation (extracted from its skinning matrix;
    /// inverse-bind matrices in the fixture are identity, so the skinning matrix
    /// is the world-space joint transform).
    fn sampled_joint_translation(
        model: &LoadedModel,
        clip_idx: usize,
        joint: usize,
        time: f32,
    ) -> Vec3 {
        let mut out = Vec::new();
        sample_clip(&model.clips[clip_idx], &model.skeleton, time, &mut out);
        Mat4::from_cols_array_2d(&out[joint].matrix)
            .w_axis
            .truncate()
    }

    fn assert_vec3_close(got: Vec3, want: Vec3, ctx: &str) {
        assert!(
            (got - want).length() < SAMPLE_EPS,
            "{ctx}: expected {want:?}, got {got:?}",
        );
    }

    #[test]
    fn multi_clip_fixture_loads_all_clips_with_authored_names_and_durations() {
        let model = load_model(&multi_clip_fixture_path()).expect("multi-clip fixture loads");

        // Two joints in topo order (root before child).
        assert_eq!(model.skeleton.joints.len(), 2, "skin has two joints");
        assert_eq!(
            model.skeleton.joints[0].parent, None,
            "topo joint 0 is the root"
        );
        assert_eq!(
            model.skeleton.joints[1].parent,
            Some(0),
            "topo joint 1 is the root's child",
        );

        // More than one clip; every authored clip retained in glTF order, each
        // reporting its own duration.
        assert_eq!(model.clips.len(), 2, "both animations load as clips");
        assert_eq!(model.clips[0].name, "idle");
        assert_eq!(model.clips[1].name, "walk");
        assert!(
            (model.clips[0].duration - 1.0).abs() < SAMPLE_EPS,
            "'idle' duration is its own latest keyframe (1.0), got {}",
            model.clips[0].duration,
        );
        assert!(
            (model.clips[1].duration - 2.0).abs() < SAMPLE_EPS,
            "'walk' duration is its own latest keyframe (2.0), got {}",
            model.clips[1].duration,
        );
    }

    #[test]
    fn multi_clip_fixture_step_channel_holds_lower_keyframe_value() {
        // "walk" clip index 1: STEP translation on the root joint (topo 0), keys
        // (0,0,0)@0, (10,0,0)@1, (20,0,0)@2. A STEP channel holds the earlier
        // keyframe's value between keys and snaps at/after a keyframe time.
        let model = load_model(&multi_clip_fixture_path()).expect("multi-clip fixture loads");

        // Between keys 0 and 1: holds the lower key exactly (NOT the (5,0,0) a
        // LINEAR track would lerp to at the midpoint).
        assert_vec3_close(
            sampled_joint_translation(&model, 1, 0, 0.5),
            Vec3::new(0.0, 0.0, 0.0),
            "STEP holds the earlier keyframe value mid-span",
        );
        // At a keyframe time: snaps to that keyframe's value.
        assert_vec3_close(
            sampled_joint_translation(&model, 1, 0, 1.0),
            Vec3::new(10.0, 0.0, 0.0),
            "STEP snaps to the keyframe value at its time",
        );
        // After that keyframe, before the next: still holds it.
        assert_vec3_close(
            sampled_joint_translation(&model, 1, 0, 1.5),
            Vec3::new(10.0, 0.0, 0.0),
            "STEP holds the keyframe value until the next key",
        );
    }

    #[test]
    fn multi_clip_fixture_cubicspline_channel_samples_keyframe_values_not_tangents() {
        // "walk" clip index 1: CUBICSPLINE translation on the child joint (topo
        // 1). The loader degrades cubic to LINEAR by extracting each keyframe's
        // VALUE element — (5,5,5)@0 and (7,7,7)@1 — discarding the in/out
        // tangents (-1/-2 and 9/8). Sampling at the keys must return the values,
        // which are distinct from any adjacent tangent element.
        let model = load_model(&multi_clip_fixture_path()).expect("multi-clip fixture loads");

        // At t=0: keyframe-0 value (5,5,5) — not the in-tangent (-1,-1,-1) nor
        // the out-tangent (9,9,9).
        assert_vec3_close(
            sampled_joint_translation(&model, 1, 1, 0.0),
            Vec3::new(5.0, 5.0, 5.0),
            "CUBICSPLINE sample is keyframe-0 value, not its tangents",
        );
        // The stored track holds exactly the two extracted VALUE elements — no
        // tangents — and is marked LINEAR (cubic degraded). Asserting the track
        // contents directly pins keyframe-1's value (7,7,7); sampling at t=1 is
        // avoided because the root's STEP translation channel snaps at t=1, which
        // would compose into the child's world translation and obscure the check.
        let child_translation = &model.clips[1].joints[1].translation;
        assert_eq!(
            child_translation.mode(),
            Interp::Linear,
            "CUBICSPLINE channel is stored as LINEAR (cubic degraded at load)",
        );
        assert_eq!(
            child_translation.values().len(),
            2,
            "two keyframes → two extracted values (tangents discarded)",
        );
        assert_vec3_close(
            child_translation.values()[0],
            Vec3::new(5.0, 5.0, 5.0),
            "extracted keyframe-0 value, not a tangent",
        );
        assert_vec3_close(
            child_translation.values()[1],
            Vec3::new(7.0, 7.0, 7.0),
            "extracted keyframe-1 value, not a tangent",
        );
        // The midpoint lerps between the extracted VALUES (linear degrade), so it
        // lands between (5,5,5) and (7,7,7) — never near a tangent magnitude.
        assert_vec3_close(
            sampled_joint_translation(&model, 1, 1, 0.5),
            Vec3::new(6.0, 6.0, 6.0),
            "CUBICSPLINE degrades to LINEAR: midpoint lerps between extracted values",
        );
    }
}
