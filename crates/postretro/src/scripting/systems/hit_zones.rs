// Game-side skeletal hit-zone store + the standalone entity-raycast facility:
// retains each model TYPE's CPU skeleton, clips, and authored joint-zone table
// plus a derived broad-phase bound seeded from rest pose and swept from clips,
// and resolves the nearest TARGETABLE entity a ray strikes (authored-AABB or
// bone-posed-capsule).
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

use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::components::mesh::{MeshAnimation, MeshComponent};
use postretro_entities::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};
use postretro_model::ModelHandle;
use postretro_model::anim::{
    BlendSource, LocalTrs, Loop, capture_blend, sample_blended_world, sample_clip_looped_world,
};
use postretro_model::gltf_loader::{self, JointZone};
use postretro_model::sample_params::{
    CaptureInstruction, ClipSample, FadeSource, MeshSampleParams, instance_phase,
};
use postretro_model::skeleton::{AnimationClip, Skeleton};
use postretro_render_data::cone_frustum::Aabb;

/// Engine default capsule radius (meters) for a zone-bearing joint whose
/// authored `hitZoneRadius` is absent or invalid at this consumer boundary. The
/// loader preserves only positive finite authored radii; direct `JointZone`
/// construction can still bypass that loader policy.
pub(crate) const DEFAULT_ZONE_RADIUS: f32 = 0.12;

