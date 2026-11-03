// Side count: the dynamic-tier lights whose influence reaches each mover's
// swept bounds, split by whether the mover carries them.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use super::pose::MoverPoses;
use crate::map_data::{CarriedLightLink, LightType, MapLight};

/// One analysed mover's side count. Lights are by source index, ascending,
/// which is authored entity order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SideCount {
    pub(crate) mover_id: u32,
    /// Dynamic lights this mover carries.
    pub(crate) carried: Vec<usize>,
    /// Every other dynamic light reaching the mover's swept bounds.
    pub(crate) others: Vec<usize>,
}

/// Side counts of every analysed mover, ascending by `mover_id`.
///
/// A light's influence is its falloff sphere at its authored origin, every
/// direction for a sun. A dynamic bake-only light is skipped: it has no
/// runtime presence, so no shadow to price.
pub(crate) fn side_counts(
    poses: &MoverPoses,
    lights: &[MapLight],
    carried_links: &[CarriedLightLink],
) -> Vec<SideCount> {
    let dynamic: Vec<usize> = lights
        .iter()
        .enumerate()
        .filter(|(_, light)| light.is_dynamic && !light.bake_only)
        .map(|(source_index, _)| source_index)
        .collect();
    poses
        .analysed
        .iter()
        .map(|mover| {
            let mut count = SideCount {
                mover_id: mover.mover_id,
                carried: Vec::new(),
                others: Vec::new(),
            };
            if !mover.swept.is_valid() {
                return count;
            }
            for &source_index in &dynamic {
                let light = &lights[source_index];
                let reaches = match light.light_type {
                    LightType::Directional => true,
                    LightType::Point | LightType::Spot => {
                        let nearest = light.origin.clamp(mover.swept.min, mover.swept.max);
                        light.origin.distance(nearest) <= f64::from(light.falloff_range)
                    }
                };
                if !reaches {
                    continue;
                }
                let carried = carried_links.iter().any(|link| {
                    link.source_light_index == source_index && link.mover_id == mover.mover_id
                });
                if carried {
                    count.carried.push(source_index);
                } else {
                    count.others.push(source_index);
                }
            }
            count
        })
        .collect()
}
