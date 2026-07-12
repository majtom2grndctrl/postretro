//! Trigger diagnostics snapshots and app-side egui overlay projection.
//! See: context/lib/entity_model.md §5 · context/lib/rendering_pipeline.md §12

use glam::{Mat4, Vec3, Vec4};
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry, TriggerActivation};

use crate::render;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::trigger_bindings::TriggerBindingTable;
use crate::trigger_system::{TriggerEventEdge, TriggerSystem};

pub(crate) struct TriggerOverlayLabel {
    screen_position: Option<egui::Pos2>,
    projected_edges: Vec<(egui::Pos2, egui::Pos2)>,
    text: String,
    armed: bool,
}

/// Build dev-tools rows from engine-owned trigger state.
/// Renderer consumes row data, never trigger components or entity IDs.
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
                on_fire_resolved: bindings.is_bound(id, TriggerEventEdge::Enter),
                on_exit: trigger.on_exit.clone(),
                on_exit_resolved: bindings.is_bound(id, TriggerEventEdge::Exit),
            })
        })
        .collect();
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    rows
}

/// Prepare app-side labels and AABB edges from live camera state.
/// Projection stays outside the renderer because it reads trigger component data.
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
    if viewport_size_points.x <= 0.0 || viewport_size_points.y <= 0.0 {
        return Vec::new();
    }
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
    let clip_corners = corners.map(|corner| view_projection * corner.extend(1.0));
    EDGES
        .iter()
        .filter_map(|&(from, to)| {
            let (from, to) = clip_segment_to_viewport(clip_corners[from], clip_corners[to])?;
            Some((
                clip_to_screen(from, viewport_size_points)?,
                clip_to_screen(to, viewport_size_points)?,
            ))
        })
        .collect()
}

fn world_to_screen(
    world_position: Vec3,
    view_projection: Mat4,
    viewport_size_points: egui::Vec2,
) -> Option<egui::Pos2> {
    clip_to_screen(
        view_projection * world_position.extend(1.0),
        viewport_size_points,
    )
}

/// Clip a homogeneous segment before perspective division. Clipping in clip
/// space keeps edges that cross the viewport or near plane finite on screen.
fn clip_segment_to_viewport(start: Vec4, end: Vec4) -> Option<(Vec4, Vec4)> {
    if !start.is_finite() || !end.is_finite() {
        return None;
    }

    const MIN_CLIP_W: f32 = 1.0e-5;
    let start_distances = clip_plane_distances(start, MIN_CLIP_W);
    let end_distances = clip_plane_distances(end, MIN_CLIP_W);
    let mut enter: f32 = 0.0;
    let mut exit: f32 = 1.0;

    for (&start_distance, &end_distance) in start_distances.iter().zip(end_distances.iter()) {
        if start_distance < 0.0 && end_distance < 0.0 {
            return None;
        }
        if start_distance < 0.0 {
            enter = enter.max(start_distance / (start_distance - end_distance));
        } else if end_distance < 0.0 {
            exit = exit.min(start_distance / (start_distance - end_distance));
        }
        if enter > exit {
            return None;
        }
    }

    let direction = end - start;
    let clipped_start = start + direction * enter;
    let clipped_end = start + direction * exit;
    (clipped_start.is_finite() && clipped_end.is_finite()).then_some((clipped_start, clipped_end))
}

fn clip_plane_distances(clip: Vec4, minimum_w: f32) -> [f32; 7] {
    [
        clip.w + clip.x,
        clip.w - clip.x,
        clip.w + clip.y,
        clip.w - clip.y,
        clip.z,
        clip.w - clip.z,
        clip.w - minimum_w,
    ]
}

