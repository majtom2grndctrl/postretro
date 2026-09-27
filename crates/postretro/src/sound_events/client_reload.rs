// Connected-client reload edges, derived from the replicated owner-private
// reload and ammo slots so a client hears its own reloads. Presentation only;
// they lag the host by one round trip.
// See: context/lib/audio.md §4 · context/lib/networking.md §Combat authority

use postretro_entities::EntityId;
use postretro_foundation::ReloadStyle;

/// One frame's replicated reload state for the local pawn's active weapon,
/// beside that weapon's authored reload shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ReloadSample {
    pub(crate) weapon: EntityId,
    pub(crate) style: ReloadStyle,
    pub(crate) capacity: u32,
    /// `player.reloadActive`.
    pub(crate) active: bool,
    /// `player.reloadProgress`.
    pub(crate) progress: f32,
    /// `player.ammo`.
    pub(crate) ammo: f32,
    /// `player.ammoReserve`, absent until replicated.
    pub(crate) reserve: Option<f32>,
}

impl ReloadSample {
    /// Whether this sample, taken while the flag held, shows the host ending
    /// the reload. The host ends one only with the magazine full or the
    /// reserve empty, and neither holds while it continues. A magazine reload
    /// also projects its completion endpoint as full progress, which survives
    /// a shot fired between completion and the sample.
    fn shows_completion(&self) -> bool {
        self.ammo >= self.capacity as f32
            || self.reserve.is_some_and(|reserve| reserve <= 0.0)
            || (self.style == ReloadStyle::Magazine && self.progress >= 1.0)
    }
}

/// Turns successive replicated reload samples into the reload events the host
/// raised for them:
/// - start is the flag rising;
/// - a shell is ammo rising on a per-shell reload, the start sample included;
///   shells that arrive in one snapshot sound once;
/// - complete is the flag falling after a held sample that showed completion.
///
/// Any other fall is a cancel or a switch and plays nothing. Only a reload
/// whose start this tracker saw on the current weapon produces shells or a
/// complete, so the samples a switch leaves behind play nothing.
#[derive(Debug, Default)]
pub(crate) struct ClientReloadEdges {
    previous: Option<ReloadSample>,
    /// The latest held sample of the reload in progress.
    held: Option<ReloadSample>,
}

impl ClientReloadEdges {
    /// Observe this frame's replicated state and return the reload events it
    /// completes, in host order. A missing sample or a weapon change resets
    /// the baseline without an edge.
    pub(crate) fn observe(&mut self, sample: Option<ReloadSample>) -> Vec<&'static str> {
        let Some(current) = sample else {
            self.previous = None;
            self.held = None;
            return Vec::new();
        };
        let Some(previous) = self
            .previous
            .replace(current)
            .filter(|previous| previous.weapon == current.weapon)
        else {
            self.held = None;
            return Vec::new();
        };
        let shell = current.style == ReloadStyle::PerShell && current.ammo > previous.ammo;
        let mut edges = Vec::new();
        match (previous.active, current.active) {
            (false, true) => {
                edges.push("reload_started");
                if shell {
                    edges.push("reload_shell_loaded");
                }
                self.held = Some(current);
            }
            (true, true) => {
                if let Some(held) = self.held.as_mut() {
                    if shell {
                        edges.push("reload_shell_loaded");
                    }
                    *held = current;
                }
            }
            (true, false) => {
                if self.held.take().is_some_and(|held| held.shows_completion()) {
                    edges.push("reload_completed");
                }
            }
            (false, false) => {}
        }
        edges
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rifle() -> EntityId {
        EntityId::from_raw(1)
    }

    fn pistol() -> EntityId {
        EntityId::from_raw(2)
    }

    /// One replicated state as the owner-private projection publishes it.
    #[derive(Clone, Copy)]
    struct Slots {
        active: bool,
        progress: f32,
        ammo: f32,
        reserve: f32,
    }

    const fn idle(ammo: f32, reserve: f32) -> Slots {
        Slots {
            active: false,
            progress: 0.0,
            ammo,
            reserve,
        }
    }

    const fn held(progress: f32, ammo: f32, reserve: f32) -> Slots {
        Slots {
            active: true,
            progress,
            ammo,
            reserve,
        }
    }

    fn sample(weapon: EntityId, style: ReloadStyle, capacity: u32, slots: Slots) -> ReloadSample {
        ReloadSample {
            weapon,
            style,
            capacity,
            active: slots.active,
            progress: slots.progress,
            ammo: slots.ammo,
            reserve: Some(slots.reserve),
        }
    }

