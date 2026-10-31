//! Prep-plan equivalence: every CPU install plan, built from level data and
//! the device limits alone (as the load pool builds it), against the
//! main-thread oracle install of the same level.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Equivalence)

use std::path::Path;

use postretro_level_format::prm::PrmFormat;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_render_cpu::animated_lightmap::plan::AnimatedLightmapPlan;
use postretro_render_cpu::fluid_surface::prepared::{
    PreparedFluidSurfaces, prepare_fluid_surfaces,
};
use postretro_render_cpu::lightmap_family::{
    LightmapFamilyInputs, LightmapStreamingInputs, plan_lightmap_family,
};
use postretro_render_cpu::lightmap_pool::{plan_static_pool, plan_streaming_pool};
use postretro_render_cpu::sh_residency::plan_sh_residency;
use postretro_render_cpu::sh_volume::{
    animated_light_count, dynamic_compose_grid_fits, sh_sampler_lattice, usable_sh_volume,
};
use postretro_render_cpu::texture_prep::prepare_world_textures;
use postretro_render_cpu::uv_normalize::diffuse_dims;
use postretro_render_data::material::Material;

use super::gpu_test_harness::offscreen_renderer_or_skip;
use super::renderer_textures::tests::{chain, write_prm};
use super::residency::ResidencyAllocationState;
use super::sh_compose_dispatch::build_dynamic_compose_grid_upload;
use super::*;

/// Diffuse dimensions read the old way: off the textures `install_textures`
/// staged, else the active set's.
fn uploaded_diffuse_dims(renderer: &Renderer) -> Vec<(u32, u32)> {
    let full = renderer.full();
    let loaded = match (&full.pending_textures, &full.level) {
        (Some(pending), _) => &pending.loaded_textures,
        (None, Some(level)) => &level.loaded_textures,
        (None, None) => panic!("no staged or installed materials"),
    };
    super::renderer_models::loaded_texture_dimensions(loaded)
        .into_iter()
        .map(|[width, height]| (width, height))
        .collect()
}

// UV normalize can run right after texture prep: the dimensions it reads
// from prepared textures are the ones the upload creates, bundles and every
// placeholder kind alike.
#[test]
fn prepared_diffuse_dims_match_the_uploaded_textures() {
    let test = "prepared_diffuse_dims_match_the_uploaded_textures";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let small = [0x11u8; 32];
    let wide = [0x22u8; 32];
    let missing = [0x33u8; 32];
    let no_diffuse = [0x44u8; 32];
    write_prm(
        dir.path(),
        small,
        [
            Some(chain(PrmFormat::Rgba8UnormSrgb, 4, 4, 0x5A)),
            None,
            None,
            None,
        ],
    );
    write_prm(
        dir.path(),
        wide,
        [
            Some(chain(PrmFormat::Rgba8UnormSrgb, 8, 4, 0x3C)),
            Some(chain(PrmFormat::R8Unorm, 8, 4, 0x77)),
            None,
            None,
        ],
    );
    write_prm(
        dir.path(),
        no_diffuse,
        [
            None,
            Some(chain(PrmFormat::R8Unorm, 2, 2, 0x10)),
            None,
            None,
        ],
    );
    let names: Vec<String> = ["small", "zero", "missing", "wide", "no_diffuse"]
        .iter()
        .map(|name| name.to_string())
        .collect();
    let keys = TextureCacheKeysSection {
        keys: vec![small, [0u8; 32], missing, wide, no_diffuse],
    };
    let materials = vec![Material::Default; names.len()];

    renderer.install_textures(&names, &keys, dir.path(), &materials);
    let prepared = diffuse_dims(&prepare_world_textures(&names, &keys, dir.path()));
    assert_eq!(prepared, uploaded_diffuse_dims(&renderer));
    assert_eq!(prepared, [(4, 4), (64, 64), (64, 64), (8, 4), (64, 64)]);
}

/// The fluid summary `InstalledFluidSurfaces::prep_summary` reads back.
fn prepared_fluid_summary(prepared: &PreparedFluidSurfaces) -> String {
    format!(
        "{:?}",
        (
            prepared.mesh.vertices.len(),
            (prepared.mesh.indices.len() * 4) as u64,
            (prepared.probe_indices.len() * 4) as u64,
            &prepared.planar_parts,
            prepared.has_probe_bound_face,
            prepared.mesh.grid,
        )
    )
}

