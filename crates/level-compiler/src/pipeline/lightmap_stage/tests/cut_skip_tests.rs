//! Exact skips on a cut face through the stage: the chart cull drops one dark
//! sub-chart, tiles anchor per sub-chart, and overlap twins stay identical.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::collections::HashMap;

use super::*;
use crate::chart_raster::{CHART_PADDING_TEXELS, chart_interior_dims, chart_texel_world_position};
use crate::lightmap_layer::{ExactSkips, LightmapLayer, SkipStats};

/// A sub-chart's face and one of its interior texels.
type SubTexel = (usize, [i32; 2]);

/// The cut floor's atlas and trace inputs, prepared as `bake_cut_geometry`
/// prepares them; chart planning never reads the lights.
struct CutAtlas {
    geometry: GeometryResult,
    primitives: Vec<BvhPrimitive>,
    prepared: lightmap_bake::PreparedAtlas,
}

impl CutAtlas {
    fn new(lights: &[MapLight]) -> Self {
        let config = config(false);
        let mut geometry = horizontal_quads(&[FLOOR]);
        let prepared = lightmap_bake::prepare_atlas_within(
            &mut geometry,
            &StaticBakedLights::from_lights(lights),
            config.lightmap_density,
            &[],
            lightmap_bake::BlockOrdering::by_cell_id(config.direction_texel_scale),
            CUT_POOL_EDGE,
            &BakeControl::unrestricted(),
        )
        .expect("cut fixture prepares");
        let (_, primitives, _) = build_bvh(&geometry).expect("cut fixture BVH");
        Self {
            geometry,
            primitives,
            prepared,
        }
    }

    fn charts(&self) -> &[lightmap_bake::Chart] {
        &self.prepared.charts
    }

    fn cut_faces(&self) -> std::ops::Range<usize> {
        self.prepared
            .cut_parents
            .iter()
            .find(|cut| cut.parent == 0)
            .expect("the floor is cut")
            .faces
            .clone()
    }

    /// World x and z ranges of a chart's texel centres.
    fn texel_span(&self, face: usize) -> ([f32; 2], [f32; 2]) {
        let chart = &self.charts()[face];
        let (width, height) = chart_interior_dims(chart);
        let first = chart_texel_world_position(chart, 0, 0);
        let last = chart_texel_world_position(chart, width - 1, height - 1);
        (
            [first.x.min(last.x), first.x.max(last.x)],
            [first.z.min(last.z), first.z.max(last.z)],
        )
    }

    /// Every parent-grid texel two sub-charts both cover, as each one's face
    /// and interior texel.
    fn twins(&self) -> Vec<(SubTexel, SubTexel)> {
        let faces: Vec<usize> = self.cut_faces().collect();
        let span = |face: usize| {
            let chart = &self.charts()[face];
            let origin = chart.window.expect("a sub-chart").origin.map(|o| o as i32);
            let (width, height) = chart_interior_dims(chart);
            (
                origin,
                [origin[0]..origin[0] + width, origin[1]..origin[1] + height],
            )
        };
        let mut twins = Vec::new();
        for (i, &a) in faces.iter().enumerate() {
            for &b in &faces[i + 1..] {
                let ((oa, sa), (ob, sb)) = (span(a), span(b));
                let u = sa[0].start.max(sb[0].start)..sa[0].end.min(sb[0].end);
                let v = sa[1].start.max(sb[1].start)..sa[1].end.min(sb[1].end);
                for gy in v.clone() {
                    for gx in u.clone() {
                        twins.push(((a, [gx - oa[0], gy - oa[1]]), (b, [gx - ob[0], gy - ob[1]])));
                    }
                }
            }
        }
        twins
    }

    /// The atlas layer and within-layer index of a chart's interior texel.
    fn atlas_texel(&self, face: usize, [x, y]: [i32; 2]) -> (u32, u32) {
        let placement = self.prepared.placements[face];
        let padding = CHART_PADDING_TEXELS as i32;
        let (ax, ay) = (
            placement.x as i32 + padding + x,
            placement.y as i32 + padding + y,
        );
        (
            placement.layer,
            ay as u32 * self.prepared.atlas_width + ax as u32,
        )
    }

    /// Every cached partition record of `lights`, by (light, layer, index).
    fn cached_records(
        &self,
        cache: &StageCache,
        lights: &[MapLight],
    ) -> HashMap<(usize, u32, u32), u32> {
        let config = config(false);
        let shared = SharedAtlas {
            charts: &self.prepared.charts,
            placements: &self.prepared.placements,
            atlas_width: self.prepared.atlas_width,
            atlas_height: self.prepared.atlas_height,
            layout: &self.prepared.layout,
        };
        let mut records = HashMap::new();
        for (index, light) in lights.iter().enumerate() {
            for layer in 0..self.prepared.layer_count {
                let key = CacheKey::new(
                    "lightmap_layer",
                    lightmap_layer::LAYER_FORMAT_VERSION,
                    &lightmap_layer::layer_input_hash(
                        light,
                        &shared,
                        &self.primitives,
                        &self.geometry,
                        config.lightmap_density,
                        test_args().soft_shadow_samples,
                        config.penumbra_floor,
                        layer,
                    ),
                );
                let bytes = cache.get(&key).expect("the stage cached every partition");
                let partition = LightmapLayer::from_bytes(&bytes).expect("a partition decodes");
                for texel in partition.texels {
                    records.insert((index, layer, texel.idx), texel.raw_visibility.to_bits());
                }
            }
        }
        records
    }
}

