// Pre-cut lower bound on the lightmap stage's encoded irradiance bytes.
// See: context/lib/build_pipeline.md §Compiler pipeline

use super::{Chart, LightmapBakeError};
use crate::chart_raster::CHART_PADDING_TEXELS;
use crate::geometry::GeometryResult;

/// Uncut charts pay their padded extent. A cut face pays only its polygon's
/// projected texel area: windows missing the polygon never become blocks.
/// This runs before planning the cut grid, so extreme density remains cheap.
pub(crate) fn minimum_working_set_bytes(
    geometry: &GeometryResult,
    charts: &[Chart],
    pool_edge: u32,
    uncompressed_irradiance: bool,
) -> u64 {
    let mut area = 0u64;
    for (face, chart) in charts.iter().enumerate() {
        let texels = if chart.width_texels <= pool_edge && chart.height_texels <= pool_edge {
            u64::from(chart.width_texels) * u64::from(chart.height_texels)
        } else {
            let range = geometry.face_index_ranges[face];
            let indices = &geometry.geometry.indices
                [range.index_offset as usize..(range.index_offset + range.index_count) as usize];
            let u = chart.u_axis.as_dvec3();
            let v = chart.v_axis.as_dvec3();
            let projected_area: f64 = indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|tri| {
                    let p = |i: u32| {
                        glam::Vec3::from(geometry.geometry.vertices[i as usize].position).as_dvec3()
                    };
                    let a = p(tri[1]) - p(tri[0]);
                    let b = p(tri[2]) - p(tri[0]);
                    (a.dot(u) * b.dot(v) - a.dot(v) * b.dot(u)).abs() * 0.5
                })
                .sum();
            let padding = 2 * CHART_PADDING_TEXELS;
            let pitch_u = f64::from(chart.width_texels.saturating_sub(padding))
                / f64::from(chart.uv_extent[0]);
            let pitch_v = f64::from(chart.height_texels.saturating_sub(padding))
                / f64::from(chart.uv_extent[1]);
            // Round down: this is a lower bound, never a packing estimate.
            (projected_area * pitch_u * pitch_v).floor() as u64
        };
        area = area.saturating_add(texels);
    }
    area.saturating_mul(if uncompressed_irradiance { 8 } else { 1 })
}

pub(super) fn check_minimum_working_set(
    geometry: &GeometryResult,
    charts: &[Chart],
    pool_edge: u32,
    budget: u64,
    uncompressed_irradiance: bool,
) -> Result<(), LightmapBakeError> {
    let minimum = minimum_working_set_bytes(geometry, charts, pool_edge, uncompressed_irradiance);
    if minimum > budget {
        return Err(LightmapBakeError::WorkingSetMinimumExceeded { minimum, budget });
    }
    Ok(())
}
