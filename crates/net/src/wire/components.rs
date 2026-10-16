// Component wire mirrors and raw-payload validation.
// See: context/lib/networking.md

use bitcode::{Decode, Encode};

use super::ValidationError;

// Engine-aligned component discriminants. Unreplicated kinds still reserve their
// engine number here; this inventory is not a replication allow-list.
pub const COMPONENT_KIND_TRANSFORM: u16 = 0;
pub const COMPONENT_KIND_LIGHT: u16 = 1;
pub const COMPONENT_KIND_BILLBOARD_EMITTER: u16 = 2;
pub const COMPONENT_KIND_SPRITE_VISUAL: u16 = 3;
pub const COMPONENT_KIND_FOG_VOLUME: u16 = 4;
pub const COMPONENT_KIND_PLAYER_MOVEMENT_STATE: u16 = 5;
pub const COMPONENT_KIND_WEAPON: u16 = 6;
pub const COMPONENT_KIND_DESCRIPTOR_PROVENANCE: u16 = 7;
pub const COMPONENT_KIND_MESH_ANIMATION_STATE: u16 = 8;
pub const COMPONENT_KIND_HEALTH: u16 = 9;
pub const COMPONENT_KIND_AGENT: u16 = 10;
pub const COMPONENT_KIND_BRAIN: u16 = 11;
pub const COMPONENT_KIND_KINEMATIC_MOVER_STATE: u16 = 12;
pub const COMPONENT_KIND_TRIGGER_VOLUME: u16 = 13;
pub const COMPONENT_KIND_AMMO_RESERVE: u16 = 14;
pub const COMPONENT_KIND_SPAWNER: u16 = 15;
pub const COMPONENT_KIND_ENTITY_STATE: u16 = 16;
pub const COMPONENT_KIND_DEFERRED_EFFECT: u16 = 17;
pub const COMPONENT_KIND_INVENTORY: u16 = 18;
pub const COMPONENT_KIND_TOUCHABLE: u16 = 19;
pub const COMPONENT_KIND_PROJECTILE: u16 = 20;
pub const COMPONENT_KIND_HIT_VOLUME: u16 = 21;
pub const COMPONENT_KIND_MAP_ORDINAL: u16 = 22;
pub const COMPONENT_KIND_IMMERSION: u16 = 23;
pub const COMPONENT_KIND_TELEPORTER: u16 = 24;

/// Maximum key count in any replicated presentation animation channel.
pub const MAX_WIRE_CURVE_KEYS: usize = 64;

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
    /// Swimming in fluid `fluid` (an index into the host's fluid table, the
    /// table the client resolves against). Presentation names the edge sounds'
    /// fluid from it; the re-entry latch rides the outer state.
    Swim {
        fluid: u16,
    },
    /// Mantling onto a ledge. `target` (the landing feet position) and
    /// `start_offset` (entry feet minus `target`) are local to mover `mover`
    /// when it is set and world otherwise; replay resolves them through the
    /// tick's mover pose. The duration stays local tuning, and the exit
    /// direction derives from `start_offset`, so neither is carried.
    Vault {
        elapsed_ms: f32,
        target: [f32; 3],
        start_offset: [f32; 3],
        mover: Option<u32>,
        entry_speed: f32,
    },
}

/// Squared-length tolerance for collision-produced floor normals. Bitcode
/// preserves the source `f32` values exactly, so this only accommodates normal
/// floating-point normalization error; it is not a normalization step.
const SLIDING_FLOOR_NORMAL_LENGTH_SQUARED_TOLERANCE: f32 = 1.0e-3;

/// Shortest horizontal start offset a vault payload may carry, metres. The exit
/// direction normalizes it; a probe-built vault starts most of a metre short of
/// its target, so anything under a millimetre is malformed.
const VAULT_MIN_EXIT_OFFSET_M: f32 = 1.0e-3;

/// Longest start offset a vault payload may carry, metres. Far above any
/// authored ledge height plus probe reach, and small enough that replay's path
/// arithmetic cannot overflow.
const VAULT_MAX_START_OFFSET_M: f32 = 64.0;