// On the 2 × 2-cut floor, a light whose range reaches the +u sub-chart's
// padded bounds but none of its texel centres drops that sub-chart alone
// through the chart cull; a broad light clears tiles in every sub-chart,
// among them a tile holding overlap texels. The stage's bytes equal a bake
// with both culls off, and every overlap twin records the same visibility,
// bit for bit, with both culls on.
#[test]
fn cut_face_drops_only_its_dark_sub_chart_and_keeps_overlap_twins_identical_with_the_culls_on() {
    let placeholder = [over([12.0, 8.0, 12.0], 40.0, 2.0)];
    let atlas = CutAtlas::new(&placeholder);
    let faces: Vec<usize> = atlas.cut_faces().collect();
    assert!(faces.len() >= 4, "the floor cuts 2 × 2: {faces:?}");
    // The sub-chart farthest along +x, then nearest −z.
    let dark = *faces
        .iter()
        .max_by(|&&a, &&b| {
            let (sa, sb) = (atlas.texel_span(a), atlas.texel_span(b));
            sa.0[0]
                .total_cmp(&sb.0[0])
                .then(sb.1[0].total_cmp(&sa.1[0]))
        })
        .unwrap();
    let (dark_x, dark_z) = atlas.texel_span(dark);
    // 1 m up, 5.125 m short of the dark sub-chart's first texel column: its
    // range passes the 0.5 m padded bounds and stops short of the centres.
    let mut corner = over(
        [
            f64::from(dark_x[0]) - 5.125,
            1.0,
            f64::from(dark_z[0]) + 3.0,
        ],
        4.9,
        20.0,
    );
    corner.light_size = 0.25;
    let mut broad = over([12.0, 8.0, 12.0], 40.0, 2.0);
    broad.light_size = 0.25;
    let lights = [broad, corner];

    let dir = fresh_cache_dir("cut_skip");
    let cache = StageCache::new(&dir).expect("cut skip cache");
    let culled_stats = SkipStats::leak();
    let unculled_stats = SkipStats::leak();
    let culled = bake_cut(
        &[FLOOR],
        &lights,
        CutRun {
            cache: Some(&cache),
            exact_skips: ExactSkips::ON.tallied(culled_stats),
            ..CutRun::default()
        },
    );
    let unculled = bake_cut(
        &[FLOOR],
        &lights,
        CutRun {
            exact_skips: ExactSkips::OFF.tallied(unculled_stats),
            ..CutRun::default()
        },
    );
    assert!(
        culled.lightmap == unculled.lightmap,
        "lightmap bytes differ"
    );
    assert!(
        culled.section == unculled.section,
        "shadowmask bytes differ"
    );

    // Only the dark sub-chart is dropped, and only by the chart cull.
    let walked = |stats: &SkipStats, light: &MapLight| {
        let origin = light.origin.as_vec3().to_array();
        let mut faces: Vec<usize> = stats
            .charts()
            .into_iter()
            .filter(|chart| chart.light_origin == origin)
            .map(|chart| chart.face)
            .collect();
        faces.sort_unstable();
        faces
    };
    assert_eq!(culled_stats.dropped_pairs(), 1);
    let (with_cull, without) = (
        walked(culled_stats, &lights[1]),
        walked(unculled_stats, &lights[1]),
    );
    let dropped: Vec<usize> = without
        .iter()
        .copied()
        .filter(|face| !with_cull.contains(face))
        .collect();
    assert_eq!(dropped, [dark], "walked {with_cull:?} of {without:?}");
    assert!(!with_cull.is_empty(), "the corner light lights a sub-chart");

    // Overlap twins, with both culls on, read back from the stage's cache.
    let twins = atlas.twins();
    assert!(twins.len() > 100, "the cuts overlap: {}", twins.len());
    let records = atlas.cached_records(&cache, &lights);
    for &((a, ta), (b, tb)) in &twins {
        let ((la, ia), (lb, ib)) = (atlas.atlas_texel(a, ta), atlas.atlas_texel(b, tb));
        for light in 0..lights.len() {
            assert_eq!(
                records.get(&(light, la, ia)),
                records.get(&(light, lb, ib)),
                "light {light}: twins {a}:{ta:?} and {b}:{tb:?}"
            );
        }
    }

    // A clear tile of the broad light's that holds overlap texels.
    let broad_origin = lights[0].origin.as_vec3().to_array();
    let beside_overlap = culled_stats
        .charts()
        .into_iter()
        .filter(|chart| chart.light_origin == broad_origin)
        .any(|chart| {
            chart.clear_tiles().any(|tile| {
                let [x0, x1, y0, y1] = chart.tile_texels(tile);
                twins.iter().any(|&(first, second)| {
                    [first, second].iter().any(|&(face, [x, y])| {
                        face == chart.face && (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
                    })
                })
            })
        });
    assert!(beside_overlap, "a clear tile holds overlap texels");
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
}
