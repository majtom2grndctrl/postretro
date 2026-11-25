// Reflection-probe stage log: each probe's placement, face count and light share, and the section's totals.
// See: context/lib/resource_management.md §5.2 (Light share)

use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_LIGHTS_PER_PROBE, ReflectionProbesSection,
};

use super::share::ShareReport;

pub(super) fn log_probes(section: &ReflectionProbesSection, reports: &[ShareReport]) {
    for (index, (probe, report)) in section.probes.iter().zip(reports).enumerate() {
        let faces = section
            .fluid_bindings
            .iter()
            .map(|b| b.probe)
            .chain(section.world_bindings.iter().map(|b| b.probe))
            .filter(|probe| *probe as usize == index)
            .count();
        log::info!(
            "[ReflectionProbes] probe {index}: entity {}, box {:?}..{:?}, capture point {:?}, \
             {faces} face(s)",
            probe.source_entity,
            probe.box_min,
            probe.box_max,
            probe.capture_point,
        );
        if report.animated_lights == 0 {
            log::info!("[ReflectionProbes] probe {index}: no animated light reaches its capture");
            continue;
        }
        let stored: Vec<String> = probe
            .lights
            .iter()
            .zip(report.totals)
            .filter(|(light, _)| light.is_used())
            .map(|(light, total)| {
                format!(
                    "light {} (reference [{:.3}, {:.3}, {:.3}], {:.3} of the capture)",
                    light.roster_index,
                    light.reference[0],
                    light.reference[1],
                    light.reference[2],
                    total
                )
            })
            .collect();
        log::info!(
            "[ReflectionProbes] probe {index}: covered share {:.3} of {} animated light(s); \
             stored {}",
            report.covered,
            report.animated_lights,
            stored.join(", "),
        );
        let held = report.animated_lights as usize - stored.len();
        if held > 0 {
            log::info!(
                "[ReflectionProbes] probe {index}: {held} animated light(s) past the \
                 {REFLECTION_PROBE_LIGHTS_PER_PROBE} stored stay at reference; covered share {:.3}",
                report.covered,
            );
        }
    }
    log::info!(
        "[ReflectionProbes] {} probe(s), {} binding(s), {} tile byte(s)",
        section.probes.len(),
        section.fluid_bindings.len() + section.world_bindings.len(),
        section.tiles.len()
    );
}