/// Largest per-axis magnitude a vault `target` may carry, metres. No net-crate
/// world-coordinate bound exists, so this is a generous bound well past any
/// level extent that still keeps replay's path and frame rotation finite.
const VAULT_MAX_TARGET_COORD_M: f32 = 1.0e6;

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
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WirePlayerMovementState {
    pub teleport: super::WireTeleportState,
    pub velocity: [f32; 3],
    /// Protected impulse contribution already included in total velocity.
    pub knockback_velocity: [f32; 3],
    /// Protected push-drift contribution already included in total velocity.
    /// Its vertical part is non-zero only while swimming, where the layer
    /// carries the full push; outside `Swim` it is 0.
    pub push_velocity: [f32; 3],
    pub ground: WireGroundRef,
    pub air_jumps_remaining: u32,
    pub air_dashes_remaining: u32,
    pub dash_cooldown_ms: f32,
    pub air_ticks: u32,
    pub movement_state: WireMovementState,
    pub coyote_timer_ms: f32,
    pub jump_buffer_timer_ms: f32,
    pub jump_spent: bool,
    /// The fluid-jump re-entry latch: swim cannot re-enter until immersion
    /// first falls below the swimmer's exit depth.
    pub swim_reentry_latched: bool,
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
            WireMovementState::Normal | WireMovementState::Swim { .. } => true,
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
            WireMovementState::Vault {
                elapsed_ms,
                target,
                start_offset,
                mover: _,
                entry_speed,
            } => {
                elapsed_ms.is_finite()
                    && target.iter().all(|c| c.is_finite())
                    && start_offset.iter().all(|c| c.is_finite())
                    && entry_speed.is_finite()
            }
        }
    }

    /// Whether variant-specific movement invariants hold after decoding. Dash may
    /// preserve vertical boost. Slide boost is horizontal, and its optional floor
    /// normal must be safely bounded and near-unit before replay projects gravity
    /// against it. A vault's timer and speed are non-negative, its target and
    /// start offset are bounded so replay arithmetic stays finite, and its start
    /// offset has a horizontal component to derive the exit direction from.
    #[must_use]
    fn has_valid_state_contract(&self) -> bool {
        match self {
            WireMovementState::Normal
            | WireMovementState::Dash { .. }
            | WireMovementState::Crouching { .. }
            | WireMovementState::Swim { .. } => true,
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
            WireMovementState::Vault {
                elapsed_ms,
                target,
                start_offset,
                entry_speed,
                ..
            } => {
                // Components are finite here (`all_finite` runs first), so the
                // per-axis bound runs before any squaring in the length.
                *elapsed_ms >= 0.0
                    && *entry_speed >= 0.0
                    && target.iter().all(|c| c.abs() <= VAULT_MAX_TARGET_COORD_M)
                    && start_offset
                        .iter()
                        .all(|c| c.abs() <= VAULT_MAX_START_OFFSET_M)
                    && start_offset[0].hypot(start_offset[2]) >= VAULT_MIN_EXIT_OFFSET_M
                    && (start_offset[0] * start_offset[0]
                        + start_offset[1] * start_offset[1]
                        + start_offset[2] * start_offset[2])
                        <= VAULT_MAX_START_OFFSET_M * VAULT_MAX_START_OFFSET_M
            }
        }
    }
}

impl WirePlayerMovementState {
    /// Whether every replicated float is finite (no NaN/inf): velocity, protected
    /// knockback and push-drift velocity, all live timers, capsule dimensions, aim pitch, and the
    /// active state's payload. Checked at `validate` so a non-finite movement state is
    /// rejected before typed apply and never reaches the registry. Integer counters and
    /// bools cannot be non-finite, so they are not checked.
    #[must_use]
    fn all_finite(&self) -> bool {
        self.velocity.iter().all(|c| c.is_finite())
            && self.teleport.accumulated_yaw.is_finite()
            && self.knockback_velocity.iter().all(|c| c.is_finite())
            && self.push_velocity.iter().all(|c| c.is_finite())
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

/// Written light state, independent of per-frame bridge samples.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WireLightState {
    pub intensity: f32,
    pub color: [f32; 3],
    pub cone_direction: Option<[f32; 3]>,
    pub animation: Option<WireLightAnimation>,
    pub anim_start: Option<f64>,
}

/// Light animation channels supported by the scripting surface. Radius curves
/// are engine-internal and deliberately have no wire field.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WireLightAnimation {
    pub period_ms: f32,
    pub phase: Option<f32>,
    pub play_count: Option<u32>,
    pub start_active: Option<bool>,
    pub brightness: Option<Vec<f32>>,
    pub color: Option<Vec<[f32; 3]>>,
    pub direction: Option<Vec<[f32; 3]>>,
}

