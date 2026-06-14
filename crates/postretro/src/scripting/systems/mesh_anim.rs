// Game-side mesh-animation data logic: clip tables, per-instance sample times,
// crossfade weights, and smooth-interrupt capture instructions (GPU-free).
// See: context/lib/scripting.md §10.3 (Mesh Animation) · rendering_pipeline.md §9

use std::collections::HashMap;

use crate::model::ModelHandle;
use crate::model::anim::Loop;
use crate::model::sample_params::{
    CaptureInstruction, ClipSample, FadeSource, MeshFade, MeshSampleParams,
};
use crate::scripting::components::mesh::{
    AnimationState, FadeSourceKind, InterruptedOutgoing, MeshAnimation,
};

/// One model's clip table: authored clip name → glTF index, plus each clip's
/// duration by index (parallel to the glTF clip list). Built at level load from
/// the renderer's [`crate::render::mesh_pass::ClipMetadata`] and used both to
/// resolve a state's `clip_index` and to read a clip's duration for the
/// state-elapsed completion query.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ModelClipTable {
    /// Clip name → glTF index. First authored clip wins on a duplicate name
    /// (mirrors the cache-side `clip_by_name` rule).
    by_name: HashMap<String, usize>,
    /// Clip duration (seconds) by glTF index, parallel to the model's clip list.
    durations: Vec<f32>,
}

impl ModelClipTable {
    /// Build a clip table from the renderer's clip metadata (glTF index order).
    /// A duplicate authored name keeps the FIRST occurrence (lowest glTF index),
    /// matching the cache-side `clip_by_name` contract.
    pub(crate) fn from_metadata(meta: &[crate::render::mesh_pass::ClipMetadata]) -> Self {
        let mut by_name = HashMap::with_capacity(meta.len());
        let mut durations = Vec::with_capacity(meta.len());
        for (index, clip) in meta.iter().enumerate() {
            by_name.entry(clip.name.clone()).or_insert(index);
            durations.push(clip.duration);
        }
        Self { by_name, durations }
    }

    /// Resolve a clip name to its glTF index, or `None` if the model carries no
    /// clip of that name.
    pub(crate) fn index_of(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    /// Duration (seconds) of the clip at `index`, or `None` if out of range.
    pub(crate) fn duration(&self, index: usize) -> Option<f32> {
        self.durations.get(index).copied()
    }
}

/// All models' clip tables, keyed by handle. Owned beside the mesh collector at
/// the game layer (`main.rs`); built at the level-load model sweep from each
/// uploaded model's clip metadata and consulted per frame to compute sample
/// params and answer the state-elapsed query.
#[derive(Debug, Clone, Default)]
pub(crate) struct MeshClipTables {
    tables: HashMap<ModelHandle, ModelClipTable>,
}

impl MeshClipTables {
    pub(crate) fn new() -> Self {
        Self {
            tables: HashMap::new(),
        }
    }

    /// Drop every table — called by the level-load clear so a new level starts
    /// from an empty table set (mirrors the renderer's model-cache clear).
    pub(crate) fn clear(&mut self) {
        self.tables.clear();
    }

    /// Install (or replace) a model's clip table from its renderer-side clip
    /// metadata. Idempotent re-install replaces the entry.
    pub(crate) fn insert(
        &mut self,
        handle: ModelHandle,
        meta: &[crate::render::mesh_pass::ClipMetadata],
    ) {
        self.tables
            .insert(handle, ModelClipTable::from_metadata(meta));
    }

