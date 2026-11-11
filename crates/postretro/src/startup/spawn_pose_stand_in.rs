//! A connected client's unarmed stand-in camera from its host's spawn-pose
//! hint. Install takes a matching hint in place of the first spawn, and a
//! hint arriving during Settling, or while Running with the pawn unarmed,
//! moves the stand-in on arrival, before that frame's snapshots can arm the
//! pawn. Only the reveal frame holds a hint back, so it presents the settled
//! pose; the next frame's drain applies it. Arming still writes position
//! only, so the pawn arms at the hint's yaw and pitch.
//! See: context/lib/boot_sequence.md §1 (Presented pose) · context/lib/networking.md §Slot lifecycle

use glam::Vec3;
use postretro_net::wire::SpawnPoseHint;

use crate::App;
use crate::netcode::ClientSpawnPoseHint;
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

    /// This client's hint store; `None` on a host or in single-player.
    fn spawn_pose_hint_store(&self) -> Option<&ClientSpawnPoseHint> {
        self.session
            .as_ref()?
            .net_endpoint
            .as_ref()?
            .client_spawn_pose_hint()
    }

    fn spawn_pose_hint_store_mut(&mut self) -> Option<&mut ClientSpawnPoseHint> {
        self.session
            .as_mut()?
            .net_endpoint
            .as_mut()?
            .client_spawn_pose_hint_mut()
    }

    /// Install's stand-in on a connected client: the latest hint's pose when
    /// it names this install, spent so no later frame moves the camera to it
    /// again. `None` leaves install's first-spawn stand-in.
    pub(super) fn take_install_spawn_pose_hint(&mut self) -> Option<PresentedPose> {
        let (identity, level_life) = self.stand_in_install()?;
        let store = self.spawn_pose_hint_store_mut()?;
        let pose = hint_pose(store.matching(&identity, level_life)?);
        store.spend();
        Some(pose)
    }

    /// The reveal edge on a connected client. Its frame's Control drain, the
    /// first since the edge, holds any hint it brings, so the reveal frame
    /// presents the settled pose.
    pub(crate) fn hold_spawn_pose_hint_through_reveal_frame(&mut self) {
        if let Some(store) = self.spawn_pose_hint_store_mut() {
            store.hold_through_reveal_frame();
        }
    }

    /// A hint's arrival on Control. A non-finite hint is refused before it is
    /// stored. A stored hint moves the stand-in now while Settling, where
    /// Settling reads the presented pose every frame and its timer is
    /// untouched, or while Running with the pawn unarmed, so a hint that
    /// arrives with the arming baseline still sets the facing the pawn arms
    /// with. The reveal frame holds it for the next drain. Otherwise the next
    /// install takes it.
    pub(crate) fn receive_spawn_pose_hint(&mut self, hint: SpawnPoseHint) {
        let Some(store) = self.spawn_pose_hint_store_mut() else {
            return;
        };
        if !store.receive(hint) {
            log::warn!("[Net] ignoring a spawn-pose hint with a non-finite pose");
            return;
        }
        self.apply_fresh_spawn_pose_hint();
    }

    /// The start of a client's Control drain: move the unarmed stand-in to a
    /// hint the reveal frame held, before this drain and its snapshots.
    pub(crate) fn apply_held_spawn_pose_hint(&mut self) {
        self.apply_fresh_spawn_pose_hint();
    }

    /// The end of a client's Control drain: a reveal frame's hold is over.
    pub(crate) fn end_reveal_frame_spawn_pose_hold(&mut self) {
        if let Some(store) = self.spawn_pose_hint_store_mut() {
            store.end_reveal_frame();
        }
    }

    /// Move the camera to a fresh hint naming the installed level, during
    /// Settling or Running and outside the reveal frame's hold. Once the
    /// local pawn has armed, a matching hint is spent unapplied: it moves
    /// nothing. A hint naming another install stays fresh, untouched, and so
    /// does every hint while a host-named load is queued: a same-map restart's
    /// relevel names the new life before the old install unloads, and the old
    /// install would otherwise read that life as its own.
    fn apply_fresh_spawn_pose_hint(&mut self) {
        if !matches!(self.boot_state, BootState::Settling | BootState::Running) {
            return;
        }
        if self
            .level_requests
            .iter()
            .any(|request| matches!(request, crate::startup::LevelRequest::HostLevel(_)))
        {
            return;
        }
        let level_life = self.followed_level_life();
        let Some(identity) = self.published_level_identity.as_deref() else {
            return;
        };
        let Some(pose) = self
            .spawn_pose_hint_store()
            .filter(|store| store.is_fresh() && !store.holds_for_reveal_frame())
            .and_then(|store| store.matching(identity, level_life))
            .map(hint_pose)
        else {
            return;
        };
        let armed = self.local_pawn_is_armed();
        if let Some(store) = self.spawn_pose_hint_store_mut() {
            store.spend();
        }
        if armed {
            return;
        }
        self.camera.position = pose.position;
        self.camera.yaw = pose.yaw;
        self.camera.pitch = pose.pitch;
        self.place_camera_at_presented_pose();
    }
}
