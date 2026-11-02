// Off-main level-set builds: the upload thread's set equals the main-thread
// oracle's, begin/receipt/publish create nothing on main, errors end a build
// once, writes submit in bounded chunks and before the result, and tiers come
// from the dispatch snapshot.
// See: context/plans/done/level-load-pipeline/index.md (Off the main thread, Per-load resources, Settings and extent, Failure, Equivalence)

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use glam::Vec3;
use postretro_level_format::prm::PrmFormat;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_render_cpu::fluid_surface::prepared::prepare_fluid_surfaces;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::lightmap_family::{
    LightmapFamilyInputs, LightmapStreamingInputs, plan_lightmap_family,
};
use postretro_render_cpu::sh_residency::plan_sh_residency;
use postretro_render_cpu::sh_volume::{animated_light_count, usable_sh_volume};
use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
use postretro_render_cpu::texture_prep::{prepare_model_materials, prepare_world_textures};
use postretro_render_cpu::uv_normalize::{diffuse_dims, normalize_world_uvs_with_dims};
use postretro_render_data::material::Material;

use super::*;
use crate::render::fluid_baked_frame_test_fixtures::{direct_section, sh_section};
use crate::render::fluid_reflection_frame_test_fixtures::{
    FRAME, back_wall, base_pool, base_view, capture_checked, green_probe, standing_quad,
    with_world_quads,
};
use crate::render::gpu_create::gpu_creation_counts;
use crate::render::gpu_test_harness::{gpu_or_skip, offscreen_renderer_or_skip};
use crate::render::level_resource_set::PreviousLevelSet;
use crate::render::level_resource_set_tests::{capture, hash, spot_light};
use crate::render::queue_writes::QueueWrites;
use crate::render::renderer_textures::tests::{chain, with_one_triangle, write_prm};
use crate::render::upload_thread::tests::wait_for;
use crate::render::upload_thread::{UPLOAD_SUBMIT_CHUNK_BYTES, UploadThread};
use crate::render::{LevelGeometryShStorage, sh_streaming};

/// Poll `pending` until it reports, failing after a bound; its outcome is
/// reported once.
fn take(
    renderer: &mut Renderer,
    pending: &mut PendingLevelSet,
) -> Result<LevelSetId, LevelBuildError> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(outcome) = pending.try_take(renderer) {
            assert!(
                pending.try_take(renderer).is_none(),
                "a build reports exactly once"
            );
            return outcome;
        }
        assert!(
            Instant::now() < deadline,
            "the build did not report within 120 s"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn dev_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev")
}

const MODEL: &str = "models/pose-modifier-fixture/joint_zones.gltf";

fn parsed_model() -> postretro_model::gltf_loader::LoadedModel {
    postretro_model::gltf_loader::load_model(&dev_root().join(MODEL))
        .expect("the fixture model parses")
}

fn smoke_registration() -> SpriteCollectionRegistration {
    SpriteCollectionRegistration {
        baked_sidecar_eligible: false,
        spec_intensity: 0.0,
        spec_exponent: 16.0,
        lifetime: 1.0,
        emissive: 0.0,
    }
}

fn no_extras() -> GeometryExtras<'static> {
    GeometryExtras {
        volumes: &[],
        reflection_probes: None,
        fog_pixel_scale: 0,
        fog_cell_masks: None,
    }
}

fn rest() -> GeometryRest {
    GeometryRest {
        gpu_lighting_payloads: Default::default(),
        sh_plan: None,
        lightmap_plan: None,
        fluid: None,
        planar_candidate: None,
    }
}

/// The legacy-SH one-triangle level with one baked texture and a dynamic
/// spot light.
struct LegacyFixture {
    dir: tempfile::TempDir,
    names: Vec<String>,
    keys: TextureCacheKeysSection,
    materials: Vec<Material>,
    sh: postretro_level_format::sh_volume::OctahedralShVolumeSection,
    direct: postretro_level_format::direct_sh_volume::DirectShVolumeSection,
    lights: Vec<postretro_level_loader::MapLight>,
}

fn legacy_fixture(fill: u8) -> LegacyFixture {
    let dir = tempfile::tempdir().unwrap();
    let key = [fill; 32];
    write_prm(
        dir.path(),
        key,
        [
            Some(chain(PrmFormat::Rgba8UnormSrgb, 4, 4, fill)),
            None,
            None,
            None,
        ],
    );
    let sh = sh_section([-1.0; 3], [1.0; 3], [2, 2, 2], |_| Vec3::splat(0.25));
    let direct = direct_section(&sh, Vec3::splat(0.5));
    LegacyFixture {
        dir,
        names: vec![format!("baked-{fill:02x}")],
        keys: TextureCacheKeysSection { keys: vec![key] },
        materials: vec![Material::Default],
        sh,
        direct,
        lights: vec![spot_light()],
    }
}

impl LegacyFixture {
    /// The main-thread oracle: textures, geometry, then the model and the
    /// sprite collection into the published set.
    fn install_oracle(&self, renderer: &mut Renderer, with_presentation: bool) {
        renderer.install_textures(&self.names, &self.keys, self.dir.path(), &self.materials);
        with_one_triangle(
            &self.materials,
            Some(&self.sh),
            Some(&self.direct),
            &self.lights,
            |geometry| {
                renderer
                    .install_level_geometry(geometry, Default::default())
                    .expect("level resources install")
            },
        );
        if with_presentation {
            let root = dev_root();
            let parsed = Ok(parsed_model());
            assert!(
                renderer
                    .upload_parsed_skinned_model(
                        MODEL,
                        &root.join(MODEL),
                        &parsed,
                        &root.join("cache")
                    )
                    .is_some()
            );
            renderer.register_smoke_collection(
                "proof",
                "smoke_puff",
                &root.join("textures"),
                &root.join("cache"),
                smoke_registration(),
            );
        }
    }

