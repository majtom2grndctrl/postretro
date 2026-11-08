// TEMPORARY dev-only switch for the mover probe channel's slices: poses one
// named mover at one terminus as an occluder in the id-35, id-41 and id-45
// direct-SH bakes (task 1) and in the SH base's bounce and depth (task 2,
// `dev_bounce`), so the PRL holds that terminus's composed state with no
// format or compose code. Plan task 12 deletes this file and its call sites.
// See: context/plans/in-progress/mover-light-flow--probe-channel/plan.md (tasks 1, 2)

use std::sync::OnceLock;

use anyhow::{Context, bail};

use super::{INSIDE_BRUSH_RULE, InsideBrushRule, PosedOccluder, PosedSurfaces};
use crate::map_data::{MapKinematicMover, MapKinematicWaypoint};
use crate::mover_light_gate::{MoverPoses, Terminus};

/// `<mover name>:<first|last>[:dark|:skip][:black]`. The rule field
/// overrides [`INSIDE_BRUSH_RULE`] for the inside-brush A/B; `black` keeps
/// the posed sides out of the bounce bake's patches, for the patch A/B.
/// Unset, nothing changes.
pub(crate) const ENV_VAR: &str = "POSTRETRO_DEV_POSE_MOVER";

static POSED: OnceLock<PosedOccluder> = OnceLock::new();

/// The parsed switch.
pub(crate) struct DevPose {
    pub(crate) name: String,
    pub(crate) terminus: Terminus,
    pub(crate) rule: InsideBrushRule,
    pub(crate) surfaces: PosedSurfaces,
}

/// [`ENV_VAR`] parsed, or `None` when unset.
pub(crate) fn parse() -> anyhow::Result<Option<DevPose>> {
    let Ok(value) = std::env::var(ENV_VAR) else {
        return Ok(None);
    };
    let mut fields = value.split(':');
    let (Some(name), Some(terminus)) = (fields.next(), fields.next()) else {
        bail!("{ENV_VAR}={value}: expected `<mover name>:<first|last>[:dark|:skip][:black]`");
    };
    let terminus = match terminus {
        "first" => Terminus::First,
        "last" => Terminus::Last,
        other => bail!("{ENV_VAR}: terminus `{other}` is not `first` or `last`"),
    };
    let (mut rule, mut surfaces) = (INSIDE_BRUSH_RULE, PosedSurfaces::Patches);
    for field in fields {
        match field {
            "dark" => rule = InsideBrushRule::Dark,
            "skip" => rule = InsideBrushRule::SkipContainingBrush,
            "black" => surfaces = PosedSurfaces::Black,
            other => bail!("{ENV_VAR}: field `{other}` is not `dark`, `skip` or `black`"),
        }
    }
    Ok(Some(DevPose {
        name: name.to_string(),
        terminus,
        rule,
        surfaces,
    }))
}

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
    let Some(DevPose {
        name,
        terminus,
        rule,
        ..
    }) = parse()?
    else {
        return Ok(());
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
        "[MoverLight] DEV {ENV_VAR}: SH base (id 34, id 59) and direct SH (ids 35, 41, 45) \
         bake `{name}` posed at its {} waypoint `{}` ({rule:?}); direct-SH cache bypassed. \
         Not a shippable PRL.",
        terminus.label(),
        pose.waypoint,
    );
    // One pipeline per prl-build process; a second install keeps the first.
    let _ = POSED.set(PosedOccluder::new(pose, rule));
    Ok(())
}
