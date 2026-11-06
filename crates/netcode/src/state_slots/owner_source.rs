// Owner-private source values: the per-owner value each owner-private slot
// projects for one pawn, from its components or its seat's per-owner store.
// See: context/lib/networking.md §Game-logic-owned apply invariant · context/lib/scripting.md §5

use super::*;

/// The current shared-slot source value: the slot table's current value, lowered to
/// the wire mirror. `None` when the slot has no value yet or a non-finite value (kept
/// off the wire). Shared slots are global — they have one value regardless of owner.
pub(super) fn shared_source_value(slot_table: &SlotTable, name: &str) -> Option<WireSlotValue> {
    let record = slot_table.get(name)?;
    let value = record.value.as_ref()?;
    slot_value_to_wire(value)
}

/// The per-owner source value for an owner-private slot. Descriptor-fed player slots
/// read from owner-specific component state: `player.health` / `player.maxHealth`
/// from the owning pawn's live `HealthComponent`, `player.swimming` /
/// `player.immersion` from its movement state and output-only immersion component;
/// weapon cooldown, ammo, heat,
/// cell, and reload state resolve through its `Inventory` to the sibling
/// `WeaponComponent`;
/// ammo reserve reads the owning pawn's `AmmoReserve`; a defense-layer slot reads
/// the pawn's layer, and a pawn without it yields `Unset`, which clears the
/// owner's slot. Per-owner mod slots read the value for this pawn's seat before
/// the global fallback. Any other
/// owner-private slot falls back to the slot table's current global value.
/// `None` when no source value exists.
pub(super) fn owner_private_source_value(
    slot_table: &SlotTable,
    registry: &EntityRegistry,
    name: &str,
    pawn: EntityId,
    _weapon_owners: &WeaponOwners,
    weapon_slots: &WeaponSlotProjection,
) -> Option<WireSlotValue> {
    if let Some(value) =
        postretro_scripting_core::player_slots::defense_slot_value(registry, name, pawn)
    {
        // A pawn without the layer reads absent on its client as on the host,
        // so the absence is sent as `Unset`, the client's clear. Sending
        // nothing would leave a layer the pawn lost at its last value there.
        return match value {
            Some(value) => slot_value_to_wire(&value),
            None => Some(WireSlotValue::Unset),
        };
    }
    if let Some(value) = pawn_component_slot_for_pawn(registry, name, pawn) {
        // A pawn with no source sends nothing rather than falling through to
        // the global scalar, which is the host's own value.
        return value.as_ref().and_then(slot_value_to_wire);
    }
    if let Some(value) = descriptor_weapon_cooldown_for_pawn(registry, name, pawn) {
        return value;
    }
    if let Some(value) = weapon_slots.wire_sample(name) {
        return value;
    }
    // A correlated slot's descriptor is an array; its plain table value would
    // make the client reject the whole batch. Its projection above is its only
    // source.
    if ReplicatedWireShape::for_name(name).is_correlated() {
        return None;
    }
    if let Some(value) = per_owner_slot_value_for_pawn(slot_table, registry, name, pawn) {
        return value;
    }
    let record = slot_table.get(name)?;
    let value = record.value.as_ref()?;
    slot_value_to_wire(value)
}

/// Read a per-owner slot for one pawn's durable seat. The outer option says
/// whether this declaration is per-owner; the inner option is its source value.
/// A per-owner slot with no pawn-seat binding deliberately returns `Some(None)`,
/// preventing the global scalar fallback from leaking another owner's value.
pub(super) fn per_owner_slot_value_for_pawn(
    slot_table: &SlotTable,
    registry: &EntityRegistry,
    name: &str,
    pawn: EntityId,
) -> Option<Option<WireSlotValue>> {
    let record = slot_table.get(name)?;
    if !record.schema.per_owner {
        return None;
    }
    let Some(seat) = registry.seat_for_pawn(pawn) else {
        return Some(None);
    };
    Some(record.per_seat_value(seat).and_then(slot_value_to_wire))
}

/// Project ammo/reload slots from one owner's pawn and sibling weapon.
/// The outer `Option` identifies names owned by this projection; the inner
/// option is absent when ammo has no valid source, preventing fallback to the
/// host's global HUD slots and cross-owner leakage.
#[cfg(test)]
pub(super) fn descriptor_ammo_for_pawn(
    registry: &EntityRegistry,
    name: &str,
    pawn: EntityId,
    _weapon_owners: &WeaponOwners,
) -> Option<Option<SlotValue>> {
    WeaponSlotProjection::for_pawn(registry, pawn).slot_value(name)
}

/// Owner-private projection samples reload through its own endpoint cursor.
pub(super) const OWNER_PROJECTION_READ: ReloadRead =
    ReloadRead::Feedback(ReloadFeedbackConsumer::OwnerProjection);

/// One owner's slot-correlated weapon values, from its own pawn's active weapon.
pub(super) struct WeaponSlotProjection {
    pub(super) weapon: Option<EntityId>,
    /// The host wieldable slot every value describes: the pawn's active slot,
    /// whether or not it holds a weapon. `None` for a pawn with no inventory.
    pub(super) wieldable_slot: Option<usize>,
    pub(super) magazine: Option<f32>,
    pub(super) reserve: Option<f32>,
    pub(super) reload_progress: f32,
    pub(super) reload_active: bool,
    pub(super) resource: ResourceSlotProjection,
}

