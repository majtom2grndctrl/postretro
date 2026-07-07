use glam::Vec3;

use crate::render::{self, Renderer};

pub(crate) struct AgentOverlayGeometry {
    id: postretro_entities::EntityId,
    position: Vec3,
    path: Vec<Vec3>,
    waypoint_cursor: usize,
    velocity: Vec3,
    destination: Option<Vec3>,
    planned_destination: Option<Vec3>,
    radius: f32,
}

pub(crate) struct AgentOverlayLabel {
    id: postretro_entities::EntityId,
    screen_position: Option<egui::Pos2>,
    text: String,
    state: Option<String>,
    speed: f32,
    flags: AgentOverlayLabelFlags,
}

#[derive(Clone, Copy)]
struct AgentOverlayLabelFlags {
    arrived: bool,
    blocked: bool,
    has_path: bool,
}

pub(crate) fn collect_agent_overlay_snapshots_for_view(
    registry: &postretro_entities::EntityRegistry,
    view_projection: glam::Mat4,
    viewport_size_points: egui::Vec2,
    include_geometry: bool,
    include_labels: bool,
) -> (Vec<AgentOverlayGeometry>, Vec<AgentOverlayLabel>) {
    use postretro_entities::Transform;
    use postretro_entities::components::brain::BrainComponent;
    use postretro_entities::registry::{ComponentKind, ComponentValue};

    let mut geometry = Vec::new();
    let mut labels = Vec::new();
    for (id, value) in registry.iter_with_kind(ComponentKind::Agent) {
        let ComponentValue::Agent(agent) = value else {
            continue;
        };
        let transform = registry.get_component::<Transform>(id).ok();
        if include_geometry {
            if let Some(transform) = transform {
                geometry.push(AgentOverlayGeometry {
                    id,
                    position: transform.position,
                    path: agent.path.clone(),
                    waypoint_cursor: agent.waypoint_cursor,
                    velocity: agent.velocity,
                    destination: agent.destination,
                    planned_destination: agent.planned_destination,
                    radius: agent.radius,
                });
            }
        }
        if !include_labels {
            continue;
        }
        let state_label = registry
            .get_component::<BrainComponent>(id)
            .ok()
            .map(|brain| brain.state.label().to_string());
        let xz_speed = Vec3::new(agent.velocity.x, 0.0, agent.velocity.z).length();
        let flags = AgentOverlayLabelFlags {
            arrived: agent.arrived,
            blocked: agent.blocked,
            has_path: !agent.path.is_empty(),
        };
        let screen_position = transform.and_then(|transform| {
            let label_anchor = transform.position + Vec3::Y * (agent.height * 0.5 + 0.15);
            agent_overlay_world_to_screen(label_anchor, view_projection, viewport_size_points)
        });
        labels.push(AgentOverlayLabel {
            id,
            screen_position,
            text: assemble_agent_overlay_label(state_label.as_deref(), xz_speed, flags),
            state: state_label,
            speed: xz_speed,
            flags,
        });
    }
    (geometry, labels)
}

fn agent_overlay_world_to_screen(
    world_position: Vec3,
    view_projection: glam::Mat4,
    viewport_size_points: egui::Vec2,
) -> Option<egui::Pos2> {
    if viewport_size_points.x <= 0.0 || viewport_size_points.y <= 0.0 {
        return None;
    }

    let clip = view_projection * world_position.extend(1.0);
    if clip.w <= 0.0 || !clip.is_finite() {
        return None;
    }

    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite()
        || ndc.x < -1.0
        || ndc.x > 1.0
        || ndc.y < -1.0
        || ndc.y > 1.0
        || ndc.z < 0.0
        || ndc.z > 1.0
    {
        return None;
    }

    Some(egui::pos2(
        (ndc.x + 1.0) * 0.5 * viewport_size_points.x,
        (1.0 - ndc.y) * 0.5 * viewport_size_points.y,
    ))
}

