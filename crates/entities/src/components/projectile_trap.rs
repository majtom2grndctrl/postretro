//! Engine-owned map-authored projectile trap: authored timing plus the host-only runtime.
//! See: context/lib/entity_model.md §Components (ProjectileTrap)

use serde::{Deserialize, Serialize};

use super::weapon::WeaponComponent;

/// Default repeat interval when a trap authors no `interval_ms`.
pub const DEFAULT_TRAP_INTERVAL_MS: f64 = 1000.0;

/// One request a trap command issues. Requests fold into the trap's
/// [`TrapPending`] in issue order and apply at the next trap stage, before
/// its due check.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrapRequest {
    /// Run one execution of the weapon's primary action.
    Fire,
    /// Start repeating. Each field absent takes the trap's authored value.
    Start {
        interval_ms: Option<f64>,
        phase_ms: Option<f64>,
    },
    /// Cancel future repeats; an in-flight execution finishes.
    Stop,
}

/// Requests awaiting the next trap stage, folded in issue order into the
/// equivalent bounded set: `stop` applies first, then `start`, and `fire`
/// stands apart because no start or stop ever cancels it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TrapPending {
    pub fire: bool,
    pub stop: bool,
    /// The run's `(interval_ms, phase_ms)` options; `None` fields take the
    /// authored values.
    pub start: Option<(Option<f64>, Option<f64>)>,
}

/// An anchored repeat schedule. `countdown_ticks` is the number of trap
/// stages before the next due one; zero means due on this stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrapSchedule {
    pub interval_ticks: u32,
    pub countdown_ticks: u32,
}

/// The armed half of a trap, present once install resolved its weapon. It
/// rides this component rather than the registry's `Weapon` column, so weapon
/// sweeps (passive resources, cancellation, descriptor refresh) never reach a
/// trap's stripped weapon. The kind never replicates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrapRuntime {
    /// Canonical weapon descriptor name: the presentation class and the
    /// projectile's source weapon.
    pub canonical_weapon: String,
    /// Built from the install-time descriptor with its resource removed, so
    /// every shot costs nothing and nothing reloads, dry-fires or overheats.
    pub weapon: WeaponComponent,
    pub schedule: Option<TrapSchedule>,
    /// The trap's own activation clock, one step per trap stage.
    pub clock: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectileTrapComponent {
    /// Canonical weapon descriptor name authored in the `weapon` KVP.
    pub weapon_name: String,
    /// Milliseconds, kept in f64 so tick rounding matches `wait`'s exactly.
    pub interval_ms: f64,
    pub phase_ms: f64,
    pub start_on_spawn: bool,
    /// False when an authored timing KVP was invalid; parse warned once and the
    /// trap stays inert whatever its weapon.
    pub timing_valid: bool,
    /// `None` until install resolves a valid projectile weapon; an inert trap
    /// keeps `None` and drops every request.
    #[serde(default)]
    pub runtime: Option<Box<TrapRuntime>>,
    #[serde(default)]
    pub pending: TrapPending,
}

impl ProjectileTrapComponent {
    pub fn new(weapon_name: String, interval_ms: f64, phase_ms: f64, start_on_spawn: bool) -> Self {
        Self {
            weapon_name,
            interval_ms,
            phase_ms,
            start_on_spawn,
            timing_valid: true,
            runtime: None,
            pending: TrapPending::default(),
        }
    }

    pub fn is_armed(&self) -> bool {
        self.runtime.is_some()
    }
}
