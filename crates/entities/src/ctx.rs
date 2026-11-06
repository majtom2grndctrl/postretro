// Script context handle.
// See: context/lib/scripting.md
//
// `ScriptCtx` captures-by-Rc into every primitive closure at registration time.
// `!Send + !Sync` by design: the frame loop is single-threaded and both
// runtimes are `!Send`. `Rc<RefCell<_>>` over `Arc<RwLock<_>>` because
// `RwLock` poisons on panic — every primitive runs inside `catch_unwind`,
// so a poisoned lock would wedge the scripting surface after the first caught
// panic. `RefCell` has no poisoning.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use crate::data_registry::{DataRegistry, FactionSentimentState};
use crate::reactions::system_commands::{SystemCommandQueue, WeakSystemCommandQueue};
use crate::registry::EntityRegistry;
use crate::slot_table::SlotTable;

pub type PresentationNetworkedPredicate = fn(&EntityRegistry, crate::EntityId) -> bool;
pub type PresentationOverrideValidator =
    fn(&crate::presentation_overrides::PresentationOverride) -> bool;

/// Handle every primitive closure captures. Cloning is cheap — it bumps shared
/// `Rc`s. The fields behind `Rc<RefCell<_>>` are the subsystems the scripting
/// surface is allowed to touch. Extend by adding one field per subsystem.
#[derive(Clone)]
pub struct ScriptCtx {
    pub registry: Rc<RefCell<EntityRegistry>>,
    /// Engine-wide entity-type registry. Populated by the boot caller from
    /// `ModManifest.entities` after `run_mod_init` returns;
    /// consumed by the level-load spawn sweep.
    /// Survives level unload — entity-type descriptors are global, not
    /// per-level. `App` reaches this registry via `script_ctx.data_registry`;
    /// no separate handle is held on `App`.
    pub data_registry: Rc<RefCell<DataRegistry>>,
    /// Sparse host-authoritative sentiment that diverges from the immutable
    /// faction registry. This is session state, so replacing faction content
    /// during staged mod-init or a level change must not reseed it; replacement
    /// only removes values equal to the refreshed baseline.
    pub faction_sentiment: Rc<RefCell<FactionSentimentState>>,
    /// Engine-global typed state slots. Populated during mod init and retained
    /// until process exit; production level-clear paths never touch it.
    pub slot_table: Rc<RefCell<SlotTable>>,
    /// Engine frame counter, incremented once at the start of the Game logic
    /// phase. No current consumers — reserved for future primitives that need
    /// a per-frame ordering stamp.
    pub frame: Rc<Cell<u64>>,
    /// Runtime world gravity in m/s² (negative = downward; default -20.32).
    /// Sole owner — `App` does not hold a parallel handle. Seeded from the
    /// level default gravity (worldspawn `initialGravity`, carried in the
    /// EnvironmentVolumes PRL header) on every level load via
    /// `self.script_ctx.gravity.set(...)`, mutated by the `worldSetGravity`
    /// primitive through the captured `ScriptCtx` clone, and read each frame
    /// by `App` (`script_ctx.gravity.get()`) as the particle pool environment resolver
    /// default outside authored gravity volumes
    /// for buoyancy integration. The `Cell` lets the primitive closures mutate
    /// without a `&mut ScriptCtx` borrow.
    pub gravity: Rc<Cell<f32>>,
    /// Whether this process owns authoritative per-seat state mutations.
    /// Connected clients still compose reaction descriptors so they can render
    /// presentation work, but owner-slot additions must remain host-only.
    /// Also the host/single-player role gate for every npc/player group
    /// command (`group_resolution::group_commands_apply_here` reuses it).
    pub owner_slot_writes_enabled: Rc<Cell<bool>>,
    /// System-reaction command queue (M13 HUD dynamics). System reactions
    /// (`Primitive` descriptors with no `tag`) push typed commands here; `App`
    /// drains it once per frame after the post-tick event drains and routes
    /// each command to its subsystem consumer. The shared handle keeps engine
    /// services (audio/input/UI) out of the scripting surface — the queue is
    /// the seam. See: context/lib/scripting.md §10.4.
    pub system_commands: SystemCommandQueue,
    /// Ordered host-only teleporter changes, consumed at the next fixed tick.
    pub pending_teleporter_mutations: Rc<RefCell<Vec<(crate::EntityId, bool)>>>,
    /// Last request per pawn, drained at the next authoritative tick start.
    pub pending_respawns:
        Rc<RefCell<std::collections::BTreeMap<crate::EntityId, crate::RespawnRequest>>>,
    pub respawn_placements: Rc<RefCell<crate::RespawnPlacements>>,
    /// Ordered host-only ghost-link changes, consumed at the next fixed tick.
    pub pending_ghost_link_commands:
        Rc<RefCell<Vec<(crate::EntityId, postretro_foundation::GhostLinkCommand)>>>,
    /// Written state is independent of components rewritten by bridge sampling.
    pub presentation_overrides: Rc<RefCell<crate::presentation_overrides::PresentationOverrides>>,
    /// The level-relative clock the bridges will sample in this frame.
    pub presentation_script_time: Rc<Cell<f64>>,
    /// Injected by composition; scripting-core never depends upward on netcode.
    pub presentation_networked_predicate: Rc<Cell<Option<PresentationNetworkedPredicate>>>,
    pub presentation_override_validator: Rc<Cell<Option<PresentationOverrideValidator>>>,
    /// The `wait` landing currently draining, if any (`reactions::machine_local`).
    pub landing: Rc<crate::reactions::machine_local::LandingScope>,
}