    /// The clip table for a model handle, or `None` if the model never uploaded.
    pub(crate) fn get(&self, handle: &ModelHandle) -> Option<&ModelClipTable> {
        self.tables.get(handle)
    }
}

/// Resolve a state map's `clip_index` fields against a model's clip table.
/// Returns the names of states whose clip is absent from the model (so the
/// caller can warn once at level load). A state naming a missing clip keeps
/// `clip_index = None` (unusable — `switch_animation_state` warns + no-ops).
///
/// Pure data logic over the component's state map; the caller (level-load
/// validation in `main.rs`) writes the mutated component back.
pub(crate) fn resolve_state_clips(
    states: &mut HashMap<String, AnimationState>,
    table: &ModelClipTable,
) -> Vec<MissingClip> {
    let mut missing = Vec::new();
    for (state_name, state) in states.iter_mut() {
        match table.index_of(&state.clip) {
            Some(index) => state.clip_index = Some(index),
            None => {
                state.clip_index = None;
                missing.push(MissingClip {
                    state: state_name.clone(),
                    clip: state.clip.clone(),
                });
            }
        }
    }
    missing
}

/// A state whose declared clip is absent from its model — surfaced by
/// [`resolve_state_clips`] so the level-load validation can warn once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MissingClip {
    /// The declaring state's name.
    pub(crate) state: String,
    /// The clip name that did not resolve.
    pub(crate) clip: String,
}

/// Convert a crossfade duration in milliseconds to seconds, treating `0` (or any
/// non-positive value) as a hard cut.
fn crossfade_seconds(crossfade_ms: f32) -> f32 {
    (crossfade_ms / 1000.0).max(0.0)
}

/// Pack an entry stamp's bit pattern into a snapshot-store tag. Quantizing to the
/// raw `f64` bits makes a re-emitted capture under a frozen clock compare equal
/// (idempotent capture): the same `entered_at` yields the same tag.
fn snapshot_tag(entered_at: f64) -> crate::model::sample_params::SnapshotTag {
    entered_at.to_bits()
}

/// The clip-local sample time for a state: `anim_time - entered_at`, plus a
/// per-instance phase for LOOPING states (de-syncs a wave) and no phase for
/// one-shot states (plays from entry, synced to the triggering event).
fn state_time(state: &AnimationState, entered_at: f64, anim_time: f64, phase: f32) -> f32 {
    let elapsed = (anim_time - entered_at) as f32;
    if state.looping {
        elapsed + phase
    } else {
        elapsed
    }
}

/// Build a [`ClipSample`] leg for a resolved state at its entry stamp.
fn clip_sample(
    state: &AnimationState,
    entered_at: f64,
    anim_time: f64,
    phase: f32,
) -> Option<ClipSample> {
    let clip_index = state.clip_index?;
    Some(ClipSample {
        clip_index,
        time: state_time(state, entered_at, anim_time, phase),
        loop_policy: loop_policy(state),
    })
}

/// The loop policy for a state: `Wrap` for looping, `Clamp` for one-shot.
fn loop_policy(state: &AnimationState) -> Loop {
    if state.looping {
        Loop::Wrap
    } else {
        Loop::Clamp
    }
}

/// The per-instance animation result the collector emits for one entity: the
/// resolved sample params plus an optional one-time capture instruction.
pub(crate) struct AnimResult {
    pub(crate) sample: MeshSampleParams,
    pub(crate) capture: Option<CaptureInstruction>,
}

/// Compute one animated entity's sample params + capture instruction from its
/// `MeshAnimation` state, the frame's animation clock, and the per-instance
/// phase. Returns `None` when the current state is unresolved (no usable clip)
/// — the caller falls back to the bind pose.
///
/// The phase is the per-instance looping-wave de-sync; one-shot states ignore it
/// (they play from entry, synced to the triggering event).
pub(crate) fn animate_entity(
    anim: &MeshAnimation,
    anim_time: f64,
    phase: f32,
) -> Option<AnimResult> {
    let current = anim.states.get(&anim.current_state)?;
    // A pending current stamp reads as just-entered (elapsed 0). The resolve
    // pass fills it before the collector runs, but guard anyway: an unfilled
    // stamp produces a zero-elapsed pose with no fade.
    let entered_at = anim.entered_at.unwrap_or(anim_time);
    let primary = clip_sample(current, entered_at, anim_time, phase)?;

    // Is a crossfade active? Only when a previous state is recorded AND its
    // stamp resolved AND the current entry stamp resolved (a still-pending
    // stamp contributes no fade — the same-tick collapse semantics).
    let fade = active_fade(anim, anim_time, phase, entered_at, current);

    // A `"smooth"` interrupt emits a one-time capture instruction when the
    // entered state's interrupt policy is Smooth AND a fade is active (the
    // resolve pass recorded `FadeSourceKind::Snapshot`). The capture freezes the
    // in-flight blended pose `S` sampled at the FROZEN interrupt instant, so it
    // is idempotent across re-emission and reconstructable on a late first frame.
    let capture = build_capture(anim, phase, &fade);

    Some(AnimResult {
        sample: MeshSampleParams { primary, fade },
        capture,
    })
}

