// One static light's patch direct: its sparse layer of bounced outgoing radiance, keyed on content.
// See: context/lib/build_pipeline.md §Build Cache

use std::io::{self, Write};

use glam::Vec3;
use rayon::prelude::*;

use super::patches::{MIN_PATCH_AREA_M2, PATCH_SIZE_M, PatchSet, Side};
use crate::bake_content_keys::{bounce_material_hash, fold_bounce_hash};
use crate::bake_control::BakeControl;
use crate::cache::CacheKey;
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{
    DEFAULT_AREA_SAMPLE_COUNT, fold_soft_visibility_constants, soft_visibility_early_out,
};
use crate::map_data::{FalloffModel, LightType, MapLight};
use crate::sh_bake::{
    RaytracingCtx, bounce_contribution, reflected_radiance, segment_clear, soft_visibility_seed,
};
use crate::surface_materials::stats::BounceMaterials;
use crate::texel_footprint::ProbeEarlyOut;

/// Build-cache stage id of one light's layer.
pub(crate) const SH_PATCH_LAYER_STAGE_ID: &str = "sh_patch_layer";
/// Epoch of the layer's shading and payload; bump when either changes.
pub(crate) const SH_PATCH_LAYER_STAGE_VERSION: u32 = 2;

/// Patches one governed work item shades. Chunks split a layer's reach in a
/// fixed way, so the layer does not depend on which worker ran which chunk.
pub(super) const PATCHES_PER_CHUNK: usize = 256;

/// Progress units one light's layer advances: one per chunk of its reach.
pub(super) fn layer_progress_units(patches_in_reach: usize) -> usize {
    patches_in_reach.div_ceil(PATCHES_PER_CHUNK)
}

/// Host bytes a layer over `reach` patches holds once shaded or decoded:
/// at most an entry for each side of every patch. `None` on overflow.
pub(super) fn layer_bytes(reach: u64) -> Option<u64> {
    reach.checked_mul((Side::BOTH.len() * size_of::<LayerEntry>()) as u64)
}

/// The most a layer over `reach` patches holds beside itself while it
/// resolves, by step: shading's chunk scratch (exact, both sides per patch)
/// with the in-reach list; then a cache write's stream buffer; or, on a
/// hit, the raw entry it decodes. `None` on overflow.
pub(super) fn layer_transient_bytes(reach: u64) -> Option<u64> {
    let chunks = reach.div_ceil(PATCHES_PER_CHUNK as u64);
    let shading = layer_bytes(reach)?
        .checked_add(chunks.checked_mul(size_of::<ChunkEntries>() as u64)?)?
        .checked_add(reach.checked_mul(size_of::<u32>() as u64)?)?;
    let payload = reach
        .checked_mul((Side::BOTH.len() * ENTRY_BYTES) as u64)?
        .checked_add(HEADER_BYTES as u64)?;
    let stream = crate::cache::StageCache::stream_buffer_bytes(payload);
    Some(shading.max(stream).max(payload))
}

/// One lit side of one patch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayerEntry {
    pub(crate) patch: u32,
    pub(crate) side: Side,
    /// Outgoing radiance: the light's colour and intensity, its Lambert
    /// term and visibility at the patch, and the patch's albedo over π.
    pub(crate) radiance: [f32; 3],
}

/// One light's patch direct over the patches in its reach, ascending by
/// (patch, side). A side the light does not reach has no entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SparseLayer {
    pub(crate) entries: Vec<LayerEntry>,
    #[cfg(test)]
    pub(crate) held: crate::live_bytes::Held,
}

/// blake3 over exactly the fields that shape a light's patch shading: its
/// kind, position, colour, intensity, falloff, range, emitter size and cone
/// or aim. No list position, source index, label, tag or tier field enters,
/// so two lights of equal content share a digest wherever they stand in the
/// entity list. The embedded-emitter flag is folded beside it in the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct LightDigest([u8; 32]);

