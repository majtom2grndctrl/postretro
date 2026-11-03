// The revive-marker producer on a connected client, over real retained remote
// assist facts, entity mappings, interpolation buffers and pair table. Row ids
// name the acceptance rows of
// context/plans/in-progress/E18--player-revive-assist/addendum-revive-marker.md.

use glam::{Mat4, Vec3};
use postretro_entities::{EntityId, EntityRegistry, PresentationFact, PresentationTemplateHandle};
use postretro_foundation::PlayerMovementComponent;
use postretro_net::wire::{
    ComponentPayload, EntityRecord, NO_WIRE_TELEPORTER, NetworkId, SnapshotMessage,
    WireAssistFacts, WireAssistPair, WireGroundRef, WireMovementState, WirePlayerMovementState,
    WireTeleportState, WireTransform,
};
use postretro_scripting_core::data_descriptors::PresentationWorldAnchor;

use super::*;
use crate::netcode::ClientOverlayFactState;
use crate::presentation_pool::MotionPreference;

const LOCAL: u32 = 3;
const DOWN: u8 = 1;

fn config(max_visible: usize) -> AssistableTeammatesOverlayConfig {
    AssistableTeammatesOverlayConfig {
        template: PresentationTemplateHandle::from("reviveMarker"),
        world_anchor: PresentationWorldAnchor {
            socket: String::new(),
            offset_y: 0.6,
        },
        max_visible,
    }
}

fn movement(life_state: u8, assistable: bool) -> ComponentPayload {
    ComponentPayload::PlayerMovementState(WirePlayerMovementState {
        life_state,
        life_state_elapsed_ticks: 0,
        life_state_table_revision: 0,
        assistable,
        teleport: WireTeleportState::new(0, 0.0, NO_WIRE_TELEPORTER, &[]).unwrap(),
        knockback_velocity: [0.0; 3],
        push_velocity: [0.0; 3],
        velocity: [0.0; 3],
        ground: WireGroundRef::Airborne,
        air_jumps_remaining: 0,
        air_dashes_remaining: 0,
        dash_cooldown_ms: 0.0,
        air_ticks: 0,
        movement_state: WireMovementState::Normal,
        coyote_timer_ms: 0.0,
        jump_buffer_timer_ms: 0.0,
        jump_spent: false,
        swim_reentry_latched: false,
        capsule_half_height: 0.8,
        capsule_eye_height: 1.5,
        aim_pitch: 0.0,
    })
}

/// One remote pawn's record at `x` on the X axis.
#[derive(Clone, Copy)]
struct Remote {
    network_id: u32,
    x: f32,
    assistable: bool,
}

