// Per-light layer and composited-section cache keys, plus cached-payload validation.
// See: context/lib/build_pipeline.md §Build Cache

use glam::DVec3;

use super::{
    LAYER_FORMAT_VERSION, LightmapLayer, SharedAtlas, atlas_layer_count, geometry_world_aabb,
    layer_influence_aabb,
};
use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::chart_interior_dims;
use crate::geometry::GeometryResult;
use crate::map_data::MapLight;

/// The atlas layout descriptor folded into a layer's cache key. Captures atlas
/// dimensions, resolved chart sampling extents/dimensions, per-chart
/// placements, and the cell-block table (order, extents, bake-layer origins,
/// direction scale) so an atlas repack (which shifts every placement), a block
/// reorder, or a per-surface density override invalidates all layers by
/// changing this fingerprint.
///
/// `ChartPlacement` does not derive `Serialize`, so this folds its `x`/`y`/`layer`
/// fields directly into the digest — the deterministically-derived proxy-bytes
/// fingerprint the animated-weight-map stage uses for its non-`Serialize` atlas
/// types. Every resolved per-chart sampling input is folded explicitly because
/// a scale-region edit can change texel world positions or resize a lone chart
/// while its 64² atlas and `(0, 0, 0)` placement remain unchanged. Raw region
/// definitions are intentionally not folded: equivalent resolved chart
/// outcomes share cache identity.
pub(crate) fn atlas_layout_fingerprint(atlas: &SharedAtlas<'_>) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&atlas.atlas_width.to_le_bytes());
    hasher.update(&atlas.atlas_height.to_le_bytes());
    hasher.update(&(atlas.charts.len() as u32).to_le_bytes());
    for chart in atlas.charts {
        for component in [chart.origin.x, chart.origin.y, chart.origin.z] {
            hasher.update(&component.to_le_bytes());
        }
        for component in [chart.u_axis.x, chart.u_axis.y, chart.u_axis.z] {
            hasher.update(&component.to_le_bytes());
        }
        for component in [chart.v_axis.x, chart.v_axis.y, chart.v_axis.z] {
            hasher.update(&component.to_le_bytes());
        }
        for component in chart.uv_min {
            hasher.update(&component.to_le_bytes());
        }
        for component in chart.uv_extent {
            hasher.update(&component.to_le_bytes());
        }
        for component in [chart.normal.x, chart.normal.y, chart.normal.z] {
            hasher.update(&component.to_le_bytes());
        }
        hasher.update(&chart.width_texels.to_le_bytes());
        hasher.update(&chart.height_texels.to_le_bytes());
    }
    hasher.update(&(atlas.placements.len() as u32).to_le_bytes());
    for p in atlas.placements {
        hasher.update(&p.x.to_le_bytes());
        hasher.update(&p.y.to_le_bytes());
        // Fold the atlas layer so a repack that moves a chart to a different
        // array layer (same x/y) still invalidates the per-light cache.
        hasher.update(&p.layer.to_le_bytes());
    }
    let layout = atlas.layout;
    hasher.update(&layout.direction_texel_scale.to_le_bytes());
    hasher.update(&(layout.blocks.len() as u32).to_le_bytes());
    for block in &layout.blocks {
        for field in [
            block.cell_id,
            block.width,
            block.height,
            block.layer,
            block.x,
            block.y,
        ] {
            hasher.update(&field.to_le_bytes());
        }
    }
    hasher.update(&(layout.chart_blocks.len() as u32).to_le_bytes());
    for block in &layout.chart_blocks {
        hasher.update(&block.to_le_bytes());
    }
    hasher.finalize().as_bytes().to_vec()
}

