// Transposed charts: the direction reduction and dilation sum in the chart's own
// order, so a chart reduces bit for bit whichever way it is stored.
// See: context/lib/build_pipeline.md §Compiler pipeline (atlas preparation)

use super::*;
use crate::chart_raster::ChartPlacement;
use crate::lightmap_bake::encode::encode_atlas_layer;

/// Four directions whose sum depends on the order they are added in:
/// `((a + b) + c) + d != ((a + c) + b) + d` in f32.
fn order_sensitive() -> [Vec3; 4] {
    [
        Vec3::new(1.0e8, 3.0, 0.7),
        Vec3::new(1.0, -1.0e8, 0.3),
        Vec3::new(-1.0e8, 7.0, 0.1),
        Vec3::new(0.25, 1.0e8, 1.0),
    ]
}

#[test]
fn a_transposed_two_by_two_reduces_to_the_same_direction_texel() {
    let [a, b, c, d] = order_sensitive();
    // Chart row-major [a b; c d], stored upright and turned.
    let upright = [a, b, c, d];
    let turned = [a, c, b, d];
    let coverage = [true; 4];
    // Sanity: the inputs sum differently in the two orders.
    assert_ne!(((a + b) + c) + d, ((a + c) + b) + d);

    let (expected, _) = reduce_direction_atlas(&upright, &coverage, &[false; 4], 2, 2, 1, 2);
    let (stored_turned, _) = reduce_direction_atlas(&turned, &coverage, &[true; 4], 2, 2, 1, 2);
    assert_eq!(
        expected[0].to_array().map(f32::to_bits),
        stored_turned[0].to_array().map(f32::to_bits),
        "a transposed chart reduces in its own order"
    );
    // Without the orientation the stored-turned texels sum in atlas order.
    let (ignored, _) = reduce_direction_atlas(&turned, &coverage, &[false; 4], 2, 2, 1, 2);
    assert_ne!(
        expected[0].to_array().map(f32::to_bits),
        ignored[0].to_array().map(f32::to_bits),
        "the fixture must be order-sensitive"
    );
}

/// A `width × height` padded chart buffer with its interior covered by
/// order-sensitive values, padding uncovered, as `bake_face_chart` returns,
/// plus one uncovered interior texel whose eight covered neighbours make the
/// dilation sum order-sensitive.
fn chart_buffer(width: u32, height: u32) -> CompositedAtlas {
    let mut chart = CompositedAtlas::zeroed(width, height, 1);
    let padding = crate::chart_raster::CHART_PADDING_TEXELS;
    let values = order_sensitive();
    for y in padding..height - padding {
        for x in padding..width - padding {
            let idx = (y * width + x) as usize;
            let v = values[(x * 3 + y * 5) as usize % 4] * (1.0 + 0.01 * (x + y) as f32);
            chart.irradiance[idx * 4..idx * 4 + 4].copy_from_slice(&[v.x, v.y, v.z, 1.0]);
            chart.direction[idx] = v.normalize();
            chart.coverage[idx] = true;
        }
    }
    let hole = (3 * width + 4) as usize;
    chart.irradiance[hole * 4..hole * 4 + 4].fill(0.0);
    chart.direction[hole] = Vec3::Y;
    chart.coverage[hole] = false;
    chart
}

/// The chart-local padded texels of `placement`'s chart in `atlas`, as bits.
fn chart_local(
    atlas: &CompositedAtlas,
    placement: &ChartPlacement,
    width: u32,
    height: u32,
) -> Vec<([u32; 4], [u32; 3], bool)> {
    let mut out = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let (ax, ay) = placement.padded_texel(x, y);
            let idx = (ay * atlas.atlas_width + ax) as usize;
            let irradiance: [f32; 4] = atlas.irradiance[idx * 4..idx * 4 + 4].try_into().unwrap();
            out.push((
                irradiance.map(f32::to_bits),
                atlas.direction[idx].to_array().map(f32::to_bits),
                atlas.coverage[idx],
            ));
        }
    }
    out
}

