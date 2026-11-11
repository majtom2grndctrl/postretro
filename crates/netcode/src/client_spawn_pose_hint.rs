// A connected client's spawn-pose hint store, held on its endpoint.
// See: context/lib/networking.md §Slot lifecycle · §Session-state ledger

use postretro_net::wire::SpawnPoseHint;

use crate::NetEndpoint;

/// The latest spawn-pose hint the host sent this client.
///
/// Session state, not level state: it lives on the client endpoint, so neither
/// the level-scoped reset nor a demotion touches it, and it goes when the
/// endpoint does. It keeps one hint, the latest; a finite hint always replaces
/// the one before, whatever install it names. A hint is advisory: nothing here
/// gates reveal, Settling or participation.
#[derive(Debug, Default)]
pub struct ClientSpawnPoseHint {
    latest: Option<SpawnPoseHint>,
    /// The latest hint has not yet reached a stand-in camera, and no armed
    /// pawn has made it moot.
    fresh: bool,
    /// The reveal edge fired and its frame's Control drain has not ended: a
    /// hint arriving now waits for a later frame, so the reveal frame
    /// presents the settled pose.
    reveal_frame: bool,
}

impl ClientSpawnPoseHint {
    /// Keep `hint` as the latest. A hint with a non-finite pose component is
    /// refused before it is stored: the store is left as it was and this
    /// returns false.
    pub fn receive(&mut self, hint: SpawnPoseHint) -> bool {
        if !hint.is_finite() {
            return false;
        }
        self.latest = Some(hint);
        self.fresh = true;
        true
    }

    /// The latest hint, matching or not.
    #[must_use]
    pub fn latest(&self) -> Option<&SpawnPoseHint> {
        self.latest.as_ref()
    }

    /// The latest hint when it describes the install named by `identity` and
    /// `level_life`: the pair `Revealed` declares. A different identity, or
    /// another life of the same catalog level, does not match.
    #[must_use]
    pub fn matching(&self, identity: &str, level_life: Option<u64>) -> Option<&SpawnPoseHint> {
        self.latest
            .as_ref()
            .filter(|hint| hint.level_identity == identity && hint.level_life == level_life)
    }

    /// Whether the latest hint arrived after the last [`Self::spend`].
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        self.fresh
    }

    /// The latest hint has reached a stand-in camera, or an armed pawn made it
    /// moot: no later frame moves the camera to it again. The hint stays
    /// stored for a later install it matches.
    pub fn spend(&mut self) {
        self.fresh = false;
    }

    /// The reveal edge: hold every hint, stored but unapplied, until this
    /// frame's Control drain ends.
    pub fn hold_through_reveal_frame(&mut self) {
        self.reveal_frame = true;
    }

    /// Whether the reveal frame is holding hints back.
    #[must_use]
    pub fn holds_for_reveal_frame(&self) -> bool {
        self.reveal_frame
    }

    /// A Control drain ended: the reveal frame's hold, if any, is over.
    pub fn end_reveal_frame(&mut self) {
        self.reveal_frame = false;
    }
}

impl NetEndpoint {
    /// This client's spawn-pose hint store; `None` on a host.
    #[must_use]
    pub fn client_spawn_pose_hint(&self) -> Option<&ClientSpawnPoseHint> {
        match self {
            Self::Client {
                spawn_pose_hint, ..
            } => Some(spawn_pose_hint),
            Self::Host { .. } => None,
        }
    }

    /// Mutable [`Self::client_spawn_pose_hint`].
    pub fn client_spawn_pose_hint_mut(&mut self) -> Option<&mut ClientSpawnPoseHint> {
        match self {
            Self::Client {
                spawn_pose_hint, ..
            } => Some(spawn_pose_hint),
            Self::Host { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(identity: &str, level_life: Option<u64>) -> SpawnPoseHint {
        SpawnPoseHint {
            level_identity: identity.into(),
            level_life,
            eye: [1.0, 2.0, 3.0],
            yaw: 0.5,
            pitch: -0.2,
        }
    }

    // C12: each pose component, poisoned in turn.
    #[test]
    fn non_finite_hint_is_refused_and_the_latest_finite_one_stays() {
        let mut store = ClientSpawnPoseHint::default();
        assert!(store.receive(hint("e1m1", Some(1))));
        store.spend();
        let poisons: [fn(&mut SpawnPoseHint); 5] = [
            |hint| hint.eye[0] = f32::NAN,
            |hint| hint.eye[1] = f32::INFINITY,
            |hint| hint.eye[2] = f32::NEG_INFINITY,
            |hint| hint.yaw = f32::NAN,
            |hint| hint.pitch = f32::INFINITY,
        ];
        for poison in poisons {
            let mut bad = hint("e1m2", Some(2));
            poison(&mut bad);
            assert!(!store.receive(bad));
            assert_eq!(store.latest(), Some(&hint("e1m1", Some(1))));
            assert!(!store.is_fresh(), "a refused hint is not fresh");
        }
    }

    // C5 at the store: identity and life both name the install.
    #[test]
    fn hint_matches_only_the_install_it_names() {
        let mut store = ClientSpawnPoseHint::default();
        assert!(store.matching("e1m1", Some(2)).is_none(), "no hint yet");
        store.receive(hint("e1m1", Some(2)));
        assert!(store.matching("e1m1", Some(2)).is_some());
        assert!(store.matching("e1m1", Some(1)).is_none(), "another life");
        assert!(store.matching("e1m1", None).is_none());
        assert!(store.matching("e1m2", Some(2)).is_none(), "another level");

        store.receive(hint("path:maps/a.prl", None));
        assert!(store.matching("path:maps/a.prl", None).is_some());
        assert!(
            store.matching("e1m1", Some(2)).is_none(),
            "only the latest hint is kept"
        );
    }
}
