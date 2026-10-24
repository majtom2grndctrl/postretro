//! Receiver-footprint switch through the production stage: memo re-keying,
//! reference parity, worker-count determinism and footprint key padding.
use std::cell::Cell;

use super::*;
use crate::lightmap_bake::{
    SoftProbes, light_texel_contribution_and_visibility_with, segment_clear_remembering,
    soft_visibility_with,
};
use crate::lightmap_layer::{LAYER_AABB_PADDING_METERS, LayerTexel, LightmapLayer};
use crate::map_data::{DEFAULT_LIGHT_SIZE, MapLightmapScaleRegion};
use crate::texel_footprint::TexelFrame;

/// (lightmap bytes, shadowmask bytes, layer count) of one fused bake.
type Baked = (Vec<u8>, Vec<u8>, u32);

fn floored(uncompressed_irradiance: bool, penumbra_floor: bool) -> LightmapConfig {
    LightmapConfig {
        penumbra_floor,
        ..config(uncompressed_irradiance)
    }
}

/// One fused stage bake of `geometry` on a `workers`-thread pool, with
/// `regions` as `_lightmap_scale` regions.
fn bake_on(
    workers: usize,
    cache: Option<&StageCache>,
    lights: &[MapLight],
    config: &LightmapConfig,
    geometry: GeometryResult,
    regions: &[MapLightmapScaleRegion],
) -> Baked {
    let progress = StageProgress::indeterminate();
    let control = BakeControl::new(Arc::new(Governor::new(workers, false)), &progress);
    ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .expect("build fixture pool")
        .install(|| bake_with(&control, cache, lights, config, geometry, regions))
}

/// [`bake_on`] on the calling thread, where log capture reads the stage's
/// cache lines.
fn bake_here(
    cache: Option<&StageCache>,
    lights: &[MapLight],
    config: &LightmapConfig,
    geometry: GeometryResult,
) -> Baked {
    bake_with(
        &BakeControl::unrestricted(),
        cache,
        lights,
        config,
        geometry,
        &[],
    )
}

fn bake_with(
    control: &BakeControl,
    cache: Option<&StageCache>,
    lights: &[MapLight],
    config: &LightmapConfig,
    mut geometry: GeometryResult,
    regions: &[MapLightmapScaleRegion],
) -> Baked {
    let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
    let static_lights = StaticBakedLights::from_lights(lights);
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let prepared = lightmap_bake::prepare_atlas(
        &mut geometry,
        &static_lights,
        config.lightmap_density,
        regions,
    )
    .expect("fixture atlas must prepare");
    let output = bake_fused_prepared(
        &test_args(),
        cache,
        control,
        &BakeControl::unrestricted(),
        &mut geometry,
        &static_lights,
        &alpha_lights,
        None,
        &[],
        &bvh,
        &primitives,
        config,
        prepared,
    )
    .expect("fused fixture bake must succeed");
    (
        output.lightmap.section.to_bytes(),
        output
            .shadowmask
            .expect("lit fixture faces must emit id 42")
            .to_bytes(),
        output.lightmap.layer_count,
    )
}

/// The prepared atlas and trace inputs the stage keys and bakes against, as
/// [`bake_on`] derives them.
struct Prepared {
    geometry: GeometryResult,
    bvh: bvh::bvh::Bvh<f32, 3>,
    primitives: Vec<BvhPrimitive>,
    atlas: PreparedAtlas,
}

impl Prepared {
    fn new(
        mut geometry: GeometryResult,
        lights: &[MapLight],
        density: f32,
        regions: &[MapLightmapScaleRegion],
    ) -> Self {
        let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
        let atlas = lightmap_bake::prepare_atlas(
            &mut geometry,
            &StaticBakedLights::from_lights(lights),
            density,
            regions,
        )
        .expect("fixture atlas must prepare");
        Self {
            geometry,
            bvh,
            primitives,
            atlas,
        }
    }