    /// The same level queued on the upload thread.
    fn queue(&self, builder: &LevelSetBuilder, with_presentation: bool) {
        builder.textures(
            prepare_world_textures(&self.names, &self.keys, self.dir.path()),
            self.materials.clone(),
        );
        builder.geometry_from(self.geometry_source(), rest());
        if with_presentation {
            let root = dev_root();
            let model = parsed_model();
            let materials = prepare_model_materials(&model.submeshes, &root.join("cache"));
            builder.model(MODEL, model, materials);
            builder.smoke(vec![SmokeCollectionRequest {
                collection_id: "proof".into(),
                asset: "smoke_puff".into(),
                texture_root: root.join("textures"),
                prm_cache_root: root.join("cache"),
                registration: smoke_registration(),
            }]);
        }
    }

    fn geometry_source(&self) -> GeometrySource {
        let (materials, sh, direct, lights) = (
            self.materials.clone(),
            self.sh.clone(),
            self.direct.clone(),
            self.lights.clone(),
        );
        Box::new(move |visit| {
            with_one_triangle(&materials, Some(&sh), Some(&direct), &lights, |geometry| {
                visit(geometry, no_extras())
            })
        })
    }

    fn build(&self, renderer: &mut Renderer, tiers: LevelBuildSnapshotInputs) -> LevelSetId {
        let (builder, mut pending) = renderer.begin_level_set_build(tiers);
        self.queue(&builder, false);
        builder.finish();
        take(renderer, &mut pending).expect("the off-main build succeeds")
    }
}

/// The fluid pool level: two opaque quads, the base pool bound to the green
/// probe, no textures (one placeholder slot).
fn fluid_source() -> GeometrySource {
    Box::new(|visit| {
        let volumes = [base_pool()];
        let probes = green_probe(&[(0, 0)]);
        with_world_quads(&[standing_quad(), back_wall()], |geometry| {
            visit(
                geometry,
                GeometryExtras {
                    volumes: &volumes,
                    reflection_probes: Some(&probes),
                    fog_pixel_scale: 0,
                    fog_cell_masks: None,
                },
            )
        })
    })
}

fn fluid_rest() -> GeometryRest {
    GeometryRest {
        fluid: prepare_fluid_surfaces(
            &[base_pool()],
            Some(&green_probe(&[(0, 0)])),
            &[(64, 64)],
            1,
            None,
        ),
        ..rest()
    }
}

fn install_fluid_oracle(renderer: &mut Renderer) {
    with_world_quads(&[standing_quad(), back_wall()], |geometry| {
        renderer
            .install_level_geometry(geometry, Default::default())
            .expect("level resources install")
    });
    renderer.install_fluid_surfaces(&[base_pool()], Some(&green_probe(&[(0, 0)])));
}

fn build_fluid(renderer: &mut Renderer, tiers: LevelBuildSnapshotInputs) -> LevelSetId {
    let (builder, mut pending) = renderer.begin_level_set_build(tiers);
    builder.geometry_from(fluid_source(), fluid_rest());
    builder.finish();
    take(renderer, &mut pending).expect("the off-main fluid build succeeds")
}

/// Everything a set's build decided, by family.
fn summary(level: &LevelResourceSet) -> String {
    level_resource_summary(level)
}

fn active_summary(renderer: &Renderer) -> String {
    summary(renderer.level())
}

fn tiers_now(renderer: &Renderer) -> LevelBuildSnapshotInputs {
    renderer.level_build_tiers()
}

fn publish_retire(renderer: &mut Renderer, id: LevelSetId) {
    renderer.publish_level_set(id, PreviousLevelSet::Retire);
}

/// An optional target extent.
type TargetExtent = Option<(u32, u32)>;

/// The reflection targets the published set lent the passes.
fn lent_targets(renderer: &Renderer) -> (TargetExtent, TargetExtent) {
    let full = renderer.full();
    (
        full.fluid_surface.reflection_target_extent(),
        full.fluid_surface.planar_layer_extent(),
    )
}

