// Local entity bindings used while applying host snapshots. The client keeps
// stable PRL mover identities here so mover phase is applied to the loaded copy.

use std::collections::{HashMap, HashSet};

use glam::Vec3;
use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
use postretro_entities::components::light::LightComponent;
use postretro_entities::presentation_binding::{
    PresentationBindingKey, PresentationBindingKind, PresentationBindingResolver,
};

use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, FogVolumeComponent,
    KinematicMoverComponent,
};
use postretro_net::wire::{
    ComponentPayload, NetworkId, WireKinematicMoverState, WirePresentationBinding,
};

use crate::presentation_state::apply_presentation_payload;

/// An installed map region a record binds to by map-order index and source
/// entity. Its records carry state only: the region is never interpolated,
/// materialized from a baseline, or despawned on unbinding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RegionBinding {
    Teleporter(u16),
    GhostLink(u16),
}

/// How far past the newest snapshot's arrival a client extrapolates the host's
/// level tick: three snapshot intervals (6 ticks, 100 ms). Snapshots arrive
/// every interval while the host runs, so in steady play this bound is never
/// reached even with a lost snapshot or two and a frame of jitter; when they
/// stop, the reading holds at most this far past the host's last stamped tick
/// plus its transit, which bounds how early a beat can land during a host
/// stall.
const MAX_LEVEL_TICK_EXTRAPOLATION_TICKS: f64 = 3.0 * crate::host::SNAPSHOT_TICK_INTERVAL as f64;

#[derive(Debug, Default)]
pub(super) struct ClientBindings {
    /// Network ids bound to installed local teleporter and ghost-link regions.
    pub(super) region_network_ids: HashMap<NetworkId, RegionBinding>,
    /// Network ids bound to PRL-loaded local kinematic movers, with their stable
    /// compile-time mover id. These entities are predicted/reconciled in place and
    /// must never be remote-interpolated or materialized from a baseline.
    pub(super) mover_network_ids: HashMap<NetworkId, u32>,
    pub(super) presentation_network_ids: HashMap<NetworkId, PresentationBindingKey>,
    rejected_presentation_ids: HashSet<NetworkId>,
    warned_presentation_bindings: HashSet<(u8, u32)>,
    /// Host and local script clocks at the highest-sequence accepted snapshot.
    clock_anchor: Option<(f64, f64)>,
    /// The host's level tick and the local script clock at the same snapshot:
    /// the host timeline a client's `levelLoad` tails land on, re-anchored on
    /// every newest accepted snapshot.
    level_tick_anchor: Option<(u32, f64)>,
    last_applied: HashMap<(NetworkId, u16), ComponentPayload>,
    deferred: HashMap<(NetworkId, u16), ComponentPayload>,
}

impl ClientBindings {
    /// Resolve only authored identities. A rejection remains acknowledged and
    /// ignored until participation/level state is cleared by the caller.
    pub(super) fn resolve_presentation_baseline(
        &mut self,
        registry: &EntityRegistry,
        network_id: NetworkId,
        binding: &WirePresentationBinding,
        components: &[ComponentPayload],
        resolver: &dyn PresentationBindingResolver,
    ) -> Option<EntityId> {
        if self.is_presentation_rejected(network_id) {
            return None;
        }
        let resolved = (|| {
            let kind = match binding.kind {
                0 => PresentationBindingKind::MapLight,
                1 => PresentationBindingKind::FogVolume,
                2 => PresentationBindingKind::MapEntity,
                _ => return None,
            };
            let key = PresentationBindingKey {
                kind,
                index: binding.index,
            };
            if self
                .presentation_network_ids
                .get(&network_id)
                .is_some_and(|prior| *prior != key)
            {
                return None;
            }
            let (id, origin) = resolver.resolve(registry, key)?;
            if !registry.exists(id)
                || !origin.is_finite()
                || !binding.origin.iter().all(|v| v.is_finite())
            {
                return None;
            }
            // Roundtrip checks the local identity kind as well as its index.
            if resolver.key_for_entity(registry, id)?.0 != key {
                return None;
            }
            if origin
                .as_dvec3()
                .distance(Vec3::from_array(binding.origin).as_dvec3())
                > 0.01
            {
                return None;
            }
            if components.is_empty()
                || !components
                    .iter()
                    .all(|payload| presentation_component_present(registry, id, payload))
            {
                return None;
            }
            Some((id, key))
        })();
        if let Some((id, key)) = resolved {
            self.presentation_network_ids.insert(network_id, key);
            return Some(id);
        }
        self.forget_presentation_identity(network_id);
        self.rejected_presentation_ids.insert(network_id);
        if self
            .warned_presentation_bindings
            .insert((binding.kind, binding.index))
        {
            log::warn!(
                "[Net] presentation binding kind {} index {} for {network_id:?} does not match local kind, origin or components; dropping update",
                binding.kind,
                binding.index
            );
        }
        None
    }

