// Enemy clip clock: game-time clip time for the drawn animation state, the
// clip-timed attack's once-per-entry moment, and `brain.animationDone`.
// See: context/lib/entity_model.md §7c (Clip-timed attacks)

use std::sync::Arc;

use postretro_entities::components::brain::{BrainComponent, ClipClock, ClipFacts, TravelVariant};
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::ActionVerb;

use super::graph_eval::animation_for_path;
use super::{LocomotionIntent, launch, travel_variants};
use crate::agent_steering;
use crate::scripting_systems::hit_zones::{HitZoneStore, instance_phase};

/// Slack, far below one tick, on every comparison of clip time against a
/// moment or a clip's end. Clip time sums `dt × rate` in floating point, so a
/// moment on a tick boundary must still land on that tick (P1). Every test
/// shifts the reached window `(previous, time]` by the same slack, so
/// consecutive ticks' windows stay contiguous.
const CLIP_TIME_SLACK: f64 = 1.0e-4;

/// Where the clock reads an enemy's clips: its mesh's declared states and the
/// loaded model's clips.
#[derive(Clone, Copy)]
pub(super) struct ClipSource<'a> {
    pub(super) registry: &'a EntityRegistry,
    pub(super) clips: Option<&'a HitZoneStore>,
    pub(super) id: EntityId,
}

/// Whether the active path's attack may form a proposal this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Moment {
    /// No clip-timed attack on the path, or its event cannot time it (a clip
    /// that did not resolve or lacks the event): today's first-dwell-tick rule.
    Untimed,
    /// The moment falls on this tick and its once-per-entry latch was open.
    Reached,
    /// Before the moment, or this entry's one chance is spent.
    Pending,
}

impl Moment {
    pub(super) fn allows_proposal(self) -> bool {
        !matches!(self, Moment::Pending)
    }
}

/// The top of an enemy's compute tick, before any guard reads the clock.
///
/// A switch of the mesh that the brain's own apply did not make closes the
/// pending moment (an impact effect, a reaction), even one switched off and
/// back since the last AI tick (P8). The check precedes transitions, so an
/// entry this tick opens its latch afresh after it (P6).
///
/// Then the clock advances by `dt` at the rate the previous tick's pose stage
/// set, when the mesh plays the tracked state (P2). A clock tracking nothing
/// binds to the drawn state instead, without advancing.
pub(super) fn begin_tick(brain: &mut BrainComponent, source: &ClipSource<'_>, tick_dt: f32) {
    let mesh = source
        .registry
        .get_component::<MeshComponent>(source.id)
        .ok()
        .and_then(|mesh| mesh.animation.as_ref());
    if let Some(animation) = mesh {
        let switches = animation.switches.get();
        if brain
            .clip_clock
            .mesh_switches
            .is_some_and(|seen| seen != switches)
        {
            brain.clip_clock.moment_open = false;
        }
        brain.clip_clock.mesh_switches = Some(switches);
    }

    let Some(state) = brain.clip_clock.state.as_deref() else {
        let presented = brain.presented_travel_variant;
        sync_drawn_state(brain, source, presented);
        return;
    };
    let rate = mesh
        .filter(|animation| animation.current_state == state)
        .map_or(1.0, |animation| f64::from(animation.rate));
    let clock = &mut brain.clip_clock;
    clock.previous_time = clock.time;
    clock.time += f64::from(tick_dt.max(0.0)) * rate;
    if clock.clip.is_none() {
        clock.clip = clip_facts(source, state_name(clock));
    }
}

