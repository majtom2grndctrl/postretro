// glTF → engine skinned-model loader (CPU-only; no wgpu).
// See: context/lib/rendering_pipeline.md §9 · context/lib/build_pipeline.md §Baked texture mips · context/lib/entity_model.md §7

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use glam::{Mat4, Quat, Vec3};
use gltf::accessor::{DataType, Dimensions};
use postretro_foundation::pose::MAX_FEET;
use postretro_level_format::gltf_resolve::resolve_material_base_color_path;
use postretro_level_format::octahedral;
use thiserror::Error;

use super::clip_library::{ClipLibrarySources, MeshRig, append_library_clips, read_clip_library};
use super::gltf_clips::parse_clip;
pub use super::gltf_extras::JointZone;
#[cfg(test)]
use super::gltf_extras::LegRef;
use super::gltf_extras::{
    JointPoseMetadata, LegNameFamily, read_clip_library_path, read_joint_zone, read_model_tags,
    read_mount_axes, read_pose_masks, read_socket_name,
};
use super::mesh::{SkinnedMesh, SkinnedVertex};
pub use super::mount::MountAxes;
use super::pose_modifier::{JointMask, LegChain, ModifierEntry, PoseModifier, PoseModifierStack};
use super::skeleton::{AnimationClip, Joint, RestLocal, Skeleton, SkeletonBuildError};

const MIN_PACKED_DIRECTION_LENGTH_SQUARED: f32 = 1.0e-12;

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

/// Convention-named skeletal pose masks authored on glTF joint nodes.
///
/// Every mask contains indices into the model's topologically ordered
/// [`Skeleton::joints`] array. A joint may belong to more than one mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PoseMaskSet {
    pub aim_spine: JointMask,
    pub upper_body: JointMask,
    pub lower_body: JointMask,
}

/// A named attachment point authored on a model node.
///
/// Skinned sockets identify a topologically ordered skeleton joint. Rigid
/// sockets carry their fully composed rest transform in mesh-node local space.
#[derive(Debug, Clone, PartialEq)]
pub enum SocketBinding {
    SkinnedJoint(usize),
    RigidRest(Mat4),
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
    /// Animation clips in the mesh's joint order: the glTF document's, in authored
    /// order, then those bound from its clip library, in library order. All clips
    /// load; addressed by name or index.
    pub clips: Vec<AnimationClip>,
    /// One submesh per mesh primitive, in primitive order: the material cache
    /// key and the index range it occupies in the merged buffer. The renderer
    /// resolves each key against the shared material/texture cache and draws each
    /// range against its (possibly shared) material.
    pub submeshes: Vec<Submesh>,
    /// Author-named attachment points. The table keeps the first valid node in
    /// deterministic document-node traversal order when names are duplicated.
    pub sockets: HashMap<String, SocketBinding>,
    /// Optional raw-source weapon axes declared on the selected mesh node's
    /// `extras.mount`. Malformed metadata degrades to `None` without rejecting
    /// the model; the optional applied Euler degrades independently.
    pub mount: Option<MountAxes>,
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
    /// Convention-named pose masks, reindexed to the same topological joint
    /// order as `skeleton` and `joint_zones`.
    pub pose_masks: PoseMaskSet,
    /// Ordered modifier stack derived from `pose_masks` (and `legs`) at load
    /// time. The upper/lower split precedes the aim-pitch bend when both are
    /// authored; a non-empty `legs` set appends one foot-IK entry.
    pub pose_stack: PoseModifierStack,
    /// Ordered leg set built from `legL`/`legR`/`footL`/`footR` (biped side
    /// names, L=0, R=1) or `leg{i}`/`foot{i}` (indexed, N-leg) joint tags,
    /// reindexed to the same topological joint order as `skeleton`. Leg `i`
    /// consumes foot probe `i`; the set is capped at
    /// [`postretro_foundation::pose::MAX_FEET`]. Kept OUTSIDE the fixed `Copy`
    /// [`PoseMaskSet`] because it is a variable-length list, and read game-side
    /// by the fixed-tick ground-probe step. Empty when no leg tags are authored.
    pub legs: Vec<LegChain>,
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
    /// A required glTF attribute or accessor is absent (e.g. a primitive's
    /// POSITION, or a skin's inverseBindMatrices).
    #[error("glTF missing required attribute '{attribute}': {path}")]
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
    /// A present accessor's stride, count or ranges cannot be read safely.
    #[error("glTF accessor '{attribute}' {reason}: {path}")]
    UnreadableAccessor {
        path: String,
        attribute: String,
        reason: &'static str,
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
    /// A present float stream carries NaN or infinity.
    #[error("glTF accessor '{attribute}' has non-finite value at element {index}: {path}")]
    NonFiniteAccessorValue {
        path: String,
        attribute: String,
        index: usize,
    },
    /// A present direction stream carries a zero or near-zero vector that cannot
    /// be normalized for octahedral packing.
    #[error("glTF accessor '{attribute}' has degenerate direction at element {index}: {path}")]
    DegenerateDirectionVector {
        path: String,
        attribute: String,
        index: usize,
    },
    /// A triangle-list primitive does not contain whole triangles.
    #[error("glTF primitive '{attribute}' count {count} is not divisible by 3: {path}")]
    IncompleteTriangleList {
        path: String,
        attribute: String,
        count: usize,
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
    /// A skinned model's clip library, or a buffer it references, could not be
    /// read or parsed.
    #[error("clip library {library} of {path} could not be read: {source}")]
    ClipLibraryUnreadable {
        path: String,
        library: String,
        #[source]
        source: gltf::Error,
    },
    /// The loader selected no locomotion root, which defines the root for binding.
    #[error("glTF skin has no selected root to bind clip library {library}: {path}")]
    ClipLibraryNoRoot { path: String, library: String },
    /// The clip library has no node named as the mesh's root joint.
    #[error("clip library {library} has no joint named '{joint}', the root of {path}")]
    ClipLibraryRootMissing {
        path: String,
        library: String,
        joint: String,
    },
    /// A root's local rest Y is not a finite positive height, or the two heights
    /// give no finite ratio above zero.
    #[error(
        "root rest heights must be finite and positive, with a usable ratio, to bind clip library {library}: mesh {mesh_height}, library {library_height}: {path}"
    )]
    ClipLibraryRootHeight {
        path: String,
        library: String,
        mesh_height: f32,
        library_height: f32,
    },
    /// Neither the model nor its clip library carries a clip.
    #[error("clip library {library} has no clips and {path} embeds none")]
    ClipLibraryEmpty { path: String, library: String },
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

/// Refuse an accessor gltf cannot safely read: shape first, since readability
/// depends on the element size the shape fixes. Call before the matching
/// `read_*`, which checks neither.
fn validate_readable_accessor(
    accessor: &gltf::Accessor,
    buffers: &[gltf::buffer::Data],
    path_str: &str,
    attribute: &str,
    expected: &[(DataType, Dimensions)],
) -> Result<(), ModelLoadError> {
    let actual = (accessor.data_type(), accessor.dimensions());
    if !expected.contains(&actual) {
        return Err(ModelLoadError::InvalidAccessorShape {
            path: path_str.to_string(),
            attribute: attribute.to_string(),
            expected: format_accessor_shapes(expected),
            actual: format_accessor_shape(actual),
        });
    }
    match accessor_read_refusal(accessor, buffers) {
        Some(reason) => Err(ModelLoadError::UnreadableAccessor {
            path: path_str.to_string(),
            attribute: attribute.to_string(),
            reason,
        }),
        None => Ok(()),
    }
}

/// Why gltf cannot safely read `accessor`, or `None` when it can. Call once the
/// accessor's shape is known good, so `size()` is the element size gltf reads.
///
/// `gltf::accessor::Iter` trusts what document validation leaves unchecked. A
/// stride below the element size panics on the first read, in release too. Its
/// range arithmetic overflows on a zero count or a huge one. A sparse accessor
/// with no buffer view allocates `count` elements whatever the file's size, so
/// it is refused; one with a buffer view reads, its own ranges checked.
pub(super) fn accessor_read_refusal(
    accessor: &gltf::Accessor,
    buffers: &[gltf::buffer::Data],
) -> Option<&'static str> {
    if accessor.count() == 0 {
        return Some("has no elements");
    }
    let Some(view) = accessor.view() else {
        return Some("has no buffer view");
    };
    let element_size = accessor.size();
    let base = view_read_refusal(
        &view,
        accessor.offset(),
        accessor.count(),
        element_size,
        buffers,
    );
    if base.is_some() {
        return base;
    }
    let sparse = accessor.sparse()?;
    if sparse.count() == 0 || sparse.count() > accessor.count() {
        return Some("has a sparse count outside its elements");
    }
    let indices = sparse.indices();
    let values = sparse.values();
    view_read_refusal(
        &indices.view(),
        indices.offset(),
        sparse.count(),
        indices.index_type().size(),
        buffers,
    )
    .or_else(|| {
        view_read_refusal(
            &values.view(),
            values.offset(),
            sparse.count(),
            element_size,
            buffers,
        )
    })
}

/// Why `count` elements at `offset` in `view` cannot be read, if they cannot.
fn view_read_refusal(
    view: &gltf::buffer::View,
    offset: usize,
    count: usize,
    element_size: usize,
    buffers: &[gltf::buffer::Data],
) -> Option<&'static str> {
    let stride = view.stride().unwrap_or(element_size);
    if stride < element_size {
        return Some("has a byte stride below its element size");
    }
    let read_end = (count - 1)
        .checked_mul(stride)
        .and_then(|span| span.checked_add(element_size))
        .and_then(|span| span.checked_add(offset));
    let view_end = view.offset().checked_add(view.length());
    let buffer_length = buffers.get(view.buffer().index()).map(|data| data.0.len());
    match (read_end, view_end, buffer_length) {
        (Some(read_end), Some(view_end), Some(buffer_length))
            if read_end <= view.length() && view_end <= buffer_length =>
        {
            None
        }
        _ => Some("overruns its buffer"),
    }
}

fn format_accessor_shapes(shapes: &[(DataType, Dimensions)]) -> String {
    shapes
        .iter()
        .map(|&shape| format_accessor_shape(shape))
        .collect::<Vec<_>>()
        .join(" or ")
}

