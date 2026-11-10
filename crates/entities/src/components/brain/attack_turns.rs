// Per-brain attack-turn storage: held turns, the pending request and the wait
// clock. Storage only; `postretro-ai` owns every rule that reads or writes it.
// See: context/lib/entity_model.md §7c (Attack budget)

use crate::registry::EntityId;

/// Most tokens one attack may draw. Draw lists are stored inline so a brain
/// clone never allocates; load rejects an attack drawing more.
pub const MAX_ATTACK_DRAWS: usize = postretro_foundation::attack_budget::MAX_ATTACK_DRAWS;

/// Most turns with draws one enemy holds at once. No two of its turns share a
/// token, so this bounds the distinct tokens one enemy can have in flight; a
/// request past it waits until a held turn ends.
pub const MAX_HELD_TURNS: usize = 4;

/// Most undrawn turns one enemy holds at once, stored apart so an undrawn
/// turn never takes a drawn turn's place. An undrawn attack past it still
/// fires; it simply holds no turn.
pub const MAX_UNDRAWN_TURNS: usize = 4;

/// A registered attack token, by its index in the mod's token table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttackTokenId(pub u16);

/// One token an attack draws and how much of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedDraw {
    pub token: AttackTokenId,
    pub cost: u32,
}

/// An attack's resolved draw list, inline. Empty opts out of the budget: the
/// attack is always granted and never counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedDraws {
    draws: [ResolvedDraw; MAX_ATTACK_DRAWS],
    len: u8,
}

impl ResolvedDraws {
    pub const NONE: Self = Self {
        draws: [ResolvedDraw {
            token: AttackTokenId(0),
            cost: 0,
        }; MAX_ATTACK_DRAWS],
        len: 0,
    };

    /// `None` when `draws` exceeds [`MAX_ATTACK_DRAWS`].
    pub fn from_slice(draws: &[ResolvedDraw]) -> Option<Self> {
        if draws.len() > MAX_ATTACK_DRAWS {
            return None;
        }
        let mut resolved = Self::NONE;
        resolved.draws[..draws.len()].copy_from_slice(draws);
        resolved.len = draws.len() as u8;
        Some(resolved)
    }

    pub fn as_slice(&self) -> &[ResolvedDraw] {
        &self.draws[..self.len as usize]
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether the two lists draw the same tokens at the same costs, in any
    /// order: a reload that only reorders `draws` changes nothing.
    pub fn same_draws(&self, other: &Self) -> bool {
        let (ours, theirs) = (self.as_slice(), other.as_slice());
        let count = |list: &[ResolvedDraw], draw: &ResolvedDraw| {
            list.iter().filter(|entry| *entry == draw).count()
        };
        ours.len() == theirs.len()
            && ours
                .iter()
                .all(|draw| count(ours, draw) == count(theirs, draw))
    }

    /// Whether the two lists draw any token in common.
    pub fn shares_token(&self, other: &Self) -> bool {
        self.as_slice()
            .iter()
            .any(|draw| other.as_slice().iter().any(|o| o.token == draw.token))
    }
}

/// The attack pool a turn counts in: the default pool, or a running
/// `ai_attack_pool` entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AttackPoolId {
    #[default]
    Default,
    Pool(EntityId),
}

/// A turn one enemy holds for one of its attacks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeldTurn {
    /// The attack's index in the brain graph's `attacks` (name order).
    pub attack: u16,
    pub target: EntityId,
    /// The pool the turn counts in now: its holder's current pool.
    pub pool: AttackPoolId,
    /// The draws resolved at grant.
    pub draws: ResolvedDraws,
    /// Granted with no mark on the path: holds until the attack's cooldown
    /// ends. A marked grant holds while a mark for the attack is on the path.
    pub until_cooldown: bool,
}

/// The one turn an enemy asked for on its latest tick, while ungranted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnRequest {
    pub attack: u16,
    pub draws: ResolvedDraws,
    /// Requested by a marked activity rather than a ready fire step.
    pub marked: bool,
    pub target: EntityId,
    pub pool: AttackPoolId,
}

/// When an enemy's current wait began: its first request after last holding
/// a turn, on `target`. A gap in requesting keeps it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaitClock {
    pub since_tick: u64,
    pub target: EntityId,
}

/// Every attack-turn fact one brain carries. Transient: a restored brain
/// holds no turn and waits on no clock.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AttackTurnState {
    held: [Option<HeldTurn>; MAX_HELD_TURNS + MAX_UNDRAWN_TURNS],
    pub request: Option<TurnRequest>,
    pub wait: Option<WaitClock>,
    /// The last AI tick this brain was evaluated on. A gap means it left the
    /// AI set, which drops every turn, request and clock.
    pub seen_tick: u64,
}

impl AttackTurnState {
    pub fn held(&self) -> impl Iterator<Item = &HeldTurn> {
        self.held.iter().flatten()
    }

    pub fn holds_any(&self) -> bool {
        self.held.iter().any(Option::is_some)
    }

    pub fn holds_attack(&self, attack: u16) -> bool {
        self.held().any(|turn| turn.attack == attack)
    }

    /// Whether a turn drawing `draws` fits beside the turns held now. Drawn
    /// and undrawn turns each have their own capacity.
    pub fn has_room_for(&self, draws: &ResolvedDraws) -> bool {
        let undrawn = draws.is_empty();
        let capacity = if undrawn {
            MAX_UNDRAWN_TURNS
        } else {
            MAX_HELD_TURNS
        };
        self.held()
            .filter(|turn| turn.draws.is_empty() == undrawn)
            .count()
            < capacity
    }

    /// Store a granted turn; `false` when its capacity is taken.
    pub fn hold(&mut self, turn: HeldTurn) -> bool {
        if !self.has_room_for(&turn.draws) {
            return false;
        }
        match self.held.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => {
                *slot = Some(turn);
                true
            }
            None => false,
        }
    }

    /// Keep each held turn for which `keep` returns true.
    pub fn retain_held(&mut self, mut keep: impl FnMut(&mut HeldTurn) -> bool) {
        for slot in &mut self.held {
            if let Some(turn) = slot
                && !keep(turn)
            {
                *slot = None;
            }
        }
    }

    /// Drop every turn, the request and the wait clock.
    pub fn clear(&mut self) {
        let seen_tick = self.seen_tick;
        *self = Self {
            seen_tick,
            ..Self::default()
        };
    }
}