/// The summary `AnimatedLightmapResources::plan_summary` reads back for the
/// resource the GPU half builds from `plan` (a rejected plan binds the
/// fallback dummy).
fn animated_plan_summary(plan: &Result<AnimatedLightmapPlan, String>) -> String {
    type Dispatch<'a> = Option<(
        &'a Vec<postretro_render_cpu::animated_lightmap::plan::DispatchTile>,
        &'a Vec<u32>,
        u32,
        u32,
    )>;
    let (state, supplied, extent, dispatch): (_, _, Option<wgpu::Extent3d>, Dispatch<'_>) =
        match plan {
            Ok(AnimatedLightmapPlan::Absent) => {
                (ResidencyAllocationState::Dummy, false, None, None)
            }
            Ok(AnimatedLightmapPlan::Empty) => (ResidencyAllocationState::Data, true, None, None),
            Ok(AnimatedLightmapPlan::NoBlockFrame) | Err(_) => {
                (ResidencyAllocationState::Fallback, true, None, None)
            }
            Ok(AnimatedLightmapPlan::Atlas(atlas)) => (
                ResidencyAllocationState::Data,
                true,
                Some(wgpu::Extent3d {
                    width: atlas.extent.width,
                    height: atlas.extent.height,
                    depth_or_array_layers: atlas.extent.depth_or_array_layers,
                }),
                Some((
                    &atlas.master_tiles,
                    &atlas.chunk_cell_ids,
                    atlas.master_tiles.len() as u32,
                    atlas.max_workgroups_per_dim,
                )),
            ),
        };
    format!("{:?}", (state, supplied, extent, dispatch))
}

/// Install `world` through the main-thread oracle, after building every CPU
/// install plan from the level data and the device limits alone, and assert
/// each plan matches what the install built: the streamed-SH residency
/// state, the SH volume's animated count and sampler lattice, the static
/// lightmap pool and animated atlas, and the fluid surfaces. With a
/// `prm_root` the level's textures install first and their prepared
/// diffuse dimensions are checked too.
/// Which plan kinds a level exercised, so a sweep can prove it covered each.
#[derive(Debug, Default, Clone, Copy)]
struct PrepCoverage {
    streamed_sh: bool,
    legacy_sh: bool,
    animated_atlas: bool,
    fluid: bool,
}

