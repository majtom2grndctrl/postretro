//! A connected client's unarmed stand-in camera from its host's spawn-pose
//! hint. Install takes a matching hint in place of the first spawn, a hint
//! arriving during Settling retargets the settle view, and a Running client
//! whose pawn has not armed moves to a fresh one on a later frame. Arming
//! still writes position only, so the pawn arms at the hint's yaw and pitch.
//! See: context/lib/boot_sequence.md §1 (Presented pose) · context/lib/networking.md §Slot lifecycle

use glam::Vec3;
use postretro_net::wire::SpawnPoseHint;

use crate::App;
use crate::startup::BootState;
use crate::startup::presented_pose::PresentedPose;

fn hint_pose(hint: &SpawnPoseHint) -> PresentedPose {
    PresentedPose {
        position: Vec3::from_array(hint.eye),
        yaw: hint.yaw,
        pitch: hint.pitch,
    }
}

impl App {
    /// The install a hint must name: the published parity identity and the
    /// followed host level life, the pair the reveal declares. `None` with no
    /// installed or installing level.
    fn stand_in_install(&self) -> Option<(String, Option<u64>)> {
        let identity = self.published_level_identity.clone()?;
        Some((identity, self.followed_level_life()))
    }

    fn local_pawn_is_armed(&self) -> bool {
        self.session.as_ref().is_some_and(|session| {
            crate::followed_player_pawn(&session.scripting.script_ctx.registry.borrow()).is_some()
        })
    }

    /// Install's stand-in on a connected client: the latest hint's pose when
    /// it names this install, spent so no later frame moves the camera to it
    /// again. `None` leaves install's first-spawn stand-in.
    pub(super) fn take_install_spawn_pose_hint(&mut self) -> Option<PresentedPose> {
        let (identity, level_life) = self.stand_in_install()?;
        let store = self
            .session
            .as_mut()?
            .net_endpoint
            .as_mut()?
            .client_spawn_pose_hint_mut()?;
        let pose = hint_pose(store.matching(&identity, level_life)?);
        store.spend();
        Some(pose)
    }

    /// A hint's arrival on Control. A non-finite hint is refused before it is
    /// stored. A stored hint retargets a Settling view now; Settling reads the
    /// presented pose every frame and its timer is untouched. A Running
    /// client applies it on a later drain ([`Self::apply_held_spawn_pose_hint`]),
    /// so the reveal frame presents the settled pose even when its poll brings
    /// a hint. Otherwise the next install takes it.
    pub(crate) fn receive_spawn_pose_hint(&mut self, hint: SpawnPoseHint) {
        let Some(store) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
            .and_then(|endpoint| endpoint.client_spawn_pose_hint_mut())
        else {
            return;
        };
        if !store.receive(hint) {
            log::warn!("[Net] ignoring a spawn-pose hint with a non-finite pose");
            return;
        }
        if self.boot_state == BootState::Settling {
            self.apply_fresh_spawn_pose_hint();
        }
    }

    /// The start of a Running client's Control drain: move the unarmed
    /// stand-in to a hint that arrived in an earlier drain.
    pub(crate) fn apply_held_spawn_pose_hint(&mut self) {
        if self.boot_state == BootState::Running {
            self.apply_fresh_spawn_pose_hint();
        }
    }

    /// Move the camera to a fresh hint naming the installed level. Once the
    /// local pawn has armed a hint moves nothing, so it is spent unapplied.
    /// One naming another install stays fresh for the install it names.
    fn apply_fresh_spawn_pose_hint(&mut self) {
        let armed = self.local_pawn_is_armed();
        let install = self.stand_in_install();
        let Some(store) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
            .and_then(|endpoint| endpoint.client_spawn_pose_hint_mut())
        else {
            return;
        };
        if !store.is_fresh() {
            return;
        }
        if armed {
            store.spend();
            return;
        }
        let Some(pose) = install.and_then(|(identity, level_life)| {
            store.matching(&identity, level_life).map(hint_pose)
        }) else {
            return;
        };
        store.spend();
        self.camera.position = pose.position;
        self.camera.yaw = pose.yaw;
        self.camera.pitch = pose.pitch;
        self.place_camera_at_presented_pose();
    }
}
