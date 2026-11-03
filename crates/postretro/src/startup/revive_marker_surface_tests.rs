//! The revive-marker producer on a listen host and in single player, over the
//! dev mod's two-seat production tick loop: registry life states, each pawn's
//! `assistable` bool and the pair values the real assist pass evaluated. Row ids
//! name the acceptance rows of
//! context/plans/in-progress/E18--player-revive-assist/addendum-revive-marker.md.
//!
//! See: context/lib/ui.md §1 · context/lib/scripting.md §12 (Assist)
use super::fluids_swim_surface_tests::dev_manifest;
use super::player_death_surface_tests::{Consumer, command};
use crate::client_assist::{MarkerScratch, anchor_host_markers, select_host_markers};
use crate::impact_policy::AssistableTeammatesOverlayConfig;
use crate::presentation_pool::{OverlayKind, PresentationPool};
use glam::Vec3;
use postretro_entities::components::health::set_health_absolute;
use postretro_entities::{
    EntityId, EntityRegistry, PresentationFact, PresentationTemplateHandle, Transform,
};
use postretro_foundation::PlayerMovementComponent;
use postretro_scripting_core::data_descriptors::{
    ASSISTABLE_TEAMMATES_IN_RANGE_FACT, PresentationOverlayKind, PresentationWorldAnchor,
    presentation_overlay_of_kind,
};
use postretro_sim::scripting_systems::hit_zones::HitZoneStore;

const KIND: OverlayKind = OverlayKind::AssistableTeammates;
const LIFT: f32 = 0.6;

fn config(socket: Option<&str>) -> AssistableTeammatesOverlayConfig {
    AssistableTeammatesOverlayConfig {
        template: PresentationTemplateHandle::from("reviveMarker"),
        world_anchor: PresentationWorldAnchor {
            socket: socket.map(str::to_string),
            offset_y: LIFT,
        },
        max_visible: 8,
    }
}

/// The host's marker state across frames.
#[derive(Default)]
struct Markers {
    pool: PresentationPool,
    scratch: MarkerScratch,
    zones: HitZoneStore,
}

impl Markers {
    /// Selection: where the frame loop runs it, while transforms hold the
    /// authoritative pose.
    fn select(&mut self, h: &Consumer, config: &AssistableTeammatesOverlayConfig) {
        select_host_markers(
            &h.ctx.registry.borrow(),
            &h.assist,
            &mut self.pool,
            Some(config),
            &mut self.scratch,
        );
    }

    /// Anchors: after the presented pose is written.
    fn anchor(&mut self, registry: &EntityRegistry, config: &AssistableTeammatesOverlayConfig) {
        anchor_host_markers(
            registry,
            &mut self.pool,
            &self.zones,
            Some(config),
            0.0,
            &mut self.scratch,
        );
    }

    fn in_range(&self, pawn: EntityId) -> Option<bool> {
        match self
            .pool
            .overlay_facts(KIND, pawn)?
            .get(ASSISTABLE_TEAMMATES_IN_RANGE_FACT)
        {
            Some(PresentationFact::Bool(value)) => Some(*value),
            other => panic!("inRange is a Bool fact, got {other:?}"),
        }
    }

    fn marked(&self) -> Vec<EntityId> {
        self.pool.tracked_overlay_ids_iter(KIND).collect()
    }
}

fn place(h: &Consumer, pawn: EntityId, position: Vec3) {
    let mut pose = h.pose(pawn);
    pose.position = position;
    h.ctx
        .registry
        .borrow_mut()
        .set_component(pawn, pose)
        .unwrap();
}

/// Down `pawn` (one tick), then run one more tick so the assist pass has
/// evaluated pairs against the downed state.
fn down(h: &mut Consumer, pawn: EntityId) {
    set_health_absolute(&mut h.ctx.registry.borrow_mut(), pawn, 0.0);
    h.tick(command());
    h.tick(command());
    assert_eq!(h.name(pawn), "down");
}

/// The other seat downed 1 m from the host: inside the dev mod's 1.5 m reach.
fn other_down_beside_the_host() -> Consumer {
    let mut h = Consumer::new();
    let beside = h.pose(h.host).position + Vec3::X;
    let other = h.other;
    place(&h, other, beside);
    down(&mut h, other);
    h
}