impl LightDigest {
    pub(crate) fn of(light: &MapLight) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"sh patch layer light\0");
        hasher.update(&[match light.light_type {
            LightType::Point => 0,
            LightType::Spot => 1,
            LightType::Directional => 2,
        }]);
        for v in light.origin.to_array() {
            hasher.update(&v.to_le_bytes());
        }
        for v in light.color {
            hasher.update(&v.to_le_bytes());
        }
        hasher.update(&light.intensity.to_le_bytes());
        hasher.update(&[match light.falloff_model {
            FalloffModel::Linear => 0,
            FalloffModel::InverseDistance => 1,
            FalloffModel::InverseSquared => 2,
        }]);
        hasher.update(&light.falloff_range.to_le_bytes());
        hasher.update(&light.light_size.to_le_bytes());
        hasher.update(&light.angular_diameter.to_le_bytes());
        for angle in [light.cone_angle_inner, light.cone_angle_outer] {
            fold_optional(&mut hasher, angle.map(|a| a.to_le_bytes()));
        }
        fold_optional(
            &mut hasher,
            light.cone_direction.map(|d| {
                let mut bytes = [0u8; 12];
                for (chunk, v) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(d) {
                    *chunk = v.to_le_bytes();
                }
                bytes
            }),
        );
        Self(*hasher.finalize().as_bytes())
    }

    /// The digest's bytes, for the gathered volume's key to fold.
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The layer's soft-visibility seed axis: the digest's first eight
    /// bytes, so a light's lattice rotation moves only with its content.
    pub(crate) fn seed_axis(&self) -> u64 {
        let mut axis = [0u8; 8];
        axis.copy_from_slice(&self.0[..8]);
        u64::from_le_bytes(axis)
    }
}