fn assert_prep_matches_install(
    renderer: &mut Renderer,
    label: &str,
    mut world: postretro_level_loader::LevelWorld,
    prm_root: Option<&Path>,
) -> PrepCoverage {
    let limits = renderer.install_device_limits();
    let materials = vec![Material::Default; world.texture_names.len()];
    let dims = match prm_root {
        Some(root) => {
            renderer.install_textures(
                &world.texture_names,
                &world.texture_cache_keys,
                root,
                &materials,
            );
            let prepared = diffuse_dims(&prepare_world_textures(
                &world.texture_names,
                &world.texture_cache_keys,
                root,
            ));
            assert_eq!(
                prepared,
                uploaded_diffuse_dims(renderer),
                "{label}: diffuse dims"
            );
            prepared
        }
        None => Vec::new(),
    };
    renderer.normalize_world_uvs(&mut world);

    let sh_plan = world.sh_stream_manifest().map(|manifest| {
        plan_sh_residency(manifest, &world.cells).expect("the plan accepts the level")
    });
    let payloads = world.take_gpu_lighting_payloads();
    let geometry = level_world_to_geometry(&world, &materials);
    let (stream_base, stream_descriptors) = match &geometry.sh_storage {
        LevelGeometryShStorage::Legacy => (None, None),
        LevelGeometryShStorage::Streaming(manifest) => (
            Some((manifest.base().grid_origin, manifest.base().cell_size)),
            Some(manifest.base().animation_descriptors.as_slice()),
        ),
    };
    let usable = usable_sh_volume(geometry.sh_volume, &limits);
    let cpu_animated_count = animated_light_count(usable, stream_descriptors);
    let cpu_lattice = sh_sampler_lattice(
        stream_base,
        usable.map(|section| (section.grid_origin, section.cell_size)),
    );
    let family = plan_lightmap_family(
        &LightmapFamilyInputs {
            lightmap: geometry.lightmap,
            shadowmask: geometry.shadowmask_atlas,
            streaming: geometry
                .lightmap_streaming
                .map(|streaming| LightmapStreamingInputs {
                    content_tag: streaming.content_tag,
                    pool_cap_layers: streaming.pool_cap_layers,
                }),
            payload_blocks: &payloads.blocks,
            weight_maps: geometry.animated_light_weight_maps,
            animated_chunks: geometry.animated_light_chunks,
            bvh_leaves: &geometry.bvh.leaves,
            animated_light_count: cpu_animated_count,
        },
        &limits,
    );
    // The static pool the install planned the old way, on the live limits.
    let device_limits = renderer.device.limits();
    let legacy_static_pool = match geometry.lightmap_streaming {
        Some(streaming) => plan_streaming_pool(
            geometry.lightmap,
            geometry.shadowmask_atlas,
            streaming.content_tag,
            streaming.pool_cap_layers,
            device_limits.max_texture_dimension_2d,
            device_limits.max_texture_array_layers,
        ),
        None => plan_static_pool(
            geometry.lightmap,
            geometry.shadowmask_atlas,
            &payloads.blocks,
            device_limits.max_texture_dimension_2d,
            device_limits.max_texture_array_layers,
        ),
    };
    assert_eq!(
        family.static_pool, legacy_static_pool,
        "{label}: static pool"
    );

    renderer
        .install_level_geometry(&geometry, payloads)
        .expect("level resources install");
    let material_count = renderer.level().gpu_textures.len();
    if prm_root.is_some() {
        assert_eq!(material_count, world.texture_names.len(), "{label}");
    }
    let dims = if prm_root.is_some() {
        dims
    } else {
        uploaded_diffuse_dims(renderer)
    };
    let prepared_fluid = prepare_fluid_surfaces(
        &world.environment_volumes.volumes,
        world.reflection_probes.as_ref(),
        &dims,
        material_count,
        cpu_lattice.as_ref(),
    );
    renderer.install_fluid_surfaces(
        &world.environment_volumes.volumes,
        world.reflection_probes.as_ref(),
    );

    let level = renderer.level();
    assert_eq!(
        level
            .sh_streaming
            .as_ref()
            .map(sh_streaming::ShResidencyState::plan_summary),
        sh_plan.as_ref().map(sh_streaming::residency_plan_summary),
        "{label}: SH residency plan"
    );
    assert_eq!(
        level.sh_volume_resources.animation.animated_light_count(),
        cpu_animated_count,
        "{label}: animated light count"
    );
    assert_eq!(
        level.sh_sampler_lattice(),
        cpu_lattice,
        "{label}: SH sampler lattice"
    );
    assert_eq!(
        level.animated_lightmap.plan_summary(),
        animated_plan_summary(&family.animated),
        "{label}: animated lightmap"
    );
    assert_eq!(
        level
            .fluid_surfaces
            .as_ref()
            .map(fluid_surface::InstalledFluidSurfaces::prep_summary),
        prepared_fluid.as_ref().map(prepared_fluid_summary),
        "{label}: fluid surfaces"
    );
    PrepCoverage {
        streamed_sh: sh_plan.is_some(),
        legacy_sh: level.sh_volume_resources.present,
        animated_atlas: matches!(family.animated, Ok(AnimatedLightmapPlan::Atlas(_))),
        fluid: prepared_fluid.is_some(),
    }
}

// Every plan prep builds for a streamed level matches the oracle install:
// SH residency (owner walk, layout, ownership, floors, pools, row space,
// sample-region tables, sparse compose layouts, slot minima), the lattice,
// and the lightmap family.
#[test]
fn prep_plans_match_the_oracle_install_on_a_streamed_fixture() {
    let test = "prep_plans_match_the_oracle_install_on_a_streamed_fixture";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let (_temp, path) = sh_streaming::write_nine_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    assert!(world.sh_stream_manifest().is_some(), "the fixture streams");
    let coverage = assert_prep_matches_install(&mut renderer, "nine-cluster", world, None);
    assert!(coverage.streamed_sh);
}

