// Author payload validation deliberately leaves respawn health policy to execution.
use super::validate_consequential_args;
use serde_json::json;

#[test]
fn life_state_payloads_validate_names_and_leave_health_normalization_to_respawn() {
    for (primitive, args) in [
        ("setLifeState", json!({ "state": "down" })),
        ("respawn", json!({})),
        ("respawn", json!({ "health": -1, "state": "alive" })),
        ("respawn", json!({ "health": 1e40 })),
    ] {
        validate_consequential_args("revive", "primitive", primitive, &args).unwrap();
    }
    for (primitive, args, path) in [
        ("setLifeState", json!({}), "args.state"),
        (
            "setLifeState",
            json!({ "state": "bad state" }),
            "args.state",
        ),
        ("respawn", json!({ "state": 12 }), "args.state"),
        ("respawn", json!({ "state": "" }), "args.state"),
        ("respawn", json!({ "health": "full" }), "args.health"),
    ] {
        let error =
            validate_consequential_args("revive", "sequence step 2", primitive, &args).unwrap_err();
        assert!(
            error.contains("reaction `revive` sequence step 2"),
            "{error}"
        );
        assert!(error.contains(path), "{error}");
    }
}
