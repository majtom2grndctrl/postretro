// Enemy clip clock: game-time clip time for the drawn animation state, the
// clip-timed attack's once-per-entry moment, `brain.animationDone`, and a
// stance release's exit: its moment gate and the flinch that staggers it.
// See: context/lib/entity_model.md §7c (Clip-timed attacks; Stances and
//      attacks, Release exit)

use std::sync::Arc;

use postretro_entities::components::brain::{
    BrainComponent, ClipClock, ClipFacts, Stagger, TravelVariant,
};
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::ActionVerb;
use postretro_foundation::stance_defaults::DEFAULT_RECOVERY_MS;

use super::graph_eval::animation_for_path;
use super::{LocomotionIntent, launch, travel_variants};
use crate::agent_steering;
use crate::scripting_systems::hit_zones::{HitZoneStore, instance_phase};

/// Slack, far below one tick, on every comparison of clip time against a
/// moment or a clip's end. Clip time sums `dt × rate` in floating point, so a
/// moment on a tick boundary must still land on that tick rather than the
/// next. Every test shifts the reached window `(previous, time]` by the same
/// slack, so consecutive ticks' windows stay contiguous.
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
    /// No clip-timed attack on the path, or one its drawn clip cannot time
    /// (the clip did not resolve or lacks the event) while this entry's moment
    /// is unspent. The untimed rule: the attack may propose from its first
    /// dwell tick. A stance release whose moment an earlier clip of this
    /// entry timed reads [`Moment::Pending`] instead.
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
/// back since the last AI tick. The check precedes transitions, so an entry
/// this tick opens its latch afresh after it: a projectile flinch landing
/// before the AI stage never cancels the swing the enemy is only now
/// starting.
///
/// Then the clock advances by `dt` at the rate the previous tick's pose stage
/// set, when the mesh plays the tracked state: the pose stage runs after the
/// AI, so that rate is the one presentation drew the last tick at. The rate is
/// runtime-only on the mesh, so the first tick after a restore advances at 1.
/// A clock tracking nothing binds to the drawn state instead, without
/// advancing. While a dropped enemy's redraw is pending the clock holds; a
/// foreign switch drops the redraw, so the flinch stays on screen.
///
/// A flinch already playing over a release or recover advances first, at the
/// rate presentation drew it; a foreign switch then starts one, or moves a
/// playing one's end to the new reaction's (see [`stagger`]). A reaction
/// state is not one the graph rate-scales (no activity steers on it), so it
/// draws at 1 and the flinch's time, the cap included, runs in game time.
pub(super) fn begin_tick(brain: &mut BrainComponent, source: &ClipSource<'_>, tick_dt: f32) {
    let mesh = source
        .registry
        .get_component::<MeshComponent>(source.id)
        .ok()
        .and_then(|mesh| mesh.animation.as_ref());
    if let Some(stagger) = brain.clip_clock.stagger.as_mut() {
        let rate = mesh.map_or(1.0, |animation| f64::from(animation.rate));
        stagger.elapsed += f64::from(tick_dt.max(0.0)) * rate;
    }
    if let Some(animation) = mesh {
        let switches = animation.switches.get();
        let foreign = match brain.clip_clock.mesh_switches {
            Some(seen) if switches >= seen => switches != seen,
            // Defensive: a count below the one seen is a mesh replaced without
            // the drop's hand-off (`note_replaced_mesh`), which counts from
            // zero, so only the switches it made since are foreign.
            Some(_) => switches != 0,
            // A fresh brain, or one saved before the count was kept: bind.
            None => false,
        };
        if foreign {
            brain.clip_clock.moment_open = false;
            // A flinch on a dropped enemy's fresh mesh wins over its pending
            // redraw: apply leaves the flinch on screen. The brain's own
            // switch back onto the tracked state later still restarts the
            // clock (`redrawn`).
            brain.clip_clock.redraw = false;
            stagger(brain, source, &animation.current_state);
        }
        brain.clip_clock.mesh_switches = Some(switches);
    }

    let Some(state) = brain.clip_clock.state.as_deref() else {
        let presented = brain.presented_travel_variant;
        sync_drawn_state(brain, source, presented);
        return;
    };
    // A dropped enemy's fresh mesh does not draw the tracked clip yet, and the
    // clock is the pre-pickup one: it holds until the redraw restarts it.
    if brain.clip_clock.redraw {
        return;
    }
    let rate = mesh
        .filter(|animation| animation.current_state == state)
        .map_or(1.0, |animation| f64::from(animation.rate));
    let clock = &mut brain.clip_clock;
    // A restart after the last tick's check keeps its window opening below
    // the start, so a moment exactly there is reached now.
    if !std::mem::take(&mut clock.late_restart) {
        clock.previous_time = clock.time;
    }
    let advance = f64::from(tick_dt.max(0.0)) * rate;
    clock.time += advance;
    // Summed across restarts: a variant switch restarts `time`, never this.
    clock.release_elapsed += advance;
    if clock.clip.is_none() {
        clock.clip = clip_facts(source, state_name(clock));
        if clock.unphased
            && let Some(facts) = clock.clip
        {
            // A loop restarted while its model was still loading: move it to
            // the drawn phase once, so its moments sit where the renderer and
            // hit zones draw them.
            clock.unphased = false;
            if facts.looping {
                let phase = f64::from(instance_phase(source.id.to_raw(), facts.duration));
                clock.time += phase;
                clock.previous_time += phase;
            }
        }
    }
}