/// `brain.animationDone`: the drawn non-looping clip reached its duration on
/// an earlier tick, so a moment on its last tick fires before an exit on it
/// (P4). Always false on a loop; true when the clip did not resolve, or the
/// path draws no state.
pub(super) fn animation_done(clock: &ClipClock, source: &ClipSource<'_>) -> bool {
    let Some(state) = clock.state.as_deref() else {
        return true;
    };
    // Unresolved facts are not cached, so they are read again.
    let (looping, duration) = match clock.clip {
        Some(facts) => (facts.looping, Some(facts.duration)),
        None => read_clip(source, state, None)
            .map_or((false, None), |read| (read.looping, read.duration)),
    };
    !looping
        && duration
            .is_none_or(|duration| f64::from(duration) <= clock.previous_time + CLIP_TIME_SLACK)
}

/// The depth of the activity whose own action is `action`, when it fires at a
/// clip event. Load rejects `at` on a selector row, so an `at` action always
/// belongs to an activity on the path; identity, not equality, finds it.
pub(super) fn at_owner(brain: &BrainComponent, action: &ActionVerb) -> Option<usize> {
    let ActionVerb::Attack { at: Some(_), .. } = action else {
        return None;
    };
    (0..brain.active_depth()).rev().find(|&depth| {
        brain
            .activity_at_depth(depth)
            .and_then(|(_, activity)| activity.action.as_ref())
            .is_some_and(|own| std::ptr::eq(own, action))
    })
}

/// The Attack block's clip work, after transitions and heading: the moment
/// latch opens on its activity's entry; a knockback launch closes it (P15);
/// the clock restarts when the drawn state changed; then the moment is read,
/// and reaching it spends the latch whether or not this tick's gates pass.
pub(super) fn settle(
    brain: &mut BrainComponent,
    source: &ClipSource<'_>,
    entry_depth: Option<usize>,
    at_owner: Option<usize>,
    travel_variant: TravelVariant,
) -> Moment {
    match (entry_depth, at_owner) {
        (Some(entry), Some(owner)) if entry <= owner => brain.clip_clock.moment_open = true,
        (Some(_), None) => brain.clip_clock.moment_open = false,
        _ => {}
    }
    // Launched means knockback velocity remains: whatever clip shows, the
    // swing never connects.
    if brain.launch.impact_yaw.is_some() {
        brain.clip_clock.moment_open = false;
    }
    sync_drawn_state(brain, source, travel_variant);

    let moment = moment(brain, source, at_owner);
    if moment == Moment::Reached {
        brain.clip_clock.moment_open = false;
    }
    moment
}

fn moment(brain: &BrainComponent, source: &ClipSource<'_>, at_owner: Option<usize>) -> Moment {
    let Some(event) = at_owner
        .and_then(|owner| brain.activity_at_depth(owner))
        .and_then(|(_, activity)| match activity.action.as_ref() {
            Some(ActionVerb::Attack {
                at: Some(event), ..
            }) => Some(event.as_str()),
            _ => None,
        })
    else {
        return Moment::Untimed;
    };
    let clock = &brain.clip_clock;
    let Some((event_time, facts)) = clock
        .state
        .as_deref()
        .and_then(|state| event_time(source, state, event))
    else {
        return Moment::Untimed;
    };
    if clock.moment_open
        && reached(
            clock.previous_time,
            clock.time,
            f64::from(event_time),
            facts,
        )
    {
        Moment::Reached
    } else {
        Moment::Pending
    }
}

/// Whether a moment at `event` seconds falls in this tick's window. A loop
/// tests the first pass after the window opens; clip time never wraps.
fn reached(previous: f64, time: f64, event: f64, facts: ClipFacts) -> bool {
    let (open, close) = (previous + CLIP_TIME_SLACK, time + CLIP_TIME_SLACK);
    let duration = f64::from(facts.duration);
    let pass = if facts.looping && duration > 0.0 {
        event + (((open - event) / duration).floor() + 1.0) * duration
    } else {
        event
    };
    open < pass && pass <= close
}

