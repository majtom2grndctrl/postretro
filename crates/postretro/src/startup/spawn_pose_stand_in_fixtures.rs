//! A connected client App for the stand-in tests. It drains the Control
//! messages a relay client received from the real host, installs its own copy
//! of the level through install's identity, segment-B and camera stages,
//! settles and reveals through the Settling edge, and arms the way
//! `maybe_arm_local_pawn` does: by marking a pawn at the host's spawn as the
//! local player before the predicted tick's camera follow.
//! See: context/lib/testing_guide.md §4

use super::lifecycle_test_fixtures::{
    catalog_map, connect_test_app_as_client, drop_in_flight_load, install_capture_world,
    level_world, script_ctx, test_app,
};
use super::spawn_pose_handoff_fixtures::{LEVEL, Relay, descriptors};
use super::*;
use crate::session::level_streaming::settle::{ResourceSettle, SettleReport};
use crate::startup::presented_pose::PresentedPose;
use postretro_entities::{EntityId, Transform};
use postretro_foundation::PlayerMovementComponent;
use postretro_level_format::map_entity::MapEntityRecord;
use postretro_net::wire::{ServerControlMessage, SpawnPoseHint};
use postretro_sim::scripting::map_entity::MapEntity;
use std::time::Instant;

/// A second catalog level the host can move to.
pub(super) const OTHER_LEVEL: &str = "other-arena";

/// What a settle check reports for a level that streams nothing: settled.
pub(super) const SETTLED: SettleReport = SettleReport {
    sh: ResourceSettle::NotStreamed,
    lightmap: ResourceSettle::NotStreamed,
};

/// The pose a hint describes.
pub(super) fn hint_pose(hint: &SpawnPoseHint) -> PresentedPose {
    PresentedPose {
        position: Vec3::from_array(hint.eye),
        yaw: hint.yaw,
        pitch: hint.pitch,
    }
}

/// `actual` is within 1 cm of `expected`, facing the same way.
#[track_caller]
pub(super) fn assert_pose(actual: PresentedPose, expected: PresentedPose, what: &str) {
    assert!(
        actual.position.distance(expected.position) < 0.01,
        "{what}: position {:?}, expected {:?}",
        actual.position,
        expected.position
    );
    assert!(
        (actual.yaw - expected.yaw).abs() < 1e-6 && (actual.pitch - expected.pitch).abs() < 1e-6,
        "{what}: yaw {} pitch {}, expected yaw {} pitch {}",
        actual.yaw,
        actual.pitch,
        expected.yaw,
        expected.pitch
    );
}

/// `points` as the level file carries them, in this client's map order.
fn client_world(points: &[MapEntity]) -> postretro_level_loader::LevelWorld {
    let mut world = level_world(LEVEL, 1);
    world.map_entities = points
        .iter()
        .map(|point| MapEntityRecord {
            classname: point.classname.clone(),
            origin: point.origin.to_array(),
            angles: point.angles.to_array(),
            key_values: point
                .key_values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            tags: point.tags.clone(),
        })
        .collect();
    world
}

/// The uncatalogued load the relay host installs from `install_from(None, ..)`.
fn uncatalogued_load() -> InFlightLevelLoad {
    InFlightLevelLoad {
        map_path: PathBuf::from("content/dev/maps/spawn-arena.prl"),
        content_root: PathBuf::from("content/dev"),
        entry: LevelLoadEntry {
            catalog_id: None,
            path: "maps/spawn-arena.prl".to_string(),
            name: "Spawn arena".to_string(),
            tags: Vec::new(),
            loading_tree: Vec::new(),
        },
    }
}

pub(super) struct ClientView {
    pub(super) app: App,
    /// This client's own copy of the level's `player_spawn` placements, in its
    /// local map order.
    points: Vec<MapEntity>,
    /// Install's first-spawn stand-in on the last install: standing position
    /// and angles.
    first_spawn: Option<(Vec3, Vec3)>,
}

impl ClientView {
    /// A connected client at its frontend, before any level, whose catalog
    /// holds the host's levels and whose level copy carries `points`.
    pub(super) fn new(points: Vec<MapEntity>) -> Self {
        let mut app = test_app();
        connect_test_app_as_client(&mut app);
        let ctx = script_ctx(&app);
        ctx.data_registry.borrow_mut().entities = descriptors();
        ctx.data_registry.borrow_mut().replace_maps(vec![
            catalog_map(LEVEL, "maps/spawn-arena.prl", "Spawn arena", &[]),
            catalog_map(OTHER_LEVEL, "maps/other-arena.prl", "Other arena", &[]),
        ]);
        app.boot_state = BootState::Frontend;
        Self {
            app,
            points,
            first_spawn: None,
        }
    }

