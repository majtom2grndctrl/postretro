//! The prep tasks' bodies: pure functions over hand-outs, the assembled
//! level and the dispatch inputs. Every level-data check here is the same
//! function the main-thread oracle install calls.
//! See: context/plans/in-progress/level-load-pipeline/index.md (The commit cannot fail on level data)

use std::path::Path;

use postretro_level_loader::{
    CompleteHandOuts, GeometryHandOut, LevelWorld, ShManifestMetadata, ShStreamingMode,
    SpatialHandOut, assemble,
};
use postretro_render_cpu::device_limits::DeviceLimits;
use postretro_render_cpu::fluid_surface::prepared::{
    PreparedFluidSurfaces, prepare_fluid_surfaces,
};
use postretro_render_cpu::lightmap_family::{LightmapFamilyPlan, plan_lightmap_family};
use postretro_render_cpu::sh_residency::{ShResidencyPlan, plan_sh_residency_parts};
use postretro_render_cpu::sh_volume::{animated_light_count, sh_sampler_lattice, usable_sh_volume};
use postretro_render_cpu::texture_prep::{PreparedWorldTextures, prepare_world_textures};
use postretro_render_cpu::uv_normalize::{diffuse_dims, normalize_world_uvs_with_dims};
use postretro_render_data::material::Material;

use crate::collision::CollisionWorld;
use crate::render::{LevelInstallError, level_world_to_geometry, lightmap_family_inputs};
use crate::session::level_streaming::{
    StreamingLevelData, StreamingLevelParts, decode_streaming_level_data,
};

/// Texture prep's product: the CPU pass, the material table, and the dims UV
/// normalize and the fluid pack read.
#[derive(Debug)]
pub(super) struct TexturePrep {
    pub(super) textures: PreparedWorldTextures,
    pub(super) materials: Vec<Material>,
    pub(super) dims: Vec<(u32, u32)>,
}

/// Hand-out 2 alone: read, parse and plan every world sidecar, then the dims
/// the GPU pass will bind and each name's material. Never fails: an unusable
/// sidecar degrades to a placeholder with a warning, as the oracle's does.
pub(super) fn prep_textures(geometry: &GeometryHandOut, prm_cache_root: &Path) -> TexturePrep {
    let textures = prepare_world_textures(
        &geometry.texture_names,
        &geometry.texture_cache_keys,
        prm_cache_root,
    );
    let dims = diffuse_dims(&textures);
    TexturePrep {
        materials: level_texture_materials(&geometry.texture_names),
        textures,
        dims,
    }
}

/// Each world texture's material, derived from its name; an unknown prefix
/// warns once and takes the default. The oracle install's renderer stage and
/// prep both build the table here.
pub(crate) fn level_texture_materials(texture_names: &[String]) -> Vec<Material> {
    let mut warned = std::collections::HashSet::new();
    texture_names
        .iter()
        .map(|name| {
            let warned_count = warned.len();
            let material = postretro_render_data::material::derive_material(name, &mut warned);
            let prefix = postretro_render_data::material::parse_prefix(name);
            if material == Material::Default && !prefix.is_empty() && warned.len() > warned_count {
                log::warn!(
                    "[Material] Unknown prefix '{prefix}' in texture '{name}' — using default material",
                );
            }
            material
        })
        .collect()
}

/// Hand-outs 1 and 3: the streamed-SH residency plan, from the manifest
/// metadata (no cluster topology) and the cells' bounds. Reads no id-49
/// member, so the unchecked members before the verdict never reach it. The
/// same plan the oracle's `install_level_geometry` builds first.
pub(super) fn plan_streamed_sh(
    metadata: &ShManifestMetadata,
    spatial: &SpatialHandOut,
) -> Result<ShResidencyPlan, String> {
    plan_sh_residency_parts(
        metadata.content_tag(),
        metadata.cluster_directory(),
        metadata.base(),
        metadata.sources(),
        &spatial.cells,
    )
    .map_err(|error| LevelInstallError::StreamedSh(error).to_string())
}

