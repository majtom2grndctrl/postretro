// glTF → engine skinned-model loader (CPU-only; no wgpu).
// See: context/lib/rendering_pipeline.md §9 · context/lib/build_pipeline.md §Baked texture mips
//
// One model, external-PNG textures, a single animation clip. Multi-primitive
// meshes merge into one interleaved stream; the per-primitive material split is
// preserved as submesh index ranges so the renderer can draw each material's
// triangles separately. Multi-mesh / multi-clip generality is out of scope.

use std::collections::HashMap;
use std::path::Path;

use glam::{Mat4, Quat, Vec3};
use postretro_level_format::octahedral;
use thiserror::Error;

use super::mesh::{SkinnedMesh, SkinnedVertex};
use super::skeleton::{AnimationClip, Joint, JointTracks, RestLocal, Skeleton, Track};

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
/// clips, and the per-primitive submeshes (material key + index range).
#[derive(Debug, Clone, Default)]
pub struct LoadedModel {
    /// The skinned geometry. A model with multiple primitives merges them into
    /// one interleaved stream; `submeshes` carries the per-primitive split.
    pub mesh: SkinnedMesh,
    /// The joint hierarchy the mesh binds against, stored parent-before-child
    /// (topological) so the sampler composes world matrices in one forward
    /// sweep. Empty for a static model loaded through this path.
    pub skeleton: Skeleton,
    /// Animation clips parsed from the glTF. The hardcoded slice ships one clip.
    pub clips: Vec<AnimationClip>,
    /// One submesh per mesh primitive, in primitive order: the material cache
    /// key and the index range it occupies in the merged buffer. The renderer
    /// resolves each key against the shared material/texture cache and draws each
    /// range against its (possibly shared) material.
    pub submeshes: Vec<Submesh>,
}

/// Errors surfaced while loading a glTF model. Every malformed/unsupported input
/// returns one of these — the loader never panics; the caller handles absence.
#[derive(Debug, Error)]
pub enum ModelLoadError {
    /// The glTF (or a referenced buffer / image) could not be read or parsed.
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

/// The all-zero cache key, hex-encoded (64 chars). `load_textures` reads this as
/// the "no source PNG" sentinel and binds a silent placeholder.
fn zero_material_key() -> String {
    "0".repeat(64)
}

/// Resolve a material's base-color texture to its baked `.prm` cache key by
/// content-hashing the source PNG.
///
/// Recipe (mirrors the level compiler's `filename_key_for` for a
/// diffuse-present texture, per `build_pipeline.md` §Baked texture mips):
/// `blake3(baseColor PNG bytes)`, hex-encoded — the same key the offline baker
/// names the `.prm` with, so `load_textures` opens `<key>.prm` directly. We
/// reproduce the recipe inline because `filename_key_for` is private to the
/// level-compiler crate.
///
/// Degrades to the zero sentinel (never errors/panics) when the base-color image
/// has no URI, is embedded in a buffer view (`Source::View`), or its file is
/// missing/unreadable — an unresolvable material renders a placeholder rather
/// than failing the whole load.
fn content_hash_material_key(material: &gltf::Material, parent_dir: &Path) -> String {
    let Some(uri) = material
        .pbr_metallic_roughness()
        .base_color_texture()
        .and_then(|info| match info.texture().source().source() {
            gltf::image::Source::Uri { uri, .. } => Some(uri.to_string()),
            gltf::image::Source::View { .. } => None,
        })
    else {
        return zero_material_key();
    };

    let png_path = parent_dir.join(uri);
    match std::fs::read(&png_path) {
        Ok(png_bytes) => {
            let key = *blake3::hash(&png_bytes).as_bytes();
            hex_encode(&key)
        }
        Err(_) => zero_material_key(),
    }
}

/// Lowercase-hex encode a 32-byte cache key into the 64-char string
/// `load_textures` / `cache_filename_for_key` consume.
fn hex_encode(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
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

    Ok(LoadedModel {
        mesh: skinned_mesh,
        skeleton,
        clips,
        submeshes,
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
                joints[topo_idx].translation = Track {
                    times,
                    values: it.map(Vec3::from).collect(),
                };
            }
            gltf::animation::util::ReadOutputs::Rotations(it) => {
                joints[topo_idx].rotation = Track {
                    times,
                    values: it
                        .into_f32()
                        .map(|q| Quat::from_xyzw(q[0], q[1], q[2], q[3]))
                        .collect(),
                };
            }
            gltf::animation::util::ReadOutputs::Scales(it) => {
                joints[topo_idx].scale = Track {
                    times,
                    values: it.map(Vec3::from).collect(),
                };
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
}
