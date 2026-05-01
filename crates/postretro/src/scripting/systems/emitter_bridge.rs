// Emitter bridge: walks `BillboardEmitterComponent` entities each tick, spawns
// particle entities (Transform + ParticleState + SpriteVisual) into the registry.
// See: context/lib/scripting.md

use std::collections::HashMap;

use glam::Vec3;

use crate::fx::smoke::MAX_SPRITES;
use crate::scripting::components::billboard_emitter::{BillboardEmitterComponent, SpinAnimation};
use crate::scripting::components::particle::ParticleState;
use crate::scripting::components::sprite_visual::SpriteVisual;
use crate::scripting::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};

use super::eval_curve;

/// Per-emitter transient state held bridge-side. Insert on first-seen; remove
/// when the emitter is despawned or loses its `BillboardEmitterComponent`.
/// Authoritative component fields stay on the component itself — the bridge
/// only owns simulation-derived state that the script surface should not see.
#[derive(Debug, Clone, PartialEq)]
struct EmitterBridgeState {
    /// Fractional emission accumulator. Advances by `delta * rate` each tick;
    /// decremented by `1.0` per spawned particle. Reset to `0.0` when `rate`
    /// drops to zero so a re-activation does not produce an immediate extra
    /// particle from prior fractional state.
    accumulator: f32,
    /// Elapsed time within the active `SpinAnimation` tween, in seconds. Reset
    /// to `0.0` when a new tween starts or the tween completes.
    spin_elapsed: f32,
    /// Per-emitter LCG state. Seeded once on first-seen from the emitter's
    /// world-space position so two emitters at distinct positions produce
    /// independent spawn streams; advanced via the Park-Miller-style mix in
    /// [`EmitterBridgeState::next_u32`]. Held here (not as a global) so a
    /// shared static does not leak deterministic seeds across emitters.
    rand_state: u32,
    /// Frame-tick counter; XORed into the seed on first-seen so the same
    /// emitter position across reload runs does not always reproduce the same
    /// stream. Bumped each frame the entry is observed.
    step: u32,
    /// Last frame on which a per-cap warning was emitted, in seconds since
    /// level load. Used to throttle `log::warn!` to at most once per second
    /// per emitter (spec §"Budget enforcement").
    last_warn_time: f32,
    /// Last spin animation observed on the component. Used to detect external
    /// mutations (e.g. a `setSpinRate` reaction installing a new tween or
    /// clearing one mid-flight) so [`spin_elapsed`] can be reset on transition.
    /// Without this, a mid-tween cancellation would leak elapsed time into the
    /// next tween.
    last_spin_animation: Option<SpinAnimation>,
}

impl EmitterBridgeState {
    fn new(seed: u32) -> Self {
        Self {
            accumulator: 0.0,
            spin_elapsed: 0.0,
            rand_state: seed,
            step: 0,
            last_warn_time: f32::NEG_INFINITY,
            last_spin_animation: None,
        }
    }

    /// LCG advance — Park-Miller / Numerical Recipes constants. Lifted from
    /// the retired `fx::smoke` ring-buffer emitter but per-emitter (not
    /// global) so two emitters never share a stream.
    fn next_u32(&mut self) -> u32 {
        self.rand_state = self
            .rand_state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.rand_state
    }

    /// Pseudo-random `f32` in `[0.0, 1.0)`. Mirrors the smoke helper.
    fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1 << 24) as f32
    }
}

/// Emitter bridge owning the per-emitter transient state. Constructed once and
/// run via [`EmitterBridge::update`] each frame after the script `tick`
/// dispatch and before the particle simulation.
pub(crate) struct EmitterBridge {
    states: HashMap<EntityId, EmitterBridgeState>,
}

impl EmitterBridge {
    pub(crate) fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    /// Number of live tracked emitter entries. Exposed for tests asserting
    /// state cleanup; not used by production paths yet.
    #[allow(dead_code)]
    pub(crate) fn tracked_count(&self) -> usize {
        self.states.len()
    }