/// Written fog state, independent of per-frame bridge samples.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WireFogState {
    pub density: f32,
    pub glow: f32,
    pub edge_softness: f32,
    pub falloff: f32,
    pub tint: [f32; 3],
    pub saturation: f32,
    pub min_brightness: f32,
    pub light_range: f32,
    pub animation: Option<WireFogAnimation>,
    pub anim_start: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WireFogAnimation {
    pub period_ms: f32,
    pub phase: Option<f32>,
    pub play_count: Option<u32>,
    pub density: Option<Vec<f32>>,
    pub saturation: Option<Vec<f32>>,
    pub min_brightness: Option<Vec<f32>>,
    pub light_range: Option<Vec<f32>>,
}

/// Written emitter state; elapsed animation time stays off the wire.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WireEmitterState {
    pub rate: f32,
    pub spin_rate: f32,
    pub spin_animation: Option<WireSpinAnimation>,
    pub anim_start: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WireSpinAnimation {
    pub duration: f32,
    pub rate_curve: Vec<f32>,
}

fn curve_is_finite<T>(curve: &Option<Vec<T>>, finite: impl Fn(&T) -> bool) -> bool {
    curve.as_ref().is_none_or(|keys| keys.iter().all(finite))
}

fn curve_has_valid_length<T>(curve: &Option<Vec<T>>) -> bool {
    curve
        .as_ref()
        .is_none_or(|keys| (1..=MAX_WIRE_CURVE_KEYS).contains(&keys.len()))
}

fn vector_is_finite(vector: &[f32; 3]) -> bool {
    vector.iter().all(|value| value.is_finite())
}

impl WireLightAnimation {
    fn all_finite(&self) -> bool {
        self.period_ms.is_finite()
            && self.phase.is_none_or(f32::is_finite)
            && curve_is_finite(&self.brightness, |value| value.is_finite())
            && curve_is_finite(&self.color, vector_is_finite)
            && curve_is_finite(&self.direction, vector_is_finite)
    }

    fn has_valid_bounds(&self) -> bool {
        self.period_ms > 0.0
            && curve_has_valid_length(&self.brightness)
            && curve_has_valid_length(&self.color)
            && curve_has_valid_length(&self.direction)
    }
}

impl WireFogAnimation {
    fn all_finite(&self) -> bool {
        self.period_ms.is_finite()
            && self.phase.is_none_or(f32::is_finite)
            && [
                &self.density,
                &self.saturation,
                &self.min_brightness,
                &self.light_range,
            ]
            .iter()
            .all(|curve| curve_is_finite(curve, |value| value.is_finite()))
    }

    fn has_valid_bounds(&self) -> bool {
        // Fog's CPU evaluator requires positive finite counts; light uses zero to loop.
        self.period_ms > 0.0
            && self.play_count != Some(0)
            && [
                &self.density,
                &self.saturation,
                &self.min_brightness,
                &self.light_range,
            ]
            .iter()
            .all(|curve| curve_has_valid_length(curve))
    }
}

impl WireSpinAnimation {
    fn all_finite(&self) -> bool {
        self.duration.is_finite() && self.rate_curve.iter().all(|value| value.is_finite())
    }

    fn has_valid_bounds(&self) -> bool {
        self.duration > 0.0 && (1..=MAX_WIRE_CURVE_KEYS).contains(&self.rate_curve.len())
    }
}

impl WireLightState {
    /// Shared receive and producer/capture gate for presentation state.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !(self.intensity.is_finite()
            && vector_is_finite(&self.color)
            && self.cone_direction.as_ref().is_none_or(vector_is_finite)
            && self.anim_start.is_none_or(f64::is_finite)
            && self
                .animation
                .as_ref()
                .is_none_or(WireLightAnimation::all_finite))
        {
            return Err(ValidationError::NonFinitePresentationState(
                COMPONENT_KIND_LIGHT,
            ));
        }
        if !self
            .animation
            .as_ref()
            .is_none_or(WireLightAnimation::has_valid_bounds)
        {
            return Err(ValidationError::InvalidPresentationState(
                COMPONENT_KIND_LIGHT,
            ));
        }
        Ok(())
    }
}

