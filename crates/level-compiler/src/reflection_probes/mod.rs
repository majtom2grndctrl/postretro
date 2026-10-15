//! Reflection-probe capture stage (PRL id 54): derive probes over up-facing
//! fluid faces, capture each probe's static world into an octahedral tile.
//! See: context/lib/resource_management.md §5 · build_pipeline.md §Compiler pipeline, §Build Cache

mod box_fit;
mod capture;
mod hit;
mod lightmap_sample;
mod materials;
mod placement;
mod sh_sample;
mod shading;

#[cfg(test)]
mod reflection_probes_test_fixtures;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use bvh::bvh::Bvh;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, LightmapSection};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_LIGHTS_PER_PROBE, REFLECTION_PROBE_TILE_BORDER,
    REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbeLightShare, ReflectionProbeRecord,
    ReflectionProbesSection,
};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use rayon::prelude::*;

use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;

use box_fit::WorldBounds;
use capture::{SUPERSAMPLE_OFFSETS, TILE_INTERIOR, capture_row, encode_tile};
use hit::SceneTracer;
use lightmap_sample::LightmapSampler;
use materials::SurfaceMaterials;
use placement::{PlacedProbe, apply_probe_cap, bindings, derive_probes, up_facing_faces};
use sh_sample::{PRODUCTION_PROBE_OCCLUSION, ShVolumeSampler};
use shading::{CaptureScene, PRODUCTION_INDIRECT_WEIGHT};

/// Cache stage identity for the whole id-54 section.
const STAGE_ID: &str = "reflection_probes";
/// Bump when placement, capture shading, sampling or the payload changes.
const STAGE_VERSION: u32 = 1;

/// What the stage reads. Everything here enters its cache key.
pub(crate) struct ReflectionProbeInputs<'a> {
    /// The static world: movers and environment volumes are not in it.
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    /// Id 52, whose up-facing fluid faces derive the probes.
    pub environment_volumes: &'a EnvironmentVolumesSection,
    /// Id 22 as encoded: the capture decodes the texels it reads.
    pub lightmap: &'a LightmapSection,
    /// The packed pre-BC6H base SH volume id 34 encodes. Warm SH differs from
    /// cold, so the key hashes this output, not the SH bake's inputs.
    pub sh_volume: &'a OctahedralShVolumeSection,
    /// Texture name → `.prm` key, from the TextureMips stage.
    pub texture_keys: &'a HashMap<String, [u8; 32]>,
    pub prm_root: &'a Path,
}

/// Id 54 for the level, from the cache when its key hits; `None` when no
/// probe is kept, so the section is omitted.
pub(crate) fn bake_or_load_reflection_probes(
    inputs: &ReflectionProbeInputs<'_>,
    stage_cache: Option<&StageCache>,
    control: &BakeControl,
) -> anyhow::Result<Option<ReflectionProbesSection>> {
    if up_facing_faces(inputs.environment_volumes).is_empty() {
        return Ok(None);
    }
    // Placement and capture are pure functions of the keyed inputs, so a hit
    // skips both.
    let key = cache_key(inputs)?;
    if let Some(section) = stage_cache
        .and_then(|cache| cache.get(&key))
        .and_then(|bytes| {
            ReflectionProbesSection::from_bytes(&bytes)
                .map_err(|e| log::warn!("[cache] corrupt reflection_probes entry, re-baking: {e}"))
                .ok()
        })
    {
        log::info!("[cache] reflection_probes hit");
        log_probes(&section);
        return Ok(Some(section));
    }
    log::info!("[cache] reflection_probes miss");

    let Some(bounds) = WorldBounds::from_positions(
        inputs
            .geometry
            .geometry
            .vertices
            .iter()
            .map(|v| &v.position),
    ) else {
        log::warn!("[ReflectionProbes] the level has no static world to reflect; no probes");
        return Ok(None);
    };
    let tracer = SceneTracer::new(inputs.bvh, inputs.primitives, inputs.geometry);
    let probes = apply_probe_cap(derive_probes(inputs.environment_volumes, &tracer, &bounds));
    if probes.is_empty() {
        return Ok(None);
    }

    let scene = CaptureScene {
        tracer,
        materials: SurfaceMaterials::new(
            &inputs.geometry.texture_names.names,
            inputs.texture_keys,
            inputs.prm_root,
        ),
        lightmap: LightmapSampler::new(inputs.lightmap),
        sh: ShVolumeSampler::new(inputs.sh_volume, PRODUCTION_PROBE_OCCLUSION)?,
        max_distance: ((bounds.max - bounds.min).length() + 1.0) as f32,
    };
    let section = capture_section(&scene, &probes, control)?;
    if let Some(cache) = stage_cache {
        cache.put(&key, &section.to_bytes());
    }
    log_probes(&section);
    Ok(Some(section))
}

