// Animated-light chunks builder.
//
// For every BVH leaf that owns a face overlapped by an animated light, build
// a recursive face-local UV-space partition where every emitted chunk carries
// at most `MAX_ANIMATED_LIGHTS_PER_CHUNK` animated-light indices. Returns a
// per-leaf `(chunk_range_start, chunk_range_count)` table parallel to the BVH
// leaf array; the pack stage stamps these onto the on-disk `BvhLeaf` records
// during serialization. The runtime BVH walk over visible leaves then
// enumerates visible animated-light chunks as a union of contiguous ranges.
//
// Light indices in the emitted flat pool index into the **filtered** light
// list passed by the caller (the `!is_dynamic && animation.is_some()`
// filter — same namespace and iteration order as the animated-light
// `AnimationDescriptor` buffer the runtime compose pass consumes). No
// per-entry remap is needed downstream. `bake_only` animated lights
// participate in weight-map compose, so they are NOT filtered out here.
//
// See: context/plans/in-progress/animated-light-chunks/index.md

use std::collections::HashMap;

use glam::Vec3;
use postretro_level_format::animated_light_chunks::{
    AnimatedLightChunk, AnimatedLightChunksSection, MAX_ANIMATED_LIGHTS_PER_CHUNK,
};
use postretro_level_format::bvh::BvhSection;

use crate::geometry::FaceIndexRange;
use crate::light_namespaces::AnimatedBakedLights;
use crate::lightmap_bake::Chart;

/// How many overflow events to log individually before falling back to a
/// single summary log line. Keeps the compile log readable on pathological
/// inputs while preserving the first few diagnostic lines.
const MAX_OVERFLOW_LOG_LINES: u64 = 8;

