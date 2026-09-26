// Where a gameplay event happened: emitter identity and contact data for
// presentation. Sound anchors and `playSound`'s `at` resolve from this; the
// simulation never names an audio type.
// See: context/lib/audio.md §4 · context/lib/scripting.md §12

use std::borrow::Cow;
use std::sync::Arc;

use glam::Vec3;

use crate::data_descriptors::BehaviorGraphDescriptor;
use crate::registry::EntityId;

/// The emitter of one named gameplay event.
#[derive(Debug, Clone, PartialEq)]
pub enum Emitter {
    /// An entity. `origin` is its transform position when the event fired, so
    /// an emitter removed before presentation resolves still has a position.
    Entity { id: EntityId, origin: Vec3 },
    /// Every contact of one activation on one tick.
    Contacts(Vec<ImpactContact>),
}

/// One weapon contact: where it landed, the surface normal there, and what it
/// hit. Carried whole so a later surface-material pass needs no new plumbing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpactContact {
    pub point: Vec3,
    pub normal: Vec3,
    pub hit: ContactHit,
}

/// What an impact contact struck.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactHit {
    Entity(EntityId),
    World,
}

impl ImpactContact {
    /// A contact on `target`, or on world geometry when `target` is `None`.
    pub fn new(point: Vec3, normal: Vec3, target: Option<EntityId>) -> Self {
        Self {
            point,
            normal,
            hit: target.map_or(ContactHit::World, ContactHit::Entity),
        }
    }
}

// Named gameplay events paired with their emitters. The simulation hands these
// to the app drain, which fires each address and resolves each sound from the
// emitter and the descriptor identity it carries.

/// A player-movement event (`landed`, `jumped`, state edges) on its pawn.
#[derive(Debug, Clone, PartialEq)]
pub struct MovementEmission {
    pub address: &'static str,
    pub emitter: Emitter,
}

/// A weapon event: fire, dry fire, spawn, or impact. `weapon` is the canonical
/// name of the weapon descriptor the event came from, whoever wielded it.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponEmission {
    pub address: &'static str,
    pub emitter: Emitter,
    pub weapon: Option<String>,
}

/// An enemy event on the enemy. `address` is absent for an entered activity
/// that authors no `on_enter`: it still has an emission, because its sound
/// plays on entry regardless.
#[derive(Debug, Clone, PartialEq)]
pub struct AiEmission {
    pub address: Option<Cow<'static, str>>,
    pub emitter: Emitter,
    pub cue: AiCue,
}

/// Which descriptor an enemy event came from. The graph is the one the brain
/// held when the event fired, so a later reseat never re-points it.
#[derive(Debug, Clone, PartialEq)]
pub enum AiCue {
    /// The named attack fired.
    Attack {
        graph: Arc<BehaviorGraphDescriptor>,
        attack: String,
    },
    /// The brain entered the activity at root-to-leaf `path` in `graph`.
    Entered {
        graph: Arc<BehaviorGraphDescriptor>,
        path: Vec<usize>,
    },
}