#[test]
fn a_transposed_chart_dilates_bit_for_bit_as_it_would_upright() {
    let (width, height) = (9, 6);
    let chart = chart_buffer(width, height);
    let upright = ChartPlacement::at(2, 2, 0);
    let turned = ChartPlacement {
        transposed: true,
        ..upright
    };
    let mut dilated = Vec::new();
    for placement in [upright, turned] {
        let mut atlas = CompositedAtlas::zeroed(16, 16, 1);
        scatter_chart_into_atlas(&chart, &placement, &mut atlas);
        atlas.dilate();
        dilated.push(chart_local(&atlas, &placement, width, height));
    }
    assert_eq!(dilated[0], dilated[1]);
    // The ring took values: dilation ran.
    assert!(dilated[0][0].2 && dilated[0][(width + 1) as usize].2);

    // Without the orientation, the hole's eight neighbours sum in atlas
    // order and drift: the fixture is order-sensitive.
    let mut atlas = CompositedAtlas::zeroed(16, 16, 1);
    scatter_chart_into_atlas(&chart, &turned, &mut atlas);
    atlas.transposed.fill(false);
    atlas.dilate();
    assert_ne!(dilated[0], chart_local(&atlas, &turned, width, height));
}

/// A `2.3 × 1` m quad: a non-square chart.
fn stretched_quad_geometry() -> GeometryResult {
    let mut geometry = unit_quad_geometry();
    for vertex in &mut geometry.geometry.vertices {
        vertex.position[0] *= 2.3;
    }
    geometry
}

/// Bake one chart upright and turned at the same even origin through the
/// monolithic bake and the per-layer encode: every pre-BC value read through
/// the chart's own grid matches, irradiance and direction alike.
#[test]
fn a_baked_chart_stored_transposed_reads_back_identical_through_its_own_grid() {
    let mut geometry = stretched_quad_geometry();
    let lights = vec![point_light_above()];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let light_refs: Vec<&MapLight> = static_lights.entries().iter().map(|e| e.light).collect();
    let prepared = prepare_atlas(&mut geometry, &static_lights, 0.1, &[]).unwrap();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    assert_eq!(prepared.charts.len(), 1);
    let chart = &prepared.charts[0];
    let (width, height) = (chart.width_texels, chart.height_texels);
    assert_ne!(width, height, "the fixture chart is not square");
    let edge = (width.max(height) + 2).div_ceil(4) * 4;

    let upright = ChartPlacement::at(2, 2, 0);
    let turned = ChartPlacement {
        transposed: true,
        ..upright
    };
    let mut planes = Vec::new();
    let mut directions = Vec::new();
    for placement in [upright, turned] {
        let atlas = bake_monolithic_atlas(
            &bvh,
            &primitives,
            &geometry,
            &light_refs,
            &prepared.charts,
            &[placement],
            edge,
            edge,
            1,
            8,
            crate::texel_footprint::PENUMBRA_FLOOR_DEFAULT,
        );
        planes.push(chart_local(&atlas, &placement, width, height));
        let (_, direction) = encode_atlas_layer(&atlas, true, 2);
        // Whole direction texels of the chart, in its own row-major order.
        let direction_width = edge / 2;
        let mut texels = Vec::new();
        for j in 0..height.div_ceil(2) {
            for i in 0..width.div_ceil(2) {
                let (ax, ay) = placement.padded_texel(2 * i, 2 * j);
                let at = ((ay / 2) * direction_width + ax / 2) as usize * 2;
                texels.push([direction[at], direction[at + 1]]);
            }
        }
        directions.push(texels);
    }
    assert!(planes[0].iter().any(|t| t.2), "the chart baked");
    assert_eq!(planes[0], planes[1], "irradiance and full-res direction");
    assert_eq!(directions[0], directions[1], "half-res direction");
}