    /// Walk every entity with a `BillboardEmitterComponent`, fire bursts +
    /// rate-based emission + spin animation, and spawn particle entities.
    ///
    /// `current_time` is seconds since level load (matches the engine frame
    /// clock). Used for rate-limited cap warnings.
    pub(crate) fn update(&mut self, registry: &mut EntityRegistry, delta: f32, current_time: f32) {
        if delta <= 0.0 {
            // Defensive: a zero / negative delta should not advance accumulators
            // or animations. Still purge stale entries below.
            self.purge_stale(registry);
            return;
        }

        // --- Pass 1: snapshot per-emitter component + position + live count.
        // Walking the registry while emitting `set_component` / `spawn` calls
        // would alias the borrow. Snapshot here, then mutate in pass 2.
        struct Plan {
            id: EntityId,
            component: BillboardEmitterComponent,
            origin: Vec3,
            live_count: usize,
        }

        // Tally per-emitter live particle counts in one pass.
        let mut live_counts: HashMap<EntityId, usize> = HashMap::new();
        for (_pid, value) in registry.iter_with_kind(ComponentKind::ParticleState) {
            if let ComponentValue::ParticleState(p) = value
                && let Some(parent) = p.emitter
            {
                *live_counts.entry(parent).or_insert(0) += 1;
            }
        }

        let mut plans: Vec<Plan> = Vec::new();
        let mut seen_ids: Vec<EntityId> = Vec::new();
        for (id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter) {
            if let ComponentValue::BillboardEmitter(component) = value {
                seen_ids.push(id);
                let origin = registry
                    .get_component::<Transform>(id)
                    .map(|t| t.position)
                    .unwrap_or(Vec3::ZERO);
                plans.push(Plan {
                    id,
                    component: component.clone(),
                    origin,
                    live_count: live_counts.get(&id).copied().unwrap_or(0),
                });
            }
        }

        // --- Pass 2: per-emitter logic. Mutate state, write component back,
        // spawn particles. Each iteration is a single emitter; borrows do not
        // overlap with the `iter_with_kind` borrow above (it dropped at end of
        // pass 1).
        for plan in plans {
            let Plan {
                id,
                mut component,
                origin,
                mut live_count,
            } = plan;

            // First-seen: seed the LCG from emitter position. XOR with
            // entity raw id so two emitters at the same authored origin do
            // not collide.
            let state = self.states.entry(id).or_insert_with(|| {
                let seed = origin.x.to_bits()
                    ^ origin.y.to_bits()
                    ^ origin.z.to_bits()
                    ^ id.to_raw()
                    ^ 0xDEAD_BEEF;
                EmitterBridgeState::new(seed)
            });
            // XOR step counter into the rand state so a re-tick from the same
            // origin does not reproduce a previous frame's spawn stream.
            state.step = state.step.wrapping_add(1);
            state.rand_state ^= state.step;

            let mut component_changed = false;

            // --- 1. Burst handling. Bursts fire even when rate == 0; clamp
            // to per-emitter headroom.
            if let Some(requested) = component.burst {
                let headroom = MAX_SPRITES.saturating_sub(live_count);
                let to_spawn = (requested as usize).min(headroom);
                let dropped = (requested as usize) - to_spawn;
                if dropped > 0 {
                    rate_limited_warn(
                        state,
                        current_time,
                        format_args!(
                            "[EmitterBridge] emitter {id} burst dropped {dropped} particles (cap {MAX_SPRITES}, live {live_count})"
                        ),
                    );
                }
                for _ in 0..to_spawn {
                    spawn_one(registry, &component, origin, id, state);
                    live_count += 1;
                }
                component.burst = None;
                component_changed = true;
            }

            // --- 2. Rate-based emission.
            if component.rate > 0.0 {
                state.accumulator += delta * component.rate;
                while state.accumulator >= 1.0 {
                    if live_count >= MAX_SPRITES {
                        rate_limited_warn(
                            state,
                            current_time,
                            format_args!(
                                "[EmitterBridge] emitter {id} at cap {MAX_SPRITES}, dropping rate-based spawn"
                            ),
                        );
                        // Preserve fractional progress so resumption is smooth
                        // — don't carry the integer portion that would grant a
                        // free spawn the moment a slot frees up. Drops the
                        // backlog without erasing in-flight sub-tick progress.
                        state.accumulator = state.accumulator.fract();
                        break;
                    }
                    spawn_one(registry, &component, origin, id, state);
                    live_count += 1;
                    state.accumulator -= 1.0;
                }
            } else {
                // rate == 0: clear pending fractional accumulation so a future
                // re-activation does not surface an immediate spurious spawn.
                state.accumulator = 0.0;
            }

            // --- 3. Spin animation tween. Runs after emission per spec
            // ("After emission: if spin_animation.is_some() …"). When the
            // component's `spin_animation` differs from what the bridge saw
            // last frame (external mutation via `setSpinRate`), reset
            // `spin_elapsed` so a new tween starts at t = 0 and a cleared
            // tween does not leak elapsed time into a later install.
            if state.last_spin_animation != component.spin_animation {
                state.spin_elapsed = 0.0;
                state.last_spin_animation = component.spin_animation.clone();
            }
            if let Some(anim) = component.spin_animation.clone() {
                state.spin_elapsed += delta;
                let duration = anim.duration.max(f32::EPSILON);
                let t = (state.spin_elapsed / duration).clamp(0.0, 1.0);
                let new_rate = eval_curve(&anim.rate_curve, t);
                if (component.spin_rate - new_rate).abs() > f32::EPSILON {
                    component.spin_rate = new_rate;
                    component_changed = true;
                }
                if t >= 1.0 {
                    // Tween complete: settle to last keyframe and clear.
                    if let Some(&final_rate) = anim.rate_curve.last() {
                        component.spin_rate = final_rate;
                    }
                    component.spin_animation = None;
                    state.spin_elapsed = 0.0;
                    state.last_spin_animation = None;
                    component_changed = true;
                }
            }

            if component_changed {
                // Stale-id error path is unreachable here — we just iterated
                // the live entity. Document with `expect` per project policy.
                registry
                    .set_component(id, component)
                    .expect("emitter id is live: just iterated from registry");
            }
        }

        // --- Pass 3: cleanup. Drop tracking entries for emitters that no
        // longer exist or have lost their component since last frame.
        let alive: std::collections::HashSet<EntityId> = seen_ids.into_iter().collect();
        self.states.retain(|id, _| alive.contains(id));
    }

