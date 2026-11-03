//! Authority-only state remap after a committed descriptor snapshot.
//! See: context/lib/scripting.md §12.

use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{ComponentKind, ComponentValue, ScriptCtx};
use postretro_foundation::{
    UNDECLARED_LIFE_STATE, clear_life_state, enter_life_state, restamp_life_state,
};

pub(crate) fn remap_live_life_states(ctx: &ScriptCtx, previous_names: &[String]) {
    if !crate::group_resolution::group_commands_apply_here(ctx) {
        return;
    }
    let data = ctx.data_registry.borrow();
    let mut registry = ctx.registry.borrow_mut();
    let pawns: Vec<_> = registry
        .query_by_component_and_tag(ComponentKind::PlayerMovement, None)
        .map(|(id, _)| id)
        .collect();
    for pawn in pawns {
        let block = registry
            .get_component::<DescriptorProvenance>(pawn)
            .ok()
            .and_then(|provenance| {
                data.entities.iter().find(|descriptor| {
                    descriptor.canonical_name.as_deref() == Some(provenance.canonical_name.as_str())
                })
            })
            .and_then(|descriptor| descriptor.life_states.as_ref());
        let Ok(ComponentValue::PlayerMovement(movement)) =
            registry.get_component_value_mut(pawn, ComponentKind::PlayerMovement)
        else {
            continue;
        };
        let was_declared = movement.life_state != UNDECLARED_LIFE_STATE;
        let old_name = was_declared
            .then(|| previous_names.get(usize::from(movement.life_state)))
            .flatten();
        match block {
            Some(block) => {
                if let Some((id, profile)) = old_name.and_then(|name| block.state(name)) {
                    movement.life_state = id;
                    movement.life_state_table_revision = block.table_revision;
                    restamp_life_state(movement, Some(profile));
                } else {
                    if was_declared {
                        log::warn!(
                            "[LifeState] pawn {pawn:?} state `{}` removed by reload; entering initial",
                            old_name.map_or("<unknown>", String::as_str)
                        );
                    }
                    let (id, profile) = block.initial_state();
                    enter_life_state(movement, id, profile, block.table_revision);
                }
            }
            None if was_declared => {
                log::warn!(
                    "[LifeState] pawn {pawn:?} descriptor no longer declares life states; clearing state"
                );
                clear_life_state(movement, data.life_state_table_revision);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests;
