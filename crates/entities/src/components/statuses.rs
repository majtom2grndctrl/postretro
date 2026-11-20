// Live status instances: a dense, registry-owned table of the entities that
// carry at least one, keyed by status and applier, plus the off-tick
// applications latched to the next fixed tick. Bookkeeping only; the sim's
// status stage deals the damage.
// See: context/lib/entity_model.md §Components (Status effects)

use postretro_foundation::status::{MAX_STATUS_INSTANCES, StatusDeclaration, StatusStacking};

use crate::registry::EntityId;

/// Who applied an instance: the applying impact's damager, or with none the
/// applying source id. Separate appliers keep separate instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusApplier {
    Entity(EntityId),
    /// An interned source id ([`StatusTable::source_id`]).
    Source(u16),
}

/// One live instance on one entity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusInstance {
    /// Index into the committed vocabulary's `statuses`.
    pub status: u16,
    pub applier: StatusApplier,
    /// Interned source id of the applying hit: the contributor its ticks
    /// credit.
    pub source: u16,
    pub stacks: u8,
    /// Creation order, unique per table.
    pub serial: u64,
    /// Host tick of the next damage tick.
    pub next_due: u64,
    /// Host tick whose stage ends the instance; `None` lasts until cleared.
    pub expires: Option<u64>,
    /// Host tick of the latest application, for per-tick dedupe.
    pub last_applied: u64,
}

impl StatusInstance {
    /// Expiry rank for replacement: until-cleared outranks every timed one.
    fn time_left_rank(&self) -> u64 {
        self.expires.unwrap_or(u64::MAX)
    }
}

/// The instances one entity carries, in creation order.
#[derive(Debug, Clone)]
pub struct StatusSet {
    pub entity: EntityId,
    /// The entity's registry slot, so removal fixes the slot index in O(1).
    slot: usize,
    len: usize,
    instances: [Option<StatusInstance>; MAX_STATUS_INSTANCES],
}

impl StatusSet {
    fn new(entity: EntityId, slot: usize) -> Self {
        Self {
            entity,
            slot,
            len: 0,
            instances: [None; MAX_STATUS_INSTANCES],
        }
    }

    pub fn instances(&self) -> impl Iterator<Item = &StatusInstance> {
        self.instances[..self.len].iter().flatten()
    }

    fn instances_mut(&mut self) -> impl Iterator<Item = &mut StatusInstance> {
        self.instances[..self.len].iter_mut().flatten()
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// One bit per status kind with a live instance.
    pub fn presence_bits(&self) -> u64 {
        self.instances()
            .filter(|instance| instance.status < 64)
            .fold(0, |bits, instance| bits | (1 << instance.status))
    }

    fn find_mut(&mut self, status: u16, applier: StatusApplier) -> Option<&mut StatusInstance> {
        self.instances_mut()
            .find(|instance| instance.status == status && instance.applier == applier)
    }

    fn push(&mut self, instance: StatusInstance) {
        if self.len == MAX_STATUS_INSTANCES {
            // Replace the least time left; among equals the oldest goes.
            let victim = (0..self.len)
                .min_by_key(|&index| {
                    let held = self.instances[index].expect("dense prefix");
                    (held.time_left_rank(), held.serial)
                })
                .expect("a full set has instances");
            self.remove_at(victim);
        }
        self.instances[self.len] = Some(instance);
        self.len += 1;
    }

    fn remove_at(&mut self, index: usize) {
        self.instances.copy_within(index + 1..self.len, index);
        self.len -= 1;
        self.instances[self.len] = None;
    }

    fn retain(&mut self, mut keep: impl FnMut(&StatusInstance) -> bool) {
        let mut index = 0;
        while index < self.len {
            let instance = self.instances[index].expect("dense prefix");
            if keep(&instance) {
                index += 1;
            } else {
                self.remove_at(index);
            }
        }
    }
}

/// An application made outside a fixed tick, applied at the next tick start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingStatusApplication {
    pub target: EntityId,
    pub status: u16,
    pub applier: StatusApplier,
    pub source: u16,
}

/// What an application did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusApplyOutcome {
    Created,
    Refreshed,
    /// This applier already applied this kind to this target this tick.
    Deduplicated,
}

/// Every live instance in the level, the latched off-tick applications and
/// the interned source ids instances credit.
#[derive(Debug, Clone, Default)]
pub struct StatusTable {
    sets: Vec<StatusSet>,
    /// Registry slot index → index into `sets`, `u32::MAX` when absent.
    by_slot: Vec<u32>,
    sources: Vec<String>,
    pending: Vec<PendingStatusApplication>,
    next_serial: u64,
    /// Whether an application counts on the current host tick (inside the
    /// fixed tick and its post-tick remote-hit flush) or latches to the next.
    immediate: bool,
    /// Due-tick scratch the status stage reuses each tick.
    due: Vec<(EntityId, u64)>,
}