fn assemble_agent_overlay_label(
    fsm_state_label: Option<&str>,
    xz_speed: f32,
    flags: AgentOverlayLabelFlags,
) -> String {
    format!(
        "state:{} speed:{:.2} arrived:{} blocked:{} has_path:{}",
        fsm_state_label.unwrap_or("-"),
        xz_speed,
        flags.arrived,
        flags.blocked,
        flags.has_path
    )
}

pub(crate) fn agent_overlay_diagnostics_rows(
    labels: &[AgentOverlayLabel],
) -> Vec<render::AgentDiagnosticsRow> {
    labels
        .iter()
        .map(|label| render::AgentDiagnosticsRow {
            id: label.id.to_string(),
            state: label.state.clone(),
            speed: label.speed,
            arrived: label.flags.arrived,
            blocked: label.flags.blocked,
            has_path: label.flags.has_path,
        })
        .collect()
}

fn agent_overlay_live_destination(agent: &AgentOverlayGeometry) -> Option<Vec3> {
    agent.destination
}

fn agent_overlay_planned_destination(agent: &AgentOverlayGeometry) -> Option<Vec3> {
    agent.planned_destination
}

fn agent_marker_segments(center: Vec3, radius: f32) -> [(Vec3, Vec3); 3] {
    let arm = radius.max(0.05);
    [
        (
            center - Vec3::new(arm, 0.0, 0.0),
            center + Vec3::new(arm, 0.0, 0.0),
        ),
        (
            center - Vec3::new(0.0, 0.0, arm),
            center + Vec3::new(0.0, 0.0, arm),
        ),
        (
            center - Vec3::new(0.0, arm, 0.0),
            center + Vec3::new(0.0, arm, 0.0),
        ),
    ]
}

pub(crate) fn paint_agent_overlay_labels(ctx: &egui::Context, labels: &[AgentOverlayLabel]) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("agent_overlay_labels"),
    ));
    let font_id = egui::FontId::monospace(12.0);
    let text_color = egui::Color32::from_rgb(235, 245, 255);
    let shadow_color = egui::Color32::from_black_alpha(190);

    for label in labels {
        let Some(position) = label.screen_position else {
            continue;
        };
        let _entity_id = label.id;
        let anchor = position + egui::vec2(0.0, -4.0);
        painter.text(
            anchor + egui::vec2(1.0, 1.0),
            egui::Align2::CENTER_BOTTOM,
            &label.text,
            font_id.clone(),
            shadow_color,
        );
        painter.text(
            anchor,
            egui::Align2::CENTER_BOTTOM,
            &label.text,
            font_id.clone(),
            text_color,
        );
    }
}