    /// Standalone version of the cleanup pass used on no-op early returns.
    fn purge_stale(&mut self, registry: &mut EntityRegistry) {
        self.states.retain(|id, _| {
            registry
                .get_component::<BillboardEmitterComponent>(*id)
                .is_ok()
        });
    }
}

impl Default for EmitterBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawn one particle entity attached to `parent`. Direction is randomized
/// inside the emitter's `spread` cone around `velocity`; magnitude is
/// preserved.
fn spawn_one(
    registry: &mut EntityRegistry,
    component: &BillboardEmitterComponent,
    origin: Vec3,
    parent: EntityId,
    state: &mut EmitterBridgeState,
) {
    let velocity = Vec3::from(component.velocity);
    let speed = velocity.length();
    let dir = if speed > 0.0 {
        let base = velocity / speed;
        sample_cone_direction(base, component.spread, state)
    } else {
        // No bias direction: sample a uniform direction on the unit sphere.
        // `spread` is meaningless without a bias, so we treat the whole sphere
        // as the cone (equivalent to spread = π).
        sample_cone_direction(Vec3::Y, std::f32::consts::PI, state)
    };

    let Some(particle_id) = registry.try_spawn(
        Transform {
            position: origin,
            ..Transform::default()
        },
        &[],
    ) else {
        // Registry exhausted; the bridge cannot spawn more particles. The
        // light bridge uses the same log; mirror that wording.
        log::warn!(
            "[EmitterBridge] entity registry exhausted; dropping particle for emitter {parent}"
        );
        return;
    };

    let particle = ParticleState {
        velocity: (dir * speed).into(),
        age: 0.0,
        lifetime: component.lifetime,
        buoyancy: component.buoyancy,
        drag: component.drag,
        size_curve: component.size_over_lifetime.clone(),
        opacity_curve: component.opacity_over_lifetime.clone(),
        emitter: Some(parent),
    };
    let visual = SpriteVisual {
        sprite: component.sprite.clone(),
        // Sim runs the same frame; it overwrites size/opacity from curves at
        // t = 0 immediately. Spawn-frame zeros are only visible for one tick.
        size: 0.0,
        opacity: 0.0,
        rotation: 0.0,
        tint: component.color,
    };
    // `set_component` on a freshly-allocated ID can only fail on a stale ID.
    let _ = registry.set_component(particle_id, particle);
    let _ = registry.set_component(particle_id, visual);
}

