// Reflection-probe animated-light roster: each light's reference by roster index, and the identity check a capture runs on the baked tables.
// See: context/lib/resource_management.md §5.2 (Light share)

use postretro_level_format::sh_volume::AnimationDescriptor;

use super::ReflectionProbeInputs;
use super::reference::reference_value;

/// Each animated light's reference value, by roster index: the runtime's
/// descriptors are the SH section's, one per AnimatedBakedLights row.
pub(super) fn references(descriptors: &[AnimationDescriptor]) -> Vec<[f32; 3]> {
    descriptors.iter().map(reference_value).collect()
}

/// Every light the baked transport names must have a descriptor, and each
/// shipped descriptor table (ids 27 and 45) must be the identity over the
/// whole roster: a share's roster index indexes the runtime descriptor
/// buffer directly, so roster slot `r` must name descriptor `r`. This
/// refuses a table of another length as well as a non-identity entry. The
/// loader's check of section 54 (`validate_reflection_probe_lights` in
/// `postretro-level-loader`) is narrower: identity over each table's own
/// length, and every stored share's light inside the roster.
pub(super) fn check_roster(
    inputs: &ReflectionProbeInputs<'_>,
    roster: usize,
) -> anyhow::Result<()> {
    let tables = [
        (
            "DeltaShVolumes (id 27)",
            inputs
                .delta_sh
                .map(|delta| delta.animation_descriptor_indices.as_slice()),
        ),
        (
            "AnimatedDirectShDeltaVolumes (id 45)",
            inputs
                .animated_direct_sh
                .map(|section| section.animation_descriptor_indices.as_slice()),
        ),
    ];
    for (table, indices) in tables {
        let Some(indices) = indices else {
            continue;
        };
        anyhow::ensure!(
            indices.len() == roster,
            "{table} has {} animated light(s) over the {roster}-light roster; reflection \
             probes need the identity roster",
            indices.len(),
        );
        if let Some((slot, descriptor)) = indices
            .iter()
            .enumerate()
            .find(|&(slot, &descriptor)| descriptor as usize != slot)
        {
            anyhow::bail!(
                "{table} maps animated light {slot} to descriptor {descriptor}; reflection \
                 probes need the identity roster"
            );
        }
    }
    if let Some(delta) = inputs.delta_sh {
        anyhow::ensure!(
            delta.affinity_lights.iter().all(|&l| (l as usize) < roster),
            "DeltaShVolumes names an animated light past the {roster}-light roster"
        );
    }
    if let Some(maps) = inputs.animated_weight_maps {
        anyhow::ensure!(
            maps.texel_lights
                .iter()
                .all(|t| (t.light_index as usize) < roster),
            "AnimatedLightWeightMaps names an animated light past the {roster}-light roster"
        );
    }
    Ok(())
}
