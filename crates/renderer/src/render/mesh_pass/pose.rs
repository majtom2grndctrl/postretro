// Skinned-pose sampling, smooth-interrupt snapshots and the time-slicing palette cache.
// See: context/lib/rendering_pipeline.md §9

use std::collections::HashMap;

use postretro_model::BonePaletteEntry;
use postretro_model::anim::{BlendSource, LocalTrs};
use postretro_model::sample_params::{ClipSample, FadeSource, MeshSampleParams, SnapshotTag};
use postretro_model::skeleton::{AnimationClip, Skeleton};
use postretro_render_cpu::mesh_instances::MeshPaletteCacheKey;

/// One captured `"smooth"`-interrupt snapshot: the per-joint local-TRS pose
/// frozen at the interrupt instant, tagged with the entered state's entry stamp.
/// A subsequent snapshot fade blends against `pose` only when its
/// [`SnapshotTag`] matches `tag`; a mismatch (a replacement fade) drops the entry.
///
/// The pose buffer is owned (cloned out of the sampler's snapshot capture) — a
/// snapshot outlives the frame that captured it, so it cannot borrow the
/// per-frame scratch.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StoredSnapshot {
    pub(super) tag: SnapshotTag,
    pub(super) pose: Vec<LocalTrs>,
}

/// Per-entity snapshot store for `"smooth"` interrupts: a plain CPU-side map
/// keyed by entity seed, each entry tagged. A GPU-free seam (mirrors the
/// `model_bounds` precedent) so the smooth-interrupt logic is unit-testable
/// without a `wgpu::Device`.
///
/// Lifecycle: a capture instruction installs (or refreshes) an entry; end-of-frame
/// retention keeps only entries whose entity has an active matching snapshot fade
/// in the current plan. Culled, budget-dropped, despawned, and no-mesh frames
/// evict stale entries. Emptied wholesale at level load by
/// [`MeshPass::clear_for_level_load`].
#[derive(Debug, Default)]
pub(super) struct SnapshotStore {
    pub(super) entries: HashMap<u32, StoredSnapshot>,
}

impl SnapshotStore {
    /// Apply a capture instruction: capture `blend(outgoing, incoming)` at the
    /// instruction's weight into the store, tagged. **Idempotent:** if the stored
    /// entry already carries this tag, nothing is evaluated (a re-emission under a
    /// frozen clock is a no-op). A snapshot-referencing outgoing source that
    /// MISSES the store captures `blend(fallback, incoming)` instead (the capture
    /// frame for the referenced snapshot was culled — degrade to the fallback).
    ///
    /// `resolve_clip` maps a clip index to its `&AnimationClip` (the model's clip
    /// list); a missing clip aborts the capture (no usable pose).
    pub(super) fn apply_capture<'a>(
        &mut self,
        capture: &postretro_model::sample_params::CaptureInstruction,
        skeleton: &Skeleton,
        resolve_clip: impl Fn(usize) -> Option<&'a AnimationClip>,
        scratch: &mut Vec<LocalTrs>,
    ) {
        // Idempotent: a matching tag means this capture already landed.
        if self
            .entries
            .get(&capture.seed)
            .is_some_and(|e| e.tag == capture.tag)
        {
            return;
        }

        // Resolve the incoming (entered) clip leg.
        let Some(incoming) = clip_blend_source(&capture.incoming, &resolve_clip) else {
            return;
        };

        // Resolve the outgoing source: a snapshot reference blends against the
        // stored pose if present, else its fallback clip (degrade-on-miss). A
        // clip source resolves directly.
        let outgoing_clip;
        let outgoing: BlendSource = match capture.outgoing {
            FadeSource::Snapshot { tag, fallback } => {
                match self.entries.get(&capture.seed) {
                    Some(stored) if stored.tag == tag => BlendSource::Snapshot(&stored.pose),
                    _ => {
                        // Store miss / stale tag: capture from the fallback clip.
                        let Some(src) = clip_blend_source(&fallback, &resolve_clip) else {
                            return;
                        };
                        outgoing_clip = src;
                        outgoing_clip.as_blend_source()
                    }
                }
            }
            FadeSource::Clip(leg) => {
                let Some(src) = clip_blend_source(&leg, &resolve_clip) else {
                    return;
                };
                outgoing_clip = src;
                outgoing_clip.as_blend_source()
            }
        };

        postretro_model::anim::capture_blend(
            &outgoing,
            &incoming.as_blend_source(),
            capture.weight,
            skeleton,
            scratch,
        );
        self.entries.insert(
            capture.seed,
            StoredSnapshot {
                tag: capture.tag,
                pose: scratch.clone(),
            },
        );
    }

    /// Look up an entry whose tag matches `tag`, for a snapshot fade.
    pub(super) fn matching(&self, seed: u32, tag: SnapshotTag) -> Option<&[LocalTrs]> {
        self.entries
            .get(&seed)
            .filter(|e| e.tag == tag)
            .map(|e| e.pose.as_slice())
    }

    /// End-of-frame retention. A snapshot survives only when the same entity had
    /// an active snapshot fade whose tag matched the stored capture this frame.
    pub(super) fn retain_active_snapshot_fades(
        &mut self,
        active_fades: &HashMap<u32, SnapshotTag>,
    ) {
        if active_fades.is_empty() {
            self.entries.clear();
            return;
        }

        self.entries
            .retain(|seed, stored| active_fades.get(seed).is_some_and(|tag| *tag == stored.tag));
    }

    /// Empty the store (level-load clear).
    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// One cached palette run: the last sampled bone-palette matrices for an entity,