const ABSENT: u32 = u32::MAX;

impl StatusTable {
    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }

    pub fn sets(&self) -> &[StatusSet] {
        &self.sets
    }

    pub fn pending(&self) -> &[PendingStatusApplication] {
        &self.pending
    }

    pub fn applications_immediate(&self) -> bool {
        self.immediate
    }

    pub fn set_applications_immediate(&mut self, immediate: bool) {
        self.immediate = immediate;
    }

    /// Intern `source_id`, returning its stable index. Interning allocates
    /// only the first time a source id is seen.
    pub fn intern_source(&mut self, source_id: &str) -> u16 {
        if let Some(index) = self.sources.iter().position(|known| known == source_id) {
            return index as u16;
        }
        if self.sources.len() >= usize::from(u16::MAX) {
            log::warn!("[Status] source-id table is full; crediting `{source_id}` as the last");
            return u16::MAX - 1;
        }
        self.sources.push(source_id.to_string());
        (self.sources.len() - 1) as u16
    }

    pub fn source_id(&self, source: u16) -> &str {
        self.sources
            .get(usize::from(source))
            .map_or("", String::as_str)
    }

    pub(crate) fn set_at(&self, slot: usize) -> Option<&StatusSet> {
        let index = *self.by_slot.get(slot)?;
        (index != ABSENT).then(|| &self.sets[index as usize])
    }

    fn set_index(&self, slot: usize, entity: EntityId) -> Option<usize> {
        let index = *self.by_slot.get(slot)?;
        (index != ABSENT && self.sets[index as usize].entity == entity).then_some(index as usize)
    }

    /// Apply `status` to `entity` on host tick `tick`. The caller has checked
    /// the target is live with health above zero.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply(
        &mut self,
        slot: usize,
        entity: EntityId,
        status: u16,
        declaration: &StatusDeclaration,
        applier: StatusApplier,
        source: u16,
        tick: u64,
    ) -> StatusApplyOutcome {
        let expires = declaration
            .duration_ticks
            .map(|duration| tick + u64::from(duration));
        if let Some(index) = self.set_index(slot, entity)
            && let Some(instance) = self.sets[index].find_mut(status, applier)
        {
            if instance.last_applied == tick {
                return StatusApplyOutcome::Deduplicated;
            }
            instance.last_applied = tick;
            instance.expires = expires;
            if declaration.stacking == StatusStacking::Stack {
                instance.stacks = (instance.stacks + 1).min(declaration.max_stacks.max(1));
            }
            return StatusApplyOutcome::Refreshed;
        }
        let serial = self.next_serial;
        self.next_serial += 1;
        let instance = StatusInstance {
            status,
            applier,
            source,
            stacks: 1,
            serial,
            next_due: tick + u64::from(declaration.tick_ticks.max(1)),
            expires,
            last_applied: tick,
        };
        let index = match self.set_index(slot, entity) {
            Some(index) => index,
            None => {
                if self.by_slot.len() <= slot {
                    self.by_slot.resize(slot + 1, ABSENT);
                }
                self.sets.push(StatusSet::new(entity, slot));
                let index = self.sets.len() - 1;
                self.by_slot[slot] = index as u32;
                index
            }
        };
        self.sets[index].push(instance);
        StatusApplyOutcome::Created
    }

    /// Latch an application made outside a fixed tick.
    pub(crate) fn latch(&mut self, application: PendingStatusApplication) {
        self.pending.push(application);
    }

    /// Take the latched applications, leaving the buffer's capacity behind
    /// for [`Self::restore_pending_buffer`].
    pub(crate) fn take_pending(&mut self) -> Vec<PendingStatusApplication> {
        std::mem::take(&mut self.pending)
    }

    pub(crate) fn restore_pending_buffer(&mut self, mut buffer: Vec<PendingStatusApplication>) {
        buffer.clear();
        if self.pending.is_empty() {
            self.pending = buffer;
        }
    }

    /// Remove every instance of `status` on `entity`, and its latched
    /// applications.
    pub(crate) fn clear_kind(&mut self, slot: usize, entity: EntityId, status: u16) {
        self.pending
            .retain(|pending| !(pending.target == entity && pending.status == status));
        if let Some(index) = self.set_index(slot, entity) {
            self.sets[index].retain(|instance| instance.status != status);
            self.drop_if_empty(index);
        }
    }

    /// Remove every instance on `entity` and its latched applications.
    pub(crate) fn clear_entity(&mut self, slot: usize, entity: EntityId) {
        self.pending.retain(|pending| pending.target != entity);
        if let Some(index) = self.set_index(slot, entity) {
            self.remove_set(index);
        }
    }

    /// Forget everything: a level unload.
    pub(crate) fn clear(&mut self) {
        for set in &self.sets {
            self.by_slot[set.slot] = ABSENT;
        }
        self.sets.clear();
        self.pending.clear();
    }

    fn drop_if_empty(&mut self, index: usize) {
        if self.sets[index].is_empty() {
            self.remove_set(index);
        }
    }

    fn remove_set(&mut self, index: usize) {
        let removed = self.sets.swap_remove(index);
        self.by_slot[removed.slot] = ABSENT;
        if let Some(moved) = self.sets.get(index) {
            self.by_slot[moved.slot] = index as u32;
        }
    }

    /// Whether `entity` carries a live instance of `status`.
    pub(crate) fn has(&self, slot: usize, entity: EntityId, status: u16) -> bool {
        self.set_index(slot, entity).is_some_and(|index| {
            self.sets[index]
                .instances()
                .any(|instance| instance.status == status)
        })
    }

    pub(crate) fn instance(
        &self,
        slot: usize,
        entity: EntityId,
        serial: u64,
    ) -> Option<StatusInstance> {
        let index = self.set_index(slot, entity)?;
        self.sets[index]
            .instances()
            .find(|instance| instance.serial == serial)
            .copied()
    }

    pub(crate) fn instance_mut(
        &mut self,
        slot: usize,
        entity: EntityId,
        serial: u64,
    ) -> Option<&mut StatusInstance> {
        let index = self.set_index(slot, entity)?;
        self.sets[index]
            .instances_mut()
            .find(|instance| instance.serial == serial)
    }

    /// Fill the reusable due scratch with every instance due on `tick`, in
    /// table order and, within an entity, creation order. Stacks clamp to a
    /// lowered `maxStacks` here, the first tick after a hot reload.
    pub(crate) fn collect_due(
        &mut self,
        tick: u64,
        statuses: &[StatusDeclaration],
    ) -> Vec<(EntityId, u64)> {
        let mut due = std::mem::take(&mut self.due);
        due.clear();
        for set in &mut self.sets {
            let entity = set.entity;
            for instance in set.instances_mut() {
                if let Some(declaration) = statuses.get(usize::from(instance.status)) {
                    instance.stacks = instance.stacks.min(declaration.max_stacks.max(1));
                }
                if instance.next_due <= tick {
                    due.push((entity, instance.serial));
                }
            }
        }
        due
    }

    /// Hand the due scratch back for the next tick.
    pub(crate) fn restore_due(&mut self, due: Vec<(EntityId, u64)>) {
        self.due = due;
    }

    /// End every instance whose time ran out by `tick`, after its due tick
    /// landed.
    pub(crate) fn expire(&mut self, tick: u64) {
        let mut index = 0;
        while index < self.sets.len() {
            self.sets[index].retain(|instance| instance.expires.is_none_or(|end| end > tick));
            if self.sets[index].is_empty() {
                self.remove_set(index);
            } else {
                index += 1;
            }
        }
    }

    /// Keep instances and latched applications across a vocabulary commit by
    /// status name; a removed kind's go.
    pub(crate) fn remap(&mut self, map: impl Fn(u16) -> Option<u16>) {
        self.pending
            .retain_mut(|pending| match map(pending.status) {
                Some(status) => {
                    pending.status = status;
                    true
                }
                None => false,
            });
        let mut index = 0;
        while index < self.sets.len() {
            let set = &mut self.sets[index];
            set.retain(|instance| map(instance.status).is_some());
            for instance in set.instances_mut() {
                instance.status = map(instance.status).expect("retained above");
            }
            if self.sets[index].is_empty() {
                self.remove_set(index);
            } else {
                index += 1;
            }
        }
    }
}

