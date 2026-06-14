// Game-side skeletal hit-zone store + the standalone entity-raycast facility:
// retains each model TYPE's CPU skeleton, clips, and authored joint-zone table
// plus a derived broad-phase bound swept from the clips, and resolves the
// nearest TARGETABLE entity a ray strikes (authored-AABB or bone-posed-capsule).
//
// CPU-only — no wgpu, no `crate::render`. nalgebra is confined to the `parry3d`
// ray/capsule boundary inside this module; engine-facing types are all `glam`.
// See: context/lib/entity_model.md §7 · rendering_pipeline.md §9

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
use parry3d::math::{Isometry, Point, Vector};
use parry3d::query::RayCast;
use parry3d::shape::{Ball, Capsule};

use crate::lighting::cone_frustum::Aabb;
use crate::model::ModelHandle;
use crate::model::anim::{BlendSource, Loop, sample_blended_world, sample_clip_looped_world};
use crate::model::gltf_loader::{self, JointZone};
use crate::model::sample_params::{ClipSample, FadeSource, MeshSampleParams};
use crate::model::skeleton::{AnimationClip, Skeleton};
use crate::scripting::components::health::HealthComponent;
use crate::scripting::components::mesh::{MeshAnimation, MeshComponent};
use crate::scripting::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};

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
/// The fields are read by the Task 4 raycast facility ([`nearest_entity_hit`])
/// in this module — the live shipping consumer.
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
/// level-load model sweep and cleared on level change. Consumed by the Task 4
/// raycast facility ([`nearest_entity_hit`]), which the weapon hitscan delegates
/// to.
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
    pub(crate) fn get(&self, handle: &ModelHandle) -> Option<&ModelHitZones> {
        self.models.get(handle)
    }

    /// Install a pre-built model entry under `handle` for tests in OTHER modules
    /// (the weapon delegation tests) that cannot reach the private `models` map.
    /// Production installs go through [`insert_from_load`](Self::insert_from_load).
    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, handle: ModelHandle, model: ModelHitZones) {
        self.models.insert(handle, model);
    }
}

/// Cross-check an archetype's DECLARED zone-multiplier tags against the zone tags
/// a model actually carries, RETURNING the declared tags that name no zone on the
/// model (sorted, deduplicated). A health descriptor's `zone_multipliers` keys
/// are the declared set; `joint_zones` is the model's authored zone table.
///
/// Pure data logic (no logging) so the unknown set is unit-testable, mirroring
/// [`super::mesh_anim::resolve_state_clips`] returning its `MissingClip` set. The
/// caller (level-load validation in `main.rs`) warns once per archetype per
/// returned tag.
pub(crate) fn unknown_zone_multiplier_tags<'a>(
    declared: impl IntoIterator<Item = &'a str>,
    joint_zones: &[Option<JointZone>],
) -> Vec<String> {
    let known: std::collections::HashSet<&str> = joint_zones
        .iter()
        .filter_map(|z| z.as_ref().map(|j| j.tag.as_str()))
        .collect();
    let mut unknown: Vec<String> = declared
        .into_iter()
        .filter(|tag| !known.contains(tag))
        .map(str::to_string)
        .collect();
    unknown.sort();
    unknown.dedup();
    unknown
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

// ---------------------------------------------------------------------------
// Entity-raycast facility (Task 4)
//
// A standalone, weapon-agnostic ray query: given ANY origin / direction / range
// it walks every TARGETABLE entity and returns the nearest hit. "Targetable"
// widens entity_model.md §7's old "hitbox present" rule to: health AND (an
// authored AABB hitbox OR a zone-bearing skinned model). The weapon's hitscan
// delegates here; so can any future system (no weapon/camera type in the
// signature). nalgebra never leaves this section — it is converted to/from glam
// at the `parry3d` boundary.
// ---------------------------------------------------------------------------

/// One resolved entity hit along a ray. `toi` is the ray parameter (distance,
/// since `direction` is unit length) used to pick the nearest of several
/// contenders; `point`/`normal` are world-space; `target` is the struck entity;
/// `zone` is the authored zone tag of the struck bone capsule (`None` for an
/// AABB-only entity, or a zone-bearing entity struck via a future non-tagged
/// path that never arises today).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EntityRayHit {
    /// Ray parameter (distance) of the entry impact, in `[0, range]`.
    pub(crate) toi: f32,
    /// World-space impact point (`origin + direction * toi`).
    pub(crate) point: Vec3,
    /// World-space surface normal at the impact, signed back toward the ray.
    pub(crate) normal: Vec3,
    /// The entity struck.
    pub(crate) target: EntityId,
    /// The authored zone tag of the struck bone capsule, if the hit landed on a
    /// skeletal hit zone. `None` for an authored-AABB hit.
    pub(crate) zone: Option<String>,
}

