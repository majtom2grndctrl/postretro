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
use super::capture_exclusion::ProbeExclusion;
use super::exclusion::ExclusionConstants;
use super::placement::PlacedProbe;
use super::reference::REFERENCE_MEAN_SAMPLES;
use super::sdf_lights::SdfLights;
use super::sh_sample::PRODUCTION_PROBE_OCCLUSION;
use super::shading::PRODUCTION_INDIRECT_WEIGHT;
use super::share::ShareReport;
use super::{ReflectionProbeInputs, STAGE_ID, STAGE_VERSION};
use crate::cache::CacheKey;
use crate::map_data::{BrushVolume, ReflectionCapture, TextureProjection};

/// The cache entry: the section, then each probe's share report, so a warm
/// build logs the covered share a cold one does.
pub(super) fn cache_payload(section: &ReflectionProbesSection, reports: &[ShareReport]) -> Vec<u8> {
    let bytes = section.to_bytes();
    let mut out = Vec::with_capacity(8 + bytes.len() + reports.len() * ShareReport::BYTES);
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(&bytes);
    for report in reports {
        report.to_bytes(&mut out);
    }
    out
}

pub(super) fn parse_cache_payload(
    bytes: &[u8],
) -> anyhow::Result<(ReflectionProbesSection, Vec<ShareReport>)> {
    let len = bytes
        .get(..8)
        .map(|b| u64::from_le_bytes(b.try_into().expect("eight bytes")) as usize)
        .ok_or_else(|| anyhow::anyhow!("truncated entry"))?;
    let body = bytes
        .get(8..8 + len)
        .ok_or_else(|| anyhow::anyhow!("truncated section"))?;
    let section = ReflectionProbesSection::from_bytes(body)?;
    let rest = &bytes[8 + len..];
    anyhow::ensure!(
        rest.len() == section.probes.len() * ShareReport::BYTES,
        "share reports do not match the probe count"
    );
    let reports = rest
        .chunks(ShareReport::BYTES)
        .map(|chunk| ShareReport::from_bytes(chunk).expect("chunk has the report size"))
        .collect();
    Ok((section, reports))
}

