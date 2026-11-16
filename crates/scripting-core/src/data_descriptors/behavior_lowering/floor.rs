// The co-op floor warning, deduped per stance across one mod load.
// See: context/lib/scripting.md §2 (Attack and stance declarations)

use std::cell::RefCell;
use std::collections::BTreeSet;

use postretro_foundation::stance_defaults::STANCE_HOLD_FLOOR_MS;

/// A warned stance: its name and the bits of its hold minimum, so two
/// different stances sharing a name in one mod each warn.
type WarnedStance = (String, u64);

thread_local! {
    /// Stances already warned in the open scope; `None` outside one.
    static WARNED: RefCell<Option<BTreeSet<WarnedStance>>> = const { RefCell::new(None) };
}

/// One mod load's dedupe of floor warnings. The outermost scope on a thread
/// owns the set; scopes opened inside it (each behavior's lowering) share it,
/// so a mod load warns once per stance and a lone lowering once per behavior.
pub(crate) struct FloorWarningScope {
    outermost: bool,
}

/// Open a floor-warning scope for one mod load. Mod init and the staged
/// reload each hold one across every entity they read.
pub(crate) fn stance_floor_warning_scope() -> FloorWarningScope {
    let outermost = WARNED.with(|warned| {
        let mut warned = warned.borrow_mut();
        let outermost = warned.is_none();
        if outermost {
            *warned = Some(BTreeSet::new());
        }
        outermost
    });
    FloorWarningScope { outermost }
}

impl Drop for FloorWarningScope {
    fn drop(&mut self) {
        if self.outermost {
            WARNED.with(|warned| *warned.borrow_mut() = None);
        }
    }
}

/// Warn, once per open scope, that `stance`'s hold can release at `min_ms`,
/// below the co-op floor. One stance used by many archetypes warns once;
/// keying by name alone would let one stance hide another of its name.
pub(super) fn warn_below_floor(stance: &str, min_ms: f64) {
    // `-0.0` and `0.0` are one hold time: adding `0.0` folds the sign, so
    // they key alike and print alike.
    let min_ms = min_ms + 0.0;
    let first = WARNED.with(|warned| match warned.borrow_mut().as_mut() {
        Some(set) => set.insert((stance.to_string(), min_ms.to_bits())),
        None => true,
    });
    if first {
        log::warn!(
            "[Scripting] stance \"{stance}\" can release {min_ms} ms into its hold, below the co-op floor of {STANCE_HOLD_FLOOR_MS} ms (the remote interpolation ceiling plus a reaction budget); a remote player may see the strike before the tell"
        );
    }
}
