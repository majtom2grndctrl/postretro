// Engine <-> wire conversion for ghost-link records: armed state, categories and script-set look ramps.
// See: context/lib/networking.md §Current contract

use postretro_entities::GhostLinkComponent;
use postretro_foundation::{GhostColorFilter, GhostScalarRamp, GhostTintRamp};
use postretro_net::wire::{
    GHOST_LOOK_OPACITY, GHOST_LOOK_SATURATION, GHOST_LOOK_TINT, WireGhostLinkState,
    WireGhostScalarRamp, WireGhostTintRamp,
};

/// The host's always-on record for one link. Only the look fields a script has
/// set ride, each with its ramp as the component holds it: the unclamped
/// from-value at the applying host tick and the host script time it starts at.
/// Style values and limiter state never reach the record.
#[must_use]
pub fn ghost_link_state_to_wire(link: &GhostLinkComponent) -> WireGhostLinkState {
    let filter = &link.filter;
    let mut look_fields = 0;
    let tint = filter.tint.map(|ramp| {
        look_fields |= GHOST_LOOK_TINT;
        WireGhostTintRamp {
            from: ramp.from,
            to: ramp.to,
            start_time: ramp.start_time,
            duration_ms: ramp.duration_ms,
        }
    });
    let mut scalar = |ramp: Option<GhostScalarRamp>, bit| {
        ramp.map(|ramp| {
            look_fields |= bit;
            WireGhostScalarRamp {
                from: ramp.from,
                to: ramp.to,
                start_time: ramp.start_time,
                duration_ms: ramp.duration_ms,
            }
        })
    };
    let opacity = scalar(filter.opacity, GHOST_LOOK_OPACITY);
    let saturation = scalar(filter.saturation, GHOST_LOOK_SATURATION);
    WireGhostLinkState {
        look_fields,
        tint,
        opacity,
        saturation,
        ..WireGhostLinkState::new(link.index, link.source_entity, link.armed, link.members)
    }
}

/// The script-set look fields a validated record carries, still on the host's
/// script clock: the receiving client rebases each start time before it
/// presents ([`WireGhostLinkState::validate`] has rejected a mask/slot mismatch,
/// so the slots alone say which fields are set).
#[must_use]
pub fn ghost_color_filter_from_wire(state: &WireGhostLinkState) -> GhostColorFilter {
    let scalar = |ramp: Option<WireGhostScalarRamp>| {
        ramp.map(|ramp| GhostScalarRamp {
            from: ramp.from,
            to: ramp.to,
            start_time: ramp.start_time,
            duration_ms: ramp.duration_ms,
        })
    };
    GhostColorFilter {
        tint: state.tint.map(|ramp| GhostTintRamp {
            from: ramp.from,
            to: ramp.to,
            start_time: ramp.start_time,
            duration_ms: ramp.duration_ms,
        }),
        opacity: scalar(state.opacity),
        saturation: scalar(state.saturation),
    }
}

#[cfg(test)]
mod tests {
    use postretro_foundation::{GhostColorFilterTargets, GhostLook};
    use postretro_net::wire::{
        COMPONENT_KIND_GHOST_LINK, ComponentPayload, GHOST_LOOK_FIELD_BITS, RawComponentPayload,
    };

    use super::*;

    const STYLE: GhostLook = GhostLook {
        tint: [1.0, 1.0, 1.0],
        opacity: 0.5,
        saturation: 0.0,
    };

    fn link(filter: GhostColorFilter) -> GhostLinkComponent {
        GhostLinkComponent {
            index: 3,
            source_entity: 44,
            armed: true,
            members: 0b1011,
            filter,
        }
    }

    // Snapshot half, round trip: a link whose filter sets all three
    // fields encodes, survives the codec and validation, and applies back to
    // the same ramps, start times on the host clock included.
    #[test]
    fn ghost_link_look_ramps_round_trip_encode_decode_apply_with_every_field() {
        let mut filter = GhostColorFilter::default();
        filter.set(
            STYLE,
            &GhostColorFilterTargets {
                tint: Some([1.0, 0.4, 0.3]),
                opacity: Some(0.6),
                saturation: Some(0.25),
                period_ms: 1500.0,
            },
            12.75,
        );
        let wire = ghost_link_state_to_wire(&link(filter));
        assert_eq!(wire.look_fields, GHOST_LOOK_FIELD_BITS);
        // The payload envelope as it crosses the wire: one slot, its kind named.
        let raw = RawComponentPayload {
            teleporter: None,
            component_kind: COMPONENT_KIND_GHOST_LINK,
            transform: None,
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
            ghost_link: Some(wire),
        };
        let bytes = postretro_net::wire::encode(&raw);
        let decoded: RawComponentPayload = postretro_net::wire::decode(&bytes).unwrap();
        let Ok(ComponentPayload::GhostLinkState(decoded)) = decoded.validate() else {
            panic!("a ghost-link envelope validates as a ghost-link payload");
        };
        assert_eq!(
            (
                decoded.index,
                decoded.source_entity,
                decoded.armed,
                decoded.members
            ),
            (3, 44, true, 0b1011)
        );
        assert_eq!(ghost_color_filter_from_wire(&decoded), filter);
    }

    // Fields no script has set stay local: an opacity-only filter rides as
    // opacity alone, and a link no script touched carries an empty mask.
    #[test]
    fn ghost_link_record_carries_only_the_fields_a_script_set() {
        assert_eq!(
            ghost_link_state_to_wire(&link(GhostColorFilter::default())),
            WireGhostLinkState::new(3, 44, true, 0b1011)
        );
        let mut filter = GhostColorFilter::default();
        filter.set(
            STYLE,
            &GhostColorFilterTargets {
                tint: None,
                opacity: Some(0.9),
                saturation: None,
                period_ms: 0.0,
            },
            2.0,
        );
        let wire = ghost_link_state_to_wire(&link(filter));
        assert_eq!(wire.validate(), Ok(()));
        assert_eq!(wire.look_fields, GHOST_LOOK_OPACITY);
        assert!(wire.tint.is_none() && wire.saturation.is_none());
        let opacity = wire.opacity.unwrap();
        assert_eq!((opacity.from, opacity.to), (0.5, 0.9));
        assert_eq!(ghost_color_filter_from_wire(&wire), filter);
    }
}