/// reused on a time-sliced SKIP frame so the pass re-uploads a valid pose without
/// re-sampling. The `Vec` is reused in place on a resample (cleared + extended),
/// so a steady-state cache hit allocates nothing.
#[derive(Debug, Default)]
struct CachedPalette {
    /// The entity's last sampled palette run (one `BonePaletteEntry` per joint).
    run: Vec<BonePaletteEntry>,
    /// Set true when the entry is touched (resample or skip) this frame; entries
    /// left `false` after a frame are evicted, so the cache never exceeds the
    /// frame's planned-instance count.
    seen_this_frame: bool,
}

/// Renderer-side per-entity palette cache for animation time-slicing,
/// keyed by the collector's stable per-instance identity (GPU-free data logic,
/// unit-testable without a `wgpu::Device`). On a RESAMPLE frame the
/// pass samples the pose and refreshes the cached run; on a SKIPPED frame it
/// re-uploads the cached run with no sampling. A cache MISS forces a resample
/// that frame regardless of the collector's flag (the collector cannot see
/// renderer-side cache state), so a culled instance re-entering view never shows
/// a stale pose.
///
/// Eviction: entries not touched in a frame are dropped at [`end_frame`], so the
/// cache is bounded by the frame's planned-instance count (≤ `MAX_INSTANCES`
/// entries, ≤ `MAX_PALETTE_ENTRIES` total slots). Emptied wholesale at level load
/// by [`PaletteCache::clear`] (instance identities are not stable across levels).
///
/// [`end_frame`]: PaletteCache::end_frame
#[derive(Debug, Default)]
pub(super) struct PaletteCache {
    entries: HashMap<MeshPaletteCacheKey, CachedPalette>,
}

impl PaletteCache {
    /// Resolve whether this instance must sample this frame. Returns `true` when
    /// the collector asked to resample OR the cache misses (no entry for this
    /// key) — the miss upgrade is what keeps a re-entering instance from showing
    /// a stale pose. A `false` return means a valid cached run exists and the
    /// collector cleared the instance to skip.
    pub(super) fn must_sample(&self, key: MeshPaletteCacheKey, collector_resample: bool) -> bool {
        collector_resample || !self.entries.contains_key(&key)
    }

    /// Store a freshly sampled run for `key`, reusing the entry's `Vec` storage
    /// in place (cleared + extended — no realloc on a steady-state hit). Marks the
    /// entry seen this frame so eviction keeps it.
    pub(super) fn store(&mut self, key: MeshPaletteCacheKey, run: &[BonePaletteEntry]) {
        let entry = self.entries.entry(key).or_default();
        entry.run.clear();
        entry.run.extend_from_slice(run);
        entry.seen_this_frame = true;
    }

    /// The cached run for `key` on a SKIP frame, or `None` if absent. Also marks
    /// the entry seen so a skipped instance is not evicted. (A skip only reaches
    /// here when `must_sample` already returned `false`, i.e. the entry exists.)
    pub(super) fn touch_cached(&mut self, key: MeshPaletteCacheKey) -> Option<&[BonePaletteEntry]> {
        let entry = self.entries.get_mut(&key)?;
        entry.seen_this_frame = true;
        Some(entry.run.as_slice())
    }

    /// Evict entries not touched this frame and reset the per-entry seen flags for
    /// the next frame. Called once at the end of the per-frame sample/upload pass,
    /// so the cache holds exactly this frame's planned instances.
    pub(super) fn end_frame(&mut self) {
        self.entries.retain(|_, e| e.seen_this_frame);
        for e in self.entries.values_mut() {
            e.seen_this_frame = false;
        }
    }