/// Resolve the active crossfade leg, or `None` when no fade is in flight (no
/// previous state, a pending previous stamp, or the fade window has elapsed).
fn active_fade(
    anim: &MeshAnimation,
    anim_time: f64,
    phase: f32,
    entered_at: f64,
    current: &AnimationState,
) -> Option<MeshFade> {
    // A still-pending current stamp contributes no fade.
    anim.entered_at?;

    let crossfade = crossfade_seconds(current.crossfade_ms);
    // Hard cut: zero-length window → no fade leg, primary plays alone.
    if crossfade <= 0.0 {
        return None;
    }
    let elapsed = (anim_time - entered_at) as f32;
    let weight = (elapsed / crossfade).clamp(0.0, 1.0);
    // Fade window closed → steady state, one clip sample, no fade.
    if weight >= 1.0 {
        return None;
    }

    let from = fade_from_source(anim, anim_time, phase)?;
    Some(MeshFade { from, weight })
}

/// Resolve what the active fade blends *out of*: a snapshot reference (with its
/// fallback clip leg) for a recorded `"smooth"` snapshot fade, else the outgoing
/// state's clip leg on its own advanced timeline (`"snap"` and normal fades).
/// Returns `None` if the outgoing state is unresolved or its stamp is pending.
fn fade_from_source(anim: &MeshAnimation, anim_time: f64, phase: f32) -> Option<FadeSource> {
    let prev_name = anim.previous_state.as_ref()?;
    let prev = anim.states.get(prev_name)?;
    let prev_entered = anim.previous_entered_at?;
    // The outgoing clip leg: advances on its OWN timeline from its own stamp.
    // A non-looping outgoing clip clamps (Loop::Clamp via loop_policy).
    let outgoing = clip_sample(prev, prev_entered, anim_time, phase)?;

    match anim.fade_source {
        FadeSourceKind::Snapshot => {
            // The snapshot's tag is the ENTERED state's entry stamp (the capture
            // was tagged with it). The fallback is the outgoing clip leg — the
            // SAME pair a `"snap"` would have used, so a missed capture degrades
            // cleanly.
            let tag = snapshot_tag(anim.entered_at?);
            Some(FadeSource::Snapshot {
                tag,
                fallback: outgoing,
            })
        }
        FadeSourceKind::Clip => Some(FadeSource::Clip(outgoing)),
    }
}