impl ScriptCtx {
    pub fn new() -> Self {
        Self {
            registry: Rc::new(RefCell::new(EntityRegistry::new())),
            data_registry: Rc::new(RefCell::new(DataRegistry::new())),
            faction_sentiment: Rc::new(RefCell::new(FactionSentimentState::default())),
            slot_table: Rc::new(RefCell::new(SlotTable::new())),
            frame: Rc::new(Cell::new(0)),
            // Seeded to NaN so any code path that constructs `ScriptCtx`
            // without going through the level-install startup lifecycle
            // surfaces immediately. That lifecycle seeds from
            // the EnvironmentVolumes header gravity before the first frame. NaN
            // propagates through `particle_sim::tick` and is visually
            // obvious. The `worldSetGravity` primitive rejects non-finite
            // writes, so scripts cannot reintroduce this sentinel.
            gravity: Rc::new(Cell::new(f32::NAN)),
            owner_slot_writes_enabled: Rc::new(Cell::new(true)),
            system_commands: SystemCommandQueue::new(),
            pending_teleporter_mutations: Rc::new(RefCell::new(Vec::new())),
            pending_respawns: Rc::new(RefCell::new(Default::default())),
            respawn_placements: Rc::new(RefCell::new(Default::default())),
            pending_ghost_link_commands: Rc::new(RefCell::new(Vec::new())),
            presentation_overrides: Rc::new(RefCell::new(Default::default())),
            presentation_script_time: Rc::new(Cell::new(0.0)),
            presentation_networked_predicate: Rc::new(Cell::new(None)),
            presentation_override_validator: Rc::new(Cell::new(None)),
            landing: Rc::default(),
        }
    }
}

impl Default for ScriptCtx {
    fn default() -> Self {
        Self::new()
    }
}

/// A [`ScriptCtx`] that does not keep the context alive, for a holder the
/// context itself owns (the system queue's state sink). Holding a strong
/// clone there would be a reference cycle that leaks the whole context.
#[derive(Clone)]
pub struct WeakScriptCtx {
    registry: Weak<RefCell<EntityRegistry>>,
    data_registry: Weak<RefCell<DataRegistry>>,
    faction_sentiment: Weak<RefCell<FactionSentimentState>>,
    slot_table: Weak<RefCell<SlotTable>>,
    frame: Weak<Cell<u64>>,
    gravity: Weak<Cell<f32>>,
    owner_slot_writes_enabled: Weak<Cell<bool>>,
    system_commands: WeakSystemCommandQueue,
    pending_teleporter_mutations: Weak<RefCell<Vec<(crate::EntityId, bool)>>>,
    pending_respawns:
        Weak<RefCell<std::collections::BTreeMap<crate::EntityId, crate::RespawnRequest>>>,
    respawn_placements: Weak<RefCell<crate::RespawnPlacements>>,
    pending_ghost_link_commands:
        Weak<RefCell<Vec<(crate::EntityId, postretro_foundation::GhostLinkCommand)>>>,
    presentation_overrides: Weak<RefCell<crate::presentation_overrides::PresentationOverrides>>,
    presentation_script_time: Weak<Cell<f64>>,
    presentation_networked_predicate: Weak<Cell<Option<PresentationNetworkedPredicate>>>,
    presentation_override_validator: Weak<Cell<Option<PresentationOverrideValidator>>>,
    landing: Weak<crate::reactions::machine_local::LandingScope>,
}

