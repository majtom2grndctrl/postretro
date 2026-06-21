// Tests: announce widgets + frontend drains.

use super::super::*;
use super::common::*;

#[test]
fn announce_widget_js_rejects_empty_text() {
    let src = r#"({ anchor:"center", offset:[0.0,0.0], root:{ kind:"announce", text:"" } })"#;
    let err = eval_js(src, |ctx, v| {
        anchored_tree_from_js_value(ctx, v).unwrap_err()
    });
    assert!(
        matches!(err, DescriptorError::InvalidShape { ref reason } if reason.contains("announce.text")),
        "empty announce text must be a named InvalidShape error, got {err:?}"
    );
}

#[test]
fn announce_widget_lua_rejects_empty_text() {
    let src = r#"return { anchor="center", offset={0.0,0.0}, root={ kind="announce", text="" } }"#;
    let err = eval_lua(src, |v| anchored_tree_from_lua_value(v).unwrap_err());
    assert!(
        matches!(err, DescriptorError::InvalidShape { ref reason } if reason.contains("announce.text")),
        "empty announce text must be a named InvalidShape error, got {err:?}"
    );
}

#[test]
fn drain_frontend_js_parses_menu_background_and_camera() {
    let frontend = eval_js(
        r#"({
            frontend: {
                menuTree: "mainMenu",
                backgroundLevel: "menu_backdrop",
                camera: { position: [4, 2, 8], yaw: -0.6, pitch: -0.1 },
            },
        })"#,
        |_ctx, v| {
            let obj = Object::from_value(v).expect("must be an object");
            drain_frontend_js(&obj, "test").expect("frontend must parse")
        },
    )
    .expect("frontend present");

    assert_eq!(frontend.menu_tree, "mainMenu");
    assert_eq!(frontend.background_level.as_deref(), Some("menu_backdrop"));
    assert_eq!(frontend.camera.position, [4.0, 2.0, 8.0]);
    assert_eq!(frontend.camera.yaw, -0.6);
    assert_eq!(frontend.camera.pitch, -0.1);
}

#[test]
fn drain_frontend_js_rejects_structurally_invalid_frontend() {
    let result = eval_js(r#"({ frontend: 5 })"#, |_ctx, v| {
        let obj = Object::from_value(v).expect("must be an object");
        drain_frontend_js(&obj, "test")
    });

    assert!(
        result.is_err(),
        "a present non-object frontend block must abort mod-init"
    );
}

#[test]
fn drain_frontend_lua_parses_menu_background_and_camera() {
    let lua = mlua::Lua::new();
    let value: mlua::Value = lua
        .load(
            r#"return {
                frontend = {
                    menuTree = "mainMenu",
                    backgroundLevel = "menu_backdrop",
                    camera = { position = {4, 2, 8}, yaw = -0.6, pitch = -0.1 },
                },
            }"#,
        )
        .eval()
        .unwrap();
    let LuaValue::Table(table) = value else {
        panic!("expected table");
    };
    let frontend = drain_frontend_lua(&table, "test")
        .expect("frontend must parse")
        .expect("frontend present");

    assert_eq!(frontend.menu_tree, "mainMenu");
    assert_eq!(frontend.background_level.as_deref(), Some("menu_backdrop"));
    assert_eq!(frontend.camera.position, [4.0, 2.0, 8.0]);
    assert_eq!(frontend.camera.yaw, -0.6);
    assert_eq!(frontend.camera.pitch, -0.1);
}
