// Game-side skeletal hit-zone store: retains each model TYPE's CPU skeleton,
// clips, and authored joint-zone table, plus a derived broad-phase bound swept
// from the clips. CPU-only — no wgpu, no `crate::render`. The per-shot raycast
// facility (Task 4) will be added here and consume this store.
// See: context/lib/entity_model.md §7 · rendering_pipeline.md §9

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use glam::Vec3;

use crate::lighting::cone_frustum::Aabb;
use crate::model::ModelHandle;
use crate::model::anim::{Loop, sample_clip_looped_world};
use crate::model::gltf_loader::{self, JointZone};
use crate::model::skeleton::{AnimationClip, Skeleton};

/// Engine default capsule radius (meters) for a zone-bearing joint whose
/// authored `hitZoneRadius` is absent (`JointZone::radius == None`). The loader
/// stores the radius as-authored; this is where the default is applied — the
/// downstream-consumer policy `gltf_loader::read_joint_zone` defers to.
pub(crate) const DEFAULT_ZONE_RADIUS: f32 = 0.12;

/// Number of FIXED, uniform time samples taken per clip when sweeping the
/// derived broad-phase bound, INCLUDING both endpoints (`t = 0` and
/// `t = clip.duration`). At least 8 (the plan floor) so a limb's swung arc is
/// captured densely enough that the union encloses the full motion, not just the
/// endpoints. Deterministic: the same model always yields the same bound.
pub(crate) const BOUND_SAMPLES_PER_CLIP: usize = 8;

/// One model TYPE's retained CPU hit-zone data, keyed by [`ModelHandle`] in the
/// [`HitZoneStore`].
///
/// The skeleton and clips are `Arc`-wrapped so the future per-shot raycast path
/// (Task 4) can clone a cheap handle to re-sample the live pose without ever
/// deep-cloning keyframes. `joint_zones` is parallel to `skeleton.joints`
/// (entry `i` describes joint `i`), exactly as the loader produced it.
///
/// The fields are populated now (and exercised by this module's tests) but read
/// by no shipping consumer until Task 4 lands the raycast facility — gated like
/// the `StateElapsed` precedent so the seam stays dead-code-free in shipping
/// builds without losing the test coverage.
#[cfg_attr(not(test), allow(dead_code))] // Task 4 (raycast facility) reads these fields.
#[derive(Clone)]
pub(crate) struct ModelHitZones {
    /// The model's joint hierarchy (parent-before-child). Shared, never mutated.
    pub(crate) skeleton: Arc<Skeleton>,
    /// The model's animation clips, in authored order. Shared, never mutated.
    pub(crate) clips: Arc<Vec<AnimationClip>>,
    /// Per-joint authored hit zones, parallel to `skeleton.joints`. `None` where
    /// a joint carries no zone. Empty for a model with no zones at all.
    pub(crate) joint_zones: Vec<Option<JointZone>>,
    /// Model-local broad-phase AABB swept from EVERY clip (≥8 uniform samples
    /// each, endpoints included) and inflated by each joint's capsule radius.
    /// `None` for an AABB-only model (no zone tags) — those skip the derived
    /// bound and fall back to the authored hitbox elsewhere.
    pub(crate) derived_bound: Option<Aabb>,
}

impl ModelHitZones {
    /// True when this model carries at least one authored joint zone — i.e. the
    /// skeletal hit-zone path applies and a [`derived_bound`](Self::derived_bound)
    /// was computed.
    fn has_zones(joint_zones: &[Option<JointZone>]) -> bool {
        joint_zones.iter().any(Option::is_some)
    }
}

/// All models' hit-zone data, keyed by handle. One entry per model TYPE (not per
/// instance). Owned by the `App` beside `collision_world`; populated at the
/// level-load model sweep and cleared on level change. Nothing consumes it yet
/// besides tests — the Task 4 raycast facility lands the runtime consumer.
#[derive(Default)]
pub(crate) struct HitZoneStore {
    models: HashMap<ModelHandle, ModelHitZones>,
}