/// `brain.animationDone`: the drawn non-looping clip reached its duration on
/// an earlier tick, so a moment on its last tick fires before an exit on it.
/// Always false on a loop; true when the clip did not resolve, or the path
/// draws no state. False while a dropped enemy's redraw is pending: the
/// redrawn clip has not started.
pub(super) fn animation_done(clock: &ClipClock, source: &ClipSource<'_>) -> bool {
    let Some(state) = clock.state.as_deref() else {
        return true;
    };
    // Unresolved facts are not cached, because a model may load after the
    // brain binds; they are read again.
    let (looping, duration) = match clock.clip {
        Some(facts) => (facts.looping, Some(facts.duration)),
        None => read_clip(source, state, None)
            .map_or((false, None), |read| (read.looping, read.duration)),
    };
    !looping
        && duration.is_none_or(|duration| {
            !clock.redraw && f64::from(duration) <= clock.previous_time + CLIP_TIME_SLACK
        })
}

/// `brain.animationDone` for the rest of a tick whose path now draws another
/// state than the clock tracks: the value the clock reads once [`settle`]
/// restarts it. A resolved clip starts short of its end, and a loop never
/// reads done; a clip that did not resolve counts as played through. `None`
/// when the drawn state is unchanged.
///
/// An entered path's selectors, and a reseated path's rows, read this rather
/// than the predecessor's clip.
pub(super) fn entered_animation_done(
    brain: &BrainComponent,
    source: &ClipSource<'_>,
) -> Option<bool> {
    let read = match drawn_state(brain, source, brain.presented_travel_variant) {
        None => return brain.clip_clock.state.is_some().then_some(true),
        Some((base, variant)) if brain.clip_clock.tracks(base, variant.suffix()) => return None,
        Some((base, TravelVariant::Base)) => read_clip(source, base, None),
        Some((base, variant)) => read_clip(source, &format!("{base}{}", variant.suffix()), None),
    };
    Some(read.is_none_or(|read| !read.looping && read.duration.is_none()))
}

/// The depth of the activity on the path whose own action fires at a clip
/// event. Read from the path's activities, not the action a selector resolves
/// this tick: a `move` selector resolving a position goal suppresses the action
/// for that tick, and must neither close the latch on entry nor defer the
/// moment past its tick. Only leaves author an action, so the deepest one owns
/// the path's.
fn timed_owner(brain: &BrainComponent) -> Option<usize> {
    let (depth, action) = (0..brain.active_depth()).rev().find_map(|depth| {
        brain
            .activity_at_depth(depth)
            .and_then(|(_, activity)| activity.action.as_ref())
            .map(|action| (depth, action))
    })?;
    matches!(action, ActionVerb::Attack { at: Some(_), .. }).then_some(depth)
}

