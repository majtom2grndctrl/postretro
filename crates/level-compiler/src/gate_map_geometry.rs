// Gate-map geometry harness: the pre-cut and post-cut geometry the bake tracers trace, built by the pipeline's own stage functions.
// Shared by the ray-traversal census tests and the coverage-proof gate-map tests.
// See: context/lib/build_pipeline.md §Ray traversal

use std::time::{Duration, Instant};

use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;

use crate::buried_lights::BuriedLights;
use crate::geometry::GeometryResult;
use crate::light_namespaces::StaticBakedLights;
use crate::pipeline::runtime_cells;

/// The two geometries the bake tracers trace, built by the pipeline's own
/// stage functions in its order: the pre-cut set (SH, delta and direct SH,
/// the chunk light list, the emitter tests) and the post-cut set (lightmap,
/// shadowmask, weight maps, reflection probes).
pub(crate) struct MapGeometries {
    pub(crate) pre_cut: GeometryResult,
    pub(crate) post_cut: GeometryResult,
    pub(crate) pre_cut_brushes: Vec<Option<u32>>,
    pub(crate) post_cut_brushes: Vec<Option<u32>>,
    /// Post-cut face → pre-cut face, when a face was cut.
    pub(crate) face_parent: Option<Vec<usize>>,
    pub(crate) lightmap_density: f32,
    pub(crate) built_in: Duration,
}

/// Parse → partition with occluders → buried lights → data script and its
/// membership manifest → static light namespace → runtime cells → geometry,
/// then atlas preparation's chart planning and oversize-face cut, with the
/// defaults `prl-build <map> --release` uses.
pub(crate) fn map_geometries(map: &str) -> MapGeometries {
    let started = Instant::now();
    let input = crate::fixture_pipeline::fixture_path(map);
    let argv = [
        input.to_str().expect("UTF-8 path").to_owned(),
        "--release".to_owned(),
        "--no-tui".to_owned(),
    ];
    let args = crate::parse_args_from(argv.into_iter()).expect("arguments parse");
    let mut map_data = crate::parse::parse_map_file(&args.input, args.format).expect("map parses");
    let runtime_cells::OccluderPartition { result, tier } =
        runtime_cells::partition_with_occluders(
            &map_data.brush_volumes,
            &map_data.kinematic_movers,
        )
        .expect("map partitions");
    let buried_lights = BuriedLights::classify(&result.tree, &map_data.lights);
    // Membership can turn static lights animated, and a map with no static
    // light cuts nothing, so the manifest applies before the namespace forms.
    let compiled = crate::compile_worldspawn_data_script(
        &args.input,
        map_data.data_script.as_deref(),
        &map_data.lights,
        &buried_lights,
        crate::script_light_membership::map_members_from_map(
            &map_data.kinematic_movers,
            &map_data.trigger_volumes,
            &map_data.map_entities,
        ),
    )
    .expect("data script compiles");
    if let Some(script) = compiled {
        crate::script_light_membership::apply_manifest(
            &mut map_data.lights,
            &map_data.light_start_active_defaults,
            &script.membership_manifest,
        )
        .expect("membership applies");
    }
    let static_lights = StaticBakedLights::from_lights_excluding(&map_data.lights, &buried_lights);
    let (runtime, _) = runtime_cells::RuntimeCells::of_partition(&result.tree, &tier);
    let (mut post_cut, pre_cut_brushes) = crate::geometry::extract_geometry_with_brushes(
        &result.faces,
        &runtime.cells.tree,
        &runtime.cells.exterior_cells,
    );
    let pre_cut = post_cut.clone();
    let config = crate::lightmap_config(&args, map_data.lightmap_density);
    let cut = crate::lightmap_bake::plan_cut_charts(
        &mut post_cut,
        &static_lights,
        config.lightmap_density,
        &map_data.lightmap_scale_regions,
        LIGHTMAP_POOL_LAYER_EDGE,
        args.working_set_max_bytes,
        config.uncompressed_irradiance,
    )
    .expect("charts plan");
    let (post_cut_brushes, face_parent) = match &cut.face_remap {
        Some(remap) => {
            let mut parent = vec![usize::MAX; remap.last().map_or(0, |r| r.end)];
            for (old, range) in remap.iter().enumerate() {
                parent[range.clone()].fill(old);
            }
            (
                crate::geometry::remap_face_brushes(&pre_cut_brushes, remap),
                Some(parent),
            )
        }
        None => (pre_cut_brushes.clone(), None),
    };
    MapGeometries {
        pre_cut,
        post_cut,
        pre_cut_brushes,
        post_cut_brushes,
        face_parent,
        lightmap_density: config.lightmap_density,
        built_in: started.elapsed(),
    }
}