    /// Feed each state for two frames, as a frame loop between snapshots
    /// does, and collect every edge.
    fn run(style: ReloadStyle, capacity: u32, states: &[Slots]) -> Vec<&'static str> {
        let mut edges = ClientReloadEdges::default();
        states
            .iter()
            .flat_map(|slots| [*slots, *slots])
            .flat_map(|slots| edges.observe(Some(sample(rifle(), style, capacity, slots))))
            .collect()
    }

    #[test]
    fn a_completed_magazine_reload_plays_start_and_complete() {
        assert_eq!(
            run(
                ReloadStyle::Magazine,
                8,
                &[
                    idle(2.0, 24.0),
                    held(0.0, 2.0, 24.0), // Started endpoint
                    held(0.5, 2.0, 24.0),
                    held(1.0, 8.0, 18.0), // Completed endpoint: ammo rises, no shell
                    idle(8.0, 18.0),
                ],
            ),
            ["reload_started", "reload_completed"],
        );
    }

    #[test]
    fn a_magazine_completion_followed_by_a_shot_still_plays_complete() {
        assert_eq!(
            run(
                ReloadStyle::Magazine,
                8,
                &[
                    idle(2.0, 24.0),
                    held(0.0, 2.0, 24.0),
                    held(1.0, 7.0, 18.0), // fired the tick after completing
                    idle(7.0, 18.0),
                ],
            ),
            ["reload_started", "reload_completed"],
        );
    }

    #[test]
    fn a_cancelled_reload_plays_its_start_only() {
        // A magazine reload ends early only when a switch lowers the weapon.
        assert_eq!(
            run(
                ReloadStyle::Magazine,
                8,
                &[
                    idle(2.0, 24.0),
                    held(0.0, 2.0, 24.0),
                    held(0.4, 2.0, 24.0),
                    idle(2.0, 24.0),
                ],
            ),
            ["reload_started"],
        );
    }

    #[test]
    fn a_per_shell_reload_plays_each_shell_then_complete() {
        assert_eq!(
            run(
                ReloadStyle::PerShell,
                4,
                &[
                    idle(1.0, 10.0),
                    held(0.0, 1.0, 10.0),
                    held(1.0, 2.0, 9.0), // shell endpoint
                    held(0.3, 2.0, 9.0),
                    held(1.0, 4.0, 7.0), // two shells in one snapshot sound once
                    idle(4.0, 7.0),
                ],
            ),
            [
                "reload_started",
                "reload_shell_loaded",
                "reload_shell_loaded",
                "reload_completed",
            ],
        );
    }

    #[test]
    fn a_per_shell_reload_that_empties_the_reserve_plays_complete() {
        assert_eq!(
            run(
                ReloadStyle::PerShell,
                8,
                &[
                    idle(0.0, 2.0),
                    held(0.0, 0.0, 2.0),
                    held(1.0, 1.0, 1.0),
                    held(1.0, 2.0, 0.0),
                    idle(2.0, 0.0),
                ],
            ),
            [
                "reload_started",
                "reload_shell_loaded",
                "reload_shell_loaded",
                "reload_completed",
            ],
        );
    }

    #[test]
    fn a_per_shell_reload_cancelled_by_fire_plays_start_and_shells_only() {
        assert_eq!(
            run(
                ReloadStyle::PerShell,
                8,
                &[
                    idle(0.0, 10.0),
                    held(0.0, 0.0, 10.0),
                    held(1.0, 1.0, 9.0),
                    held(1.0, 2.0, 8.0), // a shell endpoint, then fire cancels
                    idle(1.0, 8.0),
                ],
            ),
            [
                "reload_started",
                "reload_shell_loaded",
                "reload_shell_loaded"
            ],
        );
    }

    #[test]
    fn a_first_shell_in_the_start_snapshot_plays_start_and_shell() {
        assert_eq!(
            run(
                ReloadStyle::PerShell,
                8,
                &[idle(0.0, 10.0), held(0.0, 1.0, 9.0)],
            ),
            ["reload_started", "reload_shell_loaded"],
        );
    }

    #[test]
    fn a_host_repoint_to_a_fuller_weapon_mid_reload_plays_no_complete() {
        // The ammo slot repoints to the incoming weapon as the flag falls.
        for style in [ReloadStyle::Magazine, ReloadStyle::PerShell] {
            assert_eq!(
                run(
                    style,
                    8,
                    &[
                        idle(0.0, 24.0),
                        held(0.0, 0.0, 24.0),
                        held(0.5, 0.0, 24.0),
                        idle(12.0, 0.0),
                    ],
                ),
                ["reload_started"],
                "{style:?}",
            );
        }
    }

    #[test]
    fn a_local_weapon_switch_mid_reload_plays_nothing_after_the_switch() {
        let mut edges = ClientReloadEdges::default();
        let magazine = |weapon, slots| Some(sample(weapon, ReloadStyle::Magazine, 8, slots));
        assert!(edges.observe(magazine(rifle(), idle(0.0, 24.0))).is_empty());
        assert_eq!(
            edges.observe(magazine(rifle(), held(0.0, 0.0, 24.0))),
            ["reload_started"]
        );
        // The client's own switch changes the weapon before the host's
        // projection repoints; the stale reload it leaves plays nothing.
        for slots in [
            held(0.5, 0.0, 24.0),
            held(1.0, 8.0, 16.0),
            idle(8.0, 16.0),
            idle(12.0, 50.0),
        ] {
            assert!(edges.observe(magazine(pistol(), slots)).is_empty());
        }
    }

    #[test]
    fn missing_state_resets_the_baseline_without_an_edge() {
        let mut edges = ClientReloadEdges::default();
        let magazine = |slots| Some(sample(rifle(), ReloadStyle::Magazine, 8, slots));
        assert!(
            edges.observe(magazine(held(0.0, 0.0, 24.0))).is_empty(),
            "the first sample is a baseline"
        );
        assert!(edges.observe(None).is_empty());
        assert!(
            edges.observe(magazine(held(0.5, 0.0, 24.0))).is_empty(),
            "baseline again"
        );
        assert!(
            edges.observe(magazine(idle(8.0, 16.0))).is_empty(),
            "a reload whose start was not seen completes nothing"
        );
    }
}
