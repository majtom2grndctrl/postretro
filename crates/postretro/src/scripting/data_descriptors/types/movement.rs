// Data-context descriptors: player-movement descriptor params.
// See: context/lib/scripting.md

use super::super::*;

/// Authored player-movement component preset. The four core sub-objects
/// (`capsule`, `ground`, `air`, `fall`) are required when `movement` is
/// present; `dash` is optional — its absence disables dash entirely; `crouch`
/// is optional — its absence disables crouch entirely. The data-archetype
/// spawn path materializes the runtime movement component from this.
/// `ground.max_slope` is in degrees on the wire and converted to a cosine at
/// materialization (not here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlayerMovementDescriptor {
    pub(crate) capsule: CapsuleParams,
    pub(crate) ground: GroundParams,
    pub(crate) air: AirParams,
    pub(crate) fall: FallParams,
    /// Stuck-stop deadzone enable flag. See `PlayerMovementComponent` for
    /// trigger semantics. Defaults to `true`; the JS/Luau parsers fall back
    /// to this default when the field is omitted, keeping the deadzone
    /// opt-out (not opt-in) for existing authored descriptors.
    pub(crate) stuck_stop_enabled: bool,
    /// Horizontal-displacement threshold (metres) gating the deadzone. See
    /// `PlayerMovementComponent` for tuning rationale. Defaults to `1.0e-3`.
    pub(crate) stuck_stop_threshold: f32,
    /// Optional dash tuning. Absent ⇒ dash disabled (no `DashParams`
    /// materialized). When present, all of its fields are required, matching
    /// the present-then-all-required discipline of `ground`/`air`/`fall`.
    pub(crate) dash: Option<DashParams>,
    /// Optional input-forgiveness tuning (coyote time + jump buffer). Absent ⇒
    /// the documented engine defaults apply (both ~100 ms). Per-field zero
    /// disables that grace independently. See `ForgivenessParams`.
    pub(crate) forgiveness: Option<ForgivenessParams>,
    /// Optional crouch tuning. Absent ⇒ crouch disabled (no `CrouchParams`
    /// materialized). When present, all of its fields are required, matching
    /// the present-then-all-required discipline of `dash`.
    pub(crate) crouch: Option<CrouchParams>,
    /// Optional first-person view-feel tuning (head bob, strafe tilt, ambient
    /// sway). Absent ⇒ view feel disabled (no `ViewFeelParams` materialized).
    /// A render-only camera effect — see `ViewFeelParams`.
    pub(crate) view_feel: Option<ViewFeelParams>,
}

impl PlayerMovementDescriptor {
    /// Default values for the stuck-stop fields. Used by the JS/Luau parsers
    /// when the descriptor omits them so existing scripts keep working.
    pub(crate) const DEFAULT_STUCK_STOP_ENABLED: bool = true;
    pub(crate) const DEFAULT_STUCK_STOP_THRESHOLD: f32 = 1.0e-3;
}

/// Input-forgiveness tuning: coyote time (a grounded jump permitted for a window
/// after leaving a ledge) and jump buffering (a jump pressed shortly before
/// landing fires on the landing tick). Both windows are in MILLISECONDS,
/// advanced off the same `dt` the movement tick already accumulates
/// (`dt * 1000.0` per tick), mirroring the dash cooldown's ms accounting.
///
/// This sub-object is OPTIONAL on [`PlayerMovementDescriptor`]: when the whole
/// `forgiveness` object is absent, the documented engine defaults apply (see the
/// `DEFAULT_*` constants). When the object is present, each wire field is
/// individually optional — `forgiveness_params_from_js` / `forgiveness_params_from_lua`
/// substitute per-field defaults for any omitted key; the resulting Rust struct
/// always carries concrete `f32` values (`coyote_ms == 0.0` means explicitly
/// zeroed, not absent). An explicit `0` disables that grace independently (the
/// regression fixtures pin both to zero to preserve exact edge timing). Field
/// names are camelCase on the wire (`coyoteMs`, `jumpBufferMs`) and snake_case
/// in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct ForgivenessParams {
    /// Coyote-time window in milliseconds: a grounded jump is permitted for this
    /// long after ground is lost (with no prior jump). `0` disables coyote time.
    pub(crate) coyote_ms: f32,
    /// Jump-buffer window in milliseconds: a jump pressed while airborne is
    /// retained for this long and fires on the landing tick. `0` disables jump
    /// buffering.
    pub(crate) jump_buffer_ms: f32,
}

