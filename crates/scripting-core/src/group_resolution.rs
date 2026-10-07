// Group-target resolution: the one chokepoint that turns a `{ kind, tag? }`
// group into the live entities it addresses on this machine, right now.
// See: context/lib/scripting.md §12 (Entity addressing)
//
// Named dispatch, scheduled sequence steps and trigger-tick commands all
// resolve groups here, so the kind rules and their ordering cannot drift
// between paths.

use postretro_entities::registry::{ComponentKind, EntityId, EntityRegistry};
use postretro_entities::{GroupKind, GroupTarget};

/// Resolve `target` against `registry` in registry slot order.
///
/// - `npc`: entities carrying a `BrainComponent`, excluding any entity bound to
///   a seat — a brained pawn is a player, never an NPC.
/// - `player`: pawns bound to a seat. A pawn whose seat is in a disconnect hold
///   has had its binding cleared (`SeatTable::hold_disconnected_client`), so it
///   resolves to nothing until reclaim rebinds it. The marked local pawn is
///   included even when no seat ledger bound it, so single player always
///   reaches its own pawn.
///
/// `tag`, when present, keeps only entities carrying it. Slot order is the
/// deterministic order: identical registry histories yield identical match
/// orders, including when a later spawn reuses a slot a despawn freed.
pub fn resolve_group(registry: &EntityRegistry, target: &GroupTarget) -> Vec<EntityId> {
    let tag = target.tag.as_deref();
    match target.kind {
        GroupKind::Npc => registry
            .query_by_component_and_tag(ComponentKind::Brain, tag)
            .map(|(id, _)| id)
            .filter(|id| registry.seat_for_pawn(*id).is_none())
            .collect(),
        GroupKind::Player => {
            let local_pawn = registry.local_player_pawn();
            registry
                .query_by_component_and_tag(ComponentKind::Transform, tag)
                .map(|(id, _)| id)
                .filter(|id| registry.seat_for_pawn(*id).is_some() || Some(*id) == local_pawn)
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::Transform;
    use postretro_entities::components::brain::BrainComponent;
    use postretro_foundation::Seat;

    fn brain() -> BrainComponent {
        BrainComponent::from_graph(&postretro_foundation::BehaviorGraphDescriptor {
            knockback: Default::default(),
            envelope: postretro_foundation::BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: std::collections::BTreeMap::from([(
                    "idle".to_string(),
                    postretro_foundation::BehaviorActivityDescriptor {
                        sound: None,
                        animation: None,
                        motion: Some(postretro_foundation::MotionVerb::Hold),
                        action: None,
                        on_enter: None,
                        layers: Default::default(),
                    },
                )]),
                transitions: Default::default(),
            },
            candidate_filter: None,
            retaliation: None,
            patrol: None,
            attacks: Default::default(),
            engagement_radius: None,
            move_speed: 1.0,
        })
    }

    fn spawn_npc(registry: &mut EntityRegistry, tags: &[&str]) -> EntityId {
        let tags: Vec<String> = tags.iter().map(|tag| tag.to_string()).collect();
        let id = registry.try_spawn(Transform::default(), &tags).unwrap();
        registry.set_component(id, brain()).unwrap();
        id
    }

    fn npcs(tag: Option<&str>) -> GroupTarget {
        GroupTarget {
            kind: GroupKind::Npc,
            tag: tag.map(str::to_string),
        }
    }

    fn players() -> GroupTarget {
        GroupTarget {
            kind: GroupKind::Player,
            tag: None,
        }
    }

    #[test]
    fn npc_group_excludes_a_seat_bound_brained_pawn_and_untagged_npcs_when_filtered() {
        let mut registry = EntityRegistry::new();
        let tagged = spawn_npc(&mut registry, &["x"]);
        let untagged = spawn_npc(&mut registry, &[]);
        let brained_pawn = spawn_npc(&mut registry, &["x"]);
        registry.bind_pawn_seat(brained_pawn, Seat(1));
        let prop = registry
            .try_spawn(Transform::default(), &["x".to_string()])
            .unwrap();

        assert_eq!(resolve_group(&registry, &npcs(Some("x"))), vec![tagged]);
        assert_eq!(
            resolve_group(&registry, &npcs(None)),
            vec![tagged, untagged],
            "a tagless npc group reaches every NPC; never a pawn or a brainless entity ({prop:?})"
        );
    }

    #[test]
    fn player_group_is_seat_bound_pawns_plus_an_unbound_local_pawn() {
        let mut registry = EntityRegistry::new();
        let remote = registry.spawn(Transform::default());
        let held = registry.spawn(Transform::default());
        let bystander = registry.spawn(Transform::default());
        registry.bind_pawn_seat(remote, Seat(1));
        registry.bind_pawn_seat(held, Seat(2));
        // A disconnect hold clears the held seat's pawn binding.
        registry.clear_pawn_seat(held);
        assert_eq!(resolve_group(&registry, &players()), vec![remote]);

        let local = registry.spawn(Transform::default());
        registry.mark_local_player_pawn(local).unwrap();
        assert_eq!(
            resolve_group(&registry, &players()),
            vec![remote, local],
            "single player reaches the local pawn even with no seat ledger ({bystander:?} is no player)"
        );
    }

    // G4: deterministic order, including a spawn that reuses a freed slot.
    #[test]
    fn group_resolution_order_is_slot_order_and_repeats_across_identical_histories() {
        fn history() -> (EntityRegistry, Vec<EntityId>) {
            let mut registry = EntityRegistry::new();
            let a = spawn_npc(&mut registry, &["x"]);
            let b = spawn_npc(&mut registry, &["x"]);
            let c = spawn_npc(&mut registry, &["x"]);
            registry.despawn(a).unwrap();
            // Reuses `a`'s freed slot, so it resolves ahead of `b` and `c`.
            let d = spawn_npc(&mut registry, &["x"]);
            (registry, vec![d, b, c])
        }
        let (first, expected) = history();
        let (second, _) = history();
        assert_eq!(resolve_group(&first, &npcs(Some("x"))), expected);
        assert_eq!(
            resolve_group(&first, &npcs(Some("x"))),
            resolve_group(&second, &npcs(Some("x"))),
        );
    }
}