fn fold_optional<const N: usize>(hasher: &mut blake3::Hasher, value: Option<[u8; N]>) {
    match value {
        Some(bytes) => {
            hasher.update(&[1]);
            hasher.update(&bytes);
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

/// The inputs every layer key of one bake shares: the geometry a layer
/// reads ([`layer_geometry_digest`]), the bounce materials, and the
/// constants that shape patches and their shading.
pub(crate) struct LayerKeys {
    shared: blake3::Hasher,
    /// [`Self::cell_id_fingerprint`], which no layer key folds.
    cell_ids: [u8; 32],
}

impl LayerKeys {
    /// Digests the geometry once. Built whenever the bake has a cache, for
    /// its reads and its writes alike.
    pub(crate) fn new(geometry: &GeometryResult, materials: Option<&BounceMaterials>) -> Self {
        let mut shared = blake3::Hasher::new();
        shared.update(&layer_geometry_digest(geometry));
        fold_bounce_hash(
            &mut shared,
            bounce_material_hash(materials, geometry).as_ref(),
        );
        shared.update(&PATCH_SIZE_M.to_le_bytes());
        shared.update(&MIN_PATCH_AREA_M2.to_le_bytes());
        // Patch direct samples the default lattice whatever
        // `--soft-shadow-samples` says, so that flag stays out.
        shared.update(&DEFAULT_AREA_SAMPLE_COUNT.to_le_bytes());
        // An unresolved texture reflects this grey, so it shapes layer bytes.
        shared.update(&crate::surface_materials::stats::GREY_BOUNCE_ALBEDO.to_le_bytes());
        // The patch seed's avalanche offset and the occlusion segment's
        // self-intersection step, both the SH bake's.
        shared.update(&crate::sh_bake::SAMPLING_LATTICE_OFFSET.to_le_bytes());
        shared.update(&crate::sh_bake::RAY_EPSILON.to_le_bytes());
        // Soft visibility's own lift, lattice rotation, sun ray length,
        // probe count and sampling epoch.
        fold_soft_visibility_constants(&mut shared);
        Self {
            shared,
            cell_ids: cell_id_digest(geometry),
        }
    }

    /// The digest of the inputs every layer key shares, for a key over the
    /// layers' sum to fold.
    pub(crate) fn shared_fingerprint(&self) -> [u8; 32] {
        *self.shared.finalize().as_bytes()
    }

    /// Every face's cell id, in face order, for the gathered volume's key.
    /// The BVH sorts its primitives by cell id, and a closest hit keeps the
    /// first of equal-distance hits, so a renumber can move a later-bounce
    /// or gather tie. A layer's occlusion is any-hit, whose answer no
    /// traversal order changes, so no layer key folds this.
    pub(crate) fn cell_id_fingerprint(&self) -> [u8; 32] {
        self.cell_ids
    }

    /// The key of the layer of a light with content `digest`. `embedded` is
    /// that light's effective flag: set only with the penumbra floor on and
    /// the light classified, so an unflagged light keys the same either way
    /// and no other light's flag reaches this key.
    pub(crate) fn key(&self, digest: LightDigest, embedded: bool) -> CacheKey {
        let mut hasher = self.shared.clone();
        hasher.update(&digest.0);
        hasher.update(&[u8::from(embedded)]);
        CacheKey::new(
            SH_PATCH_LAYER_STAGE_ID,
            SH_PATCH_LAYER_STAGE_VERSION,
            hasher.finalize().as_bytes(),
        )
    }
}

/// blake3 over exactly the geometry a layer reads: every vertex position,
/// the index buffer, and each face's index range and texture index. Patches
/// build from those, their normals from the positions; occlusion traces
/// those triangles; albedo follows the texture index. A face's cell id
/// (`leaf_index`) stays out: it only orders BVH primitives, and occlusion
/// is any-hit ([`LayerKeys::cell_id_fingerprint`]). The texture-name table
/// stays out too: albedo enters through the bounce-material fold, by index,
/// and the pipeline appends mover-only names before this stage.
fn layer_geometry_digest(geometry: &GeometryResult) -> [u8; 32] {
    let section = &geometry.geometry;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"sh patch layer geometry\0");
    hasher.update(&(section.vertices.len() as u64).to_le_bytes());
    fold_words(
        &mut hasher,
        section
            .vertices
            .iter()
            .flat_map(|vertex| vertex.position.map(f32::to_le_bytes)),
    );
    hasher.update(&(section.indices.len() as u64).to_le_bytes());
    fold_words(
        &mut hasher,
        section.indices.iter().map(|index| index.to_le_bytes()),
    );
    hasher.update(&(section.faces.len() as u64).to_le_bytes());
    fold_words(
        &mut hasher,
        section
            .faces
            .iter()
            .zip(&geometry.face_index_ranges)
            .flat_map(|(face, range)| {
                [face.texture_index, range.index_offset, range.index_count].map(u32::to_le_bytes)
            }),
    );
    *hasher.finalize().as_bytes()
}

/// blake3 over every face's cell id, in face order.
fn cell_id_digest(geometry: &GeometryResult) -> [u8; 32] {
    let faces = &geometry.geometry.faces;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"sh base face cell ids\0");
    hasher.update(&(faces.len() as u64).to_le_bytes());
    fold_words(
        &mut hasher,
        faces.iter().map(|face| face.leaf_index.to_le_bytes()),
    );
    *hasher.finalize().as_bytes()
}

/// Feed 4-byte words to `hasher` through a fixed buffer, so a large
/// geometry hashes in a few large updates rather than one per word.
fn fold_words(hasher: &mut blake3::Hasher, words: impl Iterator<Item = [u8; 4]>) {
    let mut buffer = [0u8; 4096];
    let mut len = 0;
    for word in words {
        buffer[len..len + 4].copy_from_slice(&word);
        len += 4;
        if len == buffer.len() {
            hasher.update(&buffer);
            len = 0;
        }
    }
    hasher.update(&buffer[..len]);
}

// Payload layout, little-endian: a header of magic, the patch set's size
// and the entry count (4 B each), then per entry 4 B patch, 1 B side and
// 12 B RGB radiance. A layout change needs an
// `SH_PATCH_LAYER_STAGE_VERSION` bump.
const LAYER_MAGIC: [u8; 4] = *b"SRPL";
const HEADER_BYTES: usize = 12;
const ENTRY_BYTES: usize = 17;

impl SparseLayer {
    /// Serialized length: a header naming the patch set's size and the
    /// entry count, then each entry's patch, side and radiance.
    pub(crate) fn byte_len(&self) -> usize {
        HEADER_BYTES + ENTRY_BYTES * self.entries.len()
    }

    /// Write the serialized layer, entry by entry, so a cache write holds no
    /// second copy of the layer.
    pub(crate) fn write_to(&self, patch_count: usize, out: &mut dyn Write) -> io::Result<()> {
        let patch_count = u32::try_from(patch_count).expect("patch count checked at plan time");
        // Up to two entries per patch, so a count past `u32::MAX` is possible
        // in principle: the entry is then not cached, and the bake goes on.
        let entry_count = u32::try_from(self.entries.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "patch layer entry count exceeds u32",
            )
        })?;
        out.write_all(&LAYER_MAGIC)?;
        out.write_all(&patch_count.to_le_bytes())?;
        out.write_all(&entry_count.to_le_bytes())?;
        for entry in &self.entries {
            let mut bytes = [0u8; ENTRY_BYTES];
            bytes[..4].copy_from_slice(&entry.patch.to_le_bytes());
            bytes[4] = entry.side as u8;
            for (chunk, v) in bytes[5..]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(entry.radiance)
            {
                *chunk = v.to_le_bytes();
            }
            out.write_all(&bytes)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn to_bytes(&self, patch_count: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.byte_len());
        self.write_to(patch_count, &mut bytes)
            .expect("writing to a Vec cannot fail");
        bytes
    }

    /// Decode a layer for a set of `patch_count` patches, or `None` when the
    /// bytes are not one: wrong magic or patch count, a length that is not
    /// the entry count's, or entries out of range or out of order.
    pub(crate) fn from_bytes(bytes: &[u8], patch_count: usize) -> Option<Self> {
        let header = bytes.get(..HEADER_BYTES)?;
        let word = |at: usize| {
            u32::from_le_bytes(
                header[at..at + 4]
                    .try_into()
                    .expect("a header word is four bytes"),
            )
        };
        if header[..4] != LAYER_MAGIC || word(4) as usize != patch_count {
            return None;
        }
        let count = word(8) as usize;
        let body = &bytes[HEADER_BYTES..];
        if body.len() != count.checked_mul(ENTRY_BYTES)? {
            return None;
        }
        let mut entries = Vec::with_capacity(count);
        #[cfg(test)]
        let held = crate::live_bytes::hold_vec("patch layers", &entries);
        for record in body.as_chunks::<ENTRY_BYTES>().0 {
            let patch = u32::from_le_bytes(
                record[..4]
                    .try_into()
                    .expect("an entry opens with its four-byte patch"),
            );
            let side = match record[4] {
                0 => Side::Front,
                1 => Side::Back,
                _ => return None,
            };
            let mut radiance = [0.0f32; 3];
            for (v, chunk) in radiance.iter_mut().zip(record[5..].as_chunks::<4>().0) {
                *v = f32::from_le_bytes(*chunk);
            }
            let ascending = entries
                .last()
                .is_none_or(|last: &LayerEntry| (last.patch, last.side) < (patch, side));
            if patch as usize >= patch_count || !ascending {
                return None;
            }
            entries.push(LayerEntry {
                patch,
                side,
                radiance,
            });
        }
        Some(Self {
            entries,
            #[cfg(test)]
            held,
        })
    }
}