#[test]
fn off_main_build_matches_the_oracle_set() {
    let test = "off_main_build_matches_the_oracle_set";
    // Legacy SH, a baked texture, a spot light, a model and a sprite
    // collection.
    {
        let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
            return;
        };
        let fixture = legacy_fixture(0x5A);
        fixture.install_oracle(&mut renderer, true);
        let oracle = active_summary(&renderer);
        let oracle_frame = capture(&mut renderer, true);

        let (builder, mut pending) = renderer.begin_level_set_build(renderer.level_build_tiers());
        fixture.queue(&builder, true);
        builder.finish();
        let id = take(&mut renderer, &mut pending).expect("the off-main build succeeds");
        publish_retire(&mut renderer, id);
        assert_eq!(active_summary(&renderer), oracle, "legacy: summaries");
        assert_eq!(
            hash(&capture(&mut renderer, true)),
            hash(&oracle_frame),
            "legacy: frames"
        );
        assert!(
            oracle.contains(MODEL) && oracle.contains("proof"),
            "{oracle}"
        );
    }

    // Streamed SH through the public geometry part, with prep's plans. A
    // streamed frame needs drain batches, so this fixture compares summaries.
    {
        let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
            return;
        };
        let (_temp, path) = sh_streaming::write_nine_cluster_prl();
        let load = || postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
        let root = tempfile::tempdir().unwrap();

        let mut world = load();
        assert!(world.sh_stream_manifest().is_some(), "the fixture streams");
        let materials = vec![Material::Default; world.texture_names.len()];
        renderer.install_textures(
            &world.texture_names,
            &world.texture_cache_keys,
            root.path(),
            &materials,
        );
        renderer.normalize_world_uvs(&mut world);
        let payloads = world.take_gpu_lighting_payloads();
        renderer
            .install_level_geometry(&level_world_to_geometry(&world, &materials), payloads)
            .expect("oracle install");
        renderer.install_fluid_surfaces(
            &world.environment_volumes.volumes,
            world.reflection_probes.as_ref(),
        );
        let oracle = active_summary(&renderer);

        let mut world = load();
        let prepared =
            prepare_world_textures(&world.texture_names, &world.texture_cache_keys, root.path());
        let dims = diffuse_dims(&prepared);
        normalize_world_uvs_with_dims(&mut world, &dims);
        let limits = renderer.install_device_limits();
        let sh_residency = world
            .sh_stream_manifest()
            .map(|manifest| plan_sh_residency(manifest, &world.cells).expect("the plan accepts"));
        let payloads = world.take_gpu_lighting_payloads();
        let lightmap_family = {
            let geometry = level_world_to_geometry(&world, &materials);
            let descriptors = match &geometry.sh_storage {
                LevelGeometryShStorage::Streaming(manifest) => {
                    Some(manifest.base().animation_descriptors.as_slice())
                }
                LevelGeometryShStorage::Legacy => None,
            };
            let count =
                animated_light_count(usable_sh_volume(geometry.sh_volume, &limits), descriptors);
            plan_lightmap_family(
                &LightmapFamilyInputs {
                    lightmap: geometry.lightmap,
                    shadowmask: geometry.shadowmask_atlas,
                    streaming: geometry.lightmap_streaming.map(|streaming| {
                        LightmapStreamingInputs {
                            content_tag: streaming.content_tag,
                            pool_cap_layers: streaming.pool_cap_layers,
                        }
                    }),
                    payload_blocks: &payloads.blocks,
                    weight_maps: geometry.animated_light_weight_maps,
                    animated_chunks: geometry.animated_light_chunks,
                    bvh_leaves: &geometry.bvh.leaves,
                    animated_light_count: count,
                },
                &limits,
            )
        };
        let fluid = prepare_fluid_surfaces(
            &world.environment_volumes.volumes,
            world.reflection_probes.as_ref(),
            &dims,
            dims.len(),
            None,
        );
        let world = Arc::new(world);
        let (builder, mut pending) = renderer.begin_level_set_build(renderer.level_build_tiers());
        builder.textures(prepared, materials.clone());
        builder.geometry(LevelGeometryParts {
            world: Arc::clone(&world),
            texture_materials: materials,
            gpu_lighting_payloads: payloads,
            sh_residency,
            lightmap_family,
            fluid,
            planar_candidate: None,
        });
        builder.finish();
        let id = take(&mut renderer, &mut pending).expect("the streamed build succeeds");
        assert!(
            Arc::try_unwrap(world).is_ok(),
            "the geometry part dropped its share of the level before reporting"
        );
        publish_retire(&mut renderer, id);
        assert_eq!(active_summary(&renderer), oracle, "streamed: summaries");
        assert!(
            renderer.level().sh_streaming.is_some(),
            "the set streams SH"
        );
    }

    // Fluid with its reflection targets at High.
    {
        let Some(mut renderer) = offscreen_renderer_or_skip(test, FRAME, FRAME) else {
            return;
        };
        renderer.set_reflection_quality(FluidReflectionQuality::High);
        install_fluid_oracle(&mut renderer);
        let oracle = active_summary(&renderer);
        let oracle_targets = lent_targets(&renderer);
        assert!(
            oracle_targets.0.is_some(),
            "the oracle holds a reflection target"
        );
        let oracle_frame = capture_checked(&mut renderer, base_view());

        let id = {
            let tiers = tiers_now(&renderer);
            build_fluid(&mut renderer, tiers)
        };
        publish_retire(&mut renderer, id);
        assert_eq!(active_summary(&renderer), oracle, "fluid: summaries");
        assert_eq!(
            lent_targets(&renderer),
            oracle_targets,
            "fluid: reflection targets"
        );
        assert_eq!(
            hash(&capture_checked(&mut renderer, base_view())),
            hash(&oracle_frame),
            "fluid: frames"
        );
    }
}

