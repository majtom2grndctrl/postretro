// Reflection-probe stage cache identity: the key over every input the capture reads, and the entry's framing.
// See: context/lib/build_pipeline.md §Build Cache

use std::collections::BTreeSet;

use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_LIGHTS_PER_PROBE, REFLECTION_PROBE_TILE_BORDER,
    REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbesSection,
};

use super::capture::SUPERSAMPLE_OFFSETS;
use super::capture_visibility::SUBDIVISIONS;
use super::placement::Placement;
use super::reference::REFERENCE_MEAN_SAMPLES;
use super::sdf_lights::SdfLights;
use super::sh_sample::PRODUCTION_PROBE_OCCLUSION;
use super::shading::PRODUCTION_INDIRECT_WEIGHT;
use super::share::ShareReport;
use super::split::ADJACENT_FACE_M;
use super::split_triggers::split_key_bytes;
use super::{ReflectionProbeInputs, STAGE_ID, STAGE_VERSION, StageLimits};
use crate::cache::CacheKey;

/// The cache entry: the section, then how many probes the cap dropped, then
/// each probe's share report, so a warm build logs the covered share and
/// warns of the cap as a cold one does.
pub(super) fn cache_payload(
    section: &ReflectionProbesSection,
    capped: usize,
    reports: &[ShareReport],
) -> Vec<u8> {
    let bytes = section.to_bytes();
    let mut out = Vec::with_capacity(12 + bytes.len() + reports.len() * ShareReport::BYTES);
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(&bytes);
    out.extend_from_slice(&(capped as u32).to_le_bytes());
    for report in reports {
        report.to_bytes(&mut out);
    }
    out
}

pub(super) fn parse_cache_payload(
    bytes: &[u8],
) -> anyhow::Result<(ReflectionProbesSection, usize, Vec<ShareReport>)> {
    let len = bytes
        .get(..8)
        .map(|b| u64::from_le_bytes(b.try_into().expect("eight bytes")) as usize)
        .ok_or_else(|| anyhow::anyhow!("truncated entry"))?;
    let body = bytes
        .get(8..8 + len)
        .ok_or_else(|| anyhow::anyhow!("truncated section"))?;
    let section = ReflectionProbesSection::from_bytes(body)?;
    let capped = bytes
        .get(8 + len..12 + len)
        .map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")) as usize)
        .ok_or_else(|| anyhow::anyhow!("truncated cap count"))?;
    let rest = &bytes[12 + len..];
    anyhow::ensure!(
        rest.len() == section.probes.len() * ShareReport::BYTES,
        "share reports do not match the probe count"
    );
    let reports = rest
        .chunks(ShareReport::BYTES)
        .map(|chunk| ShareReport::from_bytes(chunk).expect("chunk has the report size"))
        .collect();
    Ok((section, capped, reports))
}

/// `blake3` over every input the capture reads, in a fixed order, plus the
/// constants that shape its output, splitting's and the cap's included.
/// Placement enters as its output: every probe's box, capture point and
/// bound faces, and how many lead as authored.
pub(super) fn cache_key(
    inputs: &ReflectionProbeInputs<'_>,
    placement: &Placement,
    limits: StageLimits,
) -> anyhow::Result<CacheKey> {
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
    let mut placed = Vec::new();
    placed.extend_from_slice(&(placement.authored as u64).to_le_bytes());
    for probe in &placement.probes {
        placed.extend_from_slice(&probe.source_entity.to_le_bytes());
        for value in probe
            .box_min
            .iter()
            .chain(&probe.box_max)
            .chain(&probe.capture_point)
        {
            placed.extend_from_slice(&value.to_le_bytes());
        }
        placed.extend_from_slice(&(probe.faces.len() as u64).to_le_bytes());
        for &(volume, face) in &probe.faces {
            placed.extend_from_slice(&volume.to_le_bytes());
            placed.extend_from_slice(&face.to_le_bytes());
        }
    }
    section("placement", &placed);
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
    // The animated lights the capture reads: their baked direct and bounce as
    // the level ships them, and their runtime descriptors (whose clamped cycle
    // mean is each light's reference).
    section(
        "animated weight maps",
        &inputs
            .animated_weight_maps
            .map(AnimatedLightWeightMapsSection::to_bytes)
            .unwrap_or_default(),
    );
    section(
        "delta sh",
        &inputs
            .delta_sh
            .map(DeltaShVolumesSection::to_bytes)
            .unwrap_or_default(),
    );
    let mut descriptors = Vec::new();
    for d in &inputs.sh_volume.animation_descriptors {
        for v in [d.period, d.phase]
            .into_iter()
            .chain(d.base_color)
            .chain(d.brightness.iter().copied())
            .chain(d.color.iter().flatten().copied())
        {
            descriptors.extend_from_slice(&v.to_le_bytes());
        }
        descriptors.extend_from_slice(&(d.brightness.len() as u32).to_le_bytes());
        descriptors.extend_from_slice(&(d.color.len() as u32).to_le_bytes());
    }
    section("animation descriptors", &descriptors);
    // The sdf lights the runtime selects and their chunk windows.
    section(
        "sdf lights",
        &SdfLights::new(inputs.alpha_lights, inputs.chunk_light_list)
            .key_bytes(inputs.chunk_light_list),
    );
    let mut constants = Vec::new();
    for value in [
        REFLECTION_PROBE_TILE_DIMENSION,
        REFLECTION_PROBE_TILE_BORDER,
        u32::from(PRODUCTION_PROBE_OCCLUSION),
        REFERENCE_MEAN_SAMPLES,
        REFLECTION_PROBE_LIGHTS_PER_PROBE as u32,
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
    let mut split = split_key_bytes();
    split.extend_from_slice(&limits.rounds.to_le_bytes());
    split.extend_from_slice(&(limits.cap as u64).to_le_bytes());
    split.extend_from_slice(&SUBDIVISIONS.to_le_bytes());
    split.extend_from_slice(&ADJACENT_FACE_M.to_le_bytes());
    section("split", &split);
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
