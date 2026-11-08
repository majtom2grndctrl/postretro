// TEMPORARY dev-only switch for the mover probe channel's direct slice: poses
// one named mover at one terminus as an occluder in the id-35, id-41 and id-45
// direct-SH bakes, so the PRL holds that terminus's composed atlas with no
// format or compose code. Plan task 12 deletes this file and its call sites.
// See: context/plans/in-progress/mover-light-flow--probe-channel/plan.md (task 1)

use std::sync::OnceLock;

use anyhow::{Context, bail};

use super::{INSIDE_BRUSH_RULE, InsideBrushRule, PosedOccluder};
use crate::map_data::{MapKinematicMover, MapKinematicWaypoint};
use crate::mover_light_gate::{MoverPoses, Terminus};

/// `<mover name>:<first|last>[:dark|:skip]`. The third field overrides
/// [`INSIDE_BRUSH_RULE`] for the inside-brush A/B. Unset, nothing changes.
pub(crate) const ENV_VAR: &str = "POSTRETRO_DEV_POSE_MOVER";

static POSED: OnceLock<PosedOccluder> = OnceLock::new();

/// The posed mover the direct bakes trace against, when the switch is set.
pub(crate) fn posed_occluder() -> Option<&'static PosedOccluder> {
    POSED.get()
}

/// Whether the switch is on: the direct-SH bakes it touches must not read
/// or write their caches, which key nothing of the pose.
pub(crate) fn active() -> bool {
    POSED.get().is_some()
}

/// Read [`ENV_VAR`] and resolve it against the map's movers. A no-op when
/// unset; an unknown mover, unposable mover or malformed value fails the
/// build by name.
pub(crate) fn install(
    movers: &[MapKinematicMover],
    waypoints: &[MapKinematicWaypoint],
) -> anyhow::Result<()> {
    let Ok(value) = std::env::var(ENV_VAR) else {
        return Ok(());
    };
    let mut fields = value.split(':');
    let (Some(name), Some(terminus)) = (fields.next(), fields.next()) else {
        bail!("{ENV_VAR}={value}: expected `<mover name>:<first|last>[:dark|:skip]`");
    };
    let terminus = match terminus {
        "first" => Terminus::First,
        "last" => Terminus::Last,
        other => bail!("{ENV_VAR}: terminus `{other}` is not `first` or `last`"),
    };
    let rule = match fields.next() {
        None => INSIDE_BRUSH_RULE,
        Some("dark") => InsideBrushRule::Dark,
        Some("skip") => InsideBrushRule::SkipContainingBrush,
        Some(other) => bail!("{ENV_VAR}: inside-brush rule `{other}` is not `dark` or `skip`"),
    };
    let mover_id = movers
        .iter()
        .find(|mover| mover.name == name)
        .map(|mover| mover.mover_id)
        .with_context(|| format!("{ENV_VAR}: no kinematic_mover named `{name}`"))?;
    let poses = MoverPoses::resolve(movers, waypoints);
    let mover = poses
        .mover(mover_id)
        .with_context(|| format!("{ENV_VAR}: mover `{name}` has no analysable termini"))?;
    let pose = mover.pose(terminus).clone();
    log::warn!(
        "[MoverLight] DEV {ENV_VAR}: direct SH (ids 35, 41, 45) bakes `{name}` posed at its {} \
         waypoint `{}` ({rule:?}); direct-SH cache bypassed. Not a shippable PRL.",
        terminus.label(),
        pose.waypoint,
    );
    // One pipeline per prl-build process; a second install keeps the first.
    let _ = POSED.set(PosedOccluder::new(pose, rule));
    Ok(())
}