/// Walk every TARGETABLE entity and return the nearest hit along the ray within
/// `range`, or `None` when no targetable entity lies on the ray.
///
/// Standalone and weapon-agnostic: callable with any `origin` / `direction`
/// (assumed unit length) / `range`. Targetable = health AND (an authored AABB
/// hitbox OR a zone-bearing skinned model). For each:
/// - **AABB-only** (health + hitbox, no zone-bearing model): broad phase is the
///   authored AABB; narrow phase is the same AABB via [`ray_aabb_slab`].
/// - **Zone-bearing** (health + a model with ≥1 tagged joint): broad phase is the
///   model's derived bound (model-local, transformed to a world-axis-aligned
///   enclosure by the entity's position+yaw); narrow phase poses the skeleton at
///   the entity's current animation time and ray-tests one capsule per tagged
///   joint. A zone-bearing model with NO tagged joints keeps AABB behavior.
///
/// Zero-HP entities (pending-despawn this tick) are skipped so a corpse cannot
/// absorb a shot for one frame. `anim_time` is the game-layer animation clock;
/// `store` is the per-model hit-zone data (Task 3). The result's `zone` carries
/// the struck bone's tag for a capsule hit (Task 5 reads it).
pub(crate) fn nearest_entity_hit(
    registry: &EntityRegistry,
    store: &HitZoneStore,
    anim_time: f64,
    origin: Vec3,
    direction: Vec3,
    range: f32,
) -> Option<EntityRayHit> {
    let mut nearest: Option<EntityRayHit> = None;

    for (id, value) in registry.iter_with_kind(ComponentKind::Health) {
        let ComponentValue::Health(HealthComponent {
            current, hitbox, ..
        }) = value
        else {
            continue;
        };

        // Zero-HP entities are pending-despawn this tick (the death sweep runs
        // after weapon fire); skip them so a corpse cannot absorb a shot and
        // block the wall behind it for one frame. `current` is floored at exactly
        // 0.0 by the apply_damage chokepoint — exact equality is sound.
        if *current == 0.0 {
            continue;
        }

        let Ok(transform) = registry.get_component::<Transform>(id) else {
            continue;
        };

        // Prefer the zone-bearing path when the entity's model carries tagged
        // joints; otherwise fall back to the authored AABB. A zone-bearing model
        // with NO tagged joints has no derived bound and falls through to AABB.
        let zoned = zone_bearing_entry(registry, store, id);

        let hit = match (zoned, hitbox) {
            (Some(zones), _) => {
                // The entity's animation, if any, drives the posed skeleton; a
                // mesh with no animation block (stateless prop) poses to the
                // model's first clip at the clock.
                let animation = registry
                    .get_component::<MeshComponent>(id)
                    .ok()
                    .and_then(|m| m.animation.clone());
                nearest_zone_hit(
                    zones,
                    transform,
                    animation.as_ref(),
                    anim_time,
                    id,
                    origin,
                    direction,
                    range,
                )
            }
            (None, Some(hitbox)) => {
                let center = transform.position + hitbox.offset;
                ray_aabb_slab(
                    origin,
                    direction,
                    center - hitbox.half_extents,
                    center + hitbox.half_extents,
                    range,
                )
                .map(|(toi, normal)| EntityRayHit {
                    toi,
                    point: origin + direction * toi,
                    normal,
                    target: id,
                    zone: None,
                })
            }
            // Health but neither an authored hitbox nor a zone-bearing model:
            // not targetable.
            (None, None) => None,
        };

        if let Some(hit) = hit {
            if nearest.as_ref().is_none_or(|n| hit.toi < n.toi) {
                nearest = Some(hit);
            }
        }
    }

    nearest
}

/// The model hit-zone entry for an entity IF its mesh model is zone-bearing (has
/// a derived bound, i.e. ≥1 tagged joint). `None` when the entity has no mesh,
/// the model is unloaded, or the model carries no zone tags — in which case the
/// caller falls back to the authored AABB (today's behavior, byte-identical).
fn zone_bearing_entry<'a>(
    registry: &EntityRegistry,
    store: &'a HitZoneStore,
    id: EntityId,
) -> Option<&'a ModelHitZones> {
    let mesh = registry.get_component::<MeshComponent>(id).ok()?;
    let entry = store.get(&ModelHandle::from(mesh.model.clone()))?;
    // Only a zone-bearing model (derived bound present) takes the capsule path.
    entry.derived_bound.as_ref().map(|_| entry)
}