/// Emit the one-time snapshot-capture instruction for a `"smooth"` interrupt
/// frame, or `None` when no capture is due. A capture is due when the entered
/// state recorded a snapshot fade source (`FadeSourceKind::Snapshot`) and a fade
/// is active. The pass evaluates the in-flight blended pose `S` into the store,
/// tagged by the entered (NEW) stamp — idempotent on re-emission, a fresh entry
/// that supersedes the prior one.
///
/// `S` is the exact pose the entity showed at the interrupt instant `t2`
/// (= `entered_at`), namely `blend(OUT@t2, IN@t2, w_interrupted)`, where the
/// interrupted fade was OUT→IN:
/// - `IN` = the state that was current before the switch — now `previous_state` —
///   sampled at `t2` on its own timeline from `previous_entered_at`,
/// - `OUT` = the leg the interrupted fade was blending *out of*, carried across
///   the switch in `interrupted_outgoing` (a clip leg sampled at `t2` on its own
///   timeline, or a reference to the prior snapshot for a chained interrupt),
/// - `w_interrupted` = `((t2 - in_stamp) / IN.crossfade_seconds).clamp(0,1)`.
///
/// All legs are sampled at the FROZEN `entered_at`, not the moving `anim_time`.
/// That makes the capture idempotent and recomputable: a frame whose capture was
/// culled, re-evaluated on the first planned frame, reconstructs the SAME `S`
/// (it is the interrupt-instant pose by construction, independent of how far the
/// clock has since advanced). The pass's tag check then no-ops every later
/// re-emission.
///
/// The chained-snapshot case: when `OUT` references a prior snapshot, the store
/// hit/miss disambiguates uniformly — a HIT freezes `blend(prior_snapshot, IN)`
/// (no discontinuity even over a snapshot source); a MISS (the prior capture
/// frame was culled) degrades to the carried fallback clip leg, exactly as the
/// sampling miss path does.
fn build_capture(
    anim: &MeshAnimation,
    phase: f32,
    fade: &Option<MeshFade>,
) -> Option<CaptureInstruction> {
    if anim.fade_source != FadeSourceKind::Snapshot {
        return None;
    }
    // No fade in flight → nothing to capture (window closed or hard cut).
    fade.as_ref()?;

    // The interrupt instant: the entered (NEW) state's frozen entry stamp.
    let t2 = anim.entered_at?;

    // IN: the interrupted incoming — the state that was current before the switch
    // (now `previous_state`), sampled at t2 on its own timeline.
    let prev_name = anim.previous_state.as_ref()?;
    let prev = anim.states.get(prev_name)?;
    let in_stamp = anim.previous_entered_at?;
    let incoming = clip_sample(prev, in_stamp, t2, phase)?;

    // w_interrupted: how far the interrupted OUT→IN fade had progressed at t2,
    // measured against IN's own crossfade window. Clamped so a finished or
    // zero-window interrupted fade reads as fully IN.
    let in_crossfade = crossfade_seconds(prev.crossfade_ms);
    let weight = if in_crossfade > 0.0 {
        (((t2 - in_stamp) as f32) / in_crossfade).clamp(0.0, 1.0)
    } else {
        // A hard-cut IN window means the entity was showing IN alone — capture it.
        1.0
    };

    // OUT: the stashed outgoing source of the interrupted fade, sampled at t2.
    // A clip leg advances on its own timeline; a prior-snapshot reference carries
    // the incoming as its fallback so a culled prior capture degrades to IN.
    let outgoing = match anim.interrupted_outgoing.as_ref() {
        Some(InterruptedOutgoing::Clip { state, entered_at }) => {
            let out_state = anim.states.get(state)?;
            FadeSource::Clip(clip_sample(out_state, *entered_at, t2, phase)?)
        }
        Some(InterruptedOutgoing::Snapshot { tag }) => FadeSource::Snapshot {
            tag: *tag,
            fallback: incoming,
        },
        // No stashed outgoing (e.g. a clip fade whose previous stamp was pending):
        // degrade OUT to IN so the capture freezes the IN pose — no panic.
        None => FadeSource::Clip(incoming),
    };

    Some(CaptureInstruction {
        seed: 0, // filled by the collector (per-instance EntityId)
        tag: snapshot_tag(t2),
        outgoing,
        incoming,
        weight,
    })
}

/// One entity's state-elapsed query result: current state name, elapsed seconds
/// since entry, and (for non-looping states) whether the clip has completed.
/// A pending stamp reads `elapsed = 0`, `complete = false`; a looping state
/// never completes; a non-looping state completes exactly when its clip duration
/// has elapsed. Tests consume this now; the future AI state-selection layer is
/// the named consumer — `allow(dead_code)` off the test build until that lands.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StateElapsed {
    pub(crate) state: String,
    pub(crate) elapsed: f32,
    pub(crate) complete: bool,
}