    /// Empty the cache (level-load clear).
    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// A resolved clip blend source's owned parts, so a `BlendSource::Clip` can be
/// reconstructed by reference. `BlendSource` borrows the clip, so the borrow
/// must outlive the `BlendSource` — this holds the `(clip, time, loop)` and
/// hands out a fresh `BlendSource` on demand.
struct ClipBlend<'a> {
    clip: &'a AnimationClip,
    time: f32,
    loop_policy: postretro_model::anim::Loop,
}

impl<'a> ClipBlend<'a> {
    fn as_blend_source(&self) -> BlendSource<'a> {
        BlendSource::Clip {
            clip: self.clip,
            time: self.time,
            loop_policy: self.loop_policy,
        }
    }
}

/// Resolve a [`ClipSample`] leg into a borrowed [`ClipBlend`], or `None` if its
/// clip index is absent from the model.
fn clip_blend_source<'a>(
    leg: &ClipSample,
    resolve_clip: &impl Fn(usize) -> Option<&'a AnimationClip>,
) -> Option<ClipBlend<'a>> {
    let clip = resolve_clip(leg.clip_index)?;
    Some(ClipBlend {
        clip,
        time: leg.time,
        loop_policy: leg.loop_policy,
    })
}

/// Per-instance values that select and modify one palette sample. Grouping
/// these keeps the sampling seam aligned with the render-plan payload while the
/// model-owned skeleton and modifier stack remain separate cache data.
pub(super) struct InstancePoseSample<'a> {
    pub(super) params: &'a MeshSampleParams,
    pub(super) pose_inputs: Option<&'a postretro_entities::PoseInputs>,
    pub(super) seed: u32,
}

/// Sample an active modified pose whose primary animation clip is unresolved.
/// The skeleton rest pose becomes the entered endpoint, so a model whose
/// primary clip failed to resolve can still consume presentation inputs
/// (in production `pose_inputs` is only produced for meshes with an
/// animation block, so this is that block's clip going unresolved — not a
/// model authored with no animation at all), and an in-flight fade retains
/// its normal outgoing→incoming weight convention.
fn sample_modified_rest_instance<'a>(
    instance: &InstancePoseSample<'_>,
    pose_inputs: &postretro_entities::PoseInputs,
    skeleton: &Skeleton,
    pose_stack: &postretro_model::pose_modifier::PoseModifierStack,
    store: &SnapshotStore,
    resolve_clip: &impl Fn(usize) -> Option<&'a AnimationClip>,
    out: &mut Vec<BonePaletteEntry>,
) {
    let sample = instance.params;
    let rest = BlendSource::Rest;
    let Some(fade) = sample.fade else {
        postretro_model::anim::sample_rest_pose_modified(skeleton, pose_stack, pose_inputs, out);
        return;
    };

    let outgoing = match fade.from {
        FadeSource::Clip(leg) => {
            clip_blend_source(&leg, resolve_clip).map(|source| source.as_blend_source())
        }
        FadeSource::Snapshot { tag, fallback } => store
            .matching(instance.seed, tag)
            .map(BlendSource::Snapshot)
            .or_else(|| {
                clip_blend_source(&fallback, resolve_clip).map(|source| source.as_blend_source())
            }),
    };

    match outgoing {
        Some(outgoing) => postretro_model::anim::sample_blended_modified(
            &outgoing,
            &rest,
            fade.weight,
            skeleton,
            pose_stack,
            Some(pose_inputs),
            out,
        ),
        None => {
            postretro_model::anim::sample_rest_pose_modified(skeleton, pose_stack, pose_inputs, out)
        }
    }
}