/// Ray-test one zone-bearing entity: broad phase against the model's derived
/// bound (transformed to a world-axis-aligned enclosure), then per tagged joint
/// a posed-capsule narrow test. Returns the nearest capsule hit within `range`,
/// or `None` (broad-phase reject, or no tagged capsule on the ray).
#[allow(clippy::too_many_arguments)] // a flat parameter list keeps the facility weapon/camera-free.
fn nearest_zone_hit(
    zones: &ModelHitZones,
    transform: &Transform,
    animation: Option<&MeshAnimation>,
    anim_time: f64,
    id: EntityId,
    origin: Vec3,
    direction: Vec3,
    range: f32,
) -> Option<EntityRayHit> {
    // Model→world by POSITION + YAW only (no pitch/roll/scale) — the game-tick
    // placement, deliberately NOT the renderer's interpolated transform.
    let model_to_world = position_yaw_matrix(transform);

    // Broad phase: the derived bound is model-local; transform it to a tight
    // world-axis-aligned enclosure and ray-test that AABB. A reject here means
    // no capsule can be hit (the bound encloses every posed capsule by
    // construction), so we skip the narrow phase entirely.
    let bound = zones.derived_bound.as_ref()?.transformed(&model_to_world);
    ray_aabb_slab(origin, direction, bound.min, bound.max, range)?;

    // Narrow phase: pose the skeleton at the entity's current animation time and
    // test one capsule per tagged joint.
    let world_joints = pose_world_joints(zones, animation, anim_time);

    let mut nearest: Option<EntityRayHit> = None;
    for (joint_index, zone) in zones.joint_zones.iter().enumerate() {
        let Some(zone) = zone else {
            continue; // untagged joints are not hittable
        };
        let Some(world_joint) = world_joints.get(joint_index) else {
            continue;
        };

        // Capsule segment: this joint's posed origin to its FIRST CHILD's posed
        // origin. A tagged LEAF joint (no child) is a zero-length sphere.
        let a_model = world_joint.w_axis.truncate();
        let b_model = first_child_origin(zones, &world_joints, joint_index);
        let radius = zone.radius.unwrap_or(DEFAULT_ZONE_RADIUS);

        // Model→world for the two segment endpoints (position + yaw only).
        let a = model_to_world.transform_point3(a_model);
        let b_world = b_model.map(|b| model_to_world.transform_point3(b));

        let Some((toi, normal)) = ray_capsule_or_ball(origin, direction, a, b_world, radius, range)
        else {
            continue;
        };

        if nearest.as_ref().is_none_or(|n| toi < n.toi) {
            nearest = Some(EntityRayHit {
                toi,
                point: origin + direction * toi,
                normal,
                target: id,
                zone: Some(zone.tag.clone()),
            });
        }
    }

    nearest
}

/// The model-local posed-origin of `joint_index`'s FIRST CHILD (lowest joint
/// index whose parent is `joint_index`), or `None` for a leaf — which the caller
/// renders as a zero-length sphere at the joint origin.
fn first_child_origin(
    zones: &ModelHitZones,
    world_joints: &[Mat4],
    joint_index: usize,
) -> Option<Vec3> {
    zones
        .skeleton
        .joints
        .iter()
        .enumerate()
        .find(|(_, joint)| joint.parent == Some(joint_index))
        .and_then(|(child_index, _)| world_joints.get(child_index))
        .map(|child| child.w_axis.truncate())
}

/// Compose the entity's model→world matrix from POSITION + YAW only. Pitch, roll,
/// and scale are deliberately dropped: hit zones use the game-tick placement, not
/// the renderer's interpolated full transform. Yaw is extracted from the stored
/// quaternion as rotation about world +Y.
fn position_yaw_matrix(transform: &Transform) -> Mat4 {
    let yaw = yaw_of(transform.rotation);
    Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), transform.position)
}

/// The yaw angle (rotation about world +Y) of a quaternion. Projects the
/// quaternion's forward direction onto the XZ plane and takes its heading, so
/// any pitch/roll baked into the quaternion is discarded.
fn yaw_of(rotation: Quat) -> f32 {
    let forward = rotation * Vec3::NEG_Z;
    forward.x.atan2(-forward.z)
}

/// Pose the model's skeleton at `anim_time` into per-joint MODEL-space world
/// matrices (pre-inverse-bind).
///
/// When the entity carries an animation block, its current state resolves —
/// through the SAME render-free [`mesh_anim::animate_entity`] the renderer's
/// collector uses — to a [`MeshSampleParams`] (primary clip leg + optional
/// crossfade), which [`pose_from_params`] then samples (single or blended). An
/// unresolved state, or a stateless / no-animation mesh, poses the model's first
/// clip looped at the clock; a model with no clips poses each joint to rest.
///
/// Phase de-sync is intentionally omitted: it is a looping-only cosmetic offset
/// the renderer applies per instance (`mesh_instances::instance_phase`, a render
/// module the facility must not import), so the facility hit-tests at the raw
/// animation clock — faithful in pose, minus a cosmetic looping offset.
fn pose_world_joints(
    zones: &ModelHitZones,
    animation: Option<&MeshAnimation>,
    anim_time: f64,
) -> Vec<Mat4> {
    let mut out = Vec::new();
    let skeleton = zones.skeleton.as_ref();
    let clips = zones.clips.as_ref();

    // Resolve the entity's current animation to render-free sample params via the
    // shared resolver (phase 0 — see the doc note). `animate_entity` returns
    // `None` for an unresolved current state; fall through to the default pose.
    if let Some(params) = animation
        .and_then(|anim| super::mesh_anim::animate_entity(anim, anim_time, 0.0).map(|r| r.sample))
    {
        pose_from_params(skeleton, clips, &params, &mut out);
        return out;
    }

    // Default pose: the model's first clip, looped at the clock; or rest if the
    // model carries no clips at all.
    match clips.first() {
        Some(clip) => {
            sample_clip_looped_world(clip, skeleton, anim_time as f32, Loop::Wrap, &mut out)
        }
        None => pose_rest(skeleton, &mut out),
    }
    out
}

