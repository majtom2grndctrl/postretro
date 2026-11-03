//! Target-keyed overlay storage for [`PresentationPool`], partitioned by kind.
//!
//! Each overlay kind owns a disjoint entity-keyed map, so roster retain,
//! clear, linger expiry, and the visible cap of one kind never touch another
//! kind's instances. Only level-lifetime and authoring-replacement resets drop
//! every kind at once.

use std::collections::HashMap;

use postretro_entities::{
    EntityId, PresentationFact, PresentationFacts, PresentationTemplateHandle,
};

use super::{PresentationPool, update_fact};

/// Pool-side identity of a keyed overlay family. The manifest's overlay
/// source maps onto exactly one of these; the pool never reads manifest data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverlayKind {
    /// Health/shield bars over recently damaged enemies.
    DamagedEnemies,
    /// Downed-teammate markers. Always projected as authoritative entries,
    /// so local linger expiry never removes them.
    AssistableTeammates,
}

impl OverlayKind {
    const COUNT: usize = 2;

    const fn index(self) -> usize {
        match self {
            OverlayKind::DamagedEnemies => 0,
            OverlayKind::AssistableTeammates => 1,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct LivePresentationOverlay {
    pub(super) template: PresentationTemplateHandle,
    pub(super) facts: PresentationFacts,
    pub(super) world_anchor: glam::Vec3,
    last_damaged_time_seconds: f64,
    /// Stable producer identity used only when local damage times tie.
    /// Authoritative overlays are selected externally and never ranked here.
    eviction_order: u64,
    pub(super) instance_id: u64,
    linger_seconds: f64,
    pub(super) suppressed: bool,
    /// The authoritative roster owns expiry/removal independently of drawing.
    authoritative: bool,
}

/// One entity-keyed map per [`OverlayKind`]; the key is effectively
/// `(kind, entity)`.
#[derive(Debug, Default)]
pub(super) struct KeyedOverlays {
    by_kind: [HashMap<EntityId, LivePresentationOverlay>; OverlayKind::COUNT],
}

impl KeyedOverlays {
    fn map(&self, kind: OverlayKind) -> &HashMap<EntityId, LivePresentationOverlay> {
        &self.by_kind[kind.index()]
    }

    fn map_mut(&mut self, kind: OverlayKind) -> &mut HashMap<EntityId, LivePresentationOverlay> {
        &mut self.by_kind[kind.index()]
    }

    pub(super) fn len(&self) -> usize {
        self.by_kind.iter().map(HashMap::len).sum()
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &LivePresentationOverlay> {
        self.by_kind.iter().flat_map(HashMap::values)
    }

    /// Drop locally owned entries whose linger has elapsed. Each entry carries
    /// its own clock, and authoritative entries of every kind are exempt.
    pub(super) fn expire_lingering(&mut self, frame_time_seconds: f64) {
        for map in &mut self.by_kind {
            map.retain(|_, overlay| {
                overlay.authoritative
                    || frame_time_seconds - overlay.last_damaged_time_seconds
                        < overlay.linger_seconds
            });
        }
    }

    fn clear_all(&mut self) {
        for map in &mut self.by_kind {
            map.clear();
        }
    }

    fn ensure_capacity(&mut self, kind: OverlayKind, max_visible: usize) {
        let map = self.map_mut(kind);
        if map.capacity() < max_visible {
            map.reserve(max_visible.saturating_sub(map.len()));
        }
    }
}

impl PresentationPool {
    /// Create or refresh one target-keyed overlay of `kind`. Its cap counts
    /// only that kind's entries and is separate from the spawn ring's
    /// `capacity`; no archetype or kind can evict another.
    pub fn refresh_overlay(
        &mut self,
        kind: OverlayKind,
        entity: EntityId,
        template: PresentationTemplateHandle,
        linger_seconds: f64,
        max_visible: usize,
        eviction_order: u64,
    ) {
        if max_visible == 0 {
            return;
        }
        self.overlays.ensure_capacity(kind, max_visible);
        let frame_time_seconds = self.frame_time_seconds;
        if let Some(overlay) = self.overlays.map_mut(kind).get_mut(&entity) {
            overlay.template = template;
            overlay.linger_seconds = linger_seconds.max(0.0);
            overlay.last_damaged_time_seconds = frame_time_seconds;
            overlay.eviction_order = eviction_order;
            overlay.authoritative = false;
            return;
        }
        if self.overlays.map(kind).len() >= max_visible {
            let Some((least_recent, least_recent_key)) = self.least_recent_overlay(kind) else {
                return;
            };
            let incoming_key = (frame_time_seconds, eviction_order);
            if incoming_key <= least_recent_key {
                return;
            }
            self.overlays.map_mut(kind).remove(&least_recent);
        }
        let instance_id = self.next_overlay_instance_id();
        self.overlays.map_mut(kind).insert(
            entity,
            LivePresentationOverlay {
                template,
                facts: PresentationFacts::new(),
                world_anchor: glam::Vec3::ZERO,
                last_damaged_time_seconds: frame_time_seconds,
                eviction_order,
                instance_id,
                linger_seconds: linger_seconds.max(0.0),
                suppressed: true,
                authoritative: false,
            },
        );
    }

    /// Project an externally selected roster of `kind` without ranking or
    /// restarting a local linger clock. The cap counts only that kind's
    /// entries. The caller removes outgoing members before adding
    /// replacements, and owns expiry even when rendering is skipped.
    pub fn upsert_authoritative_overlay(
        &mut self,
        kind: OverlayKind,
        entity: EntityId,
        template: PresentationTemplateHandle,
        max_visible: usize,
    ) -> bool {
        if max_visible == 0 {
            return false;
        }
        if let Some(overlay) = self.overlays.map_mut(kind).get_mut(&entity) {
            overlay.template = template;
            overlay.authoritative = true;
            return true;
        }
        if self.overlays.map(kind).len() >= max_visible {
            return false;
        }
        self.overlays.ensure_capacity(kind, max_visible);
        let instance_id = self.next_overlay_instance_id();
        self.overlays.map_mut(kind).insert(
            entity,
            LivePresentationOverlay {
                template,
                facts: PresentationFacts::new(),
                world_anchor: glam::Vec3::ZERO,
                last_damaged_time_seconds: 0.0,
                eviction_order: 0,
                instance_id,
                linger_seconds: 0.0,
                suppressed: true,
                authoritative: true,
            },
        );
        true
    }

    fn next_overlay_instance_id(&mut self) -> u64 {
        let instance_id = self.next_instance_id;
        self.next_instance_id = self.next_instance_id.wrapping_add(1);
        instance_id
    }

    /// Update the shipped damaged-enemy fact tuple in place. These three names
    /// are stable template vocabulary, so the warm per-frame stamp reuses map
    /// nodes and key Strings instead of rebuilding a BTreeMap.
    pub fn stamp_damaged_enemy_overlay(
        &mut self,
        entity: EntityId,
        health_fraction: f32,
        shield_fraction: f32,
        has_shield: bool,
        world_anchor: glam::Vec3,
        suppressed: bool,
    ) {
        let Some(overlay) = self
            .overlays
            .map_mut(OverlayKind::DamagedEnemies)
            .get_mut(&entity)
        else {
            return;
        };
        update_fact(
            &mut overlay.facts,
            "healthFraction",
            PresentationFact::Number(health_fraction),
        );
        update_fact(
            &mut overlay.facts,
            "shieldFraction",
            PresentationFact::Number(shield_fraction),
        );
        update_fact(
            &mut overlay.facts,
            "hasShield",
            PresentationFact::Bool(has_shield),
        );
        overlay.world_anchor = world_anchor;
        overlay.suppressed = suppressed;
    }

    /// Set one named fact on an existing overlay in place, reusing its map node
    /// and key String on the warm path. Missing overlays are ignored.
    pub fn stamp_overlay_fact(
        &mut self,
        kind: OverlayKind,
        entity: EntityId,
        name: &'static str,
        value: PresentationFact,
    ) {
        if let Some(overlay) = self.overlays.map_mut(kind).get_mut(&entity) {
            update_fact(&mut overlay.facts, name, value);
        }
    }

    /// Update only the moving world anchor and anchor-derived suppression.
    /// Value-only network facts must not reset the last-damage linger clock.
    pub fn update_overlay_anchor(
        &mut self,
        kind: OverlayKind,
        entity: EntityId,
        world_anchor: Option<glam::Vec3>,
        suppressed: bool,
    ) {
        let Some(overlay) = self.overlays.map_mut(kind).get_mut(&entity) else {
            return;
        };
        let Some(world_anchor) = world_anchor else {
            overlay.suppressed = true;
            return;
        };
        overlay.world_anchor = world_anchor;
        overlay.suppressed = suppressed;
    }

    pub fn evict_overlay(&mut self, kind: OverlayKind, entity: EntityId) {
        self.overlays.map_mut(kind).remove(&entity);
    }

    /// Prune one kind's projected set in place to match its externally owned
    /// roster. Other kinds are untouched.
    pub fn retain_overlays(
        &mut self,
        kind: OverlayKind,
        mut predicate: impl FnMut(EntityId) -> bool,
    ) {
        self.overlays
            .map_mut(kind)
            .retain(|entity, _| predicate(*entity));
    }

    /// Whether this entity owns a projected overlay of `kind`. Local overlays
    /// use frame-time linger; authoritative overlays remain until explicit
    /// removal.
    pub fn has_overlay(&self, kind: OverlayKind, entity: EntityId) -> bool {
        self.overlays.map(kind).contains_key(&entity)
    }

    /// Drop one kind's overlays when its configuration goes away. Other kinds
    /// and spawn presentation survive.
    pub fn clear_overlays(&mut self, kind: OverlayKind) {
        self.overlays.map_mut(kind).clear();
    }

    /// Drop every kind's overlays after their authoring snapshot is replaced.
    /// Spawn presentation intentionally survives this separately-owned reset.
    pub fn clear_all_overlays(&mut self) {
        self.overlays.clear_all();
    }

    /// Drop every world-anchored instance at a level-lifetime boundary.
    /// Template registrations are session-scoped, but their live anchors and
    /// facts are not.
    pub fn clear_world_instances(&mut self) {
        self.spawns.clear();
        self.overlays.clear_all();
    }

    /// Live `(spawns, overlays)` totals; the overlay count spans every kind.
    #[cfg(any(test, feature = "test-support"))]
    pub fn live_counts(&self) -> (usize, usize) {
        (self.spawns.len(), self.overlays.len())
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn overlay_count(&self, kind: OverlayKind) -> usize {
        self.overlays.map(kind).len()
    }

    #[cfg(test)]
    pub fn tracked_overlay_ids(&self, kind: OverlayKind) -> Vec<EntityId> {
        self.overlays.map(kind).keys().copied().collect()
    }

    pub fn tracked_overlay_ids_iter(
        &self,
        kind: OverlayKind,
    ) -> impl Iterator<Item = EntityId> + '_ {
        self.overlays.map(kind).keys().copied()
    }

    fn least_recent_overlay(&self, kind: OverlayKind) -> Option<(EntityId, (f64, u64))> {
        self.overlays
            .map(kind)
            .iter()
            .min_by(|(_, left), (_, right)| {
                left.last_damaged_time_seconds
                    .partial_cmp(&right.last_damaged_time_seconds)
                    .expect("presentation frame clock stays finite")
                    .then_with(|| left.eviction_order.cmp(&right.eviction_order))
            })
            .map(|(entity, overlay)| {
                (
                    *entity,
                    (overlay.last_damaged_time_seconds, overlay.eviction_order),
                )
            })
    }

    #[cfg(test)]
    pub(super) fn overlay_ids(&self, kind: OverlayKind) -> Vec<EntityId> {
        let mut ids: Vec<_> = self.overlays.map(kind).keys().copied().collect();
        ids.sort_by_key(|id| id.to_raw());
        ids
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn overlay_facts(&self, kind: OverlayKind, entity: EntityId) -> Option<&PresentationFacts> {
        self.overlays
            .map(kind)
            .get(&entity)
            .map(|overlay| &overlay.facts)
    }

    #[cfg(test)]
    pub fn overlay_is_suppressed(&self, kind: OverlayKind, entity: EntityId) -> Option<bool> {
        self.overlays
            .map(kind)
            .get(&entity)
            .map(|overlay| overlay.suppressed)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn overlay_anchor(&self, kind: OverlayKind, entity: EntityId) -> Option<glam::Vec3> {
        self.overlays
            .map(kind)
            .get(&entity)
            .map(|overlay| overlay.world_anchor)
    }
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec3};
    use postretro_entities::{EntityRegistry, Transform};

    use super::super::MotionPreference;
    use super::*;

    const BAR: OverlayKind = OverlayKind::DamagedEnemies;
    const MARKER: OverlayKind = OverlayKind::AssistableTeammates;

    fn template(name: &str) -> PresentationTemplateHandle {
        PresentationTemplateHandle::from(name)
    }

    fn advance(pool: &mut PresentationPool, registry: &mut EntityRegistry, dt: f32) -> usize {
        let inputs = pool.advance_and_collect_inputs(
            registry,
            dt,
            Mat4::IDENTITY,
            [800, 600],
            MotionPreference::Full,
        );
        let count = inputs.len();
        pool.recycle_draw_inputs(inputs);
        count
    }

    // R8: the same entity may hold one entry per kind.
    #[test]
    fn one_entity_holds_independent_entries_per_kind() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(0);
        pool.refresh_overlay(BAR, pawn, template("bar"), 1.0, 4, 1);
        assert!(pool.upsert_authoritative_overlay(MARKER, pawn, template("marker"), 4));
        pool.stamp_overlay_fact(MARKER, pawn, "inRange", PresentationFact::Bool(true));

        assert_eq!(pool.live_counts(), (0, 2));
        assert_eq!(advance(&mut pool, &mut registry, 0.0), 2);
        assert_eq!(
            pool.overlay_facts(MARKER, pawn).unwrap().get("inRange"),
            Some(&PresentationFact::Bool(true))
        );
        assert!(pool.overlay_facts(BAR, pawn).unwrap().is_empty());

        pool.evict_overlay(BAR, pawn);
        assert!(!pool.has_overlay(BAR, pawn));
        assert!(pool.has_overlay(MARKER, pawn));
    }

    // R8: roster retain and no-config clear of one kind leave the other alone.
    #[test]
    fn retain_and_clear_are_scoped_to_one_kind() {
        let mut registry = EntityRegistry::new();
        let enemy = registry.spawn(Transform::default());
        let pawn = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(0);
        assert!(pool.upsert_authoritative_overlay(BAR, enemy, template("bar"), 4));
        assert!(pool.upsert_authoritative_overlay(MARKER, pawn, template("marker"), 4));

        pool.retain_overlays(BAR, |_| false);
        assert_eq!(pool.overlay_ids(BAR), []);
        assert_eq!(pool.overlay_ids(MARKER), [pawn]);

        assert!(pool.upsert_authoritative_overlay(BAR, enemy, template("bar"), 4));
        pool.retain_overlays(MARKER, |_| false);
        assert_eq!(pool.overlay_ids(BAR), [enemy]);
        assert_eq!(pool.overlay_ids(MARKER), []);

        assert!(pool.upsert_authoritative_overlay(MARKER, pawn, template("marker"), 4));
        pool.clear_overlays(BAR);
        assert_eq!(pool.overlay_ids(BAR), []);
        assert_eq!(pool.overlay_ids(MARKER), [pawn]);

        assert!(pool.upsert_authoritative_overlay(BAR, enemy, template("bar"), 4));
        pool.clear_overlays(MARKER);
        assert_eq!(pool.overlay_ids(BAR), [enemy]);
        assert_eq!(pool.overlay_ids(MARKER), []);
        assert_eq!(
            pool.tracked_overlay_ids_iter(BAR).collect::<Vec<_>>(),
            [enemy]
        );
        assert_eq!(pool.tracked_overlay_ids_iter(MARKER).count(), 0);
    }

    // R13 (pool): per-frame linger expiry removes an elapsed local bar but
    // never an authoritative marker, however long the frame gap.
    #[test]
    fn linger_expiry_never_removes_an_authoritative_marker() {
        let mut registry = EntityRegistry::new();
        let enemy = registry.spawn(Transform::default());
        let pawn = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(0);
        pool.refresh_overlay(BAR, enemy, template("bar"), 0.1, 4, 1);
        assert!(pool.upsert_authoritative_overlay(MARKER, pawn, template("marker"), 4));
        pool.update_overlay_anchor(MARKER, pawn, Some(Vec3::new(0.0, 1.0, 0.0)), false);

        for _ in 0..3 {
            advance(&mut pool, &mut registry, 60.0);
        }

        assert_eq!(pool.overlay_ids(BAR), []);
        assert_eq!(pool.overlay_ids(MARKER), [pawn]);
        assert_eq!(
            pool.overlay_anchor(MARKER, pawn),
            Some(Vec3::new(0.0, 1.0, 0.0))
        );
    }

    // R8: the authoritative cap counts only its own kind's entries.
    #[test]
    fn authoritative_cap_is_applied_per_kind() {
        let mut registry = EntityRegistry::new();
        let enemies: Vec<_> = (0..2)
            .map(|_| registry.spawn(Transform::default()))
            .collect();
        let pawns: Vec<_> = (0..3)
            .map(|_| registry.spawn(Transform::default()))
            .collect();
        let mut pool = PresentationPool::new(0);
        for &enemy in &enemies {
            assert!(pool.upsert_authoritative_overlay(BAR, enemy, template("bar"), 2));
        }
        // A full bar kind does not consume the marker budget.
        assert!(pool.upsert_authoritative_overlay(MARKER, pawns[0], template("marker"), 2));
        assert!(pool.upsert_authoritative_overlay(MARKER, pawns[1], template("marker"), 2));
        assert!(!pool.upsert_authoritative_overlay(MARKER, pawns[2], template("marker"), 2));
        // A full marker kind does not block a bar replacement either.
        pool.retain_overlays(BAR, |entity| entity == enemies[0]);
        assert!(pool.upsert_authoritative_overlay(BAR, pawns[2], template("bar"), 2));

        assert_eq!(pool.overlay_ids(BAR), [enemies[0], pawns[2]]);
        assert_eq!(pool.overlay_ids(MARKER), [pawns[0], pawns[1]]);
    }

    // R8: local-linger cap eviction picks its victim only from its own kind.
    #[test]
    fn local_cap_eviction_never_evicts_another_kind() {
        let mut registry = EntityRegistry::new();
        let first = registry.spawn(Transform::default());
        let second = registry.spawn(Transform::default());
        let pawn = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(0);
        // The marker is older and ranks lowest if the cap were shared.
        assert!(pool.upsert_authoritative_overlay(MARKER, pawn, template("marker"), 1));
        pool.refresh_overlay(BAR, first, template("bar"), 10.0, 1, 1);
        advance(&mut pool, &mut registry, 0.5);
        pool.refresh_overlay(BAR, second, template("bar"), 10.0, 1, 2);

        assert_eq!(pool.overlay_ids(BAR), [second]);
        assert_eq!(pool.overlay_ids(MARKER), [pawn]);
    }

    // R8: authoring replacement and level unload clear both kinds.
    #[test]
    fn reload_and_unload_clear_every_kind() {
        let mut registry = EntityRegistry::new();
        let enemy = registry.spawn(Transform::default());
        let pawn = registry.spawn(Transform::default());
        let mut pool = PresentationPool::new(0);
        let fill = |pool: &mut PresentationPool| {
            pool.refresh_overlay(BAR, enemy, template("bar"), 1.0, 4, 1);
            assert!(pool.upsert_authoritative_overlay(MARKER, pawn, template("marker"), 4));
        };

        fill(&mut pool);
        pool.clear_all_overlays();
        assert_eq!(pool.live_counts(), (0, 0));

        fill(&mut pool);
        pool.clear_world_instances();
        assert_eq!(pool.live_counts(), (0, 0));
    }
}