    /// One Control drain, as the client's poll runs it.
    pub(super) fn drain(&mut self, messages: Vec<ServerControlMessage>) {
        crate::client_drain_control(&mut self.app, messages);
    }

    /// Drain, in one go, what `client_id` received since the last forward.
    pub(super) fn follow(&mut self, relay: &mut Relay, client_id: u64) {
        let messages = relay.forward(client_id);
        self.drain(messages);
    }

    /// Install the level a relevel requested, as a load's commit runs it: the
    /// queued request unloads any installed level and starts the load, whose
    /// stages the fixture stands in for. With no relevel, the uncatalogued
    /// level the relay host installs, as a client booted on it would.
    pub(super) fn install(&mut self) {
        if !self.app.level_requests.is_empty() {
            self.app.drain_level_requests();
        }
        let load = self
            .app
            .level_load
            .clone()
            .unwrap_or_else(uncatalogued_load);
        // The load cannot read a fixture path; install its payload directly.
        drop_in_flight_load(&mut self.app);
        self.app.level_load = Some(load);
        let world = client_world(&self.points);
        self.app.install_level_identity(&world);
        self.first_spawn = install_capture_world(&mut self.app, &world);
        self.app.level = Some(world);
        self.app.install_level_camera_pose(self.first_spawn);
        self.app.enter_installed_level_settling();
    }

    /// The stand-in install takes without a matching hint: this client's
    /// first `player_spawn`, standing.
    pub(super) fn first_spawn_pose(&self) -> PresentedPose {
        let (position, angles) = self.first_spawn.expect("the level has a player_spawn");
        PresentedPose {
            position,
            yaw: angles.y,
            pitch: angles.x,
        }
    }

    /// A settled check, then the reveal edge on the next redraw.
    pub(super) fn reveal(&mut self) {
        assert_eq!(self.app.boot_state, BootState::Settling);
        self.app.advance_settle(SETTLED, Instant::now());
        assert!(self.app.reveal_if_released(), "a settled level reveals");
        assert_eq!(self.app.boot_state, BootState::Running);
    }

    pub(super) fn camera(&self) -> PresentedPose {
        PresentedPose {
            position: self.app.camera.position,
            yaw: self.app.camera.yaw,
            pitch: self.app.camera.pitch,
        }
    }

    /// The camera, and both interpolation endpoints a frame renders between,
    /// are at `expected`.
    #[track_caller]
    pub(super) fn assert_at(&self, expected: PresentedPose, what: &str) {
        assert_pose(self.camera(), expected, what);
        for endpoint in [
            self.app.frame_timing.previous_state.position,
            self.app.frame_timing.current_state.position,
        ] {
            assert!(
                endpoint.distance(expected.position) < 0.01,
                "{what}: a rendered endpoint sits at {endpoint:?}, not {:?}",
                expected.position
            );
        }
    }

    /// The latest hint this client's store keeps.
    pub(super) fn stored_hint(&self) -> Option<SpawnPoseHint> {
        self.app
            .session
            .as_ref()?
            .net_endpoint
            .as_ref()?
            .client_spawn_pose_hint()?
            .latest()
            .cloned()
    }

    /// The host's first baseline for `host_pawn` arms this client's pawn:
    /// a pawn at the host pawn's spawn transform and movement, marked the
    /// local player as `maybe_arm_local_pawn` marks it, then the arming
    /// frame's predicted tick, whose camera follow writes position only.
    pub(super) fn arm(&mut self, relay: &Relay, host_pawn: EntityId) -> EntityId {
        let (transform, movement) = {
            let host = script_ctx(&relay.app);
            let registry = host.registry.borrow();
            (
                *registry.get_component::<Transform>(host_pawn).unwrap(),
                registry
                    .get_component::<PlayerMovementComponent>(host_pawn)
                    .unwrap()
                    .clone(),
            )
        };
        let pawn = {
            let ctx = script_ctx(&self.app);
            let mut registry = ctx.registry.borrow_mut();
            let pawn = registry.spawn(transform);
            registry.set_component(pawn, movement).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
            pawn
        };
        self.app.run_client_predicted_tick(true, |_| None);
        pawn
    }
}