    fn shared(&self) -> SharedAtlas<'_> {
        SharedAtlas {
            charts: &self.atlas.charts,
            placements: &self.atlas.placements,
            atlas_width: self.atlas.atlas_width,
            atlas_height: self.atlas.atlas_height,
            layout: &self.atlas.layout,
        }
    }

    /// The stage's cache key for `light`'s partition on `layer`.
    fn layer_key(&self, light: &MapLight, config: &LightmapConfig, layer: u32) -> CacheKey {
        CacheKey::new(
            "lightmap_layer",
            lightmap_layer::LAYER_FORMAT_VERSION,
            &lightmap_layer::layer_input_hash(
                light,
                &self.shared(),
                &self.primitives,
                &self.geometry,
                config.lightmap_density,
                test_args().soft_shadow_samples,
                config.penumbra_floor,
                layer,
            ),
        )
    }

    /// `light`'s raw visibility at interior texel `texel` of chart `face`,
    /// baked through the per-light layer walk.
    fn visibility_at(
        &self,
        light: &MapLight,
        penumbra_floor: bool,
        face: usize,
        texel: (i32, i32),
    ) -> Option<f32> {
        let shared = self.shared();
        let mut idx = None;
        lightmap_layer::for_each_light_layer_chart_texel(&shared, face, |sample| {
            if (sample.tx, sample.ty) == texel {
                idx = Some(sample.idx);
            }
        });
        let idx = idx.expect("the texel lies in the chart");
        lightmap_layer::bake_light_layer_controlled(
            light,
            &shared,
            &self.bvh,
            &self.primitives,
            &self.geometry,
            self.atlas.placements[face].layer,
            test_args().soft_shadow_samples,
            penumbra_floor,
            &BakeControl::unrestricted(),
        )
        .texels
        .iter()
        .find(|record| record.idx == idx)
        .map(|record| record.raw_visibility)
    }
}

fn directional(aim: [f32; 3], angular_diameter: f32, color: [f32; 3]) -> MapLight {
    let mut light = point_light(DVec3::new(0.0, 50.0, 0.0), color);
    light.light_type = LightType::Directional;
    light.cone_direction = Some(Vec3::from(aim).normalize().to_array());
    light.angular_diameter = angular_diameter;
    light
}

/// Hard, default-size and directional lights interleaved over
/// [`penumbra_cell_geometry`]'s floor and raised occluder, so every type's
/// shadow edge lands on the floor and one thread's footprint memo serves
/// each type in turn.
fn mixed_lights() -> Vec<MapLight> {
    let mut hard = point_light(DVec3::new(3.3, 3.0, 2.7), [1.0, 0.9, 0.8]);
    hard.falloff_range = 8.0;
    let mut default_size = point_light(DVec3::new(2.6, 2.5, 3.4), [0.3, 1.0, 0.4]);
    default_size.falloff_range = 8.0;
    default_size.light_size = DEFAULT_LIGHT_SIZE;
    let mut spot = point_light(DVec3::new(3.0, 3.5, 3.1), [1.0, 0.5, 0.2]);
    spot.light_type = LightType::Spot;
    spot.falloff_range = 8.0;
    spot.cone_direction = Some([0.0, -1.0, 0.0]);
    spot.cone_angle_inner = Some(1.2);
    spot.cone_angle_outer = Some(1.4);
    vec![
        hard,
        directional([0.3, -1.0, 0.2], 2.0, [0.4, 0.5, 0.6]),
        default_size,
        directional([-0.2, -1.0, 0.35], 0.0, [0.6, 0.5, 0.4]),
        spot,
    ]
}

fn mixed_geometry() -> GeometryResult {
    penumbra_cell_geometry(false)
}

/// Every partition of `lights` read back from `cache`, keyed as the stage
/// keys it.
fn cached_partitions(
    cache: &StageCache,
    prepared: &Prepared,
    lights: &[MapLight],
    config: &LightmapConfig,
) -> Vec<Vec<u8>> {
    let mut partitions = Vec::new();
    for layer in 0..prepared.atlas.layer_count {
        for light in lights {
            partitions.push(
                cache
                    .get(&prepared.layer_key(light, config, layer))
                    .expect("the stage cached every partition"),
            );
        }
    }
    partitions
}

