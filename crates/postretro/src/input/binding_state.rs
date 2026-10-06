// The session's binding layers and the effective table built from them.
// See: context/lib/input.md §2 (layering)

use super::binding_table::{AuthorLayer, EffectiveTable, PlayerLayer};
use super::relevance::RelevanceFacts;
use super::system::InputSystem;

/// What the effective table was last built from. The table rebuilds at mod
/// init, on hot reload (entity types change), on rebind or swap (the layers
/// change), and when co-op host tuning installs or clears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingSources {
    pub entity_types_generation: u64,
    /// The installed host tuning's generation while participating, else
    /// `None`. A demote clears tuning without bumping its generation, so the
    /// presence is part of the key.
    pub tuning: Option<u64>,
}

#[derive(Debug, Default)]
pub struct BindingState {
    author: AuthorLayer,
    player: PlayerLayer,
    swap_confirm_cancel: bool,
    /// Bumped on every layer change so the next refresh rebuilds.
    layers_revision: u64,
    table: EffectiveTable,
    built_from: Option<(BindingSources, u64)>,
}

impl BindingState {
    pub fn needs_rebuild(&self, sources: BindingSources) -> bool {
        self.built_from != Some((sources, self.layers_revision))
    }

    /// Rebuild the effective table and hand its gameplay bindings to the
    /// input system. Input state and preferences survive (P4).
    pub fn rebuild(
        &mut self,
        sources: BindingSources,
        facts: RelevanceFacts,
        input: &mut InputSystem,
    ) {
        self.table =
            EffectiveTable::build(&self.author, &self.player, facts, self.swap_confirm_cancel);
        input.set_bindings(self.table.gameplay_bindings());
        self.built_from = Some((sources, self.layers_revision));
    }

    #[allow(dead_code)]
    pub fn table(&self) -> &EffectiveTable {
        &self.table
    }

    #[allow(dead_code)]
    pub fn set_author_layer(&mut self, author: AuthorLayer) {
        if self.author != author {
            self.author = author;
            self.layers_revision += 1;
        }
    }

    #[allow(dead_code)]
    pub fn set_player_layer(&mut self, player: PlayerLayer) {
        if self.player != player {
            self.player = player;
            self.layers_revision += 1;
        }
    }

    #[allow(dead_code)]
    pub fn set_swap_confirm_cancel(&mut self, swap: bool) {
        if self.swap_confirm_cancel != swap {
            self.swap_confirm_cancel = swap;
            self.layers_revision += 1;
        }
    }
}
