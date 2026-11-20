// `setEmissiveColor` reaction primitive: write the scripted linear tint on
// every target emissive volume.
// See: context/lib/scripting.md §10.2a

use serde::{Deserialize, Serialize};

use postretro_entities::{EntityId, EntityRegistry};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetEmissiveColorArgs {
    pub(crate) color: [f32; 3],
}

/// Write the fixed colour on every target. A negative or non-finite channel
/// reads 0 and a channel above 1 reads 1, with one warning per write
/// (`research.md` P49), so no script pushes a face past the mapper's
/// multiplier. Brightness and any running animation are left alone, start
/// time included (P44).
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetEmissiveColorArgs,
) {
    if targets.is_empty() {
        log::debug!("[Scripting] setEmissiveColor: empty target set, no-op");
        return;
    }
    let color = fix_color(args.color);
    super::write_targets(registry, targets, "setEmissiveColor", |volume| {
        volume.color = color;
    });
}

fn fix_color(color: [f32; 3]) -> [f32; 3] {
    let fixed = color.map(|channel| {
        if channel.is_finite() {
            channel.clamp(0.0, 1.0)
        } else {
            0.0
        }
    });
    if fixed != color {
        log::warn!(
            "[Scripting] setEmissiveColor: color {color:?} has a channel outside [0, 1] or \
             non-finite; using {fixed:?} (negative and non-finite read 0, above 1 reads 1)"
        );
    }
    fixed
}