/// Shade `light`'s layer over `in_reach`, the ascending indices of the
/// patches its range reaches ([`super::PatchReachIndex::in_reach`]): at each
/// patch centroid, for each side whose normal the light reaches, the
/// bounce's own Lambert term ([`bounce_contribution`]: range, cone and `n·l`
/// early-outs) times soft visibility times the patch's albedo over π, the
/// same terms the per-hit estimator takes at a gather hit. An `embedded`
/// light (its emitter sphere reaches into geometry) traces its full sample
/// set with no early-out; every other light takes the probe-and-escalate
/// set. The visibility seed mixes the patch's seed id, the side and
/// `light_axis` ([`LightDigest::seed_axis`]). Each chunk takes a governor
/// permit and advances `control`'s progress by one.
pub(crate) fn shade_light_layer(
    ctx: &RaytracingCtx<'_>,
    patches: &PatchSet,
    in_reach: &[u32],
    light: &MapLight,
    light_axis: u64,
    embedded: bool,
    control: &BakeControl,
) -> SparseLayer {
    let early_out = ProbeEarlyOut::for_probe_point(embedded);
    let trace = |from, to| segment_clear(ctx, from, to);
    #[cfg(test)]
    let _chunks_held = crate::live_bytes::hold(
        "patch layer chunk scratch",
        (layer_progress_units(in_reach.len()) * size_of::<ChunkEntries>()) as u64,
    );
    let chunks: Vec<ChunkEntries> = in_reach
        .par_chunks(PATCHES_PER_CHUNK)
        .map(|slice| {
            let _permit = control.governor().enter();
            // At most both sides of every patch: exact, so no growth.
            let mut entries = Vec::with_capacity(Side::BOTH.len() * slice.len());
            #[cfg(test)]
            let held = crate::live_bytes::hold_vec("patch layer chunk scratch", &entries);
            for &index in slice {
                #[cfg(test)]
                crate::bake_counters::bump(|counts| counts.patch_light_evaluations += 1);
                let patch = &patches.patches()[index as usize];
                for side in Side::BOTH {
                    let normal = side.normal(patch.normal);
                    let Some(contribution) = bounce_contribution(light, patch.centroid, normal)
                    else {
                        continue;
                    };
                    let seed = soft_visibility_seed(patch.seed_id(), side as u64, light_axis);
                    let visibility = soft_visibility_early_out(
                        patch.centroid,
                        normal,
                        light,
                        seed,
                        DEFAULT_AREA_SAMPLE_COUNT,
                        early_out,
                        trace,
                    );
                    if visibility <= 0.0 {
                        continue;
                    }
                    let radiance: Vec3 = reflected_radiance(
                        ctx.bounce_materials,
                        patch.texture_index,
                        contribution * visibility,
                    );
                    entries.push(LayerEntry {
                        patch: index,
                        side,
                        radiance: radiance.to_array(),
                    });
                }
            }
            control.advance(1);
            ChunkEntries {
                entries,
                #[cfg(test)]
                _held: held,
            }
        })
        .collect();
    // Exact, so a layer holds no growth slack beside its priced entries.
    let mut entries = Vec::with_capacity(chunks.iter().map(|chunk| chunk.entries.len()).sum());
    #[cfg(test)]
    let held = crate::live_bytes::hold_vec("patch layers", &entries);
    for chunk in chunks {
        entries.extend(chunk.entries);
    }
    SparseLayer {
        entries,
        #[cfg(test)]
        held,
    }
}