impl WireFogState {
    /// Shared receive and producer/capture gate for presentation state.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !([
            self.density,
            self.glow,
            self.edge_softness,
            self.falloff,
            self.saturation,
            self.min_brightness,
            self.light_range,
        ]
        .iter()
        .all(|value| value.is_finite())
            && vector_is_finite(&self.tint)
            && self.anim_start.is_none_or(f64::is_finite)
            && self
                .animation
                .as_ref()
                .is_none_or(WireFogAnimation::all_finite))
        {
            return Err(ValidationError::NonFinitePresentationState(
                COMPONENT_KIND_FOG_VOLUME,
            ));
        }
        if !self
            .animation
            .as_ref()
            .is_none_or(WireFogAnimation::has_valid_bounds)
        {
            return Err(ValidationError::InvalidPresentationState(
                COMPONENT_KIND_FOG_VOLUME,
            ));
        }
        Ok(())
    }
}

impl WireEmitterState {
    /// Shared receive and producer/capture gate for presentation state.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !(self.rate.is_finite()
            && self.spin_rate.is_finite()
            && self.anim_start.is_none_or(f64::is_finite)
            && self
                .spin_animation
                .as_ref()
                .is_none_or(WireSpinAnimation::all_finite))
        {
            return Err(ValidationError::NonFinitePresentationState(
                COMPONENT_KIND_BILLBOARD_EMITTER,
            ));
        }
        if !self
            .spin_animation
            .as_ref()
            .is_none_or(WireSpinAnimation::has_valid_bounds)
        {
            return Err(ValidationError::InvalidPresentationState(
                COMPONENT_KIND_BILLBOARD_EMITTER,
            ));
        }
        Ok(())
    }
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
    pub teleporter: Option<super::WireTeleporterState>,
    pub component_kind: u16,
    pub transform: Option<WireTransform>,
    pub player_movement: Option<WirePlayerMovementState>,
    pub mesh_animation_state: Option<WireMeshAnimationState>,
    pub kinematic_mover: Option<WireKinematicMoverState>,
    pub light: Option<WireLightState>,
    pub emitter: Option<WireEmitterState>,
    pub fog: Option<WireFogState>,
}

/// A validated component payload. Constructed only by [`RawComponentPayload::validate`],
/// so a typed payload always has exactly the inner value its kind requires.
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentPayload {
    TeleporterState(super::WireTeleporterState),
    Transform(WireTransform),
    PlayerMovementState(WirePlayerMovementState),
    MeshAnimationState(WireMeshAnimationState),
    KinematicMoverState(WireKinematicMoverState),
    LightState(WireLightState),
    EmitterState(WireEmitterState),
    FogState(WireFogState),
}

