//! Engine-owned map-authored attack pool: its authored profile and member tag, and its run state.
//! See: context/lib/entity_model.md §7c (Attack budget, Attack pools)

use serde::{Deserialize, Serialize};

use crate::registry::EntityId;

/// One request on the attack pools, applied in issue order by the drain that
/// runs immediately before the AI tick. A spawn's join rides the same list so
/// it orders against the starts and stops issued beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackPoolRequest {
    /// Start `pool`, or re-start it when running: assign its current tag
    /// holders and rank it latest.
    Start(EntityId),
    /// Stop `pool`: its members return to the default pool. A no-op when
    /// stopped.
    Stop(EntityId),
    /// A spawned enemy joins the latest-started running pool whose member tag
    /// it carries, if any.
    Join(EntityId),
}

/// A map-placed `ai_attack_pool`. Host-owned and never replicated. Its
/// running state, run count and start rank serialize, so a restored level
/// keeps its pools and their order without replaying any start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttackPoolComponent {
    /// The tag whose holders a start assigns, and whose spawns join while the
    /// pool runs.
    pub member_tag: String,
    /// The mod budget profile the pool's members arbitrate under. Empty means
    /// `default`.
    pub budget_profile: String,
    #[serde(default)]
    pub running: bool,
    /// Counts the pool's starts from stopped. A start on a running pool keeps
    /// it, so the pool's refreshing counts stand; a new run begins open.
    #[serde(default)]
    pub run: u64,
    /// The pool's place in start order: each start or re-start takes one past
    /// every pool's rank. A spawn matching two running pools joins the higher.
    #[serde(default)]
    pub start_rank: u64,
}

impl AttackPoolComponent {
    /// A stopped pool, as a level installs it.
    pub fn new(member_tag: String, budget_profile: String) -> Self {
        Self {
            member_tag,
            budget_profile,
            running: false,
            run: 0,
            start_rank: 0,
        }
    }
}
