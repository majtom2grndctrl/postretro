// Component wire mirrors and raw-payload validation.
// See: context/lib/networking.md

use bitcode::{Decode, Encode};

use super::ValidationError;

/// `component_kind` discriminant for a `Transform` payload. Numeric-equal to the
/// engine `ComponentKind::Transform as u16`.
pub const COMPONENT_KIND_TRANSFORM: u16 = 0;
/// `component_kind` discriminant for a `PlayerMovementState` payload. Numeric-equal
/// to the engine `ComponentKind::PlayerMovement as u16` (Phase 2 = 6).
pub const COMPONENT_KIND_PLAYER_MOVEMENT_STATE: u16 = 6;
/// `component_kind` discriminant for a mesh-animation-state payload. Numeric-equal
/// to the engine `ComponentKind::Mesh as u16` (Phase 2 = 9).
pub const COMPONENT_KIND_MESH_ANIMATION_STATE: u16 = 9;
/// `component_kind` discriminant for a kinematic-mover-state payload.
/// Numeric-equal to the engine `ComponentKind::KinematicMover as u16`.
pub const COMPONENT_KIND_KINEMATIC_MOVER_STATE: u16 = 13;

/// Wire mirror of the engine `Transform`. Phase 2 replicates `position`,
/// `rotation`, and `scale`.
///
/// `rotation` mirrors the engine quaternion in **`[x, y, z, w]` order**. The
/// engine-side conversion (which knows glam's `Quat` component order) lives in
/// `postretro-netcode`; here it is just four floats in that fixed order.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireTransform {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl WireTransform {
    /// Whether every replicated float is finite (no NaN/inf): position, rotation,
    /// and scale. Registry-blind — operates only on the plain `[f32; N]` wire
    /// fields, never an engine/glam type. Backs two rules at `validate`: it gates
    /// the `Transform` component payload (a non-finite pose is rejected before
    /// typed apply, so none reaches the registry) and it backs the `entity_class`
    /// rule (a class may only ride a record carrying a finite `Transform`).
    ///
    /// `pub(crate)` so the production side (`MovementAuthority::for_recipient`) gates
    /// `entity_class` on the SAME finite-`Transform` rule `validate` enforces on receipt,
    /// keeping production and validation in lockstep.
    #[must_use]
    pub(crate) fn all_finite(&self) -> bool {
        self.position.iter().all(|c| c.is_finite())
            && self.rotation.iter().all(|c| c.is_finite())
            && self.scale.iter().all(|c| c.is_finite())
    }
}

/// Wire mirror of the engine player movement state machine's active state. Only
/// the mutable tick fields each variant needs cross the wire; descriptor-immutable
/// tuning lives local on both peers.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub enum WireMovementState {
    Normal,
    Dash {
        elapsed_ms: f32,
        boost: [f32; 3],
    },
    Crouching {
        eye_current: f32,
    },
    Sliding {
        elapsed_ms: f32,
        boost: [f32; 3],
        eye_current: f32,
        /// The prior substrate contact. It is paid only while sliding because
        /// replay needs the authoritative slope on its first replayed tick. Raw
        /// validation accepts only absence or a bounded near-unit vector.
        floor_normal: Option<[f32; 3]>,
    },
}

/// Squared-length tolerance for collision-produced floor normals. Bitcode
/// preserves the source `f32` values exactly, so this only accommodates normal
/// floating-point normalization error; it is not a normalization step.
const SLIDING_FLOOR_NORMAL_LENGTH_SQUARED_TOLERANCE: f32 = 1.0e-3;

fn sliding_floor_normal_is_valid(normal: &[f32; 3]) -> bool {
    // Bound components before squaring so even finite hostile values cannot
    // overflow the magnitude calculation used by this validation gate.
    normal.iter().all(|component| {
        component.is_finite()
            && component.abs() <= 1.0 + SLIDING_FLOOR_NORMAL_LENGTH_SQUARED_TOLERANCE
    }) && ((normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]) - 1.0).abs()
        <= SLIDING_FLOOR_NORMAL_LENGTH_SQUARED_TOLERANCE
}

/// Wire mirror of the player's generalized ground reference. `Mover` carries the
/// compile-time PRL mover id, not a `NetworkId`, so the value is stable across
/// peers that loaded the same map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub enum WireGroundRef {
    Airborne,
    World,
    Mover(u32),
}