pub(crate) fn emit_agent_overlay_geometry(
    renderer: &mut Renderer,
    agents: &[AgentOverlayGeometry],
) {
    const COLOR_AGENT_VELOCITY: [u8; 4] = [80, 180, 255, 255];
    const COLOR_AGENT_LIVE_DESTINATION: [u8; 4] = [255, 210, 80, 255];
    const COLOR_AGENT_PLANNED_DESTINATION: [u8; 4] = [185, 130, 255, 255];

    let state = renderer.agent_overlay_state();
    if !state.enabled {
        return;
    }

    for agent in agents {
        let _entity_id = agent.id;
        if state.paths {
            renderer.emit_agent_path_overlay(
                agent.position,
                &agent.path,
                agent.waypoint_cursor,
                agent.radius,
            );
        }
        if state.velocities && agent.velocity.length_squared() > 0.0 {
            renderer.push_debug_line(
                agent.position,
                agent.position + agent.velocity,
                COLOR_AGENT_VELOCITY,
            );
        }
        if state.destinations {
            if let Some(planned_destination) = agent_overlay_planned_destination(agent) {
                if Some(planned_destination) != agent_overlay_live_destination(agent) {
                    for (start, end) in agent_marker_segments(planned_destination, agent.radius) {
                        renderer.push_debug_line(start, end, COLOR_AGENT_PLANNED_DESTINATION);
                    }
                }
            }
            if let Some(destination) = agent_overlay_live_destination(agent) {
                for (start, end) in agent_marker_segments(destination, agent.radius) {
                    renderer.push_debug_line(start, end, COLOR_AGENT_LIVE_DESTINATION);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "dev-tools")]
    fn assert_vec3_near(actual: Vec3, expected: Vec3) {
        const EPSILON: f32 = 0.000_001;
        let delta = (actual - expected).abs();
        assert!(
            delta.cmple(Vec3::splat(EPSILON)).all(),
            "expected {actual:?} to be within {EPSILON} of {expected:?}",
        );
    }

    #[cfg(feature = "dev-tools")]
    fn assert_pos2_near(actual: egui::Pos2, expected: egui::Pos2) {
        const EPSILON: f32 = 0.000_1;
        assert!(
            (actual.x - expected.x).abs() <= EPSILON && (actual.y - expected.y).abs() <= EPSILON,
            "expected {actual:?} to be within {EPSILON} of {expected:?}",
        );
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn agent_overlay_snapshot_collects_geometry_and_brainless_label() {
        use postretro_entities::components::agent::AgentComponent;
        use postretro_entities::{EntityRegistry, Transform};

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            ..Transform::default()
        });
        let mut agent = AgentComponent::new(0.35, 1.8, 0.4, 4.0);
        agent.path = vec![Vec3::new(2.0, 2.0, 3.0), Vec3::new(3.0, 2.0, 4.0)];
        agent.waypoint_cursor = 1;
        agent.velocity = Vec3::new(0.5, 0.0, 0.25);
        agent.destination = Some(Vec3::new(4.0, 2.0, 5.0));
        agent.planned_destination = Some(Vec3::new(3.5, 2.0, 4.5));
        registry.set_component(id, agent).unwrap();

        let (snapshot, labels) = collect_agent_overlay_snapshots_for_view(
            &registry,
            glam::Mat4::IDENTITY,
            egui::Vec2::ZERO,
            true,
            true,
        );

        assert_eq!(snapshot.len(), 1);
        assert_eq!(labels.len(), 1);
        let rows = agent_overlay_diagnostics_rows(&labels);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id.to_string());
        assert_eq!(rows[0].state, None);
        let expected_speed = Vec3::new(0.5, 0.0, 0.25).length();
        assert!((rows[0].speed - expected_speed).abs() <= 0.000_001);
        assert!(!rows[0].arrived);
        assert!(!rows[0].blocked);
        assert!(rows[0].has_path);
        let sampled = &snapshot[0];
        assert_eq!(sampled.id, id);
        assert_vec3_near(sampled.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(sampled.path.len(), 2);
        assert_vec3_near(sampled.path[0], Vec3::new(2.0, 2.0, 3.0));
        assert_vec3_near(sampled.path[1], Vec3::new(3.0, 2.0, 4.0));
        assert_eq!(sampled.waypoint_cursor, 1);
        assert_vec3_near(sampled.velocity, Vec3::new(0.5, 0.0, 0.25));
        let live_destination =
            agent_overlay_live_destination(sampled).expect("live destination is recorded");
        assert_vec3_near(live_destination, Vec3::new(4.0, 2.0, 5.0));
        let planned_destination = agent_overlay_planned_destination(sampled)
            .expect("planned destination is recorded separately");
        assert_vec3_near(planned_destination, Vec3::new(3.5, 2.0, 4.5));
        assert!((sampled.radius - 0.35).abs() <= 0.000_001);

        let label = &labels[0];
        assert_eq!(label.id, id);
        assert_eq!(label.screen_position, None);
        assert_eq!(label.state, None);
        assert_eq!(
            label.text,
            "state:- speed:0.56 arrived:false blocked:false has_path:true"
        );
        assert!(label.flags.has_path);
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn agent_overlay_row_only_snapshot_skips_geometry_path_clone() {
        use postretro_entities::components::agent::AgentComponent;
        use postretro_entities::{EntityRegistry, Transform};

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        let mut agent = AgentComponent::new(0.35, 1.8, 0.4, 4.0);
        agent.path = vec![Vec3::new(1.0, 0.0, 0.0); 4];
        registry.set_component(id, agent).unwrap();

        let (geometry, labels) = collect_agent_overlay_snapshots_for_view(
            &registry,
            glam::Mat4::IDENTITY,
            egui::Vec2::ZERO,
            false,
            true,
        );

        assert!(geometry.is_empty());
        assert_eq!(labels.len(), 1);
        assert_eq!(agent_overlay_diagnostics_rows(&labels).len(), 1);
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn agent_overlay_rows_include_agent_without_transform() {
        use postretro_entities::components::agent::AgentComponent;
        use postretro_entities::{EntityRegistry, Transform};

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        let mut agent = AgentComponent::new(0.35, 1.8, 0.4, 4.0);
        agent.blocked = true;
        registry.set_component(id, agent).unwrap();
        registry.remove_component::<Transform>(id).unwrap();

        let (geometry, labels) = collect_agent_overlay_snapshots_for_view(
            &registry,
            glam::Mat4::IDENTITY,
            egui::Vec2::ZERO,
            true,
            true,
        );

        assert!(geometry.is_empty());
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].screen_position, None);
        let rows = agent_overlay_diagnostics_rows(&labels);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id.to_string());
        assert!(rows[0].blocked);
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn agent_overlay_projection_maps_in_front_point_inside_viewport() {
        let render_camera =
            crate::camera::RenderCamera::new(Vec3::ZERO, 4.0 / 3.0, 0.0, 0.0, 0.0, Vec3::ZERO);
        let screen = agent_overlay_world_to_screen(
            Vec3::new(0.0, 0.0, -2.0),
            render_camera.view_projection,
            egui::vec2(800.0, 600.0),
        )
        .expect("center point in front of camera should project to viewport center");

        assert_pos2_near(screen, egui::pos2(400.0, 300.0));
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn agent_overlay_projection_rejects_behind_camera_point() {
        let render_camera =
            crate::camera::RenderCamera::new(Vec3::ZERO, 4.0 / 3.0, 0.0, 0.0, 0.0, Vec3::ZERO);

        assert_eq!(
            agent_overlay_world_to_screen(
                Vec3::new(0.0, 0.0, 2.0),
                render_camera.view_projection,
                egui::vec2(800.0, 600.0),
            ),
            None
        );
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn agent_overlay_projection_rejects_offscreen_point() {
        let render_camera =
            crate::camera::RenderCamera::new(Vec3::ZERO, 4.0 / 3.0, 0.0, 0.0, 0.0, Vec3::ZERO);

        assert_eq!(
            agent_overlay_world_to_screen(
                Vec3::new(100.0, 0.0, -2.0),
                render_camera.view_projection,
                egui::vec2(800.0, 600.0),
            ),
            None
        );
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn agent_overlay_label_includes_state_speed_and_flags() {
        let label = assemble_agent_overlay_label(
            Some("alert"),
            3.456,
            AgentOverlayLabelFlags {
                arrived: true,
                blocked: false,
                has_path: true,
            },
        );

        assert_eq!(
            label,
            "state:alert speed:3.46 arrived:true blocked:false has_path:true"
        );
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn agent_overlay_label_uses_placeholder_for_brainless_agent() {
        let label = assemble_agent_overlay_label(
            None,
            0.0,
            AgentOverlayLabelFlags {
                arrived: false,
                blocked: true,
                has_path: false,
            },
        );

        assert_eq!(
            label,
            "state:- speed:0.00 arrived:false blocked:true has_path:false"
        );
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn agent_marker_segments_use_radius_floor_and_three_axes() {
        let center = Vec3::new(10.0, 20.0, 30.0);
        let segments = agent_marker_segments(center, 0.01);
        let arm = 0.05;

        assert_vec3_near(segments[0].0, center - Vec3::new(arm, 0.0, 0.0));
        assert_vec3_near(segments[0].1, center + Vec3::new(arm, 0.0, 0.0));
        assert_vec3_near(segments[1].0, center - Vec3::new(0.0, 0.0, arm));
        assert_vec3_near(segments[1].1, center + Vec3::new(0.0, 0.0, arm));
        assert_vec3_near(segments[2].0, center - Vec3::new(0.0, arm, 0.0));
        assert_vec3_near(segments[2].1, center + Vec3::new(0.0, arm, 0.0));
    }
}
