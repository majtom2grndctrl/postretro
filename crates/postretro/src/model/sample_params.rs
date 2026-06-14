// Render-free per-instance animation sample-parameter types: the resolved clip
// legs, crossfade, snapshot reference, and one-time capture instruction the
// game side computes and the renderer's pose sampler consumes.
//
// CPU-only by contract (no wgpu, no `crate::render`). These are plain data, not
// GPU types, so they live in the model layer where BOTH producers import them:
// the game-side animation resolver (`scripting/systems/mesh_anim.rs`) and the
// hit-zone raycast facility build them; the renderer's sample-params builder
// (`render::mesh_pass`) consumes them. Relocated out of `render::mesh_instances`
// so the hit-zone facility can resolve a pose without importing any renderer
// type (honors the renderer-owns-GPU boundary).
// See: context/lib/rendering_pipeline.md §9 · entity_model.md §7

use crate::model::anim::Loop;

/// One sampled clip leg: its index into the model's clip list, the clip-local
/// time (seconds) to sample at, and whether time wraps (looping) or clamps
/// (one-shot). `Copy` plain-old-data — no heap, so a per-instance buffer of
/// these allocates nothing in steady state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ClipSample {
    /// Index into the model's glTF-order clip list.
    pub(crate) clip_index: usize,
    /// Clip-local time (seconds) to sample at — the GPU layer feeds this to the
    /// pose sampler, which applies the wrap/clamp itself.
    pub(crate) time: f32,
    /// Loop policy: `Wrap` for looping states, `Clamp` for one-shot states.
    pub(crate) loop_policy: Loop,
}

/// Which source the active crossfade blends *out of*, plus the data the GPU
/// layer needs to resolve it. `Copy` POD: a snapshot is referenced by entity
/// seed against the pass's snapshot store, never carried inline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FadeSource {
    /// Blend from a clip leg (the outgoing state's clip on its own advanced
    /// timeline — `"snap"` interrupts and normal clip→clip fades).
    Clip(ClipSample),
    /// Blend from the per-entity snapshot captured for a `"smooth"` interrupt.
    /// `tag` matches the store entry's tag (a store miss or tag mismatch
    /// degrades to `fallback`). `fallback` is the interrupted state's
    /// `(clip, time)` — the SAME pair a `"snap"` would have used, so a missed
    /// capture cleanly downgrades the fade to a hard clip blend.
    Snapshot {
        /// Entry-stamp tag identifying which capture this fade expects.
        tag: SnapshotTag,
        /// Fallback clip leg if the snapshot store misses (capture frame culled).
        fallback: ClipSample,
    },
}

/// Per-instance animation sample parameters — what the GPU layer feeds the
/// pose sampler this frame. `Copy` plain-old-data; the default
/// ([`MeshSampleParams::stateless`]) reproduces today's stateless behavior
/// (first clip, looped, phase-offset time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MeshSampleParams {
    /// The state currently being entered / held — always sampled.
    pub(crate) primary: ClipSample,
    /// The active crossfade, if a fade is in flight: what to blend *from* and
    /// the blend weight (`0` → all `from`, `1` → all `primary`). `None` once the
    /// fade window closes (steady state — one clip sample per instance).
    pub(crate) fade: Option<MeshFade>,
}

/// An active crossfade leg: the outgoing source and the current blend weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MeshFade {
    /// What the fade blends out of (clip leg or snapshot reference).
    pub(crate) from: FadeSource,
    /// Blend weight in `[0, 1]`: `0` → all `from`, `1` → all `primary`.
    pub(crate) weight: f32,
}

/// Tag identifying one snapshot-store entry: the entered state's pending-or-
/// resolved entry stamp, quantized to the clock's bit pattern so a re-emitted
/// capture under a frozen clock compares equal (idempotent capture). Derived
/// from the entered state's `entered_at: f64`; a `None` (pending) stamp never
/// produces a snapshot fade, so the tag always has a concrete origin.
pub(crate) type SnapshotTag = u64;

/// A one-time snapshot-capture instruction emitted on a `"smooth"` interrupt
/// frame: capture the in-flight blended pose into the per-entity snapshot store,
/// tagged so subsequent frames blend against it. All `Copy` POD; the outgoing
/// source may itself reference a prior snapshot (snapshot×clip capture), in
/// which case `outgoing` carries the same `(clip, time)` fallback the sampling
/// frames use so a store miss degrades cleanly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CaptureInstruction {
    /// Entity seed keying the snapshot store (the raw `EntityId`).
    pub(crate) seed: u32,
    /// Tag for the new store entry (the entered state's entry stamp bits). The
    /// pass skips a capture whose tag already matches the stored entry.
    pub(crate) tag: SnapshotTag,
    /// The in-flight blend's outgoing source.
    pub(crate) outgoing: FadeSource,
    /// The in-flight blend's incoming (entered) clip leg.
    pub(crate) incoming: ClipSample,
    /// The in-flight blend's weight at the interrupt instant.
    pub(crate) weight: f32,
}

impl MeshSampleParams {
    /// The stateless `prop_mesh` default: sample the model's first clip (glTF
    /// index 0), looping, with no crossfade. The clip-local time is filled by the
    /// collector (animation clock + per-instance phase) — this names the legs.
    pub(crate) fn stateless(time: f32) -> Self {
        Self {
            primary: ClipSample {
                clip_index: 0,
                time,
                loop_policy: Loop::Wrap,
            },
            fade: None,
        }
    }
}
