// `grantLayer` binds as an in-tick consequential command; a bad layer
// reference never binds.
// See: context/lib/scripting.md §12 (Entity addressing)

use postretro_entities::SlotTable;
use postretro_test_log_capture::LogCapture;

use super::command_binding::bind_command;
use crate::trigger_commands::{BoundTarget, BoundTriggerCommand, BoundTriggerCommandKind};

fn bind(args: serde_json::Value) -> Option<BoundTriggerCommand> {
    bind_command(
        "grantLayer",
        Some(BoundTarget::Activators),
        &args,
        &SlotTable::new(),
        None,
    )
}

#[test]
fn grant_layer_binds_an_in_tick_command_with_an_optional_amount() {
    let shield = serde_json::json!({ "kind": "defenseLayer", "name": "shield" });
    let fill = bind(serde_json::json!({ "layer": shield })).expect("fill binds");
    assert_eq!(fill.kind(), BoundTriggerCommandKind::GrantLayer);
    let BoundTriggerCommand::GrantLayer { layer, amount, .. } = fill else {
        unreachable!()
    };
    assert_eq!((layer.as_str(), amount), ("shield", None));

    let add = bind(serde_json::json!({ "layer": shield, "amount": -1.0 }))
        .expect("a negative amount reaches the chokepoint to reject");
    let BoundTriggerCommand::GrantLayer { amount, .. } = add else {
        unreachable!()
    };
    assert_eq!(amount, Some(-1.0));
}

#[test]
fn grant_layer_with_a_bad_reference_does_not_bind() {
    let capture = LogCapture::start();
    for args in [
        serde_json::json!({ "layer": "shield" }),
        serde_json::json!({ "layer": { "kind": "damageType", "name": "shock" } }),
        serde_json::json!({ "layer": { "kind": "defenseLayer", "name": "shield" }, "extra": 1 }),
    ] {
        assert!(bind(args.clone()).is_none(), "{args}");
    }
    capture.assert_logged(
        log::Level::Warn,
        "[Trigger] grantLayer has a bad layer reference",
    );
    capture.assert_logged(log::Level::Warn, "[Trigger] grantLayer has invalid args");
}
