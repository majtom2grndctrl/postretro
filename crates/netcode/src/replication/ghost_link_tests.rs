// Host ghost-link registration and the always-on link-state record.
// See: context/lib/networking.md §Current contract
use std::collections::HashSet;

use postretro_entities::{GhostLinkComponent, Transform};
use postretro_net::wire::WireGhostLinkState;

use super::*;

fn spawn_link(registry: &mut EntityRegistry, index: u16, armed: bool, members: u8) -> EntityId {
    let link = registry.spawn(Transform::default());
    registry
        .set_component(
            link,
            GhostLinkComponent {
                index,
                source_entity: 40 + u32::from(index),
                armed,
                members,
                filter: Default::default(),
            },
        )
        .unwrap();
    link
}

fn link_payloads(
    registry: &EntityRegistry,
    set: &ReplicableSet,
    allocator: &mut NetworkIdAllocator,
) -> Vec<WireGhostLinkState> {
    let mut states: Vec<_> = produce_owned_snapshots(
        registry,
        set,
        allocator,
        &MovementOwners::new(),
        &HostCommandQueues::new(),
    )
    .into_iter()
    .flat_map(|snapshot| snapshot.components)
    .filter_map(|payload| match payload {
        ComponentPayload::GhostLinkState(state) => Some(state),
        _ => None,
    })
    .collect();
    states.sort_by_key(|state| state.index);
    states
}

// Every installed link registers at install, unlike bound presentation state
// that waits for a script write, so a joiner's baseline carries each link's
// current state. The record mirrors the component, with no look field set.
#[test]
fn ghost_links_register_at_install_and_replicate_their_current_state_always() {
    let mut registry = EntityRegistry::new();
    let first = spawn_link(&mut registry, 0, true, 0b1111);
    let second = spawn_link(&mut registry, 1, false, 0b0101);
    let mut allocator = NetworkIdAllocator::new();
    let mut set = ReplicableSet::new();
    let mut tracked = HashSet::new();
    host_register_loaded_ghost_links(&registry, &mut allocator, &mut set, &mut tracked);
    assert!(set.contains(first) && set.contains(second));
    assert_eq!(tracked, HashSet::from([first, second]));

    assert_eq!(
        link_payloads(&registry, &set, &mut allocator),
        [
            WireGhostLinkState::new(0, 40, true, 0b1111),
            WireGhostLinkState::new(1, 41, false, 0b0101),
        ]
    );
    for state in link_payloads(&registry, &set, &mut allocator) {
        assert_eq!(state.validate(), Ok(()));
        assert_eq!(state.look_fields, 0, "no script has set a look field");
    }

    // A host-side arm is the next record's state.
    let mut armed = *registry
        .get_component::<GhostLinkComponent>(second)
        .unwrap();
    armed.armed = true;
    registry.set_component(second, armed).unwrap();
    assert!(link_payloads(&registry, &set, &mut allocator)[1].armed);

    // Re-registration is idempotent and forgets links a reload removed.
    registry.despawn(first).unwrap();
    host_register_loaded_ghost_links(&registry, &mut allocator, &mut set, &mut tracked);
    assert!(!set.contains(first));
    assert_eq!(tracked, HashSet::from([second]));
}