// P6: a warm cache from one switch setting is never served to the other.
// Each first build at a setting misses both memos and every partition and
// equals a cold build at that setting; returning to a setting already cached
// serves that setting's own entries. With the switch off, every cached layer
// texel stores centre-point visibility.
#[test]
fn toggling_the_penumbra_floor_rekeys_every_memo_and_layer() {
    let lights = mixed_lights();
    let cold = |on: bool| bake_on(1, None, &lights, &floored(false, on), mixed_geometry(), &[]);
    let (cold_on, cold_off) = (cold(true), cold(false));
    assert_ne!(cold_on.0, cold_off.0, "the floor must change id 22");
    assert_ne!(cold_on.1, cold_off.1, "the floor must change id 42");
    let partitions = lights.len() * cold_on.2 as usize;

    for (first, second) in [(true, false), (false, true)] {
        let dir = fresh_cache_dir("penumbra_floor_toggle");
        let cache = StageCache::new(&dir).expect("create toggle cache");
        let warm =
            |on: bool| bake_here(Some(&cache), &lights, &floored(false, on), mixed_geometry());
        let expected = |on: bool| if on { &cold_on } else { &cold_off };
        assert_eq!(&warm(first), expected(first));
        cache.clear_test_accesses();

        let logs = LogCapture::start();
        assert_eq!(
            &warm(second),
            expected(second),
            "toggling to {second} must equal a cold build at {second}"
        );
        logs.assert_logged_once(Level::Info, "[cache] lightmap_section miss");
        logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas miss");
        let layers = cache.test_access("lightmap_layer");
        assert_eq!(layers.read_hits, 0, "no partition from {first} is served");
        assert_eq!(layers.read_attempts, partitions);
        drop(logs);
        cache.clear_test_accesses();

        if !second {
            assert_layers_store_centre_point_visibility(&cache, &lights);
        }

        let logs = LogCapture::start();
        assert_eq!(&warm(first), expected(first));
        logs.assert_logged_once(Level::Info, "[cache] lightmap_section hit");
        logs.assert_logged_once(Level::Info, "[cache] shadowmask_atlas hit");
        drop(logs);
        drop(cache);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// With the switch off, every cached partition of the stage's floor-off bake
/// holds exactly the centre-point soft visibility the chart walk samples.
fn assert_layers_store_centre_point_visibility(cache: &StageCache, lights: &[MapLight]) {
    let config = floored(false, false);
    let samples = test_args().soft_shadow_samples;
    let prepared = Prepared::new(mixed_geometry(), lights, config.lightmap_density, &[]);
    let shared = prepared.shared();
    let mut fractional = 0;
    for light in lights {
        let probes = SoftProbes::new(light, samples);
        for layer in 0..prepared.atlas.layer_count {
            let mut expected = Vec::new();
            for face in 0..shared.placements.len() {
                if shared.placements[face].layer != layer {
                    continue;
                }
                let last_occluder = Cell::new(None);
                lightmap_layer::for_each_light_layer_chart_texel(&shared, face, |sample| {
                    let (_, _, visibility) = light_texel_contribution_and_visibility_with(
                        light,
                        sample.world_p,
                        sample.surface_normal,
                        || {
                            soft_visibility_with(
                                sample.world_p,
                                sample.surface_normal,
                                light,
                                sample.seed,
                                || probes,
                                |from, to| {
                                    segment_clear_remembering(
                                        &prepared.bvh,
                                        &prepared.primitives,
                                        &prepared.geometry,
                                        from,
                                        to,
                                        &last_occluder,
                                    )
                                },
                            )
                        },
                    );
                    if let Some(visibility) = visibility {
                        expected.push(LayerTexel {
                            idx: sample.idx,
                            raw_visibility: visibility,
                        });
                    }
                });
            }
            expected.sort_unstable_by_key(|texel| texel.idx);
            let stored = LightmapLayer::from_bytes(
                &cache
                    .get(&prepared.layer_key(light, &config, layer))
                    .expect("the floor-off build cached every partition"),
            )
            .expect("cached partition decodes");
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&stored.texels),
                bytemuck::cast_slice::<_, u8>(&expected),
                "{:?} light, layer {layer}: stored visibility is not centre-point",
                light.light_type
            );
            fractional += stored
                .texels
                .iter()
                .filter(|texel| texel.raw_visibility > 0.0 && texel.raw_visibility < 1.0)
                .count();
        }
    }
    assert!(fractional > 0, "the soft lights' penumbrae reach the floor");
}