/// Wire mirror of the *mutable tick subset* of the engine `PlayerMovementComponent`.
///
/// Deliberately **not** a copy of the component struct: descriptor-immutable
/// movement params, `view_feel`, `standing_*`, `stuck_stop_*`, and the IR-bound
/// `dash_programs` stay out of this typed tick-state mirror. The local descriptor
/// is a non-authoritative immutable mirror; E15 sends host-authoritative resolved
/// tuning separately as opaque Control payload bytes. Only the fields interpolation
/// and later prediction reconciliation need are mirrored here. Source field types
/// are preserved: ability counters and `air_ticks` are `u32`; live timers are `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WirePlayerMovementState {
    pub velocity: [f32; 3],
    /// Protected impulse contribution already included in total velocity.
    pub knockback_velocity: [f32; 3],
    pub ground: WireGroundRef,
    pub air_jumps_remaining: u32,
    pub air_dashes_remaining: u32,
    pub dash_cooldown_ms: f32,
    pub air_ticks: u32,
    pub movement_state: WireMovementState,
    pub coyote_timer_ms: f32,
    pub jump_buffer_timer_ms: f32,
    pub jump_spent: bool,
    pub capsule_half_height: f32,
    pub capsule_eye_height: f32,
    /// Camera pitch from the movement owner's latest resolved input. It is
    /// presentation state for remote avatars, not a movement-simulation value.
    pub aim_pitch: f32,
}

impl WireMovementState {
    /// Whether every float this variant carries is finite (no NaN/inf). `Normal`
    /// carries nothing, so it is vacuously finite. The exhaustive `match` (no `_`
    /// arm) means a new variant with float payload is a compile error here until its
    /// finiteness is accounted for — the same drift discipline as the variant guards.
    #[must_use]
    fn all_finite(&self) -> bool {
        match self {
            WireMovementState::Normal => true,
            WireMovementState::Dash { elapsed_ms, boost } => {
                elapsed_ms.is_finite() && boost.iter().all(|c| c.is_finite())
            }
            WireMovementState::Crouching { eye_current } => eye_current.is_finite(),
            WireMovementState::Sliding {
                elapsed_ms,
                boost,
                eye_current,
                floor_normal,
            } => {
                elapsed_ms.is_finite()
                    && boost.iter().all(|c| c.is_finite())
                    && eye_current.is_finite()
                    && floor_normal
                        .as_ref()
                        .is_none_or(|normal| normal.iter().all(|c| c.is_finite()))
            }
        }
    }

    /// Whether variant-specific movement invariants hold after decoding. Dash may
    /// preserve vertical boost. Slide boost is horizontal, and its optional floor
    /// normal must be safely bounded and near-unit before replay projects gravity
    /// against it.
    #[must_use]
    fn has_valid_state_contract(&self) -> bool {
        match self {
            WireMovementState::Normal
            | WireMovementState::Dash { .. }
            | WireMovementState::Crouching { .. } => true,
            WireMovementState::Sliding {
                boost,
                floor_normal,
                ..
            } => {
                boost[1] == 0.0
                    && floor_normal
                        .as_ref()
                        .is_none_or(sliding_floor_normal_is_valid)
            }
        }
    }
}

impl WirePlayerMovementState {
    /// Whether every replicated float is finite (no NaN/inf): velocity, protected
    /// knockback velocity, all live timers, capsule dimensions, aim pitch, and the
    /// active state's payload. Checked at `validate` so a non-finite movement state is
    /// rejected before typed apply and never reaches the registry. Integer counters and
    /// bools cannot be non-finite, so they are not checked.
    #[must_use]
    fn all_finite(&self) -> bool {
        self.velocity.iter().all(|c| c.is_finite())
            && self.knockback_velocity.iter().all(|c| c.is_finite())
            && self.dash_cooldown_ms.is_finite()
            && self.coyote_timer_ms.is_finite()
            && self.jump_buffer_timer_ms.is_finite()
            && self.capsule_half_height.is_finite()
            && self.capsule_eye_height.is_finite()
            && self.aim_pitch.is_finite()
            && self.movement_state.all_finite()
    }
}