/// Sample a uniformly-distributed unit vector inside the cone of half-angle
/// `spread` around `axis`.
///
/// **Math:** for a cone of half-angle `α`, the solid-angle-uniform CDF on the
/// elevation `θ ∈ [0, α]` is `F(θ) = (1 − cos θ) / (1 − cos α)`. Inverting at
/// `u ∈ [0, 1)`:
///
/// ```text
/// cos θ = 1 − u · (1 − cos α)
/// θ     = acos(1 − u · (1 − cos α))
/// ```
///
/// Azimuth `φ` is uniform on `[0, 2π)`. The local-frame direction
/// `(sin θ cos φ, sin θ sin φ, cos θ)` rotates onto `axis` via an arbitrary
/// orthonormal basis (`axis`, `tangent`, `bitangent`). When `spread == 0` the
/// formula collapses to `axis` exactly.
fn sample_cone_direction(axis: Vec3, spread: f32, state: &mut EmitterBridgeState) -> Vec3 {
    let axis = axis.normalize_or_zero();
    if axis == Vec3::ZERO {
        return Vec3::Y;
    }
    let spread = spread.max(0.0);
    if spread <= f32::EPSILON {
        return axis;
    }

    let u = state.next_f32();
    let phi = state.next_f32() * std::f32::consts::TAU;
    // `1 - u · (1 - cos α)` — see doc comment.
    let cos_theta = 1.0 - u * (1.0 - spread.cos());
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();

    // Build an orthonormal frame around `axis`. Pick a helper not parallel to
    // `axis`; a small ε guard keeps us off the degenerate case.
    let helper = if axis.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let tangent = axis.cross(helper).normalize_or_zero();
    let bitangent = axis.cross(tangent);

    let local =
        tangent * (sin_theta * phi.cos()) + bitangent * (sin_theta * phi.sin()) + axis * cos_theta;
    // `local` is a sum of three unit-scaled orthogonal components; should be
    // unit-length to within float epsilon. Normalize defensively.
    local.normalize_or_zero()
}