impl ForgivenessParams {
    /// Feel-friendly engine default for the coyote-time window (milliseconds),
    /// applied when the `forgiveness` sub-object or the `coyoteMs` field is
    /// absent. ~100 ms ≈ 6 ticks at 60 Hz — a forgiving-but-tight grace.
    pub(crate) const DEFAULT_COYOTE_MS: f32 = 100.0;
    /// Feel-friendly engine default for the jump-buffer window (milliseconds),
    /// applied when the `forgiveness` sub-object or the `jumpBufferMs` field is
    /// absent. ~100 ms ≈ 6 ticks at 60 Hz.
    pub(crate) const DEFAULT_JUMP_BUFFER_MS: f32 = 100.0;

    /// The defaults materialized as a struct, used when the whole `forgiveness`
    /// sub-object is omitted.
    pub(crate) const DEFAULT: ForgivenessParams = ForgivenessParams {
        coyote_ms: Self::DEFAULT_COYOTE_MS,
        jump_buffer_ms: Self::DEFAULT_JUMP_BUFFER_MS,
    };
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CapsuleParams {
    pub(crate) radius: f32,
    pub(crate) half_height: f32,
    /// Camera attachment point measured upward from the capsule center, in
    /// meters. The camera-follow path adds this to the pawn's position each
    /// tick to derive eye position. Must lie in `(0, half_height + radius]` —
    /// the upper bound is the top of the capsule.
    pub(crate) eye_height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GroundParams {
    pub(crate) speed: SpeedParams,
    pub(crate) accel: f32,
    pub(crate) step_height: f32,
    pub(crate) max_slope: f32,
}

/// Walk/run/crouch ground speeds. The movement tick selects `run` while the
/// sprint input is held, `crouch` while crouched, and `walk` otherwise; the
/// chosen value is the omnidirectional horizontal speed target (and airborne
/// speed cap), not a forward-only bonus. All three fields are required when
/// `ground` is present and validated non-negative finite. `crouch` is in
/// world-units/sec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SpeedParams {
    pub(crate) walk: f32,
    pub(crate) run: f32,
    pub(crate) crouch: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AirParams {
    pub(crate) forward_steer: f32,
    pub(crate) accel: f32,
    pub(crate) max_control_speed: f32,
    pub(crate) bunny_hop: bool,
    pub(crate) jumps: u32,
    pub(crate) jump_velocity: f32,
    pub(crate) jump_ceiling: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FallParams {
    pub(crate) terminal_velocity: f32,
}

/// A dash numeric field: either a bare literal or an engine-evaluated IR
/// expression over the movement-local input namespace ([`MovementScope`]).
///
/// `#[serde(untagged)]` mirrors the substrate's untagged-`IrValue` precedent: a
/// bare JSON number deserializes to [`NumberOrIr::Literal`], an op-tagged object
/// (`{"op": …}`) to [`NumberOrIr::Ir`]. The two are disjoint on the wire — a
/// number never matches the `Ir` (object) variant and an object never matches
/// the `f32` variant — so variant ordering is not load-bearing.
///
/// The hand-written JS/Luau parsers route values explicitly (object → IR,
/// scalar → literal) so the literal path keeps its range validators; the derived
/// serde impls exist so `DashParams` round-trips through `PlayerMovementComponent`'s
/// `Serialize`/`Deserialize`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum NumberOrIr {
    Literal(f32),
    Ir(IrNode),
}

/// A dash boolean field: a bare literal or an engine-evaluated IR expression.
/// The boolean analogue of [`NumberOrIr`]; see its docs for the untagged-serde
/// rationale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum BoolOrIr {
    Literal(bool),
    Ir(IrNode),
}

impl NumberOrIr {
    /// The literal value when this field is a bare scalar, else `None`.
    /// Tests use this to assert expression fields round-trip as `None`.
    pub(crate) fn literal(&self) -> Option<f32> {
        match self {
            NumberOrIr::Literal(v) => Some(*v),
            NumberOrIr::Ir(_) => None,
        }
    }
}

impl BoolOrIr {
    /// The literal value when this field is a bare boolean, else `None`. See
    /// [`NumberOrIr::literal`].
    pub(crate) fn literal(&self) -> Option<bool> {
        match self {
            BoolOrIr::Literal(v) => Some(*v),
            BoolOrIr::Ir(_) => None,
        }
    }
}

impl From<f32> for NumberOrIr {
    fn from(v: f32) -> Self {
        NumberOrIr::Literal(v)
    }
}

impl From<bool> for BoolOrIr {
    fn from(v: bool) -> Self {
        BoolOrIr::Literal(v)
    }
}

impl PartialEq<f32> for NumberOrIr {
    fn eq(&self, other: &f32) -> bool {
        matches!(self, NumberOrIr::Literal(v) if v == other)
    }
}

impl PartialEq<bool> for BoolOrIr {
    fn eq(&self, other: &bool) -> bool {
        matches!(self, BoolOrIr::Literal(v) if v == other)
    }
}

/// Dash tuning. Optional on [`PlayerMovementDescriptor`] (absent disables
/// dash); when present, all fields are required and validated. Field names are
/// camelCase on the wire (`boostSpeed`, `momentumRetention`, …) and snake_case
/// in Rust. Stored later by `PlayerMovementComponent` as `Option<DashParams>`.
///
/// Each value field accepts a bare literal OR an engine-evaluated expression
/// ([`NumberOrIr`] / [`BoolOrIr`]) over the movement-local input namespace; the
/// evaluation moment is engine-pinned per field (see `movement.md` §2). The
/// hand-written parsers validate an expression at declaration: a literal keeps
/// its existing range check; an expression is bound against
/// [`MovementScope::for_validation`] and its root type checked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DashParams {
    /// Impulse magnitude applied on dash, world-units/sec (entry-moment). A
    /// literal must be finite > 0.
    pub(crate) boost_speed: NumberOrIr,
    /// Fraction of pre-dash momentum folded into the dash, unitless `[0, 1]`
    /// (entry-moment).
    pub(crate) momentum_retention: NumberOrIr,
    /// In-dash steering authority, unitless `[0, 1]` (per-tick).
    pub(crate) steer_control: NumberOrIr,
    /// Decay rate of the dash impulse, world-units/sec² (per-tick). A literal
    /// `0` is legitimate.
    pub(crate) dash_drag: NumberOrIr,
    /// Cooldown between dashes in milliseconds (entry-moment). A literal `0` is
    /// legitimate.
    pub(crate) cooldown_ms: NumberOrIr,
    /// Number of air dashes allowed before landing. Stays a plain integer (no
    /// expression form).
    pub(crate) air_dashes: u32,
    /// Whether the dash preserves the pre-dash vertical velocity (entry-moment).
    pub(crate) preserve_vertical: BoolOrIr,
}