/// Pose the model's skeleton at `anim_time` per a resolved [`MeshSampleParams`]:
/// sample the primary clip alone, or blend the primary against the active fade's
/// FROM-leg. A [`FadeSource::Snapshot`] from-leg degrades to its carried
/// `fallback` clip (the snapshot store is renderer-owned and unreadable game-
/// side, so a snapshot fade samples the incoming clip — accepted scope). Writes
/// per-joint MODEL-space world matrices into `out`.
fn pose_from_params(
    skeleton: &Skeleton,
    clips: &[AnimationClip],
    params: &MeshSampleParams,
    out: &mut Vec<Mat4>,
) {
    let primary_src = clip_blend_source(clips, &params.primary);
    let Some(primary) = primary_src else {
        pose_rest(skeleton, out);
        return;
    };

    match params.fade {
        Some(fade) => {
            // Degrade a snapshot from-leg to its carried fallback clip.
            let from_leg = match fade.from {
                FadeSource::Clip(leg) => leg,
                FadeSource::Snapshot { fallback, .. } => fallback,
            };
            match clip_blend_source(clips, &from_leg) {
                Some(from) => sample_blended_world(&from, &primary, fade.weight, skeleton, out),
                // Missing from-clip: sample the primary alone.
                None => sample_clip_looped_world(
                    clip_of(clips, params.primary.clip_index).unwrap(),
                    skeleton,
                    params.primary.time,
                    params.primary.loop_policy,
                    out,
                ),
            }
        }
        None => sample_clip_looped_world(
            clip_of(clips, params.primary.clip_index).unwrap(),
            skeleton,
            params.primary.time,
            params.primary.loop_policy,
            out,
        ),
    }
}

/// Build a [`BlendSource::Clip`] for a [`ClipSample`] leg against `clips`, or
/// `None` if its index is out of range.
fn clip_blend_source<'a>(clips: &'a [AnimationClip], leg: &ClipSample) -> Option<BlendSource<'a>> {
    clip_of(clips, leg.clip_index).map(|clip| BlendSource::Clip {
        clip,
        time: leg.time,
        loop_policy: leg.loop_policy,
    })
}

/// The clip at `index`, or `None` if out of range.
fn clip_of(clips: &[AnimationClip], index: usize) -> Option<&AnimationClip> {
    clips.get(index)
}

/// Pose the skeleton to its REST hierarchy (no clip): sample an empty static
/// clip so every joint holds its rest-local, composed parent-before-child.
fn pose_rest(skeleton: &Skeleton, out: &mut Vec<Mat4>) {
    let rest_clip = AnimationClip {
        name: String::new(),
        duration: 0.0,
        joints: Vec::new(),
    };
    sample_clip_looped_world(&rest_clip, skeleton, 0.0, Loop::Clamp, out);
}

/// Ray-test a segment-defined capsule (or a ball, for a zero-length leaf joint).
/// `a`/`b_world` are the world-space segment endpoints; `None` `b_world` means a
/// leaf joint → a [`Ball`] of `radius` at `a`. Uses `parry3d` ray casting with
/// the shape placed by an `Isometry` translation (the shape carries its own
/// segment geometry, so the placement is a pure translate — no `+Y` `cast_capsule`
/// convention). Returns `(toi, world normal)` clamped to `range`, or `None`.
fn ray_capsule_or_ball(
    origin: Vec3,
    direction: Vec3,
    a: Vec3,
    b_world: Option<Vec3>,
    radius: f32,
    range: f32,
) -> Option<(f32, Vec3)> {
    let ray = parry3d::query::Ray::new(
        Point::new(origin.x, origin.y, origin.z),
        Vector::new(direction.x, direction.y, direction.z),
    );

    // Place the shape at the origin; its geometry is defined directly in world
    // space (capsule segment endpoints, or a ball at `a`). nalgebra stays here.
    let isometry = Isometry::identity();

    let intersection = match b_world {
        Some(b) if (b - a).length() > 1.0e-6 => {
            let capsule =
                Capsule::new(Point::new(a.x, a.y, a.z), Point::new(b.x, b.y, b.z), radius);
            capsule.cast_ray_and_get_normal(&isometry, &ray, range, true)
        }
        // Leaf joint (or a degenerate zero-length segment): a sphere at `a`.
        _ => {
            let ball = Ball::new(radius);
            let ball_at = Isometry::translation(a.x, a.y, a.z);
            ball.cast_ray_and_get_normal(&ball_at, &ray, range, true)
        }
    }?;

    let toi = intersection.time_of_impact;
    if toi > range {
        return None;
    }
    let normal = Vec3::new(
        intersection.normal.x,
        intersection.normal.y,
        intersection.normal.z,
    );
    Some((toi, normal))
}