/// The dev maps (animated atlases, fluids, SH either way; streamed
/// lightmaps under `POSTRETRO_LIGHTMAP_STREAMING=stream`) through both
/// paths: the off-main set equals the oracle's.
#[test]
#[ignore = "requires the compiled dev-mod maps and their baked .prm cache"]
fn off_main_build_matches_the_oracle_set_on_dev_maps() {
    let test = "off_main_build_matches_the_oracle_set_on_dev_maps";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 64, 64) else {
        return;
    };
    let root = dev_root();
    let cache = root.join("cache");
    let mut maps: Vec<_> = std::fs::read_dir(root.join("maps"))
        .expect("content/dev/maps exists")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "prl"))
        .collect();
    maps.sort();
    let (mut checked, mut animated, mut streamed_lightmap) = (0, 0, 0);
    for map in maps {
        let label = map.file_stem().unwrap().to_string_lossy().into_owned();
        let load = || postretro_level_loader::load_prl(map.to_str().unwrap());
        let Ok(mut world) = load() else {
            eprintln!("[OffMainEquivalence] {label}: skipped (stale)");
            continue;
        };
        let materials = vec![Material::Default; world.texture_names.len()];
        renderer.install_textures(
            &world.texture_names,
            &world.texture_cache_keys,
            &cache,
            &materials,
        );
        renderer.normalize_world_uvs(&mut world);
        let payloads = world.take_gpu_lighting_payloads();
        renderer
            .install_level_geometry(&level_world_to_geometry(&world, &materials), payloads)
            .expect("oracle install");
        renderer.install_fluid_surfaces(
            &world.environment_volumes.volumes,
            world.reflection_probes.as_ref(),
        );
        let oracle = active_summary(&renderer);

        let mut world = load().unwrap();
        let streams_lightmap = world.lightmap_stream_manifest().is_some();
        let prepared =
            prepare_world_textures(&world.texture_names, &world.texture_cache_keys, &cache);
        let dims = diffuse_dims(&prepared);
        normalize_world_uvs_with_dims(&mut world, &dims);
        let sh_plan = world
            .sh_stream_manifest()
            .map(|manifest| plan_sh_residency(manifest, &world.cells).expect("the plan accepts"));
        let payloads = world.take_gpu_lighting_payloads();
        let fluid = prepare_fluid_surfaces(
            &world.environment_volumes.volumes,
            world.reflection_probes.as_ref(),
            &dims,
            dims.len(),
            renderer.level().sh_sampler_lattice().as_ref(),
        );
        let world = Arc::new(world);
        let source_world = Arc::clone(&world);
        let source_materials = materials.clone();
        let (builder, mut pending) = renderer.begin_level_set_build(renderer.level_build_tiers());
        builder.textures(prepared, materials);
        builder.geometry_from(
            Box::new(move |visit| {
                let world = source_world;
                let geometry = level_world_to_geometry(&world, &source_materials);
                visit(
                    &geometry,
                    GeometryExtras {
                        volumes: &world.environment_volumes.volumes,
                        reflection_probes: world.reflection_probes.as_ref(),
                        fog_pixel_scale: 0,
                        fog_cell_masks: None,
                    },
                )
            }),
            GeometryRest {
                gpu_lighting_payloads: payloads,
                sh_plan,
                lightmap_plan: None,
                fluid,
                planar_candidate: None,
            },
        );
        builder.finish();
        let id = take(&mut renderer, &mut pending).expect("the off-main build succeeds");
        publish_retire(&mut renderer, id);
        assert_eq!(active_summary(&renderer), oracle, "{label}");
        let level = renderer.level();
        let animated_plan = level.animated_lightmap.plan_summary();
        let coverage = format!(
            "streamed SH {}, animated {:.48}, {}",
            level.sh_streaming.is_some(),
            animated_plan,
            level.lightmap_residency_report.log_line()
        );
        animated += usize::from(animated_plan.contains("Some(("));
        streamed_lightmap += usize::from(streams_lightmap);
        renderer.release_level_resources();
        renderer
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the device drains");
        drop(world);
        eprintln!("[OffMainEquivalence] {label}: the off-main set matches the oracle ({coverage})");
        checked += 1;
    }
    eprintln!(
        "[OffMainEquivalence] {checked} map(s), {animated} animated atlas, {streamed_lightmap} streamed lightmap"
    );
    assert!(checked > 0, "no current dev-mod map loaded");
}

#[test]
fn begin_receipt_and_publish_create_nothing_on_main() {
    let test = "begin_receipt_and_publish_create_nothing_on_main";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, FRAME, FRAME) else {
        return;
    };
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    let fixture = legacy_fixture(0x5A);
    // Warm the renderer's sampler map with the fixture's mip count, as a
    // resident level would have.
    fixture.install_oracle(&mut renderer, false);

    for label in ["legacy", "fluid"] {
        let before = gpu_creation_counts();
        let (builder, mut pending) = renderer.begin_level_set_build(renderer.level_build_tiers());
        match label {
            "legacy" => fixture.queue(&builder, true),
            _ => builder.geometry_from(fluid_source(), fluid_rest()),
        }
        builder.finish();
        let id = take(&mut renderer, &mut pending).expect("the build succeeds");
        let rebuilds = (
            renderer.extent_rebuilds(),
            renderer.reflection_tier_rebuilds(),
        );
        publish_retire(&mut renderer, id);
        let main = gpu_creation_counts().since(before);
        assert_eq!(
            main.total(),
            0,
            "{label}: begin, receipt and publish create nothing on main: {main:?}"
        );
        assert_eq!(
            (
                renderer.extent_rebuilds(),
                renderer.reflection_tier_rebuilds()
            ),
            rebuilds,
            "{label}: no counted fallback"
        );
        let upload = pending.upload_creations().expect("taken");
        assert!(
            upload.buffers > 0 && upload.textures > 0 && upload.bind_groups > 0,
            "{label}: the build created on the upload thread: {upload:?}"
        );
        assert_eq!(upload.pipelines, 0, "{label}: no pipeline per load");
    }
    assert!(
        lent_targets(&renderer).0.is_some(),
        "the fluid set lent its own target"
    );
}