pub(super) fn format_accessor_shape(shape: (DataType, Dimensions)) -> String {
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

fn validate_finite_vec3_stream(
    path_str: &str,
    attribute: &str,
    values: &[[f32; 3]],
) -> Result<(), ModelLoadError> {
    for (i, value) in values.iter().enumerate() {
        if !value.iter().all(|component| component.is_finite()) {
            return Err(ModelLoadError::NonFiniteAccessorValue {
                path: path_str.to_string(),
                attribute: attribute.to_string(),
                index: i,
            });
        }
    }
    Ok(())
}

fn validate_finite_vec2_stream(
    path_str: &str,
    attribute: &str,
    values: &[[f32; 2]],
) -> Result<(), ModelLoadError> {
    for (i, value) in values.iter().enumerate() {
        if !value.iter().all(|component| component.is_finite()) {
            return Err(ModelLoadError::NonFiniteAccessorValue {
                path: path_str.to_string(),
                attribute: attribute.to_string(),
                index: i,
            });
        }
    }
    Ok(())
}

fn validate_finite_vec4_stream(
    path_str: &str,
    attribute: &str,
    values: &[[f32; 4]],
) -> Result<(), ModelLoadError> {
    for (i, value) in values.iter().enumerate() {
        if !value.iter().all(|component| component.is_finite()) {
            return Err(ModelLoadError::NonFiniteAccessorValue {
                path: path_str.to_string(),
                attribute: attribute.to_string(),
                index: i,
            });
        }
    }
    Ok(())
}

fn validate_packable_vec3_directions(
    path_str: &str,
    attribute: &str,
    values: &[[f32; 3]],
) -> Result<(), ModelLoadError> {
    for (i, value) in values.iter().enumerate() {
        let length_squared = value[0] * value[0] + value[1] * value[1] + value[2] * value[2];
        if length_squared <= MIN_PACKED_DIRECTION_LENGTH_SQUARED {
            return Err(ModelLoadError::DegenerateDirectionVector {
                path: path_str.to_string(),
                attribute: attribute.to_string(),
                index: i,
            });
        }
    }
    Ok(())
}

fn validate_packable_vec4_xyz_directions(
    path_str: &str,
    attribute: &str,
    values: &[[f32; 4]],
) -> Result<(), ModelLoadError> {
    for (i, value) in values.iter().enumerate() {
        let length_squared = value[0] * value[0] + value[1] * value[1] + value[2] * value[2];
        if length_squared <= MIN_PACKED_DIRECTION_LENGTH_SQUARED {
            return Err(ModelLoadError::DegenerateDirectionVector {
                path: path_str.to_string(),
                attribute: attribute.to_string(),
                index: i,
            });
        }
    }
    Ok(())
}

fn validate_finite_mat4_stream(
    path_str: &str,
    attribute: &str,
    values: &[[[f32; 4]; 4]],
) -> Result<(), ModelLoadError> {
    for (i, value) in values.iter().enumerate() {
        if !value
            .iter()
            .flatten()
            .all(|component| component.is_finite())
        {
            return Err(ModelLoadError::NonFiniteAccessorValue {
                path: path_str.to_string(),
                attribute: attribute.to_string(),
                index: i,
            });
        }
    }
    Ok(())
}

fn validate_triangle_count(
    path_str: &str,
    attribute: &str,
    count: usize,
) -> Result<(), ModelLoadError> {
    if count.is_multiple_of(3) {
        return Ok(());
    }
    Err(ModelLoadError::IncompleteTriangleList {
        path: path_str.to_string(),
        attribute: attribute.to_string(),
        count,
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
    mesh_node: Option<gltf::Node<'a>>,
    skin: Option<gltf::Skin<'a>>,
}

fn select_model<'a>(
    document: &'a gltf::Document,
    path_str: &str,
) -> Result<SelectedModel<'a>, ModelLoadError> {
    for node in document.nodes() {
        if let (Some(mesh), Some(skin)) = (node.mesh(), node.skin()) {
            if skin.joints().next().is_none() {
                continue;
            }
            return Ok(SelectedModel {
                mesh,
                mesh_node: Some(node),
                skin: Some(skin),
            });
        }
    }

    let mesh = document
        .meshes()
        .next()
        .ok_or_else(|| ModelLoadError::NoMesh(path_str.to_string()))?;
    let mesh_node = document.nodes().find(|node| {
        node.mesh()
            .is_some_and(|node_mesh| node_mesh.index() == mesh.index())
    });
    Ok(SelectedModel {
        mesh,
        mesh_node,
        skin: None,
    })
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

/// In-progress accumulation of one leg's joints while scanning topo joints.
#[derive(Default)]
struct LegBuilder {
    /// Joints tagged into this leg's chain via a `leg*` name.
    chain_mask: JointMask,
    /// The joint tagged as this leg's foot/ankle target via a `foot*` name.
    foot_joint: Option<usize>,
    /// More than one joint declared itself as this leg's foot target. The
    /// loader cannot choose a meaningful end effector, so the whole leg drops.
    duplicate_foot_target: bool,
}

fn pose_masks_from_topo_metadata(
    topo_order: &[usize],
    rest_pose_metadata: &[JointPoseMetadata],
    path_str: &str,
) -> (PoseMaskSet, Vec<f32>, Vec<LegChain>) {
    let mut masks = PoseMaskSet::default();
    let mut aim_bend_weights = vec![1.0; topo_order.len()];
    // Keyed by leg index → ascending iteration yields the ordered leg set
    // (L=0 before R=1; `leg{i}` in `i` order).
    let mut leg_builders: BTreeMap<usize, LegBuilder> = BTreeMap::new();
    let mut saw_side = false;
    let mut saw_indexed = false;

    for (topo_idx, &skin_idx) in topo_order.iter().enumerate() {
        let metadata = rest_pose_metadata
            .get(skin_idx)
            .copied()
            .unwrap_or(JointPoseMetadata {
                aim_bend_weight: 1.0,
                ..JointPoseMetadata::default()
            });
        if metadata.saw_side_leg_name {
            note_family(LegNameFamily::Side, &mut saw_side, &mut saw_indexed);
        }
        if metadata.saw_indexed_leg_name {
            note_family(LegNameFamily::Indexed, &mut saw_side, &mut saw_indexed);
        }
        if metadata.aim_spine {
            let _ = masks.aim_spine.insert(topo_idx);
            aim_bend_weights[topo_idx] = metadata.aim_bend_weight;
        }
        if metadata.upper_body {
            let _ = masks.upper_body.insert(topo_idx);
        }
        if metadata.lower_body {
            let _ = masks.lower_body.insert(topo_idx);
        }
        if let Some(leg) = metadata.leg_chain {
            note_family(leg.family, &mut saw_side, &mut saw_indexed);
            let _ = leg_builders
                .entry(leg.index)
                .or_default()
                .chain_mask
                .insert(topo_idx);
        }
        if let Some(foot) = metadata.foot_target {
            note_family(foot.family, &mut saw_side, &mut saw_indexed);
            let builder = leg_builders.entry(foot.index).or_default();
            if builder.foot_joint.replace(topo_idx).is_some() {
                log::warn!(
                    "[Model] leg {} in {path_str} has more than one foot target (footL/footR/foot{{i}}); dropping the leg",
                    foot.index
                );
                builder.duplicate_foot_target = true;
            }
        }
    }

    if saw_side && saw_indexed {
        log::warn!(
            "[Model] leg tags in {path_str} mix side names (legL/legR) and indexed names (leg{{i}}) for the same leg set; keeping both"
        );
    }

    let legs = build_leg_set(leg_builders, path_str);
    (masks, aim_bend_weights, legs)
}

fn note_family(family: LegNameFamily, saw_side: &mut bool, saw_indexed: &mut bool) {
    match family {
        LegNameFamily::Side => *saw_side = true,
        LegNameFamily::Indexed => *saw_indexed = true,
    }
}

/// Assemble the ordered, capacity-capped leg set from the per-index builders.
///
/// Iteration is ascending leg index (biped L=0 before R=1). A leg is emitted
/// only when it has BOTH a chain (at least one `leg*` joint) and a foot/ankle
/// target (`foot*`); an incomplete leg warns and is dropped. The foot joint is
/// folded into the chain mask so the solver always finds its end effector in
/// the chain. Legs past [`MAX_FEET`] warn and are dropped.
fn build_leg_set(leg_builders: BTreeMap<usize, LegBuilder>, path_str: &str) -> Vec<LegChain> {
    let mut legs = Vec::new();
    for (index, builder) in leg_builders {
        if builder.duplicate_foot_target {
            continue;
        }
        match (builder.chain_mask.is_empty(), builder.foot_joint) {
            (false, Some(foot_joint)) => {
                if legs.len() >= MAX_FEET {
                    log::warn!(
                        "[Model] more than {MAX_FEET} legs authored in {path_str}; dropping leg {index}"
                    );
                    continue;
                }
                let mut chain_mask = builder.chain_mask;
                // The foot/ankle joint is the chain's end effector; ensure it is
                // a chain member even if only `foot*` (not `leg*`) tagged it.
                let _ = chain_mask.insert(foot_joint);
                legs.push(LegChain {
                    chain_mask,
                    foot_joint,
                });
            }
            (true, Some(_)) => log::warn!(
                "[Model] leg {index} in {path_str} has a foot target (foot{index}/footL/footR) but no leg chain; dropping"
            ),
            (false, None) => log::warn!(
                "[Model] leg {index} in {path_str} has a chain but no foot/ankle target; dropping"
            ),
            (true, None) => {}
        }
    }
    legs
}

/// Keep only solver-ready two-bone leg declarations. The tag accumulator cannot
/// validate topology because it runs before [`Skeleton::new`] establishes the
/// parent links; do that once the topo-ordered skeleton exists. A FootIk leg is
/// exactly hip → knee → ankle/foot, with the named foot target at its terminal
/// joint. Disconnected, branched, oversized, and mis-targeted masks degrade to
/// no leg instead of reaching the solver as a partially meaningful declaration.
fn validate_leg_set(skeleton: &Skeleton, legs: Vec<LegChain>, path_str: &str) -> Vec<LegChain> {
    legs.into_iter()
        .filter_map(|leg| {
            let Some(chain) = ordered_aim_spine_chain(skeleton, leg.chain_mask) else {
                log::warn!(
                    "[Model] foot-IK leg in {path_str} is not one connected non-branching parent-linked chain; dropping"
                );
                return None;
            };
            if chain.len() != 3 {
                log::warn!(
                    "[Model] foot-IK leg in {path_str} has {} tagged joints; expected hip → knee → ankle/foot; dropping",
                    chain.len()
                );
                return None;
            }
            if chain.last().copied() != Some(leg.foot_joint) {
                log::warn!(
                    "[Model] foot-IK leg in {path_str} names foot joint {} but its chain ends at {}; dropping",
                    leg.foot_joint,
                    chain.last().copied().unwrap_or_default()
                );
                return None;
            }
            Some(leg)
        })
        .collect()
}

fn ordered_aim_spine_chain(skeleton: &Skeleton, mask: JointMask) -> Option<Vec<usize>> {
    let joints: Vec<usize> = mask
        .iter()
        .filter(|&joint| joint < skeleton.joints.len())
        .collect();
    if joints.is_empty() {
        return None;
    }

    let roots: Vec<usize> = joints
        .iter()
        .copied()
        .filter(|&joint| {
            skeleton.joints[joint]
                .parent
                .is_none_or(|parent| !mask.contains(parent))
        })
        .collect();
    if roots.len() != 1 {
        return None;
    }

    let mut children = vec![Vec::new(); skeleton.joints.len()];
    for &joint in &joints {
        if let Some(parent) = skeleton.joints[joint].parent
            && mask.contains(parent)
        {
            children[parent].push(joint);
        }
    }
    if joints.iter().any(|&joint| children[joint].len() > 1) {
        return None;
    }

    let mut ordered = Vec::with_capacity(joints.len());
    let mut current = Some(roots[0]);
    while let Some(joint) = current {
        ordered.push(joint);
        current = children[joint].first().copied();
    }
    (ordered.len() == joints.len()).then_some(ordered)
}

fn build_pose_modifier_stack(
    skeleton: &Skeleton,
    masks: PoseMaskSet,
    aim_bend_weights: &[f32],
    legs: &[LegChain],
    path_str: &str,
) -> PoseModifierStack {
    let mut stack = PoseModifierStack::default();

    if !masks.upper_body.is_empty() && !masks.lower_body.is_empty() {
        stack.push(ModifierEntry {
            mask: masks.upper_body,
            modifier: PoseModifier::UpperLowerSplit {
                lower_body_mask: masks.lower_body,
            },
        });
    }

    if !masks.aim_spine.is_empty() {
        if let Some(chain) = ordered_aim_spine_chain(skeleton, masks.aim_spine) {
            // `chain` is root->tip, parent-linked. `Skeleton::new` enforces
            // parent index < child index, so for a connected chain this order
            // equals ascending joint index — matching `AimPitchBend`'s
            // mask-parallel (ascending-index) `bend_weights` contract.
            let bend_weights = chain
                .iter()
                .map(|&joint| aim_bend_weights.get(joint).copied().unwrap_or(1.0))
                .collect();
            stack.push(ModifierEntry {
                mask: masks.aim_spine,
                modifier: PoseModifier::AimPitchBend { bend_weights },
            });
        } else {
            log::warn!(
                "[Model] aimSpine pose mask in {path_str} is not one connected parent-linked chain; dropping aim-pitch bend"
            );
        }
    }

    // Presence of leg tags builds the foot-IK modifier, exactly as `aimSpine`
    // tags build the aim-pitch bend — no descriptor gate. The per-leg masks live
    // inside each `LegChain`, so the enclosing entry mask is unused (empty).
    if !legs.is_empty() {
        stack.push(ModifierEntry {
            mask: JointMask::new(),
            modifier: PoseModifier::FootIk {
                legs: legs.to_vec(),
            },
        });
    }

    stack
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
    let sources = read_model_sources(path)?;
    load_model_from_sources(path, sources)
}

/// Everything [`load_model`] reads from disk, read once: the glTF document
/// (its bytes' blake3 kept), every buffer it references (external `.bin`
/// files and `data:` URIs alike), and each primitive's material key of the
/// mesh the load selects, which content-hashes that material's base-color
/// PNG, and the clip library a skinned model names. A content-keyed model cache
/// hashes exactly this, and a cache miss parses from it without reading any file
/// a second time.
pub struct ModelSources {
    document: gltf::Document,
    buffers: Vec<gltf::buffer::Data>,
    document_digest: [u8; 32],
    material_keys: Vec<String>,
    library: Option<ClipLibrarySources>,
}

impl ModelSources {
    /// blake3 of the glTF document's bytes as read.
    pub fn document_digest(&self) -> [u8; 32] {
        self.document_digest
    }

    /// Each referenced buffer's bytes, in buffer-index order.
    pub fn buffers(&self) -> impl Iterator<Item = &[u8]> {
        self.buffers.iter().map(|data| data.0.as_slice())
    }

    /// One material key per primitive of the selected mesh, in primitive
    /// order: the key each [`Submesh`] of the load will carry.
    pub fn material_keys(&self) -> &[String] {
        &self.material_keys
    }

    /// The clip library this load binds: `None` for a model that names none, a
    /// malformed reference, and a static model, which ignores its reference.
    pub fn library(&self) -> Option<&ClipLibrarySources> {
        self.library.as_ref()
    }
}

impl std::fmt::Debug for ModelSources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelSources")
            .field("buffers", &self.buffers.len())
            .field("material_keys", &self.material_keys)
            .field("library", &self.library.is_some())
            .finish_non_exhaustive()
    }
}

// Compile-time guard: sources are read on a load-pool thread and parsed there
// or on the main thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<ModelSources>();
};