/// One model TYPE's retained CPU hit-zone data, keyed by [`ModelHandle`] in the
/// [`HitZoneStore`].
///
/// The skeleton and clips are `Arc`-wrapped so the per-shot raycast path can
/// clone a cheap handle to re-sample the live pose without ever deep-cloning
/// keyframes. `joint_zones` is parallel to `skeleton.joints` (entry `i` describes
/// joint `i`), exactly as the loader produced it.
///
/// The fields are read by the entity-raycast facility ([`nearest_entity_hit`])
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
    /// Model-local broad-phase AABB seeded from rest pose, then expanded from
    /// every clip's key poses and conservative hierarchy reach.
    /// `None` for an AABB-only model (no zone tags), OR for a zone-bearing model
    /// whose bound came out non-finite (a directly-constructed NaN/Inf pose that
    /// bypassed loader validation) — an untrustworthy broad phase. Either way a
    /// `None` here routes the entity to the authored hitbox in the consumer.
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
/// level-load model sweep and cleared on level change. Consumed by the
/// entity-raycast facility ([`nearest_entity_hit`]), which the weapon hitscan
/// delegates to.
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
    /// entry. The derived bound is computed once, here, from the loaded skeleton
    /// rest pose and clips.
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
        // none here (it falls back to the authored hitbox in the consumer). A
        // non-finite bound also resolves to `None` (see `derive_bound`), degrading
        // that model to the authored AABB.
        let derived_bound = if ModelHitZones::has_zones(&joint_zones) {
            derive_bound(&skeleton, &clips, &joint_zones)
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
    /// never loaded (or its load failed). The entity-raycast facility
    /// [`nearest_entity_hit`] looks the per-instance model up here.
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

/// Capsule radius for a zone-bearing joint. Validate here too because
/// `JointZone` is public and tests/tools can construct one without the loader.
fn zone_radius(zone: &JointZone) -> f32 {
    zone.radius
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(DEFAULT_ZONE_RADIUS)
}

/// Derive a zone-bearing model's model-local broad-phase AABB from rest + clips,
/// or `None` when no trustworthy bound exists.
///
/// Starts with rest-pose zone capsules, then samples every clip at its actual
/// channel key times plus endpoints. A short authored excursion between former
/// uniform sample points is therefore included. The bound then gets ONE
/// conservative hierarchy reach envelope, unioned over all clips: translations
/// and scales are bounded from key values, while rotations can swing descendants
/// between key poses. The union-max envelope dominates not just each clip but
/// every pairwise CROSSFADE BLEND of them (see [`joint_reach_bounds`]). Static
/// zoned models with no clips still get a usable rest-pose bound.
///
/// The authored hitbox is deliberately not consulted. A limb must never silently
/// sit or swing outside the broad phase.
///
/// Returns `None` when a posed capsule center is non-finite (a directly-
/// constructed NaN/Inf rest translation or keyframe value that bypassed loader
/// validation): the precise broad phase is then untrustworthy, so the consumer
/// degrades to the authored AABB rather than a poisoned box that would reject
/// every ray. A finite but empty bound (no joints) collapses to a zero box.
fn derive_bound(
    skeleton: &Skeleton,
    clips: &[AnimationClip],
    joint_zones: &[Option<JointZone>],
) -> Option<Aabb> {
    let mut bound = Aabb::empty();
    // Reused across clips/samples — `sample_clip_looped_world` clears and refills.
    let mut world = Vec::new();

    pose_rest(skeleton, &mut world);
    if !expand_bound_for_zone_capsules(&mut bound, skeleton, &world, joint_zones) {
        return None;
    }

    for clip in clips {
        for t in clip_bound_times(clip) {
            sample_clip_looped_world(clip, skeleton, t, Loop::Clamp, &mut world);
            if !expand_bound_for_zone_capsules(&mut bound, skeleton, &world, joint_zones) {
                return None;
            }
        }
    }

    // One reach envelope over ALL clips (and thus all their blends), applied once
    // as the conservative backstop to the key-time sampling above.
    expand_bound_for_reach(&mut bound, skeleton, clips, joint_zones);

    // An empty skeleton leaves the box inverted; collapse it to a well-formed
    // zero box, matching `Aabb::from_points`' empty contract.
    if bound.min.x > bound.max.x {
        return Some(Aabb::default());
    }
    // Defensive: the capsule guards above already bail on a non-finite pose and
    // the reach envelope is finite by construction — but never hand back a
    // non-finite box that the slab test would silently reject every ray against.
    if !bound.min.is_finite() || !bound.max.is_finite() {
        return None;
    }
    Some(bound)
}

/// Expand `bound` by each tagged joint's posed capsule (its origin plus its first
/// child's origin), inflated by the zone radius. Returns `false` if any posed
/// capsule center is non-finite (NaN/Inf): such a center is NOT folded in (glam's
/// comparison-based min/max would poison the whole box, and a later finite
/// `expand` would silently un-poison it to a wrong, too-small box), and the
/// `false` propagates up so the caller returns `None` (degrade to authored AABB).
fn expand_bound_for_zone_capsules(
    bound: &mut Aabb,
    skeleton: &Skeleton,
    world_joints: &[Mat4],
    joint_zones: &[Option<JointZone>],
) -> bool {
    let mut finite = true;
    for (joint_index, zone) in joint_zones.iter().enumerate() {
        let Some(zone) = zone else {
            continue;
        };
        let Some(joint_world) = world_joints.get(joint_index) else {
            continue;
        };
        let radius = zone_radius(zone);
        finite &= expand_bound_for_finite_sphere(bound, joint_world.w_axis.truncate(), radius);
        if let Some(child_index) = first_child_index(skeleton, joint_index) {
            if let Some(child_world) = world_joints.get(child_index) {
                finite &=
                    expand_bound_for_finite_sphere(bound, child_world.w_axis.truncate(), radius);
            }
        }
    }
    finite
}

/// Expand `bound` by a sphere at `center` of `radius`, but only when `center` is
/// finite. Returns whether the center was finite; a non-finite center is skipped
/// (not folded in) so it cannot poison the box.
fn expand_bound_for_finite_sphere(bound: &mut Aabb, center: Vec3, radius: f32) -> bool {
    if !center.is_finite() {
        return false;
    }
    let radius = Vec3::splat(radius);
    bound.expand(center + radius);
    bound.expand(center - radius);
    true
}

fn clip_bound_times(clip: &AnimationClip) -> Vec<f32> {
    let mut times = Vec::new();
    times.push(0.0);
    if clip.duration.is_finite() && clip.duration > 0.0 {
        times.push(clip.duration);
    }
    for tracks in &clip.joints {
        times.extend(
            tracks
                .translation
                .times()
                .iter()
                .chain(tracks.rotation.times())
                .chain(tracks.scale.times())
                .copied()
                .filter(|t| t.is_finite() && *t >= 0.0),
        );
    }
    times.sort_by(f32::total_cmp);
    times.dedup_by(|a, b| *a == *b);
    times
}

/// Expand `bound` by the conservative hierarchy reach envelope: for each tagged
/// joint (and its capsule's child endpoint), an origin-centered sphere of that
/// joint's worst-case reach plus the zone radius. See [`joint_reach_bounds`] for
/// the envelope's construction and why the sphere is origin-centered.
fn expand_bound_for_reach(
    bound: &mut Aabb,
    skeleton: &Skeleton,
    clips: &[AnimationClip],
    joint_zones: &[Option<JointZone>],
) {
    let reach = joint_reach_bounds(skeleton, clips);
    for (joint_index, zone) in joint_zones.iter().enumerate() {
        let Some(zone) = zone else {
            continue;
        };
        let radius = zone_radius(zone);
        if let Some(reach) = reach.get(joint_index) {
            expand_bound_for_origin_sphere(bound, *reach + radius);
        }
        if let Some(child_index) = first_child_index(skeleton, joint_index) {
            if let Some(reach) = reach.get(child_index) {
                expand_bound_for_origin_sphere(bound, *reach + radius);
            }
        }
    }
}

/// Expand `bound` by an origin-centered sphere of `radius`. Takes only a radius
/// because the reach envelope is a distance from the SKELETON-LOCAL ORIGIN, not a
/// joint-centered box — an origin-centered sphere covers the joint in any
/// direction the (untracked) rotations could swing it. See [`joint_reach_bounds`].
fn expand_bound_for_origin_sphere(bound: &mut Aabb, radius: f32) {
    let extent = Vec3::splat(radius.max(0.0));
    bound.expand(extent);
    bound.expand(-extent);
}

/// Per-joint worst-case reach: the farthest a joint's posed origin can sit from
/// the SKELETON-LOCAL ORIGIN, via triangle-inequality accumulation of ancestor
/// offsets and scale, UNIONED over all clips.
///
/// For each joint, `t_max` is the longest local translation it takes across its
/// rest pose and every clip's keyframes; `s_max` is its largest absolute scale
/// component likewise. A single parent-before-child pass accumulates
/// `scale_chain[i] = scale_chain[parent] * s_max[i]` and
/// `reach[i] = reach[parent] + scale_chain[parent] * t_max[i]`. Rotations do not
/// enter: an operator-norm-1 rotation cannot lengthen `|t|`, and the reach feeds
/// an origin-centered sphere (see [`expand_bound_for_origin_sphere`]) that covers
/// any orientation — so ignoring rotation is conservative, not lossy.
///
/// Unioning `t_max`/`s_max` over ALL clips is what makes the envelope cover
/// CROSSFADE BLENDS, not just each clip alone. A blend factor `lerp(a, b, w)`
/// never exceeds `max(a, b) ≤` the per-joint union max, yet a blend's composed
/// scale chain (concave in `w`, with an interior maximum) CAN exceed either
/// clip's own chain — so a per-clip envelope would under-cover the worst blend
/// pose. The union-max envelope dominates every clip and every pairwise blend.
fn joint_reach_bounds(skeleton: &Skeleton, clips: &[AnimationClip]) -> Vec<f32> {
    let mut reach = Vec::with_capacity(skeleton.joints.len());
    let mut scale_chain = Vec::with_capacity(skeleton.joints.len());

    for (i, joint) in skeleton.joints.iter().enumerate() {
        let local_offset = max_translation_len(joint.rest_local.translation, clips, i);
        let local_scale = max_scale_component(joint.rest_local.scale, clips, i);

        let (joint_reach, joint_scale) = match joint.parent {
            Some(parent) => {
                let parent_reach = reach.get(parent).copied().unwrap_or(0.0);
                let parent_scale = scale_chain.get(parent).copied().unwrap_or(1.0);
                (
                    parent_reach + parent_scale * local_offset,
                    parent_scale * local_scale,
                )
            }
            None => (local_offset, local_scale),
        };
        reach.push(joint_reach);
        scale_chain.push(joint_scale);
    }

    reach
}

/// The longest local translation joint `joint_index` takes: its rest translation
/// or any finite translation keyframe value across EVERY clip. Non-finite values
/// are ignored (`finite_vec3`); a joint with no data contributes 0.
fn max_translation_len(rest: Vec3, clips: &[AnimationClip], joint_index: usize) -> f32 {
    let mut max_len = finite_vec3(rest).map_or(0.0, Vec3::length);
    for clip in clips {
        let Some(tracks) = clip.joints.get(joint_index) else {
            continue;
        };
        for value in tracks
            .translation
            .values()
            .iter()
            .copied()
            .filter_map(finite_vec3)
        {
            max_len = max_len.max(value.length());
        }
    }
    max_len
}

/// The largest absolute scale component joint `joint_index` takes: its rest scale
/// or any finite scale keyframe value across EVERY clip. Non-finite values are
/// ignored; a joint with no data contributes the neutral scale 1.
fn max_scale_component(rest: Vec3, clips: &[AnimationClip], joint_index: usize) -> f32 {
    let mut max_scale = finite_vec3(rest).map_or(1.0, max_abs_component);
    for clip in clips {
        let Some(tracks) = clip.joints.get(joint_index) else {
            continue;
        };
        for value in tracks
            .scale
            .values()
            .iter()
            .copied()
            .filter_map(finite_vec3)
        {
            max_scale = max_scale.max(max_abs_component(value));
        }
    }
    max_scale
}

fn finite_vec3(value: Vec3) -> Option<Vec3> {
    value.is_finite().then_some(value)
}

fn max_abs_component(value: Vec3) -> f32 {
    value.x.abs().max(value.y.abs()).max(value.z.abs())
}

// ---------------------------------------------------------------------------
// Entity-raycast facility
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
///   joint. A no-tag model keeps AABB behavior. When the precise pose is
///   UNAVAILABLE (a chained smooth-interrupt snapshot needing renderer-only
///   data), a zone-bearing entity degrades to its authored AABB so it stays
///   hittable; an available-pose capsule miss stays a miss (no AABB fallback).
///
/// Zero-HP entities (pending-despawn this tick) are skipped so a corpse cannot
/// absorb a shot for one frame. `anim_time` is the game-layer animation clock;
/// `store` is the per-model hit-zone data ([`HitZoneStore`]). The result's `zone`
/// carries the struck bone's tag for a capsule hit (the zone-multiplier damage
/// routing site reads it).
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
        // joints; otherwise fall back to the authored AABB. A no-tag model (or a
        // model whose derived bound came out non-finite) has no derived bound and
        // falls through to AABB.
        let zoned = zone_bearing_entry(registry, store, id);

        let hit = match zoned {
            Some(zoned) => {
                // The entity's animation, if any, drives the posed skeleton; a
                // mesh with no animation block (stateless prop) poses to the
                // model's first clip at the clock. The animation is cloned LAZILY
                // — only AFTER the broad phase survives — so a broad-phase reject
                // never deep-clones the state map. The closure keeps the registry
                // borrow live so the reject path allocates nothing.
                match nearest_zone_hit(
                    zoned.zones,
                    transform,
                    zoned.origin_offset,
                    || {
                        registry
                            .get_component::<MeshComponent>(id)
                            .ok()
                            .and_then(|m| m.animation.clone())
                    },
                    anim_time,
                    id,
                    origin,
                    direction,
                    range,
                ) {
                    // Pose available: the capsule result is AUTHORITATIVE. A
                    // `Resolved(None)` is a genuine miss (broad-phase reject or no
                    // capsule on the ray) — a zone-bearing posed model uses its
                    // capsules, never its coarse AABB — so do NOT fall back.
                    ZoneResolve::Resolved(hit) => hit,
                    // Pose UNAVAILABLE (a chained smooth interrupt whose snapshot
                    // capture needs renderer-only stored data): degrade to the
                    // authored AABB so a drawn enemy stays hittable at coarse
                    // precision. No hitbox → not targetable this query.
                    ZoneResolve::Unavailable => hitbox.as_ref().and_then(|hitbox| {
                        aabb_hit(origin, direction, transform, hitbox, range, id)
                    }),
                }
            }
            // No zone-bearing model: the authored AABB is both broad and narrow
            // phase. Health without a hitbox is not targetable.
            None => hitbox
                .as_ref()
                .and_then(|hitbox| aabb_hit(origin, direction, transform, hitbox, range, id)),
        };

        if let Some(hit) = hit {
            if nearest.as_ref().is_none_or(|n| hit.toi < n.toi) {
                nearest = Some(hit);
            }
        }
    }

    nearest
}

