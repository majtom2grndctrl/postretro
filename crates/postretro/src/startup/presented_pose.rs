//! The presented pose: the one position and orientation a level entry's
//! install places, its Settling frames stream toward, and its first Running
//! frame presents.
//! See: context/lib/boot_sequence.md §3

use glam::Vec3;
use postretro_scripting_core::runtime::MenuCamera;

use crate::App;
use crate::app::world_less_frame::HeldLevelView;
use crate::camera::RenderCamera;
use crate::frame_timing::InterpolableState;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PresentedPose {
    pub(crate) position: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}

impl PresentedPose {
    fn from_menu(menu: &MenuCamera) -> Self {
        Self {
            position: Vec3::from_array(menu.position),
            yaw: menu.yaw,
            pitch: menu.pitch,
        }
    }

    /// The view a held level streams toward from this pose: the first
    /// Running frame's eye and matrix with no view-feel offset, which a
    /// player at rest on the reveal frame does not have.
    pub(crate) fn held_view(self, aspect: f32) -> HeldLevelView {
        let camera = RenderCamera::new(
            self.position,
            aspect,
            self.yaw,
            self.pitch,
            0.0,
            Vec3::ZERO,
            0.0,
        );
        HeldLevelView {
            eye: camera.eye_position,
            view_proj: camera.view_projection,
        }
    }
}

/// The menu pose whenever the frontend menu is present, otherwise the spawn
/// pose install placed. A backdrop install and a co-op client following a
/// relevel with the menu pushed both take the menu pose; install cannot tell
/// them apart, and the menu is what the first frame shows in either case.
pub(crate) fn resolve_presented_pose(
    menu: Option<&MenuCamera>,
    spawn: PresentedPose,
) -> PresentedPose {
    menu.map_or(spawn, PresentedPose::from_menu)
}

impl App {
    /// The chokepoint every reader of the presented pose goes through.
    /// Before install's camera step the spawn half is the previous camera.
    pub(crate) fn presented_pose(&self) -> PresentedPose {
        let menu = self
            .frontend_menu_is_present()
            .then(|| {
                self.session
                    .as_ref()
                    .and_then(|session| session.frontend.as_ref())
                    .map(|frontend| &frontend.camera)
            })
            .flatten();
        resolve_presented_pose(
            menu,
            PresentedPose {
                position: self.camera.position,
                yaw: self.camera.yaw,
                pitch: self.camera.pitch,
            },
        )
    }

    /// Install's last camera step: moves the camera to the presented pose and
    /// holds both interpolation endpoints there, so a frame before the first
    /// tick renders from it.
    pub(crate) fn place_camera_at_presented_pose(&mut self) {
        let pose = self.presented_pose();
        self.camera.position = pose.position;
        self.camera.yaw = pose.yaw;
        self.camera.pitch = pose.pitch;
        self.frame_timing
            .hold_state(InterpolableState::new(pose.position));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn() -> PresentedPose {
        PresentedPose {
            position: Vec3::new(1.0, 2.0, 3.0),
            yaw: 0.25,
            pitch: -0.1,
        }
    }

    #[test]
    fn presented_pose_resolves_menu_then_spawn() {
        let menu = MenuCamera {
            position: [10.0, 20.0, 30.0],
            yaw: 1.5,
            pitch: 0.3,
        };
        assert_eq!(
            resolve_presented_pose(Some(&menu), spawn()),
            PresentedPose {
                position: Vec3::new(10.0, 20.0, 30.0),
                yaw: 1.5,
                pitch: 0.3,
            },
            "the menu pose wins whenever the menu is present, orientation included"
        );
        assert_eq!(resolve_presented_pose(None, spawn()), spawn());
    }
}