/// The Attack block's clip work, after transitions and heading: the moment
/// latch opens on its activity's entry; a knockback launch closes it; the
/// clock restarts when the drawn state changed; then the moment is read, and
/// reaching it spends the latch whether or not this tick's gates pass or a
/// selector resolves the action.
pub(super) fn settle(
    brain: &mut BrainComponent,
    source: &ClipSource<'_>,
    entry_depth: Option<usize>,
    travel_variant: TravelVariant,
) -> Moment {
    let owner = timed_owner(brain);
    let clock = &mut brain.clip_clock;
    match (entry_depth, owner) {
        (Some(entry), Some(owner)) if entry <= owner => {
            clock.moment_open = true;
            clock.moment_timed = false;
        }
        (Some(_), None) => {
            clock.moment_open = false;
            clock.moment_timed = false;
        }
        _ => {}
    }
    // Whatever clip shows, a launched swing never connects, including one
    // entered mid-air after drag spent the knockback.
    if launch::launched(&brain.launch) {
        brain.clip_clock.moment_open = false;
    }
    sync_drawn_state(brain, source, travel_variant);
    let gate = release_gate(brain);

    let Some(owner) = owner else {
        // No moment: the release lasts one pass of its clip.
        if gate.is_some() {
            resolve_on_pass(&mut brain.clip_clock);
        }
        return Moment::Untimed;
    };
    let timing = event_timing(brain, source, owner);
    let defaulted = brain
        .activity_at_depth(owner)
        .and_then(|(_, activity)| activity.action.as_ref())
        .is_some_and(|ActionVerb::Attack { at_default, .. }| *at_default);
    let gated = gate == Some(owner);
    let clock = &mut brain.clip_clock;
    let Some((event, facts)) = timing else {
        if gated && clock.moment_timed {
            // A release's moment an earlier clip of this entry timed (or one
            // resolved on its entry tick) stays pending on a later clip that
            // cannot time it, such as a travel variant's lacking the event:
            // it never fires mid-swing as without `at`. Unreached within one
            // summed pass of its clip, it whiffs.
            if clock.moment_open && played_one_pass(clock) {
                clock.moment_open = false;
            }
            clock.release_resolved = !clock.moment_open;
            return Moment::Pending;
        }
        // A clip that cannot time the moment fires as without `at`, unless
        // this entry already spent a moment a clip timed.
        let moment = if clock.moment_timed && !clock.moment_open {
            Moment::Pending
        } else {
            Moment::Untimed
        };
        if gated && defaulted {
            // A defaulted `hit` the clip lacks is no moment at all: the
            // attack fires on its first passing tick while the release
            // lasts its clip.
            resolve_on_pass(clock);
        } else if gated {
            // A named moment its clip cannot time resolves on this, the
            // release's entry tick: the attack fires now if its gates pass,
            // and nothing fires later in the entry.
            clock.moment_timed = true;
            clock.moment_open = false;
            clock.release_resolved = true;
        }
        return moment;
    };
    if clock.redraw {
        // The clock holds the pre-pickup clip time until the redraw restarts
        // it, so no moment is reached on it and the latch stays as it is.
        return Moment::Pending;
    }
    clock.moment_timed = true;
    let moment =
        if clock.moment_open && reached(clock.previous_time, clock.time, f64::from(event), facts) {
            clock.moment_open = false;
            Moment::Reached
        } else {
            Moment::Pending
        };
    if gated {
        // Every moment resolves: one unreached within a pass of its clip
        // since entry is a whiff, as a montage ending without its hit frame.
        if clock.moment_open && played_one_pass(clock) {
            clock.moment_open = false;
        }
        clock.release_resolved = !clock.moment_open;
    }
    moment
}

/// The depth of the path's leaf when its own rows wait on its moment
/// (`afterMoment`): a stance release. Only a leaf's rows may carry the gate,
/// so the leaf is the one place to look.
fn release_gate(brain: &BrainComponent) -> Option<usize> {
    let depth = brain.active_depth().checked_sub(1)?;
    let (name, _) = brain.activity_at_depth(depth)?;
    brain
        .envelope_at_depth(depth)?
        .transitions
        .get(name)?
        .iter()
        .any(|row| row.after_moment)
        .then_some(depth)
}

/// Resolve a release with no moment once its clip has played one pass since
/// entry, in clip time summed across variant restarts. A clip that did not
/// resolve, or no drawn state, counts as played through, as
/// `animationDone` reads it.
fn resolve_on_pass(clock: &mut ClipClock) {
    if played_one_pass(clock) {
        clock.release_resolved = true;
    }
}

fn played_one_pass(clock: &ClipClock) -> bool {
    clock
        .clip
        .is_none_or(|facts| clock.release_elapsed + CLIP_TIME_SLACK >= f64::from(facts.duration))
}

/// Start or extend a flinch: a switch the brain did not make, while an
/// activity on the path routes flinches (`staggerTo`). The reaction now on
/// the mesh plays through, timed from its resolved clip's duration; a loop,
/// or a clip that did not resolve, takes the cap. The cap, the default
/// recovery duration, counts from the first switch, so a later flinch can
/// move the end to its own reaction's but never past it.
///
/// The compute pass reads this before transitions, so a switch landing before
/// the AI pass on the tick a release is entered starts nothing: the path is
/// still the hold's.
fn stagger(brain: &mut BrainComponent, source: &ClipSource<'_>, reaction_state: &str) {
    if !routes_flinches(brain) {
        return;
    }
    let cap = f64::from(DEFAULT_RECOVERY_MS) / 1000.0;
    let reaction = read_clip(source, reaction_state, None)
        .filter(|read| !read.looping)
        .and_then(|read| read.duration)
        .map_or(cap, f64::from);
    let stagger = brain.clip_clock.stagger.get_or_insert(Stagger::default());
    stagger.until = (stagger.elapsed + reaction).min(cap);
}

