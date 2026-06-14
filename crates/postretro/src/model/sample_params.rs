// Render-free per-instance animation sample-parameter types: the resolved clip
// legs, crossfade, snapshot reference, and one-time capture instruction — plus
// the per-instance phase offset that de-syncs a looping wave.
//
// CPU-only by contract (no wgpu, no `crate::render`). These are plain data, not
// GPU types, so they live in the model layer where every party imports them.
// SOLE producer of the sample-param types: the game-side animation resolver
// (`scripting/systems/mesh_anim.rs`). CONSUMERS: the renderer's pose sampler
// (`render::mesh_pass`) AND the hit-zone raycast facility
// (`scripting/systems/hit_zones.rs`). `instance_phase` is shared CPU logic both
// the renderer and the hit-zone facility call to compute the SAME per-instance
// phase, so capsules sample the clip-local time the renderer draws. Relocated
// out of `render::mesh_instances` so the hit-zone facility can resolve a pose
// without importing any renderer type (honors the renderer-owns-GPU boundary).
// See: context/lib/rendering_pipeline.md §9 · entity_model.md §7

use crate::model::anim::Loop;

/// Derive a per-instance animation phase offset (seconds) from the instance's
/// deterministic seed (raw `EntityId`). Spreads a spawned wave across the clip
/// so instances do not animate lock-step. The seed's low bits (entity slot
/// index) vary per entity, so hashing it and mapping to `[0, duration)` yields a
/// stable, well-distributed offset. A zero-length clip yields phase 0.
///
/// Shared CPU logic with no GPU dependency: both the renderer's collector
/// (`scripting/systems/mesh_render`) and the hit-zone facility
/// (`scripting/systems/hit_zones`) call it with the SAME seed + clip duration so
/// a hit capsule samples the same clip-local time the renderer draws.
pub(crate) fn instance_phase(seed: u32, clip_duration: f32) -> f32 {
    if clip_duration <= 0.0 {
        return 0.0;
    }
    // Cheap integer hash (splitmix32-style finalizer) so adjacent seeds — which
    // EntityId slot indices tend to be — scatter rather than march in lockstep.
    let mut h = seed;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    // Map to [0, 1) then to [0, duration).
    let frac = (h as f32) / (u32::MAX as f32 + 1.0);
    frac * clip_duration
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_phase_spreads_distinct_seeds_across_the_clip() {
        let duration = 2.0;
        let p0 = instance_phase(0, duration);
        let p1 = instance_phase(1, duration);
        let p2 = instance_phase(2, duration);
        // All in range.
        for p in [p0, p1, p2] {
            assert!((0.0..duration).contains(&p), "phase {p} in [0, {duration})");
        }
        // Adjacent seeds do not collapse to the same phase (de-sync the wave).
        assert!(
            (p0 - p1).abs() > 1.0e-4,
            "seeds 0 and 1 produce distinct phases"
        );
        assert!(
            (p1 - p2).abs() > 1.0e-4,
            "seeds 1 and 2 produce distinct phases"
        );
    }

    #[test]
    fn instance_phase_zero_for_zero_length_clip() {
        assert_eq!(instance_phase(12345, 0.0), 0.0);
    }
}
