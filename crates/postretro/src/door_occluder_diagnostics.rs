//! App-side diagnostics for baked door-to-portal occluder associations.
//!
//! The renderer receives prepared rows and immediate-mode line segments only;
//! mover phase and the render-only blocked-portal buffer remain app-owned.

use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry, KinematicMoverComponent};
use postretro_level_loader::LevelWorld;

use crate::kinematic_mover::mover_is_docked_closed;
use crate::render::{self, Renderer};

const COLOR_BLOCKED_PORTAL: [u8; 4] = [255, 48, 196, 255];

#[derive(Default)]
pub(crate) struct DoorOccluderDiagnostics {
    pub mover_rows: Vec<render::DoorOccluderDiagnosticsRow>,
    pub blocked_portal_ids: Vec<u32>,
}

/// Snapshots baked associations and the current render-frame blocker set for
/// the Diagnostics panel. It deliberately does not derive or mutate blocking.
pub(crate) fn collect(
    registry: &EntityRegistry,
    blocked_portals: &[bool],
) -> DoorOccluderDiagnostics {
    let mover_rows = registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .filter_map(|(entity, value)| {
            let ComponentValue::KinematicMover(mover) = value else {
                return None;
            };
            Some(mover_diagnostics_row(entity.to_string(), mover))
        })
        .collect();
    let blocked_portal_ids = blocked_portals
        .iter()
        .enumerate()
        .filter_map(|(portal_id, &blocked)| blocked.then_some(portal_id as u32))
        .collect();

    DoorOccluderDiagnostics {
        mover_rows,
        blocked_portal_ids,
    }
}

fn mover_diagnostics_row(
    id: String,
    mover: &KinematicMoverComponent,
) -> render::DoorOccluderDiagnosticsRow {
    render::DoorOccluderDiagnosticsRow {
        id,
        mover_id: mover.mover_id,
        sealed_portal_ids: mover.sealed_portal_ids.clone(),
        docked_closed: mover_is_docked_closed(mover),
    }
}

/// Highlights every current blocked portal using the established overlay line
/// route. `blocked_portals` is the already-derived frame buffer; diagnostics
/// never recompute it or alter mover/gameplay state.
pub(crate) fn emit_blocked_portal_geometry(
    renderer: &mut Renderer,
    world: &LevelWorld,
    blocked_portals: &[bool],
) {
    for (portal_id, &blocked) in blocked_portals.iter().enumerate() {
        if !blocked {
            continue;
        }
        let Some(portal) = world.portals.get(portal_id) else {
            continue;
        };
        if portal.polygon.len() < 2 {
            continue;
        }
        for vertex_index in 0..portal.polygon.len() {
            let start = portal.polygon[vertex_index];
            let end = portal.polygon[(vertex_index + 1) % portal.polygon.len()];
            renderer.push_debug_line_overlay(start, end, COLOR_BLOCKED_PORTAL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_entities::{KinematicMoverConfig, KinematicMoverMode, Transform};

    fn closet_door() -> KinematicMoverComponent {
        let mut mover = KinematicMoverComponent::new(
            3,
            KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::X],
                waypoint_names: vec![
                    "closet_door_closed".to_string(),
                    "closet_door_open".to_string(),
                ],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::Once,
                started: false,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        mover.sealed_portal_ids = vec![0];
        mover
    }

    #[test]
    fn closet_door_diagnostics_report_closed_and_open_blocker_states() {
        let fixture = include_str!("../../../content/dev/maps/closet-reveal.map");
        assert!(fixture.contains("\"name\" \"closet_door\""));

        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        registry
            .set_component(entity, closet_door())
            .expect("closet mover installs");

        let closed = collect(&registry, &[true]);
        assert_eq!(closed.blocked_portal_ids, vec![0]);
        assert_eq!(closed.mover_rows.len(), 1);
        assert_eq!(closed.mover_rows[0].mover_id, 3);
        assert_eq!(closed.mover_rows[0].sealed_portal_ids, vec![0]);
        assert!(closed.mover_rows[0].docked_closed);

        let mut opening = registry
            .get_component::<KinematicMoverComponent>(entity)
            .expect("closet mover remains installed")
            .clone();
        opening.segment_elapsed_ms = 1.0;
        opening.was_active_this_tick = true;
        opening.current_linear_velocity = Vec3::X;
        registry
            .set_component(entity, opening)
            .expect("opening phase writes");

        let open = collect(&registry, &[false]);
        assert!(open.blocked_portal_ids.is_empty());
        assert_eq!(open.mover_rows[0].sealed_portal_ids, vec![0]);
        assert!(!open.mover_rows[0].docked_closed);
    }
}
