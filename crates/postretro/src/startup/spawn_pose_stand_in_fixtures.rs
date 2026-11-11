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
use postretro_net::wire::{RevealedLevel, ServerControlMessage, SpawnPoseHint};
use postretro_scripting_core::runtime::{Frontend, MenuCamera};
use postretro_sim::scripting::map_entity::MapEntity;
use std::time::Instant;

/// A second catalog level the host can move to.
pub(super) const OTHER_LEVEL: &str = "other-arena";

/// What a settle check reports for a level that streams nothing: settled.
const SETTLED: SettleReport = SettleReport {
    sh: ResourceSettle::NotStreamed,
    lightmap: ResourceSettle::NotStreamed,
};

/// The frontend menu's camera: far from every start, facing its own way.
pub(super) fn menu_camera() -> MenuCamera {
    MenuCamera {
        position: [40.0, 12.0, -30.0],
        yaw: 2.0,
        pitch: 0.35,
    }
}

/// The pose the frontend menu presents.
pub(super) fn menu_pose() -> PresentedPose {
    let menu = menu_camera();
    PresentedPose {
        position: Vec3::from_array(menu.position),
        yaw: menu.yaw,
        pitch: menu.pitch,
    }
}

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

/// `messages` with every hint still in flight.
pub(super) fn without_hints(messages: &[ServerControlMessage]) -> Vec<ServerControlMessage> {
    messages
        .iter()
        .filter(|message| !matches!(message, ServerControlMessage::SpawnPoseHint(_)))
        .cloned()
        .collect()
}

/// The host restarts its level, or moves to `catalog_id`, and `client_id`'s
/// next Relevel and hint arrive. Returns what arrived, in arrival order.
pub(super) fn host_reinstalls(
    relay: &mut Relay,
    client_id: u64,
    catalog_id: &str,
    points: &[MapEntity],
) -> Vec<ServerControlMessage> {
    let hints_before = relay.hints(client_id).len();
    relay.unload();
    relay.install_from(Some(catalog_id), points.to_vec());
    relay.pump_until(|relay| relay.hints(client_id).len() > hints_before);
    relay.forward(client_id)
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
        host_named: false,
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

    /// [`Self::new`] with no endpoint: a single-player App.
    pub(super) fn single_player(points: Vec<MapEntity>) -> Self {
        let mut view = Self::new(points);
        view.app.session.as_mut().unwrap().net_endpoint = None;
        view
    }

    /// Register the mod's frontend: its menu tree, the menu camera, and the
    /// host's level as its backdrop. Nothing is presented.
    pub(super) fn with_frontend(mut self) -> Self {
        let session = self.app.session.as_mut().unwrap();
        session.modal_stack.registry_mut().register(
            "mainMenu",
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Mod,
            false,
        );
        session.frontend = Some(Frontend {
            menu_tree: "mainMenu".to_string(),
            background_level: Some(LEVEL.to_string()),
            camera: menu_camera(),
        });
        self
    }

    pub(super) fn menu_is_present(&self) -> bool {
        self.app.frontend_menu_is_present()
    }

    /// How many modal layers are pushed.
    pub(super) fn pushed_layers(&self) -> usize {
        self.app.session.as_ref().unwrap().modal_stack.len()
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

    /// Install the level a relevel requested: the queued request unloads any
    /// installed level and starts the load, then [`Self::commit`].
    pub(super) fn install(&mut self) {
        if !self.app.level_requests.is_empty() {
            self.app.drain_level_requests();
        }
        self.commit();
    }

    /// The load in flight commits, as a Loading frame's commit step runs it
    /// after that frame's transport poll; the fixture stands in for its
    /// stages. Requests queued since stay queued for the next frame's drain.
    /// With no load in flight, the uncatalogued level the relay host
    /// installs, as a client booted on it would.
    pub(super) fn commit(&mut self) {
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

    /// A settled check, then the reveal frame on the next redraw: the reveal
    /// edge and that frame's Control drain, which brings nothing.
    pub(super) fn reveal(&mut self) {
        self.reveal_edge();
        self.drain(Vec::new());
    }

    /// A settled check, then the reveal edge on the next redraw, without its
    /// frame's Control drain.
    pub(super) fn reveal_edge(&mut self) {
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

    /// The install this client last declared revealed to its host.
    pub(super) fn declared_reveal(&self) -> Option<RevealedLevel> {
        match self.app.session.as_ref()?.net_endpoint.as_ref()? {
            crate::netcode::NetEndpoint::Client { client, .. } => client.revealed_level().cloned(),
            crate::netcode::NetEndpoint::Host { .. } => None,
        }
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
