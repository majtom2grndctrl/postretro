//! VM-free foundation data and evaluation substrate for Postretro.
//! See: context/lib/scripting.md §12.

#![deny(unsafe_code)]

/// Canonical fixed simulation step, shared by tick scheduling and elapsed facts.
pub const TICK_DURATION: std::time::Duration = std::time::Duration::from_micros(16_667);

pub mod brain;
pub mod candidate;
pub mod data_descriptors;
pub mod defense;
pub mod foundation_pods;
pub mod impact_particles;
pub mod ir;
pub mod movement;
pub mod owner_tokens;
pub mod pose;
pub mod presentation;
pub mod seat;
pub mod value_types;
pub mod weapon_activation;
pub use weapon_activation::*;

pub use brain::{
    ARCHETYPE_TOLERANCE_STATE_FIELD, BRAIN_ACQUISITION_DUE_INPUT, BRAIN_ATTACK_COOLDOWN_MS_INPUT,
    BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT, BRAIN_DAMAGE_BEARING_INPUT,
    BRAIN_DAMAGE_SOURCE_KNOWN_INPUT, BRAIN_DISTANCE_FROM_ANCHOR_INPUT,
    BRAIN_DISTANCE_TO_LAST_KNOWN_INPUT, BRAIN_HAS_TARGET_INPUT, BRAIN_HEALTH_INPUT,
    BRAIN_INPUT_PREFIX, BRAIN_INPUTS, BRAIN_MAX_HEALTH_INPUT, BRAIN_NO_TARGET_DISTANCE,
    BRAIN_TARGET_DIED_INPUT, BRAIN_TARGET_DISTANCE_INPUT, BRAIN_TARGET_HEALTH_INPUT,
    BRAIN_TARGET_HOSTILE_INPUT, BRAIN_TARGET_MAX_HEALTH_INPUT, BRAIN_TARGET_REACHABLE_INPUT,
    BRAIN_TARGET_VISIBLE_INPUT, BRAIN_TIME_IN_ACTIVITY_MS_INPUT, BRAIN_TIME_SINCE_DAMAGE_MS_INPUT,
    BRAIN_TIME_SINCE_TARGET_VISIBLE_INPUT, BrainInputRef, BrainValidationScope,
    FACTION_STATE_FIELD, bind_brain_guard, resolve_brain_input,
};
pub use candidate::{
    CANDIDATE_DAMAGE_DEALT_TO_ME_INPUT, CANDIDATE_DIED_INPUT, CANDIDATE_DISTANCE_INPUT,
    CANDIDATE_HEALTH_INPUT, CANDIDATE_INPUT_PREFIX, CANDIDATE_INPUTS, CANDIDATE_MAX_HEALTH_INPUT,
    CANDIDATE_SENTIMENT_INPUT, CANDIDATE_TIME_SINCE_DAMAGE_FROM_CANDIDATE_INPUT,
    CANDIDATE_TOLERANCE_INPUT, CandidateInputRef, CandidateValidationScope, bind_candidate_filter,
    resolve_candidate_input,
};
pub use data_descriptors::*;
pub use foundation_pods::{DamagePayload, ModMapEntry, NavAgentParams, knockback_impulse};
pub use ir::*;
pub use movement::{
    ASSIST_DEFAULT_HOLD_MS, ASSIST_DEFAULT_RADIUS_M, ASSIST_HOLD_MS_LITERAL_RULE,
    ASSIST_MAX_HOLD_MS, ASSIST_MAX_RADIUS_M, ASSIST_RADIUS_LITERAL_RULE, Assistable, Capability,
    DashPrograms, GroundRef, ImmersionComponent, LifeStateMovementProfile, LifeStatePosture,
    LifeStateProfile, MovementScope, MovementState, MovementStateKind, NO_TELEPORTER,
    PlayerMovementComponent, SeamlessMapping, TeleportLatches, TeleportState,
    UNDECLARED_LIFE_STATE, assist_hold_ms_literal_valid, assist_radius_literal_valid,
    carry_life_state_stamps, clear_life_state, enter_life_state, player_capability,
    restamp_life_state, wrap_teleport_yaw,
};
pub use pose::{PoseInputs, WALKABLE_SURFACE_MIN_UP_DOT};
pub use presentation::{
    BUILTIN_PRESENTATION_TEMPLATE_ID_PREFIX, MAX_PENDING_PRESENTATION_SPAWNS,
    PresentationDrawInput, PresentationEasing, PresentationFact, PresentationFacts,
    PresentationFade, PresentationMotion, PresentationPresenter, PresentationSpawn,
    PresentationTemplateHandle, is_builtin_presentation_template_id,
};
pub use seat::Seat;
pub use value_types::{EulerDegrees, Vec3Lit};

pub use impact_particles::{ImpactParticleRequest, MAX_PENDING_IMPACT_PARTICLE_REQUESTS};
