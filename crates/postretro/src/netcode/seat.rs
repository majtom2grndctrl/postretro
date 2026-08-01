// Session-lifetime player seats and their carried cross-level state.
// See: context/lib/networking.md §Slot lifecycle

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::inventory::{Inventory, WIELDABLE_SLOT_CAPACITY};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{AmmoReserve, EntityId, EntityRegistry};
use postretro_foundation::Seat;
use postretro_net::wire::{ConnectClaim, RosterEntry, SessionId};

/// State retained by a session seat when its current pawn leaves a level.
///
/// A missing record deliberately seeds no defaults on a fresh seat.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CarriedState {
    pub(crate) health_current: Option<f32>,
    pub(crate) reserve: AmmoReserve,
    pub(crate) wieldables: [Option<String>; WIELDABLE_SLOT_CAPACITY],
    pub(crate) magazines: [Option<u32>; WIELDABLE_SLOT_CAPACITY],
    pub(crate) active_slot: usize,
    pub(crate) placement: Option<usize>,
}

impl Default for CarriedState {
    fn default() -> Self {
        Self {
            health_current: None,
            reserve: AmmoReserve::new(),
            wieldables: std::array::from_fn(|_| None),
            magazines: [None; WIELDABLE_SLOT_CAPACITY],
            active_slot: 0,
            placement: None,
        }
    }
}