/// An activity on the path routes an impact flinch (`staggerTo`).
pub(super) fn routes_flinches(brain: &BrainComponent) -> bool {
    (0..brain.active_depth()).any(|depth| {
        brain
            .activity_at_depth(depth)
            .is_some_and(|(_, activity)| activity.stagger_to.is_some())
    })
}

/// The flinch has played out: its route is taken on this evaluation.
pub(super) fn stagger_due(clock: &ClipClock) -> bool {
    clock
        .stagger
        .is_some_and(|stagger| stagger.elapsed + CLIP_TIME_SLACK >= stagger.until)
}

/// The brain's own switch drew the tracked state again after the mesh had left
/// it (an impact effect, a reaction, the launch's airborne clip, a dropped
/// enemy's fresh mesh). The drawn clip starts over, so clip time does too, as
/// on a change of state. The latch is untouched: a restart reopens no moment.
///
/// Apply draws after this tick's moment check, so the next tick's window opens
/// just below the start, as a restart in compute's own check does.
pub(super) fn redrawn(clock: &mut ClipClock, id: EntityId) {
    let facts = clock.clip;
    restart(clock, id, facts);
    clock.late_restart = true;
}

/// The owner's event time in the drawn state's resolved clip, with the clip's
/// facts. `None` when the clip did not resolve or lacks the event.
fn event_timing(
    brain: &BrainComponent,
    source: &ClipSource<'_>,
    owner: usize,
) -> Option<(f32, ClipFacts)> {
    let (_, activity) = brain.activity_at_depth(owner)?;
    let Some(ActionVerb::Attack {
        at: Some(event), ..
    }) = activity.action.as_ref()
    else {
        return None;
    };
    let read = read_clip(
        source,
        brain.clip_clock.state.as_deref()?,
        Some(event.as_str()),
    )?;
    Some((read.event?, read.facts()?))
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
/// shows, as apply requests them. A different state restarts the clock. The
/// same state keeps its time, so activities sharing a state share its clip.
fn sync_drawn_state(brain: &mut BrainComponent, source: &ClipSource<'_>, wanted: TravelVariant) {
    let name: Option<Arc<str>> = match drawn_state(brain, source, wanted) {
        None => None,
        Some((base, variant)) if brain.clip_clock.tracks(base, variant.suffix()) => return,
        Some((base, TravelVariant::Base)) => Some(Arc::from(base)),
        Some((base, variant)) => Some(Arc::from(format!("{base}{}", variant.suffix()))),
    };
    let clock = &mut brain.clip_clock;
    let Some(name) = name else {
        clock.state = None;
        clock.clip = None;
        return;
    };
    let facts = clip_facts(source, &name);
    clock.state = Some(name);
    restart(clock, source.id, facts);
}

/// Start the tracked state's clip over: at 0 for a one-shot, at the enemy's
/// drawn phase on a loop (the seed its renderer and hit zones use), the window
/// opening just below the start so a moment exactly there is reached on the
/// restart tick. A clip not yet resolved starts at 0 and owes its phase.
fn restart(clock: &mut ClipClock, id: EntityId, facts: Option<ClipFacts>) {
    let start = match facts {
        Some(ClipFacts {
            duration,
            looping: true,
        }) => f64::from(instance_phase(id.to_raw(), duration)),
        _ => 0.0,
    };
    clock.clip = facts;
    clock.unphased = facts.is_none();
    clock.time = start;
    clock.previous_time = start - 2.0 * CLIP_TIME_SLACK;
    clock.late_restart = false;
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
/// travel variant of it the mesh can draw. The attack then fires as without
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
            // Read by index or by name, so a runtime spawn bound before its
            // indices fill is checked as it will draw.
            let clip = clips
                .get_by_name(&mesh.model)
                .and_then(|model| model.state_clip(declared.clip_index, &declared.clip));
            let problem = match clip {
                // A variant whose clip does not resolve is never drawn: the
                // base clip plays in its place, as `travel_variants::presentable`
                // picks.
                None if variant != TravelVariant::Base => continue,
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
        // A defaulted `at` (a stance release's `hit`) binds silently: a clip
        // without it fires as `at: false` by design.
        if let (
            Some(ActionVerb::Attack {
                at: Some(event),
                at_default: false,
                ..
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