    pub(super) fn is_presentation_bound(&self, network_id: NetworkId) -> bool {
        self.presentation_network_ids.contains_key(&network_id)
    }

    pub(super) fn is_presentation_rejected(&self, network_id: NetworkId) -> bool {
        self.rejected_presentation_ids.contains(&network_id)
    }

    /// Mapping removal invalidates payload identity even if the registry entity
    /// survives. Rejected identities stay rejected until a full clear.
    pub(super) fn forget_presentation_identity(&mut self, network_id: NetworkId) {
        self.presentation_network_ids.remove(&network_id);
        self.last_applied.retain(|(id, _), _| *id != network_id);
        self.deferred.retain(|(id, _), _| *id != network_id);
    }

    pub(super) fn clear_presentation(&mut self) {
        self.presentation_network_ids.clear();
        self.rejected_presentation_ids.clear();
        self.warned_presentation_bindings.clear();
        self.clock_anchor = None;
        self.level_tick_anchor = None;
        self.last_applied.clear();
        self.deferred.clear();
    }

    /// Call only after the snapshot sequence gate accepts the snapshot. In a
    /// stalled-client batch, the final accepted snapshot supplies the anchor.
    pub(super) fn accept_presentation_clock(
        &mut self,
        server_script_time: f64,
        local_script_time: f64,
    ) {
        if server_script_time.is_finite() && local_script_time.is_finite() {
            self.clock_anchor = Some((server_script_time, local_script_time));
        }
    }

    /// Call beside [`Self::accept_presentation_clock`], for the same snapshot.
    pub(super) fn accept_level_tick(&mut self, level_tick: u32, local_script_time: f64) {
        if local_script_time.is_finite() {
            self.level_tick_anchor = Some((level_tick, local_script_time));
        }
    }

    /// The host's level tick now: the newest snapshot's level tick plus the
    /// local time since it arrived and half the RTT it spent in flight, in
    /// ticks. Each snapshot carries the host's true fixed-tick count, so the
    /// reading re-anchors on every one and never inherits the time-sync
    /// estimator's offset. The local term stops at
    /// [`MAX_LEVEL_TICK_EXTRAPOLATION_TICKS`]: when snapshots stop — a host
    /// stall, a host level load, a held link — the reading holds instead of
    /// running ahead of a host that is not ticking. `None` until a snapshot and
    /// an RTT sample exist.
    pub(super) fn host_level_tick(
        &self,
        local_script_time: f64,
        rtt_micros: Option<f64>,
    ) -> Option<f64> {
        let rtt = rtt_micros.filter(|rtt| rtt.is_finite() && *rtt >= 0.0)?;
        let (level_tick, accepted_at) = self.level_tick_anchor?;
        if !local_script_time.is_finite() {
            return None;
        }
        let tick_secs = postretro_sim::frame_timing::TICK_DURATION.as_secs_f64();
        let since_accept = ((local_script_time - accepted_at).max(0.0) / tick_secs)
            .min(MAX_LEVEL_TICK_EXTRAPOLATION_TICKS);
        let in_flight = rtt / 2_000_000.0 / tick_secs;
        Some(f64::from(level_tick) + since_accept + in_flight).filter(|reading| reading.is_finite())
    }

