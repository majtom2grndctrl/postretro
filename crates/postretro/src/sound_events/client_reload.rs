// Connected-client reload edges, derived from the replicated owner-private
// reload and ammo slots so a client hears its own reloads. Presentation only;
// they lag the host by one round trip.
// See: context/lib/audio.md §4 · context/lib/networking.md §Combat authority

use postretro_entities::EntityId;
use postretro_foundation::ReloadStyle;

/// One frame's replicated reload state, beside the authored reload shape of
/// the weapon it describes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ReloadSample {
    /// The weapon the replicated slots describe: the host's active weapon, as
    /// its slot-correlated cooldown sample names it. It lags a local switch by
    /// a round trip.
    pub(crate) weapon: EntityId,
    /// Whether the client's own active weapon is that weapon.
    pub(crate) wielded: bool,
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

    /// Whether this sample, taken as the flag fell, shows the final transfer
    /// of the reload `held` belonged to: ammo rose by exactly what the reserve
    /// fell. This survives a lost completion endpoint. A fire cancel spends
    /// ammo, and a repoint to another weapon replaces both counts, so neither
    /// conserves; a switch the client itself made is excluded by `wielded`.
    fn conserves_transfer_from(&self, held: &ReloadSample) -> bool {
        let (Some(held_reserve), Some(reserve)) = (held.reserve, self.reserve) else {
            return false;
        };
        let gained = self.ammo - held.ammo;
        // Counts are whole rounds; the tolerance only absorbs float storage.
        self.wielded && gained > 0.0 && (gained - (held_reserve - reserve)).abs() < 0.5
    }

    /// Whether a rising flag is a replayed completion endpoint rather than a
    /// start. A start projects its Started endpoint (no progress) or low live
    /// progress. The endpoint stream separates two equal queued endpoints with
    /// one live sample; once the reload is over that sample reads idle, and
    /// the queued completion then reads as full progress with the flag up.
    fn is_echoed_completion(&self) -> bool {
        self.progress >= 1.0
    }
}

/// Turns successive replicated reload samples into the reload events the host
/// raised for them:
/// - start is the flag rising, unless the rise replays a completion endpoint;
/// - a shell is ammo rising on a per-shell reload, the start sample included;
///   shells that arrive in one snapshot sound once;
/// - complete is the flag falling after a held sample that showed completion,
///   or on a fall that shows the final transfer (`conserves_transfer_from`).
///
/// Any other fall is a cancel or a switch and plays nothing. Samples follow
/// the weapon the host projects, not the client's own active weapon: a local
/// switch the host refuses leaves a reload in progress tracked, while a switch
/// the host performs repoints the projection, which ends the reload with no
/// edge. Only a reload whose start this tracker saw on the projected weapon
/// produces shells or a complete.
#[derive(Debug, Default)]
pub(crate) struct ClientReloadEdges {
    previous: Option<ReloadSample>,
    /// The latest held sample of the reload in progress.
    held: Option<ReloadSample>,
}

