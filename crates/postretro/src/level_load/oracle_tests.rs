//! Prep against today's path: every product the staged parse and the prep
//! tasks build equals what the main-thread oracle computes from the
//! monolithic parse of the same file, through the oracle's own functions.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Equivalence)

use std::path::Path;

use postretro_level_loader::LevelWorld;
use postretro_render_cpu::fluid_surface::prepared::prepare_fluid_surfaces;
use postretro_render_cpu::lightmap_family::plan_lightmap_family;
use postretro_render_cpu::sh_residency::plan_sh_residency;
use postretro_render_cpu::sh_volume::{animated_light_count, sh_sampler_lattice, usable_sh_volume};
use postretro_render_cpu::texture_prep::{PreparedTexture, prepare_world_textures};
use postretro_render_cpu::uv_normalize::{diffuse_dims, normalize_world_uvs_with_dims};

use super::test_support::{
    inputs_for, no_prm_root, offscreen_renderer_or_skip, run_prep, test_pool, write_textured_level,
};
use super::*;
use crate::collision::CollisionWorld;
use crate::session::level_streaming::{StreamingLevelParts, decode_streaming_level_data};

/// Every upload-shaping property of the CPU texture pass, payloads by length
/// and checksum.
pub(super) fn texture_summary(entries: &[PreparedTexture]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| match entry {
            PreparedTexture::Placeholder => "placeholder".to_string(),
            PreparedTexture::Bundle { name, slots, plan } => {
                let slots: Vec<String> = slots
                    .iter()
                    .map(|slot| match slot {
                        Ok(slot) => format!(
                            "{:?} {}x{} l{} {}b #{}",
                            slot.format,
                            slot.width,
                            slot.height,
                            slot.level_count,
                            slot.payload.len(),
                            slot.payload
                                .iter()
                                .fold(0u64, |sum, byte| sum.wrapping_mul(31) ^ u64::from(*byte))
                        ),
                        Err(error) => format!("err {error}"),
                    })
                    .collect();
                format!("{name} {plan:?} {slots:?}")
            }
        })
        .collect()
}

fn collision_summary(collision: &CollisionWorld) -> (usize, usize) {
    (collision.triangle_count(), collision.vertex_count())
}

/// The world vertices and mover vertices the upload would upload.
fn uploaded_vertices(world: &LevelWorld) -> String {
    format!(
        "{:?} {:?}",
        world.vertices,
        world
            .kinematic_geometry
            .movers
            .iter()
            .map(|mover| &mover.vertices)
            .collect::<Vec<_>>()
    )
}

/// Which products a level exercised, so a sweep can prove it covered each.
#[derive(Debug, Default, Clone, Copy)]
struct Coverage {
    baked_texture: bool,
    streamed_sh: bool,
    streamed_lightmap: bool,
    animated_atlas: bool,
    fluid: bool,
}

/// Prep `map` and compare every product with the oracle's computation over
/// the monolithic parse. With a renderer, the oracle's UV normalize runs on
/// the textures its `install_textures` uploaded, and plans check against
/// its device limits.
fn assert_prep_matches_oracle(
    label: &str,
    map: &Path,
    prm_cache_root: &Path,
    renderer: Option<&mut crate::render::Renderer>,
) -> Coverage {
    let pool = test_pool();
    let mut inputs = inputs_for(map, prm_cache_root);
    if let Some(renderer) = renderer.as_deref() {
        inputs.device_limits = renderer.install_device_limits();
    }
    let limits = inputs.device_limits;
    let sh_mode = inputs.sh_streaming_mode.clone();
    let pool_cap = inputs.lightmap_pool_cap_layers;
    let (prep, outcome, _progress) = run_prep(&pool, inputs);
    drop(prep.cancel());
    let prepared = outcome.unwrap_or_else(|failure| panic!("{label}: prep failed: {failure}"));

    let mut world = postretro_level_loader::load_prl(map.to_str().unwrap())
        .unwrap_or_else(|error| panic!("{label}: the monolithic parse failed: {error}"));

    // Textures: the oracle's CPU pass and material table.
    let oracle_textures = prepare_world_textures(
        &world.texture_names,
        &world.texture_cache_keys,
        prm_cache_root,
    );
    assert_eq!(
        texture_summary(&prepared.gpu.textures.entries),
        texture_summary(&oracle_textures.entries),
        "{label}: prepared textures"
    );
    let materials = level_texture_materials(&world.texture_names);
    assert_eq!(
        prepared.gpu.texture_materials, materials,
        "{label}: materials"
    );
    let dims = diffuse_dims(&oracle_textures);
    assert_eq!(prepared.gpu.diffuse_dims, dims, "{label}: diffuse dims");

    // Normalized vertices: the oracle's renderer path when there is one.
    match renderer {
        Some(renderer) => {
            renderer.install_textures(
                &world.texture_names,
                &world.texture_cache_keys,
                prm_cache_root,
                &materials,
            );
            renderer.normalize_world_uvs(&mut world);
        }
        None => normalize_world_uvs_with_dims(&mut world, &dims),
    }
    assert!(
        uploaded_vertices(&prepared.world) == uploaded_vertices(&world),
        "{label}: normalized vertices differ"
    );

    // SH residency plan: what `install_level_geometry` plans first.
    let oracle_sh = world
        .sh_stream_manifest()
        .map(|manifest| plan_sh_residency(manifest, &world.cells).expect("the oracle plan"));
    assert_eq!(
        format!("{:?}", prepared.gpu.sh_residency),
        format!("{oracle_sh:?}"),
        "{label}: SH residency plan"
    );

    // Lightmap family and fluids, from the oracle's geometry adapter.
    let mut geometry = crate::render::level_world_to_geometry(&world, &materials);
    if let Some(streaming) = geometry.lightmap_streaming.as_mut() {
        streaming.pool_cap_layers = pool_cap;
    }
    let usable = usable_sh_volume(geometry.sh_volume, &limits);
    let stream_base = world.sh_stream_manifest().map(|manifest| manifest.base());
    let animated = animated_light_count(
        usable,
        stream_base.map(|base| base.animation_descriptors.as_slice()),
    );
    let family = plan_lightmap_family(
        &crate::render::lightmap_family_inputs(
            &geometry,
            &world.gpu_lighting_payloads,
            &geometry.bvh.leaves,
            animated,
        ),
        &limits,
    );
    assert_eq!(
        prepared.gpu.lightmap_family, family,
        "{label}: lightmap family plan"
    );
    let lattice = sh_sampler_lattice(
        stream_base.map(|base| (base.grid_origin, base.cell_size)),
        usable.map(|section| (section.grid_origin, section.cell_size)),
    );
    let fluid = prepare_fluid_surfaces(
        &world.environment_volumes.volumes,
        world.reflection_probes.as_ref(),
        &dims,
        dims.len(),
        lattice.as_ref(),
    );
    assert_eq!(prepared.gpu.fluid, fluid, "{label}: fluid surfaces");

    // Streaming level data: what `install_level_streaming_sessions` decodes.
    let oracle_streaming = decode_streaming_level_data(StreamingLevelParts::of(&world), || {
        sh_mode.clone().map_err(anyhow::Error::msg)
    })
    .unwrap_or_else(|error| panic!("{label}: the oracle's streaming checks: {error:#}"));
    assert_eq!(
        prepared.streaming.summary(),
        oracle_streaming.summary(),
        "{label}: streaming level data"
    );

    // Collision: the install's pre-mutation build.
    let oracle_collision = CollisionWorld::from_level(&world).expect("the oracle collision");
    assert_eq!(
        collision_summary(&prepared.collision),
        collision_summary(&oracle_collision),
        "{label}: collision"
    );

    Coverage {
        baked_texture: prepared
            .gpu
            .textures
            .entries
            .iter()
            .any(|entry| matches!(entry, PreparedTexture::Bundle { .. })),
        streamed_sh: prepared.streaming.sh.is_some(),
        streamed_lightmap: prepared.streaming.lightmap.is_some(),
        animated_atlas: matches!(
            prepared.gpu.lightmap_family.animated,
            Ok(postretro_render_cpu::animated_lightmap::plan::AnimatedLightmapPlan::Atlas(_))
        ),
        fluid: prepared.gpu.fluid.is_some(),
    }
}

