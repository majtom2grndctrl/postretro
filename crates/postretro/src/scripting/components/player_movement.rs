// Player movement component: live state carried on the player entity.
// Owns `MovementState` (per-tick intent dispatcher) and `refresh_on_landing`
// (unified ability-budget landing-refresh point). Materialized at spawn from
// `PlayerMovementDescriptor`; mutated each tick by `crate::movement::tick`.
//
// See: context/lib/entity_model.md §7 (collision/movement)
//      context/lib/movement.md §4 (state-machine seam)
//      context/lib/movement.md §6 (input forgiveness)

use glam::Vec3;
use serde::{Deserialize, Serialize};

// Cross-module (game-logic stage) import: the bound dash expressions the
// component holds are `BoundProgram<MovementScope>` from the `movement` module.
// This mirrors the reverse import (`movement/scope.rs` references
// `PlayerMovementComponent`); both modules are game-logic-stage, so no subsystem
// boundary is crossed.
use crate::movement::MovementScope;
use crate::scripting::data_descriptors::{
    AirParams, BoolOrIr, CapsuleParams, CrouchParams, DashParams, FallParams, ForgivenessParams,
    GroundParams, NumberOrIr, PlayerMovementDescriptor, ViewFeelParams,
};
use crate::scripting::ir::{BakedIr, BindError, BoundProgram, CURRENT_IR_VERSION, IrNode, bind};

/// Bound dash value expressions, one slot per expression-capable
/// [`DashParams`] field. A slot is `Some` exactly when the corresponding field
/// is authored as an expression ([`NumberOrIr::Ir`] / [`BoolOrIr::Ir`]); a
/// literal field leaves it `None` and reads its bare value directly.
///
/// This is DERIVED data: [`PlayerMovementComponent::from_descriptor`] is the sole
/// builder, binding each expression against [`MovementScope::for_validation`].
/// The same static type table already validated the expressions at descriptor
/// declaration (Task 2), so a bind failure here is unreachable in practice; if it
/// ever occurs `from_descriptor` warns once and materializes with dash disabled.
///
/// Not serialized (`#[serde(skip)]` on the component field) and compared as
/// always-equal: it carries no authoritative state, only programs re-derivable
/// from `dash`, which IS serialized and compared.
#[derive(Debug, Default, Clone)]
pub(crate) struct DashPrograms {
    pub(crate) boost_speed: Option<BoundProgram<MovementScope>>,
    pub(crate) momentum_retention: Option<BoundProgram<MovementScope>>,
    pub(crate) steer_control: Option<BoundProgram<MovementScope>>,
    pub(crate) dash_drag: Option<BoundProgram<MovementScope>>,
    pub(crate) cooldown_ms: Option<BoundProgram<MovementScope>>,
    pub(crate) preserve_vertical: Option<BoundProgram<MovementScope>>,
}

impl PartialEq for DashPrograms {
    /// Programs are derived from `dash` (which is compared), so two components
    /// with equal `dash` fields have equivalent programs by construction. Treat
    /// them as always equal so `PlayerMovementComponent`'s derived `PartialEq`
    /// rests entirely on the authoritative descriptor fields.
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl DashPrograms {
    /// Bind every expression-form field of `dash` against the validation scope,
    /// producing the per-field bound-program slots. Returns `Err` on the first
    /// bind failure (unreachable post-declaration; `from_descriptor` degrades to
    /// dash-disabled if it ever fires). A literal field yields a `None` slot.
    fn from_dash(dash: &DashParams) -> Result<Self, BindError> {
        Ok(Self {
            boost_speed: bind_number_field(&dash.boost_speed)?,
            momentum_retention: bind_number_field(&dash.momentum_retention)?,
            steer_control: bind_number_field(&dash.steer_control)?,
            dash_drag: bind_number_field(&dash.dash_drag)?,
            cooldown_ms: bind_number_field(&dash.cooldown_ms)?,
            preserve_vertical: bind_bool_field(&dash.preserve_vertical)?,
        })
    }
}

/// Bind a number dash field's expression against the validation scope, or `None`
/// when the field is a literal. The IR node is wrapped in a read-only [`BakedIr`]
/// envelope, mirroring the declaration-time validation in `data_descriptors.rs`.
fn bind_number_field(field: &NumberOrIr) -> Result<Option<BoundProgram<MovementScope>>, BindError> {
    match field {
        NumberOrIr::Literal(_) => Ok(None),
        NumberOrIr::Ir(node) => bind_dash_node(node).map(Some),
    }
}

/// Boolean analogue of [`bind_number_field`].
fn bind_bool_field(field: &BoolOrIr) -> Result<Option<BoundProgram<MovementScope>>, BindError> {
    match field {
        BoolOrIr::Literal(_) => Ok(None),
        BoolOrIr::Ir(node) => bind_dash_node(node).map(Some),
    }
}

/// Wrap an [`IrNode`] in a read-only [`BakedIr`] envelope and bind it against
/// [`MovementScope::for_validation`] — the same scope and envelope the
/// declaration-time validator used, so the bind cannot newly fail here.
fn bind_dash_node(node: &IrNode) -> Result<BoundProgram<MovementScope>, BindError> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: node.clone(),
    };
    bind(&baked, &MovementScope::for_validation())
}