impl crate::registry::EntityRegistry {
    /// The live status table.
    pub fn statuses(&self) -> &StatusTable {
        &self.statuses
    }

    /// Mark whether applications count on the current host tick (inside the
    /// fixed tick and its post-tick remote-hit flush) or latch to the next.
    pub fn set_status_applications_immediate(&mut self, immediate: bool) {
        self.statuses.set_applications_immediate(immediate);
    }

    /// Intern a source id for an applier or a credited contributor.
    pub fn intern_status_source(&mut self, source_id: &str) -> u16 {
        self.statuses.intern_source(source_id)
    }

    /// Apply registered status `status` to `target`. Inside a fixed tick it
    /// counts on this tick; outside one it latches to the next tick start.
    /// Returns `None` when refused: a stale target, one without health or at
    /// health ≤ 0, or an unregistered kind.
    pub fn apply_status(
        &mut self,
        target: EntityId,
        status: u16,
        applier: StatusApplier,
        source: u16,
    ) -> Option<StatusApplyOutcome> {
        if !self.status_target_accepts(target, status) {
            return None;
        }
        if !self.statuses.applications_immediate() {
            self.statuses.latch(PendingStatusApplication {
                target,
                status,
                applier,
                source,
            });
            return Some(StatusApplyOutcome::Created);
        }
        let tick = self.host_tick();
        let vocabulary = std::sync::Arc::clone(self.defense_vocabulary());
        let declaration = vocabulary.statuses.get(usize::from(status))?;
        Some(self.statuses.apply(
            target.slot_index(),
            target,
            status,
            declaration,
            applier,
            source,
            tick,
        ))
    }