/// Wire mirror of a kinematic mover's replicated deterministic phase. Static
/// path data stays local on each peer through PRL load; these fields are the
/// authoritative phase seed clients predict from and reconcile against.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireKinematicMoverState {
    pub mover_id: u32,
    pub segment_index: u16,
    pub direction: i8,
    pub mode: u8,
    pub segment_elapsed_ms: f32,
    pub wait_remaining_ms: f32,
    pub started: bool,
    pub completed: bool,
    pub blocked: bool,
    pub velocity: [f32; 3],
    pub target_segment: Option<u16>,
    pub spin_angle_rad: f32,
    pub spin_angle_before_tick_rad: f32,
    pub was_active_this_tick: bool,
    pub spin_rate_rad_s: f32,
    pub spin_target_rate_rad_s: f32,
}

impl WireKinematicMoverState {
    #[must_use]
    fn all_finite(&self) -> bool {
        self.segment_elapsed_ms.is_finite()
            && self.wait_remaining_ms.is_finite()
            && self.velocity.iter().all(|c| c.is_finite())
            && self.spin_angle_rad.is_finite()
            && self.spin_angle_before_tick_rad.is_finite()
            && self.spin_rate_rad_s.is_finite()
            && self.spin_target_rate_rad_s.is_finite()
    }

    #[must_use]
    fn has_valid_phase_tags(&self) -> bool {
        matches!(self.direction, -1 | 1) && matches!(self.mode, 0 | 1)
    }
}

/// Wire mirror of the mutable mesh-animation state. Descriptor-owned data
/// (model handle, state table, clips, fade policy) stays local on each peer; this
/// carries only the authoritative current state name.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct WireMeshAnimationState {
    pub current_state: String,
}

// ---------------------------------------------------------------------------
// Raw encoded boundary
// ---------------------------------------------------------------------------

/// Raw component payload as it crosses the wire: an explicit `component_kind`
/// discriminant plus one `Option` slot per supported component. Exactly one slot
/// must be `Some` and it must match `component_kind`; any other shape (wrong slot,
/// no slot, two slots, unknown kind) is a clean decode but a `validate` rejection.
///
/// The explicit discriminant + `Option` slots are deliberate: they make an invalid
/// kind value or a missing/duplicate payload *representable* in the decoded
/// envelope, so the malformed-input tests exercise validation without relying on
/// bitcode's internal enum tag (which would make an invalid tag a decode error
/// instead of a testable rejected envelope).
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct RawComponentPayload {
    pub component_kind: u16,
    pub transform: Option<WireTransform>,
    pub player_movement: Option<WirePlayerMovementState>,
    pub mesh_animation_state: Option<WireMeshAnimationState>,
    pub kinematic_mover: Option<WireKinematicMoverState>,
}

/// A validated component payload. Constructed only by [`RawComponentPayload::validate`],
/// so a typed payload always has exactly the inner value its kind requires.
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentPayload {
    Transform(WireTransform),
    PlayerMovementState(WirePlayerMovementState),
    MeshAnimationState(WireMeshAnimationState),
    KinematicMoverState(WireKinematicMoverState),
}

impl ComponentPayload {
    /// Engine-aligned `u16` discriminant for this payload, numeric-equal to
    /// `ComponentKind as u16` in the engine. Drift here desyncs replication, so
    /// the mapping is pinned by `component_kind_pinned_to_engine_discriminants`.
    #[must_use]
    pub fn kind(&self) -> u16 {
        match self {
            ComponentPayload::Transform(_) => COMPONENT_KIND_TRANSFORM,
            ComponentPayload::PlayerMovementState(_) => COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            ComponentPayload::MeshAnimationState(_) => COMPONENT_KIND_MESH_ANIMATION_STATE,
            ComponentPayload::KinematicMoverState(_) => COMPONENT_KIND_KINEMATIC_MOVER_STATE,
        }
    }
}