#[test]
fn building_off_main_beside_the_active_set_changes_nothing_it_draws() {
    let test = "building_off_main_beside_the_active_set_changes_nothing_it_draws";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let active = legacy_fixture(0x5A);
    active.install_oracle(&mut renderer, false);
    capture(&mut renderer, true);
    let warm = capture(&mut renderer, true);
    let warm_frame = renderer.full().dynamic_depth_cache_diagnostics.frame;

    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let entered = Arc::new((Mutex::new(false), Condvar::new()));
    let (builder, mut pending) = renderer.begin_level_set_build(renderer.level_build_tiers());
    {
        let (gate, entered) = (Arc::clone(&gate), Arc::clone(&entered));
        builder.test_part(move |_| {
            *entered.0.lock().unwrap() = true;
            entered.1.notify_all();
            let (open, wake) = &*gate;
            let mut open = open.lock().unwrap();
            while !*open {
                open = wake.wait(open).unwrap();
            }
            Ok(())
        });
    }
    let beside = legacy_fixture(0xA5);
    beside.queue(&builder, true);
    builder.finish();
    {
        let (flag, wake) = &*entered;
        let mut flag = flag.lock().unwrap();
        while !*flag {
            flag = wake.wait_timeout(flag, Duration::from_secs(30)).unwrap().0;
        }
    }
    for _ in 0..3 {
        assert!(
            pending.try_take(&mut renderer).is_none(),
            "held at the gate"
        );
        assert_eq!(
            hash(&capture(&mut renderer, true)),
            hash(&warm),
            "the active draw holds"
        );
    }
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let id = take(&mut renderer, &mut pending).expect("the build succeeds");
    assert_eq!(renderer.resident_level_sets(), vec![id]);
    assert_eq!(
        hash(&capture(&mut renderer, true)),
        hash(&warm),
        "a resident set draws nothing"
    );
    assert_eq!(
        renderer.full().dynamic_depth_cache_diagnostics.frame,
        warm_frame,
        "the build reset no cache"
    );
}

#[test]
fn upload_errors_end_the_build_once() {
    let test = "upload_errors_end_the_build_once";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let fixture = legacy_fixture(0x5A);
    let tiers = renderer.level_build_tiers();
    let resident = renderer.resident_level_sets();

    // A real validation error, then a panic: each ends its build once, and
    // the parts queued behind do nothing.
    for row in ["validation", "panic"] {
        let (builder, mut pending) = renderer.begin_level_set_build(tiers);
        builder.textures(
            prepare_world_textures(&fixture.names, &fixture.keys, fixture.dir.path()),
            fixture.materials.clone(),
        );
        builder.test_part(move |upload| {
            if row == "validation" {
                // MAP_READ beside STORAGE: a descriptor wgpu rejects.
                let _ = upload.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("invalid level build buffer"),
                    size: 16,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                });
                Ok(())
            } else {
                panic!("injected level build panic")
            }
        });
        let later = Arc::new(Mutex::new(false));
        {
            let later = Arc::clone(&later);
            builder.test_part(move |_| {
                *later.lock().unwrap() = true;
                Ok(())
            });
        }
        builder.geometry_from(fixture.geometry_source(), rest());
        builder.finish();
        match (row, take(&mut renderer, &mut pending)) {
            ("validation", Err(LevelBuildError::Upload(UploadError::Wgpu(message)))) => {
                assert!(!message.is_empty())
            }
            ("panic", Err(LevelBuildError::Upload(UploadError::Panicked(message)))) => {
                assert!(message.contains("injected level build panic"), "{message}")
            }
            (row, other) => panic!("{row}: wrong outcome {other:?}"),
        }
        assert!(
            !*later.lock().unwrap(),
            "{row}: a later part ran after the failure"
        );
        assert_eq!(
            renderer.resident_level_sets(),
            resident,
            "{row}: nothing reached the resident sets"
        );
    }

    // A level-data error from a GPU half: a streamed level with no residency
    // plan.
    {
        let (_temp, path) = sh_streaming::write_nine_cluster_prl();
        let mut world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
        let payloads = world.take_gpu_lighting_payloads();
        let (builder, mut pending) = renderer.begin_level_set_build(tiers);
        builder.geometry(LevelGeometryParts {
            world: Arc::new(world),
            texture_materials: Vec::new(),
            gpu_lighting_payloads: payloads,
            sh_residency: None,
            lightmap_family: plan_lightmap_family(
                &LightmapFamilyInputs {
                    lightmap: None,
                    shadowmask: None,
                    streaming: None,
                    payload_blocks: &[],
                    weight_maps: None,
                    animated_chunks: None,
                    bvh_leaves: &[],
                    animated_light_count: 0,
                },
                &renderer.install_device_limits(),
            ),
            fluid: None,
            planar_candidate: None,
        });
        builder.finish();
        assert!(matches!(
            take(&mut renderer, &mut pending),
            Err(LevelBuildError::LevelData(LevelInstallError::StreamedSh(_)))
        ));
        assert_eq!(
            renderer.resident_level_sets(),
            resident,
            "level data: nothing resident"
        );
    }

    // The upload thread gone.
    {
        let retirement = renderer.take_upload_retirement();
        assert!(retirement.is_some());
        let (builder, mut pending) = renderer.begin_level_set_build(tiers);
        fixture.queue(&builder, false);
        builder.finish();
        assert!(matches!(
            take(&mut renderer, &mut pending),
            Err(LevelBuildError::Upload(UploadError::ThreadGone))
        ));
        assert_eq!(
            renderer.resident_level_sets(),
            resident,
            "thread gone: nothing resident"
        );
    }
}

