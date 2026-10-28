//! Load/reload-only lowering of authored names. No per-tick name-table work.

use std::collections::BTreeSet;

use crate::DataRegistry;

impl DataRegistry {
    /// Rebuild once after startup's descriptor drain, and on snapshot replacement.
    /// Manifest validation guarantees at most 255 distinct names (255 is reserved).
    pub fn rebuild_life_state_table(&mut self) {
        let names: Vec<String> = self
            .entities
            .iter()
            .filter_map(|descriptor| descriptor.life_states.as_ref())
            .flat_map(|block| block.states.keys().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        assert!(
            names.len() <= usize::from(u8::MAX),
            "validated life-state table limit"
        );
        if names != self.life_state_names {
            self.life_state_table_revision = self.life_state_table_revision.wrapping_add(1);
            self.life_state_names = names;
        }
        for block in self
            .entities
            .iter_mut()
            .filter_map(|descriptor| descriptor.life_states.as_mut())
        {
            block.state_ids = block
                .states
                .keys()
                .map(|name| {
                    let id = self
                        .life_state_names
                        .binary_search(name)
                        .expect("state in rebuilt table");
                    (name.clone(), id as u8)
                })
                .collect();
            block.table_revision = self.life_state_table_revision;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntityTypeDescriptor, LifeStatesDescriptor};
    use postretro_foundation::LifeStateProfile;

    fn descriptor(name: &str, initial: &str, names: &[&str]) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            life_states: Some(LifeStatesDescriptor {
                initial: initial.to_owned(),
                states: names
                    .iter()
                    .map(|name| ((*name).to_owned(), LifeStateProfile::default()))
                    .collect(),
                rejoin: Default::default(),
                state_ids: Default::default(),
                table_revision: 0,
            }),
            canonical_name: Some(name.to_owned()),
            ghost: false,
            faction: None,
            tolerance: None,
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    #[test]
    fn life_state_table_is_global_sorted_and_survives_level_clear() {
        let mut registry = DataRegistry::new();
        registry.upsert_entity_type(descriptor("first", "down", &["down", "alive"]));
        registry.upsert_entity_type(descriptor("second", "spectating", &["spectating", "alive"]));
        registry.rebuild_life_state_table();
        assert_eq!(registry.life_state_names, ["alive", "down", "spectating"]);
        assert_eq!(
            registry.entities[0]
                .life_states
                .as_ref()
                .unwrap()
                .initial_state()
                .0,
            1
        );
        assert_eq!(
            registry.entities[1]
                .life_states
                .as_ref()
                .unwrap()
                .initial_state()
                .0,
            2
        );
        let revision = registry.life_state_table_revision;
        registry.clear();
        assert_eq!(registry.life_state_names, ["alive", "down", "spectating"]);
        assert_eq!(registry.life_state_table_revision, revision);
    }

    #[test]
    fn replacement_relowers_every_descriptor_and_revises_only_changed_names() {
        let mut registry = DataRegistry::new();
        registry.replace_entity_types(vec![descriptor("player", "down", &["alive", "down"])]);
        let revision = registry.life_state_table_revision;
        let mut edited = descriptor("player", "down", &["alive", "down"]);
        edited
            .life_states
            .as_mut()
            .unwrap()
            .states
            .get_mut("down")
            .unwrap()
            .fire = false;
        registry.replace_entity_types(vec![edited]);
        assert_eq!(registry.life_state_table_revision, revision);
        assert!(
            !registry.entities[0]
                .life_states
                .as_ref()
                .unwrap()
                .profile(1)
                .unwrap()
                .fire
        );
        registry.replace_entity_types(vec![descriptor(
            "player",
            "down",
            &["ahead", "alive", "down"],
        )]);
        let block = registry.entities[0].life_states.as_ref().unwrap();
        assert_eq!(block.initial_state().0, 2);
        assert_eq!(block.table_revision, revision.wrapping_add(1));
        assert!(block.profile(255).is_none());
        assert!(block.state("removed").is_none());
    }

    #[test]
    fn life_state_profile_removal_keeps_revision_when_another_descriptor_keeps_the_name() {
        let mut registry = DataRegistry::new();
        registry.replace_entity_types(vec![
            descriptor("first", "down", &["alive", "down"]),
            descriptor("second", "down", &["down"]),
        ]);
        let revision = registry.life_state_table_revision;
        registry.replace_entity_types(vec![
            descriptor("first", "alive", &["alive"]),
            descriptor("second", "down", &["down"]),
        ]);
        assert_eq!(registry.life_state_table_revision, revision);
        assert!(
            registry.entities[0]
                .life_states
                .as_ref()
                .unwrap()
                .profile(1)
                .is_none()
        );
        assert!(
            registry.entities[1]
                .life_states
                .as_ref()
                .unwrap()
                .profile(1)
                .is_some()
        );
    }

    #[test]
    fn life_state_table_uses_ids_through_254_and_wraps_revision() {
        let names: Vec<String> = (0..255).map(|index| format!("state{index}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut registry = DataRegistry::new();
        registry.life_state_table_revision = u8::MAX;
        registry.replace_entity_types(vec![descriptor("player", "state0", &names)]);
        assert_eq!(registry.life_state_table_revision, 0);
        let block = registry.entities[0].life_states.as_ref().unwrap();
        assert_eq!(block.state_ids.values().copied().max(), Some(254));
        assert!(block.profile(255).is_none());
    }
}