/// One work item's entries, waiting for the layer to take them.
struct ChunkEntries {
    entries: Vec<LayerEntry>,
    #[cfg(test)]
    _held: crate::live_bytes::Held,
}

#[cfg(test)]
mod tests {
    use super::super::test_scene::{Scene, room_lights};
    use super::*;

    /// Every layer key of `lights` over `scene`, and the gathered volume's.
    fn keys(scene: &Scene, lights: &[MapLight]) -> (Vec<String>, String) {
        let layers = lights
            .iter()
            .map(|light| scene.key(light).as_filename())
            .collect();
        (layers, scene.volume_key(lights).as_filename())
    }

    // Research P20: a face's cell id and the texture-name table reach no
    // layer, so a brush edit that only renumbers cells, or a mover-only
    // texture the pipeline appends to the name table, leaves every layer
    // key. A cell renumber still misses the gathered volume, whose
    // closest-hit ties follow BVH order; a name append does not. A moved
    // vertex or a face's texture index re-keys every layer.
    #[test]
    fn a_cell_renumber_or_a_texture_name_append_leaves_every_layer_key() {
        let base = Scene::new();
        let lights = room_lights();
        let (layers, volume) = keys(&base, &lights);

        let mut renumbered = base.geometry.clone();
        for face in &mut renumbered.geometry.faces {
            face.leaf_index += 3;
        }
        let (renumbered_layers, renumbered_volume) = keys(&Scene::of(renumbered), &lights);
        assert_eq!(
            renumbered_layers, layers,
            "a cell renumber re-keyed a layer"
        );
        assert_ne!(renumbered_volume, volume, "a cell renumber kept the volume");

        let mut appended = base.geometry.clone();
        appended
            .texture_names
            .names
            .push("bounce/mover_only".to_owned());
        let (appended_layers, appended_volume) = keys(&Scene::of(appended), &lights);
        assert_eq!(appended_layers, layers, "a name append re-keyed a layer");
        assert_eq!(appended_volume, volume, "a name append re-keyed the volume");

        let mut moved = base.geometry.clone();
        moved.geometry.vertices[0].position[1] += 0.01;
        let mut retextured = base.geometry.clone();
        retextured.geometry.faces[0].texture_index += 1;
        for (edit, geometry) in [("moved vertex", moved), ("texture index", retextured)] {
            let (edited, _) = keys(&Scene::of(geometry), &lights);
            for (before, after) in layers.iter().zip(&edited) {
                assert_ne!(before, after, "{edit} kept a layer key");
            }
        }
    }
}
