// Window controls cross both SDK runtimes and the shipped frontend menu.
// See: context/lib/scripting.md §12 · context/lib/player_options.md §7
use super::*;

#[test]
fn display_mode_actions_and_slot_refs_match_both_sdk_runtimes() {
    let ts = quickjs_fixture_value(
        r#"
        import { displayModeAction, getGameState } from "postretro/ui";
        const { options, window } = getGameState();
        JSON.stringify({ actions: ["next", "previous", "apply", "keep", "revert"].map(displayModeAction), options, window });
    "#,
    );
    let luau = luau_fixture_value(
        r#"
        local Ui = require("postretro/ui")
        local refs = Ui.getGameState()
        return { actions = { Ui.displayModeAction("next"), Ui.displayModeAction("previous"), Ui.displayModeAction("apply"), Ui.displayModeAction("keep"), Ui.displayModeAction("revert") }, options = refs.options, window = refs.window }
    "#,
    );
    assert_eq!(ts, luau);
    assert_eq!(
        ts["actions"],
        serde_json::json!([
            "ui.displayMode.next",
            "ui.displayMode.previous",
            "ui.displayMode.apply",
            "ui.displayMode.keep",
            "ui.displayMode.revert"
        ])
    );
    assert_eq!(ts["options"]["windowMode"]["slot"], "options.windowMode");
    for field in [
        "Width",
        "Height",
        "RefreshHz",
        "BitDepth",
        "Monitor",
        "RevertSeconds",
        "CanApply",
    ] {
        assert_eq!(
            ts["window"][format!("displayMode{field}")]["slot"],
            format!("window.displayMode{field}")
        );
    }
}

#[test]
fn shipped_frontend_window_controls_compile_and_evaluate() {
    let value = quickjs_fixture_value(&format!(
        "{}\nJSON.stringify({{ menu: optionsMenu, title: frontendMenu, reactions: frontendReactions }});",
        include_str!("../../../../content/dev/scripts/frontend-menu.ts")
    ));
    let reactions = value["reactions"].as_array().unwrap();
    for mode in ["windowed", "borderless", "exclusive"] {
        let name = format!("frontend.options.windowMode.{mode}");
        let reaction = reactions
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap();
        assert_eq!(reaction["primitive"], "setState");
        assert_eq!(reaction["args"]["slot"], "options.windowMode");
        assert_eq!(reaction["args"]["value"], mode);
    }
    let menu = serde_json::to_string(&value["menu"]).unwrap();
    for wire in [
        "ui.displayMode.next",
        "ui.displayMode.previous",
        "ui.displayMode.apply",
        "window.displayModeWidth",
        "window.displayModeHeight",
        "window.displayModeRefreshHz",
        "window.displayModeCanApply",
    ] {
        assert!(menu.contains(wire), "compiled menu missing {wire}");
    }
    let title = serde_json::to_string(&value["title"]).unwrap();
    assert!(
        title.contains(r#""slot":"session.hostAddress""#),
        "title menu binds the listen-host address: {title}"
    );
}