/// Hash the influence-bounded geometry slice for one light: the content of every
/// face whose AABB overlaps the light's influence AABB.
///
/// This is the whole-stage `GeometryResult` content hash *restricted* to the
/// influence-overlapping faces. Faces are gathered by AABB overlap against the
/// per-face `BvhPrimitive`s (the BVH is an accelerator only; iterating the
/// already-collected primitive slice yields the identical set), mapped back to
/// face identity, then taken in canonical `sort_key` order — NOT the post-`Bvh::
/// build` permutation — so the hash is decoupled from BVH build determinism.
/// Each face is hashed by its geometry (`index_offset..index_offset+index_count`
/// indices, plus the referenced vertices), the face content that actually
/// affects the bake's chart shapes and shadow queries.
///
/// Occlusion is local to the influence sphere (any occluder on a light→texel
/// segment is nearer than the texel, hence inside `falloff_range`), so the
/// falloff-AABB slice is a sound conservative dependency set.
pub(super) fn geometry_slice_hash(
    light: &MapLight,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    world_aabb: (DVec3, DVec3),
) -> Vec<u8> {
    let (inf_min, inf_max) = layer_influence_aabb(light, world_aabb);

    // Gather overlapping primitives, then sort by canonical `sort_key` so the
    // hash is independent of however the BVH permuted the slice.
    let mut overlapping: Vec<&BvhPrimitive> = primitives
        .iter()
        .filter(|p| aabb_overlaps(p, inf_min, inf_max))
        .collect();
    overlapping.sort_by_key(|p| p.sort_key);

    let section = &geometry.geometry;
    let mut hasher = blake3::Hasher::new();
    for prim in overlapping {
        // Map the primitive back to its face slice. `index_offset`/`index_count`
        // on the primitive are the same range stored in `face_index_ranges`.
        let start = prim.index_offset as usize;
        let end = start + prim.index_count as usize;
        hasher.update(&prim.sort_key.to_le_bytes());
        for i in start..end {
            let vi = section.indices[i];
            hasher.update(&vi.to_le_bytes());
            let v = &section.vertices[vi as usize];
            hasher.update(&bytemuck_f32x3(&v.position));
            hasher.update(&[v.uv[0].to_le_bytes(), v.uv[1].to_le_bytes()].concat());
            hasher.update(&v.normal_oct[0].to_le_bytes());
            hasher.update(&v.normal_oct[1].to_le_bytes());
        }
    }
    hasher.finalize().as_bytes().to_vec()
}

fn aabb_overlaps(prim: &BvhPrimitive, inf_min: DVec3, inf_max: DVec3) -> bool {
    let p_min = prim.aabb_min;
    let p_max = prim.aabb_max;
    (p_min[0] as f64) <= inf_max.x
        && (p_max[0] as f64) >= inf_min.x
        && (p_min[1] as f64) <= inf_max.y
        && (p_max[1] as f64) >= inf_min.y
        && (p_min[2] as f64) <= inf_max.z
        && (p_max[2] as f64) >= inf_min.z
}