/// Track the drawn state: the path's animation state with the travel variant
/// the mesh can present, or the base state while an airborne launch clip
/// shows, as apply requests them. A different state restarts the clock: at 0
/// for a one-shot, at the enemy's drawn phase on a loop (the seed its renderer
/// and hit zones use), the window opening just below the start so a moment
/// exactly there is reached on the restart tick. The same state keeps its
/// time, so activities sharing a state share its clip.
fn sync_drawn_state(brain: &mut BrainComponent, source: &ClipSource<'_>, wanted: TravelVariant) {
    let mut clock = std::mem::take(&mut brain.clip_clock);
    match drawn_state(brain, source, wanted) {
        None => {
            clock.state = None;
            clock.clip = None;
        }
        Some((base, variant)) if !clock.tracks(base, variant.suffix()) => {
            let name: Arc<str> = match variant {
                TravelVariant::Base => Arc::from(base),
                _ => Arc::from(format!("{base}{}", variant.suffix())),
            };
            let facts = clip_facts(source, &name);
            let start = match facts {
                Some(ClipFacts {
                    duration,
                    looping: true,
                }) => f64::from(instance_phase(source.id.to_raw(), duration)),
                _ => 0.0,
            };
            clock.clip = facts;
            clock.state = Some(name);
            clock.time = start;
            clock.previous_time = start - 2.0 * CLIP_TIME_SLACK;
        }
        Some(_) => {}
    }
    brain.clip_clock = clock;
}

fn drawn_state<'b>(
    brain: &'b BrainComponent,
    source: &ClipSource<'_>,
    wanted: TravelVariant,
) -> Option<(&'b str, TravelVariant)> {
    // The stride apply reads: steering velocity alone, behind the aggro gate.
    let moving = brain.aggro_armed
        && agent_steering::path_state(source.registry, source.id)
            .is_some_and(|path| LocomotionIntent::from_velocity(path.steering_velocity()).moving);
    let base = animation_for_path(brain, moving)?;
    let variant = if launch::airborne_clip(source.registry, source.id, &brain.launch).is_some() {
        TravelVariant::Base
    } else {
        travel_variants::presentable(source.registry, source.id, Some(base), wanted)
    };
    Some((base, variant))
}

fn state_name(clock: &ClipClock) -> &str {
    clock.state.as_deref().unwrap_or_default()
}

/// What the clock reads of a drawn state's clip.
#[derive(Clone, Copy)]
struct ClipRead {
    /// The mesh state loops its clip.
    looping: bool,
    /// The clip's length, when it resolves.
    duration: Option<f32>,
    /// The asked-for event's time, when the resolved clip carries it.
    event: Option<f32>,
}

impl ClipRead {
    fn facts(self) -> Option<ClipFacts> {
        self.duration.map(|duration| ClipFacts {
            duration,
            looping: self.looping,
        })
    }
}

/// Read `state`'s clip through the lookup the playback-rate path uses: the
/// mesh's declared state, then the model's clip by index or name. `None` when
/// the mesh does not declare the state.
fn read_clip(source: &ClipSource<'_>, state: &str, event: Option<&str>) -> Option<ClipRead> {
    let mesh = source
        .registry
        .get_component::<MeshComponent>(source.id)
        .ok()?;
    let declared = mesh.animation.as_ref()?.states.get(state)?;
    let clip = source
        .clips
        .and_then(|store| store.get_by_name(&mesh.model))
        .and_then(|model| model.state_clip(declared.clip_index, &declared.clip));
    Some(ClipRead {
        looping: declared.looping,
        duration: clip.map(|clip| clip.duration),
        event: clip
            .zip(event)
            .and_then(|(clip, event)| clip.event_time(event)),
    })
}

fn clip_facts(source: &ClipSource<'_>, state: &str) -> Option<ClipFacts> {
    read_clip(source, state, None).and_then(ClipRead::facts)
}

/// The time of `event` in the drawn state's resolved clip, with the clip's
/// facts. `None` when the clip did not resolve or lacks the event.
fn event_time(source: &ClipSource<'_>, state: &str, event: &str) -> Option<(f32, ClipFacts)> {
    let read = read_clip(source, state, Some(event))?;
    Some((read.event?, read.facts()?))
}

