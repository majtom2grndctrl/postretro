// Resolved configuration for the assistable-teammate marker overlay.
// See: context/lib/ui.md §1 (presentation layer)

use postretro_entities::PresentationTemplateHandle;
use postretro_scripting_core::data_descriptors::PresentationWorldAnchor;

/// The authored half of the assistable-teammate marker: which template draws
/// it, where it anchors, and how many may show. Every machine derives which
/// teammates are marked from data it already holds, so this carries no state.
#[derive(Debug, Clone, PartialEq)]
pub struct AssistableTeammatesOverlayConfig {
    pub template: PresentationTemplateHandle,
    pub world_anchor: PresentationWorldAnchor,
    pub max_visible: usize,
}

#[cfg(test)]
mod tests {
    use super::super::ImpactPolicyRuntime;
    use postretro_entities::ScriptCtx;
    use postretro_scripting_core::data_descriptors::{PresentationOverlay, PresentationTemplate};
    use postretro_test_log_capture::LogCapture;

    fn template(id: &str, world_anchor: serde_json::Value) -> PresentationTemplate {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "root": { "kind": "text", "content": "x", "fontSize": 12, "color": [1, 1, 1, 1] },
            "lifetimeMs": 1, "motion": { "rise": 0, "easing": "linear" },
            "fade": { "startMs": 1 }, "spawnScatter": { "radius": 0 },
            "worldAnchor": world_anchor,
        }))
        .unwrap()
    }

    fn overlay(over: serde_json::Value, template: &str, max_visible: usize) -> PresentationOverlay {
        serde_json::from_value(serde_json::json!({
            "over": over, "template": template, "maxVisible": max_visible,
        }))
        .unwrap()
    }

    fn runtime() -> ImpactPolicyRuntime {
        let mut runtime = ImpactPolicyRuntime::new(ScriptCtx::new());
        runtime.replace_presentation_templates(vec![
            template(
                "enemyStatus",
                serde_json::json!({ "socket": "head", "offsetY": 0.35 }),
            ),
            template("reviveMarker", serde_json::json!({ "offsetY": 0.6 })),
        ]);
        runtime
    }

    #[test]
    fn both_overlay_kinds_install_side_by_side_with_per_kind_config() {
        let mut runtime = runtime();
        runtime.replace_presentation_overlays(vec![
            overlay(
                serde_json::json!({ "kind": "damagedEnemies", "lingerMs": 500, "hideAtFull": true }),
                "enemyStatus",
                4,
            ),
            overlay(
                serde_json::json!({ "kind": "assistableTeammates" }),
                "reviveMarker",
                8,
            ),
        ]);

        let damaged = runtime
            .client_overlay_config()
            .expect("damaged-enemy config");
        assert_eq!(damaged.template.0.as_str(), "enemyStatus");
        assert_eq!(damaged.max_visible, 4);

        let marker = runtime
            .assistable_teammates_overlay_config()
            .expect("marker config");
        assert_eq!(marker.template.0.as_str(), "reviveMarker");
        assert_eq!(marker.max_visible, 8);
        assert!(marker.world_anchor.socket.is_empty());
        assert_eq!(marker.world_anchor.offset_y, 0.6);
    }

    #[test]
    fn marker_alone_leaves_damaged_enemy_config_absent_and_replacement_clears_it() {
        let mut runtime = runtime();
        runtime.replace_presentation_overlays(vec![overlay(
            serde_json::json!({ "kind": "assistableTeammates" }),
            "reviveMarker",
            8,
        )]);
        assert!(runtime.client_overlay_config().is_none());
        assert!(runtime.assistable_teammates_overlay_config().is_some());

        runtime.replace_presentation_overlays(Vec::new());
        assert!(runtime.assistable_teammates_overlay_config().is_none());
    }

    #[test]
    fn repeated_kind_keeps_first_and_warns_naming_the_kind() {
        let mut runtime = runtime();
        let capture = LogCapture::start();
        runtime.replace_presentation_overlays(vec![
            overlay(
                serde_json::json!({ "kind": "assistableTeammates" }),
                "reviveMarker",
                3,
            ),
            overlay(
                serde_json::json!({ "kind": "assistableTeammates" }),
                "enemyStatus",
                5,
            ),
        ]);
        capture.assert_logged_once(
            log::Level::Warn,
            "repeats kind `assistableTeammates`; only the first",
        );
        let marker = runtime.assistable_teammates_overlay_config().unwrap();
        assert_eq!(marker.template.0.as_str(), "reviveMarker");
        assert_eq!(marker.max_visible, 3);
    }
}
