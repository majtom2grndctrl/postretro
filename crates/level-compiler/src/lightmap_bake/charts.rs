// Per-face lightmap chart planning: projection axes, extents, and texel dimensions.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::HashSet;

use glam::Vec3;

use super::LightmapBakeError;
use crate::chart_raster::CHART_PADDING_TEXELS;
use crate::geometry::GeometryResult;
use crate::map_data::MapLightmapScaleRegion;

/// Face-local 2D chart plan. `pub` so the animated-light-chunks builder can reuse the same
/// per-face projection without duplicating the unwrap logic.
#[derive(Debug, Clone)]
pub struct Chart {
    pub origin: Vec3,
    pub u_axis: Vec3,
    pub v_axis: Vec3,
    pub uv_min: [f32; 2],
    pub uv_extent: [f32; 2],
    pub normal: Vec3,
    /// Includes padding.
    pub width_texels: u32,
    pub height_texels: u32,
    /// BSP leaf this chart's face belongs to. The multi-bin packer keeps all of
    /// one leaf's charts on a single atlas array layer (a leaf is the runtime
    /// draw/visibility unit, so its charts must share a layer to avoid a
    /// per-face layer switch in the hot path).
    pub leaf_index: u32,
}

pub(super) fn plan_charts(
    geom: &GeometryResult,
    texel_density: f32,
    scale_regions: &[MapLightmapScaleRegion],
) -> Result<Vec<Chart>, LightmapBakeError> {
    let global_density = texel_density.max(1.0e-4);
    let section = &geom.geometry;

    let mut charts = Vec::with_capacity(section.faces.len());
    for (face_index, range) in geom.face_index_ranges.iter().enumerate() {
        // Charts map 1:1 with faces, so the chart's leaf is its face's leaf.
        let leaf_index = section.faces[face_index].leaf_index;
        let start = range.index_offset as usize;
        // A degenerate face still belongs to its leaf, so its placeholder chart
        // carries the real `leaf_index` — keeping the leaf's charts cohesive in
        // the packer even when one face is degenerate.
        if range.index_count < 3 {
            charts.push(empty_chart_for_leaf(leaf_index));
            continue;
        }

        let i0 = section.indices[start] as usize;
        let i1 = section.indices[start + 1] as usize;
        let i2 = section.indices[start + 2] as usize;
        let p0 = Vec3::from(section.vertices[i0].position);
        let p1 = Vec3::from(section.vertices[i1].position);
        let p2 = Vec3::from(section.vertices[i2].position);

        let edge1 = p1 - p0;
        let edge2 = p2 - p0;
        let normal_raw = edge1.cross(edge2);
        if normal_raw.length_squared() < 1.0e-12 {
            charts.push(empty_chart_for_leaf(leaf_index));
            continue;
        }
        // Prefer the stored vertex normal: the cross-product direction depends on winding and can
        // invert the Lambert term if it disagrees with the stored normal.
        let stored_normal_raw = section.vertices[i0].decode_normal();
        let stored_normal = Vec3::new(
            stored_normal_raw[0],
            stored_normal_raw[1],
            stored_normal_raw[2],
        );
        let normal = if stored_normal.length_squared() > 0.5 {
            stored_normal.normalize()
        } else {
            normal_raw.normalize()
        };

        let u_axis = edge1.normalize_or_zero();
        if u_axis.length_squared() < 0.5 {
            charts.push(empty_chart_for_leaf(leaf_index));
            continue;
        }
        let v_axis = normal.cross(u_axis).normalize_or_zero();
        if v_axis.length_squared() < 0.5 {
            charts.push(empty_chart_for_leaf(leaf_index));
            continue;
        }

        let mut u_min = f32::INFINITY;
        let mut u_max = f32::NEG_INFINITY;
        let mut v_min = f32::INFINITY;
        let mut v_max = f32::NEG_INFINITY;

        let mut seen_verts: Vec<usize> = Vec::new();
        let mut seen_set: HashSet<usize> = HashSet::new();
        let end = start + range.index_count as usize;
        let mut tri = start;
        while tri + 3 <= end {
            for j in 0..3 {
                let vi = section.indices[tri + j] as usize;
                if seen_set.insert(vi) {
                    seen_verts.push(vi);
                }
            }
            tri += 3;
        }

        for &vi in &seen_verts {
            let p = Vec3::from(section.vertices[vi].position);
            let rel = p - p0;
            let u = rel.dot(u_axis);
            let v = rel.dot(v_axis);
            if u < u_min {
                u_min = u;
            }
            if u > u_max {
                u_max = u;
            }
            if v < v_min {
                v_min = v;
            }
            if v > v_max {
                v_max = v;
            }
        }

        let density = resolved_chart_density(p0, global_density, scale_regions);
        if !density.is_finite() || density <= 0.0 {
            return Err(LightmapBakeError::InvalidChartDensity {
                face_index,
                density_m_per_texel: density,
            });
        }
        let u_extent = (u_max - u_min).max(density);
        let v_extent = (v_max - v_min).max(density);

        let width_texels = chart_texel_dimension(u_extent, density, face_index, "width")?;
        let height_texels = chart_texel_dimension(v_extent, density, face_index, "height")?;

        charts.push(Chart {
            origin: p0,
            u_axis,
            v_axis,
            uv_min: [u_min, v_min],
            uv_extent: [u_extent, v_extent],
            normal,
            width_texels,
            height_texels,
            leaf_index,
        });
    }
    Ok(charts)
}

/// Convert one finite chart extent to its padded texel dimension without a
/// lossy float-to-`u32` conversion. Extremely small effective densities can
/// make this quotient infinite even when their authored scale was finite.
fn chart_texel_dimension(
    extent_m: f32,
    density_m_per_texel: f32,
    face_index: usize,
    axis: &'static str,
) -> Result<u32, LightmapBakeError> {
    let texels = (extent_m / density_m_per_texel).ceil();
    let padded_texels = texels + 2.0 * CHART_PADDING_TEXELS as f32;
    if !padded_texels.is_finite() || padded_texels < 1.0 || padded_texels >= u32::MAX as f32 {
        return Err(LightmapBakeError::ChartDimensionOverflow {
            face_index,
            axis,
            extent_m,
            density_m_per_texel,
            texels: padded_texels,
        });
    }
    Ok(padded_texels.max(1.0) as u32)
}

/// Resolve one chart's density from its first face vertex. Region iteration is
/// deliberately source order: later matching brush entities override earlier
/// ones without sorting or storing raw-region identity in cache keys.
pub(super) fn resolved_chart_density(
    origin: Vec3,
    global_density: f32,
    scale_regions: &[MapLightmapScaleRegion],
) -> f32 {
    let mut density = global_density;
    for region in scale_regions {
        if origin.x >= region.min[0]
            && origin.x <= region.max[0]
            && origin.y >= region.min[1]
            && origin.y <= region.max[1]
            && origin.z >= region.min[2]
            && origin.z <= region.max[2]
        {
            density = global_density / region.scale;
        }
    }
    density
}

/// Placeholder chart for a degenerate face. Carries the face's `leaf_index` so
/// the packer keeps the leaf's charts cohesive even when one face degenerates.
pub(super) fn empty_chart_for_leaf(leaf_index: u32) -> Chart {
    Chart {
        origin: Vec3::ZERO,
        u_axis: Vec3::X,
        v_axis: Vec3::Y,
        uv_min: [0.0, 0.0],
        uv_extent: [0.0, 0.0],
        normal: Vec3::Y,
        width_texels: 1,
        height_texels: 1,
        leaf_index,
    }
}
