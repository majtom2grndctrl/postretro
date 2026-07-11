//! Dev-tools trigger diagnostics and egui overlay projection.
//! See: context/lib/entity_model.md §5 · context/lib/rendering_pipeline.md §12

use glam::{Mat4, Vec3};
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry, TriggerActivation};

use crate::render;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::trigger_bindings::{TriggerBindingEdge, TriggerBindingTable};
use crate::trigger_system::TriggerSystem;

pub(crate) struct TriggerOverlayLabel {
    screen_position: Option<egui::Pos2>,
    projected_edges: Vec<(egui::Pos2, egui::Pos2)>,
    text: String,
    armed: bool,
}

/// Build the renderer's trigger table from engine-owned component and binding
/// state. The renderer never receives trigger component, binding, or entity-id
/// types across this boundary.
pub(crate) fn collect_trigger_diagnostics_rows(
    registry: &EntityRegistry,
    bridge: &TriggerVolumeBridge,
    trigger_system: &TriggerSystem,
    bindings: &TriggerBindingTable,
) -> Vec<render::TriggerDiagnosticsRow> {
    let mut rows: Vec<_> = registry
        .iter_with_kind(ComponentKind::TriggerVolume)
        .filter_map(|(id, value)| {
            let ComponentValue::TriggerVolume(trigger) = value else {
                return None;
            };
            let name = bridge
                .name(id)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| id.to_string());
            let tags = registry
                .get_tags(id)
                .ok()
                .map(|tags| tags.join(", "))
                .unwrap_or_default();
            Some(render::TriggerDiagnosticsRow {
                name,
                tags,
                activation: activation_label(trigger.activation).to_string(),
                armed: trigger.armed,
                latched: trigger.latched,
                rearm_remaining_ms: trigger.rearm_remaining_ms,
                occupancy: trigger_system.occupancy(id),
                on_fire: trigger.on_fire.clone(),
                on_fire_resolved: bindings.is_bound(id, TriggerBindingEdge::Enter),
                on_exit: trigger.on_exit.clone(),
                on_exit_resolved: bindings.is_bound(id, TriggerBindingEdge::Exit),
            })
        })
        .collect();
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    rows
}

/// Prepare screen-space labels and AABB edges for the egui foreground layer.
/// Projection stays app-side because it depends on the live camera and does not
/// require the GPU renderer to know about trigger-volume components.
pub(crate) fn collect_trigger_overlay_labels(
    registry: &EntityRegistry,
    bridge: &TriggerVolumeBridge,
    trigger_system: &TriggerSystem,
    view_projection: Mat4,
    viewport_size_points: egui::Vec2,
) -> Vec<TriggerOverlayLabel> {
    let mut labels: Vec<_> = registry
        .iter_with_kind(ComponentKind::TriggerVolume)
        .filter_map(|(id, value)| {
            let ComponentValue::TriggerVolume(trigger) = value else {
                return None;
            };
            let (aabb_min, aabb_max) = bridge.aabb(id)?;
            let center = (aabb_min + aabb_max) * 0.5;
            let name = bridge
                .name(id)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| id.to_string());
            Some(TriggerOverlayLabel {
                screen_position: world_to_screen(center, view_projection, viewport_size_points),
                projected_edges: projected_aabb_edges(
                    aabb_min,
                    aabb_max,
                    view_projection,
                    viewport_size_points,
                ),
                text: format!(
                    "{name} [{}] occ:{}",
                    activation_label(trigger.activation),
                    trigger_system.occupancy(id)
                ),
                armed: trigger.armed,
            })
        })
        .collect();
    labels.sort_by(|left, right| left.text.cmp(&right.text));
    labels
}

pub(crate) fn paint_trigger_overlay_labels(ctx: &egui::Context, labels: &[TriggerOverlayLabel]) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("trigger_overlay_labels"),
    ));
    let font_id = egui::FontId::monospace(12.0);
    let shadow_color = egui::Color32::from_black_alpha(190);

    for label in labels {
        let color = if label.armed {
            egui::Color32::from_rgb(110, 245, 170)
        } else {
            egui::Color32::from_rgb(255, 170, 100)
        };
        for &(start, end) in &label.projected_edges {
            painter.line_segment([start, end], egui::Stroke::new(1.0, color));
        }
        let Some(position) = label.screen_position else {
            continue;
        };
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
            color,
        );
    }
}

fn activation_label(activation: TriggerActivation) -> &'static str {
    match activation {
        TriggerActivation::Touch => "touch",
        TriggerActivation::Use => "use",
    }
}

fn projected_aabb_edges(
    min: Vec3,
    max: Vec3,
    view_projection: Mat4,
    viewport_size_points: egui::Vec2,
) -> Vec<(egui::Pos2, egui::Pos2)> {
    let corners = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    let projected =
        corners.map(|corner| world_to_screen(corner, view_projection, viewport_size_points));
    EDGES
        .iter()
        .filter_map(|&(from, to)| Some((projected[from]?, projected[to]?)))
        .collect()
}

fn world_to_screen(
    world_position: Vec3,
    view_projection: Mat4,
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;
    use postretro_entities::{
        EntityId, MoverCommand, Transform, TriggerFireMode, TriggerVolumeComponent,
    };

    fn spawn_trigger(registry: &mut EntityRegistry) -> EntityId {
        let id = registry.spawn(Transform {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        });
        registry.set_tags(id, vec!["plate".into()]).unwrap();
        registry
            .set_component(
                id,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    String::new(),
                    "open_door".into(),
                    "close_door".into(),
                    MoverCommand::Start,
                    TriggerFireMode::Once,
                    250.0,
                    true,
                ),
            )
            .unwrap();
        id
    }

    #[test]
    fn trigger_rows_snapshot_component_state_without_renderer_types() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry);
        let mut bridge = TriggerVolumeBridge::new();
        bridge.insert_for_test(trigger, Vec3::splat(-1.0), Vec3::splat(1.0));

        let rows = collect_trigger_diagnostics_rows(
            &registry,
            &bridge,
            &TriggerSystem::default(),
            &TriggerBindingTable::default(),
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tags, "plate");
        assert_eq!(rows[0].activation, "touch");
        assert!(rows[0].armed);
        assert_eq!(rows[0].occupancy, 0);
        assert_eq!(rows[0].on_fire, "open_door");
        assert!(!rows[0].on_fire_resolved);
        assert_eq!(rows[0].on_exit, "close_door");
        assert!(!rows[0].on_exit_resolved);
    }

    #[test]
    fn projected_aabb_edges_emit_all_twelve_visible_edges() {
        let edges = projected_aabb_edges(
            Vec3::new(-0.5, -0.5, 0.0),
            Vec3::new(0.5, 0.5, 0.5),
            Mat4::IDENTITY,
            egui::vec2(640.0, 480.0),
        );

        assert_eq!(edges.len(), 12);
    }
}
