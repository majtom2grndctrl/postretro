// glTF → engine skinned-model loader (CPU-only; no wgpu).
// See: context/lib/rendering_pipeline.md §9 · context/lib/build_pipeline.md §Baked texture mips

use std::collections::HashMap;
use std::path::Path;

use glam::{Mat4, Quat, Vec3};
use postretro_level_format::gltf_resolve::resolve_material_base_color_path;
use postretro_level_format::octahedral;
use serde::Deserialize;
use thiserror::Error;

use super::mesh::{SkinnedMesh, SkinnedVertex};
use super::skeleton::{AnimationClip, Interp, Joint, JointTracks, RestLocal, Skeleton, Track};

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
    /// sweep. Empty for a static model loaded through this path.
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
}

/// Errors surfaced while loading a glTF model. Every malformed/unsupported input
/// returns one of these — the loader never panics; the caller handles absence.
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
    /// The skin references more joints than the `u8` joint index can address.
    #[error("glTF skin has {count} joints, exceeding the {max}-joint ceiling: {path}")]
    TooManyJoints {
        path: String,
        count: usize,
        max: usize,
    },
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

/// Load a skinned model from a glTF file at `path`.
///
/// Parses one mesh (all primitives merged into a single interleaved stream), its
/// skeleton (joints stored parent-before-child with inverse-bind matrices), and
/// its animation clips. Material textures resolve to their baked `.prm` cache key
/// by content-hashing the source PNG at runtime. Returns an error (never panics)
/// on malformed or unsupported input.
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

    // --- Skeleton ---------------------------------------------------------
    // Take the first skin if present; a static model (no skin) loads through the
    // rigid single-bone degenerate path (skeleton stays empty, joints → 0).
    //
    // `skin_joint_to_topo` reindexes mesh `JOINTS_0` (skin-joint indices) into
    // topo order; `node_to_topo` reindexes animation channel targets (node
    // indices) into the same topo order.
    let skin = document.skins().next();
    let (skeleton, skin_joint_to_topo, node_to_topo) = match skin.as_ref() {
        Some(skin) => build_skeleton(skin, &buffers, &path_str)?,
        None => (Skeleton::default(), HashMap::new(), HashMap::new()),
    };

    // --- Mesh -------------------------------------------------------------
    // Load exactly one mesh: the skinned mesh node's mesh, or the first mesh in
    // the document. Multi-mesh selection is the broadening task.
    let mesh = document
        .meshes()
        .next()
        .ok_or_else(|| ModelLoadError::NoMesh(path_str.clone()))?;

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
    })
}