fn bytemuck_f32x3(v: &[f32; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    for c in v {
        out.extend_from_slice(&c.to_le_bytes());
    }
    out
}

/// The full input fingerprint for one light's lightmap layer cache key.
///
/// Folds, under a fixed byte layout:
/// - the light's params (whole `MapLight`, fixed `postcard` encoding for the
///   key hash — `postcard` is used here only; the layer blob itself uses the
///   bytemuck codec),
/// - the influence-bounded geometry slice hash,
/// - `lightmap_density` + `area_sample_count`,
/// - the atlas layout descriptor (dims + per-chart placements).
/// - `target_layer`, so partitions with the same light and layout cannot alias.
///
/// Consumers pass this digest to `CacheKey::new("lightmap_layer",
/// LAYER_FORMAT_VERSION, &hash)` so the layer stage owns invalidation.
pub fn layer_input_hash(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    lightmap_density: f32,
    area_sample_count: u32,
    target_layer: u32,
) -> [u8; 32] {
    let world_aabb = geometry_world_aabb(geometry);

    let mut hasher = blake3::Hasher::new();
    hasher
        .update(&postcard::to_allocvec(light).expect("postcard serialize MapLight for layer key"));
    hasher.update(&geometry_slice_hash(
        light, primitives, geometry, world_aabb,
    ));
    hasher.update(&lightmap_density.to_le_bytes());
    hasher.update(&area_sample_count.to_le_bytes());
    hasher.update(&atlas_layout_fingerprint(atlas));
    hasher.update(&target_layer.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Validate a decoded cache partition against the current shared atlas. Cache
/// corruption and stale/mismatched payloads are callers' soft misses, never a
/// PRL-load error.
pub fn validate_layer_partition(
    partition: &LightmapLayer,
    atlas: &SharedAtlas<'_>,
    target_layer: u32,
) -> Result<(), String> {
    if partition.atlas_width != atlas.atlas_width || partition.atlas_height != atlas.atlas_height {
        return Err(format!(
            "dimensions {}x{} != {}x{}",
            partition.atlas_width, partition.atlas_height, atlas.atlas_width, atlas.atlas_height
        ));
    }
    let layer_count = atlas_layer_count(atlas);
    if partition.layer_count != layer_count {
        return Err(format!(
            "layer_count {} != {}",
            partition.layer_count, layer_count
        ));
    }
    if target_layer >= layer_count {
        return Err(format!(
            "target layer {target_layer} out of bounds for {layer_count} layers"
        ));
    }
    if partition.target_layer != target_layer {
        return Err(format!(
            "partition target layer {} != {target_layer}",
            partition.target_layer
        ));
    }
    let Some(plane) = (atlas.atlas_width as usize).checked_mul(atlas.atlas_height as usize) else {
        return Err("atlas dimensions overflow texel plane size".to_string());
    };
    if atlas.charts.len() != atlas.placements.len() {
        return Err(format!(
            "chart count {} != placement count {}",
            atlas.charts.len(),
            atlas.placements.len()
        ));
    }

    let mut covered = vec![false; plane];
    for (chart, placement) in atlas.charts.iter().zip(atlas.placements) {
        if placement.layer != target_layer || chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0
        {
            continue;
        }
        let (interior_width, interior_height) = chart_interior_dims(chart);
        let padding = crate::chart_raster::CHART_PADDING_TEXELS as i32;
        for y in 0..interior_height {
            for x in 0..interior_width {
                let atlas_x = placement.x as i32 + padding + x;
                let atlas_y = placement.y as i32 + padding + y;
                let expected_idx = atlas_y as u32 * atlas.atlas_width + atlas_x as u32;
                covered[expected_idx as usize] = true;
            }
        }
    }
    let mut previous = None;
    for (record_index, texel) in partition.texels.iter().enumerate() {
        let idx = texel.idx as usize;
        if idx >= plane {
            return Err(format!(
                "texel {record_index} idx {} out of bounds for {plane} texels",
                texel.idx
            ));
        }
        if !covered[idx] {
            return Err(format!(
                "texel {record_index} idx {} is outside the covered chart interiors on layer {target_layer}",
                texel.idx
            ));
        }
        let visibility = texel.raw_visibility;
        if !visibility.is_nan() && (!visibility.is_finite() || !(0.0..=1.0).contains(&visibility)) {
            return Err(format!(
                "texel {record_index} raw visibility {visibility} is outside 0..=1 or infinite"
            ));
        }
        if previous.is_some_and(|previous| texel.idx <= previous) {
            return Err(format!(
                "texel {record_index} idx {} is not strictly greater than previous idx {}",
                texel.idx,
                previous.unwrap()
            ));
        }
        previous = Some(texel.idx);
    }
    Ok(())
}

/// Validate a decoded composited-section memo against the current block
/// layout and encode configuration. A decodable but stale payload is a soft
/// cache miss.
pub fn validate_cached_lightmap_section(
    section: &postretro_level_format::lightmap::LightmapSection,
    atlas: &SharedAtlas<'_>,
    uncompressed_irradiance: bool,
) -> Result<(), String> {
    use postretro_level_format::lightmap::LightmapMode;

    let layout = atlas.layout;
    if section.direction_texel_scale != layout.direction_texel_scale {
        return Err(format!(
            "direction scale {} != {}",
            section.direction_texel_scale, layout.direction_texel_scale
        ));
    }
    let expected_irradiance_format =
        crate::lightmap_bake::irradiance_format(uncompressed_irradiance);
    if section.irradiance_format != expected_irradiance_format {
        return Err(format!(
            "irradiance format {} != {expected_irradiance_format}",
            section.irradiance_format
        ));
    }
    if section.mode != LightmapMode::Shadowed {
        return Err(format!(
            "lightmap mode {:?} != {:?}",
            section.mode,
            LightmapMode::Shadowed
        ));
    }
    if section.blocks.len() != layout.blocks.len() {
        return Err(format!(
            "block count {} != {}",
            section.blocks.len(),
            layout.blocks.len()
        ));
    }
    for (id, (stored, expected)) in section.blocks.iter().zip(&layout.blocks).enumerate() {
        if stored.cell_id != expected.cell_id
            || u32::from(stored.width) != expected.width
            || u32::from(stored.height) != expected.height
        {
            return Err(format!(
                "block {id} is cell {} {}x{}, expected cell {} {}x{}",
                stored.cell_id,
                stored.width,
                stored.height,
                expected.cell_id,
                expected.width,
                expected.height
            ));
        }
    }
    Ok(())
}

/// Build the cache key for the composited lightmap section — the second-level
/// memo that lets a no-edit rebuild skip the per-light layer reads, composite,
/// dilate, and BC6H encode and instead decode the section bytes directly.
///
/// The fold covers every input that determines the section bytes, under a fixed
/// unambiguous byte layout:
/// 1. `LAYER_FORMAT_VERSION` (u32 LE) — couples this key to the per-light layer
///    format the same way the private per-light `CacheKey.digest`s would; a
///    layer-format bump invalidates the section without reading the layer keys.
/// 2. light count (u32 LE) — so add/remove can never alias a reorder. The
///    fixed-width 32-byte hash records below already make a plain concatenation
///    injective, but folding the count is a cheap belt-and-suspenders guard.
/// 3. each light-layer `layer_input_hash` `[u8; 32]`, in the caller's exact
///    layer-major then global-static-light order (`ShadowType::Sdf` dropped).
///    Folding the input hashes mirrors folding the full cache keys: any
///    per-light input change (light params, geometry slice, density, atlas
///    layout, or target layer) flows through here.
/// 4. the complete prepared atlas layout fingerprint. This is required even
///    when the filtered light set is empty, because the all-Sdf fallback bytes
///    still depend on atlas dimensions.
/// 5. `texel_density` (f32 LE) — the `density` passed to `encode_section`.
///    Already folded into every `layer_input_hash` via `lightmap_density`, so
///    this is belt-and-suspenders (same rationale as the light-count fold).
/// 6. `uncompressed_irradiance` (1 byte, 0/1) — selects BC6H vs RGBA16F output.
/// 7. `direction_texel_scale` (u32 LE) — selects the post-composite direction
///    resolution. It also sets the block alignment, which the layout
///    fingerprint already covers, so a scale that changes the alignment
///    re-keys the per-light layers too.
///
/// `layer_input_hashes` must be supplied in the same filtered order the warm
/// composite loop uses; the helper does not re-derive or re-sort them.
pub fn section_input_hash(
    layer_input_hashes: &[[u8; 32]],
    atlas: &SharedAtlas<'_>,
    texel_density: f32,
    uncompressed_irradiance: bool,
    direction_texel_scale: u32,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&LAYER_FORMAT_VERSION.to_le_bytes());
    hasher.update(&(layer_input_hashes.len() as u32).to_le_bytes());
    for hash in layer_input_hashes {
        hasher.update(hash);
    }
    hasher.update(&atlas_layout_fingerprint(atlas));
    hasher.update(&texel_density.to_le_bytes());
    hasher.update(&[u8::from(uncompressed_irradiance)]);
    hasher.update(&direction_texel_scale.to_le_bytes());
    *hasher.finalize().as_bytes()
}