/// Capture every probe's tile. Work items are tile rows, so one probe spreads
/// across workers; rows land in item order, so the bytes never depend on
/// the worker count.
fn capture_section(
    scene: &CaptureScene<'_>,
    probes: &[PlacedProbe],
    control: &BakeControl,
) -> anyhow::Result<ReflectionProbesSection> {
    let rows_per_tile = TILE_INTERIOR as usize;
    let items: Vec<(usize, u32)> = (0..probes.len())
        .flat_map(|probe| (0..TILE_INTERIOR).map(move |row| (probe, row)))
        .collect();
    control.publish_total(items.len());
    let rows = items
        .par_iter()
        .map(|&(probe, row)| {
            let _permit = control.governor().enter();
            let captured = capture_row(scene, probes[probe].capture_point_vec3(), row);
            control.advance(1);
            captured
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let tiles: Vec<Vec<u8>> = rows.par_chunks(rows_per_tile).map(encode_tile).collect();

    Ok(ReflectionProbesSection {
        tile_dimension: REFLECTION_PROBE_TILE_DIMENSION,
        tile_border: REFLECTION_PROBE_TILE_BORDER,
        atlas_format: IRRADIANCE_FORMAT_BC6H,
        probes: probes
            .iter()
            .enumerate()
            .map(|(layer, probe)| ReflectionProbeRecord {
                source_entity: probe.source_entity,
                box_min: probe.box_min,
                box_max: probe.box_max,
                capture_point: probe.capture_point,
                layer: layer as u32,
                // No light share yet: every entry unused, so the runtime
                // ratio is 1 in every direction.
                lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
            })
            .collect(),
        bindings: bindings(probes),
        tiles: tiles.concat(),
    })
}

/// `blake3` over every input the stage reads, in a fixed order, plus the
/// constants that shape its output.
fn cache_key(inputs: &ReflectionProbeInputs<'_>) -> anyhow::Result<CacheKey> {
    let mut hasher = blake3::Hasher::new();
    let mut section = |label: &str, bytes: &[u8]| {
        hasher.update(label.as_bytes());
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    // Geometry carries the positions the BVH is built over, the UVs and the
    // lightmap UVs and block ids the capture interpolates.
    section(
        "geometry",
        &postcard::to_allocvec(inputs.geometry).expect("postcard serialize geometry"),
    );
    let primitives: Vec<u8> = inputs
        .primitives
        .iter()
        .flat_map(|p| [p.index_offset, p.index_count])
        .flat_map(u32::to_le_bytes)
        .collect();
    section("bvh primitives", &primitives);
    section(
        "environment volumes",
        &inputs.environment_volumes.to_bytes(),
    );
    section(
        "sh volume",
        &inputs
            .sh_volume
            .try_to_bytes()
            .map_err(|e| anyhow::anyhow!("packed SH volume violates its wire contract: {e}"))?,
    );
    // Only the materials a face of the static world can show.
    let used: BTreeSet<u32> = inputs
        .geometry
        .geometry
        .faces
        .iter()
        .map(|face| face.texture_index)
        .collect();
    let mut materials = Vec::new();
    let mut warned = std::collections::HashSet::new();
    for index in used {
        let Some(name) = inputs.geometry.texture_names.names.get(index as usize) else {
            continue;
        };
        materials.extend_from_slice(&index.to_le_bytes());
        materials.extend_from_slice(&(name.len() as u64).to_le_bytes());
        materials.extend_from_slice(name.as_bytes());
        materials.extend_from_slice(&inputs.texture_keys.get(name).copied().unwrap_or([0; 32]));
        let strength =
            postretro_render_data::material::derive_material(name, &mut warned).emissive_strength();
        materials.extend_from_slice(&strength.to_le_bytes());
    }
    section("materials", &materials);
    let mut constants = Vec::new();
    for value in [
        REFLECTION_PROBE_TILE_DIMENSION,
        REFLECTION_PROBE_TILE_BORDER,
        box_fit::FIT_AZIMUTHS as u32,
        box_fit::FIT_RING_COUNT as u32,
        u32::from(PRODUCTION_PROBE_OCCLUSION),
    ] {
        constants.extend_from_slice(&value.to_le_bytes());
    }
    for value in [
        box_fit::FIT_ELEVATION_STEP_DEG,
        box_fit::FIT_PLANE_MERGE_M,
        placement::UP_FACING_MIN_COS,
        placement::CAPTURE_LIFT_M,
        placement::PLANE_TOLERANCE_M,
    ] {
        constants.extend_from_slice(&value.to_le_bytes());
    }
    for value in SUPERSAMPLE_OFFSETS
        .iter()
        .flatten()
        .copied()
        .chain([PRODUCTION_INDIRECT_WEIGHT])
    {
        constants.extend_from_slice(&value.to_le_bytes());
    }
    section("constants", &constants);
    drop(section);
    // Id 22 streams into the hasher: it can be the largest input.
    hasher.update(b"lightmap");
    inputs
        .lightmap
        .write_to(&mut hasher)
        .map_err(|e| anyhow::anyhow!("hashing the lightmap section: {e}"))?;
    Ok(CacheKey::new(
        STAGE_ID,
        STAGE_VERSION,
        hasher.finalize().as_bytes(),
    ))
}

fn log_probes(section: &ReflectionProbesSection) {
    for (index, probe) in section.probes.iter().enumerate() {
        let faces = section
            .bindings
            .iter()
            .filter(|b| b.probe as usize == index)
            .count();
        log::info!(
            "[ReflectionProbes] probe {index}: entity {}, box {:?}..{:?}, capture point {:?}, \
             {faces} face(s)",
            probe.source_entity,
            probe.box_min,
            probe.box_max,
            probe.capture_point,
        );
    }
    log::info!(
        "[ReflectionProbes] {} probe(s), {} binding(s), {} tile byte(s)",
        section.probes.len(),
        section.bindings.len(),
        section.tiles.len()
    );
}