// P7: the production stage's section bytes equal the uncached reference
// bake's at each switch setting, cold and through a warm cache, at one worker
// and several.
#[test]
fn production_stage_matches_the_reference_bake_at_each_floor_setting() {
    let lights = mixed_lights();
    let mut references = Vec::new();
    for on in [true, false] {
        let config = floored(false, on);
        let reference = reference_lightmap_of(mixed_geometry(), &lights, &config);
        for workers in [1, 4] {
            let cold = bake_on(workers, None, &lights, &config, mixed_geometry(), &[]);
            assert_eq!(
                cold.0, reference,
                "floor {on}, {workers} worker(s): stage differs from the reference"
            );
        }
        let dir = fresh_cache_dir("penumbra_floor_reference");
        let cache = StageCache::new(&dir).expect("create reference cache");
        let warm = bake_on(4, Some(&cache), &lights, &config, mixed_geometry(), &[]);
        assert_eq!(warm.0, reference, "floor {on}: warm stage differs");
        drop(cache);
        let _ = std::fs::remove_dir_all(&dir);
        references.push(reference);
    }
    assert_ne!(
        references[0], references[1],
        "the floor must reach the reference"
    );
}

/// A default-size light 0.2 m under the plate of [`mixed_geometry`], so a
/// tenth of its sphere lies beyond it, beside a hard light that stands
/// clear. `quad_geometry` winds its quads clockwise seen from above, so the
/// plate's front, which the emitter test reads, faces down at this light.
fn embedded_lights() -> Vec<MapLight> {
    let mut embedded = point_light(DVec3::new(3.0, 0.8, 3.0), [0.3, 1.0, 0.4]);
    embedded.falloff_range = 8.0;
    embedded.light_size = DEFAULT_LIGHT_SIZE;
    let mut hard = point_light(DVec3::new(3.3, 3.0, 2.7), [1.0, 0.9, 0.8]);
    hard.falloff_range = 8.0;
    vec![embedded, hard]
}

// An emitter reaching into geometry traces its full sets on every floored
// path, so the production stage still equals the reference bake, cold and
// warm, at one worker and several (P7); the stage warns once per compile,
// cold or with every memo hit, naming only that light.
#[test]
fn an_embedded_emitter_bakes_alike_on_every_path_and_warns_once_per_compile() {
    let lights = embedded_lights();
    let geometry = mixed_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
    let flags: Vec<f32> = lights
        .iter()
        .map(|light| {
            crate::texel_footprint::embedded_emitter_fraction(light, &bvh, &primitives, &geometry)
        })
        .collect();
    assert!(flags[0] > 0.05 && flags[1] == 0.0, "fractions {flags:?}");

    for on in [true, false] {
        let config = floored(false, on);
        let reference = reference_lightmap_of(mixed_geometry(), &lights, &config);
        for workers in [1, 4] {
            let cold = bake_on(workers, None, &lights, &config, mixed_geometry(), &[]);
            assert_eq!(cold.0, reference, "floor {on}, {workers} worker(s)");
        }
    }

    let config = floored(false, true);
    let dir = fresh_cache_dir("penumbra_floor_embedded");
    let cache = StageCache::new(&dir).expect("create embedded-emitter cache");
    let warnings = |baked: &dyn Fn() -> Baked, memo: &str| {
        let logs = LogCapture::start();
        let output = baked();
        logs.assert_logged_once(Level::Info, memo);
        let warnings: Vec<String> = logs
            .records()
            .iter()
            .filter(|record| {
                record.level == Level::Warn && record.message.contains("emitter inside geometry")
            })
            .map(|record| record.message.clone())
            .collect();
        (output, warnings)
    };
    let (cold, cold_warnings) = warnings(
        &|| bake_here(Some(&cache), &lights, &config, mixed_geometry()),
        "[cache] lightmap_section miss",
    );
    let (warm, warm_warnings) = warnings(
        &|| bake_here(Some(&cache), &lights, &config, mixed_geometry()),
        "[cache] lightmap_section hit",
    );
    assert_eq!(warm, cold, "a warm bake equals the cold one");
    assert_eq!(cold_warnings.len(), 1, "{cold_warnings:?}");
    assert!(
        cold_warnings[0].contains("static light #0"),
        "{cold_warnings:?}"
    );
    assert_eq!(warm_warnings, cold_warnings, "a memo hit warns as cold did");
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);

    let logs = LogCapture::start();
    bake_here(None, &lights, &floored(false, false), mixed_geometry());
    logs.assert_not_logged(Level::Warn, "emitter inside geometry");
}