/// The player's active movement state. Mutually-exclusive: exactly one state
/// owns the per-tick velocity intent at a time. `tick` dispatches to the
/// active state's intent step, runs the shared collision substrate, then
/// applies any transition the intent returns. Three states exist today:
/// `Normal` (walk/run/jump/air-control baseline), `Dash` (directional
/// velocity-impulse burst), and `Crouching` (reduced-speed locomotion with
/// a shrunk collision capsule). Later states (slide, wall-run, vault) plug
/// in behind the same seam.
///
/// See: context/lib/movement.md §4 (state-machine seam).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub(crate) enum MovementState {
    /// Baseline locomotion: gravity, jump/air-jump, ground acceleration,
    /// ground friction, airborne cap. The behavior-unchanged baseline.
    #[default]
    Normal,
    /// Active dash burst. Carries the per-dash state that lives only while the
    /// dash is active: an elapsed-time guard against `DASH_MAX_MS` and the live
    /// additive boost vector (the D4 layer that `dash_drag` decays). The
    /// cooldown timer and air-dash charge counter persist across states, so
    /// they live on the component, not here.
    ///
    /// `elapsed_ms` accumulates `dt * 1000` each tick; the dash exits when it
    /// reaches `DASH_MAX_MS`. `boost` is the horizontal additive vector layered
    /// on top of the retained base velocity — only this layer decays under
    /// `dash_drag`.
    Dash { elapsed_ms: f32, boost: Vec3 },
    /// Active crouch. The player runs `Normal`-style gravity/locomotion but
    /// targets the crouch speed tier and holds a shrunk collision capsule.
    /// `eye_current` is the live smoothing source for the eye-height
    /// interpolation (D3): each tick it advances exponentially toward the
    /// crouched eye target and is written into `component.capsule.eye_height`
    /// for the camera follow to read. It lives on the state (not the component)
    /// because it is meaningful only while crouched — exactly like `Dash`'s
    /// `elapsed_ms`/`boost`. The standing reference dimensions the stand-up
    /// resize/probe need persist on the component (`standing_half_height` /
    /// `standing_eye_height`), since the live `capsule` is the crouched size.
    Crouching { eye_current: f32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlayerMovementComponent {
    pub(crate) capsule: CapsuleParams,
    pub(crate) ground: GroundParams,
    pub(crate) air: AirParams,
    pub(crate) fall: FallParams,
    /// Cosine of `ground.max_slope` (degrees → radians → cos), precomputed at
    /// materialization so the per-tick floor check is a single dot-product
    /// compare. A surface counts as walkable when the contact normal's Y
    /// component (the dot with world-up) is at least this value.
    pub(crate) cos_walkable: f32,
    /// Optional dash tuning, materialized from the descriptor's `dash` field.
    /// `None` ⇒ dash disabled: the `Normal` → `Dash` transition never fires and
    /// no dash impulse is ever applied.
    pub(crate) dash: Option<DashParams>,
    /// Bound dash value expressions, one slot per expression-capable `dash`
    /// field (`None` for literal fields). DERIVED from `dash` by
    /// `from_descriptor` — never authored, never serialized (`#[serde(skip)]`),
    /// and compared always-equal (see [`DashPrograms`]). When `dash` is `None`
    /// every slot is `None`. The dash intent paths read these to evaluate the
    /// expression form; literal fields skip eval entirely.
    #[serde(skip)]
    pub(crate) dash_programs: DashPrograms,
    /// Optional crouch tuning, materialized from the descriptor's `crouch`
    /// field. `None` ⇒ crouch disabled.
    pub(crate) crouch: Option<CrouchParams>,
    /// Optional first-person view-feel tuning (head bob, strafe tilt, ambient
    /// sway), materialized from the descriptor's `view_feel` field. `None` ⇒
    /// view feel disabled. A render-only camera effect consumed by the
    /// render-rate evaluator in `view_feel.rs`, called from `main.rs`; movement logic never reads it.
    pub(crate) view_feel: Option<ViewFeelParams>,
    /// Configured STANDING capsule half-height — the reference value the
    /// stand-up resize/probe grow back to. Seeded from `desc.capsule.half_height`
    /// at materialization and never mutated. Distinct from the live
    /// `capsule.half_height`, which the crouch intent shrinks to the crouched
    /// size; the standing target must come from this fixed reference, never read
    /// back from the possibly-crouched live capsule.
    pub(crate) standing_half_height: f32,
    /// Configured STANDING eye-height — the reference the eye interpolates back
    /// to when standing. Seeded from `desc.capsule.eye_height`, never mutated.
    pub(crate) standing_eye_height: f32,
    pub(crate) is_grounded: bool,
    pub(crate) velocity: Vec3,
    pub(crate) air_jumps_remaining: u32,
    /// Air-dash charges left before landing. Seeded from `dash.air_dashes` at
    /// construction (0 when dash is disabled) and refreshed on landing through
    /// `refresh_on_landing`. Distinct from the descriptor's `air_dashes` max —
    /// this is the live count consumed by airborne dashes.
    pub(crate) air_dashes_remaining: u32,
    /// Dash cooldown timer in milliseconds. Armed to `dash.cooldown_ms` on dash
    /// entry and decremented unconditionally each tick in `tick` (outside the
    /// per-state intent dispatch) so it advances in every state. A dash may
    /// only fire when this has reached 0.
    pub(crate) dash_cooldown_ms: f32,
    /// Consecutive ticks the player has spent without floor contact. Used to
    /// gate `landed` event emission so the 1-tick airborne blip introduced by
    /// the step-up probe's vertical lift cannot fire spurious landings during
    /// normal walking. Reset to 0 on any tick with floor contact; incremented
    /// otherwise.
    pub(crate) air_ticks: u32,
    /// Stuck-stop deadzone: when enabled, the slide loop zeroes horizontal
    /// velocity and rolls back XZ position when contradictory wall normals
    /// (≥60° apart horizontally) are seen within the same tick AND net
    /// horizontal displacement is below `stuck_stop_threshold`. This is the
    /// geometric signature of a corner wedge; max-iteration exhaustion alone
    /// does not trigger it. Suppresses orbital jitter in interior corners.
    /// Disable for gameplay scenarios that want looser physics-driven
    /// micro-motion.
    pub(crate) stuck_stop_enabled: bool,
    /// Horizontal-displacement threshold (metres). The deadzone fires only
    /// when contradictory wall normals were seen and net XZ displacement
    /// this tick is below this value. Tuned well below a single tick's
    /// normal displacement at the canonical ground speed (7 m/s × 1/60 s ≈
    /// 0.117 m) yet above floating-point and skin-distance noise (~1e-4 m).
    pub(crate) stuck_stop_threshold: f32,
    /// The active movement state. Drives `tick`'s per-tick velocity-intent
    /// dispatch. Defaults to `Normal`. NOT preserved across descriptor hot-reload:
    /// `from_descriptor` reseeds `dash_cooldown_ms` to 0 and refills
    /// `air_dashes_remaining` from the new descriptor, so an in-flight `Dash`
    /// surviving a swap would continue with a cleared cooldown and a full charge
    /// budget — a gameplay-state/budget mismatch. Resetting to `Normal` prevents it.
    pub(crate) movement_state: MovementState,
    // --- Input-forgiveness state (coyote time + jump buffer) ---
    //
    // Plain component fields (NOT per-state live data): `Normal` reads them
    // globally and the edges are derived ONCE per tick before the intents run.
    // The windows are materialized from the descriptor; the timers advance off
    // the same `dt` the dash cooldown uses (`dt * 1000` ms per tick). See
    // context/lib/movement.md §6 (input-forgiveness decision).
    /// Coyote-time window in milliseconds (materialized from the descriptor).
    /// `0` disables coyote time. A grounded jump is permitted while
    /// `coyote_timer_ms <= coyote_ms` after ground is lost.
    pub(crate) coyote_ms: f32,
    /// Jump-buffer window in milliseconds (materialized from the descriptor).
    /// `0` disables jump buffering.
    pub(crate) jump_buffer_ms: f32,
    /// Milliseconds since ground was last held, accumulated while airborne and
    /// reset to 0 at the landing-refresh point. The coyote ground-jump edge is
    /// armed while this is within `coyote_ms` (and no jump has been spent).
    pub(crate) coyote_timer_ms: f32,
    /// Milliseconds a pending buffered jump has left to live, counted DOWN each
    /// airborne tick. `0` means no buffer is pending. Set to `jump_buffer_ms`
    /// when jump is pressed airborne; cleared on consumption (the landing tick)
    /// or when it expires before landing (the buffer drops with no jump).
    pub(crate) jump_buffer_timer_ms: f32,
    /// Set when ANY jump (grounded, coyote-grounded, or air) fires; cleared at
    /// the landing-refresh point. Gates coyote so it cannot re-arm once a jump
    /// has been spent for the current airborne stretch.
    pub(crate) jump_spent: bool,
}

impl PlayerMovementComponent {
    /// Materialize from a descriptor. The descriptor's `ground.max_slope` is
    /// in degrees; precomputed `cos_walkable` lets the runtime skip the
    /// per-tick degrees→radians→cosine work.
    pub(crate) fn from_descriptor(desc: &PlayerMovementDescriptor) -> Self {
        let cos_walkable = desc.ground.max_slope.to_radians().cos();
        let air_jumps_remaining = desc.air.jumps;

        // Bind the dash value expressions against the validation scope. The same
        // static type table validated these at declaration (Task 2), so a bind
        // failure here is unreachable. If one ever occurs, degrade VISIBLY: warn
        // once and materialize with dash disabled rather than panicking in this
        // subsystem path (development_guide.md §6.2). `dash`/`dash_programs` move
        // together — disabling `dash` keeps the empty `dash_programs` consistent.
        let (dash, dash_programs) = match desc.dash.as_ref() {
            None => (None, DashPrograms::default()),
            Some(params) => match DashPrograms::from_dash(params) {
                Ok(programs) => (desc.dash.clone(), programs),
                Err(err) => {
                    log::warn!(
                        "[Movement] dash expression failed to bind ({err}); disabling dash for this descriptor"
                    );
                    (None, DashPrograms::default())
                }
            },
        };
        // Mirror how `air_jumps_remaining` is seeded from `air.jumps`: the
        // air-dash budget starts full at construction, 0 when dash is disabled.
        let air_dashes_remaining = dash.as_ref().map_or(0, |d| d.air_dashes);
        // Forgiveness windows materialize here: an absent `forgiveness`
        // sub-object applies the documented engine defaults; a present one
        // already merged per-field defaults at parse time (0 disables a grace).
        let forgiveness = desc.forgiveness.unwrap_or(ForgivenessParams::DEFAULT);
        Self {
            capsule: desc.capsule.clone(),
            ground: desc.ground.clone(),
            air: desc.air.clone(),
            fall: desc.fall.clone(),
            dash,
            dash_programs,
            crouch: desc.crouch.clone(),
            // View feel is a render-only camera effect: clone the descriptor's
            // tuning verbatim (no transform), mirroring ground/air/fall.
            view_feel: desc.view_feel.clone(),
            // Standing reference dimensions: captured from the descriptor's
            // configured capsule before any crouch shrink mutates the live
            // `capsule`. The stand-up resize/probe grow back to these.
            standing_half_height: desc.capsule.half_height,
            standing_eye_height: desc.capsule.eye_height,
            cos_walkable,
            is_grounded: false,
            velocity: Vec3::ZERO,
            air_jumps_remaining,
            air_dashes_remaining,
            dash_cooldown_ms: 0.0,
            air_ticks: 0,
            stuck_stop_enabled: desc.stuck_stop_enabled,
            stuck_stop_threshold: desc.stuck_stop_threshold,
            movement_state: MovementState::Normal,
            coyote_ms: forgiveness.coyote_ms,
            jump_buffer_ms: forgiveness.jump_buffer_ms,
            // Seed the player as "freshly grounded": coyote armed (timer 0), no
            // buffer pending, no jump spent. The first airborne stretch then
            // accumulates the coyote timer from a clean state.
            coyote_timer_ms: 0.0,
            jump_buffer_timer_ms: 0.0,
            jump_spent: false,
        }
    }

    /// Landing-refresh point: reset every ability budget on floor contact.
    /// Invoked from `tick` after the collision substrate reports floor
    /// contact (`SubstrateResult::hit_floor`). Resets `air_jumps_remaining`,
    /// `air_dashes_remaining` (when dash is enabled), `jump_spent`, and
    /// `coyote_timer_ms`. Future ability budgets hook this same single point
    /// so all charges replenish uniformly on landing.
    pub(crate) fn refresh_on_landing(&mut self) {
        self.air_jumps_remaining = self.air.jumps;
        // Air-dash budget refreshes through the same single landing point. When
        // dash is disabled the counter is irrelevant (no dash can consume it).
        if let Some(dash) = &self.dash {
            self.air_dashes_remaining = dash.air_dashes;
        }
        // Input-forgiveness landing refresh: clearing the jump-spent flag and
        // resetting the coyote timer re-arms coyote for the NEXT time the player
        // leaves the ground. The jump buffer is consumed/expired in the tick's
        // edge derivation, not here.
        self.jump_spent = false;
        self.coyote_timer_ms = 0.0;
    }
}

// Manual Serialize/Deserialize impls on the descriptor sub-structs are not
// present; derive on this component requires the sub-structs to derive too.