/// The events a build's hook was handed, in order.
fn record_events(builder: &LevelSetBuilder) -> Arc<Mutex<Vec<LevelBuildEvent>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let hook_events = Arc::clone(&events);
    assert!(builder.on_event(move |event| hook_events.lock().unwrap().push(event)));
    events
}

/// `pending`'s one outcome was taken: the build's sender is gone with it, so
/// no second outcome can follow.
fn assert_no_second_outcome(pending: &PendingLevelSet, row: &str) {
    assert!(
        matches!(
            pending.result.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ),
        "{row}: the build delivered exactly one outcome"
    );
}

// The caller's progress hook panicking on a stored part's or on `finish`'s
// `PartDone` comes back through `report` as an error: the build ends exactly
// once with that panic, reports `Failed` once either way, and the upload
// thread serves the next build.
#[test]
fn a_panicking_progress_hook_ends_the_build_and_the_thread_serves_on() {
    let test = "a_panicking_progress_hook_ends_the_build_and_the_thread_serves_on";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let fixture = legacy_fixture(0x5A);
    let tiers = renderer.level_build_tiers();
    let resident = renderer.resident_level_sets();
    for panicking in [LevelBuildPart::Geometry, LevelBuildPart::Finish] {
        let (builder, mut pending) = renderer.begin_level_set_build(tiers);
        let events = Arc::new(Mutex::new(Vec::new()));
        {
            let events = Arc::clone(&events);
            assert!(builder.on_event(move |event| {
                // Recorded, and the lock released, before the panic.
                events.lock().unwrap().push(event);
                if event == LevelBuildEvent::PartDone(panicking) {
                    panic!("injected hook panic");
                }
            }));
        }
        fixture.queue(&builder, false);
        builder.finish();
        match take(&mut renderer, &mut pending) {
            Err(LevelBuildError::Upload(UploadError::Panicked(message))) => {
                assert!(message.contains("injected hook panic"), "{message}")
            }
            other => panic!("{panicking:?}: wrong outcome {other:?}"),
        }
        assert_no_second_outcome(&pending, &format!("{panicking:?}"));
        let failed = events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| **event == LevelBuildEvent::Failed)
            .count();
        assert_eq!(failed, 1, "{panicking:?}: `Failed` reported once");
        assert_eq!(
            renderer.resident_level_sets(),
            resident,
            "{panicking:?}: nothing reached the resident sets"
        );
    }
    let id = fixture.build(&mut renderer, tiers);
    assert!(
        renderer.resident_level_sets().contains(&id),
        "the upload thread built the next set"
    );
}

// Settling runs outside a part's `catch_unwind`: a panic in settling itself
// (not a hook's, which `report` contains), on a stored part or after `finish`
// marked the build finished, still ends that build exactly once with the
// panic, reports `Failed` once, publishes nothing, and the upload thread
// serves the next build.
#[test]
fn a_panic_while_a_part_settles_ends_the_build_and_the_thread_serves_on() {
    let test = "a_panic_while_a_part_settles_ends_the_build_and_the_thread_serves_on";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let fixture = legacy_fixture(0x5A);
    let tiers = renderer.level_build_tiers();
    let resident = renderer.resident_level_sets();
    for at in [
        SettlePanic::Stored(LevelBuildPart::Geometry),
        SettlePanic::Finished,
    ] {
        let (builder, mut pending) = renderer.begin_level_set_build(tiers);
        let events = record_events(&builder);
        assert!(builder.panic_while_settling(at));
        fixture.queue(&builder, false);
        builder.finish();
        match take(&mut renderer, &mut pending) {
            Err(LevelBuildError::Upload(UploadError::Panicked(message))) => {
                assert!(message.contains("injected settle panic"), "{message}")
            }
            other => panic!("{at:?}: wrong outcome {other:?}"),
        }
        assert_no_second_outcome(&pending, &format!("{at:?}"));
        let events = events.lock().unwrap().clone();
        let failed = events
            .iter()
            .filter(|event| **event == LevelBuildEvent::Failed)
            .count();
        assert_eq!(failed, 1, "{at:?}: `Failed` reported once: {events:?}");
        assert!(
            !events.contains(&LevelBuildEvent::PartDone(LevelBuildPart::Finish)),
            "{at:?}: the panicking build never reported its finish: {events:?}"
        );
        assert_eq!(
            renderer.resident_level_sets(),
            resident,
            "{at:?}: nothing reached the resident sets"
        );
    }
    let id = fixture.build(&mut renderer, tiers);
    assert!(
        renderer.resident_level_sets().contains(&id),
        "the upload thread built the next set"
    );
}

#[test]
fn upload_writes_submit_in_bounded_chunks() {
    let Some(ctx) = gpu_or_skip("upload_writes_submit_in_bounded_chunks") else {
        return;
    };
    let thread = UploadThread::spawn(ctx.device.clone(), ctx.queue.clone()).unwrap();
    const MIB: u64 = 1024 * 1024;
    let writes = [12 * MIB, 12 * MIB, 12 * MIB, 40 * MIB, 4 * MIB];
    let staged = thread.submit(move |context: &UploadContext| {
        let _ = context.writes.take_batches();
        let small = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chunk proof small"),
            size: 12 * MIB,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let large = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chunk proof large"),
            size: 40 * MIB,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        for bytes in writes {
            let target = if bytes > 12 * MIB { &large } else { &small };
            context
                .writes
                .write_buffer(target, 0, &vec![0u8; bytes as usize]);
        }
        Ok(())
    });
    wait_for(staged).expect("the writes stage cleanly");
    let batches =
        wait_for(thread.submit(|context: &UploadContext| Ok(context.writes.take_batches())))
            .expect("the batch log reads");
    assert_eq!(batches, vec![24 * MIB, 12 * MIB, 40 * MIB, 4 * MIB]);
    assert_eq!(batches.iter().sum::<u64>(), writes.iter().sum::<u64>());
    for batch in &batches {
        assert!(
            *batch <= UPLOAD_SUBMIT_CHUNK_BYTES || writes.contains(batch),
            "batch {batch} exceeds the chunk and is not one oversized write"
        );
    }
}