// The CLI's defaults configure a floored bake.
#[test]
fn default_cli_settings_bake_the_floored_stage() {
    let lights = mixed_lights();
    let config = crate::lightmap_config(&test_args(), Some(0.25));
    assert!(config.penumbra_floor);
    let floored_on = floored(false, true);
    assert_eq!(config.lightmap_density, floored_on.lightmap_density);
    assert_eq!(
        bake_on(1, None, &lights, &config, mixed_geometry(), &[]),
        bake_on(1, None, &lights, &floored_on, mixed_geometry(), &[]),
    );
}

/// The uncached reference lightmap of `geometry`.
fn reference_lightmap_of(
    mut geometry: GeometryResult,
    lights: &[MapLight],
    config: &LightmapConfig,
) -> Vec<u8> {
    let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH must build");
    let static_lights = StaticBakedLights::from_lights(lights);
    let prepared =
        lightmap_bake::prepare_atlas(&mut geometry, &static_lights, config.lightmap_density, &[])
            .expect("fixture atlas must prepare");
    let mut ctx = lightmap_bake::LightmapBakeCtx {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &mut geometry,
        lights: &static_lights,
        scale_regions: &[],
    };
    lightmap_bake::bake_prepared_lightmap_controlled(
        &mut ctx,
        config,
        prepared,
        &BakeControl::unrestricted(),
    )
    .expect("reference lightmap must bake")
    .section
    .to_bytes()
}