impl HitZoneStore {
    pub(crate) fn new() -> Self {
        Self {
            models: HashMap::new(),
        }
    }

    /// Drop every entry — called by the level-load clear alongside
    /// `MeshClipTables::clear` so a new level starts from an empty store.
    pub(crate) fn clear(&mut self) {
        self.models.clear();
    }

    /// Re-load a model game-side from its glTF and install its hit-zone entry.
    ///
    /// Independent ownership: the renderer moves a model's skeleton + clips into
    /// the GPU layer (returning only tags), so the game side obtains its OWN copy
    /// by re-loading through [`gltf_loader::load_model`] against the resolved open
    /// path (`content_root.join(model_rel)`, the same recipe the renderer's
    /// `resolve_model_open_path_and_handle` uses for its cache key vs. open path).
    ///
    /// A failed/invalid load is non-fatal: it warns (naming the path) and installs
    /// nothing — mirroring `load_skinned_model`, so the sweep keeps going and the
    /// model simply has no hit-zone entry. Idempotent re-install replaces the
    /// entry. The derived bound is computed once, here, from the loaded clips.
    pub(crate) fn insert_from_load(&mut self, model_rel: &str, content_root: &Path) {
        let open_path = content_root.join(model_rel);
        let model = match gltf_loader::load_model(&open_path) {
            Ok(m) => m,
            Err(err) => {
                log::warn!(
                    "[HitZones] model load failed for {} : {err} — no hit-zone entry",
                    open_path.display(),
                );
                return;
            }
        };

        let gltf_loader::LoadedModel {
            skeleton,
            clips,
            joint_zones,
            ..
        } = model;

        // Only zone-bearing models get a derived bound; an AABB-only model needs
        // none here (it falls back to the authored hitbox in the consumer).
        let derived_bound = if ModelHitZones::has_zones(&joint_zones) {
            Some(derive_bound(&skeleton, &clips, &joint_zones))
        } else {
            None
        };

        let handle = ModelHandle::from(model_rel.to_string());
        self.models.insert(
            handle,
            ModelHitZones {
                skeleton: Arc::new(skeleton),
                clips: Arc::new(clips),
                joint_zones,
                derived_bound,
            },
        );
    }

    /// The retained hit-zone data for a model handle, or `None` if the model was
    /// never loaded (or its load failed). The Task 4 raycast facility looks the
    /// per-instance model up here.
    #[cfg_attr(not(test), allow(dead_code))] // Task 4 (raycast facility) lands the consumer.
    pub(crate) fn get(&self, handle: &ModelHandle) -> Option<&ModelHitZones> {
        self.models.get(handle)
    }
}

/// Capsule radius for joint `i`: the authored `hitZoneRadius` when present, the
/// engine default [`DEFAULT_ZONE_RADIUS`] when the zone omits it, and the default
/// for a non-zone joint too (a non-zone joint still occupies space the swept bound
/// must enclose, so it inflates by the default rather than collapsing to a point).
fn joint_radius(joint_zones: &[Option<JointZone>], i: usize) -> f32 {
    match joint_zones.get(i).and_then(Option::as_ref) {
        Some(JointZone {
            radius: Some(r), ..
        }) => *r,
        _ => DEFAULT_ZONE_RADIUS,
    }
}