/// One exhaustive binding site for every carried field.
///
/// The `CarriedState` destructure has no wildcard: extending the state record
/// requires deciding whether the new value is harvested, restored, or skipped
/// at this ledger before the crate compiles.
enum CarriedField<'a> {
    HealthCurrent(&'a mut Option<f32>),
    Reserve(&'a mut AmmoReserve),
    Wieldables(&'a mut [Option<String>; WIELDABLE_SLOT_CAPACITY]),
    Magazines(&'a mut [Option<u32>; WIELDABLE_SLOT_CAPACITY]),
    ActiveSlot(&'a mut usize),
    Placement(&'a mut Option<usize>),
}

fn carried_fields(state: &mut CarriedState) -> [CarriedField<'_>; 6] {
    let CarriedState {
        health_current,
        reserve,
        wieldables,
        magazines,
        active_slot,
        placement,
    } = state;
    [
        CarriedField::HealthCurrent(health_current),
        CarriedField::Reserve(reserve),
        CarriedField::Wieldables(wieldables),
        CarriedField::Magazines(magazines),
        CarriedField::ActiveSlot(active_slot),
        CarriedField::Placement(placement),
    ]
}

/// Future rejoin-hold expiry measured against the session's accumulated clock.
///
/// Holds are intentionally only represented here; Task 2 neither advances the
/// clock nor changes admission behavior based on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HoldDeadline(pub(crate) Duration);

/// Durable host-local player identities for one running session.
///
/// The table is intentionally above transport participation: a `Seat` is minted
/// on admission and is never reused, while a client binding and a pawn binding
/// may disappear and later be replaced.
#[derive(Debug)]
pub(crate) struct SeatTable {
    session_id: SessionId,
    next_seat: u32,
    carried: HashMap<Seat, Option<CarriedState>>,
    client_bindings: HashMap<Seat, u64>,
    pawn_bindings: HashMap<Seat, EntityId>,
    connect_claims: HashMap<Seat, ConnectClaim>,
    #[allow(dead_code)] // The deadline map is structural only until rejoin holds ship.
    hold_deadlines: HashMap<Seat, HoldDeadline>,
    next_placement_cursor: usize,
    /// Coalesces all roster-affecting mutations in one transport poll into one
    /// publication after its lifecycle batch completes.
    roster_dirty: bool,
}

impl SeatTable {
    /// Create the table for a single-player or listen-host session, reserving
    /// seat zero for the local player.
    pub(crate) fn new() -> Result<Self, getrandom::Error> {
        let mut session_id = [0; 16];
        getrandom::fill(&mut session_id)?;
        Ok(Self::with_session_id(SessionId(session_id)))
    }

    fn with_session_id(session_id: SessionId) -> Self {
        let mut carried = HashMap::new();
        carried.insert(Seat(0), None);
        Self {
            session_id,
            next_seat: 1,
            carried,
            client_bindings: HashMap::new(),
            pawn_bindings: HashMap::new(),
            connect_claims: HashMap::new(),
            hold_deadlines: HashMap::new(),
            next_placement_cursor: 0,
            roster_dirty: true,
        }
    }

    #[must_use]
    pub(crate) fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Record the host player's durable assertion on seat zero when one is
    /// available. Hosts without a persistable player id remain anonymous.
    pub(crate) fn set_local_claim(&mut self, claim: Option<ConnectClaim>) {
        let previous = self.connect_claims.get(&Seat(0));
        if previous == claim.as_ref() {
            return;
        }
        match claim {
            Some(claim) => {
                self.connect_claims.insert(Seat(0), claim);
            }
            None => {
                self.connect_claims.remove(&Seat(0));
            }
        }
        self.roster_dirty = true;
    }

    /// Mint a non-reusable remote seat at the admission edge.
    ///
    /// `None` means the u16 seat namespace is exhausted; the caller keeps the
    /// connection live, but it cannot acquire a durable player seat.
    pub(crate) fn mint_admitted(
        &mut self,
        client_id: u64,
        claim: Option<ConnectClaim>,
        is_currently_closed: bool,
    ) -> Option<Seat> {
        if is_currently_closed {
            return None;
        }
        if let Some(seat) = self.seat_for_client(client_id) {
            return Some(seat);
        }
        let seat = Seat(u16::try_from(self.next_seat).ok()?);
        self.next_seat = self.next_seat.checked_add(1)?;
        self.carried.insert(seat, None);
        self.client_bindings.insert(seat, client_id);
        if let Some(claim) = claim {
            self.connect_claims.insert(seat, claim);
        }
        self.roster_dirty = true;
        Some(seat)
    }

    #[must_use]
    pub(crate) fn seat_for_client(&self, client_id: u64) -> Option<Seat> {
        self.client_bindings
            .iter()
            .find_map(|(seat, bound)| (*bound == client_id).then_some(*seat))
    }

    #[must_use]
    pub(crate) fn seat_for_pawn(&self, pawn: EntityId) -> Option<Seat> {
        self.pawn_bindings
            .iter()
            .find_map(|(seat, bound)| (*bound == pawn).then_some(*seat))
    }

    /// Drop the short-lived client-id association after the transport reports a
    /// slot disconnect. The seat, claim, carried state, and pawn history remain.
    pub(crate) fn unbind_client(&mut self, client_id: u64) -> Option<Seat> {
        let seat = self.seat_for_client(client_id)?;
        self.client_bindings.remove(&seat);
        self.roster_dirty = true;
        Some(seat)
    }

    /// Whether this poll needs one consolidated roster publication.
    pub(crate) fn take_roster_dirty(&mut self) -> bool {
        std::mem::take(&mut self.roster_dirty)
    }

    /// Build a deterministic seat-keyed roster snapshot from the table's own
    /// lifecycle bindings and immutable claims. Seat contents never leave this
    /// table: only the id and the current connection fact are projected.
    #[must_use]
    pub(crate) fn roster_entries(&self) -> Vec<RosterEntry> {
        let mut seats: Vec<Seat> = self.carried.keys().copied().collect();
        seats.sort_unstable_by_key(|seat| seat.0);
        seats
            .into_iter()
            .map(|seat| {
                let claim = self.connect_claims.get(&seat);
                RosterEntry {
                    seat: seat.0,
                    player_id: claim.map(|claim| claim.player_id),
                    display_name: claim
                        .map(|claim| claim.display_name.clone())
                        .unwrap_or_default(),
                    connected: seat == Seat(0) || self.client_bindings.contains_key(&seat),
                }
            })
            .collect()
    }

    /// Associate a newly spawned pawn with its durable seat.
    pub(crate) fn bind_pawn(&mut self, seat: Seat, pawn: EntityId) {
        if self.carried.contains_key(&seat) {
            self.pawn_bindings.insert(seat, pawn);
        }
    }

    /// Preserve every carryable component currently present on this pawn.
    ///
    /// Lookup is by pawn identity, not current client binding: a same-poll
    /// disconnect/rebind cannot make the old dying pawn write into the new one.
    pub(crate) fn harvest_pawn(&mut self, registry: &EntityRegistry, pawn: EntityId) {
        let Some(seat) = self.seat_for_pawn(pawn) else {
            return;
        };
        let health_current = registry
            .get_component::<HealthComponent>(pawn)
            .ok()
            .map(|health| health.current);
        let reserve = registry.get_component::<AmmoReserve>(pawn).ok().cloned();
        let inventory = registry.get_component::<Inventory>(pawn).ok().cloned();
        if health_current.is_none() && reserve.is_none() && inventory.is_none() {
            return;
        }
        let carried = self
            .carried
            .entry(seat)
            .or_insert_with(|| Some(CarriedState::default()));
        let state = carried.get_or_insert_with(CarriedState::default);

        for field in carried_fields(state) {
            match field {
                CarriedField::HealthCurrent(carried_health) => {
                    if let Some(health_current) = health_current {
                        *carried_health = Some(health_current);
                    }
                }
                CarriedField::Reserve(carried_reserve) => {
                    if let Some(reserve) = reserve.as_ref() {
                        *carried_reserve = reserve.clone();
                    }
                }
                CarriedField::Wieldables(carried_wieldables) => {
                    if let Some(inventory) = inventory.as_ref() {
                        for (slot, weapon) in inventory.wieldables.iter().enumerate() {
                            let Some(weapon) = weapon else {
                                carried_wieldables[slot] = None;
                                continue;
                            };
                            if let Ok(provenance) =
                                registry.get_component::<DescriptorProvenance>(*weapon)
                            {
                                carried_wieldables[slot] = Some(provenance.canonical_name.clone());
                            }
                        }
                    }
                }
                CarriedField::Magazines(carried_magazines) => {
                    if let Some(inventory) = inventory.as_ref() {
                        for (slot, weapon) in inventory.wieldables.iter().enumerate() {
                            let Some(weapon) = weapon else {
                                carried_magazines[slot] = None;
                                continue;
                            };
                            if let Ok(weapon) = registry.get_component::<WeaponComponent>(*weapon) {
                                carried_magazines[slot] = Some(weapon.magazine);
                            }
                        }
                    }
                }
                CarriedField::ActiveSlot(carried_active_slot) => {
                    if let Some(inventory) = inventory.as_ref() {
                        *carried_active_slot = inventory.active_slot;
                    }
                }
                // Task 6 owns placement assignment. It is intentionally listed
                // here so the carry shape cannot grow around the ledger.
                CarriedField::Placement(_) => {}
            }
        }
    }

    /// Harvest every currently bound pawn. Missing pawns/components preserve
    /// their prior records exactly.
    pub(crate) fn harvest_bound_pawns(&mut self, registry: &EntityRegistry) {
        let pawns: Vec<EntityId> = self.pawn_bindings.values().copied().collect();
        for pawn in pawns {
            self.harvest_pawn(registry, pawn);
        }
    }

    /// Restore a recorded health value after descriptor materialization.
    ///
    /// A fresh seat has `None`, so it keeps the descriptor default. The absolute
    /// write preserves the component's health bounds and does not publish damage.
    #[allow(dead_code)] // Unit-test seam; production restores in the descriptor spawn helpers.
    pub(crate) fn restore_health(&self, seat: Seat, registry: &mut EntityRegistry, pawn: EntityId) {
        let Some(Some(state)) = self.carried.get(&seat) else {
            return;
        };
        let Some(health_current) = state.health_current else {
            return;
        };
        if health_current <= 0.0 {
            return;
        }
        postretro_entities::components::health::set_health_absolute(registry, pawn, health_current);
    }

    #[must_use]
    pub(crate) fn carried_state(&self, seat: Seat) -> Option<&CarriedState> {
        self.carried.get(&seat).and_then(Option::as_ref)
    }

    /// Reset live-pawn identity and level-scoped placement after actual level
    /// unload. Suspension does not call this: its world and pawn bindings remain
    /// live on resume.
    pub(crate) fn clear_pawn_bindings_for_level_unload(&mut self) {
        for state in self.carried.values_mut().flatten() {
            for field in carried_fields(state) {
                match field {
                    CarriedField::HealthCurrent(_)
                    | CarriedField::Reserve(_)
                    | CarriedField::Wieldables(_)
                    | CarriedField::Magazines(_)
                    | CarriedField::ActiveSlot(_) => {}
                    CarriedField::Placement(placement) => *placement = None,
                }
            }
        }
        self.pawn_bindings.clear();
    }

    /// Clear the only level-scoped carried member while preserving the allocation
    /// cursor. A later level's first join must not reset to placement zero.
    pub(crate) fn clear_placements_for_level_unload(&mut self) {
        for carried in self.carried.values_mut().flatten() {
            carried.placement = None;
        }
    }

    /// Assign or recall a placement for `seat` without allowing a held seat or a
    /// currently-live pawn to be overlapped whenever a free index exists.
    ///
    /// `live_placements` deliberately comes from the spawning path rather than
    /// from seat bindings alone: map installation may leave live player pawns
    /// that are not currently represented by a remote connection binding.
    pub(crate) fn assign_placement(
        &mut self,
        seat: Seat,
        placement_count: usize,
        live_placements: impl IntoIterator<Item = usize>,
    ) -> Option<usize> {
        if placement_count == 0 || !self.carried.contains_key(&seat) {
            return None;
        }

        if let Some(placement) = self
            .carried
            .get(&seat)
            .and_then(Option::as_ref)
            .and_then(|state| state.placement)
            .filter(|placement| *placement < placement_count)
        {
            return Some(placement);
        }

        let mut occupied: HashSet<usize> = live_placements
            .into_iter()
            .filter(|placement| *placement < placement_count)
            .collect();
        occupied.extend(
            self.carried
                .iter()
                .filter(|(other, _)| **other != seat && !self.client_bindings.contains_key(other))
                .filter_map(|(_, carried)| {
                    carried
                        .as_ref()
                        .and_then(|state| state.placement)
                        .filter(|placement| *placement < placement_count)
                }),
        );

        let fallback = self.next_placement_cursor % placement_count;
        let placement = (0..placement_count)
            .map(|offset| (self.next_placement_cursor + offset) % placement_count)
            .find(|candidate| !occupied.contains(candidate))
            .unwrap_or_else(|| {
                log::warn!(
                    "[Net] every player_spawn placement is occupied; reusing index {fallback}"
                );
                fallback
            });
        self.next_placement_cursor = placement.wrapping_add(1);
        let state = self
            .carried
            .get_mut(&seat)
            .expect("seat presence checked before placement assignment")
            .get_or_insert_with(CarriedState::default);
        state.placement = Some(placement);
        Some(placement)
    }

    #[cfg(test)]
    fn carried_state_for_test(&self, seat: Seat) -> Option<&CarriedState> {
        self.carried_state(seat)
    }

    #[cfg(test)]
    pub(crate) fn from_test_session_id(session_id: [u8; 16]) -> Self {
        Self::with_session_id(SessionId(session_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::data_descriptors::HealthDescriptor;
    use postretro_entities::registry::Transform;
    use postretro_scripting_core::data_descriptors::{FireMode, ResolutionMode, WeaponDescriptor};

    fn health(max: f32, current: f32) -> HealthComponent {
        let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
            max,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        });
        health.current = current;
        health
    }

    fn assert_health_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 1.0e-6,
            "expected {expected}, got {actual}"
        );
    }

    fn weapon(magazine: u32) -> WeaponComponent {
        let mut weapon = WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 10.0,
            range: 20.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        });
        weapon.magazine = magazine;
        weapon
    }

    fn provenance(canonical_name: &str) -> DescriptorProvenance {
        DescriptorProvenance {
            canonical_name: canonical_name.to_string(),
            owned_components: Default::default(),
            map_overrides: Default::default(),
            spawn_path: DescriptorSpawnPath::DefaultWeapon,
        }
    }

    #[test]
    fn single_player_health_survives_level_boundary() {
        let mut seats = SeatTable::from_test_session_id([7; 16]);
        let mut old_registry = EntityRegistry::new();
        let old_pawn = old_registry.spawn(Transform::default());
        old_registry
            .set_component(old_pawn, health(100.0, 37.5))
            .unwrap();
        seats.bind_pawn(Seat(0), old_pawn);

        seats.harvest_bound_pawns(&old_registry);
        old_registry.clear_for_level_unload();
        seats.clear_pawn_bindings_for_level_unload();

        let mut new_registry = EntityRegistry::new();
        let new_pawn = new_registry.spawn(Transform::default());
        new_registry
            .set_component(new_pawn, health(100.0, 100.0))
            .unwrap();
        seats.restore_health(Seat(0), &mut new_registry, new_pawn);
        seats.bind_pawn(Seat(0), new_pawn);

        assert_health_eq(
            new_registry
                .get_component::<HealthComponent>(new_pawn)
                .unwrap()
                .current,
            37.5,
        );
    }

    #[test]
    fn fresh_remote_seat_keeps_descriptor_health_default() {
        let mut seats = SeatTable::from_test_session_id([8; 16]);
        let seat = seats
            .mint_admitted(41, None, false)
            .expect("seat space remains");
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry.set_component(pawn, health(100.0, 100.0)).unwrap();

        seats.restore_health(seat, &mut registry, pawn);

        assert_health_eq(
            registry
                .get_component::<HealthComponent>(pawn)
                .unwrap()
                .current,
            100.0,
        );
        assert!(seats.carried_state_for_test(seat).is_none());
    }

    #[test]
    fn harvest_missing_pawn_preserves_prior_health() {
        let mut seats = SeatTable::from_test_session_id([9; 16]);
        let mut registry = EntityRegistry::new();
        let first_pawn = registry.spawn(Transform::default());
        registry
            .set_component(first_pawn, health(100.0, 42.0))
            .unwrap();
        seats.bind_pawn(Seat(0), first_pawn);
        seats.harvest_pawn(&registry, first_pawn);
        registry.despawn(first_pawn).unwrap();

        seats.harvest_bound_pawns(&registry);

        assert_health_eq(
            seats
                .carried_state_for_test(Seat(0))
                .and_then(|state| state.health_current)
                .expect("harvest retains prior health"),
            42.0,
        );
    }

    #[test]
    fn harvest_carries_inventory_names_magazines_reserve_and_active_slot() {
        let mut seats = SeatTable::from_test_session_id([10; 16]);
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry.set_component(pawn, health(100.0, 41.0)).unwrap();
        let mut reserve = AmmoReserve::new();
        reserve.credit("shells", 13);
        reserve.set_exact("rockets", 0);
        registry.set_component(pawn, reserve).unwrap();

        let pistol = registry.spawn(Transform::default());
        registry.set_component(pistol, weapon(4)).unwrap();
        registry
            .set_component(pistol, provenance("pistol"))
            .unwrap();
        let launcher = registry.spawn(Transform::default());
        registry.set_component(launcher, weapon(1)).unwrap();
        registry
            .set_component(launcher, provenance("rocket_launcher"))
            .unwrap();
        let inventory = Inventory {
            wieldables: [
                Some(pistol),
                None,
                Some(launcher),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
            active_slot: 2,
            ..Inventory::default()
        };
        registry.set_component(pawn, inventory).unwrap();
        seats.bind_pawn(Seat(0), pawn);

        seats.harvest_pawn(&registry, pawn);

        let carried = seats
            .carried_state_for_test(Seat(0))
            .expect("pawn components create a carried record");
        assert_health_eq(carried.health_current.expect("health carries"), 41.0);
        assert_eq!(carried.reserve.available("shells"), 13);
        assert_eq!(carried.reserve.available("rockets"), 0);
        assert_eq!(carried.wieldables[0].as_deref(), Some("pistol"));
        assert_eq!(carried.wieldables[1], None);
        assert_eq!(carried.wieldables[2].as_deref(), Some("rocket_launcher"));
        assert_eq!(carried.magazines[0], Some(4));
        assert_eq!(carried.magazines[2], Some(1));
        assert_eq!(carried.active_slot, 2);
    }

    #[test]
    fn nonpositive_carried_health_keeps_descriptor_default() {
        let mut seats = SeatTable::from_test_session_id([11; 16]);
        let mut old_registry = EntityRegistry::new();
        let old_pawn = old_registry.spawn(Transform::default());
        old_registry
            .set_component(old_pawn, health(100.0, 0.0))
            .unwrap();
        seats.bind_pawn(Seat(0), old_pawn);
        seats.harvest_pawn(&old_registry, old_pawn);

        let mut new_registry = EntityRegistry::new();
        let new_pawn = new_registry.spawn(Transform::default());
        new_registry
            .set_component(new_pawn, health(100.0, 100.0))
            .unwrap();
        seats.restore_health(Seat(0), &mut new_registry, new_pawn);

        assert_health_eq(
            new_registry
                .get_component::<HealthComponent>(new_pawn)
                .unwrap()
                .current,
            100.0,
        );
    }

    #[test]
    fn level_unload_clears_placement_even_without_a_live_pawn() {
        let mut seats = SeatTable::from_test_session_id([12; 16]);
        seats.carried.insert(
            Seat(0),
            Some(CarriedState {
                placement: Some(3),
                ..Default::default()
            }),
        );

        seats.clear_pawn_bindings_for_level_unload();

        assert_eq!(
            seats
                .carried_state_for_test(Seat(0))
                .expect("carried state remains")
                .placement,
            None
        );
    }

    #[test]
    fn admission_mints_each_seat_once_and_keeps_session_id() {
        let mut seats = SeatTable::from_test_session_id([3; 16]);
        let first = seats.mint_admitted(11, None, false).unwrap();
        let same = seats.mint_admitted(11, None, false).unwrap();
        let second = seats.mint_admitted(12, None, false).unwrap();

        assert_eq!(first, Seat(1));
        assert_eq!(same, first);
        assert_eq!(second, Seat(2));
        assert_eq!(seats.session_id(), SessionId([3; 16]));
    }

    #[test]
    fn same_poll_rejection_does_not_mint_an_admitted_seat() {
        let mut seats = SeatTable::from_test_session_id([4; 16]);

        let seat = seats.mint_admitted(23, None, true);

        assert_eq!(seat, None);
        assert_eq!(seats.seat_for_client(23), None);
        assert_eq!(seats.carried_state_for_test(Seat(1)), None);
    }

    #[test]
    fn roster_tracks_claims_and_connection_lifecycle_by_seat() {
        let mut seats = SeatTable::from_test_session_id([6; 16]);
        seats.set_local_claim(Some(ConnectClaim {
            player_id: postretro_net::wire::PlayerClaimId([0x10; 16]),
            display_name: "Host".to_owned(),
        }));
        let remote = seats
            .mint_admitted(
                44,
                Some(ConnectClaim {
                    player_id: postretro_net::wire::PlayerClaimId([0x20; 16]),
                    display_name: "Runner".to_owned(),
                }),
                false,
            )
            .expect("seat namespace has room");

        assert!(seats.take_roster_dirty());
        assert!(!seats.take_roster_dirty(), "a poll publishes at most once");
        assert_eq!(
            seats.roster_entries(),
            vec![
                RosterEntry {
                    seat: 0,
                    player_id: Some(postretro_net::wire::PlayerClaimId([0x10; 16])),
                    display_name: "Host".to_owned(),
                    connected: true,
                },
                RosterEntry {
                    seat: remote.0,
                    player_id: Some(postretro_net::wire::PlayerClaimId([0x20; 16])),
                    display_name: "Runner".to_owned(),
                    connected: true,
                },
            ]
        );

        assert_eq!(seats.unbind_client(44), Some(remote));
        assert!(seats.take_roster_dirty());
        assert!(
            !seats.roster_entries()[1].connected,
            "a dropped seat remains rostered but is no longer connected"
        );
    }

    #[test]
    fn placement_assignment_persists_by_seat_and_skips_live_and_held_occupants() {
        let mut seats = SeatTable::from_test_session_id([12; 16]);
        let first = seats
            .mint_admitted(41, None, false)
            .expect("first remote seat");
        let second = seats
            .mint_admitted(42, None, false)
            .expect("second remote seat");
        let third = seats
            .mint_admitted(43, None, false)
            .expect("third remote seat");

        assert_eq!(seats.assign_placement(first, 3, []), Some(0));
        assert_eq!(
            seats.assign_placement(second, 3, [0]),
            Some(1),
            "a live pawn at index zero keeps a new seat away from it"
        );
        assert_eq!(seats.unbind_client(first), Some(first));
        assert_eq!(
            seats.assign_placement(third, 3, [1]),
            Some(2),
            "a held seat reserves its old placement even with no live pawn"
        );
        assert_eq!(
            seats.assign_placement(first, 3, []),
            Some(0),
            "a fresh connection can recover the placement through its durable seat"
        );

        seats.clear_placements_for_level_unload();
        assert_eq!(
            seats.assign_placement(first, 3, [0, 1]),
            Some(2),
            "level unload clears assignments but preserves the cursor and scans occupancy"
        );
    }
}