/// Prefix of the warn-once keys for a clip-timed attack whose clip cannot time
/// it, keyed by entity class, drawn state and event: content, not entities.
const UNCLIPPED_MOMENT_WARNING: &str = "clip-event:";

/// Whether `entity`'s clips have bound, so its clip-timed attacks can be
/// checked: its mesh's model is loaded, or it has no animated mesh to wait on.
pub(super) fn clips_bound(
    registry: &EntityRegistry,
    clips: &HitZoneStore,
    entity: EntityId,
) -> bool {
    registry
        .get_component::<MeshComponent>(entity)
        .ok()
        .filter(|mesh| mesh.animation.is_some())
        .is_none_or(|mesh| clips.get_by_name(&mesh.model).is_some())
}

/// Warn once per entity class and drawn state for every clip-timed attack its
/// clip cannot time: the clip did not resolve, or lacks the event. Checks each
/// activity whose own action fires `at` an event, in its own state and every
/// travel variant of it the mesh declares. The attack then fires as without
/// `at`. A state the mesh does not declare is spawn validation's warning.
pub(super) fn warn_unclipped_moments(
    graph: &postretro_foundation::BehaviorGraphDescriptor,
    registry: &EntityRegistry,
    clips: &HitZoneStore,
    entity: EntityId,
    warned: &mut std::collections::HashSet<String>,
) {
    let mut timed = Vec::new();
    collect_timed_activities(&graph.envelope, &mut timed);
    if timed.is_empty() {
        return;
    }
    let mesh = registry.get_component::<MeshComponent>(entity).ok();
    let class = registry
        .get_component::<postretro_entities::provenance::DescriptorProvenance>(entity)
        .map(|provenance| provenance.canonical_name.as_str())
        .ok()
        .or(mesh.map(|mesh| mesh.model.as_str()))
        .unwrap_or("<unnamed>");
    let states = mesh.and_then(|mesh| mesh.animation.as_ref().map(|animation| (mesh, animation)));
    for (base, event) in timed {
        for variant in TravelVariant::ALL {
            let name = format!("{base}{}", variant.suffix());
            let Some((mesh, declared)) =
                states.and_then(|(mesh, animation)| Some((mesh, animation.states.get(&name)?)))
            else {
                continue;
            };
            let clip = clips
                .get_by_name(&mesh.model)
                .and_then(|model| model.state_clip(declared.clip_index, &declared.clip));
            let problem = match clip {
                None => "did not resolve",
                Some(clip) if clip.event_time(event).is_none() => "has no such event",
                Some(_) => continue,
            };
            if warned.insert(format!("{UNCLIPPED_MOMENT_WARNING}{class}:{name}:{event}")) {
                log::warn!(
                    "[AI] `{class}` animation state `{name}`: clip `{clip}` {problem}, so attack \
                     event `{event}` cannot time it; the attack fires as without `at`. Warned \
                     once per entity class and state.",
                    clip = declared.clip,
                );
            }
        }
    }
}

/// Every activity, at any depth, whose own action fires at a clip event: its
/// animation state and the event.
fn collect_timed_activities<'a>(
    envelope: &'a postretro_foundation::BehaviorGraphEnvelope,
    timed: &mut Vec<(&'a str, &'a str)>,
) {
    for activity in envelope.activities.values() {
        if let (
            Some(ActionVerb::Attack {
                at: Some(event), ..
            }),
            Some(state),
        ) = (activity.action.as_ref(), activity.animation.as_deref())
        {
            timed.push((state, event));
        }
        for layer in activity.layers.values() {
            if let postretro_foundation::BehaviorLayerDescriptor::Graph(child) = layer {
                collect_timed_activities(child, timed);
            }
        }
    }
}