impl ComponentPayload {
    /// Engine-aligned `u16` discriminant for this payload, numeric-equal to
    /// `ComponentKind as u16` in the engine. Drift here desyncs replication, so
    /// the mapping is pinned by `component_kind_pinned_to_engine_discriminants`.
    #[must_use]
    pub fn kind(&self) -> u16 {
        match self {
            ComponentPayload::TeleporterState(_) => COMPONENT_KIND_TELEPORTER,
            ComponentPayload::Transform(_) => COMPONENT_KIND_TRANSFORM,
            ComponentPayload::PlayerMovementState(_) => COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            ComponentPayload::MeshAnimationState(_) => COMPONENT_KIND_MESH_ANIMATION_STATE,
            ComponentPayload::KinematicMoverState(_) => COMPONENT_KIND_KINEMATIC_MOVER_STATE,
            ComponentPayload::LightState(_) => COMPONENT_KIND_LIGHT,
            ComponentPayload::EmitterState(_) => COMPONENT_KIND_BILLBOARD_EMITTER,
            ComponentPayload::FogState(_) => COMPONENT_KIND_FOG_VOLUME,
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
            + usize::from(self.teleporter.is_some())
            + usize::from(self.player_movement.is_some())
            + usize::from(self.mesh_animation_state.is_some())
            + usize::from(self.kinematic_mover.is_some())
            + usize::from(self.light.is_some())
            + usize::from(self.emitter.is_some())
            + usize::from(self.fog.is_some());

        match self.component_kind {
            COMPONENT_KIND_TELEPORTER => match self.teleporter {
                Some(state) if populated == 1 && state.index != super::NO_WIRE_TELEPORTER => Ok(ComponentPayload::TeleporterState(state)),
                Some(_) => Err(ValidationError::MismatchedComponentPayload(self.component_kind)),
                None => Err(ValidationError::MissingComponentPayload(self.component_kind)),
            },
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
            COMPONENT_KIND_PLAYER_MOVEMENT_STATE => match &self.player_movement {
                Some(m) if populated == 1 => {
                    if !m.all_finite() {
                        Err(ValidationError::NonFiniteMovementState)
                    } else if !m.movement_state.has_valid_state_contract() || !m.teleport.is_valid() {
                        Err(ValidationError::InvalidMovementState)
                    } else {
                        Ok(ComponentPayload::PlayerMovementState(m.clone()))
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
            COMPONENT_KIND_LIGHT => match &self.light {
                Some(state) if populated == 1 => {
                    state.validate()?;
                    Ok(ComponentPayload::LightState(state.clone()))
                }
                Some(_) => Err(ValidationError::MismatchedComponentPayload(
                    self.component_kind,
                )),
                None => Err(ValidationError::MissingComponentPayload(
                    self.component_kind,
                )),
            },
            COMPONENT_KIND_BILLBOARD_EMITTER => match &self.emitter {
                Some(state) if populated == 1 => {
                    state.validate()?;
                    Ok(ComponentPayload::EmitterState(state.clone()))
                }
                Some(_) => Err(ValidationError::MismatchedComponentPayload(
                    self.component_kind,
                )),
                None => Err(ValidationError::MissingComponentPayload(
                    self.component_kind,
                )),
            },
            COMPONENT_KIND_FOG_VOLUME => match &self.fog {
                Some(state) if populated == 1 => {
                    state.validate()?;
                    Ok(ComponentPayload::FogState(state.clone()))
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
            WireMovementState::Swim { fluid: 3 },
            WireMovementState::Vault {
                elapsed_ms: 16.0,
                target: [5.4, 1.2, 0.0],
                start_offset: [-0.8, -0.7, 0.0],
                mover: Some(77),
                entry_speed: 7.0,
            },
        ];
        for state in variants {
            // A finite instance of every variant must pass the finiteness gate.
            assert!(state.all_finite(), "finite variant must be all_finite");
            // The number of float fields each variant carries — derived from the
            // source enum so adding a variant (or a float field) forces an update.
            let float_field_count = match state {
                WireMovementState::Normal | WireMovementState::Swim { .. } => 0,
                WireMovementState::Dash { .. } => 4, // elapsed_ms + 3 boost components
                WireMovementState::Crouching { .. } => 1,
                WireMovementState::Sliding { floor_normal, .. } => {
                    5 + floor_normal.map_or(0, |_| 3)
                }
                // elapsed_ms + 3 target + 3 start_offset + entry_speed
                WireMovementState::Vault { .. } => 8,
            };
            // Every variant but the payload-free `Normal` and `Swim` carries
            // floats and so is non-finite-detectable.
            assert_eq!(
                float_field_count > 0,
                !matches!(
                    state,
                    WireMovementState::Normal | WireMovementState::Swim { .. }
                )
            );
        }
    }
    fn empty_raw(kind: u16) -> RawComponentPayload {
        RawComponentPayload {
            teleporter: None,
            component_kind: kind,
            transform: None,
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
        }
    }

    fn light_state() -> WireLightState {
        WireLightState {
            intensity: 2.0,
            color: [0.2, 0.4, 0.6],
            cone_direction: Some([0.0, -1.0, 0.0]),
            animation: Some(WireLightAnimation {
                period_ms: 800.0,
                phase: Some(0.25),
                play_count: Some(2),
                start_active: Some(true),
                brightness: Some(vec![1.0; MAX_WIRE_CURVE_KEYS]),
                color: Some(vec![[0.3, 0.5, 0.7]; MAX_WIRE_CURVE_KEYS]),
                direction: Some(vec![[0.0, -1.0, 0.0]; MAX_WIRE_CURVE_KEYS]),
            }),
            anim_start: Some(4.5),
        }
    }

    fn fog_state() -> WireFogState {
        WireFogState {
            density: 0.1,
            glow: 0.2,
            edge_softness: 0.3,
            falloff: 0.4,
            tint: [0.5, 0.6, 0.7],
            saturation: 1.2,
            min_brightness: 0.8,
            light_range: 12.0,
            animation: Some(WireFogAnimation {
                period_ms: 1200.0,
                phase: Some(0.5),
                play_count: Some(3),
                density: Some(vec![0.1; MAX_WIRE_CURVE_KEYS]),
                saturation: Some(vec![1.2; MAX_WIRE_CURVE_KEYS]),
                min_brightness: Some(vec![0.8; MAX_WIRE_CURVE_KEYS]),
                light_range: Some(vec![12.0; MAX_WIRE_CURVE_KEYS]),
            }),
            anim_start: Some(6.0),
        }
    }

    fn emitter_state() -> WireEmitterState {
        WireEmitterState {
            rate: 15.0,
            spin_rate: -1.5,
            spin_animation: Some(WireSpinAnimation {
                duration: 2.0,
                rate_curve: vec![3.0; MAX_WIRE_CURVE_KEYS],
            }),
            anim_start: Some(7.0),
        }
    }

    fn roundtrip(raw: RawComponentPayload, expected: ComponentPayload) {
        let bytes = bitcode::encode(&raw);
        let decoded: RawComponentPayload = bitcode::decode(&bytes).unwrap();
        assert_eq!(decoded, raw);
        assert_eq!(decoded.validate(), Ok(expected));
    }

    #[test]
    fn light_state_roundtrip_none_and_maximum_curves() {
        for animated in [false, true] {
            let mut state = light_state();
            if !animated {
                state.animation = None;
                state.anim_start = None;
                state.cone_direction = None;
            }
            let mut raw = empty_raw(COMPONENT_KIND_LIGHT);
            raw.light = Some(state.clone());
            roundtrip(raw, ComponentPayload::LightState(state));
        }
    }

    #[test]
    fn fog_state_roundtrip_none_and_maximum_curves() {
        for animated in [false, true] {
            let mut state = fog_state();
            if !animated {
                state.animation = None;
                state.anim_start = None;
            }
            let mut raw = empty_raw(COMPONENT_KIND_FOG_VOLUME);
            raw.fog = Some(state.clone());
            roundtrip(raw, ComponentPayload::FogState(state));
        }
    }

    #[test]
    fn emitter_state_roundtrip_none_and_maximum_curves() {
        for animated in [false, true] {
            let mut state = emitter_state();
            if !animated {
                state.spin_animation = None;
                state.anim_start = None;
            }
            let mut raw = empty_raw(COMPONENT_KIND_BILLBOARD_EMITTER);
            raw.emitter = Some(state.clone());
            roundtrip(raw, ComponentPayload::EmitterState(state));
        }
    }

    #[test]
    fn light_state_rejects_nonfinite_fields_and_invalid_curve_bounds() {
        let mutations: &[fn(&mut WireLightState, f32)] = &[
            |s, v| s.intensity = v,
            |s, v| s.color[1] = v,
            |s, v| s.cone_direction.as_mut().unwrap()[2] = v,
            |s, v| s.anim_start = Some(f64::from(v)),
            |s, v| s.animation.as_mut().unwrap().period_ms = v,
            |s, v| s.animation.as_mut().unwrap().phase = Some(v),
            |s, v| s.animation.as_mut().unwrap().brightness.as_mut().unwrap()[0] = v,
            |s, v| s.animation.as_mut().unwrap().color.as_mut().unwrap()[0][2] = v,
            |s, v| s.animation.as_mut().unwrap().direction.as_mut().unwrap()[0][1] = v,
        ];
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for mutate in mutations {
                let mut state = light_state();
                mutate(&mut state, value);
                let mut raw = empty_raw(COMPONENT_KIND_LIGHT);
                raw.light = Some(state);
                assert_eq!(
                    raw.validate(),
                    Err(ValidationError::NonFinitePresentationState(
                        COMPONENT_KIND_LIGHT
                    ))
                );
            }
        }
        for period in [0.0, -1.0] {
            let mut state = light_state();
            state.animation.as_mut().unwrap().period_ms = period;
            assert_eq!(
                state.validate(),
                Err(ValidationError::InvalidPresentationState(
                    COMPONENT_KIND_LIGHT
                ))
            );
        }
        for length in [0, MAX_WIRE_CURVE_KEYS + 1] {
            for channel in 0..3 {
                let mut state = light_state();
                let animation = state.animation.as_mut().unwrap();
                match channel {
                    0 => animation.brightness = Some(vec![1.0; length]),
                    1 => animation.color = Some(vec![[1.0; 3]; length]),
                    2 => animation.direction = Some(vec![[1.0; 3]; length]),
                    _ => unreachable!(),
                }
                assert_eq!(
                    state.validate(),
                    Err(ValidationError::InvalidPresentationState(
                        COMPONENT_KIND_LIGHT
                    ))
                );
            }
        }
        let mut state = light_state();
        let animation = state.animation.as_mut().unwrap();
        animation.brightness = None;
        animation.color = None;
        animation.direction = None;
        assert_eq!(state.validate(), Ok(()), "absent channels carry no curve");
    }

    #[test]
    fn fog_state_rejects_nonfinite_fields_and_invalid_curve_bounds() {
        let mutations: &[fn(&mut WireFogState, f32)] = &[
            |s, v| s.density = v,
            |s, v| s.glow = v,
            |s, v| s.edge_softness = v,
            |s, v| s.falloff = v,
            |s, v| s.tint[2] = v,
            |s, v| s.saturation = v,
            |s, v| s.min_brightness = v,
            |s, v| s.light_range = v,
            |s, v| s.anim_start = Some(f64::from(v)),
            |s, v| s.animation.as_mut().unwrap().period_ms = v,
            |s, v| s.animation.as_mut().unwrap().phase = Some(v),
            |s, v| s.animation.as_mut().unwrap().density.as_mut().unwrap()[0] = v,
            |s, v| s.animation.as_mut().unwrap().saturation.as_mut().unwrap()[0] = v,
            |s, v| {
                s.animation
                    .as_mut()
                    .unwrap()
                    .min_brightness
                    .as_mut()
                    .unwrap()[0] = v
            },
            |s, v| s.animation.as_mut().unwrap().light_range.as_mut().unwrap()[0] = v,
        ];
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for mutate in mutations {
                let mut state = fog_state();
                mutate(&mut state, value);
                let mut raw = empty_raw(COMPONENT_KIND_FOG_VOLUME);
                raw.fog = Some(state);
                assert_eq!(
                    raw.validate(),
                    Err(ValidationError::NonFinitePresentationState(
                        COMPONENT_KIND_FOG_VOLUME
                    ))
                );
            }
        }
        for period in [0.0, -1.0] {
            let mut state = fog_state();
            state.animation.as_mut().unwrap().period_ms = period;
            assert_eq!(
                state.validate(),
                Err(ValidationError::InvalidPresentationState(
                    COMPONENT_KIND_FOG_VOLUME
                ))
            );
        }
        for length in [0, MAX_WIRE_CURVE_KEYS + 1] {
            for channel in 0..4 {
                let mut state = fog_state();
                let animation = state.animation.as_mut().unwrap();
                let curves = [
                    &mut animation.density,
                    &mut animation.saturation,
                    &mut animation.min_brightness,
                    &mut animation.light_range,
                ];
                *curves.into_iter().nth(channel).unwrap() = Some(vec![1.0; length]);
                assert_eq!(
                    state.validate(),
                    Err(ValidationError::InvalidPresentationState(
                        COMPONENT_KIND_FOG_VOLUME
                    ))
                );
            }
        }
        // Regression: zero fog play counts passed receive validation and panicked at sampling.
        for play_count in [Some(0), Some(1), None] {
            let mut state = fog_state();
            state.animation.as_mut().unwrap().play_count = play_count;
            let mut raw = empty_raw(COMPONENT_KIND_FOG_VOLUME);
            raw.fog = Some(state.clone());
            if play_count == Some(0) {
                assert_eq!(
                    raw.validate(),
                    Err(ValidationError::InvalidPresentationState(
                        COMPONENT_KIND_FOG_VOLUME
                    ))
                );
            } else {
                roundtrip(raw, ComponentPayload::FogState(state));
            }
        }
        let mut light = light_state();
        light.animation.as_mut().unwrap().play_count = Some(0);
        let mut raw = empty_raw(COMPONENT_KIND_LIGHT);
        raw.light = Some(light.clone());
        roundtrip(raw, ComponentPayload::LightState(light));

        let mut state = fog_state();
        let animation = state.animation.as_mut().unwrap();
        animation.phase = None;
        animation.play_count = None;
        animation.density = None;
        animation.saturation = None;
        animation.min_brightness = None;
        animation.light_range = None;
        assert_eq!(state.validate(), Ok(()));
    }

    #[test]
    fn emitter_state_rejects_nonfinite_fields_and_invalid_curve_bounds() {
        let mutations: &[fn(&mut WireEmitterState, f32)] = &[
            |s, v| s.rate = v,
            |s, v| s.spin_rate = v,
            |s, v| s.anim_start = Some(f64::from(v)),
            |s, v| s.spin_animation.as_mut().unwrap().duration = v,
            |s, v| s.spin_animation.as_mut().unwrap().rate_curve[0] = v,
        ];
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for mutate in mutations {
                let mut state = emitter_state();
                mutate(&mut state, value);
                let mut raw = empty_raw(COMPONENT_KIND_BILLBOARD_EMITTER);
                raw.emitter = Some(state);
                assert_eq!(
                    raw.validate(),
                    Err(ValidationError::NonFinitePresentationState(
                        COMPONENT_KIND_BILLBOARD_EMITTER
                    ))
                );
            }
        }
        for duration in [0.0, -1.0] {
            let mut state = emitter_state();
            state.spin_animation.as_mut().unwrap().duration = duration;
            assert_eq!(
                state.validate(),
                Err(ValidationError::InvalidPresentationState(
                    COMPONENT_KIND_BILLBOARD_EMITTER
                ))
            );
        }
        for length in [0, MAX_WIRE_CURVE_KEYS + 1] {
            let mut state = emitter_state();
            state.spin_animation.as_mut().unwrap().rate_curve = vec![1.0; length];
            assert_eq!(
                state.validate(),
                Err(ValidationError::InvalidPresentationState(
                    COMPONENT_KIND_BILLBOARD_EMITTER
                ))
            );
        }
    }

    #[test]
    fn presentation_state_payload_slots_require_exact_kind_and_single_slot() {
        for (kind, raw) in [
            (
                COMPONENT_KIND_LIGHT,
                RawComponentPayload {
                    teleporter: None,
                    light: Some(light_state()),
                    ..empty_raw(COMPONENT_KIND_LIGHT)
                },
            ),
            (
                COMPONENT_KIND_BILLBOARD_EMITTER,
                RawComponentPayload {
                    teleporter: None,
                    emitter: Some(emitter_state()),
                    ..empty_raw(COMPONENT_KIND_BILLBOARD_EMITTER)
                },
            ),
            (
                COMPONENT_KIND_FOG_VOLUME,
                RawComponentPayload {
                    teleporter: None,
                    fog: Some(fog_state()),
                    ..empty_raw(COMPONENT_KIND_FOG_VOLUME)
                },
            ),
        ] {
            assert_eq!(raw.validate().unwrap().kind(), kind);
            assert_eq!(
                empty_raw(kind).validate(),
                Err(ValidationError::MissingComponentPayload(kind))
            );
            let mut duplicate = raw.clone();
            duplicate.transform = Some(WireTransform {
                position: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            });
            assert_eq!(
                duplicate.validate(),
                Err(ValidationError::MismatchedComponentPayload(kind))
            );
            let mut wrong_kind = raw;
            wrong_kind.component_kind = COMPONENT_KIND_TRANSFORM;
            assert_eq!(
                wrong_kind.validate(),
                Err(ValidationError::MissingComponentPayload(
                    COMPONENT_KIND_TRANSFORM
                ))
            );
        }
    }
}