/// Read the files [`load_model`] reads, and fail as it fails before its
/// parse proper: an unreadable or malformed document or buffer, a document with
/// no mesh, or an unreadable clip library. Opens the document and resolves only
/// geometry/skin/animation buffers, no image data: `gltf::import` would decode
/// every referenced PNG; `import_buffers` skips that decode. The `.glb`
/// binary-blob arg is `None` (external-`.gltf` only; embedded-image / `.glb`
/// support is out of scope).
pub fn read_model_sources(path: &Path) -> Result<ModelSources, ModelLoadError> {
    let path_str = path.display().to_string();
    let import_error = |source| ModelLoadError::Import {
        path: path_str.clone(),
        source,
    };
    let parent_dir = path.parent().unwrap_or_else(|| Path::new(""));
    // `Gltf::open` is `from_reader` over the opened file; reading the bytes
    // first keeps them for the digest and parses through the same reader.
    let bytes = std::fs::read(path).map_err(|error| import_error(gltf::Error::Io(error)))?;
    let document = gltf::Gltf::from_reader(std::io::Cursor::new(bytes.as_slice()))
        .map_err(import_error)?
        .document;
    let buffers = gltf::import_buffers(&document, Some(parent_dir), None).map_err(import_error)?;
    let (material_keys, skinned) = {
        let SelectedModel { mesh, skin, .. } = select_model(&document, &path_str)?;
        let keys = mesh
            .primitives()
            .map(|primitive| content_hash_material_key(&primitive.material(), parent_dir))
            .collect();
        (keys, skin.is_some())
    };
    // The skin check comes first: a static model's library is neither read nor
    // keyed, so a broken one cannot fail it.
    let library = match clip_library_path(&document, path) {
        Some(library_path) if skinned => Some(read_clip_library(&library_path, &path_str)?),
        Some(library_path) => {
            log::warn!(
                "[Model] static model {path_str} names clip library {}; ignoring it",
                library_path.display()
            );
            None
        }
        None => None,
    };
    Ok(ModelSources {
        document,
        buffers,
        document_digest: *blake3::hash(&bytes).as_bytes(),
        material_keys,
        library,
    })
}

/// The file a glTF document at `path` names through `extras.clipLibrary`, or
/// `None` when it names none or the value is malformed, which warns.
pub fn clip_library_path(document: &gltf::Document, path: &Path) -> Option<PathBuf> {
    read_clip_library_path(
        &document.as_json().extras,
        path.parent().unwrap_or_else(|| Path::new("")),
        &path.display().to_string(),
    )
}

/// The library the skinned model at `path` binds, read from its document alone:
/// no buffer and no library is opened, so a tool can check the reference without
/// loading the model. `None` for a static or mesh-less document, one naming no
/// library, and a malformed value, none of which a load binds.
pub fn skinned_clip_library_path(path: &Path) -> Result<Option<PathBuf>, ModelLoadError> {
    let path_str = path.display().to_string();
    let document = gltf::Gltf::open(path)
        .map_err(|source| ModelLoadError::Import {
            path: path_str.clone(),
            source,
        })?
        .document;
    let skinned = select_model(&document, &path_str).is_ok_and(|model| model.skin.is_some());
    Ok(skinned
        .then(|| clip_library_path(&document, path))
        .flatten())
}

/// [`load_model`] over sources [`read_model_sources`] already read from
/// `path`: reads no file. `path` names the model in errors and warnings.
pub fn load_model_from_sources(
    path: &Path,
    sources: ModelSources,
) -> Result<LoadedModel, ModelLoadError> {
    let path_str = path.display().to_string();
    let ModelSources {
        document,
        buffers,
        material_keys,
        library,
        ..
    } = sources;

    // The `utils` accessor readers each take a buffer-data closure that indexes
    // the imported buffer blobs (the external `.bin` resolved by
    // `import_buffers`); each helper builds its own closure locally so the
    // closure and the borrowed glTF entity share one lifetime.

    // Load exactly one mesh: prefer the first node that binds both mesh and a
    // non-empty skin so geometry and skeleton are selected as a pair. A
    // document with no usable skinned mesh node falls back to the first mesh as
    // a static model.
    let SelectedModel {
        mesh,
        mesh_node,
        skin,
    } = select_model(&document, &path_str)?;
    let mount = mesh_node
        .as_ref()
        .and_then(|node| read_mount_axes(node.extras()));

    // --- Skeleton ---------------------------------------------------------
    // Use the selected skinned node's skin. A static/no-skin model still
    // receives one identity joint so rigid vertices bound to joint 0 have a
    // matching palette entry; any authored static node hit zone maps to that
    // identity joint.
    //
    // `skin_joint_to_topo` reindexes mesh `JOINTS_0` (skin-joint indices) into
    // topo order; `node_to_topo` reindexes animation channel targets (node
    // indices) into the same topo order. `joint_names` is that order's names,
    // the only link from a joint to its clip library counterpart.
    let (
        skeleton,
        joint_zones,
        pose_masks,
        aim_bend_weights,
        legs,
        sockets,
        skin_joint_to_topo,
        node_to_topo,
        locomotion_root,
        joint_names,
    ) = match skin.as_ref() {
        Some(skin) => build_skeleton(skin, &document, &buffers, &path_str)?,
        None => (
            identity_skeleton(),
            static_identity_joint_zones(&document, mesh.index()),
            PoseMaskSet::default(),
            vec![1.0],
            Vec::new(),
            build_rigid_sockets(&document, mesh_node.as_ref(), &path_str),
            HashMap::new(),
            HashMap::new(),
            None,
            Vec::new(),
        ),
    };
    let pose_stack =
        build_pose_modifier_stack(&skeleton, pose_masks, &aim_bend_weights, &legs, &path_str);

    // --- Mesh -------------------------------------------------------------
    let (mut skinned_mesh, submeshes) = load_mesh(
        &mesh,
        &buffers,
        &skin_joint_to_topo,
        &material_keys,
        &path_str,
    )?;

    // --- Animation clips --------------------------------------------------
    // Bind, then derivation, then bounds: a bound clip's root motion and reach
    // are its bound values, and embedded clips never pass through the bind.
    let mut clips: Vec<AnimationClip> = document
        .animations()
        .map(|anim| {
            parse_clip(
                &anim,
                &buffers,
                &node_to_topo,
                skeleton.joints.len(),
                &path_str,
            )
            .derive_root_motion(locomotion_root, &path_str)
        })
        .collect();
    if let Some(library) = &library {
        append_library_clips(
            &mut clips,
            library,
            MeshRig {
                skeleton: &skeleton,
                joint_names: &joint_names,
                locomotion_root,
            },
            &path_str,
        )?;
    }
    skinned_mesh.compute_conservative_animation_bounds(&skeleton, &clips, &pose_stack);

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
        sockets,
        mount,
        tags,
        joint_zones,
        pose_masks,
        pose_stack,
        legs,
    })
}

/// Build the topologically-sorted skeleton and the skin-joint-index → topo-index
/// remap. glTF `JOINTS_0` attributes and animation targets reference joints by
/// their **skin-joint index** (position in `skin.joints()`); the renderer wants
/// joints parent-before-child, so we re-order and carry the remap.
type SkeletonMaps = (
    Skeleton,
    Vec<Option<JointZone>>,
    PoseMaskSet,
    Vec<f32>,
    Vec<LegChain>,
    HashMap<String, SocketBinding>,
    HashMap<usize, usize>,
    HashMap<usize, usize>,
    Option<usize>,
    Vec<Option<String>>,
);

fn build_skeleton(
    skin: &gltf::Skin,
    document: &gltf::Document,
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

    // Pose metadata follows the same skin-order capture and topo remap as hit
    // zones. Keeping the weight parallel here prevents authored child-first
    // skin arrays from silently attaching falloff to the wrong joint.
    let rest_pose_metadata: Vec<JointPoseMetadata> = skin
        .joints()
        .map(|node| read_pose_masks(node.extras(), node.index(), path_str))
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
            if accessor.count() != joint_nodes.len() {
                return Err(ModelLoadError::InverseBindCountMismatch {
                    path: path_str.to_string(),
                    expected: joint_nodes.len(),
                    actual: accessor.count(),
                });
            }
            validate_readable_accessor(
                &accessor,
                buffers,
                path_str,
                "inverseBindMatrices",
                &[(DataType::F32, Dimensions::Mat4)],
            )?;
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
    validate_finite_mat4_stream(path_str, "inverseBindMatrices", &inverse_binds)?;

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

    let sockets =
        build_skinned_sockets(document, &node_to_skin_joint, &skin_joint_to_topo, path_str);

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
    let (pose_masks, aim_bend_weights, legs) =
        pose_masks_from_topo_metadata(&topo_order, &rest_pose_metadata, path_str);

    // `Joint` stays nameless: names serve only a bind, here at load.
    let skin_joint_names: Vec<Option<&str>> = skin.joints().map(|node| node.name()).collect();
    let joint_names: Vec<Option<String>> = topo_order
        .iter()
        .map(|&skin_idx| skin_joint_names[skin_idx].map(str::to_string))
        .collect();

    let skeleton = Skeleton::new(joints).map_err(|error| skeleton_build_error(path_str, error))?;
    let legs = validate_leg_set(&skeleton, legs, path_str);
    let declared_locomotion_root = skin
        .skeleton()
        .and_then(|node| node_to_topo.get(&node.index()).copied());
    let locomotion_root = select_locomotion_root(&skeleton, declared_locomotion_root, path_str);

    Ok((
        skeleton,
        joint_zones,
        pose_masks,
        aim_bend_weights,
        legs,
        sockets,
        skin_joint_to_topo,
        node_to_topo,
        locomotion_root,
        joint_names,
    ))
}

/// Resolve sockets authored on a skinned model. Scanning document nodes (rather
/// than only `skin.joints()`) both gives duplicate names one deterministic
/// document-order winner and diagnoses tags accidentally placed on non-joints.
fn build_skinned_sockets(
    document: &gltf::Document,
    node_to_skin_joint: &HashMap<usize, usize>,
    skin_joint_to_topo: &HashMap<usize, usize>,
    path_str: &str,
) -> HashMap<String, SocketBinding> {
    let mut sockets = HashMap::new();
    for node in document.nodes() {
        let node_index = node.index();
        let Some(name) = read_socket_name(node.extras(), node_index, path_str) else {
            continue;
        };
        let Some(&skin_joint) = node_to_skin_joint.get(&node_index) else {
            log::warn!(
                "[Model] socket '{name}' on non-joint node {node_index} in {path_str}; ignoring"
            );
            continue;
        };
        let topo_joint = skin_joint_to_topo[&skin_joint];
        insert_socket(
            &mut sockets,
            name,
            SocketBinding::SkinnedJoint(topo_joint),
            node_index,
            path_str,
        );
    }
    sockets
}

/// Resolve sockets authored on a rigid model. Raw mesh vertices already live in
/// the selected mesh node's local frame, so the mesh node itself contributes no
/// transform; only the descendant chain to the socket is composed.
fn build_rigid_sockets(
    document: &gltf::Document,
    mesh_node: Option<&gltf::Node>,
    path_str: &str,
) -> HashMap<String, SocketBinding> {
    let mut parents = HashMap::new();
    for node in document.nodes() {
        for child in node.children() {
            parents.insert(child.index(), node.index());
        }
    }

    let mut sockets = HashMap::new();
    for node in document.nodes() {
        let node_index = node.index();
        let Some(name) = read_socket_name(node.extras(), node_index, path_str) else {
            continue;
        };
        let Some(rest_transform) = mesh_local_rest_transform(
            document,
            mesh_node.map(gltf::Node::index),
            node_index,
            &parents,
        ) else {
            log::warn!(
                "[Model] rigid socket '{name}' on node {node_index} in {path_str} has no valid acyclic path from the selected mesh node; ignoring"
            );
            continue;
        };
        insert_socket(
            &mut sockets,
            name,
            SocketBinding::RigidRest(rest_transform),
            node_index,
            path_str,
        );
    }
    sockets
}

fn mesh_local_rest_transform(
    document: &gltf::Document,
    mesh_node: Option<usize>,
    socket_node: usize,
    parents: &HashMap<usize, usize>,
) -> Option<Mat4> {
    let mesh_node = mesh_node?;
    let mut chain = Vec::new();
    let mut visited = vec![false; document.nodes().len()];
    let mut current = socket_node;
    while current != mesh_node {
        let seen = visited.get_mut(current)?;
        if *seen {
            return None;
        }
        *seen = true;
        chain.push(current);
        current = *parents.get(&current)?;
    }

    let mut transform = Mat4::IDENTITY;
    for node_index in chain.into_iter().rev() {
        let node = document.nodes().nth(node_index)?;
        transform *= Mat4::from_cols_array_2d(&node.transform().matrix());
    }
    Some(transform)
}

fn insert_socket(
    sockets: &mut HashMap<String, SocketBinding>,
    name: String,
    binding: SocketBinding,
    node_index: usize,
    path_str: &str,
) {
    if sockets.contains_key(&name) {
        log::warn!(
            "[Model] duplicate socket '{name}' on node {node_index} in {path_str}; keeping the first document-order binding"
        );
        return;
    }
    sockets.insert(name, binding);
}