/// R7: on the host, the downed teammate carries a marker reading `inRange`
/// while a hold would revive it; out of reach it stays a plain marker; the
/// host's own pawn never carries one.
#[test]
fn revive_marker_r7_host_marks_the_downed_teammate_and_prompts_in_reach() {
    let mut h = other_down_beside_the_host();
    let config = config(None);
    let mut markers = Markers::default();
    assert!(
        h.assist.pair_values(h.host, h.other).is_some(),
        "the assist pass evaluated the pair"
    );
    markers.select(&h, &config);
    assert_eq!(markers.marked(), vec![h.other]);
    assert_eq!(markers.in_range(h.other), Some(true));
    assert!(!markers.pool.has_overlay(KIND, h.host));

    // Any distance keeps the marker; past the reach the prompt goes.
    let far = h.pose(h.host).position + Vec3::X * 40.0;
    place(&h, h.other, far);
    h.tick(command());
    markers.select(&h, &config);
    assert_eq!(markers.marked(), vec![h.other]);
    assert_eq!(markers.in_range(h.other), Some(false));
}

/// R7: a downed host sees its downed teammate's marker with no prompt.
#[test]
fn revive_marker_r7_a_downed_host_sees_only_a_plain_marker() {
    let mut h = other_down_beside_the_host();
    let host = h.host;
    down(&mut h, host);
    let mut markers = Markers::default();
    markers.select(&h, &config(None));
    assert_eq!(markers.marked(), vec![h.other]);
    assert_eq!(markers.in_range(h.other), Some(false));
    assert!(!markers.pool.has_overlay(KIND, h.host));
}

/// R7: a teammate standing back up loses its marker on that frame.
#[test]
fn revive_marker_r7_host_drops_the_marker_when_the_teammate_stands() {
    let h = other_down_beside_the_host();
    let config = config(None);
    let mut markers = Markers::default();
    markers.select(&h, &config);
    assert_eq!(markers.marked(), vec![h.other]);
    {
        let mut registry = h.ctx.registry.borrow_mut();
        let mut movement = registry
            .get_component::<PlayerMovementComponent>(h.other)
            .unwrap()
            .clone();
        movement.assistable = false;
        registry.set_component(h.other, movement).unwrap();
    }
    markers.select(&h, &config);
    assert!(markers.marked().is_empty());
}

/// R7: single player marks nothing, even with its only pawn assistable.
#[test]
fn revive_marker_r7_single_player_produces_no_markers() {
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    let mut movement =
        PlayerMovementComponent::from_descriptor(&crate::mod_digest::tests::movement_descriptor());
    movement.assistable = true;
    registry.set_component(pawn, movement).unwrap();
    let mut pool = PresentationPool::default();
    let mut scratch = MarkerScratch::default();
    select_host_markers(
        &registry,
        &postretro_sim::sim::assist::AssistState::default(),
        &mut pool,
        Some(&config(None)),
        &mut scratch,
    );
    assert_eq!(pool.overlay_count(KIND), 0);
}

/// R9 (host): with no posed socket, including a named socket the model does
/// not have, the anchor is the pawn's position plus `offsetY`, never the top
/// of the standing hitbox a downed pawn keeps.
#[test]
fn revive_marker_r9_host_anchor_falls_back_to_the_position_plus_offset() {
    let h = other_down_beside_the_host();
    for socket in [None, Some("head")] {
        let config = config(socket);
        let mut markers = Markers::default();
        markers.select(&h, &config);
        markers.anchor(&h.ctx.registry.borrow(), &config);
        let position = h.pose(h.other).position;
        let anchor = markers.pool.overlay_anchor(KIND, h.other).unwrap();
        assert!(
            (anchor - (position + Vec3::Y * LIFT)).length() < 1e-5,
            "socket {socket:?}: {anchor:?} vs {position:?}"
        );
    }
}