impl ScriptCtx {
    pub fn downgrade(&self) -> WeakScriptCtx {
        WeakScriptCtx {
            registry: Rc::downgrade(&self.registry),
            data_registry: Rc::downgrade(&self.data_registry),
            faction_sentiment: Rc::downgrade(&self.faction_sentiment),
            slot_table: Rc::downgrade(&self.slot_table),
            frame: Rc::downgrade(&self.frame),
            gravity: Rc::downgrade(&self.gravity),
            owner_slot_writes_enabled: Rc::downgrade(&self.owner_slot_writes_enabled),
            system_commands: self.system_commands.downgrade(),
            pending_teleporter_mutations: Rc::downgrade(&self.pending_teleporter_mutations),
            pending_respawns: Rc::downgrade(&self.pending_respawns),
            respawn_placements: Rc::downgrade(&self.respawn_placements),
            pending_ghost_link_commands: Rc::downgrade(&self.pending_ghost_link_commands),
            presentation_overrides: Rc::downgrade(&self.presentation_overrides),
            presentation_script_time: Rc::downgrade(&self.presentation_script_time),
            presentation_networked_predicate: Rc::downgrade(&self.presentation_networked_predicate),
            presentation_override_validator: Rc::downgrade(&self.presentation_override_validator),
            landing: Rc::downgrade(&self.landing),
        }
    }
}

impl WeakScriptCtx {
    /// The context, or `None` once every owning handle has dropped.
    pub fn upgrade(&self) -> Option<ScriptCtx> {
        Some(ScriptCtx {
            registry: self.registry.upgrade()?,
            data_registry: self.data_registry.upgrade()?,
            faction_sentiment: self.faction_sentiment.upgrade()?,
            slot_table: self.slot_table.upgrade()?,
            frame: self.frame.upgrade()?,
            gravity: self.gravity.upgrade()?,
            owner_slot_writes_enabled: self.owner_slot_writes_enabled.upgrade()?,
            system_commands: self.system_commands.upgrade()?,
            pending_teleporter_mutations: self.pending_teleporter_mutations.upgrade()?,
            pending_respawns: self.pending_respawns.upgrade()?,
            respawn_placements: self.respawn_placements.upgrade()?,
            pending_ghost_link_commands: self.pending_ghost_link_commands.upgrade()?,
            presentation_overrides: self.presentation_overrides.upgrade()?,
            presentation_script_time: self.presentation_script_time.upgrade()?,
            presentation_networked_predicate: self.presentation_networked_predicate.upgrade()?,
            presentation_override_validator: self.presentation_override_validator.upgrade()?,
            landing: self.landing.upgrade()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slot_table::{
        NumericRange, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };

    fn health_slot() -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(100.0)),
            range: Some(NumericRange {
                min: 0.0,
                max: 100.0,
            }),
            persist: false,
            readonly: true,
            ownership: SlotOwnership::Engine,
            network: crate::slot_table::ReplicationScope::None,
            per_owner: false,
            accumulate: None,
        })
    }

    #[test]
    fn cloned_contexts_share_slot_table_access() {
        let ctx = ScriptCtx::new();
        let clone = ctx.clone();

        ctx.slot_table
            .borrow_mut()
            .insert("test.health".to_string(), health_slot())
            .expect("slot should be vacant");

        assert_eq!(
            clone
                .slot_table
                .borrow()
                .get("test.health")
                .and_then(|slot| slot.value.as_ref()),
            Some(&SlotValue::Number(100.0))
        );

        clone
            .slot_table
            .borrow_mut()
            .get_mut("test.health")
            .expect("slot should exist")
            .value = Some(SlotValue::Number(75.0));

        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("test.health")
                .and_then(|slot| slot.value.as_ref()),
            Some(&SlotValue::Number(75.0))
        );
    }

    #[test]
    fn data_registry_clear_leaves_slot_table_untouched() {
        let ctx = ScriptCtx::new();
        let builtin_slot_count = ctx.slot_table.borrow().len();
        ctx.slot_table
            .borrow_mut()
            .insert("test.health".to_string(), health_slot())
            .expect("slot should be vacant");

        ctx.data_registry.borrow_mut().clear();

        let slots = ctx.slot_table.borrow();
        assert_eq!(slots.len(), builtin_slot_count + 1);
        assert!(slots.get("test.health").is_some());
    }

    #[test]
    fn faction_sentiment_survives_faction_content_replacement() {
        let ctx = ScriptCtx::new();
        ctx.faction_sentiment
            .borrow_mut()
            .set(2.0, 3.0, 0.5, -1.0)
            .unwrap();

        ctx.data_registry.borrow_mut().replace_factions(
            crate::data_registry::FactionRegistry::default(),
            &mut ctx.faction_sentiment.borrow_mut(),
        );

        assert_eq!(ctx.faction_sentiment.borrow().get(2.0, 3.0), Some(0.5));
    }
}