/// Build the `AnimatedLightChunksSection` and a parallel per-leaf
/// `(chunk_range_start, chunk_range_count)` table.
///
/// The returned `Vec<(u32, u32)>` is indexed by BVH leaf slot (parallel to
/// `bvh_section.leaves`). The pack stage applies it to the on-disk `BvhLeaf`
/// records during serialization — making the dependency from "this stage
/// must run before BVH serialization" explicit at the call site rather than
/// hidden behind a side-effect on a shared struct.
///
/// Inputs:
/// - `bvh_section`: read-only. Iterated to pair leaves to faces.
/// - `filtered_lights` / `filtered_influence`: parallel slices, post
///   `!is_dynamic && animation.is_some()` filter — the same namespace and
///   iteration order the runtime `AnimationDescriptor` buffer uses, so
///   emitted indices match descriptor slots directly with no remap.
///   `bake_only` animated lights are retained here — they participate in
///   weight-map compose at runtime.
/// - `face_charts`: per-face chart data from the lightmap baker. Indexed by
///   geometry face index; supplies the face-local (origin, u_axis, v_axis)
///   basis and world-meter UV bounds.
/// - `face_index_ranges`: per-face index range parallel to `face_charts`.
///   Used only to pair BVH leaves back to the face they own. Today every
///   primitive (and therefore every leaf) covers exactly one face's range,
///   so the pairing is a single O(1) `index_offset` lookup. Generalizing
///   to a leaf spanning multiple consecutive face ranges would require a
///   different mapping structure (e.g. offset→(face, len) interval tree).
/// - `lightmap_texel_density`: meters per lightmap texel. The min UV-extent
///   floor is the texel size in meters — finer subdivision cannot be
///   addressed by the UV-indexed weight-map baker downstream.
pub fn build_animated_light_chunks(
    bvh_section: &BvhSection,
    animated_lights: &AnimatedBakedLights<'_>,
    face_charts: &[Chart],
    face_index_ranges: &[FaceIndexRange],
    lightmap_texel_density: f32,
) -> (AnimatedLightChunksSection, Vec<(u32, u32)>) {
    debug_assert_eq!(
        face_charts.len(),
        face_index_ranges.len(),
        "face_charts and face_index_ranges must be parallel per-face slices",
    );

    // Min UV extent = one lightmap texel in meters. `texel_density` in this
    // project is meters-per-texel (see `lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS`),
    // so the floor is the texel size itself; clamp to a safe positive value.
    let min_uv_extent = lightmap_texel_density.max(1.0e-4);

    // Drop directional and zero-radius animated lights from chunk emission.
    // Order matches the AnimatedBakedLights namespace so the per-chunk pool
    // feeds animation-descriptor slots directly with no remap.
    let animated: Vec<AnimatedLight> = animated_lights
        .entries()
        .iter()
        .enumerate()
        .filter_map(|(slot, e)| {
            let infl = &e.influence;
            if infl.radius != f32::MAX && infl.radius > 0.0 {
                Some(AnimatedLight {
                    filtered_index: slot as u32,
                    center: Vec3::from(infl.center),
                    radius: infl.radius,
                })
            } else {
                None
            }
        })
        .collect();

    let mut chunks: Vec<AnimatedLightChunk> = Vec::new();
    let mut light_indices: Vec<u32> = Vec::new();
    let mut leaf_chunk_ranges: Vec<(u32, u32)> = vec![(0, 0); bvh_section.leaves.len()];

    if animated.is_empty() {
        log::info!("[AnimatedLightChunks] no non-directional animated lights; section empty",);
        return (
            AnimatedLightChunksSection {
                chunks,
                light_indices,
            },
            leaf_chunk_ranges,
        );
    }

    // Pair leaves to faces. Today `bvh_build::collect_primitives` emits one
    // primitive per face, and `flatten` propagates each primitive into one
    // leaf — so a leaf's `(index_offset, index_count)` exactly matches one
    // face's `FaceIndexRange`. Build the lookup once; each leaf then resolves
    // to its face in O(1) via `face_by_offset.get(&leaf.index_offset)`.
    // Generalizing to a leaf spanning multiple consecutive face ranges would
    // require a different mapping structure (e.g. an offset→(face, len)
    // interval tree) — do not re-introduce a linear scan here.
    let mut face_by_offset: HashMap<u32, u32> = HashMap::with_capacity(face_index_ranges.len());
    for (face_index, range) in face_index_ranges.iter().enumerate() {
        if range.index_count == 0 {
            continue;
        }
        let prev = face_by_offset.insert(range.index_offset, face_index as u32);
        debug_assert!(
            prev.is_none(),
            "duplicate FaceIndexRange.index_offset {}: faces {:?} and {} collide",
            range.index_offset,
            prev,
            face_index,
        );
    }

    let mut overflow_chunks: u64 = 0;
    let mut overflow_drops: u64 = 0;
    let mut overflow_log_count: u64 = 0;

    for (leaf_idx, leaf) in bvh_section.leaves.iter().enumerate() {
        let range_start = chunks.len() as u32;

        let leaf_offset = leaf.index_offset;

        // O(1) leaf→face resolution. Today each leaf's `index_offset` is
        // exactly one face's `index_offset`. If this lookup ever misses (or
        // the face's range doesn't exactly match the leaf's), that's a real
        // change to the BVH contract — see the `face_by_offset` construction
        // above for the generalization note.
        debug_assert!(
            face_by_offset.contains_key(&leaf_offset) || leaf.index_count == 0,
            "non-empty leaf at offset {} has no matching face — BVH/geometry contract violation",
            leaf_offset,
        );
        if let Some(&face_index) = face_by_offset.get(&leaf_offset) {
            let face_index_usize = face_index as usize;
            let face_range = &face_index_ranges[face_index_usize];
            debug_assert_eq!(
                face_range.index_count, leaf.index_count,
                "leaf at offset {} spans {} indices but face {} owns {} — \
                 leaves spanning multiple faces require a different mapping \
                 structure (see face_by_offset construction)",
                leaf_offset, leaf.index_count, face_index, face_range.index_count,
            );

            let chart = &face_charts[face_index_usize];

            // Cheap reject: any animated light whose sphere overlaps the
            // chart's world-space face AABB is a candidate; pass the survivors
            // down so subdivision shrinks the candidate set.
            let face_aabb = project_uv_to_world_aabb(chart, chart.uv_min, chart.uv_extent);
            let candidates: Vec<u32> = animated
                .iter()
                .enumerate()
                .filter_map(|(i, al)| {
                    if sphere_overlaps_aabb(al.center, al.radius, face_aabb.0, face_aabb.1) {
                        Some(i as u32)
                    } else {
                        None
                    }
                })
                .collect();
            if !candidates.is_empty() {
                recurse(
                    face_index,
                    chart,
                    chart.uv_min,
                    chart.uv_extent,
                    &candidates,
                    &animated,
                    min_uv_extent,
                    &mut chunks,
                    &mut light_indices,
                    &mut overflow_chunks,
                    &mut overflow_drops,
                    &mut overflow_log_count,
                );
            }
        }

        let range_count = chunks.len() as u32 - range_start;
        leaf_chunk_ranges[leaf_idx] = (range_start, range_count);
    }

    let max_per_chunk = chunks.iter().map(|c| c.index_count).max().unwrap_or(0);
    let mean_per_chunk = if chunks.is_empty() {
        0.0
    } else {
        light_indices.len() as f64 / chunks.len() as f64
    };
    log::info!(
        "[AnimatedLightChunks] {} chunks, {} animated lights, max {} / chunk, mean {:.2} / chunk, \
         {} chunks bottomed out at min-extent floor",
        chunks.len(),
        animated.len(),
        max_per_chunk,
        mean_per_chunk,
        overflow_chunks,
    );
    if overflow_chunks > 0 {
        log::warn!(
            "[AnimatedLightChunks] {overflow_chunks} chunks exceeded cap \
             {MAX_ANIMATED_LIGHTS_PER_CHUNK} at the min-extent floor; \
             {overflow_drops} extra light entries retained beyond the cap"
        );
    }

    (
        AnimatedLightChunksSection {
            chunks,
            light_indices,
        },
        leaf_chunk_ranges,
    )
}

