// Launch-time start pose override: `--start-pose x,y,z,yaw_deg,pitch_deg`.
// See: context/lib/boot_sequence.md · context/lib/rendering_pipeline.md §12

use glam::Vec3;
use postretro_entities::{EntityRegistry, Transform};

/// A reproducible place to begin a windowed run, for measurement probes that
/// are not the map's `player_spawn`. `position` uses the spawn convention (the
/// pawn's origin, as `player_spawn` places it); yaw and pitch are engine
/// radians, so `yaw = 0` faces -Z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StartPose {
    pub(crate) position: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}

/// Reads `--start-pose x,y,z,yaw_deg,pitch_deg` (or `--start-pose=...`). A
/// malformed value warns and is ignored, so the run starts at the map spawn.
pub(crate) fn start_pose_arg(args: &[String]) -> Option<StartPose> {
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        let value = if arg == START_POSE_FLAG {
            iter.next_if(|value| !value.starts_with("--"))
                .map(String::as_str)
        } else if let Some(value) = arg.strip_prefix("--start-pose=") {
            Some(value)
        } else {
            continue;
        };
        let parsed = value.and_then(parse_start_pose);
        if parsed.is_none() {
            log::warn!(
                "[Startup] invalid --start-pose \"{}\"; expected x,y,z,yaw_deg,pitch_deg — starting at the map spawn",
                value.unwrap_or_default()
            );
        }
        return parsed;
    }
    None
}

pub(crate) const START_POSE_FLAG: &str = "--start-pose";

fn parse_start_pose(value: &str) -> Option<StartPose> {
    let parts: Vec<f32> = value
        .split(',')
        .map(|part| part.trim().parse::<f32>().ok().filter(|v| v.is_finite()))
        .collect::<Option<_>>()?;
    let [x, y, z, yaw_deg, pitch_deg] = parts[..] else {
        return None;
    };
    Some(StartPose {
        position: Vec3::new(x, y, z),
        yaw: yaw_deg.to_radians(),
        pitch: pitch_deg.to_radians(),
    })
}

/// Moves the local player pawn to the pose. Runs at install, before any fixed
/// tick, so the first tick's transform snapshot starts from it and nothing
/// interpolates across the move. Returns whether a pawn was moved.
pub(crate) fn place_local_pawn(registry: &mut EntityRegistry, pose: StartPose) -> bool {
    let Some(pawn) = registry.local_player_movement_pawn() else {
        return false;
    };
    let Ok(transform) = registry.get_component::<Transform>(pawn).copied() else {
        return false;
    };
    registry
        .set_component(
            pawn,
            Transform {
                position: pose.position,
                ..transform
            },
        )
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("postretro")
            .chain(list.iter().copied())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn start_pose_accepts_split_and_equals_forms_in_degrees() {
        let expected = Some(StartPose {
            position: Vec3::new(-3840.0, -3200.0, 96.0),
            yaw: 90.0_f32.to_radians(),
            pitch: (-10.0_f32).to_radians(),
        });
        assert_eq!(
            start_pose_arg(&args(&["map.prl", "--start-pose", "-3840,-3200,96,90,-10"])),
            expected
        );
        assert_eq!(
            start_pose_arg(&args(&[
                "--start-pose=-3840, -3200, 96, 90, -10",
                "map.prl"
            ])),
            expected
        );
    }

    #[test]
    fn malformed_start_pose_is_ignored() {
        for bad in ["1,2,3", "1,2,3,4,5,6", "a,b,c,d,e", "1,2,inf,0,0"] {
            assert_eq!(start_pose_arg(&args(&["--start-pose", bad])), None, "{bad}");
        }
        assert_eq!(start_pose_arg(&args(&["--start-pose"])), None);
        assert_eq!(start_pose_arg(&args(&["map.prl"])), None);
    }

    #[test]
    fn place_local_pawn_without_a_local_player_moves_nothing() {
        let mut registry = EntityRegistry::new();
        let bystander = registry.spawn(Transform::default());
        assert!(!place_local_pawn(
            &mut registry,
            StartPose {
                position: Vec3::ONE,
                yaw: 0.0,
                pitch: 0.0,
            }
        ));
        assert_eq!(
            registry
                .get_component::<Transform>(bystander)
                .unwrap()
                .position,
            Vec3::ZERO
        );
    }
}
