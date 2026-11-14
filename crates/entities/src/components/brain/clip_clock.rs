// Per-brain clip clock: game-time clip time for the animation an enemy's path
// selects, and the per-entry moment latch of a clip-timed attack. Storage only;
// `postretro-ai` owns every rule that reads or writes it.
// See: context/lib/entity_model.md §7c (Clip-timed attacks)

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// The clip time an enemy's AI tick accumulates for its drawn animation state.
///
/// The serialized half (tracked state, clip time, moment latch, owed phase) is
/// simulation state: a restore resumes mid-entry and fires on the same tick.
/// The cached clip facts and the accounted mesh switch count are runtime-only
/// and rebind lazily, so they stay out of serde and out of equality.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClipClock {
    /// The drawn animation state the clock times: the path-selected state with
    /// its presented travel-variant suffix (`walk_left`). `None` tracks
    /// nothing. Shared, so cloning a brain each tick never allocates; a restart
    /// is the only edge that builds a new name.
    #[serde(default)]
    pub state: Option<Arc<str>>,
    /// Unwrapped clip time in seconds: the state's start (0, or the drawn phase
    /// on a loop) plus every tick's `dt × rate` since. A loop never wraps it, so
    /// each pass of a moment is a distinct crossing.
    #[serde(default)]
    pub time: f64,
    /// [`Self::time`] before this tick's advance. A moment is reached on the
    /// tick its time falls in `(previous_time, time]`.
    #[serde(default)]
    pub previous_time: f64,
    /// Open while the active `at` attack's moment is still pending this entry.
    /// Reaching the moment, leaving the activity, a knockback launch or a
    /// foreign animation switch closes it.
    #[serde(default)]
    pub moment_open: bool,
    /// This entry drew a clip that times its moment. Once it has, a closed
    /// latch is spent: a later clip lacking the event (a travel variant's)
    /// does not fire the attack as without `at`.
    #[serde(default)]
    pub moment_timed: bool,
    /// The tracked state restarted before its clip resolved, at 0: a loop
    /// still owes its drawn phase once the clip resolves.
    #[serde(default)]
    pub unphased: bool,
    /// The tracked state's resolved clip, cached on first use. `None` when not
    /// yet looked up or when the clip did not resolve, which is looked up again.
    #[serde(skip)]
    pub clip: Option<ClipFacts>,
    /// The mesh's switch count the brain last accounted for. `None` on a fresh
    /// or restored brain, which rebinds to the mesh's current count.
    #[serde(skip)]
    pub mesh_switches: Option<u32>,
}

/// What the clock caches of the tracked state's resolved clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipFacts {
    /// Clip length in seconds.
    pub duration: f32,
    /// The animation state loops its clip.
    pub looping: bool,
}

impl ClipClock {
    /// Whether the clock tracks `base` drawn with travel-variant `suffix`.
    /// Compared piecewise, so the per-tick check composes no name.
    pub fn tracks(&self, base: &str, suffix: &str) -> bool {
        self.state.as_deref().is_some_and(|state| {
            state.len() == base.len() + suffix.len()
                && state.starts_with(base)
                && state.ends_with(suffix)
        })
    }
}

/// Only the serialized half compares: the cached facts and the switch count
/// are derived, like a brain's bound guard programs.
impl PartialEq for ClipClock {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state
            && self.time == other.time
            && self.previous_time == other.previous_time
            && self.moment_open == other.moment_open
            && self.moment_timed == other.moment_timed
            && self.unphased == other.unphased
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_clock_serializes_its_clock_and_latch_but_not_its_caches() {
        let clock = ClipClock {
            state: Some(Arc::from("walk_left")),
            time: 1.25,
            previous_time: 1.25 - 1.0 / 60.0,
            moment_open: true,
            moment_timed: true,
            unphased: true,
            clip: Some(ClipFacts {
                duration: 2.0,
                looping: true,
            }),
            mesh_switches: Some(7),
        };
        let json = serde_json::to_value(&clock).expect("clip clock serializes");
        let object = json.as_object().expect("clip clock is an object");
        assert!(!object.contains_key("clip"));
        assert!(!object.contains_key("mesh_switches"));

        let restored: ClipClock = serde_json::from_value(json).expect("clip clock loads");
        assert_eq!(restored, clock, "the serialized half round-trips");
        assert_eq!(restored.clip, None, "clip facts rebind lazily");
        assert_eq!(restored.mesh_switches, None, "the switch count rebinds");
    }

    #[test]
    fn clip_clock_tracks_a_drawn_state_by_base_and_suffix() {
        let clock = ClipClock {
            state: Some(Arc::from("walk_left")),
            ..ClipClock::default()
        };
        assert!(clock.tracks("walk", "_left"));
        assert!(clock.tracks("walk_left", ""));
        assert!(!clock.tracks("walk", ""));
        assert!(!clock.tracks("walk", "_right"));
        assert!(!ClipClock::default().tracks("walk", ""));
    }
}