/// Crouch tuning. Optional on [`PlayerMovementDescriptor`] (absent disables
/// crouch); when present, all fields are required and validated. Field names are
/// camelCase on the wire (`halfHeight`, `eyeHeight`, `transitionRate`) and
/// snake_case in Rust. Stored later by `PlayerMovementComponent` as
/// `Option<CrouchParams>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CrouchParams {
    /// Crouched capsule half-height, in metres. Must be finite > 0.
    pub(crate) half_height: f32,
    /// Crouched camera attachment point measured upward from the capsule
    /// center, in metres. Must lie in `(0, crouched half_height + radius]` —
    /// the upper bound is the top of the crouched capsule.
    pub(crate) eye_height: f32,
    /// Rate at which the capsule interpolates between standing and crouched
    /// extents, per-sec. Must be finite > 0.
    pub(crate) transition_rate: f32,
}

/// First-person view-feel tuning: a render-only camera effect bundle (head bob,
/// strafe tilt, ambient sway). OPTIONAL on [`PlayerMovementDescriptor`] — absent
/// disables view feel entirely (no `ViewFeelParams` materialized). When present,
/// each of `bob`/`tilt`/`sway` is independently optional; an absent sub-object
/// disables that motion. Within a present sub-object, all tuning fields are
/// required EXCEPT the optional `groundedOnly` gate. This two-level
/// present-then-all-required discipline mirrors the optional `dash`/`crouch`
/// sub-objects, applied at two nesting levels. View feel is consumed by the
/// render-rate evaluator in `view_feel.rs`, called from `main.rs`; this is the data surface only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ViewFeelParams {
    /// Optional head-bob tuning. Absent ⇒ no head bob.
    pub(crate) bob: Option<BobParams>,
    /// Optional strafe-tilt tuning. Absent ⇒ no strafe tilt.
    pub(crate) tilt: Option<TiltParams>,
    /// Optional ambient-sway tuning. Absent ⇒ no ambient sway.
    pub(crate) sway: Option<SwayParams>,
}

