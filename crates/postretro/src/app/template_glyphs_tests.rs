// Template glyphs resolve exactly as HUD glyphs do, and the templates re-send
// on each event that changes what a glyph draws: a rebinding, a device-family
// switch, art that finishes loading after install, and a template-only reload.
// Row ids name the acceptance rows of
// context/plans/in-progress/E18--player-revive-assist/addendum-revive-marker.md.

use gilrs::Button;
use postretro_ui::descriptor::Widget;
use postretro_ui::modal_stack::ScopeTier;

use super::super::tests::{app_with_glyphs, image_asset, set_family};
use super::*;
use crate::input::{Command, DeviceClass, PhysicalInput, PlayerLayer};

/// The marker shape: a passive row whose glyph shows only while `inRange`.
fn marker_root(word: &str) -> Widget {
    serde_json::from_value(serde_json::json!({
        "kind": "hstack", "gap": 4.0, "padding": 0.0, "align": "center",
        "visibleWhen": { "fact": "inRange" },
        "children": [
            { "kind": "text", "content": word, "fontSize": 14, "color": "ok" },
            { "kind": "glyph", "command": "nav_confirm", "id": "promptGlyph",
              "visibleWhen": { "fact": "inRange" } },
            { "kind": "glyph", "command": "dash" },
        ],
    }))
    .unwrap()
}

fn template(word: &str) -> PresentationTemplate {
    serde_json::from_value(serde_json::json!({
        "id": "reviveMarker",
        "root": serde_json::to_value(marker_root(word)).unwrap(),
        "lifetimeMs": 1, "motion": { "rise": 0, "easing": "linear" },
        "fade": { "startMs": 1 }, "spawnScatter": { "radius": 0 },
        "worldAnchor": { "offsetY": 0.6 },
    }))
    .unwrap()
}

fn session(app: &mut App) -> &mut crate::session::Session {
    app.session.as_mut().unwrap()
}

/// The resolved glyph of the one template, when the templates re-send.
fn resend(app: &mut App) -> Option<Widget> {
    let mut templates = stale_resolved_templates(session(app))?;
    assert_eq!(templates.len(), 1);
    let Widget::HStack(row) = templates.remove(0).root else {
        panic!("the marker root stays an hstack");
    };
    Some(row.children[1].clone())
}

/// The template's `dash` glyph, resolved now (whether or not it would re-send).
fn dash(app: &mut App) -> Widget {
    session(app).glyph_art.templates.resolved_for = None;
    let mut templates = stale_resolved_templates(session(app)).unwrap();
    let Widget::HStack(row) = templates.remove(0).root else {
        panic!("hstack");
    };
    row.children[2].clone()
}

/// The same subtree as a HUD tree, resolved by the snapshot path.
fn hud_glyph(app: &App, word: &str) -> Widget {
    let mut snapshot = postretro_ui::UiReadSnapshot::with_trees(
        vec![postretro_ui::UiTreeEntry {
            name: "row".into(),
            tier: ScopeTier::Mod,
            descriptor: serde_json::from_value(serde_json::json!({
                "anchor": "center", "offset": [0.0, 0.0],
                "root": serde_json::to_value(marker_root(word)).unwrap(),
            }))
            .unwrap(),
            capture_mode: postretro_ui::descriptor::CaptureMode::Passthrough,
            on_commit: None,
        }],
        Default::default(),
        Default::default(),
        0.0,
        None,
    );
    super::super::resolve_snapshot_glyphs(&mut snapshot, app.session.as_ref().unwrap());
    let Widget::HStack(row) = &snapshot.trees[0].descriptor.root else {
        panic!("row is an hstack");
    };
    row.children[1].clone()
}

fn rebind_confirm_to_west(app: &mut App) {
    let mut player = PlayerLayer::default();
    player.rows.insert(
        (Command::NavConfirm, DeviceClass::Gamepad),
        vec![Some(PhysicalInput::GamepadButton(Button::West))],
    );
    session(app).bindings.set_player_layer(player);
    app.refresh_effective_bindings();
}