    pub(super) fn presentation_host_time(
        &self,
        local_script_time: f64,
        rtt_micros: Option<f64>,
    ) -> Option<f64> {
        rtt_micros
            .filter(|rtt| rtt.is_finite() && *rtt >= 0.0)
            .zip(self.clock_anchor)
            .map(|(rtt, (host, local))| host + rtt / 2_000_000.0 + (local_script_time - local))
            .filter(|host_now| host_now.is_finite() && local_script_time.is_finite())
    }

    /// Apply after the entire snapshot batch and display materialization, on
    /// every frame including frames with no snapshot. RTT is in microseconds.
    pub(super) fn apply_presentation_batch(
        &mut self,
        registry: &mut EntityRegistry,
        map: &HashMap<NetworkId, EntityId>,
        collected: Vec<(NetworkId, ComponentPayload)>,
        local_script_time: f64,
        rtt_micros: Option<f64>,
        resolver: &mut dyn PresentationBindingResolver,
    ) -> usize {
        // A tombstone later in this batch can retire a payload collected earlier.
        self.deferred.retain(|(network_id, _), _| {
            map.contains_key(network_id) && !self.rejected_presentation_ids.contains(network_id)
        });
        let ready_clock = self.presentation_host_time(local_script_time, rtt_micros);
        let collected: Vec<_> = collected
            .into_iter()
            .filter(|(network_id, payload)| {
                !self.is_presentation_rejected(*network_id)
                    && map
                        .get(network_id)
                        .is_some_and(|id| presentation_component_present(registry, *id, payload))
            })
            .collect();
        let Some(host_now) = ready_clock else {
            for (network_id, payload) in collected {
                self.deferred.insert((network_id, payload.kind()), payload);
            }
            return 0;
        };
        // New values win over deferred values before any write occurs.
        for (network_id, payload) in &collected {
            self.deferred.remove(&(*network_id, payload.kind()));
        }
        let mut released: Vec<_> = self.deferred.drain().collect();
        released.sort_by_key(|((network_id, kind), _)| (network_id.0, *kind));
        let pending = released
            .into_iter()
            .map(|((network_id, _), payload)| (network_id, payload))
            .chain(collected);
        let mut applied = 0;
        for (network_id, payload) in pending {
            let Some(&id) = map.get(&network_id) else {
                continue;
            };
            let key = (network_id, payload.kind());
            if self.last_applied.get(&key) == Some(&payload) {
                continue;
            }
            if apply_presentation_payload(
                registry,
                id,
                &payload,
                local_script_time,
                host_now,
                resolver,
            ) {
                self.last_applied.insert(key, payload);
                applied += 1;
            }
        }
        applied
    }
}

pub(super) fn is_presentation_payload(payload: &ComponentPayload) -> bool {
    matches!(
        payload,
        ComponentPayload::LightState(_)
            | ComponentPayload::FogState(_)
            | ComponentPayload::EmitterState(_)
    )
}

fn presentation_component_present(
    registry: &EntityRegistry,
    id: EntityId,
    payload: &ComponentPayload,
) -> bool {
    match payload {
        ComponentPayload::LightState(_) => registry.get_component::<LightComponent>(id).is_ok(),
        ComponentPayload::FogState(_) => registry.get_component::<FogVolumeComponent>(id).is_ok(),
        ComponentPayload::EmitterState(_) => registry
            .get_component::<BillboardEmitterComponent>(id)
            .is_ok(),
        _ => false,
    }
}

pub(super) fn find_loaded_mover_entity(
    registry: &EntityRegistry,
    mover_id: u32,
) -> Option<EntityId> {
    registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .find_map(|(id, value)| {
            let ComponentValue::KinematicMover(mover) = value else {
                return None;
            };
            (mover.mover_id == mover_id).then_some(id)
        })
}