    /// Apply every latched application on the tick just started, in landing
    /// order. One whose target has gone, died or lost its kind since is
    /// dropped. Allocation-free once the latch buffer has grown.
    pub fn apply_latched_statuses(&mut self) {
        if self.statuses.pending().is_empty() {
            return;
        }
        let pending = self.statuses.take_pending();
        let tick = self.host_tick();
        let vocabulary = std::sync::Arc::clone(self.defense_vocabulary());
        for application in &pending {
            if !self.status_target_accepts(application.target, application.status) {
                continue;
            }
            let Some(declaration) = vocabulary.statuses.get(usize::from(application.status)) else {
                continue;
            };
            self.statuses.apply(
                application.target.slot_index(),
                application.target,
                application.status,
                declaration,
                application.applier,
                application.source,
                tick,
            );
        }
        self.statuses.restore_pending_buffer(pending);
    }

    fn status_target_accepts(&self, target: EntityId, status: u16) -> bool {
        self.exists(target)
            && usize::from(status) < self.defense_vocabulary().statuses.len()
            && self
                .get_component::<crate::components::health::HealthComponent>(target)
                .is_ok_and(|health| !crate::components::health::is_depleted(health))
    }

    /// Remove every instance of `status` on `target`, whoever applied it, and
    /// discard its latched applications.
    pub fn clear_status(&mut self, target: EntityId, status: u16) {
        if self.exists(target) {
            self.statuses
                .clear_kind(target.slot_index(), target, status);
        }
    }

    /// Remove every instance on `target`: the death latch, a respawn.
    pub fn clear_statuses(&mut self, target: EntityId) {
        if self.exists(target) {
            self.statuses.clear_entity(target.slot_index(), target);
        }
    }

    /// Whether `target` carries a live instance of `status`.
    pub fn has_status(&self, target: EntityId, status: u16) -> bool {
        self.exists(target) && self.statuses.has(target.slot_index(), target, status)
    }

    /// One bit per status kind live on `target`.
    pub fn status_presence_bits(&self, target: EntityId) -> u64 {
        if !self.exists(target) {
            return 0;
        }
        self.statuses
            .set_at(target.slot_index())
            .filter(|set| set.entity == target)
            .map_or(0, StatusSet::presence_bits)
    }

    /// The instance `serial` on `target`, if still live.
    pub fn status_instance(&self, target: EntityId, serial: u64) -> Option<StatusInstance> {
        self.statuses.instance(target.slot_index(), target, serial)
    }

    /// Move instance `serial`'s next damage tick on by `ticks`.
    pub fn advance_status_tick(&mut self, target: EntityId, serial: u64, ticks: u32) {
        if let Some(instance) = self
            .statuses
            .instance_mut(target.slot_index(), target, serial)
        {
            instance.next_due += u64::from(ticks.max(1));
        }
    }

    /// The instances due this tick, into a scratch the caller hands back
    /// with [`Self::restore_status_due`].
    pub fn collect_due_statuses(&mut self) -> Vec<(EntityId, u64)> {
        let tick = self.host_tick();
        let vocabulary = std::sync::Arc::clone(self.defense_vocabulary());
        self.statuses.collect_due(tick, &vocabulary.statuses)
    }

    pub fn restore_status_due(&mut self, due: Vec<(EntityId, u64)>) {
        self.statuses.restore_due(due);
    }

    /// End every instance whose time ran out on this tick.
    pub fn expire_statuses(&mut self) {
        let tick = self.host_tick();
        self.statuses.expire(tick);
    }
}