/// Derive a zone-bearing model's model-local broad-phase AABB from its clips.
///
/// Samples EVERY clip's joint world positions at [`BOUND_SAMPLES_PER_CLIP`]
/// FIXED uniform times including both endpoints (`t = 0` and `t = duration`),
/// unions every joint position across every sample, and inflates each by that
/// joint's capsule radius (authored, or [`DEFAULT_ZONE_RADIUS`] when omitted) —
/// so an animated limb at the far end of its swing is enclosed with its full
/// capsule. The authored hitbox is deliberately NOT consulted: a limb must never
/// silently swing outside the broad phase.
///
/// Deterministic (fixed sample times, fixed order) and computed once at load.
/// A model with no clips still yields a well-formed bound over the rest pose
/// (each clip falls back to rest for absent channels; with zero clips the box is
/// the loader's zero box). Inflating by a sphere per joint (position ± radius on
/// every axis) is a conservative superset of the true capsule, which is exactly
/// what a broad phase wants.
fn derive_bound(
    skeleton: &Skeleton,
    clips: &[AnimationClip],
    joint_zones: &[Option<JointZone>],
) -> Aabb {
    let mut bound = Aabb::empty();
    // Reused across clips/samples — `sample_clip_looped_world` clears and refills.
    let mut world = Vec::new();

    for clip in clips {
        for sample in 0..BOUND_SAMPLES_PER_CLIP {
            let t = sample_time(clip.duration, sample);
            // Clamp so the final sample lands exactly on the clip end (no wrap to
            // frame 0), and earlier samples read their authored interior pose.
            sample_clip_looped_world(clip, skeleton, t, Loop::Clamp, &mut world);
            for (i, joint_world) in world.iter().enumerate() {
                let pos = joint_world.w_axis.truncate();
                let r = joint_radius(joint_zones, i);
                bound.expand(pos + Vec3::splat(r));
                bound.expand(pos - Vec3::splat(r));
            }
        }
    }

    // No clips (or an empty skeleton) leaves the box inverted; collapse it to a
    // well-formed zero box, matching `Aabb::from_points`' empty contract.
    if bound.min.x > bound.max.x {
        return Aabb::default();
    }
    bound
}