/// Animated light, with its filtered-list index for stable index emission.
#[derive(Debug, Clone, Copy)]
struct AnimatedLight {
    filtered_index: u32,
    center: Vec3,
    radius: f32,
}

#[allow(clippy::too_many_arguments)]
fn recurse(
    face_index: u32,
    chart: &Chart,
    uv_min: [f32; 2],
    uv_extent: [f32; 2],
    candidate_indices: &[u32], // indices into `animated`
    animated: &[AnimatedLight],
    min_uv_extent: f32,
    chunks: &mut Vec<AnimatedLightChunk>,
    light_indices: &mut Vec<u32>,
    overflow_chunks: &mut u64,
    overflow_drops: &mut u64,
    overflow_log_count: &mut u64,
) {
    let (aabb_min, aabb_max) = project_uv_to_world_aabb(chart, uv_min, uv_extent);

    // Filter the candidate set by sphere-vs-world-AABB overlap.
    let mut hits: Vec<u32> = candidate_indices
        .iter()
        .copied()
        .filter(|&i| {
            let al = &animated[i as usize];
            sphere_overlaps_aabb(al.center, al.radius, aabb_min, aabb_max)
        })
        .collect();

    if hits.is_empty() {
        // Prune: no chunk emitted for this sub-region.
        return;
    }

    // Termination floor: the face's (u_axis, v_axis) basis is orthonormal
    // (see lightmap_bake.rs), so `uv_extent` is already the world-meter
    // extent along the face's own basis. A world-axis min-extent check would
    // trip on the (always-zero) thickness dimension of any axis-aligned
    // planar face — walls, floors, ceilings — and defeat subdivision on the
    // common case. Subdivide until UV extent drops below one lightmap texel.
    let uv_extent_min = uv_extent[0].min(uv_extent[1]);
    let at_min_extent = uv_extent_min <= min_uv_extent;

    if hits.len() <= MAX_ANIMATED_LIGHTS_PER_CHUNK || at_min_extent {
        if hits.len() > MAX_ANIMATED_LIGHTS_PER_CHUNK {
            *overflow_chunks += 1;
            let dropped = hits.len() - MAX_ANIMATED_LIGHTS_PER_CHUNK;
            *overflow_drops += dropped as u64;
            if *overflow_log_count < MAX_OVERFLOW_LOG_LINES {
                *overflow_log_count += 1;
                log::warn!(
                    "[AnimatedLightChunks] face {face_index} chunk bottomed out at min-extent \
                     with {} animated lights (cap {}); retaining all overlapping lights",
                    hits.len(),
                    MAX_ANIMATED_LIGHTS_PER_CHUNK,
                );
            }
        }
        // Stable order: sort by filtered_index so the on-disk pool is
        // independent of the upstream candidate iteration order.
        hits.sort_by_key(|&i| animated[i as usize].filtered_index);

        let index_offset = light_indices.len() as u32;
        for &i in &hits {
            light_indices.push(animated[i as usize].filtered_index);
        }
        chunks.push(AnimatedLightChunk {
            aabb_min: aabb_min.to_array(),
            face_index,
            aabb_max: aabb_max.to_array(),
            index_offset,
            uv_min,
            uv_max: [uv_min[0] + uv_extent[0], uv_min[1] + uv_extent[1]],
            index_count: hits.len() as u32,
            _padding: 0,
        });
        return;
    }

    // Split along the longest UV axis. Tie → split U (axis 0) for stability.
    let split_u = uv_extent[0] >= uv_extent[1];
    let (left_min, left_extent, right_min, right_extent) = if split_u {
        let half = uv_extent[0] * 0.5;
        (
            uv_min,
            [half, uv_extent[1]],
            [uv_min[0] + half, uv_min[1]],
            [uv_extent[0] - half, uv_extent[1]],
        )
    } else {
        let half = uv_extent[1] * 0.5;
        (
            uv_min,
            [uv_extent[0], half],
            [uv_min[0], uv_min[1] + half],
            [uv_extent[0], uv_extent[1] - half],
        )
    };

    recurse(
        face_index,
        chart,
        left_min,
        left_extent,
        &hits,
        animated,
        min_uv_extent,
        chunks,
        light_indices,
        overflow_chunks,
        overflow_drops,
        overflow_log_count,
    );
    recurse(
        face_index,
        chart,
        right_min,
        right_extent,
        &hits,
        animated,
        min_uv_extent,
        chunks,
        light_indices,
        overflow_chunks,
        overflow_drops,
        overflow_log_count,
    );
}