fn record(
    network_id: u32,
    baseline_id: u32,
    x: f32,
    life_state: u8,
    assistable: bool,
    assist_facts: Option<WireAssistFacts>,
) -> EntityRecord {
    EntityRecord::FullBaseline {
        network_id,
        baseline_id,
        last_processed_client_tick: (network_id == LOCAL).then_some(0),
        local_player: network_id == LOCAL,
        assist_facts,
        entity_class: None,
        active_weapon_archetype: None,
        projectile_presentation: None,
        presentation_binding: None,
        components: vec![
            ComponentPayload::Transform(WireTransform {
                position: [x, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            }),
            movement(life_state, assistable),
        ],
    }
}

fn snapshot(sequence: u32, records: Vec<EntityRecord>) -> SnapshotMessage {
    SnapshotMessage {
        sequence,
        server_tick: sequence + 1,
        records,
        state_schema_fingerprint: [0u8; 32],
        state_records: Vec::new(),
        faction_sentiment_record: None,
        ghost_audio: None,
        server_script_time: 0.0,
        level_tick: 0,
    }
}

/// A connected client: its own pawn at the origin, the given remotes, and a
/// pair table holding `pairs` (target network id, radius).
struct Client {
    registry: EntityRegistry,
    replication: ClientReplication,
    pool: PresentationPool,
    zones: HitZoneStore,
    scratch: MarkerScratch,
    sequence: u32,
    local: EntityId,
}

impl Client {
    fn new(remotes: &[Remote], pairs: &[(u32, f32)]) -> Self {
        let mut client = Self {
            registry: EntityRegistry::new(),
            replication: ClientReplication::new_for_test(),
            pool: PresentationPool::default(),
            zones: HitZoneStore::new(),
            scratch: MarkerScratch::default(),
            sequence: 0,
            local: EntityId::from_raw(0),
        };
        client.apply(remotes, pairs);
        client.local = client.entity(LOCAL);
        // The local pawn's movement component comes from its descriptor on a
        // real client; here a plain upright one stands in.
        let mut movement = PlayerMovementComponent::from_descriptor(
            &crate::mod_digest::tests::movement_descriptor(),
        );
        movement.life_state = 0;
        movement.life_use = true;
        client
            .registry
            .set_component(client.local, movement)
            .unwrap();
        client
    }

    /// Apply one snapshot carrying every remote and the local pawn.
    fn apply(&mut self, remotes: &[Remote], pairs: &[(u32, f32)]) {
        let facts = WireAssistFacts {
            pairs: pairs
                .iter()
                .map(|&(network_id, radius)| WireAssistPair {
                    network_id,
                    radius,
                    required_ticks: 180,
                })
                .collect(),
            received_required_ticks: 0,
        };
        let baseline = self.sequence + 1;
        let mut records = vec![record(LOCAL, baseline, 0.0, 0, false, Some(facts))];
        records.extend(remotes.iter().map(|remote| {
            record(
                remote.network_id,
                baseline,
                remote.x,
                if remote.assistable { DOWN } else { 0 },
                remote.assistable,
                None,
            )
        }));
        self.replication
            .apply_snapshot_for_test(&mut self.registry, &snapshot(self.sequence, records));
        self.sequence += 1;
    }

    fn entity(&self, network_id: u32) -> EntityId {
        self.replication
            .entity_network_ids()
            .into_iter()
            .find_map(|(entity, id)| (id == NetworkId(network_id)).then_some(entity))
            .expect("the replication maps this pawn")
    }

    fn set_local_use(&mut self, life_state: u8, permits_use: bool) {
        let mut movement = self
            .registry
            .get_component::<PlayerMovementComponent>(self.local)
            .unwrap()
            .clone();
        movement.life_state = life_state;
        movement.life_use = permits_use;
        self.registry.set_component(self.local, movement).unwrap();
    }

    fn frame(&mut self, config: Option<&AssistableTeammatesOverlayConfig>, tick: Option<f64>) {
        update_client_markers(
            &self.registry,
            &self.replication,
            tick,
            &mut self.pool,
            &self.zones,
            config,
            0.0,
            &mut self.scratch,
        );
    }

    fn marked(&self) -> Vec<EntityId> {
        let mut ids: Vec<_> = self.pool.tracked_overlay_ids_iter(KIND).collect();
        ids.sort();
        ids
    }

    fn in_range(&self, network_id: u32) -> Option<bool> {
        match self
            .pool
            .overlay_facts(KIND, self.entity(network_id))?
            .get(ASSISTABLE_TEAMMATES_IN_RANGE_FACT)
        {
            Some(PresentationFact::Bool(value)) => Some(*value),
            other => panic!("inRange is a Bool fact, got {other:?}"),
        }
    }

    /// The network ids that read `inRange` true.
    fn prompted(&self, remotes: &[Remote]) -> Vec<u32> {
        remotes
            .iter()
            .map(|remote| remote.network_id)
            .filter(|&id| self.in_range(id) == Some(true))
            .collect()
    }
}

fn downed(network_id: u32, x: f32) -> Remote {
    Remote {
        network_id,
        x,
        assistable: true,
    }
}

fn upright(network_id: u32, x: f32) -> Remote {
    Remote {
        network_id,
        x,
        assistable: false,
    }
}

/// R5: every mapped assistable remote is marked at any distance; an
/// unassistable remote and the local pawn never are, and a marker goes the
/// frame its facts flip to unassistable.
#[test]
fn revive_marker_r5_marks_every_assistable_remote_at_any_distance() {
    let remotes = [downed(7, 1.0), downed(8, 500.0), upright(9, 1.0)];
    let mut client = Client::new(&remotes, &[(7, 1.5), (8, 1.5), (9, 1.5)]);
    client.frame(Some(&config(8)), None);
    let mut expected = vec![client.entity(7), client.entity(8)];
    expected.sort();
    assert_eq!(client.marked(), expected);
    assert!(!client.pool.has_overlay(KIND, client.local));
    assert!(!client.pool.has_overlay(KIND, client.entity(9)));

    // An entity the replication never mapped is not a candidate.
    let stray = client
        .registry
        .spawn(postretro_entities::Transform::default());
    assert!(!client.pool.has_overlay(KIND, stray));

    let flipped = [upright(7, 1.0), downed(8, 500.0), upright(9, 1.0)];
    client.apply(&flipped, &[(7, 1.5), (8, 1.5), (9, 1.5)]);
    client.frame(Some(&config(8)), None);
    assert_eq!(client.marked(), vec![client.entity(8)]);
}

/// R5: `maxVisible` keeps the first markers in the client's `players()`
/// order (entity order), and a freed slot goes to the next in that order.
#[test]
fn revive_marker_r5_max_visible_caps_in_players_order() {
    let remotes = [downed(7, 9.0), downed(8, 1.0), downed(9, 5.0)];
    let mut client = Client::new(&remotes, &[]);
    let mut order: Vec<_> = [7, 8, 9].map(|id| client.entity(id)).to_vec();
    order.sort();
    client.frame(Some(&config(2)), None);
    assert_eq!(client.marked(), order[..2].to_vec());

    // Whichever pawn sorted first stands up; the third takes its slot.
    let first = [7, 8, 9]
        .into_iter()
        .find(|&id| client.entity(id) == order[0])
        .unwrap();
    let after: Vec<_> = remotes
        .iter()
        .map(|remote| {
            if remote.network_id == first {
                upright(first, remote.x)
            } else {
                *remote
            }
        })
        .collect();
    client.apply(&after, &[]);
    client.frame(Some(&config(2)), None);
    assert_eq!(client.marked(), order[1..].to_vec());
}

/// R6: `inRange` marks only the teammate the shared target choice picks, and
/// reads false for every case that choice rules out.
#[test]
fn revive_marker_r6_in_range_is_only_the_pawn_a_hold_would_revive() {
    let near = downed(7, 1.0);
    let far = downed(8, 1.4);
    let remotes = [near, far];
    let pairs = [(7, 1.5), (8, 1.5)];
    let mut client = Client::new(&remotes, &pairs);
    client.frame(Some(&config(8)), None);
    assert_eq!(client.prompted(&remotes), [7], "the nearer of two in range");
    assert_eq!(client.in_range(8), Some(false));

    // Equal distance: the earlier pawn in entity order wins.
    let tied = [downed(7, 1.0), downed(8, -1.0)];
    client.apply(&tied, &pairs);
    client.frame(Some(&config(8)), None);
    let earlier = if client.entity(7) < client.entity(8) {
        7
    } else {
        8
    };
    assert_eq!(client.prompted(&tied), [earlier]);

    // The local pawn's state forbids `use`: plain markers only.
    client.apply(&remotes, &pairs);
    client.set_local_use(DOWN, false);
    client.frame(Some(&config(8)), None);
    assert!(client.prompted(&remotes).is_empty());
    assert_eq!(
        client.marked().len(),
        2,
        "markers stay while the prompt goes"
    );
    client.set_local_use(0, true);

    // The pair entry has not arrived.
    client.apply(&remotes, &[(8, 1.5)]);
    client.frame(Some(&config(8)), None);
    assert_eq!(client.prompted(&remotes), [8], "7 has no pair entry");

    // Radius 0: this pawn cannot assist that one.
    client.apply(&remotes, &[(7, 0.0), (8, 0.0)]);
    client.frame(Some(&config(8)), None);
    assert!(client.prompted(&remotes).is_empty());

    // Past the radius.
    let beyond = [downed(7, 1.6), downed(8, 3.0)];
    client.apply(&beyond, &pairs);
    client.frame(Some(&config(8)), None);
    assert!(client.prompted(&beyond).is_empty());
    assert_eq!(client.marked().len(), 2);
}

/// R9 (client): with no posed socket the anchor is the interpolated pose at
/// `render_server_tick` plus `offsetY`, not the raw applied pose and never a
/// hitbox top.
#[test]
fn revive_marker_r9_client_anchor_is_the_interpolated_pose_plus_offset() {
    let mut client = Client::new(&[downed(7, 0.0)], &[]);
    // A second snapshot ten ticks later moves the pawn to x = 10.
    client.sequence = 10;
    client.apply(&[downed(7, 10.0)], &[]);
    let entity = client.entity(7);
    let raw = client
        .registry
        .get_component::<postretro_entities::Transform>(entity)
        .unwrap()
        .position;
    assert_eq!(raw.x, 10.0);

    client.frame(Some(&config(8)), Some(3.0));
    let anchor = client.pool.overlay_anchor(KIND, entity).unwrap();
    let expected = client
        .replication
        .remote_assist_position(entity, Some(3.0))
        .unwrap()
        + Vec3::Y * 0.6;
    assert!(
        (anchor - expected).length() < 1e-4,
        "{anchor:?} {expected:?}"
    );
    assert!(anchor.x < 10.0, "not the raw newest pose");
}

/// R13 (client): with only the marker overlay declared, the damaged-enemy
/// anchor pass (which runs with no config of its own) and the pool's frame
/// advance leave every marker in place.
#[test]
fn revive_marker_r13_client_markers_survive_the_no_config_damaged_enemy_pass() {
    let mut client = Client::new(&[downed(7, 1.0)], &[(7, 1.5)]);
    let mut facts = ClientOverlayFactState::default();
    for _ in 0..5 {
        client.frame(Some(&config(8)), None);
        crate::netcode::update_client_overlay_anchors(
            &client.registry,
            &[],
            &mut facts,
            &client.replication,
            &mut client.pool,
            None,
            &client.zones,
            None,
            0.0,
            0.0,
            None,
        );
        let inputs = client.pool.advance_and_collect_inputs(
            &mut client.registry,
            1.0,
            Mat4::IDENTITY,
            [800, 600],
            MotionPreference::Full,
        );
        client.pool.recycle_draw_inputs(inputs);
        assert_eq!(client.marked(), vec![client.entity(7)]);
    }

    // Without the marker overlay only markers go.
    client.frame(None, None);
    assert!(client.marked().is_empty());
}