/// Head-bob tuning. When present on `viewFeel`, all fields are required and
/// validated except `grounded_only`, which is optional and defaults to `true`.
/// Field names are camelCase on the wire (`verticalFrequency`,
/// `lateralFrequency`, `verticalAmplitude`, …) and snake_case in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct BobParams {
    /// Vertical bob cycles per metre travelled. Must be finite > 0.
    pub(crate) vertical_frequency: f32,
    /// Lateral bob cycles per metre travelled. Must be finite > 0.
    pub(crate) lateral_frequency: f32,
    /// Vertical bob amplitude. Must be finite ≥ 0.
    pub(crate) vertical_amplitude: f32,
    /// Lateral bob amplitude. Must be finite ≥ 0.
    pub(crate) lateral_amplitude: f32,
    /// Horizontal speed below which bob is suppressed. Must be finite ≥ 0.
    pub(crate) speed_threshold: f32,
    /// Whether bob applies only while grounded. Optional on the wire; the
    /// RESOLVED value is materialized here (default `true` when absent).
    pub(crate) grounded_only: bool,
}

impl BobParams {
    /// Default `groundedOnly` gate applied when the wire field is absent.
    pub(crate) const DEFAULT_GROUNDED_ONLY: bool = true;
}

/// Strafe-tilt tuning. When present on `viewFeel`, all fields are required and
/// validated except `grounded_only`, which is optional and defaults to `true`.
/// Field names are camelCase on the wire and snake_case in Rust; `tension`
/// stays literally `tension` everywhere (the author-facing spring knob).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct TiltParams {
    /// Maximum tilt angle in degrees. Must be finite in `[0, 90]`.
    pub(crate) max_angle: f32,
    /// Lateral speed at which the tilt reaches its reference. Must be finite > 0.
    pub(crate) speed_reference: f32,
    /// Spring tension governing how quickly tilt tracks lateral motion. Must be
    /// finite > 0.
    pub(crate) tension: f32,
    /// Whether tilt applies only while grounded. Optional on the wire; the
    /// RESOLVED value is materialized here (default `true` when absent).
    pub(crate) grounded_only: bool,
}

impl TiltParams {
    /// Default `groundedOnly` gate applied when the wire field is absent.
    pub(crate) const DEFAULT_GROUNDED_ONLY: bool = true;
}

/// Ambient-sway tuning. When present on `viewFeel`, all fields are required and
/// validated except `grounded_only`, which is optional and defaults to `false`.
/// Field names are camelCase on the wire and snake_case in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct SwayParams {
    /// Sway amplitude in degrees. Must be finite ≥ 0.
    pub(crate) amplitude: f32,
    /// Sway oscillation frequency in Hz. Must be finite > 0.
    pub(crate) frequency: f32,
    /// Scales how much movement speed modulates sway. Must be finite ≥ 0.
    pub(crate) speed_scale: f32,
    /// Whether sway applies only while grounded. Optional on the wire; the
    /// RESOLVED value is materialized here (default `false` when absent).
    pub(crate) grounded_only: bool,
}

impl SwayParams {
    /// Default `groundedOnly` gate applied when the wire field is absent.
    pub(crate) const DEFAULT_GROUNDED_ONLY: bool = false;
}