// A buffer created with contents stages them on the queue like a write, so
// on the upload thread they meter into the same chunks as the job's writes.
#[test]
fn buffer_contents_created_on_the_upload_thread_meter_into_its_chunks() {
    use crate::render::gpu_create::GpuCreate;
    let test = "buffer_contents_created_on_the_upload_thread_meter_into_its_chunks";
    let Some(ctx) = gpu_or_skip(test) else {
        return;
    };
    let thread = UploadThread::spawn(ctx.device.clone(), ctx.queue.clone()).unwrap();
    const MIB: u64 = 1024 * 1024;
    let staged = thread.submit(move |context: &UploadContext| {
        let _ = context.writes.take_batches();
        let target = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("buffer-init meter proof target"),
            size: 24 * MIB,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context
            .writes
            .write_buffer(&target, 0, &vec![0u8; (24 * MIB) as usize]);
        // 16 MiB more would carry the pending 24 MiB past the chunk: the
        // pending batch submits before the buffer's contents stage.
        let _ = context
            .device
            .new_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("buffer-init meter proof contents"),
                contents: &vec![0u8; (16 * MIB) as usize],
                usage: wgpu::BufferUsages::VERTEX,
            });
        Ok(())
    });
    wait_for(staged).expect("the writes and the buffer stage cleanly");
    let batches =
        wait_for(thread.submit(|context: &UploadContext| Ok(context.writes.take_batches())))
            .expect("the batch log reads");
    assert_eq!(batches, vec![24 * MIB, 16 * MIB]);
}

#[test]
fn build_result_follows_its_last_write_submission() {
    let test = "build_result_follows_its_last_write_submission";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let fixture = legacy_fixture(0x5A);
    // No textures part: `finish` itself writes the placeholder material,
    // so the build's last writes are its final job's.
    let (builder, mut pending) = renderer.begin_level_set_build(renderer.level_build_tiers());
    builder.geometry_from(fixture.geometry_source(), rest());
    builder.finish();
    let id = take(&mut renderer, &mut pending).expect("the build succeeds");
    assert_eq!(
        pending.unsubmitted_at_delivery,
        Some(0),
        "every write of the build was submitted before its set was delivered (P13)"
    );
    assert_eq!(
        renderer.level_set(id).unwrap().gpu_textures.len(),
        1,
        "the placeholder slot"
    );
    publish_retire(&mut renderer, id);
    let scope = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    capture(&mut renderer, true);
    assert!(pollster::block_on(scope.pop()).is_none());
}

#[test]
fn tier_created_off_main_registers_at_receipt() {
    let test = "tier_created_off_main_registers_at_receipt";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let fixture = legacy_fixture(0x5A);
    let mut tiers = renderer.level_build_tiers();
    tiers.spot_shadow_map_resolution /= 2;
    let resolution = tiers.spot_shadow_map_resolution;
    assert!(
        renderer.full().shadow_tiers.live(resolution).is_none(),
        "a fresh tier"
    );

    let before = gpu_creation_counts();
    let first = fixture.build(&mut renderer, tiers);
    assert_eq!(
        gpu_creation_counts().since(before).total(),
        0,
        "the tier was built off-main"
    );
    let tier = renderer.level_set(first).unwrap().shadow.tier.clone();
    assert_eq!(tier.resolution(), resolution);
    let registered = renderer
        .full()
        .shadow_tiers
        .live(resolution)
        .expect("registered at receipt");
    assert!(Arc::ptr_eq(&registered, &tier));

    // A second load at the same tier shares it.
    let second = fixture.build(&mut renderer, tiers);
    assert!(Arc::ptr_eq(
        &renderer.level_set(second).unwrap().shadow.tier,
        &tier
    ));

    // Two builds at a new tier begun together each build one; the first
    // receipt registers its tier and the second keeps its own privately.
    tiers.spot_shadow_map_resolution /= 2;
    let (a, mut pending_a) = renderer.begin_level_set_build(tiers);
    let (b, mut pending_b) = renderer.begin_level_set_build(tiers);
    for builder in [&a, &b] {
        fixture.queue(builder, false);
        builder.finish();
    }
    let a = take(&mut renderer, &mut pending_a).unwrap();
    let b = take(&mut renderer, &mut pending_b).unwrap();
    let (tier_a, tier_b) = (
        renderer.level_set(a).unwrap().shadow.tier.clone(),
        renderer.level_set(b).unwrap().shadow.tier.clone(),
    );
    assert!(!Arc::ptr_eq(&tier_a, &tier_b));
    let live = renderer
        .full()
        .shadow_tiers
        .live(tiers.spot_shadow_map_resolution)
        .unwrap();
    assert!(
        Arc::ptr_eq(&live, &tier_a),
        "the live tier stays registered"
    );
}