impl WeaponSlotProjection {
    pub(super) fn for_pawn(registry: &EntityRegistry, pawn: EntityId) -> Self {
        let inventory = if registry.exists(pawn) {
            registry.get_component::<Inventory>(pawn).ok()
        } else {
            None
        };
        let wieldable_slot = inventory.map(|inventory| inventory.active_slot);
        let weapon = inventory
            .and_then(Inventory::active_wieldable)
            .filter(|weapon| registry.get_component::<WeaponComponent>(*weapon).is_ok());
        let component =
            weapon.and_then(|weapon| registry.get_component::<WeaponComponent>(weapon).ok());
        let ammo_reserve = registry.get_component::<AmmoReserve>(pawn).ok();
        let read = |slot| {
            component.and_then(|weapon| {
                weapon_slot_value(weapon, ammo_reserve, slot, OWNER_PROJECTION_READ)
            })
        };
        let number = |slot| match read(slot) {
            Some(SlotValue::Number(value)) => Some(value),
            _ => None,
        };
        let magazine = number(PlayerSlot::Ammo);
        let reserve = number(PlayerSlot::AmmoReserve);
        let reload_progress = number(PlayerSlot::ReloadProgress).unwrap_or(0.0);
        let reload_active = matches!(
            read(PlayerSlot::ReloadActive),
            Some(SlotValue::Boolean(true))
        );

        Self {
            weapon,
            wieldable_slot,
            magazine,
            reserve,
            reload_progress,
            reload_active,
            resource: ResourceSlotProjection::of(wieldable_slot, component),
        }
    }

    pub(super) fn slot_value(&self, name: &str) -> Option<Option<SlotValue>> {
        let value = match name {
            MAGAZINE_SLOT => self.magazine.map(SlotValue::Number),
            RESERVE_SLOT => self.reserve.map(SlotValue::Number),
            RELOAD_PROGRESS_SLOT => Some(SlotValue::Number(self.reload_progress)),
            RELOAD_ACTIVE_SLOT => Some(SlotValue::Boolean(self.reload_active)),
            _ => return None,
        };
        Some(value)
    }

    /// The slot-correlated wire sample for `name`. The outer option names
    /// the slots this projection owns; the inner one is `None` when nothing is
    /// sent, so the client keeps its value. It mirrors what the host HUD
    /// publishes for its own pawn:
    ///
    /// - The magazine and reserve of a live resourceless weapon are an
    ///   authoritative absence, sent as `[slot]` so the client clears them.
    ///   With no weapon in the active slot, or no inventory, they are not sent.
    /// - Reload progress and the reload flag are always sent. A pawn with no
    ///   inventory sends the HUD's defaults (no progress, not reloading)
    ///   attributed to slot 0: it holds no weapon for the value to describe,
    ///   and any weapon the client holds in slot 0 is, for the host, not
    ///   reloading. Sending nothing would leave a client's stale reload flag up.
    pub(super) fn wire_sample(&self, name: &str) -> Option<Option<WireSlotValue>> {
        if let Some(sample) = self.resource.wire_sample(name) {
            return Some(sample);
        }
        let value = self.slot_value(name)?;
        let sample = match name {
            MAGAZINE_SLOT | RESERVE_SLOT => self.wieldable_slot.and_then(|slot| match value {
                Some(value) => wieldable_slot_sample(slot, &value),
                None if self.weapon.is_some() => Some(absent_wieldable_slot_sample(slot)),
                None => None,
            }),
            _ => value
                .and_then(|value| wieldable_slot_sample(self.wieldable_slot.unwrap_or(0), &value)),
        };
        Some(sample)
    }
}

/// Read a pawn-component slot (health, max health, swimming, immersion) for
/// `name` from `pawn` through the shared per-pawn lookup. The outer option
/// names those slots; the inner one is `None` for a pawn carrying no source
/// component. Each owned pawn is read per-owner, so each client's snapshot
/// carries its own value.
pub(super) fn pawn_component_slot_for_pawn(
    registry: &EntityRegistry,
    name: &str,
    pawn: EntityId,
) -> Option<Option<SlotValue>> {
    let slot = PlayerSlot::from_name(name).filter(|slot| !slot.reads_active_weapon())?;
    Some(player_slot_value(registry, slot, pawn))
}

/// Read the owner-private active weapon cooldown for `pawn`. The value is not on
/// the pawn: its inventory identifies the active sibling weapon entity. The
/// outer option names the cooldown slot; the inner one is `None` when there is
/// nothing to send (no inventory, no active weapon, or a non-finite cooldown),
/// so the slot never falls through to a plain table value.
pub(super) fn descriptor_weapon_cooldown_for_pawn(
    registry: &EntityRegistry,
    name: &str,
    pawn: EntityId,
) -> Option<Option<WireSlotValue>> {
    if name != WEAPON_COOLDOWN_SLOT {
        return None;
    }
    Some(active_weapon_cooldown_sample(registry, pawn))
}

pub(super) fn active_weapon_cooldown_sample(
    registry: &EntityRegistry,
    pawn: EntityId,
) -> Option<WireSlotValue> {
    let active = active_weapon(registry, pawn)?;
    let value = weapon_slot_value(
        active.weapon,
        None,
        PlayerSlot::WeaponCooldownMs,
        ReloadRead::Feedback(ReloadFeedbackConsumer::OwnerProjection),
    )?;
    wieldable_slot_sample(active.slot, &value)
}
