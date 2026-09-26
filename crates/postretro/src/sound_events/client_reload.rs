// Connected-client reload edges, derived from replicated owner-private state
// (the reload flag and the active magazine) so a client hears its own reloads.
// Presentation only; they lag the host by one round trip.
// See: context/lib/audio.md §4

use postretro_entities::EntityId;

/// The reload state one frame observed for the local pawn's active weapon.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ReloadSample {
    weapon: EntityId,
    active: bool,
    ammo: f32,
}

/// Turns successive replicated reload samples into the reload event names the
/// host raised: start is the flag rising, a shell is ammo rising while the
/// flag holds, and complete is the flag falling after ammo rose. A cancel
/// (the flag falling with no ammo gained) plays nothing. Shells that arrive in
/// one snapshot sound once.
#[derive(Debug, Default)]
pub(crate) struct ClientReloadEdges {
    previous: Option<ReloadSample>,
    ammo_rose: bool,
}

impl ClientReloadEdges {
    /// Observe this frame's replicated state, returning the reload event name
    /// it completes, if any. A missing weapon or count, or a weapon switch,
    /// resets the baseline without an edge.
    pub(crate) fn observe(
        &mut self,
        weapon: Option<EntityId>,
        active: bool,
        ammo: Option<f32>,
    ) -> Option<&'static str> {
        let (Some(weapon), Some(ammo)) = (weapon, ammo) else {
            self.previous = None;
            self.ammo_rose = false;
            return None;
        };
        let current = ReloadSample {
            weapon,
            active,
            ammo,
        };
        let previous = self.previous.replace(current);
        let Some(previous) = previous.filter(|previous| previous.weapon == weapon) else {
            self.ammo_rose = false;
            return None;
        };
        match (previous.active, active) {
            (false, true) => {
                self.ammo_rose = false;
                Some("reload_started")
            }
            (true, true) if ammo > previous.ammo => {
                self.ammo_rose = true;
                Some("reload_shell_loaded")
            }
            (true, false) => {
                let completed = self.ammo_rose || ammo > previous.ammo;
                self.ammo_rose = false;
                completed.then_some("reload_completed")
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(samples: &[(bool, f32)]) -> Vec<&'static str> {
        let weapon = EntityId::from_raw(1);
        let mut edges = ClientReloadEdges::default();
        samples
            .iter()
            .filter_map(|(active, ammo)| edges.observe(Some(weapon), *active, Some(*ammo)))
            .collect()
    }

    #[test]
    fn a_completed_magazine_reload_plays_start_and_complete() {
        assert_eq!(
            run(&[(false, 0.0), (true, 0.0), (true, 0.0), (false, 8.0)]),
            ["reload_started", "reload_completed"],
        );
    }

    #[test]
    fn a_cancelled_reload_plays_its_start_only() {
        assert_eq!(
            run(&[(false, 2.0), (true, 2.0), (false, 2.0)]),
            ["reload_started"],
        );
    }

    #[test]
    fn a_shell_reload_plays_each_arriving_shell_then_complete() {
        assert_eq!(
            run(&[
                (false, 0.0),
                (true, 0.0),
                (true, 1.0),
                (true, 3.0), // two shells in one snapshot sound once
                (false, 3.0),
            ]),
            [
                "reload_started",
                "reload_shell_loaded",
                "reload_shell_loaded",
                "reload_completed"
            ],
        );
    }

    #[test]
    fn a_weapon_switch_or_missing_state_resets_without_an_edge() {
        let mut edges = ClientReloadEdges::default();
        let (rifle, pistol) = (EntityId::from_raw(1), EntityId::from_raw(2));
        assert_eq!(
            edges.observe(Some(rifle), true, Some(0.0)),
            None,
            "first sample is a baseline"
        );
        assert_eq!(
            edges.observe(Some(pistol), false, Some(12.0)),
            None,
            "a switch is no complete"
        );
        assert_eq!(edges.observe(None, false, None), None);
        assert_eq!(
            edges.observe(Some(pistol), true, Some(12.0)),
            None,
            "baseline again"
        );
    }
}