// The same on every compiled dev-mod map, with its baked textures: SH
// streamed or legacy, lightmaps all-resident or streamed, animated atlases,
// fluids and probes, as each map exercises them.
#[test]
#[ignore = "requires the compiled dev-mod maps and their baked .prm cache"]
fn prep_plans_match_the_oracle_install_on_dev_maps() {
    let test = "prep_plans_match_the_oracle_install_on_dev_maps";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 64, 64) else {
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let mut maps: Vec<_> = std::fs::read_dir(root.join("maps"))
        .expect("content/dev/maps exists")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "prl"))
        .collect();
    maps.sort();
    assert!(!maps.is_empty(), "compile the dev-mod maps first");
    let mut checked = 0;
    let mut covered = PrepCoverage::default();
    for map in maps {
        let label = map.file_stem().unwrap().to_string_lossy().into_owned();
        // A stale compiled map the current loader refuses is not a prep case.
        let world = match postretro_level_loader::load_prl(map.to_str().unwrap()) {
            Ok(world) => world,
            Err(error) => {
                eprintln!("[PrepEquivalence] {label}: skipped, {error}");
                continue;
            }
        };
        let coverage =
            assert_prep_matches_install(&mut renderer, &label, world, Some(&root.join("cache")));
        renderer.release_level_resources();
        // Free the released set's GPU memory before the next map installs.
        renderer
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the device drains");
        eprintln!(
            "[PrepEquivalence] {label}: every plan matches the oracle install ({coverage:?})"
        );
        covered.streamed_sh |= coverage.streamed_sh;
        covered.legacy_sh |= coverage.legacy_sh;
        covered.animated_atlas |= coverage.animated_atlas;
        covered.fluid |= coverage.fluid;
        checked += 1;
    }
    assert!(checked > 0, "no current dev-mod map loaded");
    eprintln!("[PrepEquivalence] {checked} map(s), covered {covered:?}");
    assert!(
        covered.streamed_sh && covered.animated_atlas && covered.fluid,
        "the sweep exercises streamed SH, an animated atlas and fluid surfaces: {covered:?}"
    );
}

// The compose-grid check `check_sh_volume` adds is exactly the upload
// builder's fit: what makes the legacy compose constructors' `expect`
// unreachable from level data.
#[test]
fn dynamic_compose_grid_fits_matches_the_upload_builder() {
    let grid = |affinity_dims| postretro_render_cpu::sh_compose::ComposeGridParams {
        grid_dimensions: [4, 4, 4],
        atlas_dimensions: [8, 8],
        tile_dimension: 6,
        tile_border: 1,
        atlas_tiles_per_row: 1,
        tiles_per_layer: 1,
        atlas_layer_count: 1,
        affinity_dims,
        compact_atlas_tiles_per_row: 1,
        compact_atlas_tiles_per_layer: 1,
    };
    let dims = [
        [1, 1, 1],
        [0, 3, 1],
        [16_363, 1, 1],
        [16_364, 1, 1],
        [16_365, 1, 1],
        [40_000, 3, 2],
        [65_536, 65_536, 2],
        [u32::MAX, 2, 1],
    ];
    for affinity_dims in dims {
        for max_workgroups_x in [0, 1, 300, 65_535] {
            for alignment in [0, 1, 64, 256] {
                for max_buffer_size in [65_535, 65_536, 1 << 20, 16 << 20] {
                    // Skip sweeps whose row list alone would be large; the
                    // builder materializes every row id.
                    let rows = affinity_dims
                        .iter()
                        .try_fold(1u64, |count, &d| count.checked_mul(u64::from(d)));
                    if rows.is_some_and(|rows| rows <= u64::from(u32::MAX) && rows > 1 << 22) {
                        continue;
                    }
                    let builder = build_dynamic_compose_grid_upload(
                        grid(affinity_dims),
                        1,
                        max_workgroups_x,
                        alignment,
                        max_buffer_size,
                    )
                    .is_some();
                    assert_eq!(
                        dynamic_compose_grid_fits(
                            affinity_dims,
                            max_workgroups_x,
                            alignment,
                            max_buffer_size
                        ),
                        builder,
                        "{affinity_dims:?} wg={max_workgroups_x} align={alignment} max={max_buffer_size}"
                    );
                }
            }
        }
    }
    // Overflowing row counts never reach the builder's allocation: both refuse.
    assert!(!dynamic_compose_grid_fits(
        [u32::MAX, 2, 1],
        65_535,
        256,
        u64::MAX
    ));
}