// The app's level fixtures: a textured level whose lightmap streams (a real
// sidecar, UV normalize, a collision mesh), a streamed-SH level, and the
// plain streamed-lightmap level with no textures (the placeholder slot).
#[test]
fn prep_products_match_the_oracle() {
    let mut renderer = offscreen_renderer_or_skip("prep_products_match_the_oracle");

    let (textured, prm_root) = write_textured_level();
    let coverage = assert_prep_matches_oracle(
        "textured",
        textured.path(),
        prm_root.path(),
        renderer.as_mut(),
    );
    assert!(
        coverage.baked_texture && coverage.streamed_lightmap,
        "{coverage:?}"
    );

    let (_dir, streamed_sh) =
        crate::sh_streaming::sync_manifest_test_fixture::write_one_cluster_prl();
    let coverage = assert_prep_matches_oracle(
        "streamed-sh",
        &streamed_sh,
        &no_prm_root(),
        renderer.as_mut(),
    );
    assert!(coverage.streamed_sh, "{coverage:?}");

    let lightmap = crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl::write();
    let coverage = assert_prep_matches_oracle(
        "streamed-lightmap",
        lightmap.path(),
        &no_prm_root(),
        renderer.as_mut(),
    );
    assert!(coverage.streamed_lightmap, "{coverage:?}");
}

// Every compiled dev-mod map, with its baked textures. A map the current
// loader refuses (a stale bake) is skipped with its reason.
#[test]
#[ignore = "requires the compiled dev-mod maps and their baked .prm sidecars"]
fn prep_products_match_the_oracle_on_dev_maps() {
    let mut renderer = offscreen_renderer_or_skip("prep_products_match_the_oracle_on_dev_maps");
    let content_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let prm_root = crate::startup::worker::derive_prm_root_dev_layout(&content_root);
    let mut maps: Vec<_> = std::fs::read_dir(content_root.join("maps"))
        .expect("content/dev/maps exists")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "prl"))
        .collect();
    maps.sort();
    assert!(!maps.is_empty(), "compile the dev-mod maps first");
    let mut checked = 0;
    let mut covered = Coverage::default();
    for map in maps {
        let label = map.file_stem().unwrap().to_string_lossy().into_owned();
        if let Err(error) = postretro_level_loader::load_prl(map.to_str().unwrap()) {
            eprintln!("[PrepOracle] {label}: skipped, stale bake: {error}");
            continue;
        }
        let coverage = assert_prep_matches_oracle(&label, &map, &prm_root, renderer.as_mut());
        if let Some(renderer) = renderer.as_mut() {
            renderer.release_level_resources();
        }
        eprintln!("[PrepOracle] {label}: every product matches the oracle ({coverage:?})");
        covered.baked_texture |= coverage.baked_texture;
        covered.streamed_sh |= coverage.streamed_sh;
        covered.streamed_lightmap |= coverage.streamed_lightmap;
        covered.animated_atlas |= coverage.animated_atlas;
        covered.fluid |= coverage.fluid;
        checked += 1;
    }
    assert!(checked > 0, "no current dev-mod map loaded");
    eprintln!("[PrepOracle] {checked} map(s), covered {covered:?}");
}