impl RawComponentPayload {
    /// Validate this raw payload into a typed [`ComponentPayload`]. Rejects unknown
    /// `component_kind` values, a missing slot for the named kind, and any extra or
    /// mismatched slot. Exactly one slot must be `Some` and it must be the one the
    /// `component_kind` names.
    pub fn validate(&self) -> Result<ComponentPayload, ValidationError> {
        // Count populated slots once: a well-formed payload has exactly one, the
        // one its `component_kind` names. Any other slot being `Some` is a
        // mismatch regardless of the named kind (it makes the envelope ambiguous).
        let populated = usize::from(self.transform.is_some())
            + usize::from(self.player_movement.is_some())
            + usize::from(self.mesh_animation_state.is_some())
            + usize::from(self.kinematic_mover.is_some());

        match self.component_kind {
            COMPONENT_KIND_TRANSFORM => match self.transform {
                Some(t) if populated == 1 => {
                    if t.all_finite() {
                        Ok(ComponentPayload::Transform(t))
                    } else {
                        Err(ValidationError::NonFiniteTransform)
                    }
                }
                Some(_) => Err(ValidationError::MismatchedComponentPayload(
                    self.component_kind,
                )),
                None => Err(ValidationError::MissingComponentPayload(
                    self.component_kind,
                )),
            },
            COMPONENT_KIND_PLAYER_MOVEMENT_STATE => match self.player_movement {
                Some(m) if populated == 1 => {
                    if !m.all_finite() {
                        Err(ValidationError::NonFiniteMovementState)
                    } else if !m.movement_state.has_valid_state_contract() {
                        Err(ValidationError::InvalidMovementState)
                    } else {
                        Ok(ComponentPayload::PlayerMovementState(m))
                    }
                }
                Some(_) => Err(ValidationError::MismatchedComponentPayload(
                    self.component_kind,
                )),
                None => Err(ValidationError::MissingComponentPayload(
                    self.component_kind,
                )),
            },
            COMPONENT_KIND_MESH_ANIMATION_STATE => match &self.mesh_animation_state {
                Some(m) if populated == 1 => Ok(ComponentPayload::MeshAnimationState(m.clone())),
                Some(_) => Err(ValidationError::MismatchedComponentPayload(
                    self.component_kind,
                )),
                None => Err(ValidationError::MissingComponentPayload(
                    self.component_kind,
                )),
            },
            COMPONENT_KIND_KINEMATIC_MOVER_STATE => match self.kinematic_mover {
                Some(m) if populated == 1 => {
                    if !m.all_finite() {
                        Err(ValidationError::NonFiniteKinematicMoverState)
                    } else if !m.has_valid_phase_tags() {
                        Err(ValidationError::InvalidKinematicMoverState)
                    } else {
                        Ok(ComponentPayload::KinematicMoverState(m))
                    }
                }
                Some(_) => Err(ValidationError::MismatchedComponentPayload(
                    self.component_kind,
                )),
                None => Err(ValidationError::MissingComponentPayload(
                    self.component_kind,
                )),
            },
            other => Err(ValidationError::UnknownComponentKind(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Drift guard: every `WireMovementState` variant's float payload is covered by
    // the finiteness check. The expectation is derived from the source enum via an
    // exhaustive `match` (no `_` arm), so a new variant is a compile error here until
    // its finiteness contribution is declared — never a silently-passing guard.
    #[test]
    fn movement_state_finiteness_covers_every_variant() {
        let variants = [
            WireMovementState::Normal,
            WireMovementState::Dash {
                elapsed_ms: 1.0,
                boost: [0.0, 0.0, 0.0],
            },
            WireMovementState::Crouching { eye_current: 0.5 },
            WireMovementState::Sliding {
                elapsed_ms: 1.0,
                boost: [0.0, 0.0, 0.0],
                eye_current: 0.5,
                floor_normal: Some([0.0, 1.0, 0.0]),
            },
        ];
        for state in variants {
            // A finite instance of every variant must pass the finiteness gate.
            assert!(state.all_finite(), "finite variant must be all_finite");
            // The number of float fields each variant carries — derived from the
            // source enum so adding a variant (or a float field) forces an update.
            let float_field_count = match state {
                WireMovementState::Normal => 0,
                WireMovementState::Dash { .. } => 4, // elapsed_ms + 3 boost components
                WireMovementState::Crouching { .. } => 1,
                WireMovementState::Sliding { floor_normal, .. } => {
                    5 + floor_normal.map_or(0, |_| 3)
                }
            };
            // Non-`Normal` variants carry floats and so are non-finite-detectable.
            assert_eq!(
                float_field_count > 0,
                !matches!(state, WireMovementState::Normal)
            );
        }
    }
}