// P9: repeat cold bakes of interleaved hard, default-size and directional
// lights are byte-identical at one worker and several, partition by
// partition and in the section bytes; a warm build equals the cold one,
// whether every memo hits or only the layers do.
#[test]
fn mixed_light_bakes_are_byte_identical_across_workers_and_warm_equals_cold() {
    let lights = mixed_lights();
    for on in [true, false] {
        let config = floored(false, on);
        let prepared = Prepared::new(mixed_geometry(), &lights, config.lightmap_density, &[]);
        let mut runs = Vec::new();
        for workers in [1, 4, 1] {
            let dir = fresh_cache_dir("penumbra_floor_workers");
            let cache = StageCache::new(&dir).expect("create worker cache");
            let baked = bake_on(
                workers,
                Some(&cache),
                &lights,
                &config,
                mixed_geometry(),
                &[],
            );
            let partitions = cached_partitions(&cache, &prepared, &lights, &config);
            runs.push((dir, cache, baked, partitions));
        }
        let (_, cache, baked, partitions) = &runs[0];
        assert_eq!(
            baked,
            &bake_on(1, None, &lights, &config, mixed_geometry(), &[]),
            "floor {on}: the warm-miss build equals the uncached build"
        );
        for (_, _, other_baked, other_partitions) in &runs[1..] {
            assert_eq!(other_baked, baked, "floor {on}: section bytes moved");
            assert_eq!(
                other_partitions, partitions,
                "floor {on}: a layer record moved"
            );
        }

        // Every memo hits.
        cache.clear_test_accesses();
        assert_eq!(
            &bake_on(4, Some(cache), &lights, &config, mixed_geometry(), &[]),
            baked,
            "floor {on}: warm hit differs"
        );
        assert_eq!(cache.test_access("lightmap_layer").read_attempts, 0);

        // Only the layers hit: the encode setting re-keys the section memo.
        let uncompressed = floored(true, on);
        cache.clear_test_accesses();
        assert_eq!(
            bake_on(
                4,
                Some(cache),
                &lights,
                &uncompressed,
                mixed_geometry(),
                &[]
            ),
            bake_on(1, None, &lights, &uncompressed, mixed_geometry(), &[]),
            "floor {on}: a layer-only warm build differs from cold"
        );
        let layers = cache.test_access("lightmap_layer");
        assert_eq!(layers.read_hits, partitions.len());
        assert_eq!(layers.read_attempts, layers.read_hits);

        for (dir, cache, ..) in runs {
            drop(cache);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

// --- P10: footprint key padding --------------------------------------------

/// `_lightmap_scale` region coarsening the 0.25 m fixture density to 1.5 m
/// texels over the floor.
fn coarse_region() -> MapLightmapScaleRegion {
    MapLightmapScaleRegion {
        min: [-1.0, -1.0, -1.0],
        max: [16.0, 1.0, 16.0],
        planes: Vec::new(),
        scale: 1.0 / 6.0,
    }
}

const COARSE_RANGE: f32 = 5.0;

/// Height of the plate above the floor: above the 1 mm ray lift, so clip
/// segments pass beneath it.
const PLATE_HEIGHT: f32 = 0.002;

/// A horizontal plate over `[x0, x1] × [z0, z1]` at `PLATE_HEIGHT`, as one
/// face whose first triangle is degenerate. The atlas gives such a face a
/// fixed placeholder chart, so moving it changes no chart; a charted face's
/// move re-keys every layer through the atlas fingerprint, which would hide
/// what the geometry slice sees.
fn push_plate(geometry: &mut GeometryResult, x: [f32; 2], z: [f32; 2]) {
    push_quad(geometry, 0.0, 0.0, 1.0, PLATE_HEIGHT, 0);
    let first = geometry.geometry.vertices.len() - 4;
    let corners = [[x[0], z[0]], [x[1], z[0]], [x[1], z[1]], [x[0], z[1]]];
    for (vertex, [cx, cz]) in geometry.geometry.vertices[first..].iter_mut().zip(corners) {
        vertex.position = [cx, PLATE_HEIGHT, cz];
    }
    let range = geometry.face_index_ranges.last_mut().expect("plate face");
    let base = first as u32;
    geometry
        .geometry
        .indices
        .truncate(range.index_offset as usize);
    geometry
        .geometry
        .indices
        .extend([0, 0, 1, 0, 1, 2, 0, 2, 3].map(|corner| base + corner));
    range.index_count = 9;
}

/// A 15 m floor in the coarse region, an optional plate, and a fine 4 m quad
/// far away in another cell.
fn padding_geometry(plate: Option<([f32; 2], [f32; 2])>) -> GeometryResult {
    let mut geometry = quad_geometry();
    geometry.geometry.vertices.clear();
    geometry.geometry.indices.clear();
    geometry.geometry.faces.clear();
    geometry.face_index_ranges.clear();
    push_quad(&mut geometry, 0.0, 0.0, 15.0, 0.0, 0);
    push_quad(&mut geometry, 100.0, 100.0, 4.0, 0.0, 1);
    if let Some((x, z)) = plate {
        push_plate(&mut geometry, x, z);
    }
    geometry
}

// P10: beside a face coarse enough that its footprint radius exceeds the
// layer key's 0.5 m influence padding, an occluder that only a footprint
// sample can reach (past `falloff_range + 0.5 m` from the light, within one
// pitch of a texel just inside its reach) darkens that texel, and moving it
// re-keys and re-bakes the light's layer on a warm build. A light that
// reaches no coarse chart keeps its key. With the floor off the same move
// cannot change the bake and re-keys nothing.
#[test]
fn an_occluder_only_a_footprint_reaches_rekeys_the_layer_past_the_padding() {
    let regions = [coarse_region()];
    let density = config(false).lightmap_density;

    // The receiver texel: the floor's last column, middle row.
    let mut far = point_light(DVec3::new(102.0, 1.0, 102.0), [0.4, 0.6, 1.0]);
    far.falloff_range = 3.0;
    let open_floor = Prepared::new(
        padding_geometry(None),
        std::slice::from_ref(&far),
        density,
        &regions,
    );
    let floor = &open_floor.atlas.charts[0];
    let radius = TexelFrame::for_chart(floor).radius();
    assert!(
        radius > LAYER_AABB_PADDING_METERS,
        "the coarse floor's footprint radius {radius} must exceed the padding"
    );
    let (width, height) = crate::chart_raster::chart_interior_dims(floor);
    let texel = (width - 1, height / 2);
    let centre = crate::chart_raster::chart_texel_world_position(floor, texel.0, texel.1);

    // A hard light whose reach ends just past the texel, along -x of it.
    let light_height = 0.3f32;
    let along = ((COARSE_RANGE - 0.005).powi(2) - light_height.powi(2)).sqrt();
    let mut near = point_light(
        DVec3::new(
            f64::from(centre.x - along),
            f64::from(light_height),
            f64::from(centre.z),
        ),
        [1.0, 0.9, 0.8],
    );
    near.falloff_range = COARSE_RANGE;
    let lights = vec![near.clone(), far];
    let to_texel = centre.distance(near.origin.as_vec3());
    assert!(to_texel < COARSE_RANGE && to_texel > COARSE_RANGE - 0.01);

    // The plate starts just past the key's unwidened bound, beside the
    // texel; the move slides it 3 m along z, out of every reached texel's
    // footprint but still inside the widened bound.
    let bound_x = near.origin.x as f32 + COARSE_RANGE + LAYER_AABB_PADDING_METERS;
    let plate_x = [bound_x + 0.01, bound_x + 1.01];
    let beside = (plate_x, [centre.z - 0.75, centre.z + 0.75]);
    let moved = (plate_x, [centre.z + 2.25, centre.z + 3.75]);
    assert!(
        plate_x[0] - centre.x < radius,
        "the plate lies within one pitch of the texel"
    );
    let (beside_at, moved_at) = (
        Prepared::new(padding_geometry(Some(beside)), &lights, density, &regions),
        Prepared::new(padding_geometry(Some(moved)), &lights, density, &regions),
    );
    assert_eq!(
        crate::lightmap_layer::atlas_layout_fingerprint(&beside_at.shared()),
        crate::lightmap_layer::atlas_layout_fingerprint(&moved_at.shared()),
        "the move must not change any chart"
    );
    assert_eq!(
        crate::chart_raster::chart_texel_world_position(
            &beside_at.atlas.charts[0],
            texel.0,
            texel.1
        ),
        centre,
        "the floor chart is the one the texel was chosen on"
    );

    // The plate reaches the bake only through footprint samples.
    let open = open_floor.visibility_at(&near, true, 0, texel);
    let (on_beside, on_moved) = (
        beside_at.visibility_at(&near, true, 0, texel),
        moved_at.visibility_at(&near, true, 0, texel),
    );
    let off_beside = beside_at.visibility_at(&near, false, 0, texel);
    println!(
        "footprint radius {radius:.3} m; texel {to_texel:.4} m from the light; visibility \
         open {open:?}, plate beside {on_beside:?}, moved {on_moved:?}; floor off {off_beside:?}"
    );
    assert_eq!(open, Some(1.0));
    assert!(
        on_beside.is_some_and(|v| v < 1.0),
        "the plate must darken the texel"
    );
    assert_eq!(on_moved, Some(1.0), "moved away, the plate darkens nothing");
    assert_eq!(
        off_beside,
        Some(1.0),
        "the centre ray never meets the plate"
    );

    // Keys: the move re-keys the near light with the floor on and nothing
    // with it off; the far light, which reaches no coarse chart, never.
    let keys = |prepared: &Prepared, on: bool| -> Vec<String> {
        let config = floored(false, on);
        lights
            .iter()
            .map(|light| prepared.layer_key(light, &config, 0).as_filename())
            .collect()
    };
    let (on_a, on_b) = (keys(&beside_at, true), keys(&moved_at, true));
    assert_ne!(on_a[0], on_b[0], "the near light's key must see the plate");
    assert_eq!(on_a[1], on_b[1], "the far light reaches no coarse chart");
    assert_eq!(
        keys(&beside_at, false),
        keys(&moved_at, false),
        "past the unwidened bound, the floor-off key cannot see the plate"
    );

    // Warm rebuilds through the stage.
    for on in [true, false] {
        let config = floored(true, on);
        let dir = fresh_cache_dir("penumbra_floor_padding");
        let cache = StageCache::new(&dir).expect("create padding cache");
        let geometry = |plate| padding_geometry(Some(plate));
        let cold_beside = bake_on(
            1,
            Some(&cache),
            &lights,
            &config,
            geometry(beside),
            &regions,
        );
        cache.clear_test_accesses();
        let warm_moved = bake_on(1, Some(&cache), &lights, &config, geometry(moved), &regions);
        assert_eq!(
            warm_moved,
            bake_on(1, None, &lights, &config, geometry(moved), &regions),
            "floor {on}: the warm rebuild equals a cold build of the move"
        );
        let layers = cache.test_access("lightmap_layer");
        let per_light = cold_beside.2 as usize;
        if on {
            assert_eq!(
                layers.read_attempts - layers.read_hits,
                per_light,
                "the near light's layers re-bake"
            );
            assert_eq!(layers.read_hits, per_light, "the far light's layers hit");
        } else {
            assert_eq!(warm_moved, cold_beside);
        }
        drop(cache);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
