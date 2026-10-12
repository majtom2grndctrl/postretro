// Frame-time emitter bridge: reads emitter entities and spawns anonymous pool particles.
// See: context/lib/scripting.md

use std::collections::HashMap;

use glam::Vec3;

use super::particle_render::ParticleRenderCollector;
use postretro_entities::components::billboard_emitter::{BillboardEmitterComponent, SpinAnimation};
use postretro_entities::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};
use postretro_render_cpu::smoke::MAX_SPRITES;
use postretro_sim::particle_pool::{ParticleParameters, ParticlePool};

use crate::sprite_collection::derive_collection_id;
use crate::weapon::spread::sample_cone_direction as sample_shared_cone_direction;

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
    /// A new tween starts at this frame's sample time; a replicated age already
    /// includes this frame's elapsed time.
    skip_next_spin_advance: bool,
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
    /// mutations (e.g. the billboard-emitter `setSpinRate` reaction installing or
    /// clearing a tween mid-flight) so [`spin_elapsed`] can be reset on transition.
    /// Without this, a mid-tween cancellation would leak elapsed time into the
    /// next tween.
    last_spin_animation: Option<SpinAnimation>,
}

impl EmitterBridgeState {
    fn new(seed: u32) -> Self {
        Self {
            accumulator: 0.0,
            spin_elapsed: 0.0,
            skip_next_spin_advance: false,
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

    /// Cache an applied spin tween. A supplied age is sampled without adding
    /// this frame's delta again; following frames advance normally.
    pub(crate) fn seed_replicated_state(
        &mut self,
        registry: &EntityRegistry,
        id: EntityId,
        elapsed: Option<f64>,
    ) -> bool {
        if elapsed
            .is_some_and(|value| !value.is_finite() || value < 0.0 || !(value as f32).is_finite())
        {
            return false;
        }
        let Ok(component) = registry.get_component::<BillboardEmitterComponent>(id) else {
            return false;
        };
        let origin = registry
            .get_component::<Transform>(id)
            .map(|transform| transform.position)
            .unwrap_or(Vec3::ZERO);
        let state = self
            .states
            .entry(id)
            .or_insert_with(|| EmitterBridgeState::new(emitter_seed(id, origin)));
        state.last_spin_animation = component.spin_animation.clone();
        if component.spin_animation.is_none() {
            state.spin_elapsed = 0.0;
            state.skip_next_spin_advance = false;
        } else if let Some(elapsed) = elapsed {
            state.spin_elapsed = elapsed as f32;
            state.skip_next_spin_advance = component.spin_animation.is_some();
        }
        true
    }

    pub(crate) fn clear(&mut self) {
        self.states.clear();
    }

    /// Install reactions run at clock zero, before any running-frame delta.
    /// Cache their tweens without emitting particles or consuming that first delta.
    pub(crate) fn start_installed_animations(&mut self, registry: &EntityRegistry) {
        for (id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter) {
            let ComponentValue::BillboardEmitter(component) = value else {
                continue;
            };
            if component.spin_animation.is_none() {
                continue;
            }
            let origin = registry
                .get_component::<Transform>(id)
                .map(|transform| transform.position)
                .unwrap_or(Vec3::ZERO);
            let mut state = EmitterBridgeState::new(emitter_seed(id, origin));
            state.last_spin_animation = component.spin_animation.clone();
            self.states.insert(id, state);
        }
    }

    /// Number of live tracked emitter entries. Exposed for tests asserting
    /// state cleanup; not used by production paths yet.
    #[allow(dead_code)]
    pub(crate) fn tracked_count(&self) -> usize {
        self.states.len()
    }

    /// Walk every entity with a `BillboardEmitterComponent`, fire bursts +
    /// rate-based emission + spin animation, and spawn anonymous pool particles.
    ///
    /// `delta` advances emission; `spin_delta` is this frame's script-clock
    /// advance. Developer animation freeze holds tweens while particles keep running.
    ///
    /// `current_time` is seconds since level load (matches the engine frame
    /// clock). Used for rate-limited cap warnings.
    ///
    /// Pool survivor counts include every parameter block for this emitter.
    pub(crate) fn update(
        &mut self,
        registry: &mut EntityRegistry,
        delta: f32,
        spin_delta: f32,
        current_time: f32,
        pool: &mut ParticlePool,
        collector: &mut ParticleRenderCollector,
    ) {
        if delta <= 0.0 {
            // Defensive: a zero / negative delta should not advance accumulators
            // or animations. Still purge stale entries below.
            self.purge_stale(registry);
            return;
        }

        pool.set_current_time(f64::from(current_time));

        // --- Pass 1: snapshot emitter state before component tween writes.
        struct Plan {
            id: EntityId,
            component: BillboardEmitterComponent,
            origin: Vec3,
            live_count: usize,
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
                    live_count: pool.live_count(id),
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
            let state = self
                .states
                .entry(id)
                .or_insert_with(|| EmitterBridgeState::new(emitter_seed(id, origin)));
            // XOR step counter into the rand state so a re-tick from the same
            // origin does not reproduce a previous frame's spawn stream.
            state.step = state.step.wrapping_add(1);
            state.rand_state ^= state.step;

            let mut component_changed = false;
            // Resolve once per emitter frame, never per particle. The immutable
            // block is minted lazily by the pool only if a spawn succeeds.
            let collection = collector
                .resolve_collection(&derive_collection_id(
                    &component.sprite,
                    Some(component.lifetime),
                    None,
                    0.0,
                    None,
                    None,
                ))
                .unwrap_or(usize::MAX);
            let parameters = ParticleParameters {
                lifetime: component.lifetime,
                buoyancy: component.buoyancy,
                drag: component.drag,
                size_curve: component.size_over_lifetime.clone(),
                opacity_curve: component.opacity_over_lifetime.clone(),
                collection,
                tint: component.color,
            };

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
                    if spawn_one(pool, &parameters, &component, origin, id, state) {
                        live_count += 1;
                    } else {
                        break;
                    }
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
                    if !spawn_one(pool, &parameters, &component, origin, id, state) {
                        state.accumulator = state.accumulator.fract();
                        break;
                    }
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
            // last frame (external mutation via billboard-emitter `setSpinRate`), reset
            // `spin_elapsed` so a new tween starts at t = 0 and a cleared
            // tween does not leak elapsed time into a later install.
            if state.last_spin_animation != component.spin_animation {
                state.spin_elapsed = 0.0;
                state.skip_next_spin_advance = component.spin_animation.is_some();
                state.last_spin_animation = component.spin_animation.clone();
            }
            if let Some(anim) = component.spin_animation.clone() {
                if !std::mem::take(&mut state.skip_next_spin_advance) {
                    state.spin_elapsed += spin_delta;
                }
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

/// Spawn one anonymous particle for `parent`. Direction is randomized
/// inside the emitter's `spread` cone around `velocity`; magnitude is
/// preserved.
fn spawn_one(
    pool: &mut ParticlePool,
    parameters: &ParticleParameters,
    component: &BillboardEmitterComponent,
    origin: Vec3,
    parent: EntityId,
    state: &mut EmitterBridgeState,
) -> bool {
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

    pool.spawn_emitter(parent, parameters, origin, dir * speed, component.spin_rate)
}

/// Preserve the emitter's no-draw degenerate paths, then delegate its
/// pre-drawn uniforms to the shared solid-angle cone sampler.
fn sample_cone_direction(axis: Vec3, spread: f32, state: &mut EmitterBridgeState) -> Vec3 {
    let normalized_axis = axis.normalize_or_zero();
    if normalized_axis == Vec3::ZERO {
        return Vec3::Y;
    }
    let spread = spread.max(0.0);
    if spread <= f32::EPSILON {
        return normalized_axis;
    }

    sample_shared_cone_direction(axis, spread, state.next_f32(), state.next_f32())
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

fn emitter_seed(id: EntityId, origin: Vec3) -> u32 {
    origin.x.to_bits() ^ origin.y.to_bits() ^ origin.z.to_bits() ^ id.to_raw() ^ 0xDEAD_BEEF
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use postretro_entities::components::billboard_emitter::SpinAnimation;

    struct Harness {
        bridge: EmitterBridge,
        pool: ParticlePool,
        collector: ParticleRenderCollector,
    }
    impl Harness {
        fn new() -> Self {
            let mut collector = ParticleRenderCollector::new();
            collector.register_sprite(&derive_collection_id(
                "smoke",
                Some(5.0),
                None,
                0.0,
                None,
                None,
            ));
            Self {
                bridge: EmitterBridge::new(),
                pool: ParticlePool::new(),
                collector,
            }
        }
        fn update(
            &mut self,
            registry: &mut EntityRegistry,
            delta: f32,
            spin_delta: f32,
            time: f32,
            _legacy_counts: &HashMap<EntityId, usize>,
        ) {
            self.bridge.update(
                registry,
                delta,
                spin_delta,
                time,
                &mut self.pool,
                &mut self.collector,
            );
        }
    }
    impl std::ops::Deref for Harness {
        type Target = EmitterBridge;
        fn deref(&self) -> &EmitterBridge {
            &self.bridge
        }
    }
    impl std::ops::DerefMut for Harness {
        fn deref_mut(&mut self) -> &mut EmitterBridge {
            &mut self.bridge
        }
    }
    fn update_bridge(
        bridge: &mut Harness,
        registry: &mut EntityRegistry,
        delta: f32,
        current_time: f32,
    ) {
        bridge.update(registry, delta, delta, current_time, &HashMap::new());
    }

    fn base_component(rate: f32) -> BillboardEmitterComponent {
        BillboardEmitterComponent {
            rate,
            burst: None,
            spread: 0.0,
            lifetime: 5.0,
            velocity: [0.0, 1.0, 0.0],
            buoyancy: 0.0,
            drag: 0.0,
            size_over_lifetime: [1.0].into(),
            opacity_over_lifetime: [1.0].into(),
            color: [1.0, 1.0, 1.0],
            sprite: "smoke".into(),
            spin_rate: 0.0,
            spin_animation: None,
        }
    }

    fn count_particles_for(pool: &ParticlePool, parent: EntityId) -> usize {
        pool.live_count(parent)
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
        let mut bridge = Harness::new();

        // Tick 2 seconds in 60Hz steps. Expect ~20 particles total.
        let dt = 1.0 / 60.0;
        let steps = (2.0 / dt) as i32;
        for i in 0..steps {
            update_bridge(&mut bridge, &mut registry, dt, i as f32 * dt);
        }
        let total = count_particles_for(&bridge.pool, id);
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
        let mut bridge = Harness::new();

        update_bridge(&mut bridge, &mut registry, 1.0 / 60.0, 0.0);
        let total = count_particles_for(&bridge.pool, id);
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
        let mut bridge = Harness::new();

        for i in 0..120 {
            update_bridge(&mut bridge, &mut registry, 1.0 / 60.0, i as f32 / 60.0);
        }
        assert_eq!(count_particles_for(&bridge.pool, id), 0);
    }

    #[test]
    fn accumulator_cleared_when_rate_drops_to_zero() {
        let mut registry = EntityRegistry::new();
        let id = spawn_emitter(&mut registry, base_component(10.0));
        let mut bridge = Harness::new();

        // First tick at rate=10, dt=0.07 → accumulator advances to 0.7,
        // sub-1.0 so no spawn. Confirm via internal state.
        update_bridge(&mut bridge, &mut registry, 0.07, 0.0);
        assert!(bridge.states[&id].accumulator > 0.5);
        assert_eq!(count_particles_for(&bridge.pool, id), 0);

        // Drop rate to 0 — accumulator must clear.
        let mut comp = base_component(0.0);
        registry.set_component(id, comp.clone()).unwrap();
        update_bridge(&mut bridge, &mut registry, 0.01, 0.07);
        assert_eq!(bridge.states[&id].accumulator, 0.0);

        // Restore rate. With dt=0.05 and rate=10, accumulator advances to 0.5
        // — must NOT carry over the previous 0.7 to spawn an immediate particle.
        comp.rate = 10.0;
        registry.set_component(id, comp).unwrap();
        update_bridge(&mut bridge, &mut registry, 0.05, 0.08);
        assert_eq!(
            count_particles_for(&bridge.pool, id),
            0,
            "no carry-over particle should spawn"
        );
    }

    #[test]
    fn over_cap_burst_clamps_to_headroom_and_resets_burst() {
        let mut registry = EntityRegistry::new();
        let id = spawn_emitter(&mut registry, base_component(0.0));
        let mut bridge = Harness::new();

        // Pre-fill: spawn (MAX_SPRITES - 5) particles attached to `id` by
        // hand (bypass the bridge for setup).
        for _ in 0..(MAX_SPRITES - 5) {
            assert!(bridge.pool.spawn_emitter(
                id,
                &ParticleParameters {
                    lifetime: 100.0,
                    buoyancy: 0.0,
                    drag: 0.0,
                    size_curve: [1.0].into(),
                    opacity_curve: [1.0].into(),
                    collection: 0,
                    tint: [1.0; 3]
                },
                Vec3::ZERO,
                Vec3::ZERO,
                0.0
            ));
        }
        // Fire burst of 20; only 5 should land.
        let mut comp = base_component(0.0);
        comp.burst = Some(20);
        registry.set_component(id, comp).unwrap();
        update_bridge(&mut bridge, &mut registry, 1.0 / 60.0, 0.0);

        let total = count_particles_for(&bridge.pool, id);
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
        let mut bridge = Harness::new();

        // Pre-fill MAX_SPRITES particles.
        for _ in 0..MAX_SPRITES {
            assert!(bridge.pool.spawn_emitter(
                id,
                &ParticleParameters {
                    lifetime: 100.0,
                    buoyancy: 0.0,
                    drag: 0.0,
                    size_curve: [1.0].into(),
                    opacity_curve: [1.0].into(),
                    collection: 0,
                    tint: [1.0; 3]
                },
                Vec3::ZERO,
                Vec3::ZERO,
                0.0
            ));
        }
        // Fire continuous rate.
        let comp = base_component(60.0);
        registry.set_component(id, comp).unwrap();
        for i in 0..30 {
            update_bridge(&mut bridge, &mut registry, 1.0 / 60.0, i as f32 / 60.0);
        }
        assert_eq!(count_particles_for(&bridge.pool, id), MAX_SPRITES);
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
        let mut bridge = Harness::new();
        update_bridge(&mut bridge, &mut registry, 1.0 / 60.0, 0.0);

        let mut sum = Vec3::ZERO;
        for p in bridge.pool.iter().filter(|p| p.emitter == Some(id)) {
            sum += p.velocity;
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
    fn fixed_seed_emitter_cone_sequence_survives_shared_sampler_extraction() {
        let axis = Vec3::new(0.3, 0.8, -0.5).normalize();
        let mut state = EmitterBridgeState::new(0x00C0_FFEE);
        let expected = [
            Vec3::new(
                f32::from_bits(0x3E9A_3EE4),
                f32::from_bits(0x3F63_C9EC),
                f32::from_bits(0xBEAF_80DC),
            ),
            Vec3::new(
                f32::from_bits(0x3EDB_DF68),
                f32::from_bits(0x3F5F_5C4A),
                f32::from_bits(0xBE6E_AA7B),
            ),
            Vec3::new(
                f32::from_bits(0x3E02_053C),
                f32::from_bits(0x3F4C_0FB2),
                f32::from_bits(0xBF17_1FB2),
            ),
            Vec3::new(
                f32::from_bits(0x3E3D_E848),
                f32::from_bits(0x3F54_F4C0),
                f32::from_bits(0xBF05_E8C2),
            ),
        ];

        for expected in expected {
            let actual = sample_cone_direction(axis, 0.45, &mut state);
            assert_eq!(actual.x.to_bits(), expected.x.to_bits());
            assert_eq!(actual.y.to_bits(), expected.y.to_bits());
            assert_eq!(actual.z.to_bits(), expected.z.to_bits());
        }
    }

    #[test]
    fn emitter_cone_degenerate_paths_do_not_consume_rng_draws() {
        let mut state = EmitterBridgeState::new(0x00C0_FFEE);
        let initial_state = state.rand_state;

        assert_eq!(
            sample_cone_direction(Vec3::ZERO, std::f32::consts::FRAC_PI_4, &mut state),
            Vec3::Y
        );
        assert_eq!(sample_cone_direction(Vec3::Y, 0.0, &mut state), Vec3::Y);
        assert_eq!(state.rand_state, initial_state);
    }

    #[test]
    fn spawned_particles_latch_emitter_reference_and_spin_rate() {
        let mut registry = EntityRegistry::new();
        let mut comp = base_component(0.0);
        comp.burst = Some(3);
        comp.spin_rate = 1.25;
        let id = spawn_emitter(&mut registry, comp);
        let mut bridge = Harness::new();
        update_bridge(&mut bridge, &mut registry, 1.0 / 60.0, 0.0);

        let mut found = 0;
        bridge.pool.step(&registry, 0.1, |_| (0.0, Vec3::ZERO));
        for p in bridge.pool.iter() {
            assert_eq!(p.emitter, Some(id));
            assert!((p.rotation - 0.125).abs() < 1e-6);
            found += 1;
        }
        assert_eq!(found, 3);
    }

    #[test]
    fn trail_particles_stamp_the_registered_collection_id_without_fallback() {
        use postretro_visibility::VisibleCells;
        let mut registry = EntityRegistry::new();
        let mut component = base_component(0.0);
        component.sprite = "smoke_puff/smoke_puff_00.png".into();
        component.lifetime = 0.45;
        component.burst = Some(1);
        let emitter = spawn_emitter(&mut registry, component.clone());
        let mut bridge = Harness::new();
        let expected = derive_collection_id(
            &component.sprite,
            Some(component.lifetime),
            None,
            0.0,
            None,
            None,
        );
        let expected_index = bridge.collector.register_sprite(&expected);
        update_bridge(&mut bridge, &mut registry, 1.0 / 60.0, 0.0);
        let particle = bridge.pool.iter().next().unwrap();
        assert_eq!(particle.collection, expected_index);
        assert_eq!(particle.emitter, Some(emitter));
        bridge
            .collector
            .collect(&registry, &bridge.pool, None, &VisibleCells::DrawAll);
        assert_eq!(
            bridge
                .collector
                .iter_collections()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            vec![expected.as_str()]
        );
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
        let mut bridge = Harness::new();

        // Step 1 second of simulated time in fine increments so the
        // accumulator-then-anim phase order matches production.
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        for _ in 0..=60 {
            update_bridge(&mut bridge, &mut registry, dt, t);
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
        let mut bridge = Harness::new();

        // Step well past the tween duration.
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        for _ in 0..120 {
            update_bridge(&mut bridge, &mut registry, dt, t);
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
        let mut bridge = Harness::new();

        update_bridge(&mut bridge, &mut registry, 0.1, 0.0);
        assert_eq!(bridge.tracked_count(), 1);

        registry.despawn(id).unwrap();
        update_bridge(&mut bridge, &mut registry, 0.1, 0.1);
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
        let mut bridge = Harness::new();
        update_bridge(&mut bridge, &mut registry, 0.1, 0.0);
        assert_eq!(bridge.tracked_count(), 1);

        registry
            .remove_component::<BillboardEmitterComponent>(id)
            .unwrap();
        update_bridge(&mut bridge, &mut registry, 0.1, 0.1);
        assert_eq!(bridge.tracked_count(), 0);
    }

    #[test]
    fn no_stale_entries_accumulate_over_many_spawn_despawn_cycles() {
        let mut registry = EntityRegistry::new();
        let mut bridge = Harness::new();
        for cycle in 0..50 {
            let id = spawn_emitter(&mut registry, base_component(2.0));
            update_bridge(&mut bridge, &mut registry, 0.05, cycle as f32);
            registry.despawn(id).unwrap();
            update_bridge(&mut bridge, &mut registry, 0.05, cycle as f32 + 0.05);
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
            map_ordinal: None,
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

        let mut bridge = Harness::new();
        update_bridge(&mut bridge, &mut registry, 0.5, 0.0);

        let found_at_origin = bridge
            .pool
            .iter()
            .any(|p| (p.position - Vec3::new(10.0, 20.0, 30.0)).length() < 1e-3);
        assert!(
            found_at_origin,
            "expected at least one particle spawned at the map entity origin",
        );
    }
    // Regression: the host advanced a new tween by the write-frame delta ahead of its captured start.
    #[test]
    fn replicated_changed_spin_tween_matches_host_samples_and_completion() {
        for replace_running in [false, true] {
            let mut host_registry = EntityRegistry::new();
            let mut component = base_component(0.0);
            if replace_running {
                component.spin_animation = Some(SpinAnimation {
                    duration: 5.0,
                    rate_curve: vec![0.0, 2.0],
                });
            }
            let host_id = spawn_emitter(&mut host_registry, component.clone());
            let mut host = Harness::new();
            update_bridge(&mut host, &mut host_registry, 0.5, 10.0);

            component.spin_animation = Some(SpinAnimation {
                duration: 1.0,
                rate_curve: vec![0.0, 1.0],
            });
            host_registry
                .set_component(host_id, component.clone())
                .unwrap();
            let mut client_registry = EntityRegistry::new();
            let client_id = spawn_emitter(&mut client_registry, component.clone());
            let mut client = Harness::new();
            // The write frame advances 10 -> 10.5, so capture publishes start 10.5.
            // Applying at that shared instant seeds age zero, even on a different local clock.
            assert!(client.seed_replicated_state(&client_registry, client_id, Some(0.0)));

            for (host_time, client_time, expected_rate, complete) in [
                (10.5, 110.5, 0.0, false),
                (11.0, 111.0, 0.5, false),
                (11.5, 111.5, 1.0, true),
            ] {
                if host_time == 11.0 {
                    // Scalar changes and identical tween rewrites keep elapsed time.
                    let mut host_component = host_registry
                        .get_component::<BillboardEmitterComponent>(host_id)
                        .unwrap()
                        .clone();
                    host_component.rate = 0.25;
                    host_component.spin_animation = component.spin_animation.clone();
                    host_registry
                        .set_component(host_id, host_component)
                        .unwrap();
                }
                update_bridge(&mut host, &mut host_registry, 0.5, host_time);
                update_bridge(&mut client, &mut client_registry, 0.5, client_time);
                let host_component = host_registry
                    .get_component::<BillboardEmitterComponent>(host_id)
                    .unwrap();
                let client_component = client_registry
                    .get_component::<BillboardEmitterComponent>(client_id)
                    .unwrap();
                assert!((host_component.spin_rate - expected_rate).abs() < 1e-5);
                assert!((client_component.spin_rate - expected_rate).abs() < 1e-5);
                assert_eq!(host_component.spin_animation.is_none(), complete);
                assert_eq!(client_component.spin_animation.is_none(), complete);
                if !complete {
                    assert!((host.states[&host_id].spin_elapsed - (host_time - 10.5)).abs() < 1e-5);
                    assert!(
                        (client.states[&client_id].spin_elapsed
                            - host.states[&host_id].spin_elapsed)
                            .abs()
                            < 1e-5
                    );
                }
            }
        }
    }

    // Regression: installation starts at zero, not at the first running frame's sample time.
    #[test]
    fn installed_spin_uses_first_delta_without_emitting_during_install() {
        let mut registry = EntityRegistry::new();
        let mut component = base_component(0.0);
        component.burst = Some(2);
        component.spin_animation = Some(SpinAnimation {
            duration: 1.0,
            rate_curve: vec![0.0, 1.0],
        });
        let id = spawn_emitter(&mut registry, component.clone());
        let mut bridge = Harness::new();
        bridge.start_installed_animations(&registry);
        assert_eq!(
            registry
                .get_component::<BillboardEmitterComponent>(id)
                .unwrap(),
            &component
        );
        assert_eq!(bridge.pool.len(), 0);
        update_bridge(&mut bridge, &mut registry, 0.5, 0.5);
        let half = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap();
        assert!((half.spin_rate - 0.5).abs() < 1e-5);
        assert!(half.spin_animation.is_some());
        assert!(half.burst.is_none());
        assert_eq!(bridge.pool.len(), 2);
        update_bridge(&mut bridge, &mut registry, 0.5, 1.0);
        let completed = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap();
        assert!((completed.spin_rate - 1.0).abs() < 1e-5);
        assert!(completed.spin_animation.is_none());
    }

    #[test]
    fn replicated_spin_seed_samples_same_frame_age_then_advances_locally() {
        let mut registry = EntityRegistry::new();
        let mut component = base_component(0.0);
        component.spin_animation = Some(SpinAnimation {
            duration: 5.0,
            rate_curve: vec![0.0, 1.0],
        });
        let id = spawn_emitter(&mut registry, component);
        let mut bridge = Harness::new();
        assert!(bridge.seed_replicated_state(&registry, id, Some(2.0)));
        update_bridge(&mut bridge, &mut registry, 0.5, 10.0);
        assert!(
            (registry
                .get_component::<BillboardEmitterComponent>(id)
                .unwrap()
                .spin_rate
                - 0.4)
                .abs()
                < 1e-5
        );
        assert_eq!(bridge.states[&id].spin_elapsed, 2.0);
        let mut component = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap()
            .clone();
        component.rate = 0.0;
        registry.set_component(id, component).unwrap();
        assert!(bridge.seed_replicated_state(&registry, id, None));
        update_bridge(&mut bridge, &mut registry, 0.5, 10.5);
        assert_eq!(bridge.states[&id].spin_elapsed, 2.5);
        assert!(
            (registry
                .get_component::<BillboardEmitterComponent>(id)
                .unwrap()
                .spin_rate
                - 0.5)
                .abs()
                < 1e-5
        );
        assert!(bridge.seed_replicated_state(&registry, id, Some(6.0)));
        update_bridge(&mut bridge, &mut registry, 0.5, 11.0);
        let component = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap();
        assert!(component.spin_animation.is_none());
        assert_eq!(component.spin_rate, 1.0);
        bridge.clear();
        assert!(bridge.states.is_empty());
    }

    // Regression: raw particle time advanced a tween while its replicated script clock held.
    #[test]
    fn frozen_spin_clock_preserves_seed_and_particles_advance_until_resume() {
        let mut registry = EntityRegistry::new();
        let mut component = base_component(2.0);
        component.spin_animation = Some(SpinAnimation {
            duration: 5.0,
            rate_curve: vec![0.0, 10.0],
        });
        let id = spawn_emitter(&mut registry, component);
        let mut bridge = Harness::new();
        let mut live_counts = HashMap::new();
        assert!(bridge.seed_replicated_state(&registry, id, Some(1.0)));
        for _ in 0..6 {
            bridge.update(&mut registry, 0.5, 0.0, 11.0, &live_counts);
            crate::scripting_systems::particle_sim::tick(&mut bridge.pool, &registry, 0.5, |_| {
                (0.0, Vec3::ZERO)
            });
            live_counts.insert(id, bridge.pool.live_count(id));
            let emitter = registry
                .get_component::<BillboardEmitterComponent>(id)
                .unwrap();
            assert!((emitter.spin_rate - 2.0).abs() < 1e-5);
            assert!(emitter.spin_animation.is_some());
            assert!((bridge.states[&id].spin_elapsed - 1.0).abs() < 1e-5);
        }
        assert_eq!(
            live_counts[&id], 6,
            "emission still consumes raw frame time"
        );
        let oldest_age = bridge.pool.iter().map(|p| p.age).fold(0.0_f32, f32::max);
        assert!(
            (oldest_age - 3.0).abs() < 1e-5,
            "particle simulation also keeps running"
        );
        bridge.update(&mut registry, 0.5, 0.5, 11.5, &live_counts);
        assert!(
            (registry
                .get_component::<BillboardEmitterComponent>(id)
                .unwrap()
                .spin_rate
                - 3.0)
                .abs()
                < 1e-5
        );

        // A write while frozen samples age zero, then consumes the first resumed step.
        let mut replacement = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap()
            .clone();
        replacement.spin_animation = Some(SpinAnimation {
            duration: 1.0,
            rate_curve: vec![0.0, 4.0],
        });
        registry.set_component(id, replacement).unwrap();
        for (time, spin_delta, expected, complete) in [
            (11.5, 0.0, 0.0, false),
            (12.0, 0.5, 2.0, false),
            (12.5, 0.5, 4.0, true),
        ] {
            bridge.update(&mut registry, 0.5, spin_delta, time, &live_counts);
            let emitter = registry
                .get_component::<BillboardEmitterComponent>(id)
                .unwrap();
            assert!((emitter.spin_rate - expected).abs() < 1e-5);
            assert_eq!(emitter.spin_animation.is_none(), complete);
        }
    }

    #[test]
    fn replicated_spin_seed_replaced_locally_starts_at_current_sample() {
        let mut registry = EntityRegistry::new();
        let mut component = base_component(0.0);
        component.spin_animation = Some(SpinAnimation {
            duration: 5.0,
            rate_curve: vec![0.0, 1.0],
        });
        let id = spawn_emitter(&mut registry, component.clone());
        let mut bridge = Harness::new();
        assert!(bridge.seed_replicated_state(&registry, id, Some(2.0)));
        component.spin_animation.as_mut().unwrap().duration = 10.0;
        registry.set_component(id, component).unwrap();
        update_bridge(&mut bridge, &mut registry, 0.5, 10.0);
        assert!(bridge.states[&id].spin_elapsed.abs() < 1e-5);
        update_bridge(&mut bridge, &mut registry, 0.5, 10.5);
        assert!((bridge.states[&id].spin_elapsed - 0.5).abs() < 1e-5);
        assert!(
            (registry
                .get_component::<BillboardEmitterComponent>(id)
                .unwrap()
                .spin_rate
                - 0.05)
                .abs()
                < 1e-5
        );
        assert!(!bridge.seed_replicated_state(&registry, id, Some(f64::NAN)));
        registry.despawn(id).unwrap();
        assert!(!bridge.seed_replicated_state(&registry, id, Some(0.0)));
    }
    #[test]
    fn replicated_none_spin_seed_advances_a_new_tween_and_clear_drops_seed() {
        let mut registry = EntityRegistry::new();
        let mut component = base_component(0.0);
        component.spin_animation = Some(SpinAnimation {
            duration: 5.0,
            rate_curve: vec![0.0, 1.0],
        });
        let id = spawn_emitter(&mut registry, component);
        let mut bridge = Harness::new();
        assert!(bridge.seed_replicated_state(&registry, id, None));
        update_bridge(&mut bridge, &mut registry, 0.5, 10.0);
        assert_eq!(bridge.states[&id].spin_elapsed, 0.5);
        assert!(bridge.seed_replicated_state(&registry, id, Some(2.0)));
        bridge.clear();
        update_bridge(&mut bridge, &mut registry, 0.5, 10.5);
        assert!(bridge.states[&id].spin_elapsed.abs() < 1e-5);
        assert!(!bridge.states[&id].skip_next_spin_advance);
    }
    #[test]
    fn replicated_clear_drops_spin_age_and_skip_before_unseeded_replacement() {
        let mut registry = EntityRegistry::new();
        let mut component = base_component(0.0);
        component.spin_animation = Some(SpinAnimation {
            duration: 5.0,
            rate_curve: vec![0.0, 1.0],
        });
        let id = spawn_emitter(&mut registry, component.clone());
        let mut bridge = Harness::new();
        assert!(bridge.seed_replicated_state(&registry, id, Some(2.0)));
        let mut cleared = component.clone();
        cleared.spin_animation = None;
        registry.set_component(id, cleared).unwrap();
        assert!(bridge.seed_replicated_state(&registry, id, None));
        assert_eq!(bridge.states[&id].spin_elapsed, 0.0);
        assert!(!bridge.states[&id].skip_next_spin_advance);
        registry.set_component(id, component).unwrap();
        assert!(bridge.seed_replicated_state(&registry, id, None));
        update_bridge(&mut bridge, &mut registry, 0.5, 10.0);
        assert_eq!(bridge.states[&id].spin_elapsed, 0.5);
    }
}

#[cfg(test)]
#[path = "emitter_bridge_tests.rs"]
mod integration_tests;
