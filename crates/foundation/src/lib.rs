//! VM-free foundation data and evaluation substrate for Postretro.
//! See: context/lib/scripting.md §12.

#![deny(unsafe_code)]

pub mod brain;
pub mod candidate;
pub mod data_descriptors;
pub mod foundation_pods;
pub mod ir;
pub mod movement;
pub mod pose;
pub mod presentation;
pub mod seat;
pub mod value_types;

pub use brain::{
    BRAIN_ACQUISITION_DUE_INPUT, BRAIN_ATTACK_COOLDOWN_MS_INPUT,
    BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT, BRAIN_DAMAGE_BEARING_INPUT,
    BRAIN_DAMAGE_SOURCE_KNOWN_INPUT, BRAIN_DISTANCE_FROM_ANCHOR_INPUT,
    BRAIN_DISTANCE_TO_LAST_KNOWN_INPUT, BRAIN_HAS_TARGET_INPUT, BRAIN_HEALTH_INPUT,
    BRAIN_INPUT_PREFIX, BRAIN_INPUTS, BRAIN_MAX_HEALTH_INPUT, BRAIN_NO_TARGET_DISTANCE,
    BRAIN_TARGET_DIED_INPUT, BRAIN_TARGET_DISTANCE_INPUT, BRAIN_TARGET_HEALTH_INPUT,
    BRAIN_TARGET_HOSTILE_INPUT, BRAIN_TARGET_MAX_HEALTH_INPUT, BRAIN_TARGET_REACHABLE_INPUT,
    BRAIN_TARGET_VISIBLE_INPUT, BRAIN_TIME_IN_ACTIVITY_MS_INPUT, BRAIN_TIME_SINCE_DAMAGE_MS_INPUT,
    BRAIN_TIME_SINCE_TARGET_VISIBLE_INPUT, BrainInputRef, BrainValidationScope, bind_brain_guard,
    resolve_brain_input,
};
pub use candidate::{
    CANDIDATE_DAMAGE_DEALT_TO_ME_INPUT, CANDIDATE_DIED_INPUT, CANDIDATE_DISTANCE_INPUT,
    CANDIDATE_HEALTH_INPUT, CANDIDATE_INPUT_PREFIX, CANDIDATE_INPUTS, CANDIDATE_MAX_HEALTH_INPUT,
    CANDIDATE_SENTIMENT_INPUT, CANDIDATE_TIME_SINCE_DAMAGE_FROM_CANDIDATE_INPUT,
    CANDIDATE_TOLERANCE_INPUT, CandidateInputRef, CandidateValidationScope, bind_candidate_filter,
    resolve_candidate_input,
};
pub use data_descriptors::*;
pub use foundation_pods::{DamagePayload, ModMapEntry, NavAgentParams};
pub use ir::*;
pub use movement::{
    DashPrograms, GroundRef, MovementScope, MovementState, MovementStateKind,
    PlayerMovementComponent,
};
pub use pose::{PoseInputs, WALKABLE_SURFACE_MIN_UP_DOT};
pub use presentation::{
    MAX_PENDING_PRESENTATION_SPAWNS, PresentationEasing, PresentationFact, PresentationFacts,
    PresentationFade, PresentationMotion, PresentationPresenter, PresentationSpawn,
    PresentationTemplateHandle, WorldPointPresentationSpawn,
};
pub use seat::Seat;
pub use value_types::{EulerDegrees, Vec3Lit};