/// Select the joint whose translation may carry locomotion root motion.
///
/// A glTF skin may explicitly name its skeleton root. When that node belongs to
/// the skin, it must also be a hierarchy root. Otherwise a single hierarchy
/// root is unambiguous. Forests without a valid declaration have no automatic
/// locomotion root: deriving from whichever root happened to sort first would
/// attach calibration to skin-array order rather than authored intent.
fn select_locomotion_root(
    skeleton: &Skeleton,
    declared_root: Option<usize>,
    path_str: &str,
) -> Option<usize> {
    let roots: Vec<usize> = skeleton
        .joints
        .iter()
        .enumerate()
        .filter_map(|(index, joint)| joint.parent.is_none().then_some(index))
        .collect();

    if let Some(declared_root) = declared_root {
        if roots.contains(&declared_root) {
            return Some(declared_root);
        }
        log::warn!(
            "[Model] skin skeleton joint {declared_root} is not a hierarchy root in {path_str}; \
             root-motion calibration disabled"
        );
        return None;
    }

    match roots.as_slice() {
        [root] => Some(*root),
        [] => {
            log::warn!(
                "[Model] skin has no hierarchy root in {path_str}; root-motion calibration disabled"
            );
            None
        }
        _ => {
            log::warn!(
                "[Model] skin has {} hierarchy roots in {path_str} and names no in-skin skeleton root; \
                 root-motion calibration disabled",
                roots.len()
            );
            None
        }
    }
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
    material_keys: &[String],
    path_str: &str,
) -> Result<(SkinnedMesh, Vec<Submesh>), ModelLoadError> {
    let buffer_data = |buffer: gltf::Buffer| buffers.get(buffer.index()).map(|d| &d.0[..]);
    let mut out = SkinnedMesh::default();
    let mut submeshes: Vec<Submesh> = Vec::new();

    for (primitive_index, primitive) in mesh.primitives().enumerate() {
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
        validate_readable_accessor(
            &position_accessor,
            buffers,
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
        validate_finite_vec3_stream(path_str, "POSITION", &positions)?;
        let vertex_count = positions.len();

        // Optional attributes default to neutral values when a primitive omits
        // them (a rigid mesh without skinning, or an untextured primitive).
        let uvs: Vec<[u16; 2]> = match primitive.get(&gltf::mesh::Semantic::TexCoords(0)) {
            Some(accessor) => {
                validate_readable_accessor(
                    &accessor,
                    buffers,
                    path_str,
                    "TEXCOORD_0",
                    &[
                        (DataType::U8, Dimensions::Vec2),
                        (DataType::U16, Dimensions::Vec2),
                        (DataType::F32, Dimensions::Vec2),
                    ],
                )?;
                let values: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .ok_or_else(|| ModelLoadError::MissingAttribute {
                        path: path_str.to_string(),
                        attribute: "TEXCOORD_0".to_string(),
                    })?
                    .into_f32()
                    .collect();
                validate_attribute_count(path_str, "TEXCOORD_0", vertex_count, values.len())?;
                validate_finite_vec2_stream(path_str, "TEXCOORD_0", &values)?;
                values.into_iter().map(quantize_uv).collect()
            }
            None => vec![[0, 0]; vertex_count],
        };
        let normals: Vec<[u16; 2]> = match primitive.get(&gltf::mesh::Semantic::Normals) {
            Some(accessor) => {
                validate_readable_accessor(
                    &accessor,
                    buffers,
                    path_str,
                    "NORMAL",
                    &[(DataType::F32, Dimensions::Vec3)],
                )?;
                let values: Vec<[f32; 3]> = reader
                    .read_normals()
                    .ok_or_else(|| ModelLoadError::MissingAttribute {
                        path: path_str.to_string(),
                        attribute: "NORMAL".to_string(),
                    })?
                    .collect();
                validate_attribute_count(path_str, "NORMAL", vertex_count, values.len())?;
                validate_finite_vec3_stream(path_str, "NORMAL", &values)?;
                validate_packable_vec3_directions(path_str, "NORMAL", &values)?;
                values
                    .into_iter()
                    .map(|nn| octahedral::encode(nn[0], nn[1], nn[2]))
                    .collect()
            }
            // Default to +Z (octahedral center) when normals are absent.
            None => vec![octahedral::encode(0.0, 0.0, 1.0); vertex_count],
        };
        let tangents: Vec<[u16; 2]> = match primitive.get(&gltf::mesh::Semantic::Tangents) {
            Some(accessor) => {
                validate_readable_accessor(
                    &accessor,
                    buffers,
                    path_str,
                    "TANGENT",
                    &[(DataType::F32, Dimensions::Vec4)],
                )?;
                let values: Vec<[f32; 4]> = reader
                    .read_tangents()
                    .ok_or_else(|| ModelLoadError::MissingAttribute {
                        path: path_str.to_string(),
                        attribute: "TANGENT".to_string(),
                    })?
                    .collect();
                validate_attribute_count(path_str, "TANGENT", vertex_count, values.len())?;
                validate_finite_vec4_stream(path_str, "TANGENT", &values)?;
                validate_packable_vec4_xyz_directions(path_str, "TANGENT", &values)?;
                values.into_iter().map(pack_tangent).collect()
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
            validate_readable_accessor(
                &accessor,
                buffers,
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
            validate_readable_accessor(
                &accessor,
                buffers,
                path_str,
                "WEIGHTS_0",
                &[
                    (DataType::U8, Dimensions::Vec4),
                    (DataType::U16, Dimensions::Vec4),
                    (DataType::F32, Dimensions::Vec4),
                ],
            )?;
            let values =
                reader
                    .read_weights(0)
                    .ok_or_else(|| ModelLoadError::MissingAttribute {
                        path: path_str.to_string(),
                        attribute: "WEIGHTS_0".to_string(),
                    })?;
            if accessor.data_type() == DataType::F32 {
                let raw: Vec<[f32; 4]> = values.clone().into_f32().collect();
                validate_attribute_count(path_str, "WEIGHTS_0", vertex_count, raw.len())?;
                validate_finite_vec4_stream(path_str, "WEIGHTS_0", &raw)?;
            }
            let values: Vec<[u8; 4]> = values.into_u8().map(normalize_weights).collect();
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
            validate_readable_accessor(
                &accessor,
                buffers,
                path_str,
                "indices",
                &[
                    (DataType::U8, Dimensions::Scalar),
                    (DataType::U16, Dimensions::Scalar),
                    (DataType::U32, Dimensions::Scalar),
                ],
            )?;
            validate_triangle_count(path_str, "indices", accessor.count())?;
        } else {
            validate_triangle_count(path_str, "POSITION", vertex_count)?;
        }
        if let Some(accessor) = primitive.indices() {
            match reader.read_indices() {
                Some(idx) => {
                    for i in idx.into_u32() {
                        validate_primitive_index(path_str, i, vertex_count)?;
                        out.indices.push(base_vertex + i);
                    }
                }
                None => {
                    return Err(ModelLoadError::MissingAttribute {
                        path: path_str.to_string(),
                        attribute: "indices".to_string(),
                    });
                }
            }
            debug_assert_eq!(
                out.indices.len() - start as usize,
                accessor.count(),
                "read_indices yielded a different count than the accessor advertised"
            );
        } else {
            for i in 0..vertex_count as u32 {
                out.indices.push(base_vertex + i);
            }
        }
        let end = out.indices.len() as u32;

        submeshes.push(Submesh {
            // Hashed from the base-color PNG when the sources were read.
            material_key: material_keys
                .get(primitive_index)
                .cloned()
                .unwrap_or_else(zero_material_key),
            indices: start..end,
        });
    }

    // Tight bind-pose bound over the merged vertex positions. `load_model`
    // replaces the renderer-facing bound with the conservative pose envelope
    // after clips and pose modifiers are available.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::Interp;
    use std::path::PathBuf;

    #[test]
    fn locomotion_root_selection_uses_declared_root_in_a_forest() {
        let skeleton = Skeleton::new(vec![
            Joint {
                parent: None,
                inverse_bind: Mat4::IDENTITY.to_cols_array_2d(),
                rest_local: RestLocal::default(),
            },
            Joint {
                parent: Some(0),
                inverse_bind: Mat4::IDENTITY.to_cols_array_2d(),
                rest_local: RestLocal::default(),
            },
            Joint {
                parent: None,
                inverse_bind: Mat4::IDENTITY.to_cols_array_2d(),
                rest_local: RestLocal::default(),
            },
        ])
        .unwrap();

        assert_eq!(
            select_locomotion_root(&skeleton, Some(2), "fixture"),
            Some(2)
        );
        assert_eq!(
            select_locomotion_root(&skeleton, None, "fixture"),
            None,
            "an undeclared forest is ambiguous rather than defaulting to topo joint 0"
        );
    }

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
    fn required_float_stream_validation_rejects_nan_and_infinity() {
        let err = validate_finite_vec3_stream(
            "model.gltf",
            "POSITION",
            &[[0.0, 1.0, 2.0], [f32::NAN, 0.0, 0.0]],
        )
        .expect_err("NaN POSITION is malformed required geometry");
        assert!(
            matches!(
                &err,
                ModelLoadError::NonFiniteAccessorValue {
                    path,
                    attribute,
                    index: 1,
                } if path == "model.gltf" && attribute == "POSITION"
            ),
            "got {err:?}",
        );

        let err = validate_finite_mat4_stream(
            "model.gltf",
            "inverseBindMatrices",
            &[[[f32::INFINITY, 0.0, 0.0, 0.0]; 4]],
        )
        .expect_err("infinite inverse bind matrix is malformed required skin data");
        assert!(
            matches!(
                &err,
                ModelLoadError::NonFiniteAccessorValue {
                    path,
                    attribute,
                    index: 0,
                } if path == "model.gltf" && attribute == "inverseBindMatrices"
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn packable_direction_validation_rejects_zero_and_near_zero_vectors() {
        let err = validate_packable_vec3_directions(
            "model.gltf",
            "NORMAL",
            &[[0.0, 0.0, 1.0], [0.0, 0.0, 0.0]],
        )
        .expect_err("zero NORMAL cannot be octahedrally packed");
        assert!(
            matches!(
                &err,
                ModelLoadError::DegenerateDirectionVector {
                    path,
                    attribute,
                    index: 1,
                } if path == "model.gltf" && attribute == "NORMAL"
            ),
            "got {err:?}",
        );

        let err = validate_packable_vec4_xyz_directions(
            "model.gltf",
            "TANGENT",
            &[[1.0, 0.0, 0.0, 1.0], [1.0e-7, 0.0, 0.0, -1.0]],
        )
        .expect_err("near-zero TANGENT direction cannot be octahedrally packed");
        assert!(
            matches!(
                &err,
                ModelLoadError::DegenerateDirectionVector {
                    path,
                    attribute,
                    index: 1,
                } if path == "model.gltf" && attribute == "TANGENT"
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn validate_triangle_count_requires_complete_triangle_lists() {
        assert!(
            validate_triangle_count("model.gltf", "indices", 6).is_ok(),
            "two complete triangles are accepted"
        );

        let err = validate_triangle_count("model.gltf", "indices", 5)
            .expect_err("partial triangle lists are malformed");
        assert!(
            matches!(
                &err,
                ModelLoadError::IncompleteTriangleList {
                    path,
                    attribute,
                    count: 5,
                } if path == "model.gltf" && attribute == "indices"
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn validate_skinning_attribute_pair_rejects_half_authored_skinning() {
        assert!(
            !validate_skinning_attribute_presence(false, false, "model.gltf")
                .expect("absence of both attributes is rigid"),
            "absence of both JOINTS_0 and WEIGHTS_0 is the rigid path",
        );
        assert!(
            validate_skinning_attribute_presence(true, true, "model.gltf")
                .expect("both attributes present is skinned"),
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

    // `multi_clip` accessors, each read at a different site.
    const POSITION: usize = 0;
    const JOINTS: usize = 1;
    const WEIGHTS: usize = 2;
    const INDICES: usize = 3;
    const INVERSE_BINDS: usize = 4;

    /// Give `attribute` an accessor and a buffer view of its own in the fixture,
    /// so a stride patch reaches no other read. Returns the accessor's index.
    /// The optional streams borrow the WEIGHTS_0 bytes; a malformed one fails
    /// before they are read.
    fn isolate_accessor(json: &mut serde_json::Value, attribute: &str) -> usize {
        let accessor_index = match attribute {
            "POSITION" => POSITION,
            "JOINTS_0" => JOINTS,
            "WEIGHTS_0" => WEIGHTS,
            "indices" => INDICES,
            "inverseBindMatrices" => INVERSE_BINDS,
            optional => {
                let accessor_type = match optional {
                    "TEXCOORD_0" => "VEC2",
                    "NORMAL" => "VEC3",
                    _ => "VEC4",
                };
                let accessors = json["accessors"].as_array_mut().unwrap();
                accessors.push(serde_json::json!({
                    "bufferView": 2,
                    "componentType": 5126,
                    "count": 3,
                    "type": accessor_type,
                }));
                let index = accessors.len() - 1;
                json["meshes"][0]["primitives"][0]["attributes"][optional] =
                    serde_json::json!(index);
                index
            }
        };
        let view_index = json["accessors"][accessor_index]["bufferView"]
            .as_u64()
            .unwrap() as usize;
        let view = json["bufferViews"][view_index].clone();
        let views = json["bufferViews"].as_array_mut().unwrap();
        views.push(view);
        let private_view = views.len() - 1;
        json["accessors"][accessor_index]["bufferView"] = serde_json::json!(private_view);
        accessor_index
    }

    fn patch_fields(target: &mut serde_json::Value, fields: &serde_json::Value) {
        for (key, value) in fields.as_object().unwrap() {
            target[key.as_str()] = value.clone();
        }
    }

    // Regression: gltf trusts stride, count and sparse shape. A short stride
    // panicked in release, a zero count overflowed in dev, and a sparse-only
    // accessor with a huge count aborted on allocation.
    #[test]
    fn unreadable_mesh_and_skin_accessors_return_err_not_panic() {
        const ATTRIBUTES: &[&str] = &[
            "POSITION",
            "TEXCOORD_0",
            "NORMAL",
            "TANGENT",
            "JOINTS_0",
            "WEIGHTS_0",
            "indices",
            "inverseBindMatrices",
        ];
        const SHORT_STRIDE: &str = "has a byte stride below its element size";
        const OVERRUN: &str = "overruns its buffer";
        let sparse = |count: u64| {
            serde_json::json!({
                "count": count,
                "indices": { "bufferView": 3, "componentType": 5123 },
                "values": { "bufferView": 0 },
            })
        };
        let none = serde_json::json!({});

        for attribute in ATTRIBUTES {
            // A count the skin's joints disagree with is refused earlier, as a
            // count mismatch.
            let count_reason = |reason| (*attribute != "inverseBindMatrices").then_some(reason);
            let mut cases = vec![
                (
                    count_reason("has no elements"),
                    serde_json::json!({ "count": 0 }),
                    none.clone(),
                ),
                (
                    count_reason("has no buffer view"),
                    serde_json::json!({
                        "bufferView": null,
                        "count": 1_000_000_000_000u64,
                        "sparse": sparse(1),
                    }),
                    none.clone(),
                ),
                (
                    count_reason(OVERRUN),
                    serde_json::json!({ "count": u64::MAX }),
                    none.clone(),
                ),
                (
                    Some("has a sparse count outside its elements"),
                    serde_json::json!({ "sparse": sparse(4) }),
                    none.clone(),
                ),
                (
                    Some(OVERRUN),
                    serde_json::json!({ "byteOffset": 1u64 << 20 }),
                    none.clone(),
                ),
                (
                    Some(OVERRUN),
                    none.clone(),
                    serde_json::json!({ "byteLength": 1u64 << 40 }),
                ),
                (
                    Some(OVERRUN),
                    none.clone(),
                    serde_json::json!({ "byteOffset": u64::MAX }),
                ),
            ];
            // A document may declare no stride under 4, so a 2-byte index cannot
            // be short-strided, and JOINTS_0 needs its wider storage.
            match *attribute {
                "indices" => {}
                "JOINTS_0" => cases.push((
                    Some(SHORT_STRIDE),
                    serde_json::json!({ "componentType": 5123 }),
                    serde_json::json!({ "byteStride": 4 }),
                )),
                _ => cases.push((
                    Some(SHORT_STRIDE),
                    none.clone(),
                    serde_json::json!({ "byteStride": 4 }),
                )),
            }

            for (reason, accessor_patch, view_patch) in cases {
                let label = format!("{attribute} {accessor_patch} {view_patch}");
                let mut json = fixture_json(&multi_clip_fixture_path());
                let accessor_index = isolate_accessor(&mut json, attribute);
                let view_index = json["accessors"][accessor_index]["bufferView"]
                    .as_u64()
                    .unwrap() as usize;
                patch_fields(&mut json["accessors"][accessor_index], &accessor_patch);
                patch_fields(&mut json["bufferViews"][view_index], &view_patch);
                let path = write_temp_fixture("unreadable_accessor", &json);

                let result = std::panic::catch_unwind(|| load_model(&path));
                let _ = std::fs::remove_file(&path);
                let load_result = result.unwrap_or_else(|_| panic!("{label}: load panicked"));

                match (reason, &load_result) {
                    (
                        Some(expected),
                        Err(ModelLoadError::UnreadableAccessor {
                            attribute: actual_attribute,
                            reason: actual,
                            ..
                        }),
                    ) if actual_attribute == attribute && *actual == expected => {}
                    (None, Err(ModelLoadError::InverseBindCountMismatch { .. })) => {}
                    _ => panic!("{label}: expected {reason:?}, got {load_result:?}"),
                }
            }
        }
    }

    #[test]
    fn sparse_mesh_accessor_with_a_buffer_view_still_loads() {
        let mut json = fixture_json(&multi_clip_fixture_path());
        json["accessors"][POSITION]["sparse"] = serde_json::json!({
            "count": 1,
            "indices": { "bufferView": 3, "componentType": 5123 },
            "values": { "bufferView": 0 },
        });
        let path = write_temp_fixture("sparse_position", &json);

        let result = load_model(&path);
        let _ = std::fs::remove_file(&path);

        let model = result.expect("sparse POSITION loads");
        assert_eq!(model.mesh.vertices.len(), 3);
    }

    fn add_first_primitive_optional_stream(
        json: &mut serde_json::Value,
        semantic: &str,
        accessor_type: &str,
        byte_length: usize,
        byte_length_total: usize,
        data_uri: &str,
    ) {
        let buffer_view_index = json["bufferViews"]
            .as_array()
            .expect("bufferViews is an array")
            .len();
        json["bufferViews"]
            .as_array_mut()
            .expect("bufferViews is an array")
            .push(serde_json::json!({
                "buffer": 0,
                "byteOffset": 88,
                "byteLength": byte_length,
            }));
        let accessor_index = json["accessors"]
            .as_array()
            .expect("accessors is an array")
            .len();
        json["accessors"]
            .as_array_mut()
            .expect("accessors is an array")
            .push(serde_json::json!({
                "bufferView": buffer_view_index,
                "componentType": 5126,
                "count": 3,
                "type": accessor_type,
            }));
        json["meshes"][0]["primitives"][0]["attributes"][semantic] =
            serde_json::json!(accessor_index);
        json["buffers"][0]["byteLength"] = serde_json::json!(byte_length_total);
        json["buffers"][0]["uri"] = serde_json::json!(data_uri);
    }

    #[test]
    fn non_finite_present_optional_vertex_streams_return_err() {
        // Regression: present optional float streams with NaN/Inf were packed or
        // quantized into renderer-visible vertex data. Missing streams still use
        // defaults; malformed present streams are rejected.
        for (name, semantic, accessor_type, byte_length, byte_length_total, data_uri) in [
            (
                "nan_texcoord",
                "TEXCOORD_0",
                "VEC2",
                24,
                112,
                "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AACAPwAAAAAAAIA/AAAAAAAAgD8AAIA/AAABAAIAAAAAAAEAAgAAAAAAwH8AAAAAAAAAAAAAAAAAAAAAAAAAAA==",
            ),
            (
                "nan_normal",
                "NORMAL",
                "VEC3",
                36,
                124,
                "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AACAPwAAAAAAAIA/AAAAAAAAgD8AAIA/AAABAAIAAAAAAAEAAgAAAAAAwH8AAAAAAACAPwAAAAAAAAAAAACAPwAAAAAAAAAAAACAPw==",
            ),
            (
                "nan_tangent",
                "TANGENT",
                "VEC4",
                48,
                136,
                "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AACAPwAAAAAAAIA/AAAAAAAAgD8AAIA/AAABAAIAAAAAAAEAAgAAAAAAwH8AAAAAAAAAAAAAgD8AAIA/AAAAAAAAAAAAAIA/AACAPwAAAAAAAAAAAACAPw==",
            ),
        ] {
            let mut json = fixture_json(&multi_primitive_fixture_path());
            add_first_primitive_optional_stream(
                &mut json,
                semantic,
                accessor_type,
                byte_length,
                byte_length_total,
                data_uri,
            );
            let path = write_temp_fixture(name, &json);

            let result = std::panic::catch_unwind(|| load_model(&path));
            let _ = std::fs::remove_file(&path);
            let load_result = result.expect("non-finite optional stream must not panic");

            assert!(
                matches!(
                    &load_result,
                    Err(ModelLoadError::NonFiniteAccessorValue {
                        attribute,
                        index: 0,
                        ..
                    }) if attribute == semantic
                ),
                "non-finite {semantic} should be rejected, got {load_result:?}",
            );
        }
    }

    #[test]
    fn zero_normal_and_tangent_streams_return_err_before_packing() {
        // Regression: finite zero direction vectors reached octahedral packing,
        // producing NaN intermediates in renderer-visible packed vertex data.
        for (name, semantic, accessor_type, byte_length, byte_length_total, data_uri) in [
            (
                "zero_normal",
                "NORMAL",
                "VEC3",
                36,
                124,
                "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AACAPwAAAAAAAIA/AAAAAAAAgD8AAIA/AAABAAIAAAAAAAEAAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
            ),
            (
                "zero_tangent",
                "TANGENT",
                "VEC4",
                48,
                136,
                "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AACAPwAAAAAAAIA/AAAAAAAAgD8AAIA/AAABAAIAAAAAAAEAAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
            ),
        ] {
            let mut json = fixture_json(&multi_primitive_fixture_path());
            add_first_primitive_optional_stream(
                &mut json,
                semantic,
                accessor_type,
                byte_length,
                byte_length_total,
                data_uri,
            );
            let path = write_temp_fixture(name, &json);

            let result = std::panic::catch_unwind(|| load_model(&path));
            let _ = std::fs::remove_file(&path);
            let load_result = result.expect("degenerate direction stream must not panic");

            assert!(
                matches!(
                    &load_result,
                    Err(ModelLoadError::DegenerateDirectionVector {
                        attribute,
                        index: 0,
                        ..
                    }) if attribute == semantic
                ),
                "degenerate {semantic} should be rejected, got {load_result:?}",
            );
        }
    }

    #[test]
    fn non_finite_f32_weights_return_err() {
        // Regression: NaN F32 WEIGHTS_0 values were quantized before validation,
        // producing renderer-visible skinning data from malformed input.
        let mut json = fixture_json(&multi_clip_fixture_path());
        json["buffers"][0]["uri"] = serde_json::json!(
            "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAAAAAADAfwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAAAAAAABAAIAAAAAAIA/AAAAAAAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAAAAAAIA/AACAPwAAAAAAAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAAAAAACAPwAAAAAAAIA/AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAA8wQ1P/MENT8AAAAAAACAPwAAAEAAAAAAAAAAAAAAAAAAACBBAAAAAAAAAAAAAKBBAAAAAAAAAAAAAAAAAACAPwAAgL8AAIC/AACAvwAAoEAAAKBAAACgQAAAEEEAABBBAAAQQQAAAMAAAADAAAAAwAAA4EAAAOBAAADgQAAAAEEAAABBAAAAQQ=="
        );
        let path = write_temp_fixture("nan_weights", &json);

        let result = std::panic::catch_unwind(|| load_model(&path));
        let _ = std::fs::remove_file(&path);
        let load_result = result.expect("non-finite WEIGHTS_0 must not panic");

        assert!(
            matches!(
                &load_result,
                Err(ModelLoadError::NonFiniteAccessorValue {
                    attribute,
                    index: 0,
                    ..
                }) if attribute == "WEIGHTS_0"
            ),
            "non-finite WEIGHTS_0 should be rejected, got {load_result:?}",
        );
    }

    #[test]
    fn non_finite_position_returns_err_not_poisoned_bounds() {
        // Regression: NaN/Inf POSITION values reached SkinnedMesh::compute_bounds,
        // poisoning local bounds instead of failing as malformed required data.
        let mut json = fixture_json(&multi_primitive_fixture_path());
        json["buffers"][0]["uri"] = serde_json::json!(
            "data:application/octet-stream;base64,AADAfwAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AACAPwAAAAAAAIA/AAAAAAAAgD8AAIA/AAABAAIAAAAAAAEAAgAAAA=="
        );
        let path = write_temp_fixture("nan_position", &json);

        let result = std::panic::catch_unwind(|| load_model(&path));
        let _ = std::fs::remove_file(&path);
        let load_result = result.expect("non-finite POSITION must not panic");

        assert!(
            matches!(
                &load_result,
                Err(ModelLoadError::NonFiniteAccessorValue {
                    attribute,
                    index: 0,
                    ..
                }) if attribute == "POSITION"
            ),
            "non-finite POSITION should be rejected, got {load_result:?}",
        );
    }

    #[test]
    fn non_finite_inverse_bind_matrix_returns_err() {
        // Regression: Inf/NaN inverse bind matrices were stored in the skeleton,
        // contaminating downstream bone palettes.
        let mut json = fixture_json(&multi_clip_fixture_path());
        json["buffers"][0]["uri"] = serde_json::json!(
            "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAAAAAAABAAIAAAAAAIB/AAAAAAAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAAAAAAIA/AACAPwAAAAAAAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAAAAAACAPwAAAAAAAIA/AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAA8wQ1P/MENT8AAAAAAACAPwAAAEAAAAAAAAAAAAAAAAAAACBBAAAAAAAAAAAAAKBBAAAAAAAAAAAAAAAAAACAPwAAgL8AAIC/AACAvwAAoEAAAKBAAACgQAAAEEEAABBBAAAQQQAAAMAAAADAAAAAwAAA4EAAAOBAAADgQAAAAEEAAABBAAAAQQ=="
        );
        let path = write_temp_fixture("inf_inverse_bind", &json);

        let result = std::panic::catch_unwind(|| load_model(&path));
        let _ = std::fs::remove_file(&path);
        let load_result = result.expect("non-finite inverseBindMatrices must not panic");

        assert!(
            matches!(
                &load_result,
                Err(ModelLoadError::NonFiniteAccessorValue {
                    attribute,
                    index: 0,
                    ..
                }) if attribute == "inverseBindMatrices"
            ),
            "non-finite inverseBindMatrices should be rejected, got {load_result:?}",
        );
    }

    #[test]
    fn indexed_triangle_list_with_partial_triangle_returns_err() {
        // Regression: triangle-list primitives accepted dangling index tails,
        // leaving the renderer with a non-triangle draw range.
        let mut json = fixture_json(&multi_primitive_fixture_path());
        json["accessors"][2]["count"] = serde_json::json!(2);
        let path = write_temp_fixture("partial_indexed_triangles", &json);

        let result = std::panic::catch_unwind(|| load_model(&path));
        let _ = std::fs::remove_file(&path);
        let load_result = result.expect("partial triangle list must not panic");

        assert!(
            matches!(
                &load_result,
                Err(ModelLoadError::IncompleteTriangleList {
                    attribute,
                    count: 2,
                    ..
                }) if attribute == "indices"
            ),
            "partial indexed triangle list should be rejected, got {load_result:?}",
        );
    }

    #[test]
    fn present_malformed_indices_return_err_without_generated_topology() {
        // Regression: present malformed indices silently loaded as generated
        // geometry instead of rejecting the authored indexed primitive.
        let mut json = fixture_json(&multi_primitive_fixture_path());
        json["bufferViews"]
            .as_array_mut()
            .expect("bufferViews is an array")
            .push(serde_json::json!({
                "buffer": 0,
                "byteOffset": 88,
                "byteLength": 6,
            }));
        json["accessors"][2]["bufferView"] = serde_json::json!(4);
        json["buffers"][0]["byteLength"] = serde_json::json!(94);
        json["buffers"][0]["uri"] = serde_json::json!(
            "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AACAPwAAAAAAAIA/AAAAAAAAgD8AAIA/AAABAAIAAAAAAAEAAgAAAAAAAQAJAA=="
        );
        let path = write_temp_fixture("malformed_indices", &json);

        let result = std::panic::catch_unwind(|| load_model(&path));
        let _ = std::fs::remove_file(&path);
        let load_result = result.expect("malformed indices must not panic");

        assert!(
            matches!(
                &load_result,
                Err(ModelLoadError::IndexOutOfRange {
                    index: 9,
                    vertex_count: 3,
                    ..
                })
            ),
            "present malformed indices should be rejected, got {load_result:?}",
        );
    }

    #[test]
    fn generated_triangle_list_with_partial_triangle_returns_err() {
        // Regression: non-indexed triangle-list primitives generated dangling
        // index tails when POSITION count was not a multiple of three.
        let mut json = fixture_json(&multi_primitive_fixture_path());
        json["meshes"][0]["primitives"][0]
            .as_object_mut()
            .expect("primitive is an object")
            .remove("indices");
        json["accessors"][0]["count"] = serde_json::json!(2);
        let path = write_temp_fixture("partial_generated_triangles", &json);

        let result = std::panic::catch_unwind(|| load_model(&path));
        let _ = std::fs::remove_file(&path);
        let load_result = result.expect("partial generated triangle list must not panic");

        assert!(
            matches!(
                &load_result,
                Err(ModelLoadError::IncompleteTriangleList {
                    attribute,
                    count: 2,
                    ..
                }) if attribute == "POSITION"
            ),
            "partial generated triangle list should be rejected, got {load_result:?}",
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
    fn empty_skin_does_not_shadow_later_valid_skinned_node() {
        // Regression: a mesh node bound to an empty skin won model selection,
        // causing a later valid skinned mesh node to load as static/no-skin.
        let mut json = fixture_json(&multi_clip_fixture_path());
        json["skins"]
            .as_array_mut()
            .expect("fixture has skins")
            .insert(0, serde_json::json!({ "joints": [] }));
        json["nodes"][0]["skin"] = serde_json::json!(0);
        json["nodes"]
            .as_array_mut()
            .expect("fixture has nodes")
            .push(serde_json::json!({
                "name": "later_valid_mesh_node",
                "mesh": 0,
                "skin": 1
            }));
        let path = write_temp_fixture("empty_skin_before_valid_skin", &json);

        let selection = std::panic::catch_unwind(|| {
            let document = gltf::Gltf::open(&path)
                .expect("mutated fixture opens")
                .document;
            let selected = select_model(&document, &path.display().to_string())
                .expect("mutated fixture has a mesh");
            selected.skin.as_ref().map(|skin| skin.index())
        });
        let _ = std::fs::remove_file(&path);
        let skin_index = selection.expect("selecting the mutated fixture must not panic");

        assert_eq!(
            skin_index,
            Some(1),
            "selection skips empty skin 0 and uses the later non-empty skin",
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

    #[test]
    fn mesh_node_mount_extras_populate_loaded_model_mount() {
        let mut json = fixture_json(&multi_primitive_fixture_path());
        json["nodes"][0]["extras"]["mount"] = serde_json::json!({
            "barrel": [0.0, 2.0, 0.0],
            "up": [0.0, 0.0, 3.0],
            "euler": [10.0, 20.0, 30.0],
        });
        let path = write_temp_fixture("mesh_mount_extras", &json);
        let model = load_model(&path).expect("mount extras are optional metadata");
        let _ = std::fs::remove_file(path);

        assert_eq!(
            model.mount,
            Some(MountAxes {
                barrel: Vec3::Y,
                up: Vec3::Z,
                euler: Some([10.0, 20.0, 30.0]),
            }),
            "the selected mesh node carries the public mount declaration",
        );
    }

    #[test]
    fn malformed_mesh_mount_extras_do_not_fail_model_load() {
        let mut json = fixture_json(&multi_primitive_fixture_path());
        json["nodes"][0]["extras"]["mount"] = serde_json::json!({
            "barrel": [0.0, 1.0, 0.0],
            "up": "not-an-axis",
        });
        let path = write_temp_fixture("malformed_mesh_mount_extras", &json);
        let model = load_model(&path).expect("malformed metadata never rejects a model");
        let _ = std::fs::remove_file(path);

        assert_eq!(model.mount, None);
    }

    #[test]
    fn overflowed_mesh_mount_axes_degrade_loaded_model_mount_to_none() {
        // Regression: huge finite axes overflowed length-squared and surfaced
        // zero directions through LoadedModel.mount.
        let mut json = fixture_json(&multi_primitive_fixture_path());
        json["nodes"][0]["extras"]["mount"] = serde_json::json!({
            "barrel": [3.0e38, 3.0e38, 0.0],
            "up": [0.0, 0.0, 1.0],
            "euler": [0.0, 0.0, 0.0],
        });
        let path = write_temp_fixture("overflowed_mesh_mount_axes", &json);
        let model = load_model(&path).expect("invalid mount metadata never rejects a model");
        let _ = std::fs::remove_file(path);

        assert_eq!(model.mount, None);
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
    fn malformed_socket_extras_warn_and_degrade_to_none() {
        for raw in [
            r#"{ "socket": 42 }"#,
            r#"{ "socket": ["not", "a", "name"] }"#,
            r#"["not", "an", "object"]"#,
        ] {
            let extras = extras_from_raw(raw);
            assert!(
                read_socket_name(&extras, 7, "model.gltf").is_none(),
                "malformed socket extras {raw} must not produce a socket",
            );
        }
    }

    // --- Per-node extras -> pose masks and stack --------------------------

    #[test]
    fn read_pose_masks_accepts_string_array_and_multi_membership() {
        let array = extras_from_raw(
            r#"{ "poseMask": ["aimSpine", "upperBody", "lowerBody"], "aimBendWeight": 2.5 }"#,
        );
        let metadata = read_pose_masks(&array, 7, "model.gltf");
        assert!(metadata.aim_spine);
        assert!(metadata.upper_body);
        assert!(metadata.lower_body);
        assert_eq!(metadata.aim_bend_weight, 2.5);

        let string = extras_from_raw(r#"{ "poseMask": "upperBody" }"#);
        let metadata = read_pose_masks(&string, 8, "model.gltf");
        assert!(!metadata.aim_spine);
        assert!(metadata.upper_body);
        assert!(!metadata.lower_body);
    }

    #[test]
    fn read_pose_masks_defaults_invalid_aim_weight_and_ignores_bad_values() {
        for raw in [
            r#"{ "poseMask": "aimSpine" }"#,
            r#"{ "poseMask": "aimSpine", "aimBendWeight": 0.0 }"#,
            r#"{ "poseMask": "aimSpine", "aimBendWeight": -2.0 }"#,
            r#"{ "poseMask": "aimSpine", "aimBendWeight": "heavy" }"#,
        ] {
            let metadata = read_pose_masks(&extras_from_raw(raw), 3, "model.gltf");
            assert!(metadata.aim_spine, "valid membership survives {raw}");
            assert_eq!(metadata.aim_bend_weight, 1.0, "default weight for {raw}");
        }

        for raw in [
            r#"{ "poseMask": "unknownMask" }"#,
            r#"{ "poseMask": 42 }"#,
            r#"{ "poseMask": [false, "unknownMask"] }"#,
        ] {
            let metadata = read_pose_masks(&extras_from_raw(raw), 4, "model.gltf");
            assert_eq!(
                metadata,
                JointPoseMetadata {
                    aim_bend_weight: 1.0,
                    ..JointPoseMetadata::default()
                },
                "unknown or malformed poseMask degrades to no membership for {raw}",
            );
        }
    }

    #[test]
    fn read_pose_masks_non_object_extras_degrade_to_no_membership() {
        for raw in [r#"["aimSpine"]"#, r#""aimSpine""#, "42", "null"] {
            assert_eq!(
                read_pose_masks(&extras_from_raw(raw), 5, "model.gltf"),
                JointPoseMetadata {
                    aim_bend_weight: 1.0,
                    ..JointPoseMetadata::default()
                },
                "non-object pose metadata {raw} degrades without failing the load",
            );
        }
    }

    #[test]
    fn pose_masks_follow_joint_topology_remap_with_parallel_weights() {
        // Source skin order is child, root, lower; topo order is root, child,
        // lower. Memberships and weights must follow the same remap as joints.
        let source = vec![
            JointPoseMetadata {
                aim_spine: true,
                upper_body: true,
                aim_bend_weight: 2.0,
                ..JointPoseMetadata::default()
            },
            JointPoseMetadata {
                aim_spine: true,
                upper_body: true,
                aim_bend_weight: 0.25,
                ..JointPoseMetadata::default()
            },
            JointPoseMetadata {
                lower_body: true,
                aim_bend_weight: 1.0,
                ..JointPoseMetadata::default()
            },
        ];
        let (masks, weights, _legs) = pose_masks_from_topo_metadata(&[1, 0, 2], &source, "x");
        assert_eq!(masks.aim_spine.iter().collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(masks.upper_body.iter().collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(masks.lower_body.iter().collect::<Vec<_>>(), vec![2]);
        assert_eq!(weights, vec![0.25, 2.0, 1.0]);
    }

    fn test_skeleton(parents: &[Option<usize>]) -> Skeleton {
        Skeleton::new(
            parents
                .iter()
                .copied()
                .map(|parent| Joint {
                    parent,
                    inverse_bind: Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal::default(),
                })
                .collect(),
        )
        .expect("test skeleton is parent-before-child")
    }

    fn mask(indices: &[usize]) -> JointMask {
        let mut mask = JointMask::new();
        for &index in indices {
            assert!(mask.insert(index));
        }
        mask
    }

    #[test]
    fn aim_spine_chain_validation_orders_root_to_tip_and_rejects_invalid_sets() {
        let chain = test_skeleton(&[None, Some(0), Some(1), Some(0), None]);
        assert_eq!(
            ordered_aim_spine_chain(&chain, mask(&[0, 1, 2])),
            Some(vec![0, 1, 2]),
        );
        assert_eq!(
            ordered_aim_spine_chain(&chain, mask(&[0, 1, 3])),
            None,
            "branching set is rejected",
        );
        assert_eq!(
            ordered_aim_spine_chain(&chain, mask(&[0, 2])),
            None,
            "set disconnected by an unmasked parent is rejected",
        );
        assert_eq!(
            ordered_aim_spine_chain(&chain, mask(&[0, 4])),
            None,
            "multiple hierarchy roots are rejected",
        );
        assert_eq!(
            ordered_aim_spine_chain(&chain, JointMask::new()),
            None,
            "empty set is rejected",
        );
    }

    #[test]
    fn pose_stack_uses_fixed_split_then_pitch_order_and_topo_weights() {
        let skeleton = test_skeleton(&[None, Some(0), Some(1), Some(0)]);
        let masks = PoseMaskSet {
            aim_spine: mask(&[0, 1, 2]),
            upper_body: mask(&[0, 1, 2]),
            lower_body: mask(&[3]),
        };
        let stack = build_pose_modifier_stack(&skeleton, masks, &[0.25, 1.0, 2.0, 1.0], &[], "x");
        assert_eq!(stack.entries().len(), 2);
        assert!(matches!(
            stack.entries()[0].modifier,
            PoseModifier::UpperLowerSplit { .. }
        ));
        assert!(matches!(
            &stack.entries()[1].modifier,
            PoseModifier::AimPitchBend { bend_weights }
                if bend_weights == &[0.25, 1.0, 2.0]
        ));
    }

    #[test]
    fn absent_masks_build_empty_stack_and_partial_masks_build_partial_stack() {
        let skeleton = test_skeleton(&[None, Some(0)]);
        assert!(
            build_pose_modifier_stack(&skeleton, PoseMaskSet::default(), &[], &[], "x").is_empty()
        );

        let aim_only = PoseMaskSet {
            aim_spine: mask(&[0, 1]),
            ..PoseMaskSet::default()
        };
        let stack = build_pose_modifier_stack(&skeleton, aim_only, &[1.0, 1.0], &[], "x");
        assert_eq!(stack.entries().len(), 1);
        assert!(matches!(
            stack.entries()[0].modifier,
            PoseModifier::AimPitchBend { .. }
        ));

        let upper_only = PoseMaskSet {
            upper_body: mask(&[0]),
            ..PoseMaskSet::default()
        };
        assert!(build_pose_modifier_stack(&skeleton, upper_only, &[], &[], "x").is_empty());
    }

    // ---- Foot-IK leg tagging ----------------------------------------------

    fn side_leg(index: usize) -> Option<LegRef> {
        Some(LegRef {
            index,
            family: LegNameFamily::Side,
        })
    }

    fn indexed_leg(index: usize) -> Option<LegRef> {
        Some(LegRef {
            index,
            family: LegNameFamily::Indexed,
        })
    }

    fn leg_meta(chain: Option<LegRef>, foot: Option<LegRef>) -> JointPoseMetadata {
        JointPoseMetadata {
            aim_bend_weight: 1.0,
            leg_chain: chain,
            foot_target: foot,
            ..JointPoseMetadata::default()
        }
    }

    #[test]
    fn read_pose_masks_recognizes_leg_and_foot_side_and_indexed_names() {
        let l = read_pose_masks(
            &extras_from_raw(r#"{ "poseMask": ["legL", "footL"] }"#),
            1,
            "m",
        );
        assert_eq!(l.leg_chain, side_leg(0));
        assert_eq!(l.foot_target, side_leg(0));

        let r = read_pose_masks(&extras_from_raw(r#"{ "poseMask": "legR" }"#), 2, "m");
        assert_eq!(r.leg_chain, side_leg(1));
        assert_eq!(r.foot_target, None);

        let foot_r = read_pose_masks(&extras_from_raw(r#"{ "poseMask": "footR" }"#), 3, "m");
        assert_eq!(foot_r.foot_target, side_leg(1));

        let indexed = read_pose_masks(
            &extras_from_raw(r#"{ "poseMask": ["leg2", "foot2"] }"#),
            4,
            "m",
        );
        assert_eq!(indexed.leg_chain, indexed_leg(2));
        assert_eq!(indexed.foot_target, indexed_leg(2));

        // `leg`/`legX`/`foothold` are not `prefix + digits`: unknown, no membership.
        let unknown = read_pose_masks(
            &extras_from_raw(r#"{ "poseMask": ["leg", "legX", "foothold"] }"#),
            5,
            "m",
        );
        assert_eq!(unknown.leg_chain, None);
        assert_eq!(unknown.foot_target, None);
    }

    #[test]
    fn same_node_mixed_family_tag_arrays_invalidate_membership_and_retain_family_sightings() {
        // Regression: array entries previously overwrote the same Option, so
        // [legL, leg0] and [footL, foot0] silently kept the last spelling and
        // hid the model's mixed-family diagnostic.
        let leg = read_pose_masks(
            &extras_from_raw(r#"{ "poseMask": ["legL", "leg0"] }"#),
            1,
            "model.gltf",
        );
        assert_eq!(leg.leg_chain, None);
        assert!(leg.leg_chain_conflict);
        assert!(leg.saw_side_leg_name && leg.saw_indexed_leg_name);

        let foot = read_pose_masks(
            &extras_from_raw(r#"{ "poseMask": ["footL", "foot0"] }"#),
            2,
            "model.gltf",
        );
        assert_eq!(foot.foot_target, None);
        assert!(foot.foot_target_conflict);
        assert!(foot.saw_side_leg_name && foot.saw_indexed_leg_name);

        let source = vec![leg, leg_meta(indexed_leg(0), None), foot];
        let (_masks, _weights, legs) =
            pose_masks_from_topo_metadata(&[0, 1, 2], &source, "model.gltf");
        assert!(
            legs.is_empty(),
            "conflicting same-node declarations must not build a solver leg"
        );
    }

    #[test]
    fn biped_leg_set_builds_in_l_then_r_order_through_topo_remap() {
        // Skin order [hipL, kneeL, ankleL, hipR, kneeR, ankleR]; a rotated topo
        // order proves the leg joints are reindexed and the set is ordered by
        // leg index (L=0 before R=1), NOT by topo position.
        let source = vec![
            leg_meta(side_leg(0), None),        // hipL
            leg_meta(side_leg(0), None),        // kneeL
            leg_meta(side_leg(0), side_leg(0)), // ankleL (chain + foot)
            leg_meta(side_leg(1), None),        // hipR
            leg_meta(side_leg(1), None),        // kneeR
            leg_meta(side_leg(1), side_leg(1)), // ankleR (chain + foot)
        ];
        let (_masks, _weights, legs) =
            pose_masks_from_topo_metadata(&[3, 4, 5, 0, 1, 2], &source, "x");
        assert_eq!(
            legs,
            vec![
                LegChain {
                    chain_mask: mask(&[3, 4, 5]),
                    foot_joint: 5,
                },
                LegChain {
                    chain_mask: mask(&[0, 1, 2]),
                    foot_joint: 2,
                },
            ],
            "L=0 leg precedes R=1, joints reindexed through the topo remap",
        );
    }

    #[test]
    fn build_pose_modifier_stack_installs_one_foot_ik_when_legs_present_and_none_when_absent() {
        // AC 4: presence of leg tags builds exactly one FootIk entry; the same
        // model with its leg tags removed builds none (no descriptor gate).
        let skeleton = test_skeleton(&[None, Some(0), Some(1), Some(0), Some(3), Some(4)]);
        let legs = vec![
            LegChain {
                chain_mask: mask(&[0, 1, 2]),
                foot_joint: 2,
            },
            LegChain {
                chain_mask: mask(&[3, 4, 5]),
                foot_joint: 5,
            },
        ];

        let with_legs =
            build_pose_modifier_stack(&skeleton, PoseMaskSet::default(), &[], &legs, "x");
        assert_eq!(with_legs.entries().len(), 1);
        match &with_legs.entries()[0].modifier {
            PoseModifier::FootIk { legs: built } => {
                assert_eq!(built.len(), 2, "the whole 2-leg set rides one entry");
                assert_eq!(built, &legs);
            }
            other => panic!("expected a single FootIk entry, got {other:?}"),
        }

        let without_legs =
            build_pose_modifier_stack(&skeleton, PoseMaskSet::default(), &[], &[], "x");
        assert!(
            without_legs.is_empty(),
            "leg tags removed ⇒ no FootIk entry",
        );
    }

    #[test]
    fn indexed_names_build_n_leg_set_and_truncate_beyond_max_feet() {
        // Seven indexed legs, each a two-joint chain (a plain chain joint then an
        // ankle carrying both chain and foot tags). 7 > MAX_FEET, so the last leg
        // warns and is dropped, and the set is N-leg (not biped-hardcoded).
        let mut source = Vec::new();
        for i in 0..7 {
            source.push(leg_meta(indexed_leg(i), None));
            source.push(leg_meta(indexed_leg(i), indexed_leg(i)));
        }
        let topo: Vec<usize> = (0..source.len()).collect();
        let (_masks, _weights, legs) = pose_masks_from_topo_metadata(&topo, &source, "x");

        assert_eq!(legs.len(), MAX_FEET, "leg set truncates at MAX_FEET");
        assert_eq!(
            legs[0],
            LegChain {
                chain_mask: mask(&[0, 1]),
                foot_joint: 1,
            },
        );
        assert_eq!(
            legs[5].foot_joint, 11,
            "ascending index order up to the cap"
        );
    }

    #[test]
    fn leg_with_missing_chain_or_missing_foot_is_dropped() {
        // Foot target without a chain, and a chain without a foot target: both
        // are incomplete legs — warn and drop, never a load failure.
        let source = vec![
            leg_meta(None, side_leg(0)), // footL only, no chain
            leg_meta(side_leg(1), None), // legR only, no foot target
        ];
        let (_masks, _weights, legs) = pose_masks_from_topo_metadata(&[0, 1], &source, "x");
        assert!(legs.is_empty(), "incomplete legs are dropped, got {legs:?}");
    }

    #[test]
    fn leg_with_duplicate_foot_targets_is_dropped() {
        // A leg has one end effector. Keeping whichever duplicate happened to
        // scan last makes authored node order decide the solver target.
        let source = vec![
            leg_meta(side_leg(0), None),
            leg_meta(side_leg(0), None),
            leg_meta(side_leg(0), side_leg(0)),
            leg_meta(None, side_leg(0)),
        ];
        let (_masks, _weights, legs) = pose_masks_from_topo_metadata(&[0, 1, 2, 3], &source, "x");
        assert!(legs.is_empty(), "duplicate foot targets invalidate the leg");
    }

    #[test]
    fn leg_chain_validation_keeps_only_connected_terminal_three_joint_chains() {
        // Regression: malformed leg tags used to reach FootIk as arbitrary
        // masks. The solver needs one definite hip → knee → ankle chain, with
        // the authored foot target at the terminal joint.
        let skeleton = test_skeleton(&[None, Some(0), Some(1), Some(1), None, Some(4)]);
        let valid = LegChain {
            chain_mask: mask(&[0, 1, 2]),
            foot_joint: 2,
        };
        let branched = LegChain {
            chain_mask: mask(&[0, 1, 2, 3]),
            foot_joint: 2,
        };
        let disconnected = LegChain {
            chain_mask: mask(&[0, 1, 4]),
            foot_joint: 4,
        };
        let non_terminal_foot = LegChain {
            chain_mask: mask(&[0, 1, 2]),
            foot_joint: 1,
        };
        let short = LegChain {
            chain_mask: mask(&[4, 5]),
            foot_joint: 5,
        };

        assert_eq!(
            validate_leg_set(
                &skeleton,
                vec![
                    valid.clone(),
                    branched,
                    disconnected,
                    non_terminal_foot,
                    short
                ],
                "fixture",
            ),
            vec![valid],
            "only a connected hip-knee-ankle chain ending at its tagged foot survives",
        );
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
    fn skinned_socket_uses_skin_joint_to_topo_remap() {
        let mut json = fixture_json(&joint_zones_fixture_path());
        // Node 2 is skin-joint index 0, but its parent (node 1, skin index 1)
        // emits first. The sibling at skin index 2 then emits in that same
        // traversal pass, so this node resolves to topo joint 2 — never its
        // raw skin-joint index 0.
        json["nodes"][2]["extras"]["socket"] = serde_json::json!("head_grip");
        let path = write_temp_fixture("skinned_socket_remap", &json);
        let model = load_model(&path).expect("socket metadata never rejects a model");
        let _ = std::fs::remove_file(path);

        assert_eq!(
            model.sockets.get("head_grip"),
            Some(&SocketBinding::SkinnedJoint(2)),
            "socket follows the skin-joint-to-topo remap",
        );
    }

    #[test]
    fn skinned_socket_duplicates_keep_first_document_node_and_non_joints_drop() {
        let mut json = fixture_json(&joint_zones_fixture_path());
        // Node 0 is the mesh node, not a skin joint: it must not create a
        // binding. Nodes 1 then 2 are joints in document order and share the
        // same name, so the topo-root binding on node 1 wins.
        json["nodes"][0]["extras"] = serde_json::json!({ "socket": "stray" });
        json["nodes"][1]["extras"]["socket"] = serde_json::json!("grip");
        json["nodes"][2]["extras"]["socket"] = serde_json::json!("grip");
        let path = write_temp_fixture("skinned_socket_duplicate", &json);
        let model = load_model(&path).expect("duplicate socket metadata never rejects a model");
        let _ = std::fs::remove_file(path);

        assert_eq!(
            model.sockets.len(),
            1,
            "only the valid first binding remains"
        );
        assert_eq!(
            model.sockets.get("grip"),
            Some(&SocketBinding::SkinnedJoint(0)),
            "the root node is the deterministic first document-order winner",
        );
        assert!(
            !model.sockets.contains_key("stray"),
            "a socket on a skinned non-joint is ignored",
        );
    }

    #[test]
    fn malformed_skinned_socket_degrades_without_failing_load() {
        let mut json = fixture_json(&joint_zones_fixture_path());
        json["nodes"][2]["extras"]["socket"] = serde_json::json!(42);
        let path = write_temp_fixture("malformed_skinned_socket", &json);
        let model = load_model(&path).expect("malformed socket metadata never rejects a model");
        let _ = std::fs::remove_file(path);

        assert!(model.sockets.is_empty(), "malformed socket is skipped");
    }

    #[test]
    fn rigid_sockets_compose_descendant_rest_transforms_and_keep_first_duplicate() {
        let mut json = fixture_json(&multi_primitive_fixture_path());
        let nodes = json["nodes"]
            .as_array_mut()
            .expect("fixture nodes are an array");
        // Geometry is consumed in this mesh node's local frame, so its own
        // translation must not appear in the socket transforms.
        nodes[0]["translation"] = serde_json::json!([100.0, 0.0, 0.0]);
        nodes[0]["extras"] = serde_json::json!({ "socket": "origin" });
        nodes[0]["children"] = serde_json::json!([1, 3]);
        nodes.push(serde_json::json!({
            "translation": [1.0, 2.0, 3.0],
            "rotation": [0.0, 0.0, 0.70710677, 0.70710677],
            "scale": [2.0, 3.0, 4.0],
            "children": [2],
        }));
        nodes.push(serde_json::json!({
            "translation": [0.0, 1.0, 0.0],
            "extras": { "socket": "rail" },
        }));
        nodes.push(serde_json::json!({
            "translation": [50.0, 0.0, 0.0],
            "extras": { "socket": "rail" },
        }));
        nodes.push(serde_json::json!({
            "extras": { "socket": "outside" },
        }));
        let path = write_temp_fixture("rigid_socket_transforms", &json);
        let model = load_model(&path).expect("rigid socket metadata never rejects a model");
        let _ = std::fs::remove_file(path);

        let parent = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 3.0, 4.0),
            Quat::from_xyzw(0.0, 0.0, 0.70710677, 0.70710677),
            Vec3::new(1.0, 2.0, 3.0),
        );
        let expected_rail = parent * Mat4::from_translation(Vec3::Y);
        let SocketBinding::RigidRest(rail) = model
            .sockets
            .get("rail")
            .expect("descendant socket is retained")
        else {
            panic!("rigid socket must retain a rest transform");
        };
        for (actual, expected) in rail
            .to_cols_array()
            .into_iter()
            .zip(expected_rail.to_cols_array())
        {
            assert!(
                (actual - expected).abs() < 1.0e-5,
                "descendant rest transform differs: {rail:?} vs {expected_rail:?}",
            );
        }
        assert_eq!(
            model.sockets.get("origin"),
            Some(&SocketBinding::RigidRest(Mat4::IDENTITY)),
            "a socket on the mesh node is model-space identity",
        );
        assert!(
            !model.sockets.contains_key("outside"),
            "a rigid socket outside the mesh-node hierarchy is ignored",
        );
    }

    // Regression: a cyclic rigid node hierarchy made socket parent traversal
    // loop forever.
    #[test]
    fn rigid_socket_cycle_degrades_without_failing_load() {
        let mut json = fixture_json(&multi_primitive_fixture_path());
        let nodes = json["nodes"]
            .as_array_mut()
            .expect("fixture nodes are an array");
        nodes[0]["children"] = serde_json::json!([1]);
        nodes.push(serde_json::json!({ "children": [2] }));
        nodes.push(serde_json::json!({
            "children": [1],
            "extras": { "socket": "loop" },
        }));
        let path = write_temp_fixture("rigid_socket_cycle", &json);
        let model = load_model(&path).expect("malformed socket hierarchy stays non-fatal");
        let _ = std::fs::remove_file(path);

        assert!(
            !model.sockets.contains_key("loop"),
            "socket with a cyclic parent chain is skipped",
        );
    }

    #[test]
    fn joint_pose_masks_load_reindexed_and_build_fixed_order_stack() {
        let model = load_model(&joint_zones_fixture_path()).expect("pose-mask fixture loads");
        assert_eq!(
            model.pose_masks,
            PoseMaskSet {
                aim_spine: mask(&[0, 2]),
                upper_body: mask(&[0, 2]),
                lower_body: mask(&[1]),
            },
            "child-first source skin order is remapped to root-first topo indices",
        );
        assert_eq!(
            model.pose_stack,
            PoseModifierStack::new(vec![
                ModifierEntry {
                    mask: mask(&[0, 2]),
                    modifier: PoseModifier::UpperLowerSplit {
                        lower_body_mask: mask(&[1]),
                    },
                },
                ModifierEntry {
                    mask: mask(&[0, 2]),
                    modifier: PoseModifier::AimPitchBend {
                        bend_weights: vec![0.25, 2.0],
                    },
                },
            ]),
            "loader pins split before bend and preserves topo-ordered weights",
        );
    }

    #[test]
    fn malformed_or_non_chain_pose_masks_load_with_empty_or_partial_stack() {
        let cases = [
            (
                "branching_aim_spine",
                serde_json::json!({ "poseMask": ["aimSpine", "upperBody"] }),
                serde_json::json!({ "poseMask": "aimSpine" }),
                serde_json::json!({ "poseMask": ["aimSpine", "lowerBody"] }),
            ),
            (
                "disconnected_aim_spine",
                serde_json::json!({ "poseMask": "upperBody" }),
                serde_json::json!({ "poseMask": "aimSpine" }),
                serde_json::json!({ "poseMask": ["aimSpine", "lowerBody"] }),
            ),
            (
                "empty_malformed_pose_masks",
                serde_json::json!({ "poseMask": [] }),
                serde_json::json!({ "poseMask": [42, "unknownMask"] }),
                serde_json::json!({ "poseMask": { "bad": true } }),
            ),
            (
                "non_object_pose_metadata",
                serde_json::json!(["aimSpine"]),
                serde_json::json!("upperBody"),
                serde_json::json!(42),
            ),
        ];

        for (name, root, head, leaf) in cases {
            let mut json = fixture_json(&joint_zones_fixture_path());
            json["nodes"][1]["extras"] = root;
            json["nodes"][2]["extras"] = head;
            json["nodes"][3]["extras"] = leaf;
            let path = write_temp_fixture(name, &json);
            let model = load_model(&path).expect("invalid pose authoring never fails model load");
            let _ = std::fs::remove_file(path);

            let (expected_masks, expected_stack) = match name {
                "branching_aim_spine" => {
                    let masks = PoseMaskSet {
                        aim_spine: mask(&[0, 1, 2]),
                        upper_body: mask(&[0]),
                        lower_body: mask(&[1]),
                    };
                    let stack = PoseModifierStack::new(vec![ModifierEntry {
                        mask: masks.upper_body,
                        modifier: PoseModifier::UpperLowerSplit {
                            lower_body_mask: masks.lower_body,
                        },
                    }]);
                    (masks, stack)
                }
                "disconnected_aim_spine" => {
                    let masks = PoseMaskSet {
                        aim_spine: mask(&[1, 2]),
                        upper_body: mask(&[0]),
                        lower_body: mask(&[1]),
                    };
                    let stack = PoseModifierStack::new(vec![ModifierEntry {
                        mask: masks.upper_body,
                        modifier: PoseModifier::UpperLowerSplit {
                            lower_body_mask: masks.lower_body,
                        },
                    }]);
                    (masks, stack)
                }
                "empty_malformed_pose_masks" | "non_object_pose_metadata" => {
                    (PoseMaskSet::default(), PoseModifierStack::default())
                }
                _ => unreachable!("all malformed pose-mask cases have expectations"),
            };

            assert_eq!(model.pose_masks, expected_masks, "{name} exact masks");
            assert_eq!(
                model.pose_stack, expected_stack,
                "{name} retains only the valid partial stack",
            );
        }
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
        assert_eq!(model.pose_masks, PoseMaskSet::default());
        assert!(model.pose_stack.is_empty());
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
    fn malformed_animation_channel_does_not_extend_clip_duration() {
        // Regression: malformed channels skipped during track installation still
        // extended the clip duration from their input times.
        let mut json = fixture_json(&multi_clip_fixture_path());
        json["buffers"][0]["byteLength"] = serde_json::json!(432);
        json["buffers"][0]["uri"] = serde_json::json!(
            "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAAAAAAABAAIAAAAAAIA/AAAAAAAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAAAAAAIA/AACAPwAAAAAAAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAAAAAACAPwAAAAAAAIA/AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAA8wQ1P/MENT8AAAAAAACAPwAAAEAAAAAAAAAAAAAAAAAAACBBAAAAAAAAAAAAAKBBAAAAAAAAAAAAAAAAAACAPwAAgL8AAIC/AACAvwAAoEAAAKBAAACgQAAAEEEAABBBAAAQQQAAAMAAAADAAAAAwAAA4EAAAOBAAADgQAAAAEEAAABBAAAAQQAAAAAAABBBAACAPwAAAEAAAEBAAACAQAAAoEAAAMBA"
        );
        json["bufferViews"]
            .as_array_mut()
            .expect("bufferViews is an array")
            .push(serde_json::json!({
                "buffer": 0,
                "byteOffset": 400,
                "byteLength": 8,
            }));
        json["bufferViews"]
            .as_array_mut()
            .expect("bufferViews is an array")
            .push(serde_json::json!({
                "buffer": 0,
                "byteOffset": 408,
                "byteLength": 24,
            }));
        json["accessors"]
            .as_array_mut()
            .expect("accessors is an array")
            .push(serde_json::json!({
                "bufferView": 11,
                "componentType": 5126,
                "count": 2,
                "type": "SCALAR",
                "min": [0.0],
                "max": [9.0],
            }));
        json["accessors"]
            .as_array_mut()
            .expect("accessors is an array")
            .push(serde_json::json!({
                "bufferView": 12,
                "componentType": 5126,
                "count": 2,
                "type": "VEC3",
            }));
        json["animations"][0]["samplers"]
            .as_array_mut()
            .expect("samplers is an array")
            .push(serde_json::json!({
                "input": 11,
                "output": 12,
                "interpolation": "CUBICSPLINE",
            }));
        json["animations"][0]["channels"]
            .as_array_mut()
            .expect("channels is an array")
            .push(serde_json::json!({
                "sampler": 1,
                "target": {
                    "node": 1,
                    "path": "translation",
                },
            }));
        let path = write_temp_fixture("malformed_animation_duration", &json);

        let result = std::panic::catch_unwind(|| load_model(&path));
        let _ = std::fs::remove_file(&path);
        let model = result
            .expect("malformed animation channel must not panic")
            .expect("malformed animation channel degrades rather than failing load");

        assert!(
            (model.clips[0].duration - 1.0).abs() < SAMPLE_EPS,
            "skipped channel with a 9.0s key must not extend 'idle' duration, got {}",
            model.clips[0].duration,
        );
    }

    #[test]
    fn multi_clip_fixture_step_channel_on_non_root_holds_lower_keyframe_value() {
        // Root-motion neutralization intentionally rewrites the selected root's
        // translation keys. Retarget this fixture's STEP sampler to the child
        // and remove its other translation channel so this remains a pure test
        // of loaded STEP semantics on a non-neutralized track.
        let mut json = fixture_json(&multi_clip_fixture_path());
        json["animations"][1]["channels"]
            .as_array_mut()
            .expect("walk channels are an array")
            .truncate(1);
        json["animations"][1]["channels"][0]["target"]["node"] = serde_json::json!(2);
        let path = write_temp_fixture("step_non_root", &json);
        let model = load_model(&path).expect("retargeted multi-clip fixture loads");
        let _ = std::fs::remove_file(&path);

        assert_eq!(
            model.clips[1].joints[1].translation.mode(),
            Interp::Step,
            "the glTF STEP sampler reaches the runtime track",
        );

        // Between keys 0 and 1: holds the lower key exactly (NOT the (5,0,0) a
        // LINEAR track would lerp to at the midpoint).
        assert_vec3_close(
            sampled_joint_translation(&model, 1, 1, 0.5),
            Vec3::new(0.0, 0.0, 0.0),
            "STEP holds the earlier keyframe value mid-span",
        );
        // At a keyframe time: snaps to that keyframe's value.
        assert_vec3_close(
            sampled_joint_translation(&model, 1, 1, 1.0),
            Vec3::new(10.0, 0.0, 0.0),
            "STEP snaps to the keyframe value at its time",
        );
        // After that keyframe, before the next: still holds it.
        assert_vec3_close(
            sampled_joint_translation(&model, 1, 1, 1.5),
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