/// R14: on a listen host, `inRange` reads the authoritative pose the assist
/// pass read, while the anchor follows the delayed pose the host then draws.
#[test]
fn revive_marker_r14_range_reads_the_authoritative_pose_and_the_anchor_the_presented_one() {
    let mut h = other_down_beside_the_host();
    let config = config(None);
    let mut markers = Markers::default();
    let authoritative = h.pose(h.other).position;

    // Selection before presentation: in reach on the authoritative pose.
    markers.select(&h, &config);
    // Presentation writes a delayed pose well outside the reach.
    let presented = authoritative + Vec3::X * 4.0;
    place(&h, h.other, presented);
    markers.anchor(&h.ctx.registry.borrow(), &config);
    assert_eq!(markers.in_range(h.other), Some(true));
    let anchor = markers.pool.overlay_anchor(KIND, h.other).unwrap();
    assert!((anchor - (presented + Vec3::Y * LIFT)).length() < 1e-5);

    // The next tick moves the pawn out of reach on its authoritative pose
    // while the delayed pose still stands beside the host: no prompt.
    place(&h, h.other, authoritative + Vec3::X * 4.0);
    h.tick(command());
    markers.select(&h, &config);
    place(&h, h.other, authoritative);
    markers.anchor(&h.ctx.registry.borrow(), &config);
    assert_eq!(markers.in_range(h.other), Some(false));
    let anchor = markers.pool.overlay_anchor(KIND, h.other).unwrap();
    assert!((anchor - (authoritative + Vec3::Y * LIFT)).length() < 1e-5);
}

/// R13 (host and single player): with only the marker overlay declared, the
/// single-player damaged-enemy pass, which runs with no config of its own, and
/// the pool's per-frame advance keep every marker frame after frame.
#[test]
fn revive_marker_r13_host_markers_survive_the_no_config_damaged_enemy_pass() {
    let h = other_down_beside_the_host();
    let config = config(None);
    let mut markers = Markers::default();
    let mut runtime = crate::impact_policy::ImpactPolicyRuntime::new(h.ctx.clone());
    assert!(runtime.client_overlay_config().is_none());
    for _ in 0..5 {
        let _ = runtime.update_damaged_enemy_overlays(
            &mut markers.pool,
            &h.ctx.registry.borrow(),
            &markers.zones,
            0.0,
            [],
            |_| false,
        );
        markers.select(&h, &config);
        markers.anchor(&h.ctx.registry.borrow(), &config);
        let inputs = markers.pool.advance_and_collect_inputs(
            &mut h.ctx.registry.borrow_mut(),
            1.0,
            glam::Mat4::IDENTITY,
            [800, 600],
            crate::presentation_pool::MotionPreference::Full,
        );
        markers.pool.recycle_draw_inputs(inputs);
        assert_eq!(markers.marked(), vec![h.other]);
    }
}

/// Whether any node of the serialized widget tree satisfies `found`.
fn any_node(
    value: &serde_json::Value,
    found: &dyn Fn(&serde_json::Map<String, serde_json::Value>) -> bool,
) -> bool {
    match value {
        serde_json::Value::Object(map) => found(map) || map.values().any(|v| any_node(v, found)),
        serde_json::Value::Array(items) => items.iter().any(|v| any_node(v, found)),
        _ => false,
    }
}

#[test]
fn revive_marker_r11_dev_mod_declares_both_overlays_and_the_marker_template() {
    let manifest = dev_manifest();

    let overlays = &manifest.presentation_overlays;
    assert_eq!(
        overlays.len(),
        2,
        "damaged-enemy and revive-marker overlays"
    );
    for overlay in overlays {
        overlay.validate().expect("overlay validates");
    }
    assert!(
        presentation_overlay_of_kind(overlays, PresentationOverlayKind::DamagedEnemies).is_some()
    );
    let marker =
        presentation_overlay_of_kind(overlays, PresentationOverlayKind::AssistableTeammates)
            .expect("the dev mod declares the assistable-teammates overlay");

    let template = manifest
        .presentation_templates
        .iter()
        .find(|template| template.id == marker.template)
        .unwrap_or_else(|| panic!("no template `{}` registered", marker.template));
    template.validate().expect("marker template validates");

    let root = serde_json::to_value(&template.root).expect("template root serializes");
    assert!(
        any_node(&root, &|node| node.get("kind").and_then(|k| k.as_str())
            == Some("glyph")
            && node.get("command").and_then(|c| c.as_str()) == Some("assist")),
        "marker template has an `assist` Glyph: {root}"
    );
    assert!(
        any_node(&root, &|node| node
            .get("visibleWhen")
            .is_some_and(|gate| gate
                .to_string()
                .contains(ASSISTABLE_TEAMMATES_IN_RANGE_FACT))),
        "marker template gates something on `inRange`: {root}"
    );
}