fn clip_to_screen(clip: Vec4, viewport_size_points: egui::Vec2) -> Option<egui::Pos2> {
    if viewport_size_points.x <= 0.0 || viewport_size_points.y <= 0.0 {
        return None;
    }
    if clip.w <= 0.0 || !clip.is_finite() {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    const CLIP_EPSILON: f32 = 1.0e-5;
    if !ndc.is_finite()
        || ndc.x < -1.0 - CLIP_EPSILON
        || ndc.x > 1.0 + CLIP_EPSILON
        || ndc.y < -1.0 - CLIP_EPSILON
        || ndc.y > 1.0 + CLIP_EPSILON
        || ndc.z < -CLIP_EPSILON
        || ndc.z > 1.0 + CLIP_EPSILON
    {
        return None;
    }
    let ndc = ndc.clamp(Vec3::new(-1.0, -1.0, 0.0), Vec3::ONE);
    Some(egui::pos2(
        (ndc.x + 1.0) * 0.5 * viewport_size_points.x,
        (1.0 - ndc.y) * 0.5 * viewport_size_points.y,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec4};
    use postretro_entities::{
        EntityId, MoverCommand, SlotTable, Transform, TriggerFireMode, TriggerVolumeComponent,
    };
    use postretro_scripting_core::data_descriptors::{NamedReaction, ReactionDescriptor};
    use postretro_scripting_core::data_registry::DataRegistry;

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
    fn trigger_rows_report_binding_resolution_and_fallback_name() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry);
        let mut bridge = TriggerVolumeBridge::new();
        bridge.insert_for_test(trigger, Vec3::splat(-1.0), Vec3::splat(1.0));
        let mut data_registry = DataRegistry::new();
        data_registry.populate_level(
            vec![NamedReaction {
                name: "open_door".into(),
                descriptor: ReactionDescriptor::Sequence(Vec::new()),
            }],
            Vec::new(),
            &[],
        );
        let bindings = TriggerBindingTable::build(&registry, &data_registry, &SlotTable::new());

        let rows = collect_trigger_diagnostics_rows(
            &registry,
            &bridge,
            &TriggerSystem::default(),
            &bindings,
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, trigger.to_string());
        assert_eq!(rows[0].tags, "plate");
        assert_eq!(rows[0].activation, "touch");
        assert!(rows[0].armed);
        assert_eq!(rows[0].occupancy, 0);
        assert_eq!(rows[0].on_fire, "open_door");
        assert!(rows[0].on_fire_resolved);
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

    #[test]
    fn projected_aabb_edges_clip_partially_visible_segments() {
        let edges = projected_aabb_edges(
            Vec3::new(-2.0, -0.5, 0.25),
            Vec3::new(0.5, 0.5, 0.75),
            Mat4::IDENTITY,
            egui::vec2(640.0, 480.0),
        );

        assert_eq!(edges.len(), 8);
        assert!(
            edges
                .iter()
                .any(|(start, end)| start.x.abs() < 1.0e-3 || end.x.abs() < 1.0e-3)
        );
        assert_screen_edges_within_viewport(&edges, egui::vec2(640.0, 480.0));
    }

    #[test]
    fn projected_aabb_edges_clip_near_plane_crossing_segments() {
        let edges = projected_aabb_edges(
            Vec3::new(-0.5, -0.5, -0.5),
            Vec3::new(0.5, 0.5, 0.5),
            Mat4::IDENTITY,
            egui::vec2(640.0, 480.0),
        );

        assert_eq!(edges.len(), 8);
        assert_screen_edges_within_viewport(&edges, egui::vec2(640.0, 480.0));
    }

    #[test]
    fn trigger_overlay_degrades_for_missing_aabb_stale_id_zero_viewport_and_behind_camera() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry);
        let mut bridge = TriggerVolumeBridge::new();

        assert!(
            collect_trigger_overlay_labels(
                &registry,
                &bridge,
                &TriggerSystem::default(),
                Mat4::IDENTITY,
                egui::vec2(640.0, 480.0),
            )
            .is_empty()
        );

        bridge.insert_for_test(trigger, Vec3::splat(-0.5), Vec3::splat(0.5));
        let zero_viewport = collect_trigger_overlay_labels(
            &registry,
            &bridge,
            &TriggerSystem::default(),
            Mat4::IDENTITY,
            egui::Vec2::ZERO,
        );
        assert_eq!(zero_viewport.len(), 1);
        assert!(zero_viewport[0].screen_position.is_none());
        assert!(zero_viewport[0].projected_edges.is_empty());

        bridge.insert_for_test(
            trigger,
            Vec3::new(-0.5, -0.5, 1.0),
            Vec3::new(0.5, 0.5, 2.0),
        );
        let behind_camera_projection = Mat4::from_cols(
            Vec4::X,
            Vec4::Y,
            Vec4::new(0.0, 0.0, -1.0, -1.0),
            Vec4::ZERO,
        );
        let behind_camera = collect_trigger_overlay_labels(
            &registry,
            &bridge,
            &TriggerSystem::default(),
            behind_camera_projection,
            egui::vec2(640.0, 480.0),
        );
        assert_eq!(behind_camera.len(), 1);
        assert!(behind_camera[0].screen_position.is_none());
        assert!(behind_camera[0].projected_edges.is_empty());

        registry.despawn(trigger).unwrap();
        assert!(
            collect_trigger_overlay_labels(
                &registry,
                &bridge,
                &TriggerSystem::default(),
                Mat4::IDENTITY,
                egui::vec2(640.0, 480.0),
            )
            .is_empty()
        );
    }

    fn assert_screen_edges_within_viewport(
        edges: &[(egui::Pos2, egui::Pos2)],
        viewport: egui::Vec2,
    ) {
        for &(start, end) in edges {
            for point in [start, end] {
                assert!(point.x.is_finite() && point.y.is_finite());
                assert!((0.0..=viewport.x).contains(&point.x));
                assert!((0.0..=viewport.y).contains(&point.y));
            }
        }
    }
}