#[test]
fn tier_change_after_begin_skips_the_pending_build() {
    let test = "tier_change_after_begin_skips_the_pending_build";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, FRAME, FRAME) else {
        return;
    };
    renderer.set_reflection_quality(FluidReflectionQuality::Off);
    let fixture = legacy_fixture(0x5A);
    let tiers = renderer.level_build_tiers();
    let other_depth = match tiers.surface_depth_quality {
        SurfaceDepthQuality::Off => SurfaceDepthQuality::On,
        _ => SurfaceDepthQuality::Off,
    };

    let (builder, mut pending) = renderer.begin_level_set_build(tiers);
    let (fluid_builder, mut fluid_pending) = renderer.begin_level_set_build(tiers);
    // Running-frame tier changes after dispatch.
    renderer.set_spot_shadow_map_resolution(tiers.spot_shadow_map_resolution / 2);
    renderer.set_surface_depth_quality(other_depth);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    fixture.queue(&builder, false);
    builder.finish();
    fluid_builder.geometry_from(fluid_source(), fluid_rest());
    fluid_builder.finish();

    let id = take(&mut renderer, &mut pending).unwrap();
    let level = renderer.level_set(id).unwrap();
    assert_eq!(
        level.shadow.tier.resolution(),
        tiers.spot_shadow_map_resolution
    );
    assert_eq!(level.gpu_textures[0].built_at, tiers.surface_depth_quality);

    let fluid = take(&mut renderer, &mut fluid_pending).unwrap();
    assert!(
        !renderer
            .level_set(fluid)
            .unwrap()
            .fluid_targets
            .has_reflection_target(),
        "built at the dispatch tier (Off): no reflection target"
    );
    // Publishing at the kept tier (High) re-keys the target, counted.
    let before = renderer.reflection_tier_rebuilds();
    publish_retire(&mut renderer, fluid);
    assert_eq!(renderer.reflection_tier_rebuilds(), before + 1);
    assert!(lent_targets(&renderer).0.is_some());
    capture_checked(&mut renderer, base_view());
}

#[test]
fn publish_after_extent_change_rebuilds_once() {
    use postretro_render_cpu::render_extent::Extent;
    let test = "publish_after_extent_change_rebuilds_once";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, FRAME, FRAME) else {
        return;
    };
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    let e1 = renderer.scene_extent();

    // E1 → E2 → E3 between dispatch and receipt: one counted rebuild, and
    // the reflection target follows as part of it.
    let (builder, mut pending) = renderer.begin_level_set_build(renderer.level_build_tiers());
    builder.geometry_from(fluid_source(), fluid_rest());
    renderer.record_surface_size(80, 72);
    renderer.commit_extents();
    renderer.record_surface_size(96, 80);
    renderer.commit_extents();
    let e3 = renderer.scene_extent();
    assert_ne!(e1, e3);
    builder.finish();
    let id = take(&mut renderer, &mut pending).unwrap();
    assert_eq!(renderer.level_set(id).unwrap().extent.extent, e1);
    let before = (
        renderer.extent_rebuilds(),
        renderer.reflection_tier_rebuilds(),
    );
    publish_retire(&mut renderer, id);
    assert_eq!(
        renderer.extent_rebuilds(),
        before.0 + 1,
        "one counted rebuild"
    );
    assert_eq!(
        renderer.reflection_tier_rebuilds(),
        before.1,
        "no tier rebuild"
    );
    assert_eq!(renderer.level().extent.extent, e3);
    capture_checked(&mut renderer, base_view());

    // E3 → E4 → E3: the same extent over recreated targets still rebuilds
    // once, so no group binds a replaced view.
    let (builder, mut pending) = renderer.begin_level_set_build(renderer.level_build_tiers());
    builder.geometry_from(fluid_source(), fluid_rest());
    builder.finish();
    renderer.record_surface_size(64, 64);
    renderer.commit_extents();
    renderer.record_surface_size(96, 80);
    renderer.commit_extents();
    assert_eq!(renderer.scene_extent(), e3);
    let id = take(&mut renderer, &mut pending).unwrap();
    let before = renderer.extent_rebuilds();
    publish_retire(&mut renderer, id);
    assert_eq!(renderer.extent_rebuilds(), before + 1);
    capture_checked(&mut renderer, base_view());

    // A matching extent counts none.
    let id = {
        let tiers = tiers_now(&renderer);
        build_fluid(&mut renderer, tiers)
    };
    let before = renderer.extent_rebuilds();
    publish_retire(&mut renderer, id);
    assert_eq!(renderer.extent_rebuilds(), before);
    assert_eq!(Extent::new(96, 80), renderer.scene_extent());
}

/// The main thread's half of the off-main build (begin, `try_take` and the
/// receipt) never waits: no blocking receive, join, oracle pipeline wait or
/// device wait anywhere in the module's production code.
#[test]
fn off_main_build_main_thread_half_never_blocks() {
    let source = include_str!("level_build.rs");
    let production = source
        .split("#[cfg(test)]\n#[path = \"level_build_tests.rs\"]")
        .next()
        .unwrap();
    for blocking in [
        ".recv()",
        ".recv_timeout(",
        ".join()",
        "wait_for_oracle",
        "block_on",
        "PollType::wait",
        "Condvar",
    ] {
        assert!(
            !production.contains(blocking),
            "level_build.rs reaches a blocking wait: {blocking}"
        );
    }
    assert!(production.contains("self.result.try_recv()"));
}