pub(super) fn validate_kinematic_mover_state_binding(
    registry: &EntityRegistry,
    id: EntityId,
    network_id: NetworkId,
    bound_mover_id: Option<u32>,
    wire: &WireKinematicMoverState,
) -> Option<KinematicMoverComponent> {
    let Ok(mover) = registry
        .get_component::<KinematicMoverComponent>(id)
        .cloned()
    else {
        log::warn!(
            "[Net] KinematicMoverState for mover_id {} applied to entity without KinematicMover",
            wire.mover_id
        );
        return None;
    };
    if let Some(bound) = bound_mover_id
        && bound != wire.mover_id
    {
        log::warn!(
            "[Net] KinematicMoverState mover_id {} would rebind {network_id:?} from mover_id {}; dropping phase",
            wire.mover_id,
            bound
        );
        return None;
    }
    if mover.mover_id != wire.mover_id {
        log::warn!(
            "[Net] KinematicMoverState mover_id {} does not match local mover_id {}; dropping phase",
            wire.mover_id,
            mover.mover_id
        );
        return None;
    }
    Some(mover)
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::Transform;
    use postretro_entities::components::light::{FalloffKind, LightKind};
    use postretro_net::wire::{WireFogState, WireLightAnimation, WireLightState};
    use postretro_test_log_capture::LogCapture;

    struct Resolver {
        id: EntityId,
        key: PresentationBindingKey,
        origin: Vec3,
        seeds: Vec<(EntityId, Option<f64>, f32)>,
    }
    impl PresentationBindingResolver for Resolver {
        fn resolve(
            &self,
            _: &EntityRegistry,
            key: PresentationBindingKey,
        ) -> Option<(EntityId, Vec3)> {
            // Deliberately loose kind handling proves the client checks roundtrip.
            (key.index == self.key.index).then_some((self.id, self.origin))
        }
        fn key_for_entity(
            &self,
            _: &EntityRegistry,
            id: EntityId,
        ) -> Option<(PresentationBindingKey, Vec3)> {
            (id == self.id).then_some((self.key, self.origin))
        }
        fn seed_light(
            &mut self,
            registry: &EntityRegistry,
            id: EntityId,
            start: Option<f64>,
        ) -> bool {
            self.seeds.push((
                id,
                start,
                registry
                    .get_component::<LightComponent>(id)
                    .unwrap()
                    .intensity,
            ));
            true
        }
    }
    fn light() -> LightComponent {
        LightComponent {
            origin: [1.0, 2.0, 3.0],
            light_type: LightKind::Point,
            intensity: 1.0,
            color: [1.0; 3],
            falloff_model: FalloffKind::Linear,
            falloff_range: 10.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            is_dynamic: true,
            animated_slot: None,
            follow_transform: false,
            carrier: None,
            animation: None,
        }
    }
    fn setup() -> (EntityRegistry, EntityId, Resolver) {
        let mut registry = EntityRegistry::new();
        let id = registry.try_spawn(Transform::default(), &[]).unwrap();
        registry.set_component(id, light()).unwrap();
        let resolver = Resolver {
            id,
            key: PresentationBindingKey {
                kind: PresentationBindingKind::MapLight,
                index: 3,
            },
            origin: Vec3::new(1.0, 2.0, 3.0),
            seeds: Vec::new(),
        };
        (registry, id, resolver)
    }
    fn wire_binding() -> WirePresentationBinding {
        WirePresentationBinding {
            kind: 0,
            index: 3,
            origin: [1.0, 2.0, 3.0],
        }
    }
    fn payload(intensity: f32, start: Option<f64>) -> ComponentPayload {
        ComponentPayload::LightState(WireLightState {
            intensity,
            color: [0.5; 3],
            cone_direction: None,
            animation: Some(WireLightAnimation {
                period_ms: 5000.0,
                phase: None,
                play_count: Some(1),
                start_active: None,
                brightness: Some(vec![0.0, 1.0]),
                color: None,
                direction: None,
            }),
            anim_start: start,
        })
    }
    fn fog_payload() -> ComponentPayload {
        ComponentPayload::FogState(WireFogState {
            density: 0.1,
            glow: 0.0,
            edge_softness: 1.0,
            falloff: 1.0,
            tint: [1.0; 3],
            saturation: 1.0,
            min_brightness: 0.0,
            light_range: 1.0,
            animation: None,
            anim_start: None,
        })
    }

    #[test]
    fn presentation_bindings_validate_identity_origin_and_all_local_components() {
        let (registry, id, resolver) = setup();
        let before = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        let mut bindings = ClientBindings::default();
        let mut near = wire_binding();
        near.origin[0] += 0.009;
        assert_eq!(
            bindings.resolve_presentation_baseline(
                &registry,
                NetworkId(1),
                &near,
                &[payload(2.0, None)],
                &resolver
            ),
            Some(id)
        );
        assert!(bindings.is_presentation_bound(NetworkId(1)));
        // A matching baseline only binds; the frame batch does the write later.
        assert_eq!(
            registry.get_component::<LightComponent>(id).unwrap(),
            &before
        );
        let mut cases = [wire_binding(); 4];
        cases[0].index = 99;
        cases[1].kind = 1;
        cases[2].origin[0] += 0.011;
        cases[3].kind = 255;
        for (offset, case) in cases.iter().enumerate() {
            let nid = NetworkId(10 + offset as u32);
            assert_eq!(
                bindings.resolve_presentation_baseline(
                    &registry,
                    nid,
                    case,
                    &[payload(2.0, None)],
                    &resolver
                ),
                None
            );
            assert!(bindings.is_presentation_rejected(nid));
        }
        assert_eq!(
            bindings.resolve_presentation_baseline(
                &registry,
                NetworkId(20),
                &wire_binding(),
                &[payload(2.0, None), fog_payload()],
                &resolver
            ),
            None
        );
        assert_eq!(
            registry.get_component::<LightComponent>(id).unwrap(),
            &before
        );
    }

    #[test]
    fn presentation_bindings_warn_once_per_binding_and_keep_rejections_until_clear() {
        let (registry, id, resolver) = setup();
        let logs = LogCapture::start();
        let mut bindings = ClientBindings::default();
        let mut far = wire_binding();
        far.origin[0] += 1.0;
        for nid in [NetworkId(1), NetworkId(2), NetworkId(1)] {
            assert_eq!(
                bindings.resolve_presentation_baseline(
                    &registry,
                    nid,
                    &far,
                    &[payload(2.0, None)],
                    &resolver
                ),
                None
            );
        }
        logs.assert_logged_once(log::Level::Warn, "presentation binding kind 0 index 3");
        bindings.forget_presentation_identity(NetworkId(1));
        assert_eq!(
            bindings.resolve_presentation_baseline(
                &registry,
                NetworkId(1),
                &wire_binding(),
                &[payload(2.0, None)],
                &resolver
            ),
            None
        );
        bindings.clear_presentation();
        assert_eq!(
            bindings.resolve_presentation_baseline(
                &registry,
                NetworkId(1),
                &wire_binding(),
                &[payload(2.0, None)],
                &resolver
            ),
            Some(id)
        );
    }

    #[test]
    fn presentation_bindings_reject_stale_entity_and_conflicting_bound_key() {
        let (mut registry, id, mut resolver) = setup();
        let mut bindings = ClientBindings::default();
        let nid = NetworkId(1);
        assert_eq!(
            bindings.resolve_presentation_baseline(
                &registry,
                nid,
                &wire_binding(),
                &[payload(2.0, None)],
                &resolver
            ),
            Some(id)
        );
        resolver.key.index = 4;
        let mut other = wire_binding();
        other.index = 4;
        assert_eq!(
            bindings.resolve_presentation_baseline(
                &registry,
                nid,
                &other,
                &[payload(2.0, None)],
                &resolver
            ),
            None
        );
        assert!(!bindings.is_presentation_bound(nid));
        registry.despawn(id).unwrap();
        assert_eq!(
            bindings.resolve_presentation_baseline(
                &registry,
                NetworkId(2),
                &other,
                &[payload(2.0, None)],
                &resolver
            ),
            None
        );
    }

    #[test]
    fn presentation_bindings_memoize_original_payload_and_preserve_local_later_write() {
        let (mut registry, id, mut resolver) = setup();
        let nid = NetworkId(1);
        let map = HashMap::from([(nid, id)]);
        let mut bindings = ClientBindings::default();
        bindings.accept_presentation_clock(10.0, 20.0);
        let value = payload(2.0, Some(8.0));
        assert_eq!(
            bindings.apply_presentation_batch(
                &mut registry,
                &map,
                vec![(nid, value.clone())],
                20.0,
                Some(2_000_000.0),
                &mut resolver
            ),
            1
        );
        assert_eq!(resolver.seeds, vec![(id, Some(17.0), 2.0)]);
        let mut local = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone();
        local.intensity = 9.0;
        registry.set_component(id, local).unwrap();
        assert_eq!(
            bindings.apply_presentation_batch(
                &mut registry,
                &map,
                vec![(nid, value.clone())],
                22.0,
                Some(2_000_000.0),
                &mut resolver
            ),
            0
        );
        assert_eq!(
            registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .intensity,
            9.0
        );
        assert_eq!(resolver.seeds.len(), 1);
        bindings.forget_presentation_identity(nid);
        assert_eq!(
            bindings.apply_presentation_batch(
                &mut registry,
                &map,
                vec![(nid, value)],
                22.0,
                Some(2_000_000.0),
                &mut resolver
            ),
            1
        );
    }

    #[test]
    fn presentation_bindings_defer_newest_value_and_release_without_new_snapshot() {
        let (mut registry, id, mut resolver) = setup();
        let nid = NetworkId(1);
        let map = HashMap::from([(nid, id)]);
        let mut bindings = ClientBindings::default();
        bindings.accept_presentation_clock(10.0, 20.0);
        for intensity in [2.0, 3.0] {
            assert_eq!(
                bindings.apply_presentation_batch(
                    &mut registry,
                    &map,
                    vec![(nid, payload(intensity, Some(8.0)))],
                    20.0,
                    None,
                    &mut resolver
                ),
                0
            );
        }
        assert_eq!(
            registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .intensity,
            1.0
        );
        assert_eq!(bindings.deferred.len(), 1);
        assert_eq!(
            bindings.apply_presentation_batch(
                &mut registry,
                &map,
                vec![],
                22.0,
                Some(2_000_000.0),
                &mut resolver
            ),
            1
        );
        assert_eq!(resolver.seeds, vec![(id, Some(17.0), 3.0)]);
        assert!(bindings.deferred.is_empty());
    }

    #[test]
    fn presentation_bindings_initializing_frame_replaces_deferred_before_any_write() {
        let (mut registry, id, mut resolver) = setup();
        let nid = NetworkId(1);
        let map = HashMap::from([(nid, id)]);
        let mut bindings = ClientBindings::default();
        bindings.accept_presentation_clock(10.0, 20.0);
        bindings.apply_presentation_batch(
            &mut registry,
            &map,
            vec![(nid, payload(2.0, Some(8.0)))],
            20.0,
            None,
            &mut resolver,
        );
        assert_eq!(
            bindings.apply_presentation_batch(
                &mut registry,
                &map,
                vec![
                    (nid, payload(3.0, Some(9.0))),
                    (nid, payload(4.0, Some(9.0)))
                ],
                21.0,
                Some(0.0),
                &mut resolver
            ),
            2
        );
        assert_eq!(
            resolver.seeds,
            vec![(id, Some(19.0), 3.0), (id, Some(19.0), 4.0)]
        );
        assert!(bindings.deferred.is_empty());
    }

    #[test]
    fn presentation_bindings_release_other_deferred_keys_before_collected_order() {
        let (mut registry, id, mut resolver) = setup();
        let nid = NetworkId(1);
        let next = NetworkId(2);
        let other = registry.try_spawn(Transform::default(), &[]).unwrap();
        registry.set_component(other, light()).unwrap();
        let map = HashMap::from([(nid, id), (next, other)]);
        let mut bindings = ClientBindings::default();
        bindings.accept_presentation_clock(10.0, 20.0);
        bindings.apply_presentation_batch(
            &mut registry,
            &map,
            vec![(nid, payload(2.0, None))],
            20.0,
            None,
            &mut resolver,
        );
        assert_eq!(
            bindings.apply_presentation_batch(
                &mut registry,
                &map,
                vec![(next, payload(3.0, None))],
                20.0,
                Some(0.0),
                &mut resolver
            ),
            2
        );
        assert_eq!(resolver.seeds, vec![(id, None, 2.0), (other, None, 3.0)]);
    }

    #[test]
    fn presentation_bindings_batch_uses_latest_accepted_clock_and_half_rtt_microseconds() {
        let (mut registry, id, mut resolver) = setup();
        let nid = NetworkId(1);
        let map = HashMap::from([(nid, id)]);
        let mut bindings = ClientBindings::default();
        bindings.accept_presentation_clock(10.0, 20.0);
        let collected = vec![(nid, payload(2.0, Some(8.0)))];
        bindings.accept_presentation_clock(15.0, 20.0);
        assert_eq!(
            bindings.apply_presentation_batch(
                &mut registry,
                &map,
                collected,
                20.0,
                Some(200_000.0),
                &mut resolver
            ),
            1
        );
        assert!((resolver.seeds[0].1.unwrap() - 12.9).abs() < 1e-9);
    }

    #[test]
    fn presentation_bindings_retired_mapping_drops_deferred_and_earlier_collected_payload() {
        let (mut registry, id, mut resolver) = setup();
        let nid = NetworkId(1);
        let mut map = HashMap::from([(nid, id)]);
        let mut bindings = ClientBindings::default();
        bindings.accept_presentation_clock(10.0, 20.0);
        bindings.apply_presentation_batch(
            &mut registry,
            &map,
            vec![(nid, payload(2.0, None))],
            20.0,
            None,
            &mut resolver,
        );
        map.remove(&nid);
        bindings.forget_presentation_identity(nid);
        assert_eq!(
            bindings.apply_presentation_batch(
                &mut registry,
                &map,
                vec![(nid, payload(3.0, None))],
                21.0,
                Some(0.0),
                &mut resolver
            ),
            0
        );
        assert!(bindings.deferred.is_empty());
        assert!(resolver.seeds.is_empty());
        assert_eq!(
            registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .intensity,
            1.0
        );
    }

    #[test]
    fn presentation_bindings_missing_components_do_not_memoize_success() {
        let (mut registry, id, mut resolver) = setup();
        let nid = NetworkId(1);
        let map = HashMap::from([(nid, id)]);
        let mut bindings = ClientBindings::default();
        bindings.accept_presentation_clock(10.0, 20.0);
        registry.remove_component::<LightComponent>(id).unwrap();
        assert_eq!(
            bindings.apply_presentation_batch(
                &mut registry,
                &map,
                vec![(nid, payload(2.0, None))],
                20.0,
                Some(0.0),
                &mut resolver
            ),
            0
        );
        assert!(bindings.last_applied.is_empty());
        registry.set_component(id, light()).unwrap();
        assert_eq!(
            bindings.apply_presentation_batch(
                &mut registry,
                &map,
                vec![(nid, payload(2.0, None))],
                20.0,
                Some(0.0),
                &mut resolver
            ),
            1
        );
    }

    #[test]
    fn presentation_bindings_demotion_clear_preserves_local_entities_and_resets_clock_caches() {
        let (mut registry, id, mut resolver) = setup();
        let nid = NetworkId(1);
        let map = HashMap::from([(nid, id)]);
        let mut bindings = ClientBindings::default();
        bindings.resolve_presentation_baseline(
            &registry,
            nid,
            &wire_binding(),
            &[payload(2.0, None)],
            &resolver,
        );
        bindings.accept_presentation_clock(100.0, 20.0);
        bindings.accept_level_tick(300, 20.0);
        bindings.apply_presentation_batch(
            &mut registry,
            &map,
            vec![(nid, payload(2.0, None))],
            20.0,
            Some(0.0),
            &mut resolver,
        );
        bindings.apply_presentation_batch(
            &mut registry,
            &map,
            vec![(nid, payload(3.0, None))],
            20.0,
            None,
            &mut resolver,
        );
        bindings.clear_presentation();
        assert_eq!(bindings.level_tick_anchor, None);
        assert_eq!(bindings.host_level_tick(20.0, Some(0.0)), None);
        assert!(registry.exists(id));
        assert!(!bindings.is_presentation_bound(nid));
        assert!(bindings.last_applied.is_empty());
        assert!(bindings.deferred.is_empty());
        assert_eq!(bindings.clock_anchor, None);
        assert_eq!(
            bindings.resolve_presentation_baseline(
                &registry,
                nid,
                &wire_binding(),
                &[payload(2.0, None)],
                &resolver
            ),
            Some(id)
        );
        bindings.accept_presentation_clock(1.0, 30.0);
        assert_eq!(
            bindings.apply_presentation_batch(
                &mut registry,
                &map,
                vec![(nid, payload(2.0, Some(0.0)))],
                30.0,
                Some(0.0),
                &mut resolver
            ),
            1
        );
        assert_eq!(resolver.seeds.last().unwrap().1, Some(29.0));
    }

    fn assert_reading(reading: Option<f64>, expected: f64) {
        let reading = reading.expect("a reading");
        assert!(
            (reading - expected).abs() < 1e-9,
            "read {reading}, expected {expected}"
        );
    }

    // Regression: the host level tick read the time-sync estimate, whose
    // offset a host level change or stall biases for seconds, so a client
    // judged beats early. It now reads as the newest snapshot's level tick plus
    // local time since that snapshot arrived and half the RTT, in ticks, and
    // when snapshots stop — a host stall or load — the local term holds at
    // three snapshot intervals instead of running ahead of a host that is not
    // ticking.
    #[test]
    fn host_level_tick_extrapolates_from_the_newest_snapshot_and_holds_at_the_cap() {
        let tick = postretro_sim::frame_timing::TICK_DURATION.as_secs_f64();
        // Six ticks of RTT: three in flight.
        let rtt = Some(6.0 * tick * 1_000_000.0);
        let mut bindings = ClientBindings::default();
        assert_eq!(bindings.host_level_tick(5.0, rtt), None, "no snapshot yet");
        bindings.accept_level_tick(300, 5.0);
        assert_eq!(bindings.host_level_tick(5.0, None), None, "no RTT yet");
        assert_eq!(bindings.host_level_tick(f64::NAN, rtt), None);
        assert_eq!(bindings.host_level_tick(5.0, Some(f64::INFINITY)), None);

        assert_reading(bindings.host_level_tick(5.0, rtt), 303.0);
        assert_reading(bindings.host_level_tick(5.0 + 2.0 * tick, rtt), 305.0);
        assert_eq!(MAX_LEVEL_TICK_EXTRAPOLATION_TICKS, 6.0);
        for held in [5.0 + 6.0 * tick, 6.0, 15.0] {
            assert_reading(bindings.host_level_tick(held, rtt), 309.0);
        }
        // A local clock behind the anchor counts no time since it.
        assert_reading(bindings.host_level_tick(4.0, rtt), 303.0);

        // Each snapshot carries the host's true count, so the next one
        // re-anchors and no drift carries over. A non-finite local clock
        // never anchors.
        bindings.accept_level_tick(320, 15.0);
        bindings.accept_level_tick(999, f64::NAN);
        assert_reading(bindings.host_level_tick(15.0 + tick, rtt), 324.0);
    }
}