/// R4: a template glyph draws what the HUD glyph draws for the same command
/// and family, keeps its `visibleWhen`, and follows a device-family switch and
/// a rebinding with no reload.
#[test]
fn revive_marker_r4_template_glyph_matches_the_hud_glyph_and_follows_input() {
    let mut app = app_with_glyphs();
    set_family(&mut app, Some(0x045E));
    session(&mut app)
        .glyph_art
        .templates
        .install(vec![template("Hold")]);
    let glyph = resend(&mut app).expect("an install sends");
    assert_eq!(glyph, hud_glyph(&app, "Hold"));
    assert_eq!(image_asset(&glyph), Some("ui/glyphs/xbox/south"));
    let Widget::Image(image) = &glyph else {
        unreachable!()
    };
    assert_eq!(image.id.as_deref(), Some("promptGlyph"));
    assert!(image.visible_when.is_some(), "visibleWhen survives");
    assert!(
        matches!(dash(&mut app), Widget::Spacer(_)),
        "an irrelevant command draws nothing, as on the HUD"
    );
    assert!(
        resend(&mut app).is_none(),
        "nothing changed, nothing re-sends"
    );

    set_family(&mut app, Some(0x054C));
    let glyph = resend(&mut app).expect("a family switch re-sends");
    assert_eq!(image_asset(&glyph), Some("ui/glyphs/ps/south"));
    assert_eq!(glyph, hud_glyph(&app, "Hold"));

    set_family(&mut app, Some(0x045E));
    let _ = resend(&mut app);
    rebind_confirm_to_west(&mut app);
    let glyph = resend(&mut app).expect("a rebinding re-sends");
    assert_eq!(image_asset(&glyph), Some("ui/glyphs/xbox/west"));
    assert_eq!(glyph, hud_glyph(&app, "Hold"));

    // Without art for the family, the label.
    session(&mut app).device_family.note_keyboard_mouse();
    session(&mut app).device_family.end_frame();
    let glyph = resend(&mut app).expect("a family switch re-sends");
    assert!(matches!(&glyph, Widget::Text(text) if text.content == "ENTER"));
    assert_eq!(glyph, hud_glyph(&app, "Hold"));
}

/// R15: glyph art that finishes loading after the templates were installed
/// re-sends them with the art, with no binding or family change between.
#[test]
fn revive_marker_r15_art_loaded_after_install_resends_the_templates() {
    let mut app = app_with_glyphs();
    set_family(&mut app, Some(0x045E));
    let art = std::mem::take(&mut session(&mut app).glyph_art.keys);
    session(&mut app)
        .glyph_art
        .templates
        .install(vec![template("Hold")]);
    let before = resend(&mut app).unwrap();
    assert!(matches!(before, Widget::Text(_)), "no art yet: the label");

    // What a completed art load leaves behind.
    let glyph_art = &mut session(&mut app).glyph_art;
    glyph_art.keys = art;
    glyph_art.art_generation += 1;
    let after = resend(&mut app).expect("the art load re-sends");
    assert_eq!(image_asset(&after), Some("ui/glyphs/xbox/south"));
    assert_eq!(after, hud_glyph(&app, "Hold"));
}

/// R15: a staged reload that edits only the template (no binding change, so
/// no binding generation bump) sends the new template already resolved.
#[test]
fn revive_marker_r15_template_only_reload_sends_resolved_glyphs() {
    let mut app = app_with_glyphs();
    set_family(&mut app, Some(0x045E));
    session(&mut app)
        .glyph_art
        .templates
        .install(vec![template("Hold")]);
    let _ = resend(&mut app).unwrap();
    let bindings = session(&mut app).bindings.generation();

    session(&mut app)
        .glyph_art
        .templates
        .install(vec![template("Press")]);
    let mut templates = stale_resolved_templates(session(&mut app)).expect("a reload sends");
    assert_eq!(session(&mut app).bindings.generation(), bindings);
    let Widget::HStack(row) = templates.remove(0).root else {
        panic!("hstack");
    };
    assert!(matches!(&row.children[0], Widget::Text(text) if text.content == "Press"));
    assert_eq!(image_asset(&row.children[1]), Some("ui/glyphs/xbox/south"));
}

/// A template set with no glyph re-sends only on install or for a new
/// renderer; art re-registration alone leaves it.
#[test]
fn templates_without_glyphs_never_resend_on_input_changes() {
    let mut app = app_with_glyphs();
    let mut plain = template("Hold");
    plain.root = serde_json::from_value(serde_json::json!({
        "kind": "text", "content": "DOWNED", "fontSize": 14, "color": "ok",
    }))
    .unwrap();
    assert!(
        stale_resolved_templates(session(&mut app)).is_none(),
        "nothing installed"
    );
    session(&mut app).glyph_art.templates.install(vec![plain]);
    assert!(stale_resolved_templates(session(&mut app)).is_some());
    set_family(&mut app, Some(0x054C));
    rebind_confirm_to_west(&mut app);
    assert!(stale_resolved_templates(session(&mut app)).is_none());

    session(&mut app).glyph_art.invalidate();
    assert!(
        stale_resolved_templates(session(&mut app)).is_none(),
        "re-registered art does not re-send a glyph-free set"
    );
    session(&mut app).glyph_art.forget_renderer();
    assert!(
        stale_resolved_templates(session(&mut app)).is_some(),
        "a new renderer receives every set"
    );
}
