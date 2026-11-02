//! Assist facts on the local-pawn record: the flag, the local-pawn gate and
//! the wire bounds.

use super::super::{
    COMPONENT_KIND_PLAYER_MOVEMENT_STATE, EntityRecord, RECORD_KIND_DELTA, RECORD_KIND_DESPAWN,
    RECORD_KIND_FULL_BASELINE, RawComponentPayload, RawEntityRecord, ValidationError,
    WireGroundRef, WireMovementState, WirePlayerMovementState, decode, encode,
};
use super::*;

fn movement_payload() -> RawComponentPayload {
    RawComponentPayload {
        teleporter: None,
        component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
        transform: None,
        player_movement: Some(WirePlayerMovementState {
            life_state: 255,
            life_state_elapsed_ticks: 0,
            life_state_table_revision: 0,
            assistable: false,
            teleport: Default::default(),
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
        }),
        mesh_animation_state: None,
        kinematic_mover: None,
        light: None,
        emitter: None,
        fog: None,
        ghost_link: None,
    }
}

fn record(record_kind: u16, local_player: bool) -> RawEntityRecord {
    RawEntityRecord {
        record_kind,
        network_id: 4,
        baseline_id_or_ref: 2,
        new_baseline_id_or_tombstone_id: if record_kind == RECORD_KIND_DELTA {
            3
        } else {
            0
        },
        reason: 0,
        has_last_processed_client_tick: false,
        last_processed_client_tick: 0,
        local_player,
        has_assist_facts: false,
        assist_pairs: Vec::new(),
        received_required_ticks: 0,
        has_entity_class: false,
        entity_class: String::new(),
        has_active_weapon_archetype: false,
        active_weapon_archetype: String::new(),
        components: vec![movement_payload()],
        projectile_presentation: None,
        presentation_binding: None,
    }
}

fn facts() -> WireAssistFacts {
    WireAssistFacts {
        pairs: vec![
            WireAssistPair {
                network_id: 2,
                radius: 0.0,
                required_ticks: 0,
            },
            WireAssistPair {
                network_id: 7,
                radius: WIRE_ASSIST_MAX_RADIUS,
                required_ticks: WIRE_ASSIST_MAX_REQUIRED_TICKS,
            },
        ],
        received_required_ticks: WIRE_ASSIST_MAX_REQUIRED_TICKS,
    }
}

fn typed_facts(record: &EntityRecord) -> Option<&WireAssistFacts> {
    match record {
        EntityRecord::FullBaseline { assist_facts, .. }
        | EntityRecord::Delta { assist_facts, .. } => assist_facts.as_ref(),
        EntityRecord::Despawn { .. } => None,
    }
}

#[test]
fn assist_facts_ride_the_local_pawn_record_behind_their_flag() {
    for kind in [RECORD_KIND_FULL_BASELINE, RECORD_KIND_DELTA] {
        let mut raw = record(kind, true);
        facts().stamp(&mut raw);
        let decoded: RawEntityRecord = decode(&encode(&raw)).expect("record decodes");
        let typed = decoded.validate().expect("local-pawn facts validate");
        assert_eq!(typed_facts(&typed), Some(&facts()));

        // Without the flag the local record carries none.
        let typed = record(kind, true).validate().expect("unflagged validates");
        assert_eq!(typed_facts(&typed), None);
    }
}

/// O59: the last candidate leaving yields an empty list behind the flag,
/// which is distinct from an absent table.
#[test]
fn assist_facts_o59_empty_list_behind_the_flag_validates() {
    let mut raw = record(RECORD_KIND_DELTA, true);
    WireAssistFacts::default().stamp(&mut raw);
    let typed = raw.validate().expect("empty facts validate");
    assert_eq!(typed_facts(&typed), Some(&WireAssistFacts::default()));
}

#[test]
fn assist_facts_flag_on_a_non_local_record_fails_validation() {
    for kind in [RECORD_KIND_FULL_BASELINE, RECORD_KIND_DELTA] {
        let mut raw = record(kind, false);
        facts().stamp(&mut raw);
        assert_eq!(
            raw.validate(),
            Err(ValidationError::MovementMetadataWithoutMovement)
        );
    }
}

#[test]
fn assist_fields_without_the_flag_fail_validation() {
    let mut pairs = record(RECORD_KIND_DELTA, true);
    pairs.assist_pairs = facts().pairs;
    assert_eq!(pairs.validate(), Err(ValidationError::InvalidAssistFacts));

    let mut received = record(RECORD_KIND_DELTA, true);
    received.received_required_ticks = 1;
    assert_eq!(
        received.validate(),
        Err(ValidationError::InvalidAssistFacts)
    );
}

#[test]
fn assist_facts_out_of_range_values_fail_validation() {
    let radius = |radius: f32| {
        let mut facts = facts();
        facts.pairs[1].radius = radius;
        facts
    };
    let pair_ticks = {
        let mut facts = facts();
        facts.pairs[0].required_ticks = WIRE_ASSIST_MAX_REQUIRED_TICKS + 1;
        facts
    };
    let received = WireAssistFacts {
        pairs: Vec::new(),
        received_required_ticks: WIRE_ASSIST_MAX_REQUIRED_TICKS + 1,
    };
    for bad in [
        radius(f32::NAN),
        radius(f32::INFINITY),
        radius(-0.5),
        radius(WIRE_ASSIST_MAX_RADIUS + 0.01),
        pair_ticks,
        received,
    ] {
        let mut raw = record(RECORD_KIND_FULL_BASELINE, true);
        bad.stamp(&mut raw);
        assert_eq!(
            raw.validate(),
            Err(ValidationError::InvalidAssistFacts),
            "{bad:?}"
        );
    }
    // Negative zero is zero, inside the bound.
    let mut raw = record(RECORD_KIND_FULL_BASELINE, true);
    radius(-0.0).stamp(&mut raw);
    assert!(raw.validate().is_ok());
}

#[test]
fn assist_facts_on_a_despawn_fail_validation() {
    let mut raw = record(RECORD_KIND_DESPAWN, false);
    raw.components.clear();
    raw.has_assist_facts = true;
    assert_eq!(raw.validate(), Err(ValidationError::MetadataOnDespawn));
}
