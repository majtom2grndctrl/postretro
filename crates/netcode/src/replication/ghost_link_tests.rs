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

// A script's `setColorFilter` rides the next record with the ramp the host
// component holds: only the fields it named, the unclamped from-value and the
// host script start time. Fields no script set never reach the wire.
#[test]
fn ghost_link_record_carries_script_set_look_ramps_as_the_host_holds_them() {
    use postretro_foundation::{GhostColorFilterTargets, GhostLook};
    use postretro_net::wire::{GHOST_LOOK_OPACITY, GHOST_LOOK_SATURATION};

    let mut registry = EntityRegistry::new();
    let link = spawn_link(&mut registry, 0, true, 0b1111);
    let mut allocator = NetworkIdAllocator::new();
    let mut set = ReplicableSet::new();
    host_register_loaded_ghost_links(&registry, &mut allocator, &mut set, &mut HashSet::new());

    let mut component = *registry.get_component::<GhostLinkComponent>(link).unwrap();
    // Mid-ramp, the unclamped value at the applying tick is the from-value.
    let mid_ramp = GhostLook {
        tint: [1.0, 1.0, 1.0],
        opacity: 0.73,
        saturation: 0.1,
    };
    component.filter.set(
        mid_ramp,
        &GhostColorFilterTargets {
            tint: None,
            opacity: Some(0.2),
            saturation: Some(0.9),
            period_ms: 400.0,
        },
        8.5,
    );
    registry.set_component(link, component).unwrap();

    let [state] = link_payloads(&registry, &set, &mut allocator)[..] else {
        panic!("one link, one record");
    };
    assert_eq!(state.validate(), Ok(()));
    assert_eq!(
        state.look_fields,
        GHOST_LOOK_OPACITY | GHOST_LOOK_SATURATION
    );
    assert!(state.tint.is_none(), "tint stays local: no script set it");
    let opacity = state.opacity.unwrap();
    assert_eq!(
        (
            opacity.from,
            opacity.to,
            opacity.start_time,
            opacity.duration_ms
        ),
        (0.73, 0.2, 8.5, 400.0)
    );
    assert_eq!(
        crate::ghost_color_filter_from_wire(&state),
        component.filter
    );
}