/// Query an animated entity's current state, elapsed seconds since entry, and
/// completion. `None` when the current state is undeclared. A pending entry
/// stamp reads elapsed `0` / not complete; a looping state never completes; a
/// non-looping state completes when (and only when) its clip duration elapses.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn state_elapsed(
    anim: &MeshAnimation,
    table: &ModelClipTable,
    anim_time: f64,
) -> Option<StateElapsed> {
    let state = anim.states.get(&anim.current_state)?;
    let Some(entered_at) = anim.entered_at else {
        // Pending: just switched this tick, never resolved.
        return Some(StateElapsed {
            state: anim.current_state.clone(),
            elapsed: 0.0,
            complete: false,
        });
    };
    let elapsed = ((anim_time - entered_at) as f32).max(0.0);
    let complete = if state.looping {
        false
    } else {
        // Non-looping: complete exactly when the clip duration has elapsed.
        state
            .clip_index
            .and_then(|i| table.duration(i))
            .is_some_and(|duration| elapsed >= duration)
    };
    Some(StateElapsed {
        state: anim.current_state.clone(),
        elapsed,
        complete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::mesh_pass::ClipMetadata;
    use crate::scripting::components::mesh::{
        AnimationState, DEFAULT_CROSSFADE_MS, InterruptPolicy,
    };

    const EPS: f32 = 1.0e-4;

    fn meta(pairs: &[(&str, f32)]) -> Vec<ClipMetadata> {
        pairs
            .iter()
            .map(|(name, duration)| ClipMetadata {
                name: (*name).to_string(),
                duration: *duration,
            })
            .collect()
    }

    fn state(clip: &str, looping: bool, crossfade_ms: f32, idx: Option<usize>) -> AnimationState {
        AnimationState {
            clip: clip.into(),
            looping,
            crossfade_ms,
            interrupt: InterruptPolicy::Smooth,
            clip_index: idx,
        }
    }

    fn anim_with(
        states: &[(&str, AnimationState)],
        current: &str,
        entered_at: Option<f64>,
    ) -> MeshAnimation {
        let map: HashMap<String, AnimationState> = states
            .iter()
            .map(|(n, s)| (n.to_string(), s.clone()))
            .collect();
        let mut anim = MeshAnimation::new(map, current.into());
        anim.entered_at = entered_at;
        anim
    }

    #[test]
    fn clip_table_resolves_names_and_keeps_first_on_duplicate() {
        let table = ModelClipTable::from_metadata(&meta(&[
            ("idle", 1.0),
            ("walk", 2.0),
            ("idle", 9.0), // duplicate name — first wins
        ]));
        assert_eq!(table.index_of("idle"), Some(0));
        assert_eq!(table.index_of("walk"), Some(1));
        assert_eq!(table.index_of("missing"), None);
        assert_eq!(table.duration(0), Some(1.0));
        assert_eq!(table.duration(2), Some(9.0));
    }

    #[test]
    fn resolve_state_clips_fills_indices_and_reports_missing() {
        let table = ModelClipTable::from_metadata(&meta(&[("idle", 1.0), ("attack", 0.5)]));
        let mut states: HashMap<String, AnimationState> = HashMap::new();
        states.insert("idle".into(), state("idle", true, 150.0, None));
        states.insert("die".into(), state("death_clip", false, 0.0, None));

        let missing = resolve_state_clips(&mut states, &table);
        assert_eq!(states["idle"].clip_index, Some(0));
        assert_eq!(states["die"].clip_index, None, "missing clip stays None");
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].state, "die");
        assert_eq!(missing[0].clip, "death_clip");
    }

    #[test]
    fn default_state_samples_its_clip_at_spawn() {
        let anim = anim_with(
            &[("idle", state("idle", true, 150.0, Some(0)))],
            "idle",
            Some(0.0),
        );
        let result = animate_entity(&anim, 0.5, 0.0).expect("animates");
        assert_eq!(result.sample.primary.clip_index, 0);
        assert!(
            (result.sample.primary.time - 0.5).abs() < EPS,
            "elapsed = anim_time - entered"
        );
        assert!(
            result.sample.fade.is_none(),
            "no fade at spawn (no previous)"
        );
    }

    #[test]
    fn looping_state_adds_phase_one_shot_does_not() {
        let looping = anim_with(
            &[("idle", state("idle", true, 0.0, Some(0)))],
            "idle",
            Some(0.0),
        );
        let one_shot = anim_with(
            &[("attack", state("attack", false, 0.0, Some(1)))],
            "attack",
            Some(0.0),
        );
        let phase = 0.3;
        let lp = animate_entity(&looping, 1.0, phase).unwrap();
        assert!(
            (lp.sample.primary.time - (1.0 + phase)).abs() < EPS,
            "looping adds phase"
        );
        let os = animate_entity(&one_shot, 1.0, phase).unwrap();
        assert!(
            (os.sample.primary.time - 1.0).abs() < EPS,
            "one-shot ignores phase"
        );
        assert_eq!(
            os.sample.primary.loop_policy,
            Loop::Clamp,
            "one-shot clamps"
        );
        assert_eq!(lp.sample.primary.loop_policy, Loop::Wrap, "looping wraps");
    }

    #[test]
    fn crossfade_weight_progresses_over_window_then_clears() {
        let mut anim = anim_with(
            &[
                ("idle", state("idle", true, 0.0, Some(0))),
                ("walk", state("walk", true, 200.0, Some(1))), // 0.2s fade
            ],
            "walk",
            Some(1.0),
        );
        anim.previous_state = Some("idle".into());
        anim.previous_entered_at = Some(0.0);

        // At the switch instant (anim_time == entered): weight 0 → all `from`.
        let at_switch = animate_entity(&anim, 1.0, 0.0).unwrap();
        let fade = at_switch.sample.fade.expect("fade active at switch");
        assert!(
            (fade.weight - 0.0).abs() < EPS,
            "weight 0 at switch instant"
        );

        // Midway through the 0.2s window: weight ~0.5.
        let midway = animate_entity(&anim, 1.1, 0.0).unwrap();
        let fade = midway.sample.fade.expect("fade active midway");
        assert!(
            (fade.weight - 0.5).abs() < EPS,
            "weight 0.5 midway, got {}",
            fade.weight
        );

        // After the window closes: no fade leg.
        let after = animate_entity(&anim, 1.5, 0.0).unwrap();
        assert!(after.sample.fade.is_none(), "fade clears after window");
    }

    #[test]
    fn outgoing_clip_advances_on_its_own_timeline() {
        // The outgoing leg's time derives from previous_entered_at, NOT the new
        // entry stamp — so it keeps playing as it fades out.
        let mut anim = anim_with(
            &[
                ("idle", state("idle", true, 0.0, Some(0))),
                ("walk", state("walk", true, 1000.0, Some(1))),
            ],
            "walk",
            Some(2.0),
        );
        anim.previous_state = Some("idle".into());
        anim.previous_entered_at = Some(0.0);

        let result = animate_entity(&anim, 2.5, 0.0).unwrap();
        let fade = result.sample.fade.expect("fade active");
        let FadeSource::Clip(out) = fade.from else {
            panic!("clip fade source expected");
        };
        // Outgoing idle started at 0.0; at anim_time 2.5 it is 2.5s into its clip.
        assert!(
            (out.time - 2.5).abs() < EPS,
            "outgoing advances from its own stamp"
        );
        // Primary walk started at 2.0; at 2.5 it is 0.5s in.
        assert!(
            (result.sample.primary.time - 0.5).abs() < EPS,
            "primary from its own stamp"
        );
    }

    #[test]
    fn pending_stamp_contributes_no_fade() {
        let mut anim = anim_with(
            &[
                ("idle", state("idle", true, 0.0, Some(0))),
                ("walk", state("walk", true, 200.0, Some(1))),
            ],
            "walk",
            None, // pending current stamp
        );
        anim.previous_state = Some("idle".into());
        anim.previous_entered_at = Some(0.0);
        let result = animate_entity(&anim, 1.0, 0.0).unwrap();
        assert!(result.sample.fade.is_none(), "pending stamp → no fade");
    }

    #[test]
    fn snapshot_fade_carries_tag_and_fallback() {
        // A recorded snapshot fade: current=walk faded from previous_state=idle.
        // The interrupted incoming (IN) is `previous_state` = idle, so the
        // runtime fade leg blends FROM the snapshot tagged by the new (walk) entry
        // stamp, with idle as the degrade fallback.
        let mut anim = anim_with(
            &[
                ("idle", state("idle", true, 0.0, Some(0))),
                ("walk", state("walk", true, 200.0, Some(1))),
            ],
            "walk",
            Some(1.0),
        );
        anim.previous_state = Some("idle".into());
        anim.previous_entered_at = Some(0.0);
        anim.fade_source = FadeSourceKind::Snapshot;

        let result = animate_entity(&anim, 1.05, 0.0).unwrap();
        let fade = result.sample.fade.expect("snapshot fade active");
        let FadeSource::Snapshot { tag, fallback } = fade.from else {
            panic!("snapshot fade source expected");
        };
        assert_eq!(
            tag,
            1.0_f64.to_bits(),
            "tag is the entered (walk) stamp bits"
        );
        // Fallback is the interrupted-incoming idle leg (its own timeline).
        assert_eq!(fallback.clip_index, 0);
        assert!((fallback.time - 1.05).abs() < EPS);

        // And a one-time capture instruction is emitted on the smooth interrupt.
        // The capture's incoming is the interrupted incoming IN = previous_state
        // (idle), sampled at the FROZEN entered_at (t2 = 1.0), not the moving clock.
        let capture = result
            .capture
            .expect("smooth snapshot interrupt emits capture");
        assert_eq!(capture.tag, 1.0_f64.to_bits());
        assert_eq!(
            capture.incoming.clip_index, 0,
            "incoming is the interrupted incoming (previous_state = idle)"
        );
        // IN sampled at t2 = 1.0 on its own timeline (in_stamp = 0.0) → time 1.0.
        assert!((capture.incoming.time - 1.0).abs() < EPS);
    }

    #[test]
    fn state_elapsed_reports_progress_and_completion() {
        let table = ModelClipTable::from_metadata(&meta(&[("idle", 2.0), ("attack", 1.0)]));

        // Pending stamp → elapsed 0, not complete.
        let pending = anim_with(
            &[("attack", state("attack", false, 0.0, Some(1)))],
            "attack",
            None,
        );
        let q = state_elapsed(&pending, &table, 5.0).unwrap();
        assert_eq!(q.elapsed, 0.0);
        assert!(!q.complete, "pending never complete");

        // Looping idle never completes.
        let looping = anim_with(
            &[("idle", state("idle", true, 0.0, Some(0)))],
            "idle",
            Some(0.0),
        );
        let q = state_elapsed(&looping, &table, 100.0).unwrap();
        assert!((q.elapsed - 100.0).abs() < EPS);
        assert!(!q.complete, "looping never completes");

        // Non-looping attack (duration 1.0): not complete before, complete at/after.
        let one_shot = anim_with(
            &[("attack", state("attack", false, 0.0, Some(1)))],
            "attack",
            Some(0.0),
        );
        assert!(
            !state_elapsed(&one_shot, &table, 0.5).unwrap().complete,
            "before end"
        );
        assert!(
            state_elapsed(&one_shot, &table, 1.0).unwrap().complete,
            "exactly at duration"
        );
        assert!(
            state_elapsed(&one_shot, &table, 2.0).unwrap().complete,
            "after end stays complete"
        );
    }

    #[test]
    fn unresolved_current_state_does_not_animate() {
        let anim = anim_with(
            &[("broken", state("missing", true, 0.0, None))],
            "broken",
            Some(0.0),
        );
        assert!(
            animate_entity(&anim, 1.0, 0.0).is_none(),
            "an unresolved current state yields no sample params (bind pose)",
        );
    }

    // Keep DEFAULT_CROSSFADE_MS referenced so a future default-policy change
    // surfaces here (the component default is 150ms when unspecified).
    #[test]
    fn default_crossfade_constant_is_referenced() {
        assert!(crossfade_seconds(DEFAULT_CROSSFADE_MS) > 0.0);
    }
}