/// Project a face-local UV rectangle (in world-meter units) to its world-space
/// AABB via the chart's (origin, u_axis, v_axis) basis. The four UV corners
/// project to four world points; the AABB is their component-wise extent.
fn project_uv_to_world_aabb(chart: &Chart, uv_min: [f32; 2], uv_extent: [f32; 2]) -> (Vec3, Vec3) {
    let u0 = uv_min[0];
    let v0 = uv_min[1];
    let u1 = u0 + uv_extent[0];
    let v1 = v0 + uv_extent[1];
    let corner = |u: f32, v: f32| chart.origin + chart.u_axis * u + chart.v_axis * v;
    let p00 = corner(u0, v0);
    let p10 = corner(u1, v0);
    let p01 = corner(u0, v1);
    let p11 = corner(u1, v1);
    let mn = p00.min(p10).min(p01).min(p11);
    let mx = p00.max(p10).max(p01).max(p11);
    (mn, mx)
}

/// Sphere-vs-AABB overlap by closest-point distance.
fn sphere_overlaps_aabb(center: Vec3, radius: f32, aabb_min: Vec3, aabb_max: Vec3) -> bool {
    let closest = center.clamp(aabb_min, aabb_max);
    let d = closest - center;
    d.length_squared() <= radius * radius
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::MapLight;
    use postretro_level_format::bvh::{BvhLeaf, BvhSection};
    use postretro_level_format::light_influence::InfluenceRecord;

    // ---- fixture helpers -------------------------------------------------

    fn chart_xz_plane() -> Chart {
        // 1m × 1m face on XZ at y=0, +Y normal. uv_extent = (1, 1).
        Chart {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [1.0, 1.0],
            normal: Vec3::Y,
            width_texels: 32,
            height_texels: 32,
        }
    }

    /// Chart with non-axis-aligned (u,v) basis. Retained from earlier tests
    /// as a secondary fixture exercising skewed UV→world projection.
    fn chart_tilted(origin: Vec3) -> Chart {
        Chart {
            origin,
            u_axis: Vec3::new(1.0, 0.5, 0.0),
            v_axis: Vec3::new(0.0, 0.5, 1.0),
            uv_min: [0.0, 0.0],
            uv_extent: [1.0, 1.0],
            normal: Vec3::new(0.5, -1.0, 0.5).normalize(),
            width_texels: 32,
            height_texels: 32,
        }
    }

    /// Project an (x, z) face-local point on `chart_tilted` to world space,
    /// matching the chart's (u,v) basis so test fixtures can place light
    /// centers onto the face.
    fn tilted_world_point(origin: Vec3, u: f32, v: f32) -> Vec3 {
        origin + Vec3::new(1.0, 0.5, 0.0) * u + Vec3::new(0.0, 0.5, 1.0) * v
    }

    fn make_bvh_with_one_leaf() -> BvhSection {
        BvhSection {
            nodes: Vec::new(),
            leaves: vec![BvhLeaf {
                aabb_min: [0.0, 0.0, 0.0],
                material_bucket_id: 0,
                aabb_max: [1.0, 0.0, 1.0],
                index_offset: 0,
                index_count: 6,
                cell_id: 0,
                // Default zero — these fields are populated by pack-time
                // stamping from the per-leaf range table the builder returns.
                chunk_range_start: 0,
                chunk_range_count: 0,
            }],
            root_node_index: 0,
        }
    }

    fn make_bvh_with_n_leaves(n: usize) -> BvhSection {
        let leaves = (0..n)
            .map(|i| BvhLeaf {
                aabb_min: [0.0, 0.0, 0.0],
                material_bucket_id: 0,
                aabb_max: [1.0, 0.0, 1.0],
                index_offset: (i * 6) as u32,
                index_count: 6,
                cell_id: 0,
                // Default zero — these fields are populated by pack-time
                // stamping from the per-leaf range table the builder returns.
                chunk_range_start: 0,
                chunk_range_count: 0,
            })
            .collect();
        BvhSection {
            nodes: Vec::new(),
            leaves,
            root_node_index: 0,
        }
    }

    fn one_face_range() -> Vec<FaceIndexRange> {
        vec![FaceIndexRange {
            index_offset: 0,
            index_count: 6,
        }]
    }

    fn n_face_ranges(n: usize) -> Vec<FaceIndexRange> {
        (0..n)
            .map(|i| FaceIndexRange {
                index_offset: (i * 6) as u32,
                index_count: 6,
            })
            .collect()
    }

    fn mk_animated_light() -> MapLight {
        use crate::map_data::{FalloffModel, LightAnimation, LightType};
        use glam::DVec3;
        MapLight {
            origin: DVec3::new(0.5, 0.0, 0.5),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 5.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: Some(LightAnimation {
                period: 1.0,
                phase: 0.0,
                brightness: Some(vec![1.0, 0.5]),
                color: None,
                direction: None,
                start_active: true,
            }),
            cast_shadows: true,
            bake_only: false,
            is_dynamic: false,
            tag: None,
        }
    }

    fn mk_bake_only_light() -> MapLight {
        let mut l = mk_animated_light();
        l.bake_only = true;
        l
    }

    fn mk_dynamic_light() -> MapLight {
        let mut l = mk_animated_light();
        l.is_dynamic = true;
        l
    }

    fn mk_inf(cx: f32, cz: f32, r: f32) -> InfluenceRecord {
        InfluenceRecord {
            center: [cx, 0.0, cz],
            radius: r,
        }
    }

    // Deprecated-name alias kept for the original smoke tests below.
    fn animated_point_light() -> MapLight {
        mk_animated_light()
    }

    // ---- existing smoke tests (preserved) --------------------------------

    #[test]
    fn no_animated_lights_emits_empty_section_and_zero_ranges() {
        let bvh = make_bvh_with_one_leaf();
        let envelope = AnimatedBakedLights::from_parallel_slices(&[], &[]);
        let (section, ranges) = build_animated_light_chunks(
            &bvh,
            &envelope,
            &[chart_xz_plane()],
            &one_face_range(),
            0.04,
        );
        assert!(section.chunks.is_empty());
        assert!(section.light_indices.is_empty());
        assert_eq!(ranges[0], (0, 0));
    }

    #[test]
    fn directional_animated_light_skipped() {
        let bvh = make_bvh_with_one_leaf();
        let lights = vec![animated_point_light()];
        let influence = vec![InfluenceRecord {
            center: [0.0, 0.0, 0.0],
            radius: f32::MAX,
        }];
        let envelope = AnimatedBakedLights::from_parallel_slices(&lights, &influence);
        let (section, ranges) = build_animated_light_chunks(
            &bvh,
            &envelope,
            &[chart_xz_plane()],
            &one_face_range(),
            0.04,
        );
        assert!(section.chunks.is_empty());
        assert_eq!(ranges[0].1, 0);
    }

    #[test]
    fn one_overlapping_animated_light_emits_one_chunk() {
        let bvh = make_bvh_with_one_leaf();
        let lights = vec![animated_point_light()];
        let influence = vec![InfluenceRecord {
            center: [0.5, 0.0, 0.5],
            radius: 5.0,
        }];
        let envelope = AnimatedBakedLights::from_parallel_slices(&lights, &influence);
        let (section, ranges) = build_animated_light_chunks(
            &bvh,
            &envelope,
            &[chart_xz_plane()],
            &one_face_range(),
            0.04,
        );
        assert_eq!(section.chunks.len(), 1);
        assert_eq!(section.light_indices, vec![0]);
        assert_eq!(ranges[0], (0, 1));
    }

    // ---- new tests (Task 4) ----------------------------------------------

    /// Scope case 2: N ≤ cap overlapping animated lights → exactly one chunk
    /// containing all N indices.
    #[test]
    fn n_le_cap_animated_lights_emits_single_chunk() {
        // Four lights (= cap), all covering the face.
        let n = MAX_ANIMATED_LIGHTS_PER_CHUNK;
        let lights: Vec<_> = (0..n).map(|_| mk_animated_light()).collect();
        let influence: Vec<_> = (0..n).map(|_| mk_inf(0.5, 0.5, 5.0)).collect();

        let bvh = make_bvh_with_one_leaf();
        let envelope = AnimatedBakedLights::from_parallel_slices(&lights, &influence);
        let (section, ranges) = build_animated_light_chunks(
            &bvh,
            &envelope,
            &[chart_xz_plane()],
            &one_face_range(),
            0.04,
        );

        assert_eq!(section.chunks.len(), 1);
        assert_eq!(section.chunks[0].index_count as usize, n);
        assert_eq!(section.light_indices.len(), n);
        let mut got = section.light_indices.clone();
        got.sort();
        let expected: Vec<u32> = (0..n as u32).collect();
        assert_eq!(got, expected);
        assert_eq!(ranges[0].1, 1);
    }

    /// Scope case 3: > cap overlapping animated lights → subdivision produces
    /// multiple chunks, none exceeding the cap.
    #[test]
    fn over_cap_overlapping_animated_lights_subdivide() {
        // 6 lights, each concentrated in a different (u,v) quadrant so
        // subdivision actually shrinks the per-chunk overlap set.
        let uv_centers = [
            (0.1, 0.1),
            (0.9, 0.1),
            (0.1, 0.9),
            (0.9, 0.9),
            (0.5, 0.1),
            (0.5, 0.9),
        ];
        let origin = Vec3::ZERO;
        let lights: Vec<_> = uv_centers.iter().map(|_| mk_animated_light()).collect();
        let influence: Vec<_> = uv_centers
            .iter()
            .map(|&(u, v)| {
                let p = tilted_world_point(origin, u, v);
                InfluenceRecord {
                    center: [p.x, p.y, p.z],
                    // Small radius: in world meters on the tilted face a 0.2
                    // sphere covers roughly one quadrant but not the opposite.
                    radius: 0.2,
                }
            })
            .collect();

        let bvh = make_bvh_with_one_leaf();
        let envelope = AnimatedBakedLights::from_parallel_slices(&lights, &influence);
        let (section, ranges) = build_animated_light_chunks(
            &bvh,
            &envelope,
            &[chart_tilted(origin)],
            &one_face_range(),
            0.01, // 1 cm / texel — floor not triggered at 1 m face extent.
        );

        assert!(
            section.chunks.len() > 1,
            "expected subdivision, got {} chunk(s)",
            section.chunks.len()
        );
        for c in &section.chunks {
            assert!(
                (c.index_count as usize) <= MAX_ANIMATED_LIGHTS_PER_CHUNK,
                "chunk {:?} exceeds cap",
                c
            );
        }
        assert_eq!(ranges[0].1 as usize, section.chunks.len());
    }

    /// Regression: an axis-aligned planar face (walls/floors/ceilings — the
    /// common case) must subdivide. Earlier the builder applied a
    /// world-AABB min-extent floor that tripped on the planar face's
    /// zero-thickness dimension and defeated subdivision entirely.
    #[test]
    fn axis_aligned_planar_face_subdivides() {
        // 4 lights, one in each quadrant of a 1m × 1m XZ-plane face.
        let uv_centers = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
        let lights: Vec<_> = uv_centers.iter().map(|_| mk_animated_light()).collect();
        let influence: Vec<_> = uv_centers
            .iter()
            .map(|&(u, v)| InfluenceRecord {
                center: [u, 0.0, v],
                radius: 0.2,
            })
            .collect();
        // Pack two more overlapping lights at the same centers so the root
        // rect sees > cap and must subdivide.
        let mut all_lights = lights.clone();
        let mut all_influence = influence.clone();
        all_lights.push(mk_animated_light());
        all_influence.push(InfluenceRecord {
            center: [0.5, 0.0, 0.5],
            radius: 1.0,
        });

        let bvh = make_bvh_with_one_leaf();
        let envelope = AnimatedBakedLights::from_parallel_slices(&all_lights, &all_influence);
        let (section, _ranges) = build_animated_light_chunks(
            &bvh,
            &envelope,
            &[chart_xz_plane()],
            &one_face_range(),
            0.01,
        );

        assert!(
            section.chunks.len() > 1,
            "planar face must subdivide; got {} chunk(s)",
            section.chunks.len()
        );
        for c in &section.chunks {
            assert!((c.index_count as usize) <= MAX_ANIMATED_LIGHTS_PER_CHUNK);
        }
    }

    /// Scope case 4: min-extent floor forces a single chunk to exceed the cap
    /// rather than infinite-loop. Using a texel density equal to the face
    /// extent forces the very first recursion to trip the floor.
    #[test]
    fn min_extent_floor_emits_single_overfull_chunk() {
        let n_lights = MAX_ANIMATED_LIGHTS_PER_CHUNK + 3;
        let lights: Vec<_> = (0..n_lights).map(|_| mk_animated_light()).collect();
        let influence: Vec<_> = (0..n_lights).map(|_| mk_inf(0.5, 0.5, 5.0)).collect();

        let bvh = make_bvh_with_one_leaf();
        // texel density = 1.0 m/texel so `min_uv_extent >= uv_extent` on the
        // root rect; builder must NOT recurse.
        let envelope = AnimatedBakedLights::from_parallel_slices(&lights, &influence);
        let (section, _ranges) = build_animated_light_chunks(
            &bvh,
            &envelope,
            &[chart_xz_plane()],
            &one_face_range(),
            1.0,
        );

        assert_eq!(section.chunks.len(), 1);
        assert_eq!(section.chunks[0].index_count as usize, n_lights);
        assert_eq!(section.light_indices.len(), n_lights);
    }

    /// `bake_only` animated lights participate in weight-map compose at
    /// runtime (retroactive Spec 1 change, see plan Settled decisions), so
    /// the builder MUST emit a chunk for them. Only `!is_dynamic &&
    /// animation.is_some()` with a bounded influence radius gates inclusion.
    #[test]
    fn bake_only_animated_light_produces_a_chunk() {
        let lights = vec![mk_bake_only_light()];
        let influence = vec![mk_inf(0.5, 0.5, 5.0)];
        let bvh = make_bvh_with_one_leaf();
        let envelope = AnimatedBakedLights::from_parallel_slices(&lights, &influence);
        let (section, ranges) = build_animated_light_chunks(
            &bvh,
            &envelope,
            &[chart_xz_plane()],
            &one_face_range(),
            0.04,
        );
        assert_eq!(
            section.chunks.len(),
            1,
            "bake_only animated light must participate in weight-map compose",
        );
        assert_eq!(section.light_indices, vec![0]);
        assert_eq!(ranges[0].1, 1);
    }

    /// Scope case 7: animated-flagged `is_dynamic` lights are not treated as
    /// animated.
    #[test]
    fn dynamic_animated_light_is_skipped() {
        let lights = vec![mk_dynamic_light()];
        let bvh = make_bvh_with_one_leaf();
        let envelope = AnimatedBakedLights::from_lights(&lights);
        assert!(
            envelope.is_empty(),
            "is_dynamic light must not enter the envelope"
        );
        let (section, ranges) = build_animated_light_chunks(
            &bvh,
            &envelope,
            &[chart_xz_plane()],
            &one_face_range(),
            0.04,
        );
        assert!(section.chunks.is_empty());
        assert_eq!(ranges[0].1, 0);
    }

    /// Scope case 8: emitted u32 indices index into the AnimatedBakedLights
    /// namespace (positions inside the envelope). `is_dynamic` entries are
    /// dropped by `AnimatedBakedLights::from_lights`, so they never receive a
    /// slot. `bake_only` animated lights ARE retained — see
    /// `bake_only_animated_light_produces_a_chunk`.
    #[test]
    fn index_namespace_matches_envelope_slot_positions() {
        // Source list: [animated, dynamic, animated, dynamic, animated].
        // Envelope keeps positions 0, 2, 4 → slot indices 0, 1, 2.
        let lights = vec![
            mk_animated_light(),
            mk_dynamic_light(),
            mk_animated_light(),
            mk_dynamic_light(),
            mk_animated_light(),
        ];

        let bvh = make_bvh_with_one_leaf();
        let envelope = AnimatedBakedLights::from_lights(&lights);
        assert_eq!(envelope.len(), 3);
        let (section, _ranges) = build_animated_light_chunks(
            &bvh,
            &envelope,
            &[chart_xz_plane()],
            &one_face_range(),
            0.04,
        );

        assert!(!section.light_indices.is_empty());
        for &idx in &section.light_indices {
            assert!(
                matches!(idx, 0 | 1 | 2),
                "emitted index {idx} outside envelope slot positions {{0, 1, 2}}"
            );
        }
        let mut seen: Vec<u32> = section.light_indices.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen, vec![0, 1, 2]);
    }

    /// Scope case 9: two builds on identical input produce byte-identical
    /// `AnimatedLightChunks` sections AND byte-identical `BvhSection`s (the
    /// latter covers the stamped `chunk_range_*` fields).
    #[test]
    fn determinism_two_builds_byte_identical() {
        // Non-trivial input — subdivision + multiple chunks.
        let uv_centers = [(0.2, 0.2), (0.8, 0.2), (0.2, 0.8), (0.8, 0.8), (0.5, 0.5)];
        let origin = Vec3::ZERO;
        let lights: Vec<_> = uv_centers.iter().map(|_| mk_animated_light()).collect();
        let influence: Vec<_> = uv_centers
            .iter()
            .map(|&(u, v)| {
                let p = tilted_world_point(origin, u, v);
                InfluenceRecord {
                    center: [p.x, p.y, p.z],
                    radius: 0.25,
                }
            })
            .collect();
        let chart = chart_tilted(origin);
        let face_ranges = one_face_range();

        let envelope_a = AnimatedBakedLights::from_parallel_slices(&lights, &influence);
        let bvh_a = make_bvh_with_one_leaf();
        let (section_a, ranges_a) = build_animated_light_chunks(
            &bvh_a,
            &envelope_a,
            std::slice::from_ref(&chart),
            &face_ranges,
            0.01,
        );

        let envelope_b = AnimatedBakedLights::from_parallel_slices(&lights, &influence);
        let bvh_b = make_bvh_with_one_leaf();
        let (section_b, ranges_b) = build_animated_light_chunks(
            &bvh_b,
            &envelope_b,
            std::slice::from_ref(&chart),
            &face_ranges,
            0.01,
        );

        assert_eq!(section_a.to_bytes(), section_b.to_bytes());
        // Per-leaf range table must be identical across builds — covers what
        // the on-disk stamped `chunk_range_*` fields will carry.
        assert_eq!(ranges_a, ranges_b);
    }

    /// Acceptance invariant: every leaf owns a contiguous range into the chunk
    /// array. No overlap. Sum of per-leaf `chunk_range_count` equals total
    /// chunk count.
    #[test]
    fn bvh_leaf_chunk_ranges_are_contiguous_and_cover_all_chunks() {
        // Four faces/leaves. Face 0 → one chunk. Face 1 → zero (no overlap).
        // Face 2 → subdivision (multiple chunks). Face 3 → zero.
        // Tilted charts keep the projected world AABB non-degenerate so the
        // min-extent floor does not short-circuit subdivision on face 2.
        let origins = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(20.0, 0.0, 0.0),
            Vec3::new(30.0, 0.0, 0.0),
        ];
        let charts: Vec<Chart> = origins.iter().map(|&o| chart_tilted(o)).collect();

        // Six lights total: 1 for face 0, 5 for face 2 (> cap so subdivision
        // is forced).
        let lights: Vec<_> = (0..6).map(|_| mk_animated_light()).collect();
        let p0 = tilted_world_point(origins[0], 0.5, 0.5);
        let p2a = tilted_world_point(origins[2], 0.1, 0.1);
        let p2b = tilted_world_point(origins[2], 0.9, 0.1);
        let p2c = tilted_world_point(origins[2], 0.1, 0.9);
        let p2d = tilted_world_point(origins[2], 0.9, 0.9);
        let p2e = tilted_world_point(origins[2], 0.5, 0.5);
        let influence = vec![
            InfluenceRecord {
                center: [p0.x, p0.y, p0.z],
                radius: 1.5,
            },
            InfluenceRecord {
                center: [p2a.x, p2a.y, p2a.z],
                radius: 0.2,
            },
            InfluenceRecord {
                center: [p2b.x, p2b.y, p2b.z],
                radius: 0.2,
            },
            InfluenceRecord {
                center: [p2c.x, p2c.y, p2c.z],
                radius: 0.2,
            },
            InfluenceRecord {
                center: [p2d.x, p2d.y, p2d.z],
                radius: 0.2,
            },
            InfluenceRecord {
                center: [p2e.x, p2e.y, p2e.z],
                radius: 0.2,
            },
        ];

        let face_ranges = n_face_ranges(4);
        let bvh = make_bvh_with_n_leaves(4);

        let envelope = AnimatedBakedLights::from_parallel_slices(&lights, &influence);
        let (section, ranges) =
            build_animated_light_chunks(&bvh, &envelope, &charts, &face_ranges, 0.01);

        // Invariant: total = sum of per-leaf counts.
        let sum: u32 = ranges.iter().map(|r| r.1).sum();
        assert_eq!(sum as usize, section.chunks.len());

        // Invariant: ranges do not overlap AND are contiguous when we ignore
        // zero-count leaves (zero-count leaves are permitted anywhere and
        // carry a `chunk_range_start` equal to the next emit-point at their
        // turn in the walk).
        let mut expected_next: u32 = 0;
        for &(start, count) in &ranges {
            assert_eq!(
                start, expected_next,
                "leaf chunk_range_start {start} does not abut previous end {expected_next}",
            );
            expected_next = start + count;
        }
        assert_eq!(expected_next as usize, section.chunks.len());

        // Invariant: face-0 leaf has exactly one chunk; face-2 leaf has more
        // than one; faces 1 and 3 have zero.
        assert_eq!(ranges[0].1, 1);
        assert_eq!(ranges[1].1, 0);
        assert!(
            ranges[2].1 > 1,
            "expected face-2 leaf to have subdivided chunks, got {}",
            ranges[2].1
        );
        assert_eq!(ranges[3].1, 0);
    }
}