/// Throttle `log::warn!` to at most once per second per emitter. Updates
/// `state.last_warn_time` when it fires.
fn rate_limited_warn(
    state: &mut EmitterBridgeState,
    current_time: f32,
    args: std::fmt::Arguments<'_>,
) {
    if current_time - state.last_warn_time >= 1.0 {
        log::warn!("{args}");
        state.last_warn_time = current_time;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::components::billboard_emitter::SpinAnimation;

    fn base_component(rate: f32) -> BillboardEmitterComponent {
        BillboardEmitterComponent {
            rate,
            burst: None,
            spread: 0.0,
            lifetime: 5.0,
            velocity: [0.0, 1.0, 0.0],
            buoyancy: 0.0,
            drag: 0.0,
            size_over_lifetime: vec![1.0],
            opacity_over_lifetime: vec![1.0],
            color: [1.0, 1.0, 1.0],
            sprite: "smoke".into(),
            spin_rate: 0.0,
            spin_animation: None,
        }
    }

    fn count_particles_for(registry: &EntityRegistry, parent: EntityId) -> usize {
        registry
            .iter_with_kind(ComponentKind::ParticleState)
            .filter(|(_, v)| match v {
                ComponentValue::ParticleState(p) => p.emitter == Some(parent),
                _ => false,
            })
            .count()
    }

    fn spawn_emitter(
        registry: &mut EntityRegistry,
        component: BillboardEmitterComponent,
    ) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry.set_component(id, component).unwrap();
        id
    }

    #[test]
    fn rate_ten_spawns_about_ten_particles_per_second() {
        let mut registry = EntityRegistry::new();
        let id = spawn_emitter(&mut registry, base_component(10.0));
        let mut bridge = EmitterBridge::new();

        // Tick 2 seconds in 60Hz steps. Expect ~20 particles total.
        let dt = 1.0 / 60.0;
        let steps = (2.0 / dt) as i32;
        for i in 0..steps {
            bridge.update(&mut registry, dt, i as f32 * dt);
        }
        let total = count_particles_for(&registry, id);
        assert!(
            (18..=22).contains(&total),
            "expected ~20 particles for rate=10 over 2s, got {total}"
        );
    }

    #[test]
    fn burst_some_twenty_spawns_twenty_and_resets_burst() {
        let mut registry = EntityRegistry::new();
        let mut comp = base_component(0.0);
        comp.burst = Some(20);
        let id = spawn_emitter(&mut registry, comp);
        let mut bridge = EmitterBridge::new();

        bridge.update(&mut registry, 1.0 / 60.0, 0.0);
        let total = count_particles_for(&registry, id);
        assert_eq!(total, 20);
        let after = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap();
        assert_eq!(after.burst, None);
    }

    #[test]
    fn rate_zero_no_burst_spawns_zero_particles() {
        let mut registry = EntityRegistry::new();
        let id = spawn_emitter(&mut registry, base_component(0.0));
        let mut bridge = EmitterBridge::new();

        for i in 0..120 {
            bridge.update(&mut registry, 1.0 / 60.0, i as f32 / 60.0);
        }
        assert_eq!(count_particles_for(&registry, id), 0);
    }

    #[test]
    fn accumulator_cleared_when_rate_drops_to_zero() {
        let mut registry = EntityRegistry::new();
        let id = spawn_emitter(&mut registry, base_component(10.0));
        let mut bridge = EmitterBridge::new();

        // First tick at rate=10, dt=0.07 → accumulator advances to 0.7,
        // sub-1.0 so no spawn. Confirm via internal state.
        bridge.update(&mut registry, 0.07, 0.0);
        assert!(bridge.states[&id].accumulator > 0.5);
        assert_eq!(count_particles_for(&registry, id), 0);

        // Drop rate to 0 — accumulator must clear.
        let mut comp = base_component(0.0);
        registry.set_component(id, comp.clone()).unwrap();
        bridge.update(&mut registry, 0.01, 0.07);
        assert_eq!(bridge.states[&id].accumulator, 0.0);

        // Restore rate. With dt=0.05 and rate=10, accumulator advances to 0.5
        // — must NOT carry over the previous 0.7 to spawn an immediate particle.
        comp.rate = 10.0;
        registry.set_component(id, comp).unwrap();
        bridge.update(&mut registry, 0.05, 0.08);
        assert_eq!(
            count_particles_for(&registry, id),
            0,
            "no carry-over particle should spawn"
        );
    }

    #[test]
    fn over_cap_burst_clamps_to_headroom_and_resets_burst() {
        let mut registry = EntityRegistry::new();
        let id = spawn_emitter(&mut registry, base_component(0.0));
        let mut bridge = EmitterBridge::new();

        // Pre-fill: spawn (MAX_SPRITES - 5) particles attached to `id` by
        // hand (bypass the bridge for setup).
        for _ in 0..(MAX_SPRITES - 5) {
            let pid = registry.spawn(Transform::default());
            registry
                .set_component(
                    pid,
                    ParticleState {
                        velocity: [0.0; 3],
                        age: 0.0,
                        lifetime: 100.0,
                        buoyancy: 0.0,
                        drag: 0.0,
                        size_curve: vec![1.0],
                        opacity_curve: vec![1.0],
                        emitter: Some(id),
                    },
                )
                .unwrap();
        }
        // Fire burst of 20; only 5 should land.
        let mut comp = base_component(0.0);
        comp.burst = Some(20);
        registry.set_component(id, comp).unwrap();
        bridge.update(&mut registry, 1.0 / 60.0, 0.0);

        let total = count_particles_for(&registry, id);
        assert_eq!(total, MAX_SPRITES);
        let after = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap();
        assert_eq!(after.burst, None);
    }

    #[test]
    fn over_cap_rate_drops_further_spawns() {
        let mut registry = EntityRegistry::new();
        let id = spawn_emitter(&mut registry, base_component(0.0));
        let mut bridge = EmitterBridge::new();

        // Pre-fill MAX_SPRITES particles.
        for _ in 0..MAX_SPRITES {
            let pid = registry.spawn(Transform::default());
            registry
                .set_component(
                    pid,
                    ParticleState {
                        velocity: [0.0; 3],
                        age: 0.0,
                        lifetime: 100.0,
                        buoyancy: 0.0,
                        drag: 0.0,
                        size_curve: vec![1.0],
                        opacity_curve: vec![1.0],
                        emitter: Some(id),
                    },
                )
                .unwrap();
        }
        // Fire continuous rate.
        let comp = base_component(60.0);
        registry.set_component(id, comp).unwrap();
        for i in 0..30 {
            bridge.update(&mut registry, 1.0 / 60.0, i as f32 / 60.0);
        }
        assert_eq!(count_particles_for(&registry, id), MAX_SPRITES);
    }

    #[test]
    fn spawn_directions_distribute_within_cone() {
        // 500 spawns with spread = π/4. Mean direction must align with
        // velocity to within a tolerance.
        let mut registry = EntityRegistry::new();
        let mut comp = base_component(0.0);
        comp.spread = std::f32::consts::FRAC_PI_4;
        comp.velocity = [0.0, 1.0, 0.0]; // up
        comp.burst = Some(500);
        let id = spawn_emitter(&mut registry, comp);
        let mut bridge = EmitterBridge::new();
        bridge.update(&mut registry, 1.0 / 60.0, 0.0);

        let mut sum = Vec3::ZERO;
        for (_pid, value) in registry.iter_with_kind(ComponentKind::ParticleState) {
            if let ComponentValue::ParticleState(p) = value
                && p.emitter == Some(id)
            {
                sum += Vec3::from(p.velocity);
            }
        }
        let mean = sum / 500.0;
        let mean_dir = mean.normalize();
        // Ideal mean for a solid-angle-uniform cone of half-angle α is along
        // axis with length (1 + cos α) / 2. We just check direction alignment.
        let dot_with_axis = mean_dir.dot(Vec3::Y);
        assert!(
            dot_with_axis > 0.9,
            "mean direction should hug Y axis; dot={dot_with_axis}"
        );
    }

    #[test]
    fn spawned_particles_carry_emitter_back_reference() {
        let mut registry = EntityRegistry::new();
        let mut comp = base_component(0.0);
        comp.burst = Some(3);
        let id = spawn_emitter(&mut registry, comp);
        let mut bridge = EmitterBridge::new();
        bridge.update(&mut registry, 1.0 / 60.0, 0.0);

        let mut found = 0;
        for (_pid, value) in registry.iter_with_kind(ComponentKind::ParticleState) {
            if let ComponentValue::ParticleState(p) = value {
                assert_eq!(p.emitter, Some(id));
                found += 1;
            }
        }
        assert_eq!(found, 3);
    }

    #[test]
    fn spin_animation_two_second_tween_zero_to_two_pi_reads_pi_at_one_second() {
        let mut registry = EntityRegistry::new();
        let mut comp = base_component(0.0);
        comp.spin_animation = Some(SpinAnimation {
            duration: 2.0,
            rate_curve: vec![0.0, std::f32::consts::TAU],
        });
        let id = spawn_emitter(&mut registry, comp);
        let mut bridge = EmitterBridge::new();

        // Step 1 second of simulated time in fine increments so the
        // accumulator-then-anim phase order matches production.
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        for _ in 0..60 {
            bridge.update(&mut registry, dt, t);
            t += dt;
        }
        let after = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap();
        // At t = 1.0 (mid-tween), linear interp on [0, 2π] gives π.
        let expected = std::f32::consts::PI;
        assert!(
            (after.spin_rate - expected).abs() < 0.05,
            "expected spin_rate ≈ π at t=1.0s; got {}",
            after.spin_rate
        );
        // Tween still active mid-flight.
        assert!(after.spin_animation.is_some());
    }

    #[test]
    fn spin_animation_completes_and_clears_at_end_of_duration() {
        let mut registry = EntityRegistry::new();
        let mut comp = base_component(0.0);
        comp.spin_animation = Some(SpinAnimation {
            duration: 1.0,
            rate_curve: vec![0.0, 1.0, 2.5],
        });
        let id = spawn_emitter(&mut registry, comp);
        let mut bridge = EmitterBridge::new();

        // Step well past the tween duration.
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        for _ in 0..120 {
            bridge.update(&mut registry, dt, t);
            t += dt;
        }
        let after = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap();
        assert!(after.spin_animation.is_none(), "tween should clear");
        assert!(
            (after.spin_rate - 2.5).abs() < 1e-5,
            "spin_rate should settle to last keyframe; got {}",
            after.spin_rate
        );
    }

    #[test]
    fn bridge_state_cleaned_up_when_emitter_despawned() {
        let mut registry = EntityRegistry::new();
        let id = spawn_emitter(&mut registry, base_component(5.0));
        let mut bridge = EmitterBridge::new();

        bridge.update(&mut registry, 0.1, 0.0);
        assert_eq!(bridge.tracked_count(), 1);

        registry.despawn(id).unwrap();
        bridge.update(&mut registry, 0.1, 0.1);
        assert_eq!(
            bridge.tracked_count(),
            0,
            "stale entry must be purged on next update"
        );
    }

    #[test]
    fn bridge_state_cleaned_up_when_component_removed() {
        let mut registry = EntityRegistry::new();
        let id = spawn_emitter(&mut registry, base_component(5.0));
        let mut bridge = EmitterBridge::new();
        bridge.update(&mut registry, 0.1, 0.0);
        assert_eq!(bridge.tracked_count(), 1);

        registry
            .remove_component::<BillboardEmitterComponent>(id)
            .unwrap();
        bridge.update(&mut registry, 0.1, 0.1);
        assert_eq!(bridge.tracked_count(), 0);
    }

    #[test]
    fn no_stale_entries_accumulate_over_many_spawn_despawn_cycles() {
        let mut registry = EntityRegistry::new();
        let mut bridge = EmitterBridge::new();
        for cycle in 0..50 {
            let id = spawn_emitter(&mut registry, base_component(2.0));
            bridge.update(&mut registry, 0.05, cycle as f32);
            registry.despawn(id).unwrap();
            bridge.update(&mut registry, 0.05, cycle as f32 + 0.05);
        }
        assert_eq!(bridge.tracked_count(), 0);
    }

    /// End-to-end test: an FGD `billboard_emitter` map entity flows through
    /// `apply_classname_dispatch` (classname-dispatch path), the bridge ticks
    /// once, and at least one particle lands at the map entity's origin.
    /// Covers the dispatch → spawn → bridge pipeline in one shot. Lives here
    /// (not under `scripting::builtins`) because the `scripting_systems` mount
    /// only exists in the binary crate root, so integration tests that touch
    /// the bridge must live inside the systems tree.
    #[test]
    fn dispatch_then_bridge_tick_produces_particle_at_map_origin() {
        use std::collections::HashMap;

        use crate::scripting::builtins::{
            ClassnameDispatch, MapEntity, apply_classname_dispatch, register_builtins,
        };

        let mut dispatch = ClassnameDispatch::new();
        register_builtins(&mut dispatch);

        let mut kv = HashMap::new();
        // High rate guarantees at least one spawn within a 0.5 s tick window.
        kv.insert("rate".to_string(), "60".to_string());
        let entities = vec![MapEntity {
            classname: "billboard_emitter".to_string(),
            origin: Vec3::new(10.0, 20.0, 30.0),
            angles: Vec3::ZERO,
            key_values: kv,
            tags: vec![],
        }];

        let mut registry = EntityRegistry::new();
        let handled = apply_classname_dispatch(&entities, &dispatch, &mut registry);
        assert!(
            handled.contains("billboard_emitter"),
            "billboard_emitter should dispatch successfully",
        );

        let mut bridge = EmitterBridge::new();
        bridge.update(&mut registry, 0.5, 0.0);

        let mut found_at_origin = false;
        for (id, value) in registry.iter_with_kind(ComponentKind::ParticleState) {
            if !matches!(value, ComponentValue::ParticleState(_)) {
                continue;
            }
            let transform = registry
                .get_component::<Transform>(id)
                .expect("particle should have a Transform");
            if (transform.position - Vec3::new(10.0, 20.0, 30.0)).length() < 1.0e-3 {
                found_at_origin = true;
                break;
            }
        }
        assert!(
            found_at_origin,
            "expected at least one particle spawned at the map entity origin",
        );
    }
}
