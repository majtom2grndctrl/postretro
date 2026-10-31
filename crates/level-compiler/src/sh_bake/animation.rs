// Animated-light validation and runtime animation descriptors for the SH volume.
// See: context/lib/build_pipeline.md §Compiler pipeline

use postretro_level_format::sh_volume::AnimationDescriptor;

use crate::map_data::{LightAnimation, MapLight};

/// Color animation is only valid on `bake_only` or `is_animated` lights. Static
/// non-animated lights keep baked indirect lighting at a fixed authored color.
pub fn validate_light_animations(lights: &[MapLight]) -> Result<(), String> {
    for light in lights {
        let Some(anim) = light.animation.as_ref() else {
            continue;
        };
        if anim.color.is_some() && !light.bake_only && !light.is_animated {
            return Err(format!(
                "light at origin [{:.3}, {:.3}, {:.3}] has `color` animation \
                 but is neither `bake_only` nor `is_animated`; animated color on \
                 a baked light would mismatch the SH indirect bake. Either mark \
                 the light `_animated 1`, set `_bake_only 1`, or remove the \
                 color curve.",
                light.origin.x, light.origin.y, light.origin.z,
            ));
        }
    }
    Ok(())
}

/// Build the runtime animation descriptor for one animated light. `pub(crate)`
/// so the warm per-group SH path (`sh_group.rs`) recovers the volume's
/// animation-descriptor table without re-running the probe bake.
pub(crate) fn animation_descriptor_for_light(light: &MapLight) -> AnimationDescriptor {
    animation_descriptor_for(light)
}

pub(super) fn animation_descriptor_for(light: &MapLight) -> AnimationDescriptor {
    let anim: &LightAnimation = light
        .animation
        .as_ref()
        .expect("animation_descriptor_for called on a non-animated light");
    let base_color = if anim
        .color
        .as_ref()
        .is_some_and(|samples| !samples.is_empty())
    {
        // RGB curve samples replace authored hue. Retain intensity as a
        // channel-neutral multiplier; unit-radiance bake data supplies the
        // transport term.
        [light.intensity; 3]
    } else {
        [
            light.color[0] * light.intensity,
            light.color[1] * light.intensity,
            light.color[2] * light.intensity,
        ]
    };
    AnimationDescriptor {
        period: anim.period.max(1.0e-6),
        phase: anim.phase,
        // Weight maps and indirect delta tiles are unit-radiance transport.
        // This descriptor is the only authored-radiance application.
        base_color,
        brightness: anim.brightness.clone().unwrap_or_default(),
        color: anim.color.clone().unwrap_or_default(),
        direction: anim.direction.clone().unwrap_or_default(),
        start_active: if anim.start_active { 1 } else { 0 },
    }
}