/// Sample one instance's pose into `out` per its resolved [`MeshSampleParams`]:
/// explicit rest pose, a single clip, a clip→clip blend, or a snapshot→clip
/// blend. Always writes a full run into `out` and returns `true`, so the caller's
/// palette write covers the whole region.
///
/// When the primary clip does not resolve, an active modifier stack samples the
/// skeleton rest pose so a model whose primary clip is unresolved can still
/// consume pose inputs (in production, only meshes with an animation block
/// carry `pose_inputs`, so this covers that block's clip going unresolved, not
/// a model authored with no animation at all). Otherwise `out` is filled with
/// one identity (bind-pose) matrix per joint
/// rather than left untouched. The exact identity fallback prevents an inactive
/// unsampled run from inheriting another instance's densely repacked matrices.
///
/// Fade resolution mirrors the collector's intent but degrades safely at the GPU
/// seam: a [`FadeSource::Snapshot`] whose store entry is missing (capture frame
/// culled) falls back to its `(clip, time)` pair — a `"snap"`-equivalent hard
/// blend the game layer never saw.
pub(super) fn sample_instance<'a>(
    instance: InstancePoseSample<'_>,
    skeleton: &Skeleton,
    pose_stack: &postretro_model::pose_modifier::PoseModifierStack,
    store: &SnapshotStore,
    resolve_clip: &impl Fn(usize) -> Option<&'a AnimationClip>,
    out: &mut Vec<BonePaletteEntry>,
) -> bool {
    let sample = instance.params;
    let pose_inputs = instance.pose_inputs;
    let seed = instance.seed;
    if sample.is_rest_pose() {
        if let Some(inputs) = pose_inputs.filter(|_| !pose_stack.is_empty()) {
            postretro_model::anim::sample_rest_pose_modified(skeleton, pose_stack, inputs, out);
        } else {
            postretro_model::anim::sample_rest_pose(skeleton, out);
        }
        return true;
    }

    // A model whose primary clip is unresolved can still consume external
    // presentation inputs: modify its skeleton rest pose, retaining any
    // outgoing fade source. Pose inputs are presentation data and may also be
    // supplied by stateless holders, so this fallback is not limited to an
    // animation block whose clip failed to resolve. Without both an active
    // stack and inputs, preserve the historical exact identity fallback rather
    // than composing the model's rest/inverse-bind data.
    let Some(primary) = clip_blend_source(&sample.primary, resolve_clip) else {
        if let Some(inputs) = pose_inputs.filter(|_| !pose_stack.is_empty()) {
            sample_modified_rest_instance(
                &instance,
                inputs,
                skeleton,
                pose_stack,
                store,
                resolve_clip,
                out,
            );
            return true;
        }
        out.clear();
        out.resize(
            skeleton.joints.len(),
            BonePaletteEntry {
                matrix: glam::Mat4::IDENTITY.to_cols_array_2d(),
            },
        );
        return true;
    };

    let Some(fade) = sample.fade else {
        // Steady state: single clip sample (the common, allocation-free path).
        postretro_model::anim::sample_clip_looped_modified(
            primary.clip,
            skeleton,
            primary.time,
            primary.loop_policy,
            pose_stack,
            pose_inputs,
            out,
        );
        return true;
    };

    // A fade is active: blend `from` → `primary` at the weight. The blended
    // sampler takes weight 0 → `a` (the outgoing `from`), 1 → `b` (the entered
    // `primary`), matching the collector's weight convention.
    let primary_src = primary.as_blend_source();
    match fade.from {
        FadeSource::Clip(leg) => {
            let Some(from) = clip_blend_source(&leg, resolve_clip) else {
                // Outgoing clip gone: fall back to the primary alone.
                postretro_model::anim::sample_clip_looped_modified(
                    primary.clip,
                    skeleton,
                    primary.time,
                    primary.loop_policy,
                    pose_stack,
                    pose_inputs,
                    out,
                );
                return true;
            };
            postretro_model::anim::sample_blended_modified(
                &from.as_blend_source(),
                &primary_src,
                fade.weight,
                skeleton,
                pose_stack,
                pose_inputs,
                out,
            );
        }
        FadeSource::Snapshot { tag, fallback } => {
            match store.matching(seed, tag) {
                Some(pose) => {
                    postretro_model::anim::sample_blended_modified(
                        &BlendSource::Snapshot(pose),
                        &primary_src,
                        fade.weight,
                        skeleton,
                        pose_stack,
                        pose_inputs,
                        out,
                    );
                }
                None => {
                    // Store miss (capture frame culled): degrade to the fallback
                    // clip — a `"snap"`-equivalent blend the game layer never saw.
                    match clip_blend_source(&fallback, resolve_clip) {
                        Some(from) => postretro_model::anim::sample_blended_modified(
                            &from.as_blend_source(),
                            &primary_src,
                            fade.weight,
                            skeleton,
                            pose_stack,
                            pose_inputs,
                            out,
                        ),
                        None => postretro_model::anim::sample_clip_looped_modified(
                            primary.clip,
                            skeleton,
                            primary.time,
                            primary.loop_policy,
                            pose_stack,
                            pose_inputs,
                            out,
                        ),
                    }
                }
            }
        }
    }
    true
}