/// The fixed time for `sample` in `0..BOUND_SAMPLES_PER_CLIP` across a clip of
/// `duration` seconds: uniformly spaced with sample 0 at `t = 0` and the last
/// sample at `t = duration`. A non-positive duration (static clip) collapses
/// every sample to `0`. Deterministic by construction.
fn sample_time(duration: f32, sample: usize) -> f32 {
    if duration <= 0.0 || BOUND_SAMPLES_PER_CLIP <= 1 {
        return 0.0;
    }
    let frac = sample as f32 / (BOUND_SAMPLES_PER_CLIP - 1) as f32;
    frac * duration
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Mat4, Quat, Vec3};

    use crate::model::skeleton::{Interp, Joint, JointTracks, RestLocal, Track};

    fn joint(parent: Option<usize>, rest: RestLocal) -> Joint {
        Joint {
            parent,
            inverse_bind: Mat4::IDENTITY.to_cols_array_2d(),
            rest_local: rest,
        }
    }

    fn zone(tag: &str, radius: Option<f32>) -> Option<JointZone> {
        Some(JointZone {
            tag: tag.to_string(),
            radius,
        })
    }

    // --- sample_time: the deterministic uniform-with-endpoints contract ------

    #[test]
    fn sample_time_spans_zero_to_duration_uniformly() {
        let duration = 2.0;
        let times: Vec<f32> = (0..BOUND_SAMPLES_PER_CLIP)
            .map(|s| sample_time(duration, s))
            .collect();
        assert_eq!(times.len(), BOUND_SAMPLES_PER_CLIP);
        assert!(
            BOUND_SAMPLES_PER_CLIP >= 8,
            "plan floor: ≥8 samples per clip"
        );
        // Endpoints included exactly.
        assert!((times[0] - 0.0).abs() < 1.0e-6, "first sample is t=0");
        assert!(
            (times[BOUND_SAMPLES_PER_CLIP - 1] - duration).abs() < 1.0e-6,
            "last sample is t=duration"
        );
        // Strictly ascending and uniformly spaced.
        let step = duration / (BOUND_SAMPLES_PER_CLIP - 1) as f32;
        for s in 1..BOUND_SAMPLES_PER_CLIP {
            assert!(
                (times[s] - times[s - 1] - step).abs() < 1.0e-5,
                "uniform step"
            );
        }
    }

    #[test]
    fn sample_time_static_clip_collapses_to_zero() {
        for s in 0..BOUND_SAMPLES_PER_CLIP {
            assert_eq!(sample_time(0.0, s), 0.0, "non-positive duration → t=0");
        }
    }

    // --- derived bound: encloses the SWEPT limb + radius, hitbox-independent --

    /// A two-joint skeleton whose child translates along +X over the clip. The
    /// derived bound must enclose the child's FULL swept range (0 → 10 on X) plus
    /// the joint capsule radius, and must do so independent of any authored
    /// hitbox (no hitbox is consulted — the bound is computed from the clip).
    #[test]
    fn derived_bound_encloses_full_swept_limb_plus_radius() {
        let skeleton = Skeleton {
            joints: vec![
                joint(None, RestLocal::default()),
                joint(Some(0), RestLocal::default()),
            ],
        };
        // Child joint (index 1) sweeps 0 → 10 on +X across the clip; root holds.
        let child_tracks = JointTracks {
            translation: Track {
                times: vec![0.0, 1.0],
                values: vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)],
                mode: Interp::Linear,
            },
            ..Default::default()
        };
        let clip = AnimationClip {
            name: "swing".into(),
            duration: 1.0,
            joints: vec![JointTracks::default(), child_tracks],
        };
        // Zone on the child with an explicit radius; root has none (default).
        let child_radius = 0.5;
        let joint_zones = vec![None, zone("arm", Some(child_radius))];

        let bound = derive_bound(&skeleton, std::slice::from_ref(&clip), &joint_zones);

        // The child reaches x=10 at the clip end; the bound must include x=10 plus
        // its capsule radius — proving the SWEPT extreme (not just t=0) is captured
        // and that the per-joint radius inflated it.
        assert!(
            bound.max.x >= 10.0 + child_radius - 1.0e-4,
            "bound max.x {} must enclose swept limb tip (10) + radius ({child_radius})",
            bound.max.x
        );
        // The near end (x=0) minus the default radius is the min (root at origin
        // with default 0.12 radius, child also passes through x=0 at t=0).
        assert!(
            bound.min.x <= 0.0 - DEFAULT_ZONE_RADIUS + 1.0e-4,
            "bound min.x {} must enclose the near pose minus radius",
            bound.min.x
        );

        // Hitbox independence: an authored hitbox of, say, 1m would NOT contain
        // x=10; the derived bound does. We assert the bound is far larger than any
        // plausible small authored box — i.e. the clip, not a hitbox, drove it.
        assert!(
            bound.max.x > 5.0,
            "derived bound is driven by the clip sweep, not a small authored hitbox"
        );
    }

    /// An intermediate (mid-swing) sample must be inside the bound too — pinning
    /// that the union covers the whole arc, not merely the two endpoints. A joint
    /// that arcs through +Y between two endpoints both on the X axis would escape
    /// an endpoints-only box; the ≥8 interior samples catch it.
    #[test]
    fn derived_bound_covers_mid_swing_arc_not_just_endpoints() {
        let skeleton = Skeleton {
            joints: vec![joint(None, RestLocal::default())],
        };
        // Three keys: start (0,0,0), peak (0,8,0), end (0,0,0). Endpoints share
        // y=0; the arc bulges to y=8 only mid-clip.
        let tracks = JointTracks {
            translation: Track {
                times: vec![0.0, 0.5, 1.0],
                values: vec![Vec3::ZERO, Vec3::new(0.0, 8.0, 0.0), Vec3::ZERO],
                mode: Interp::Linear,
            },
            ..Default::default()
        };
        let clip = AnimationClip {
            name: "arc".into(),
            duration: 1.0,
            joints: vec![tracks],
        };
        let joint_zones = vec![zone("head", Some(0.1))];

        let bound = derive_bound(&skeleton, std::slice::from_ref(&clip), &joint_zones);

        // An endpoints-only box would have max.y ≈ 0.1 (radius at y=0). The mid
        // samples must push it toward the y=8 peak — uniform samples at 1/7..6/7
        // hit the linear ramp, the closest landing near the peak.
        assert!(
            bound.max.y > 4.0,
            "mid-swing arc (peak y=8) must be enclosed; max.y {} too small \
             (endpoints-only would miss it)",
            bound.max.y
        );
    }

    /// A joint zone with NO authored radius inflates by the engine default
    /// (0.12 m), per the locked decision that the default is applied here, not at
    /// load (`JointZone::radius` stays `None` from the loader).
    #[test]
    fn omitted_radius_inflates_by_engine_default() {
        let skeleton = Skeleton {
            joints: vec![joint(None, RestLocal::default())],
        };
        // Single joint pinned at the origin for the whole (static) clip.
        let clip = AnimationClip {
            name: "rest".into(),
            duration: 1.0,
            joints: vec![JointTracks::default()],
        };
        // Zone present, radius omitted → default applies.
        let joint_zones = vec![zone("torso", None)];

        let bound = derive_bound(&skeleton, std::slice::from_ref(&clip), &joint_zones);

        // Joint at origin, inflated by ±DEFAULT_ZONE_RADIUS on every axis.
        let expected = DEFAULT_ZONE_RADIUS;
        assert!(
            (bound.max - Vec3::splat(expected)).length() < 1.0e-4,
            "max should be +default on each axis, got {:?}",
            bound.max
        );
        assert!(
            (bound.min + Vec3::splat(expected)).length() < 1.0e-4,
            "min should be -default on each axis, got {:?}",
            bound.min
        );
    }

    /// An AABB-only model (no joint zones) gets no derived bound; a zone-bearing
    /// model does. Also pins the `has_zones` gate the store uses.
    #[test]
    fn store_computes_bound_only_for_zone_bearing_models() {
        assert!(
            !ModelHitZones::has_zones(&[None, None]),
            "no zones → AABB-only model"
        );
        assert!(
            ModelHitZones::has_zones(&[None, zone("head", None)]),
            "any zone → zone-bearing model"
        );
    }

    /// The store retains skeleton + clips + zone table per handle (Arc-shared),
    /// looks them up by handle, and clears on level change. Built directly (no
    /// glTF file) so the test is hermetic.
    #[test]
    fn store_retains_clears_and_shares_arc() {
        let skeleton = Skeleton {
            joints: vec![joint(None, RestLocal::default())],
        };
        let clips = vec![AnimationClip {
            name: "idle".into(),
            duration: 1.0,
            joints: vec![JointTracks::default()],
        }];
        let joint_zones = vec![zone("head", Some(0.2))];

        let mut store = HitZoneStore::new();
        let handle = ModelHandle::from("models/mob/scene.gltf");
        store.models.insert(
            handle.clone(),
            ModelHitZones {
                skeleton: Arc::new(skeleton),
                clips: Arc::new(clips),
                joint_zones: joint_zones.clone(),
                derived_bound: Some(Aabb::default()),
            },
        );

        let entry = store.get(&handle).expect("entry retained by handle");
        assert_eq!(entry.skeleton.joints.len(), 1, "skeleton retained");
        assert_eq!(entry.clips.len(), 1, "clips retained");
        assert_eq!(entry.joint_zones, joint_zones, "zone table retained");
        assert!(entry.derived_bound.is_some(), "bound stored beside zones");

        // Arc share, not deep clone: a cheap handle clone points at the same data.
        let shared = Arc::clone(&entry.clips);
        assert!(
            Arc::ptr_eq(&shared, &store.get(&handle).unwrap().clips),
            "clips are Arc-shared (no keyframe deep-clone on the per-shot path)"
        );

        store.clear();
        assert!(store.get(&handle).is_none(), "cleared on level change");
    }

    /// A miss (unloaded handle) returns `None` rather than panicking.
    #[test]
    fn store_lookup_miss_is_none() {
        let store = HitZoneStore::new();
        assert!(
            store.get(&ModelHandle::from("never/loaded.gltf")).is_none(),
            "an unloaded handle has no entry"
        );
    }
}