impl ClientReloadEdges {
    /// Observe this frame's replicated state and return the reload events it
    /// completes, in host order. A missing sample or a change of projected
    /// weapon resets the baseline without an edge.
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
                if !current.is_echoed_completion() {
                    edges.push("reload_started");
                    if shell {
                        edges.push("reload_shell_loaded");
                    }
                    self.held = Some(current);
                }
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
                if let Some(held) = self.held.take() {
                    if held.shows_completion() {
                        edges.push("reload_completed");
                    } else if current.conserves_transfer_from(&held) {
                        if shell {
                            edges.push("reload_shell_loaded");
                        }
                        edges.push("reload_completed");
                    }
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

    /// A sample of `weapon` while the client wields it.
    fn sample(weapon: EntityId, style: ReloadStyle, capacity: u32, slots: Slots) -> ReloadSample {
        ReloadSample {
            weapon,
            wielded: true,
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
    fn a_host_performed_switch_mid_reload_plays_nothing_after_the_repoint() {
        let mut edges = ClientReloadEdges::default();
        let magazine = |weapon, slots| Some(sample(weapon, ReloadStyle::Magazine, 8, slots));
        assert!(edges.observe(magazine(rifle(), idle(0.0, 24.0))).is_empty());
        assert_eq!(
            edges.observe(magazine(rifle(), held(0.0, 0.0, 24.0))),
            ["reload_started"]
        );
        // The host lowers the reloading rifle and the projection repoints to
        // the pistol; the reload it ended plays nothing, nor does a return.
        for (weapon, slots) in [
            (pistol(), idle(12.0, 50.0)),
            (pistol(), idle(12.0, 50.0)),
            (rifle(), idle(0.0, 24.0)),
            (rifle(), idle(0.0, 24.0)),
        ] {
            assert!(edges.observe(magazine(weapon, slots)).is_empty());
        }
    }

    // Regression: a local switch reset the tracker, so a switch the host
    // refused mid-reload (blockDuringReload) lost the reload's complete.
    #[test]
    fn a_refused_local_switch_mid_reload_still_plays_the_complete() {
        for completes_during_the_detour in [false, true] {
            let mut edges = ClientReloadEdges::default();
            let mut observed = Vec::new();
            let mut observe = |wielded, slots| {
                observed.extend(edges.observe(Some(ReloadSample {
                    wielded,
                    ..sample(rifle(), ReloadStyle::Magazine, 8, slots)
                })));
            };
            observe(true, idle(2.0, 24.0));
            observe(true, held(0.0, 2.0, 24.0));
            // The client lowers toward the pistol; the host keeps projecting
            // the reloading rifle and refuses the switch.
            observe(false, held(0.4, 2.0, 24.0));
            if completes_during_the_detour {
                observe(false, held(1.0, 8.0, 18.0));
                observe(false, idle(8.0, 18.0));
                observe(true, idle(8.0, 18.0));
            } else {
                // The refusal rolls the client back while the flag holds.
                observe(true, held(0.7, 2.0, 24.0));
                observe(true, held(1.0, 8.0, 18.0));
                observe(true, idle(8.0, 18.0));
            }
            assert_eq!(
                observed,
                ["reload_started", "reload_completed"],
                "completes during the detour: {completes_during_the_detour}",
            );
        }
    }

    // Regression: the endpoint stream's live separator between two equal
    // queued completions read idle once the reload ended, and the queued
    // completion then read as a new start.
    #[test]
    fn an_echoed_completion_after_the_reload_ends_plays_nothing_more() {
        assert_eq!(
            run(
                ReloadStyle::PerShell,
                8,
                &[
                    idle(0.0, 10.0),
                    held(0.0, 0.0, 10.0), // Started endpoint
                    held(1.0, 1.0, 9.0),  // first shell's endpoint
                    idle(2.0, 8.0),       // separator: the final shell already landed
                    held(1.0, 2.0, 8.0),  // the queued final completion, echoed
                    idle(2.0, 8.0),
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
    fn a_start_first_seen_mid_progress_still_plays_start() {
        // A lost Started endpoint leaves live progress as the first held sample.
        assert_eq!(
            run(
                ReloadStyle::Magazine,
                8,
                &[
                    idle(2.0, 24.0),
                    held(0.4, 2.0, 24.0),
                    held(1.0, 8.0, 18.0),
                    idle(8.0, 18.0),
                ],
            ),
            ["reload_started", "reload_completed"],
        );
    }

    // Regression: a single lost snapshot dropped the only held sample that
    // showed completion, the owner projection's Completed endpoint.
    #[test]
    fn a_lost_completion_endpoint_still_plays_complete_when_ammo_is_conserved() {
        assert_eq!(
            run(
                ReloadStyle::Magazine,
                8,
                &[
                    idle(2.0, 24.0),
                    held(0.0, 2.0, 24.0),
                    held(0.5, 2.0, 24.0),
                    idle(8.0, 18.0), // the Completed endpoint's snapshot was lost
                ],
            ),
            ["reload_started", "reload_completed"],
        );
        assert_eq!(
            run(
                ReloadStyle::PerShell,
                8,
                &[
                    idle(0.0, 10.0),
                    held(0.0, 0.0, 10.0),
                    held(1.0, 1.0, 9.0),
                    idle(3.0, 7.0), // the last shells and their completion were lost
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
    fn an_unconserved_or_unwielded_fall_plays_no_complete() {
        // A shot after an unseen completion spends ammo: the residual this
        // discriminator accepts.
        assert_eq!(
            run(
                ReloadStyle::Magazine,
                8,
                &[
                    idle(2.0, 24.0),
                    held(0.0, 2.0, 24.0),
                    held(0.5, 2.0, 24.0),
                    idle(7.0, 18.0),
                ],
            ),
            ["reload_started"],
        );
        // A conserving fall while the client is switching away is not trusted.
        let mut edges = ClientReloadEdges::default();
        let magazine = |wielded, slots| {
            Some(ReloadSample {
                wielded,
                ..sample(rifle(), ReloadStyle::Magazine, 8, slots)
            })
        };
        assert!(edges.observe(magazine(true, idle(2.0, 24.0))).is_empty());
        assert_eq!(
            edges.observe(magazine(true, held(0.0, 2.0, 24.0))),
            ["reload_started"]
        );
        assert!(
            edges
                .observe(magazine(false, held(0.5, 2.0, 24.0)))
                .is_empty()
        );
        assert!(edges.observe(magazine(false, idle(8.0, 18.0))).is_empty());
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