/// Build the topologically-sorted skeleton and the skin-joint-index → topo-index
/// remap. glTF `JOINTS_0` attributes and animation targets reference joints by
/// their **skin-joint index** (position in `skin.joints()`); the renderer wants
/// joints parent-before-child, so we re-order and carry the remap.
type SkeletonMaps = (Skeleton, HashMap<usize, usize>, HashMap<usize, usize>);

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
    let reader = skin.reader(buffer_data);
    let inverse_binds: Vec<[[f32; 4]; 4]> = match reader.read_inverse_bind_matrices() {
        Some(iter) => iter.collect(),
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
            // A cycle (malformed skin) would stall progress; emit the rest in
            // their original order rather than loop forever. Worst case the
            // sampler composes a slightly-wrong pose; it never panics.
            for (skin_idx, emitted_flag) in emitted.iter_mut().enumerate() {
                if !*emitted_flag {
                    *emitted_flag = true;
                    topo_order.push(skin_idx);
                }
            }
            break;
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

    Ok((Skeleton { joints }, skin_joint_to_topo, node_to_topo))
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
        let uvs: Vec<[u16; 2]> = match reader.read_tex_coords(0) {
            Some(tc) => tc.into_f32().map(quantize_uv).collect(),
            None => vec![[0, 0]; vertex_count],
        };
        let normals: Vec<[u16; 2]> = match reader.read_normals() {
            Some(n) => n
                .map(|nn| octahedral::encode(nn[0], nn[1], nn[2]))
                .collect(),
            // Default to +Z (octahedral center) when normals are absent.
            None => vec![octahedral::encode(0.0, 0.0, 1.0); vertex_count],
        };
        let tangents: Vec<[u16; 2]> = match reader.read_tangents() {
            Some(t) => t.map(pack_tangent).collect(),
            // Default tangent: +X with positive bitangent sign.
            None => vec![pack_tangent([1.0, 0.0, 0.0, 1.0]); vertex_count],
        };
        // Joints reference skin-joint indices; remap to topo order. `into_u16`
        // normalizes the (u8|u16) storage; values exceed u8 only for skeletons
        // above the MAX_JOINTS ceiling, already rejected in build_skeleton.
        let joints: Vec<[u8; 4]> = match reader.read_joints(0) {
            Some(j) => j
                .into_u16()
                .map(|quad| remap_joint_quad(quad, skin_joint_to_topo))
                .collect(),
            None => vec![[0, 0, 0, 0]; vertex_count],
        };
        let weights: Vec<[u8; 4]> = match reader.read_weights(0) {
            Some(w) => w.into_u8().map(normalize_weights).collect(),
            // Rigid degenerate case: full weight on joint 0.
            None => vec![[255, 0, 0, 0]; vertex_count],
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
        match reader.read_indices() {
            Some(idx) => {
                for i in idx.into_u32() {
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

    // Tight local-space bound over the merged vertex positions, for the per-light
    // caster cull (a later task transforms it by the instance transform).
    out.compute_bounds();

    Ok((out, submeshes))
}

/// Remap one `[u16; 4]` joint quad (skin-joint indices) to topo-order `[u8; 4]`.
/// An index with no remap entry (shouldn't happen for a consistent skin) falls
/// back to joint 0.
fn remap_joint_quad(quad: [u16; 4], skin_joint_to_topo: &HashMap<usize, usize>) -> [u8; 4] {
    let mut out = [0u8; 4];
    for (i, &j) in quad.iter().enumerate() {
        out[i] = skin_joint_to_topo
            .get(&(j as usize))
            .copied()
            .unwrap_or(0)
            .min(u8::MAX as usize) as u8;
    }
    out
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
///   outputs are the per-keyframe values (one element per input time).
/// - CUBICSPLINE stores three elements per keyframe — `[in-tangent, value,
///   out-tangent]` — so the value of keyframe `k` is `raw[3k + 1]`. We extract
///   those, discard the tangents, store the track as `Linear` (degrading cubic to
///   linear; tangent storage and hermite blending are not implemented), and warn. If the triple shape does not hold
///   (`raw.len() != 3 * key_count`) the channel is malformed for cubic, so we warn
///   and skip it — this also guards the parallel-length invariant `Track`
///   requires (otherwise 3N values would be paired with N times).
fn resolve_keyframes<T: Copy>(
    raw: Vec<T>,
    key_count: usize,
    interpolation: gltf::animation::Interpolation,
    clip_name: &str,
    channel_kind: &str,
) -> Option<(Vec<T>, Interp)> {
    use gltf::animation::Interpolation;
    match interpolation {
        Interpolation::Linear => Some((raw, Interp::Linear)),
        Interpolation::Step => Some((raw, Interp::Step)),
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
        // discarded) — true cubic evaluation is out of scope (tangent storage and hermite blending are not implemented).
        let interpolation = channel.sampler().interpolation();

        let reader = channel.reader(buffer_data);
        let Some(inputs) = reader.read_inputs() else {
            continue;
        };
        let times: Vec<f32> = inputs.collect();
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
                {
                    joints[topo_idx].translation = Track {
                        times,
                        values,
                        mode,
                    };
                }
            }
            gltf::animation::util::ReadOutputs::Rotations(it) => {
                let raw: Vec<Quat> = it
                    .into_f32()
                    .map(|q| Quat::from_xyzw(q[0], q[1], q[2], q[3]))
                    .collect();
                if let Some((values, mode)) =
                    resolve_keyframes(raw, times.len(), interpolation, &name, "rotation")
                {
                    joints[topo_idx].rotation = Track {
                        times,
                        values,
                        mode,
                    };
                }
            }
            gltf::animation::util::ReadOutputs::Scales(it) => {
                let raw: Vec<Vec3> = it.map(Vec3::from).collect();
                if let Some((values, mode)) =
                    resolve_keyframes(raw, times.len(), interpolation, &name, "scale")
                {
                    joints[topo_idx].scale = Track {
                        times,
                        values,
                        mode,
                    };
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
    fn remap_joint_quad_reindexes_and_defaults_unknown_to_zero() {
        let mut map = HashMap::new();
        map.insert(5usize, 2usize); // skin-joint 5 → topo 2
        map.insert(9usize, 0usize); // skin-joint 9 → topo 0
        // Index 7 has no mapping → defaults to joint 0.
        let out = remap_joint_quad([5, 9, 7, 5], &map);
        assert_eq!(out, [2, 0, 0, 2]);
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

    // --- Real-model load (gated on the asset existing; no GPU) ---

    fn model_path() -> PathBuf {
        // CARGO_MANIFEST_DIR is crates/postretro; the asset is two levels up.
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
                assert!(j < jcount || jcount == 0, "vertex joint {j} < {jcount}");
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
        let b = model.mesh.bounds;
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

    use crate::model::anim::sample_clip;
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
            child_translation.mode,
            Interp::Linear,
            "CUBICSPLINE channel is stored as LINEAR (cubic degraded at load)",
        );
        assert_eq!(
            child_translation.values.len(),
            2,
            "two keyframes → two extracted values (tangents discarded)",
        );
        assert_vec3_close(
            child_translation.values[0],
            Vec3::new(5.0, 5.0, 5.0),
            "extracted keyframe-0 value, not a tangent",
        );
        assert_vec3_close(
            child_translation.values[1],
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
