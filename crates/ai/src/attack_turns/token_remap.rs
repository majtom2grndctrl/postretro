// Token ids across a vocabulary commit. Ids are indexes into the registered
// list, so a reload that inserts or removes a token shifts them; everything
// keyed by id moves to the new ids by token name.
// See: context/lib/entity_model.md §7c (Attack budget)

use postretro_entities::components::brain::{
    AttackTokenId, AttackTurnState, MAX_ATTACK_DRAWS, ResolvedDraw, ResolvedDraws,
};
use postretro_foundation::attack_budget::AttackBudgetVocabulary;

/// Old token id → new token id, `None` where the new vocabulary no longer
/// registers the token.
pub(crate) struct TokenRemap {
    ids: Vec<Option<AttackTokenId>>,
}

impl TokenRemap {
    /// `None` when every old id keeps its meaning.
    pub(crate) fn between(
        old: &AttackBudgetVocabulary,
        new: &AttackBudgetVocabulary,
    ) -> Option<Self> {
        let ids: Vec<Option<AttackTokenId>> = old
            .tokens()
            .iter()
            .map(|token| new.token_index(&token.name).map(AttackTokenId))
            .collect();
        let identity = ids
            .iter()
            .enumerate()
            .all(|(index, id)| *id == Some(AttackTokenId(index as u16)));
        (!identity).then_some(Self { ids })
    }

    pub(crate) fn token(&self, token: AttackTokenId) -> Option<AttackTokenId> {
        self.ids.get(token.0 as usize).copied().flatten()
    }

    pub(crate) fn draws(&self, draws: &ResolvedDraws) -> ResolvedDraws {
        let mut kept = [ResolvedDraw {
            token: AttackTokenId(0),
            cost: 0,
        }; MAX_ATTACK_DRAWS];
        let mut len = 0;
        for draw in draws.as_slice() {
            if let Some(token) = self.token(draw.token) {
                kept[len] = ResolvedDraw {
                    token,
                    cost: draw.cost,
                };
                len += 1;
            }
        }
        ResolvedDraws::from_slice(&kept[..len]).expect("a remap never adds draws")
    }

    /// One brain's held turns and request, in new ids.
    pub(crate) fn turns(&self, turns: &mut AttackTurnState) {
        turns.retain_held(|turn| {
            turn.draws = self.draws(&turn.draws);
            true
        });
        if let Some(request) = turns.request.as_mut() {
            request.draws = self.draws(&request.draws);
        }
    }
}