/// Ray-vs-AABB slab test. Returns the entry time-of-impact (clamped to
/// `[0, range]`) and the face normal of the entered slab — the axis whose
/// near plane the ray crossed last, signed toward the ray origin so the impact
/// burst ejects back along the shot. Returns `None` on a miss, when the box is
/// entirely behind the origin, or when entry lies beyond `range`.
///
/// Relocated here verbatim from the weapon module (behavior-preserving) so the
/// facility owns both the AABB and capsule narrow phases. A degenerate
/// (zero-thickness) slab on an axis the ray runs parallel to is handled by the
/// IEEE-754 infinity arithmetic of `1.0 / 0.0`: an origin outside the slab on
/// that axis yields `±inf` bounds that fail the overlap test (miss), and inside
/// the slab yields a `-inf..inf` span that never constrains entry.
pub(crate) fn ray_aabb_slab(
    origin: Vec3,
    direction: Vec3,
    aabb_min: Vec3,
    aabb_max: Vec3,
    range: f32,
) -> Option<(f32, Vec3)> {
    let inv = Vec3::ONE / direction;

    // Per-axis slab entry/exit times. `t1`/`t2` are the unordered crossings;
    // `near`/`far` reorder them so `near <= far` regardless of ray direction.
    let t1 = (aabb_min - origin) * inv;
    let t2 = (aabb_max - origin) * inv;
    let near = t1.min(t2);
    let far = t1.max(t2);

    // Latest entry across all three slabs, earliest exit across all three.
    let t_entry = near.x.max(near.y).max(near.z);
    let t_exit = far.x.min(far.y).min(far.z);

    // Miss: the slabs do not overlap, or the box is entirely behind the origin,
    // or entry is beyond range.
    if t_entry > t_exit || t_exit < 0.0 || t_entry > range {
        return None;
    }

    // An origin inside the box has a negative entry; clamp the reported hit to
    // the origin (toi 0). The struck face is the axis of the latest entry slab.
    let toi = t_entry.max(0.0);

    let axis = if near.x >= near.y && near.x >= near.z {
        Vec3::X
    } else if near.y >= near.z {
        Vec3::Y
    } else {
        Vec3::Z
    };
    // Sign the normal toward the ray origin (against the ray on that axis).
    let normal = axis * -direction.dot(axis).signum();

    Some((toi, normal))
}

#[cfg(test)]
mod tests {
    // `Mat4`, `Quat`, `Vec3`, and the `parry3d` / sample-param imports come
    // through `super::*` (the module's own imports).
    use super::*;

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

    // --- Entity-raycast facility (Task 4) -----------------------------------

    use crate::scripting::components::health::{HealthComponent, Hitbox};
    use crate::scripting::components::mesh::MeshComponent;
    use crate::scripting::registry::{EntityRegistry, Transform};