struct ZoneBearingEntry<'a> {
    zones: &'a ModelHitZones,
    origin_offset: Vec3,
}

/// The model hit-zone entry for an entity IF its mesh model is zone-bearing (has
/// a derived bound, i.e. ≥1 tagged joint). `None` when the entity has no mesh,
/// the model is unloaded, or the model carries no zone tags — in which case the
/// caller falls back to the authored AABB (today's behavior, byte-identical).
fn zone_bearing_entry<'a>(
    registry: &EntityRegistry,
    store: &'a HitZoneStore,
    id: EntityId,
) -> Option<ZoneBearingEntry<'a>> {
    let mesh = registry.get_component::<MeshComponent>(id).ok()?;
    let entry = store.get(&ModelHandle::from(mesh.model.clone()))?;
    // Only a zone-bearing model (derived bound present) takes the capsule path.
    entry.derived_bound.as_ref().map(|_| ZoneBearingEntry {
        zones: entry,
        origin_offset: mesh.origin_offset,
    })
}

/// Ray-test an entity's authored AABB hitbox, returning the entry hit or `None`
/// on a miss. The coarse path for a non-zone entity, and the degrade target for a
/// zone-bearing entity whose precise capsule pose is unavailable this query.
fn aabb_hit(
    origin: Vec3,
    direction: Vec3,
    transform: &Transform,
    hitbox: &Hitbox,
    range: f32,
    id: EntityId,
) -> Option<EntityRayHit> {
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

/// The outcome of a zone-bearing entity's capsule query, distinguishing "pose
/// UNAVAILABLE" from "pose available, ray missed". The caller degrades an
/// unavailable pose to the authored AABB (so a drawn enemy stays hittable), but
/// treats an available-pose result as authoritative — a zone-bearing posed model
/// uses its capsules, never its coarse AABB, so an available-pose miss stands.
enum ZoneResolve {
    /// The precise capsule pose could not be produced game-side (a chained smooth
    /// interrupt whose snapshot capture needs renderer-only stored data).
    Unavailable,
    /// The pose was available and tested: `Some` is the nearest capsule hit,
    /// `None` a genuine miss (broad-phase reject, or no capsule on the ray).
    Resolved(Option<EntityRayHit>),
}

/// Ray-test one zone-bearing entity: broad phase against the model's derived
/// bound (transformed to a world-axis-aligned enclosure), then per tagged joint
/// a posed-capsule narrow test. Returns [`ZoneResolve`]: `Resolved` (with the
/// nearest hit or a genuine miss) when the pose is available, or `Unavailable`
/// when the precise pose could not be produced game-side (the caller then
/// degrades to the authored AABB).
#[allow(clippy::too_many_arguments)] // a flat parameter list keeps the facility weapon/camera-free.
fn nearest_zone_hit(
    zones: &ModelHitZones,
    transform: &Transform,
    origin_offset: Vec3,
    resolve_animation: impl FnOnce() -> Option<MeshAnimation>,
    anim_time: f64,
    id: EntityId,
    origin: Vec3,
    direction: Vec3,
    range: f32,
) -> ZoneResolve {
    // Model→world by POSITION + the same MeshComponent origin offset that render
    // uses, plus YAW only (no pitch/roll/scale). This is the game-tick placement,
    // deliberately NOT the renderer's interpolated transform.
    let model_to_world = position_yaw_matrix(transform, origin_offset);

    // Broad phase: the derived bound is model-local; transform it to a tight
    // world-axis-aligned enclosure and ray-test that AABB. A reject here means
    // no capsule can be hit (the bound encloses every posed capsule by
    // construction) — a genuine miss, NOT a pose failure, so it does not degrade
    // to the AABB (which lies inside this bound). Rejecting before resolving
    // (cloning) the animation means a rejected entity pays no deep clone.
    let Some(bound) = zones.derived_bound.as_ref() else {
        return ZoneResolve::Resolved(None);
    };
    let bound = bound.transformed(&model_to_world);
    if ray_aabb_slab(origin, direction, bound.min, bound.max, range).is_none() {
        return ZoneResolve::Resolved(None);
    }

    // Broad phase survived: NOW resolve (clone) the entity's animation. A mesh
    // with no animation block (stateless prop) poses to the model's first clip.
    let animation = resolve_animation();

    // Narrow phase: pose the skeleton at the entity's current animation time AND
    // the SAME per-instance phase the renderer draws with (seed = the raw
    // `EntityId`), then test one capsule per tagged joint. A `None` pose is
    // UNAVAILABLE (a chained snapshot fade needing renderer-only data) — signal it
    // so the caller degrades to the authored AABB rather than reporting a miss.
    let Some(world_joints) = pose_world_joints(zones, animation.as_ref(), anim_time, id.to_raw())
    else {
        return ZoneResolve::Unavailable;
    };

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
        let radius = zone_radius(zone);

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

    ZoneResolve::Resolved(nearest)
}

/// The model-local posed-origin of `joint_index`'s FIRST CHILD (lowest joint
/// index whose parent is `joint_index`), or `None` for a leaf — which the caller
/// renders as a zero-length sphere at the joint origin.
fn first_child_origin(
    zones: &ModelHitZones,
    world_joints: &[Mat4],
    joint_index: usize,
) -> Option<Vec3> {
    first_child_index(&zones.skeleton, joint_index)
        .and_then(|child_index| world_joints.get(child_index))
        .map(|child| child.w_axis.truncate())
}

fn first_child_index(skeleton: &Skeleton, joint_index: usize) -> Option<usize> {
    skeleton
        .joints
        .iter()
        .enumerate()
        .find(|(_, joint)| joint.parent == Some(joint_index))
        .map(|(child_index, _)| child_index)
}

/// Compose the entity's model→world matrix from POSITION + YAW only. Pitch, roll,
/// and scale are deliberately dropped: hit zones use the game-tick placement, not
/// the renderer's interpolated full transform. Yaw is extracted from the stored
/// quaternion as rotation about world +Y.
fn position_yaw_matrix(transform: &Transform, origin_offset: Vec3) -> Mat4 {
    let yaw = yaw_of(transform.rotation);
    Mat4::from_rotation_translation(
        Quat::from_rotation_y(yaw),
        transform.position + origin_offset,
    )
}

/// The yaw angle (rotation about world +Y) of a quaternion. Projects the
/// quaternion's forward direction onto the XZ plane and takes its heading, so
/// any pitch/roll baked into the quaternion is discarded.
fn yaw_of(rotation: Quat) -> f32 {
    let forward = rotation * Vec3::NEG_Z;
    forward.x.atan2(-forward.z)
}

/// Pose the model's skeleton at `anim_time` into per-joint MODEL-space world
/// matrices (pre-inverse-bind), or `None` when the precise pose is UNAVAILABLE.
///
/// Fail-available contract: `None` means the game side cannot reconstruct the
/// exact pose the renderer draws — specifically a chained smooth-interrupt
/// snapshot fade whose capture references renderer-only stored data (mirrors the
/// same case on [`pose_from_params`]). The caller degrades to the authored AABB
/// rather than posing a wrong fallback-clip capsule; it never means "the ray
/// missed". A model with no clips, or an unresolved state, still poses (rest or
/// first clip) and returns `Some`.
///
/// When the entity carries an animation block, its current state resolves —
/// through the SAME render-free [`mesh_anim::animate_entity`] the renderer's
/// collector uses — to a [`MeshSampleParams`] (primary clip leg + optional
/// crossfade), which [`pose_from_params`] then samples (single or blended). An
/// unresolved state, or a stateless / no-animation mesh, poses the model's first
/// clip looped at the clock; a model with no clips poses each joint to rest.
///
/// Phase de-sync is fed in at the SAME per-instance phase the renderer applies,
/// so a capsule tracks the drawn pose rather than lagging a whole clip behind it.
/// The phase is [`instance_phase`] of the entity's `EntityId` seed against the
/// CURRENT state's clip duration (the stateless/default path uses the first
/// clip's duration) — the exact seed + duration the renderer's collector uses, so
/// the values match. `instance_phase`/`state_time` apply it ONLY to looping legs;
/// one-shot states ignore it, matching the renderer.
fn pose_world_joints(
    zones: &ModelHitZones,
    animation: Option<&MeshAnimation>,
    anim_time: f64,
    seed: u32,
) -> Option<Vec<Mat4>> {
    let mut out = Vec::new();
    let skeleton = zones.skeleton.as_ref();
    let clips = zones.clips.as_ref();

    // Resolve the entity's current animation to render-free sample params via the
    // shared resolver, feeding the SAME per-instance phase the renderer draws
    // with so capsules track the drawn pose. `animate_entity` applies the phase
    // only to looping legs; it returns `None` for an unresolved current state, so
    // we fall through to the default pose.
    if let Some(anim) = animation {
        let phase = current_state_phase(anim, clips, seed);
        if let Some(result) = super::mesh_anim::animate_entity(anim, anim_time, phase) {
            let mut snapshot = Vec::new();
            if pose_from_params(
                skeleton,
                clips,
                &result.sample,
                result.capture.as_ref(),
                &mut snapshot,
                &mut out,
            ) {
                return Some(out);
            }
            return None;
        }
    }

    // Default pose: the model's first clip, looped at the clock plus the SAME
    // per-instance phase the renderer's stateless path applies (first clip's
    // duration); or rest if the model carries no clips at all.
    match clips.first() {
        Some(clip) => {
            let phase = instance_phase(seed, clip.duration);
            sample_clip_looped_world(
                clip,
                skeleton,
                anim_time as f32 + phase,
                Loop::Wrap,
                &mut out,
            )
        }
        None => pose_rest(skeleton, &mut out),
    }
    Some(out)
}

/// The per-instance phase offset for an entity's CURRENT animation state, derived
/// from that state's clip duration — the game-side mirror of the renderer
/// collector's `current_state_phase`. The seed is the raw `EntityId`; the
/// duration comes from the resolved current state's `clip_index` into the store's
/// `clips`, so the phase matches the renderer's value exactly. A state with no
/// resolved clip (or an out-of-range index) yields phase 0, matching the
/// renderer's `unwrap_or(0.0)`. `instance_phase` then zeroes a zero-length clip.
fn current_state_phase(anim: &MeshAnimation, clips: &[AnimationClip], seed: u32) -> f32 {
    let duration = anim
        .states
        .get(&anim.current_state)
        .and_then(|s| s.clip_index)
        .and_then(|i| clips.get(i))
        .map(|clip| clip.duration)
        .unwrap_or(0.0);
    instance_phase(seed, duration)
}

/// Pose the model's skeleton at `anim_time` per a resolved [`MeshSampleParams`]:
/// sample the primary clip alone, or blend the primary against the active fade's
/// FROM-leg. A [`FadeSource::Snapshot`] from-leg must have a matching exact
/// capture instruction; otherwise the pose is unavailable game-side and the
/// caller skips capsule hits for this query.
fn pose_from_params(
    skeleton: &Skeleton,
    clips: &[AnimationClip],
    params: &MeshSampleParams,
    capture: Option<&CaptureInstruction>,
    snapshot: &mut Vec<LocalTrs>,
    out: &mut Vec<Mat4>,
) -> bool {
    let primary_src = clip_blend_source(clips, &params.primary);
    let Some(primary) = primary_src else {
        pose_rest(skeleton, out);
        return true;
    };

    match params.fade {
        Some(fade) => {
            match fade.from {
                FadeSource::Clip(leg) => match clip_blend_source(clips, &leg) {
                    Some(from) => sample_blended_world(&from, &primary, fade.weight, skeleton, out),
                    // Missing from-clip: sample the primary alone.
                    None => sample_clip_looped_world(
                        clip_of(clips, params.primary.clip_index).unwrap(),
                        skeleton,
                        params.primary.time,
                        params.primary.loop_policy,
                        out,
                    ),
                },
                FadeSource::Snapshot { tag, .. } => {
                    let Some(capture) = capture.filter(|capture| capture.tag == tag) else {
                        return false;
                    };
                    if !capture_snapshot_pose(skeleton, clips, capture, snapshot) {
                        return false;
                    }
                    sample_blended_world(
                        &BlendSource::Snapshot(snapshot.as_slice()),
                        &primary,
                        fade.weight,
                        skeleton,
                        out,
                    );
                }
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
    true
}

fn capture_snapshot_pose(
    skeleton: &Skeleton,
    clips: &[AnimationClip],
    capture: &CaptureInstruction,
    out: &mut Vec<LocalTrs>,
) -> bool {
    let Some(outgoing) = exact_capture_source(clips, &capture.outgoing) else {
        return false;
    };
    let Some(incoming) = clip_blend_source(clips, &capture.incoming) else {
        return false;
    };
    capture_blend(&outgoing, &incoming, capture.weight, skeleton, out);
    true
}

fn exact_capture_source<'a>(
    clips: &'a [AnimationClip],
    source: &FadeSource,
) -> Option<BlendSource<'a>> {
    match *source {
        FadeSource::Clip(leg) => clip_blend_source(clips, &leg),
        // A chained smooth interrupt needs the previous snapshot store entry.
        // Hit zones cannot read the renderer-owned store, so do not pose a
        // fallback clip capsule that the renderer is not drawing.
        FadeSource::Snapshot { .. } => None,
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

    use postretro_model::skeleton::{Interp, Joint, JointTracks, RestLocal, Track};

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
            translation: Track::new(
                vec![0.0, 1.0],
                vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)],
                Interp::Linear,
            )
            .expect("valid swept-limb translation track"),
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

        let bound = derive_bound(&skeleton, std::slice::from_ref(&clip), &joint_zones)
            .expect("finite bound");

        // The child reaches x=10 at the clip end; the bound must include x=10 plus
        // its capsule radius — proving the SWEPT extreme (not just t=0) is captured
        // and that the per-joint radius inflated it.
        assert!(
            bound.max.x >= 10.0 + child_radius - 1.0e-4,
            "bound max.x {} must enclose swept limb tip (10) + radius ({child_radius})",
            bound.max.x
        );
        // The tagged child passes through x=0 at t=0, so its own radius must be
        // represented on the near side too.
        assert!(
            bound.min.x <= 0.0 - child_radius + 1.0e-4,
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

    /// Regression: a short authored keyframe excursion between the former fixed
    /// uniform samples must still be inside the derived broad-phase bound.
    #[test]
    fn derived_bound_includes_short_keyframe_excursion() {
        let skeleton = Skeleton {
            joints: vec![joint(None, RestLocal::default())],
        };
        // The peak lives near the start of a long clip, then returns to origin
        // before the old 1/7 uniform sample. Key-time expansion must catch it.
        let tracks = JointTracks {
            translation: Track::new(
                vec![0.0, 0.01, 0.02, 1.0],
                vec![Vec3::ZERO, Vec3::new(0.0, 8.0, 0.0), Vec3::ZERO, Vec3::ZERO],
                Interp::Linear,
            )
            .expect("valid short-excursion translation track"),
            ..Default::default()
        };
        let clip = AnimationClip {
            name: "snap".into(),
            duration: 1.0,
            joints: vec![tracks],
        };
        let joint_zones = vec![zone("head", Some(0.1))];

        let bound = derive_bound(&skeleton, std::slice::from_ref(&clip), &joint_zones)
            .expect("finite bound");

        assert!(
            bound.max.y >= 8.0 + 0.1 - 1.0e-4,
            "short keyed peak y=8 plus radius must be enclosed; max.y {} too small",
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

        let bound = derive_bound(&skeleton, std::slice::from_ref(&clip), &joint_zones)
            .expect("finite bound");

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

    /// Regression: `JointZone` is public, so non-loader construction can carry
    /// an invalid radius. Runtime consumers fall back to the engine default.
    #[test]
    fn invalid_public_radius_inflates_by_engine_default() {
        let skeleton = Skeleton {
            joints: vec![joint(None, RestLocal::default())],
        };
        let clip = AnimationClip {
            name: "rest".into(),
            duration: 0.0,
            joints: vec![JointTracks::default()],
        };
        let joint_zones = vec![zone("torso", Some(f32::NAN))];

        let bound = derive_bound(&skeleton, std::slice::from_ref(&clip), &joint_zones)
            .expect("finite bound");

        assert!(
            (bound.max - Vec3::splat(DEFAULT_ZONE_RADIUS)).length() < 1.0e-4,
            "invalid radius should use default max, got {:?}",
            bound.max
        );
        assert!(
            (bound.min + Vec3::splat(DEFAULT_ZONE_RADIUS)).length() < 1.0e-4,
            "invalid radius should use default min, got {:?}",
            bound.min
        );
    }

    /// Regression: broad phase must inflate both endpoints of a parent-tagged
    /// capsule by the parent zone radius. The child endpoint is part of that
    /// same narrow-phase capsule even when the child has no zone tag.
    #[test]
    fn parent_zone_radius_inflates_child_endpoint() {
        let child_rest = Vec3::new(10.0, 0.0, 0.0);
        let parent_radius = 2.0;
        let skeleton = Skeleton {
            joints: vec![
                joint(None, RestLocal::default()),
                joint(
                    Some(0),
                    RestLocal {
                        translation: child_rest,
                        ..RestLocal::default()
                    },
                ),
            ],
        };
        let joint_zones = vec![zone("torso", Some(parent_radius)), None];

        let bound = derive_bound(&skeleton, &[], &joint_zones).expect("finite bound");

        assert!(
            bound.max.x >= child_rest.x + parent_radius - 1.0e-4,
            "child endpoint must inherit parent capsule radius; max.x={}",
            bound.max.x
        );
    }

    /// A zone-bearing skeleton with no animation clips must still derive a bound
    /// over its rest pose. Otherwise the broad phase collapses to the origin and
    /// rejects static zoned targets before narrow phase can pose rest joints.
    #[test]
    fn derived_bound_without_clips_uses_rest_pose() {
        let rest_pos = Vec3::new(4.0, 0.0, 0.0);
        let skeleton = Skeleton {
            joints: vec![
                joint(None, RestLocal::default()),
                joint(
                    Some(0),
                    RestLocal {
                        translation: rest_pos,
                        ..RestLocal::default()
                    },
                ),
            ],
        };
        let radius = 0.25;
        let joint_zones = vec![None, zone("hand", Some(radius))];

        let bound = derive_bound(&skeleton, &[], &joint_zones).expect("finite bound");

        assert!(
            bound.max.x >= rest_pos.x + radius - 1.0e-4,
            "rest joint at x={} plus radius {} must be inside bound; max.x={}",
            rest_pos.x,
            radius,
            bound.max.x
        );
        assert!(
            bound.min.x <= rest_pos.x - radius + 1.0e-4,
            "rest joint at x={} minus radius {} must be inside bound; min.x={}",
            rest_pos.x,
            radius,
            bound.min.x
        );
    }

    /// Regression: the derived bound must enclose the worst-case CROSSFADE BLEND
    /// of two clips, not just each clip's own key poses. A composed scale chain is
    /// concave in the blend weight, so a mid-blend pose can reach FARTHER than
    /// either clip alone — the reach envelope is unioned over all clips so it
    /// dominates every pairwise blend.
    ///
    /// Chain 0->1->2 with joint 2 tagged at rest translation (L,0,0). Clip A
    /// scales joint 1 by 3; clip B scales joint 0 by 3 (anti-correlated). Each
    /// clip alone composes joint 2 to x=3L, but the w=0.5 blend composes to x=4L
    /// (scales lerp to 2 and 2, 2*2*L) — outside the per-clip key poses. The
    /// union-max envelope (here ~9L) must enclose that worst blend.
    #[test]
    fn derived_bound_encloses_worst_anticorrelated_blend_reach() {
        const L: f32 = 2.0;
        let skeleton = Skeleton {
            joints: vec![
                joint(None, RestLocal::default()),
                joint(Some(0), RestLocal::default()),
                joint(
                    Some(1),
                    RestLocal {
                        translation: Vec3::new(L, 0.0, 0.0),
                        ..RestLocal::default()
                    },
                ),
            ],
        };

        fn scale_track(s: f32) -> JointTracks {
            JointTracks {
                scale: Track::new(
                    vec![0.0, 1.0],
                    vec![Vec3::splat(s), Vec3::splat(s)],
                    Interp::Linear,
                )
                .expect("valid scale track"),
                ..Default::default()
            }
        }

        // Clip A scales joint 1 by 3; clip B scales joint 0 by 3 (anti-correlated).
        let clip_a = AnimationClip {
            name: "a".into(),
            duration: 1.0,
            joints: vec![
                JointTracks::default(),
                scale_track(3.0),
                JointTracks::default(),
            ],
        };
        let clip_b = AnimationClip {
            name: "b".into(),
            duration: 1.0,
            joints: vec![
                scale_track(3.0),
                JointTracks::default(),
                JointTracks::default(),
            ],
        };
        let joint_zones = vec![None, None, zone("tip", Some(0.1))];

        let bound = derive_bound(&skeleton, &[clip_a, clip_b], &joint_zones).expect("finite bound");

        // Pin the INVARIANT (bound encloses the worst blend pose x=4L), not the
        // exact envelope value — union-max is conservatively larger (~9L).
        assert!(
            bound.max.x >= 4.0 * L - 1.0e-4,
            "bound.max.x {} must enclose the worst-case blend reach 4L={}",
            bound.max.x,
            4.0 * L,
        );
    }

    /// Regression: a non-finite posed capsule center (a directly-constructed NaN
    /// rest translation that bypassed loader validation) must NOT yield a poisoned
    /// NaN bound that rejects every ray. `derive_bound` returns `None` so the model
    /// degrades to the authored AABB and stays hittable.
    #[test]
    fn derived_bound_none_when_pose_non_finite() {
        let skeleton = Skeleton {
            joints: vec![joint(
                None,
                RestLocal {
                    translation: Vec3::new(f32::NAN, 0.0, 0.0),
                    ..RestLocal::default()
                },
            )],
        };
        let joint_zones = vec![zone("core", Some(0.2))];

        assert!(
            derive_bound(&skeleton, &[], &joint_zones).is_none(),
            "a non-finite pose must degrade (None), not return a poisoned NaN bound"
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

    // --- Entity-raycast facility --------------------------------------------

    use postretro_entities::components::health::{HealthComponent, Hitbox};
    use postretro_entities::components::mesh::MeshComponent;
    use postretro_entities::registry::{EntityRegistry, Transform};

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
            translation: Track::new(
                vec![0.0, 2.0],
                vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)],
                Interp::Linear,
            )
            .expect("valid swinging-limb translation track"),
            ..Default::default()
        };
        let clip = AnimationClip {
            name: "swing".into(),
            duration: 2.0,
            joints: vec![JointTracks::default(), child_tracks],
        };
        // Root untagged; child tagged leaf with an explicit radius.
        let joint_zones = vec![None, zone("hand", Some(0.3))];
        let derived_bound = derive_bound(&skeleton, std::slice::from_ref(&clip), &joint_zones);
        ModelHitZones {
            skeleton: Arc::new(skeleton),
            clips: Arc::new(vec![clip]),
            joint_zones,
            derived_bound,
        }
    }

    fn const_x_clip(name: &str, x: f32) -> AnimationClip {
        AnimationClip {
            name: name.to_string(),
            duration: 1.0,
            joints: vec![JointTracks {
                translation: Track::new(
                    vec![0.0, 1.0],
                    vec![Vec3::new(x, 0.0, 0.0), Vec3::new(x, 0.0, 0.0)],
                    Interp::Linear,
                )
                .expect("valid const translation track"),
                ..Default::default()
            }],
        }
    }

    fn smooth_interrupt_model() -> ModelHitZones {
        let skeleton = Skeleton {
            joints: vec![joint(None, RestLocal::default())],
        };
        let clips = vec![
            const_x_clip("idle", 0.0),
            const_x_clip("walk", 10.0),
            const_x_clip("run", 100.0),
        ];
        let joint_zones = vec![zone("core", Some(0.25))];
        let derived_bound = derive_bound(&skeleton, &clips, &joint_zones);
        ModelHitZones {
            skeleton: Arc::new(skeleton),
            clips: Arc::new(clips),
            joint_zones,
            derived_bound,
        }
    }

    /// A static zone-bearing model: no animation clips, one tagged leaf joint in
    /// rest pose away from the origin.
    fn static_rest_zone_model() -> ModelHitZones {
        let skeleton = Skeleton {
            joints: vec![
                joint(None, RestLocal::default()),
                joint(
                    Some(0),
                    RestLocal {
                        translation: Vec3::new(4.0, 0.0, 0.0),
                        ..RestLocal::default()
                    },
                ),
            ],
        };
        let joint_zones = vec![None, zone("hand", Some(0.25))];
        let derived_bound = derive_bound(&skeleton, &[], &joint_zones);

        ModelHitZones {
            skeleton: Arc::new(skeleton),
            clips: Arc::new(vec![]),
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

    /// Regression: zoned skeletons with no clips are still hittable at their
    /// rest-pose joint positions. The ray is far from the origin, so a zero
    /// broad-phase box would reject before `pose_world_joints` falls back to rest.
    #[test]
    fn no_clip_zone_entity_hits_rest_pose_away_from_origin() {
        let mut reg = EntityRegistry::new();
        let store = store_with("static", static_rest_zone_model());
        let id = spawn_zone_entity(&mut reg, "static", Vec3::ZERO);

        let hit = nearest_entity_hit(
            &reg,
            &store,
            0.0,
            Vec3::new(4.0, 0.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            100.0,
        )
        .expect("rest-pose zone away from origin should pass broad phase and hit");

        assert_eq!(hit.target, id);
        assert_eq!(hit.zone.as_deref(), Some("hand"));
    }

    /// An ANIMATED zone entity poses through the shared `animate_entity`
    /// resolver: its current state's clip-local time (`anim_time - entered_at`)
    /// drives the limb. Entered at t=3, sampled at t=4 → clip-local 1.0 → child at
    /// x=5; a ray through x=5 hits while a ray through x=0 (the rest) misses.
    /// Exercises the `pose_from_params` single-clip path (no fade).
    #[test]
    fn animated_entity_poses_via_state_clip_local_time() {
        use postretro_entities::components::mesh::{
            AnimationState, InterruptPolicy, MeshAnimation,
        };

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
                origin_offset: Vec3::ZERO,
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

    /// Regression: smooth-interrupt snapshot fades must pose capsules from the
    /// captured in-flight blend the renderer draws, not from the fallback clip.
    #[test]
    fn smooth_snapshot_fade_hits_captured_pose_not_fallback_clip() {
        use postretro_entities::components::mesh::{
            AnimationState, FadeSourceKind, InterruptPolicy, InterruptedOutgoing, MeshAnimation,
        };

        fn state(clip: &str, crossfade_ms: f32, clip_index: usize) -> AnimationState {
            AnimationState {
                clip: clip.into(),
                looping: true,
                crossfade_ms,
                interrupt: InterruptPolicy::Smooth,
                clip_index: Some(clip_index),
            }
        }

        let mut reg = EntityRegistry::new();
        let store = store_with("smooth", smooth_interrupt_model());

        let mut states = HashMap::new();
        states.insert("A".to_string(), state("idle", 0.0, 0));
        states.insert("B".to_string(), state("walk", 200.0, 1));
        states.insert("C".to_string(), state("run", 100.0, 2));
        let mut anim = MeshAnimation::new(states, "A".into());

        // A->B was halfway through when C smoothly interrupted at t=1.1.
        // Renderer captures S = blend(A@1.1, B@1.1, 0.5) = x=5, then C fades
        // from S. The snapshot fallback clip is B at x=10.
        let t2 = 1.1_f64;
        anim.current_state = "C".into();
        anim.previous_state = Some("B".into());
        anim.previous_entered_at = Some(1.0);
        anim.entered_at = Some(t2);
        anim.fade_source = FadeSourceKind::Snapshot;
        anim.interrupted_outgoing = Some(InterruptedOutgoing::Clip {
            state: "A".into(),
            entered_at: 0.0,
        });

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
                model: "smooth".into(),
                animation: Some(anim),
                origin_offset: Vec3::ZERO,
            },
        )
        .unwrap();

        let dir = Vec3::new(0.0, 0.0, -1.0);
        let captured = nearest_entity_hit(&reg, &store, t2, Vec3::new(5.0, 0.0, 10.0), dir, 100.0)
            .expect("snapshot-captured pose at x=5 should be hittable");
        assert_eq!(captured.target, id);
        assert_eq!(captured.zone.as_deref(), Some("core"));

        let fallback = nearest_entity_hit(&reg, &store, t2, Vec3::new(10.0, 0.0, 10.0), dir, 100.0);
        assert!(
            fallback.is_none(),
            "fallback clip pose at x=10 must not be used for smooth snapshot hits"
        );
    }

    /// Regression: a CHAINED smooth interrupt (the interrupted fade's OUTGOING leg
    /// is itself a prior snapshot) cannot be reconstructed game-side — the capture
    /// references renderer-only stored data — so the precise capsule pose is
    /// UNAVAILABLE. A zone-bearing entity that has an authored hitbox must then
    /// degrade to that AABB and stay hittable, not become fully unhittable for the
    /// crossfade window.
    #[test]
    fn chained_snapshot_interrupt_degrades_to_authored_aabb() {
        use postretro_entities::components::mesh::{
            AnimationState, FadeSourceKind, InterruptPolicy, InterruptedOutgoing, MeshAnimation,
        };

        fn state(clip: &str, crossfade_ms: f32, clip_index: usize) -> AnimationState {
            AnimationState {
                clip: clip.into(),
                looping: true,
                crossfade_ms,
                interrupt: InterruptPolicy::Smooth,
                clip_index: Some(clip_index),
            }
        }

        let mut reg = EntityRegistry::new();
        let store = store_with("smooth", smooth_interrupt_model());

        let mut states = HashMap::new();
        states.insert("A".to_string(), state("idle", 0.0, 0));
        states.insert("B".to_string(), state("walk", 200.0, 1));
        states.insert("C".to_string(), state("run", 100.0, 2));
        let mut anim = MeshAnimation::new(states, "A".into());

        // C smoothly interrupted an in-flight A->B fade at t2=1.1; the interrupted
        // fade's OUTGOING leg was ITSELF a prior snapshot, so the capture's
        // `outgoing` is a `FadeSource::Snapshot` the game side cannot resolve.
        let t2 = 1.1_f64;
        anim.current_state = "C".into();
        anim.previous_state = Some("B".into());
        anim.previous_entered_at = Some(1.0);
        anim.entered_at = Some(t2);
        anim.fade_source = FadeSourceKind::Snapshot;
        anim.interrupted_outgoing = Some(InterruptedOutgoing::Snapshot {
            tag: 1.0_f64.to_bits(),
        });

        // Entity at the origin with an authored hitbox on the ray. Absent the
        // degrade it takes the capsule path and, with the pose unavailable, would
        // return `None` (unhittable) — the game-feel regression this fix closes.
        let id = reg.spawn(Transform::default());
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
        reg.set_component(
            id,
            MeshComponent {
                model: "smooth".into(),
                animation: Some(anim),
                origin_offset: Vec3::ZERO,
            },
        )
        .unwrap();

        let hit = nearest_entity_hit(
            &reg,
            &store,
            t2,
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            100.0,
        )
        .expect("chained snapshot interrupt degrades to the authored AABB and stays hittable");
        assert_eq!(hit.target, id);
        assert_eq!(hit.zone, None, "the AABB degrade carries no zone tag");
    }

    /// AC (pose-available authority): when the capsule pose is AVAILABLE and the
    /// ray misses every capsule, the result is a genuine miss — a zone-bearing
    /// posed model uses its capsules, never its coarse authored AABB. Even with a
    /// hitbox the ray WOULD strike, an available-pose miss must NOT fall back to
    /// the AABB (unlike the pose-unavailable degrade).
    #[test]
    fn posed_zone_miss_does_not_fall_back_to_authored_aabb() {
        let mut reg = EntityRegistry::new();
        let store = store_with("mob", swinging_limb_model());

        // A zone entity WITH a large authored hitbox the ray would strike if it
        // were ever consulted on the available-pose path.
        let id = reg.spawn(Transform::default());
        reg.set_component(
            id,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: Some(Hitbox {
                    half_extents: Vec3::splat(4.0),
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                zone_multipliers: std::collections::HashMap::new(),
            },
        )
        .unwrap();
        reg.set_component(id, MeshComponent::stateless("mob".into()))
            .unwrap();

        // Posed at t=1 the only capsule (child sphere) sits at (5,0,0), r=0.3. A -Z
        // ray at (2.5, 3.0) lies inside the broad bound AND inside the 4m hitbox,
        // but nowhere near the capsule → the pose is available, so the miss stands.
        let hit = nearest_entity_hit(
            &reg,
            &store,
            1.0,
            Vec3::new(2.5, 3.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            100.0,
        );
        assert!(
            hit.is_none(),
            "an available-pose capsule miss must NOT fall back to the authored AABB"
        );
    }

    /// AC (zones never desync from visuals): a LOOPING entity with a NON-ZERO
    /// per-instance phase poses its capsules at the PHASED clip-local time the
    /// renderer draws — NOT the un-phased (zero-phase) time. The facility folds in
    /// the SAME `instance_phase(seed, clip_duration)` the renderer's collector
    /// uses, so a ray through the phased limb position hits while a ray through the
    /// un-phased rest/zero-phase position misses.
    ///
    /// Setup: a looping state on the swing clip entered at t=0, sampled at
    /// anim_time=0 → elapsed 0, so `state_time == phase` and the child sits at
    /// `x = 5 * phase` (the clip ramps x from 0 at t=0 to 10 at t=2). Seed 0
    /// hashes to phase 0, so a single dummy spawn advances the real entity to a
    /// seed with a meaningful non-zero phase.
    #[test]
    fn looping_entity_poses_capsules_at_phased_time() {
        use postretro_entities::components::mesh::{
            AnimationState, InterruptPolicy, MeshAnimation,
        };

        let mut reg = EntityRegistry::new();
        let store = store_with("mob", swinging_limb_model());

        // Burn entity id 0 (phase 0) so the real entity gets a seed whose
        // `instance_phase` is non-zero — the whole point of the test.
        let _dummy = reg.spawn(Transform::default());

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
        anim.entered_at = Some(0.0); // entered at the clock origin → elapsed == 0

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
                origin_offset: Vec3::ZERO,
            },
        )
        .unwrap();

        // The SAME phase the renderer computes: seed = the raw EntityId, duration
        // = the current state's clip (the swing clip, duration 2.0). The child's
        // posed x at anim_time 0 is `5 * phase` (x = 10 * (state_time / 2)).
        let phase = instance_phase(id.to_raw(), 2.0);
        assert!(
            phase > 0.1,
            "the chosen seed must produce a meaningful non-zero phase, got {phase}"
        );
        let phased_x = 5.0 * phase;

        let dir = Vec3::new(0.0, 0.0, -1.0);

        // A ray through the PHASED limb position hits (the capsule tracks the
        // drawn pose, not the un-phased pose).
        let hit = nearest_entity_hit(
            &reg,
            &store,
            0.0,
            Vec3::new(phased_x, 0.0, 10.0),
            dir,
            100.0,
        )
        .expect("a ray through the PHASED limb position hits");
        assert_eq!(hit.target, id);
        assert_eq!(hit.zone.as_deref(), Some("hand"));
        // The reported impact sits at the phased x (within the sphere radius).
        assert!(
            approx(hit.point.x, phased_x) || (hit.point.x - phased_x).abs() < 0.3,
            "impact x {} tracks the phased limb x {phased_x}",
            hit.point.x
        );

        // A ray through the UN-PHASED (zero-phase) rest position x=0 MISSES: with
        // phase folded in, the limb is at `phased_x`, far (≥ ~0.5) from x=0, so a
        // facility that ignored phase (posing at clip-local 0) would wrongly hit
        // here. The miss proves the phase is actually applied.
        let unphased = nearest_entity_hit(&reg, &store, 0.0, Vec3::new(0.0, 0.0, 10.0), dir, 100.0);
        assert!(
            unphased.is_none(),
            "a ray through the UN-PHASED rest position (x=0) must miss once phase \
             moves the limb to x={phased_x}"
        );
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

    #[test]
    fn zone_hit_uses_mesh_origin_offset_for_model_world_transform() {
        let mut reg = EntityRegistry::new();
        let store = store_with("mob", swinging_limb_model());
        let id = reg.spawn(Transform {
            position: Vec3::new(0.0, 0.8, 0.0),
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
        reg.set_component(
            id,
            MeshComponent {
                model: "mob".into(),
                animation: None,
                origin_offset: Vec3::new(0.0, -0.8, 0.0),
            },
        )
        .unwrap();

        let hit = nearest_entity_hit(
            &reg,
            &store,
            1.0,
            Vec3::new(5.0, 0.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            100.0,
        )
        .expect("offset mesh hit zones should follow the rendered model origin");
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