/// `blake3` over every input the capture reads, in a fixed order, plus the
/// constants that shape its output. Placement enters as its output: every
/// probe's box, capture point and bound faces. Exclusion enters as its
/// inputs and output: the override table and the rule's `constants`, then
/// each probe's `exclusions` (parallel to `probes`) with what the capture
/// reads of the brushes CSG may hide, since geometry shows only what
/// survived it.
pub(super) fn cache_key(
    inputs: &ReflectionProbeInputs<'_>,
    probes: &[PlacedProbe],
    exclusions: &[ProbeExclusion<'_>],
    constants: &ExclusionConstants,
) -> anyhow::Result<CacheKey> {
    anyhow::ensure!(
        exclusions.len() == probes.len(),
        "{} exclusion(s) for {} probe(s)",
        exclusions.len(),
        probes.len()
    );
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
    let mut placement = Vec::new();
    for probe in probes {
        placement.extend_from_slice(&probe.source_entity.to_le_bytes());
        for value in probe
            .box_min
            .iter()
            .chain(&probe.box_max)
            .chain(&probe.capture_point)
        {
            placement.extend_from_slice(&value.to_le_bytes());
        }
        placement.extend_from_slice(&(probe.faces.len() as u64).to_le_bytes());
        for &(volume, face) in &probe.faces {
            placement.extend_from_slice(&volume.to_le_bytes());
            placement.extend_from_slice(&face.to_le_bytes());
        }
    }
    section("placement", &placement);
    // Resolved values only: an edit that resolves the same keys the same.
    let overrides: Vec<u8> = inputs
        .capture_override
        .iter()
        .map(|&value| match value {
            ReflectionCapture::Auto => 0,
            ReflectionCapture::Always => 1,
            ReflectionCapture::Never => 2,
        })
        .collect();
    section("capture overrides", &overrides);
    section("exclusion constants", &constants.key_bytes());
    section("exclusions", &exclusion_bytes(inputs, exclusions));
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
    // Id 45's descriptor table, which only the roster check reads: it runs
    // on a miss, so a table that stops being the identity must miss. The
    // rest of id 45 never reaches the capture and stays out of the key.
    // Absent and empty differ, as the check skips an absent table.
    let animated_direct_table: Vec<u8> = inputs
        .animated_direct_sh
        .map(|section| {
            std::iter::once(section.animation_descriptor_indices.len() as u32)
                .chain(section.animation_descriptor_indices.iter().copied())
                .flat_map(u32::to_le_bytes)
                .collect()
        })
        .unwrap_or_default();
    section("animated direct descriptor table", &animated_direct_table);
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

/// Per probe: its excluded brushes and their post-cut faces (the mask), each
/// excluded hull's planes (its exit test) and contacts, and every kept
/// contact candidate's post-cut faces (the fill's pool), planes (kept solid)
/// and sides (the fill's covering test, material and projection). CSG hides
/// what an excluded hull buries, so the geometry alone does not necessarily
/// tell a flush pillar from a sunk one.
fn exclusion_bytes(
    inputs: &ReflectionProbeInputs<'_>,
    exclusions: &[ProbeExclusion<'_>],
) -> Vec<u8> {
    let mut out = Vec::new();
    let u32s = |out: &mut Vec<u8>, values: &[u32]| {
        out.extend_from_slice(&(values.len() as u64).to_le_bytes());
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
    };
    for exclusion in exclusions {
        let excluded = exclusion.excluded();
        u32s(&mut out, excluded);
        for (slot, &brush) in excluded.iter().enumerate() {
            u32s(&mut out, exclusion.faces_of(slot));
            hull_planes(&mut out, &inputs.brush_volumes[brush as usize]);
            let (kept, others) = exclusion.contacts_of(slot);
            u32s(&mut out, &kept);
            u32s(&mut out, &others);
        }
        let kept: Vec<u32> = exclusion.kept_contacts().collect();
        u32s(&mut out, &kept);
        for &brush in &kept {
            u32s(&mut out, exclusion.brush_faces(brush));
            let volume = &inputs.brush_volumes[brush as usize];
            hull_planes(&mut out, volume);
            hull_sides(&mut out, volume);
        }
    }
    out
}

fn f64s(out: &mut Vec<u8>, values: impl IntoIterator<Item = f64>) {
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn hull_planes(out: &mut Vec<u8>, volume: &BrushVolume) {
    out.extend_from_slice(&(volume.planes.len() as u64).to_le_bytes());
    for plane in &volume.planes {
        f64s(
            out,
            plane.normal.to_array().into_iter().chain([plane.distance]),
        );
    }
}

fn hull_sides(out: &mut Vec<u8>, volume: &BrushVolume) {
    out.extend_from_slice(&(volume.sides.len() as u64).to_le_bytes());
    for side in &volume.sides {
        f64s(
            out,
            side.normal.to_array().into_iter().chain([side.distance]),
        );
        out.extend_from_slice(&(side.vertices.len() as u64).to_le_bytes());
        f64s(out, side.vertices.iter().flat_map(|v| v.to_array()));
        out.extend_from_slice(&(side.texture.len() as u64).to_le_bytes());
        out.extend_from_slice(side.texture.as_bytes());
        match &side.tex_projection {
            TextureProjection::Standard {
                u_offset,
                v_offset,
                angle,
                scale_u,
                scale_v,
            } => {
                out.push(0);
                f64s(out, [*u_offset, *v_offset, *angle, *scale_u, *scale_v]);
            }
            TextureProjection::Valve {
                u_axis,
                u_offset,
                v_axis,
                v_offset,
                scale_u,
                scale_v,
            } => {
                out.push(1);
                f64s(
                    out,
                    u_axis
                        .to_array()
                        .into_iter()
                        .chain([*u_offset])
                        .chain(v_axis.to_array())
                        .chain([*v_offset, *scale_u, *scale_v]),
                );
            }
        }
    }
}