    const FACILITY_EPS: f32 = 1.0e-4;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < FACILITY_EPS
    }

    /// A two-joint skeleton: root (joint 0) at the origin, child (joint 1) a
    /// TAGGED LEAF. The child translates along +X over a 1s looping clip from
    /// `0` to `5`, so the live pose moves the child sphere far from its rest
    /// (origin) position. Only the leaf is tagged, so exactly one sphere is
    /// hittable — at the CHILD's posed origin.
    fn swinging_limb_model() -> ModelHitZones {
        let skeleton = Skeleton {
            joints: vec![
                joint(None, RestLocal::default()),
                joint(Some(0), RestLocal::default()),
            ],
        };
        // Duration 2.0 with the tip at x=10 at t=2, so sampling at t=1 gives x=5
        // WITHOUT wrapping (Loop::Wrap maps t==duration back to 0; t=1 < 2 is a
        // genuine interior sample).
        let child_tracks = JointTracks {
            translation: Track {
                times: vec![0.0, 2.0],
                values: vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)],
                mode: Interp::Linear,
            },
            ..Default::default()
        };
        let clip = AnimationClip {
            name: "swing".into(),
            duration: 2.0,
            joints: vec![JointTracks::default(), child_tracks],
        };
        // Root untagged; child tagged leaf with an explicit radius.
        let joint_zones = vec![None, zone("hand", Some(0.3))];
        let derived_bound = Some(derive_bound(
            &skeleton,
            std::slice::from_ref(&clip),
            &joint_zones,
        ));
        ModelHitZones {
            skeleton: Arc::new(skeleton),
            clips: Arc::new(vec![clip]),
            joint_zones,
            derived_bound,
        }
    }

    /// Install a model in the store under `handle`.
    fn store_with(handle: &str, model: ModelHitZones) -> HitZoneStore {
        let mut store = HitZoneStore::new();
        store.models.insert(ModelHandle::from(handle), model);
        store
    }

    /// Spawn a health + stateless-mesh entity (no animation block → poses the
    /// model's first clip looped at the animation clock).
    fn spawn_zone_entity(reg: &mut EntityRegistry, model: &str, position: Vec3) -> EntityId {
        let id = reg.spawn(Transform {
            position,
            ..Transform::default()
        });
        reg.set_component(
            id,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: None,
                death_handled: false,
                zone_multipliers: std::collections::HashMap::new(),
            },
        )
        .unwrap();
        reg.set_component(id, MeshComponent::stateless(model.to_string()))
            .unwrap();
        id
    }

    /// Spawn a health + authored-AABB entity (no mesh).
    fn spawn_aabb_entity(reg: &mut EntityRegistry, position: Vec3, half_extents: Vec3) -> EntityId {
        let id = reg.spawn(Transform {
            position,
            ..Transform::default()
        });
        reg.set_component(
            id,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: Some(Hitbox {
                    half_extents,
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                zone_multipliers: std::collections::HashMap::new(),
            },
        )
        .unwrap();
        id
    }

    /// AC: shooting a zone-bearing entity registers a hit only where the POSED
    /// capsules are. A ray through the limb's REST position misses; with the clip
    /// advanced so the limb moved, a ray through its POSED position hits. Sample
    /// time is driven directly via `anim_time`.
    #[test]
    fn posed_limb_hits_where_it_moved_not_at_rest() {
        let mut reg = EntityRegistry::new();
        let store = store_with("mob", swinging_limb_model());
        // Entity at the origin; the child leaf starts at the origin (rest) and
        // swings to world (5,0,0) at anim_time = 1.0.
        let id = spawn_zone_entity(&mut reg, "mob", Vec3::ZERO);

        // A ray straight down -Z through the POSED position x=5: a vertical line
        // at (5, 0, z) shooting toward -Z. At rest (t=0) the child is at the
        // origin → miss. Posed (t=1) the child is at (5,0,0) → hit.
        let origin = Vec3::new(5.0, 0.0, 10.0);
        let dir = Vec3::new(0.0, 0.0, -1.0);

        let at_rest = nearest_entity_hit(&reg, &store, 0.0, origin, dir, 100.0);
        assert!(
            at_rest.is_none(),
            "a ray through the limb's POSED position misses while it is at REST"
        );

        let posed = nearest_entity_hit(&reg, &store, 1.0, origin, dir, 100.0)
            .expect("the posed limb lies on the ray");
        assert_eq!(posed.target, id, "the zone entity is hit");
        assert_eq!(posed.zone.as_deref(), Some("hand"), "zone tag surfaced");
    }

    /// An ANIMATED zone entity poses through the shared `animate_entity`
    /// resolver: its current state's clip-local time (`anim_time - entered_at`)
    /// drives the limb. Entered at t=3, sampled at t=4 → clip-local 1.0 → child at
    /// x=5; a ray through x=5 hits while a ray through x=0 (the rest) misses.
    /// Exercises the `pose_from_params` single-clip path (no fade).
    #[test]
    fn animated_entity_poses_via_state_clip_local_time() {
        use crate::scripting::components::mesh::{AnimationState, InterruptPolicy, MeshAnimation};

        let mut reg = EntityRegistry::new();
        let store = store_with("mob", swinging_limb_model());

        // One looping state on clip index 0 (the swing clip), no crossfade.
        let mut states = HashMap::new();
        states.insert(
            "move".to_string(),
            AnimationState {
                clip: "swing".into(),
                looping: true,
                crossfade_ms: 0.0,
                interrupt: InterruptPolicy::Smooth,
                clip_index: Some(0),
            },
        );
        let mut anim = MeshAnimation::new(states, "move".into());
        anim.entered_at = Some(3.0); // resolved entry stamp

        let id = reg.spawn(Transform::default());
        reg.set_component(
            id,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: None,
                death_handled: false,
                zone_multipliers: std::collections::HashMap::new(),
            },
        )
        .unwrap();
        reg.set_component(
            id,
            MeshComponent {
                model: "mob".into(),
                animation: Some(anim),
            },
        )
        .unwrap();

        let origin = Vec3::new(5.0, 0.0, 10.0);
        let dir = Vec3::new(0.0, 0.0, -1.0);

        // anim_time 3.0 → clip-local 0.0 → child at rest (origin) → miss at x=5.
        assert!(
            nearest_entity_hit(&reg, &store, 3.0, origin, dir, 100.0).is_none(),
            "at the entry instant the limb is at rest; a ray through x=5 misses"
        );
        // anim_time 4.0 → clip-local 1.0 → child at x=5 → hit.
        let hit = nearest_entity_hit(&reg, &store, 4.0, origin, dir, 100.0)
            .expect("the state-posed limb lies on the ray");
        assert_eq!(hit.target, id);
        assert_eq!(hit.zone.as_deref(), Some("hand"));
    }

    /// AC: a ray through a posed limb OUTSIDE an authored-hitbox-sized box still
    /// hits — the derived bound admits it, so an undersized reference box must
    /// not cause a silent miss. The posed limb at x=5 is far outside any small
    /// AABB centered on the entity, yet the derived bound (swept to x=5+radius)
    /// admits the broad phase and the capsule narrow-phase hits.
    #[test]
    fn posed_limb_outside_small_box_still_hits_via_derived_bound() {
        let mut reg = EntityRegistry::new();
        let store = store_with("mob", swinging_limb_model());
        spawn_zone_entity(&mut reg, "mob", Vec3::ZERO);

        // The derived bound must reach past x=5 (limb tip + radius); a 0.5m box
        // would not. Confirm the bound is wide.
        let bound = store
            .get(&ModelHandle::from("mob"))
            .unwrap()
            .derived_bound
            .unwrap();
        assert!(
            bound.max.x >= 5.0,
            "derived bound encloses the swept limb tip (x>=5), not a tiny box"
        );

        let hit = nearest_entity_hit(
            &reg,
            &store,
            1.0,
            Vec3::new(5.0, 0.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            100.0,
        )
        .expect("posed limb outside a small box is still hit");
        assert_eq!(hit.zone.as_deref(), Some("hand"));
    }

    /// AC (key two-phase correctness): a zone-bearing entity is never hit by a
    /// ray that passes INSIDE its broad-phase bound but OUTSIDE every capsule.
    /// The bound spans x∈[~-0.3, ~5.3]; a ray through x=2.5 (inside the bound)
    /// but on the +Y side, far from the thin capsule line, must miss.
    #[test]
    fn ray_inside_bound_outside_capsule_misses() {
        let mut reg = EntityRegistry::new();
        let store = store_with("mob", swinging_limb_model());
        spawn_zone_entity(&mut reg, "mob", Vec3::ZERO);

        // Posed at t=1: the only capsule is the child sphere at (5,0,0), r=0.3.
        // Aim a -Z ray at (2.5, 3.0, z): x=2.5 is inside the bound's X span, but
        // (2.5, 3.0) is nowhere near the sphere at (5,0,0) → narrow-phase miss.
        let inside_bound_x = nearest_entity_hit(
            &reg,
            &store,
            1.0,
            Vec3::new(2.5, 3.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            100.0,
        );
        assert!(
            inside_bound_x.is_none(),
            "inside the broad bound but outside every capsule must NOT hit"
        );
    }

    /// AC: nearest-of ordering across an AABB-only entity and a zone-bearing
    /// entity in one scene. A nearer zone capsule beats a farther AABB and vice
    /// versa; the chosen target/zone reflects the nearer contender.
    #[test]
    fn nearest_of_aabb_and_zone_entities() {
        let mut reg = EntityRegistry::new();
        let store = store_with("mob", swinging_limb_model());

        // Zone entity at the origin: posed (t=1) its hand sphere sits at (5,0,0).
        // Place the zone entity so the hand sphere is NEAR along a -Z ray at x=5.
        let zone_id = spawn_zone_entity(&mut reg, "mob", Vec3::ZERO);
        // AABB entity also on the x=5 line but FARTHER down -Z.
        let _aabb_far = spawn_aabb_entity(&mut reg, Vec3::new(5.0, 0.0, -20.0), Vec3::splat(1.0));

        let origin = Vec3::new(5.0, 0.0, 10.0);
        let dir = Vec3::new(0.0, 0.0, -1.0);

        // Hand sphere at z=0 (toi 10) is nearer than the AABB at z=-20 (toi 30).
        let hit = nearest_entity_hit(&reg, &store, 1.0, origin, dir, 100.0)
            .expect("a contender lies on the ray");
        assert_eq!(
            hit.target, zone_id,
            "nearer zone capsule wins over the AABB"
        );
        assert_eq!(hit.zone.as_deref(), Some("hand"));

        // Now make the AABB nearer (z=5, toi 5) than the hand sphere (toi 10).
        let mut reg2 = EntityRegistry::new();
        let _zone = spawn_zone_entity(&mut reg2, "mob", Vec3::ZERO);
        let aabb_near = spawn_aabb_entity(&mut reg2, Vec3::new(5.0, 0.0, 5.0), Vec3::splat(0.5));
        let hit2 = nearest_entity_hit(&reg2, &store, 1.0, origin, dir, 100.0)
            .expect("a contender lies on the ray");
        assert_eq!(hit2.target, aabb_near, "nearer AABB beats the farther zone");
        assert_eq!(hit2.zone, None, "an AABB hit reports no zone tag");
    }

    /// AC: the zero-HP corpse skip holds for the facility too — a zero-HP zone
    /// entity on the ray is not hit.
    #[test]
    fn zero_hp_zone_entity_is_skipped() {
        let mut reg = EntityRegistry::new();
        let store = store_with("mob", swinging_limb_model());
        let id = spawn_zone_entity(&mut reg, "mob", Vec3::ZERO);
        let mut health = reg.get_component::<HealthComponent>(id).unwrap().clone();
        health.current = 0.0;
        reg.set_component(id, health).unwrap();

        let hit = nearest_entity_hit(
            &reg,
            &store,
            1.0,
            Vec3::new(5.0, 0.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            100.0,
        );
        assert!(hit.is_none(), "a zero-HP zone entity (corpse) is skipped");
    }

    /// AC: an entity whose model has NO zone tags falls back to AABB behavior
    /// byte-identically — the facility routes it through the authored hitbox and
    /// never the capsule path. A model with no derived bound (no tags) plus a
    /// hitbox is hit exactly like today.
    #[test]
    fn no_zone_model_falls_back_to_authored_aabb() {
        let mut reg = EntityRegistry::new();
        // A model installed WITHOUT zone tags → no derived bound → AABB path.
        let skeleton = Skeleton {
            joints: vec![joint(None, RestLocal::default())],
        };
        let no_zone = ModelHitZones {
            skeleton: Arc::new(skeleton),
            clips: Arc::new(vec![]),
            joint_zones: vec![None],
            derived_bound: None,
        };
        let store = store_with("plain", no_zone);

        // Entity carries BOTH the no-zone mesh and an authored hitbox.
        let id = reg.spawn(Transform {
            position: Vec3::new(0.0, 0.0, -4.0),
            ..Transform::default()
        });
        reg.set_component(
            id,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: Some(Hitbox {
                    half_extents: Vec3::splat(0.5),
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                zone_multipliers: std::collections::HashMap::new(),
            },
        )
        .unwrap();
        reg.set_component(id, MeshComponent::stateless("plain".into()))
            .unwrap();

        let hit = nearest_entity_hit(
            &reg,
            &store,
            0.0,
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
            10.0,
        )
        .expect("the authored AABB is hit on the ray");
        assert_eq!(hit.target, id);
        assert_eq!(hit.zone, None, "AABB fallback carries no zone tag");
        assert!(approx(hit.toi, 3.5), "near face at z = -3.5");
    }

    // --- Relocated AABB slab unit tests (moved from the weapon module) -------

    #[test]
    fn ray_aabb_slab_hits_box_dead_ahead() {
        let (toi, normal) = ray_aabb_slab(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(-0.5, -0.5, -3.5),
            Vec3::new(0.5, 0.5, -2.5),
            10.0,
        )
        .expect("ray should hit the box");
        assert!(approx(toi, 2.5), "entry toi is the near face distance");
        assert!((normal - Vec3::new(0.0, 0.0, 1.0)).length() < FACILITY_EPS);
    }

    #[test]
    fn ray_aabb_slab_misses_off_axis_box() {
        assert!(
            ray_aabb_slab(
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::new(2.5, -0.5, -3.5),
                Vec3::new(3.5, 0.5, -2.5),
                10.0,
            )
            .is_none()
        );
    }

    #[test]
    fn ray_aabb_slab_rejects_box_behind_origin() {
        assert!(
            ray_aabb_slab(
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::new(-0.5, -0.5, 2.5),
                Vec3::new(0.5, 0.5, 3.5),
                10.0,
            )
            .is_none()
        );
    }

    #[test]
    fn ray_aabb_slab_rejects_box_beyond_range() {
        let min = Vec3::new(-0.5, -0.5, -8.5);
        let max = Vec3::new(0.5, 0.5, -7.5);
        assert!(ray_aabb_slab(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), min, max, 5.0).is_none());
        assert!(ray_aabb_slab(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), min, max, 10.0).is_some());
    }

    #[test]
    fn ray_aabb_slab_face_normal_tracks_struck_side() {
        let (toi, normal) = ray_aabb_slab(
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(2.5, -0.5, -0.5),
            Vec3::new(3.5, 0.5, 0.5),
            10.0,
        )
        .expect("ray should hit the box");
        assert!(approx(toi, 2.5));
        assert!((normal - Vec3::new(-1.0, 0.0, 0.0)).length() < FACILITY_EPS);
    }

    // --- unknown_zone_multiplier_tags: the level-load cross-check ------------

    #[test]
    fn unknown_zone_tags_reports_declared_tags_absent_from_model() {
        let zones = vec![zone("head", None), None, zone("torso", None)];
        let unknown = unknown_zone_multiplier_tags(["head", "leg", "torso", "tail"], &zones);
        assert_eq!(
            unknown,
            vec!["leg".to_string(), "tail".to_string()],
            "only tags absent from the model are returned, sorted"
        );
    }

    #[test]
    fn unknown_zone_tags_empty_when_all_declared_tags_exist() {
        let zones = vec![zone("head", None), zone("torso", None)];
        let unknown = unknown_zone_multiplier_tags(["head", "torso"], &zones);
        assert!(unknown.is_empty());
    }

    #[test]
    fn unknown_zone_tags_all_unknown_for_model_without_zones() {
        // An AABB-only model (or a failed load) carries no zones: every declared
        // tag is unknown.
        let unknown = unknown_zone_multiplier_tags(["head", "leg"], &[]);
        assert_eq!(unknown, vec!["head".to_string(), "leg".to_string()]);
    }
}
