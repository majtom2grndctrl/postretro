// Per-brain clip clock: game-time clip time for the animation an enemy's path
// selects, and the per-entry moment latch of a clip-timed attack. Storage, plus
// the one hand-off a held enemy's drop makes; `postretro-ai` owns every rule
// that reads it.
// See: context/lib/entity_model.md §7c (Clip-timed attacks)

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::BrainComponent;
use crate::components::mesh::MeshComponent;
use crate::registry::{EntityId, EntityRegistry};

/// The clip time an enemy's AI tick accumulates for its drawn animation state.
///
/// Everything but the cached clip facts is simulation state, serialized so a
/// restore behaves exactly as an unsaved run: it fires on the same tick and
/// reads the same switches as foreign. The cached facts are runtime-only and
/// rebind lazily, so they stay out of serde and out of equality.
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
    /// The clock restarted after its tick's moment check, when apply drew the
    /// tracked clip again: the next tick's window opens just below the start
    /// instead of at it, so a moment exactly at the start is still reached.
    #[serde(default)]
    pub late_restart: bool,
    /// The mesh's switch count the brain last accounted for. A count that
    /// moved since is a switch the brain did not make. `None` on a fresh brain,
    /// or one saved before the count was kept, which binds to the mesh's count.
    #[serde(default)]
    pub mesh_switches: Option<u32>,
    /// A fresh mesh replaced the one the brain drew (a held enemy's drop), so
    /// it shows its default state: the first AI apply after its clip indices
    /// fill draws the brain's state on it again, which restarts the clock.
    /// Until then the clock holds, reaches no moment and reads not done. A
    /// foreign switch first (a flinch) drops the request.
    #[serde(default)]
    pub redraw: bool,
    /// Clip time advanced since the active path's last entry, summed across
    /// the restarts a travel-variant switch makes, never game time: a slowed
    /// stride counts slower. A stance release's one pass of its clip reads it.
    #[serde(default)]
    pub release_elapsed: f64,
    /// The entered release's moment resolved (fired, whiffed or cancelled),
    /// or with no moment its clip played one pass; a repeating release's
    /// moments all resolved and its passes played: its `afterMoment` rows
    /// open on the next evaluation. Cleared by every entry.
    #[serde(default)]
    pub release_resolved: bool,
    /// The entered release's moments resolved so far (fired or whiffed), one
    /// per pass of a release that repeats (`passes`). Cleared by every entry.
    #[serde(default)]
    pub release_moments: u32,
    /// An impact flinch during an activity with a flinch route (`staggerTo`):
    /// its reaction plays out before the enemy takes the route. Cleared by
    /// every entry.
    #[serde(default)]
    pub stagger: Option<Stagger>,
    /// The tracked state's resolved clip, cached on first use. `None` when not
    /// yet looked up or when the clip did not resolve, which is looked up again.
    #[serde(skip)]
    pub clip: Option<ClipFacts>,
}

/// A flinch playing out over a release or recover, since the first switch
/// that started it: the reaction's end in clip time, the cap in game time.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Stagger {
    /// Clip time since the first switch, in seconds.
    pub elapsed: f64,
    /// The latest reaction's end, in clip time since the first switch. `None`
    /// for a loop or a clip that did not resolve, which only the cap ends.
    pub until: Option<f64>,
    /// Game time since the first switch, in seconds. The route is taken once
    /// it reaches the default recovery duration, whatever rate the reaction
    /// plays at, if the reaction has not ended first.
    #[serde(default)]
    pub game_elapsed: f64,
}

/// What the clock caches of the tracked state's resolved clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipFacts {
    /// Clip length in seconds.
    pub duration: f32,
    /// The animation state loops its clip.
    pub looping: bool,
    /// A loop that starts at clip time 0, not the drawn phase: a repeating
    /// release's state (`MeshAnimation::loop_phase`).
    pub zero_phase: bool,
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

/// Only the serialized half compares: the cached facts are derived, like a
/// brain's bound guard programs.
impl PartialEq for ClipClock {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state
            && self.time == other.time
            && self.previous_time == other.previous_time
            && self.moment_open == other.moment_open
            && self.moment_timed == other.moment_timed
            && self.unphased == other.unphased
            && self.late_restart == other.late_restart
            && self.mesh_switches == other.mesh_switches
            && self.redraw == other.redraw
            && self.release_elapsed == other.release_elapsed
            && self.release_resolved == other.release_resolved
            && self.release_moments == other.release_moments
            && self.stagger == other.stagger
    }
}

/// A fresh mesh now stands in for the one `id`'s brain drew: a held enemy's
/// drop attaches one drawing its default state, its switch count back at zero.
/// The brain accounts for that count, so only a switch on the fresh mesh is
/// foreign (a flinch before the next AI tick still cancels the swing), and asks
/// the AI to draw its state again. The drop calls this right after attaching
/// the mesh; it does nothing for an entity without a brain.
pub fn note_replaced_mesh(registry: &mut EntityRegistry, id: EntityId) {
    let Ok(mut brain) = registry.get_component::<BrainComponent>(id).cloned() else {
        return;
    };
    brain.clip_clock.mesh_switches = registry
        .get_component::<MeshComponent>(id)
        .ok()
        .and_then(|mesh| mesh.animation.as_ref())
        .map(|animation| animation.switches.get());
    brain.clip_clock.redraw = true;
    let _ = registry.set_component(id, brain);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_clock_serializes_its_clock_latch_gate_and_switch_count_but_not_its_clip_cache() {
        let clock = ClipClock {
            state: Some(Arc::from("walk_left")),
            time: 1.25,
            previous_time: 1.25 - 1.0 / 60.0,
            moment_open: true,
            moment_timed: true,
            unphased: true,
            late_restart: true,
            mesh_switches: Some(7),
            redraw: true,
            release_elapsed: 0.5,
            release_resolved: true,
            release_moments: 2,
            stagger: Some(Stagger {
                elapsed: 0.1,
                until: Some(0.4),
                game_elapsed: 0.05,
            }),
            clip: Some(ClipFacts {
                duration: 2.0,
                looping: true,
                zero_phase: false,
            }),
        };
        let json = serde_json::to_value(&clock).expect("clip clock serializes");
        let object = json.as_object().expect("clip clock is an object");
        assert!(!object.contains_key("clip"));

        let restored: ClipClock = serde_json::from_value(json).expect("clip clock loads");
        assert_eq!(restored, clock, "the serialized half round-trips");
        assert_eq!(restored.mesh_switches, Some(7), "the switch count survives");
        assert_eq!(restored.clip, None, "clip facts rebind lazily");

        let old: ClipClock =
            serde_json::from_value(serde_json::json!({ "state": "walk", "time": 0.5 }))
                .expect("a clock saved before the count was kept loads");
        assert_eq!(old.mesh_switches, None, "and binds to the mesh's count");
        assert!(!old.redraw && !old.late_restart);
        assert!(
            !old.release_resolved && old.release_moments == 0 && old.stagger.is_none(),
            "a clock saved before the release exit holds no gate and no flinch"
        );
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