/// Hand-outs 2 and 3: the release indirect draws' index-range check, the one
/// the oracle install runs before any mutation. Bounds-checked, so it holds
/// on hand-outs the parse may yet reject.
pub(super) fn check_geometry_ranges(
    geometry: &GeometryHandOut,
    spatial: &SpatialHandOut,
) -> Result<(), String> {
    crate::render::validate_level_geometry_ranges(&spatial.bvh.leaves, geometry.indices.len())
        .map_err(|error| error.to_string())
}

/// The accepted parse's hand-outs as one level, its world and mover UVs
/// normalized by the bound diffuse dims: the vertices the upload uploads.
pub(super) fn assemble_level(hand_outs: CompleteHandOuts, dims: &[(u32, u32)]) -> LevelWorld {
    let mut world = assemble(hand_outs);
    normalize_world_uvs_with_dims(&mut world, dims);
    world
}

/// The static collision world the commit installs; the oracle builds it with
/// the same call before any install mutation.
pub(super) fn build_collision(world: &LevelWorld) -> Result<CollisionWorld, String> {
    CollisionWorld::from_level(world).map_err(|error| format!("static collision: {error}"))
}

/// The streaming sessions' level-data checks, on the completed parse's
/// manifest (its cluster topology installed).
pub(super) fn decode_streaming(
    world: &LevelWorld,
    sh_streaming_mode: &Result<ShStreamingMode, String>,
) -> Result<StreamingLevelData, String> {
    decode_streaming_level_data(StreamingLevelParts::of(world), || {
        sh_streaming_mode.clone().map_err(anyhow::Error::msg)
    })
    .map_err(|error| format!("{error:#}"))
}

/// The SH volume's animated descriptor count and sampler lattice, as the
/// renderer binds them on these limits.
fn sh_volume_view(
    world: &LevelWorld,
    limits: &DeviceLimits,
) -> (
    u32,
    Option<postretro_render_cpu::fluid_surface::FluidGridLattice>,
) {
    let usable = usable_sh_volume(world.sh_volume(), limits);
    let streamed = world.sh_stream_manifest().map(|manifest| manifest.base());
    let count = animated_light_count(
        usable,
        streamed.map(|base| base.animation_descriptors.as_slice()),
    );
    let lattice = sh_sampler_lattice(
        streamed.map(|base| (base.grid_origin, base.cell_size)),
        usable.map(|section| (section.grid_origin, section.cell_size)),
    );
    (count, lattice)
}

/// The lightmap family's CPU plan: the static pool and the animated atlas,
/// from the same inputs the oracle install plans from.
pub(super) fn plan_lightmap(
    world: &LevelWorld,
    limits: &DeviceLimits,
    pool_cap_layers: u32,
) -> LightmapFamilyPlan {
    // The plan reads no material.
    let mut geometry = level_world_to_geometry(world, &[]);
    if let Some(streaming) = geometry.lightmap_streaming.as_mut() {
        streaming.pool_cap_layers = pool_cap_layers;
    }
    let (animated_light_count, _) = sh_volume_view(world, limits);
    plan_lightmap_family(
        &lightmap_family_inputs(
            &geometry,
            &world.gpu_lighting_payloads,
            &geometry.bvh.leaves,
            animated_light_count,
        ),
        limits,
    )
}

/// The fluid surfaces' CPU pack: normalized by the bound dims, one material
/// per bound slot, cut along the SH sampler lattice.
pub(super) fn prep_fluid(
    world: &LevelWorld,
    dims: &[(u32, u32)],
    limits: &DeviceLimits,
) -> Option<PreparedFluidSurfaces> {
    let (_, lattice) = sh_volume_view(world, limits);
    prepare_fluid_surfaces(
        &world.environment_volumes.volumes,
        world.reflection_probes.as_ref(),
        dims,
        dims.len(),
        lattice.as_ref(),
    )
}
