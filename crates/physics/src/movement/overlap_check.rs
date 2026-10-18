// The tick overlap check: own movement that starts a tick clear of static geometry
// must end it clear. It reports a breach and never reverts the move.
// See: context/lib/movement.md §1 · context/lib/development_guide.md §6.4

use glam::Vec3;

use crate::collision::{CollisionCapsule, CollisionWorld, StaticOverlap};

/// Switches the check on (`1`) or off (`0`) in any build. Unset, debug-assertion
/// builds check and release builds do not. Test builds always check.
pub const OVERLAP_CHECK_ENV: &str = "POSTRETRO_OVERLAP_CHECK";

/// Whether the check runs, from the variable's value and the build. `Err`
/// carries the build default for a malformed value, which the caller warns
/// about once.
pub fn resolve_overlap_check(value: Option<&str>, debug_assertions: bool) -> Result<bool, bool> {
    match value.map(str::trim) {
        None => Ok(debug_assertions),
        Some("1") => Ok(true),
        Some("0") => Ok(false),
        Some(_) => Err(debug_assertions),
    }
}

/// Test builds arm the check unconditionally: every movement test asserts the
/// invariant.
#[cfg(any(test, feature = "test-support"))]
fn overlap_check_enabled() -> bool {
    true
}

#[cfg(not(any(test, feature = "test-support")))]
fn overlap_check_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let value = std::env::var(OVERLAP_CHECK_ENV).ok();
        resolve_overlap_check(value.as_deref(), cfg!(debug_assertions)).unwrap_or_else(|default| {
            log::warn!(
                "[Movement] {OVERLAP_CHECK_ENV}={} is not 0 or 1; the overlap check stays {}",
                value.as_deref().unwrap_or_default(),
                if default { "on" } else { "off" },
            );
            default
        })
    })
}

/// Check one body's own movement for the tick: `start` is its pose after mover
/// carry and mover displacement, `end` its pose before any teleport relocation,
/// both at the capsule it ended the tick with. A tick that starts intersecting
/// is not checked, so a mover-caused embed stays silent until the body is
/// clear again. Returns the breach for the caller to log; the move stands.
///
/// In test builds a breach panics, unless a test disarmed the assertion with
/// [`disarm_overlap_assert_for_test`].
pub fn check_own_movement(
    world: &CollisionWorld,
    start: Vec3,
    end: Vec3,
    capsule: CollisionCapsule,
) -> Option<StaticOverlap> {
    // A body that did not move ends where it started: clear if it started clear.
    if !overlap_check_enabled() || start == end {
        return None;
    }
    let overlap = world.overlap_timer().time(|| {
        world
            .static_overlap(end, capsule)
            .filter(|_| !world.overlaps_static(start, capsule))
    })?;
    #[cfg(any(test, feature = "test-support"))]
    if test_assert::armed() {
        panic!(
            "own movement ended a tick inside static geometry: started clear at {start:?}, \
             ended at {end:?} (texture {:?})",
            overlap.texture
        );
    }
    Some(overlap)
}

#[cfg(any(test, feature = "test-support"))]
mod test_assert {
    use std::cell::Cell;

    thread_local! {
        static ARMED: Cell<bool> = const { Cell::new(true) };
    }

    pub(super) fn armed() -> bool {
        ARMED.with(Cell::get)
    }

    pub(super) fn set(armed: bool) -> bool {
        ARMED.with(|cell| cell.replace(armed))
    }
}

/// Keeps the test assertion off on this thread until dropped, so a test can
/// observe the report a breach returns outside tests.
#[cfg(any(test, feature = "test-support"))]
pub struct OverlapAssertDisarmed {
    was_armed: bool,
}

#[cfg(any(test, feature = "test-support"))]
pub fn disarm_overlap_assert_for_test() -> OverlapAssertDisarmed {
    OverlapAssertDisarmed {
        was_armed: test_assert::set(false),
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for OverlapAssertDisarmed {
    fn drop(&mut self) {
        test_assert::set(self.was_armed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor() -> CollisionWorld {
        CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-50.0, 0.0, -50.0),
                Vec3::new(50.0, 0.0, -50.0),
                Vec3::new(50.0, 0.0, 50.0),
                Vec3::new(-50.0, 0.0, 50.0),
            ],
            vec![[0, 2, 1], [0, 3, 2]],
        )
    }

    const CAPSULE: CollisionCapsule = CollisionCapsule {
        radius: 0.4,
        half_height: 0.8,
    };
    /// Centre height resting a skin above the floor.
    const REST: f32 = 1.22;

    #[test]
    fn after_a_mover_caused_embed_the_first_tick_that_starts_clear_is_checked_again() {
        let world = floor();
        let _disarmed = disarm_overlap_assert_for_test();
        // A mover left the body embedded: ticks that start inside stay silent,
        // even one that sinks it deeper.
        let embedded = Vec3::new(0.0, REST - 0.2, 0.0);
        assert_eq!(
            check_own_movement(&world, embedded, embedded - Vec3::Y * 0.05, CAPSULE),
            None
        );
        // The first tick that starts clear is checked: a forced overlap is caught.
        let clear = Vec3::new(0.0, REST, 0.0);
        let sunk = Vec3::new(0.1, REST - 0.07, 0.0);
        let overlap = check_own_movement(&world, clear, sunk, CAPSULE)
            .expect("the first clear-start tick is checked again");
        assert_eq!(overlap.position, sunk);
        // Resting contact inside the skin band, or within the tolerance, is
        // not a breach.
        let resting = Vec3::new(0.2, REST - 0.0195, 0.0);
        assert_eq!(check_own_movement(&world, clear, resting, CAPSULE), None);
    }

    #[test]
    #[should_panic(expected = "own movement ended a tick inside static geometry")]
    fn an_armed_check_panics_on_a_breach() {
        let world = floor();
        check_own_movement(
            &world,
            Vec3::new(0.0, REST, 0.0),
            Vec3::new(0.0, REST - 0.1, 0.0),
            CAPSULE,
        );
    }

    #[test]
    fn the_check_defaults_on_with_debug_assertions_and_off_in_release() {
        assert_eq!(resolve_overlap_check(None, true), Ok(true));
        assert_eq!(resolve_overlap_check(None, false), Ok(false));
    }

    #[test]
    fn the_variable_turns_the_check_on_or_off_in_any_build() {
        for debug in [false, true] {
            assert_eq!(resolve_overlap_check(Some("1"), debug), Ok(true));
            assert_eq!(resolve_overlap_check(Some("0"), debug), Ok(false));
        }
        assert_eq!(
            resolve_overlap_check(Some("yes"), false),
            Err(false),
            "a malformed value keeps the build default"
        );
    }
}
