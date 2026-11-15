// `forEach` over an engine list in both SDK runtimes: one row per capacity
// index, shown while present, with row-suffixed ids and same-row references.
// See: context/lib/ui.md §1.1 (Repeated rows)
use super::*;

const TYPESCRIPT_FIXTURE: &str = r#"
    import { getGameState } from "postretro";
    import { Button, HStack, Text, bindState, forEach, stateEquals } from "postretro/ui";
    const { session } = getGameState();
    const rows = forEach(session.players, (p) =>
      HStack({ id: "row", gap: 4, focusNeighbors: { down: "kick", up: "outside" } }, [
        Text({ id: "name", content: "", bind: bindState(p.name) }),
        Button({ id: "kick", labelledBy: "name", onPress: "kick" }),
      ]));
    const gated = forEach(session.players, (p) =>
      Text({ content: "down", visibleWhen: stateEquals(p.status, "down") }));
    let rejected = "accepted";
    try { (forEach as any)([], () => Text({ content: "x" })); } catch (error) { rejected = String((error as Error).message); }
    JSON.stringify({ rows, gated, rejected });
"#;

const LUAU_FIXTURE: &str = r#"
    local UI = require("postretro/ui")
    local session = getGameState().session
    local rows = UI.forEach(session.players, function(p)
      return UI.HStack({ id = "row", gap = 4, focusNeighbors = { down = "kick", up = "outside" } }, {
        UI.Text({ id = "name", content = "", bind = UI.bindState(p.name) }),
        UI.Button({ id = "kick", labelledBy = "name", onPress = "kick" }),
      })
    end)
    local gated = UI.forEach(session.players, function(p)
      return UI.Text({ content = "down", visibleWhen = UI.stateEquals(p.status, "down") })
    end)
    local ok, message = pcall(function() return UI.forEach({}, function() return UI.Text({ content = "x" }) end) end)
    local rejected = if ok then "accepted" else tostring(message)
    return { rows = rows, gated = gated, rejected = string.match(rejected, "forEach: .*") }
"#;

/// L6: `forEach` builds one row per capacity index, each shown only while its
/// row is present (and its own `visibleWhen` holds, through a wrapping stack),
/// suffixes every authored id with its row and points same-row `labelledBy` /
/// `focusNeighbors` at the row's copies while an outside target stays. The
/// tree is fixed at build: it carries every row whatever the live values.
#[test]
fn for_each_unrolls_rows_with_suffixed_ids_and_matches_across_runtimes() {
    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(typescript, luau);

    let capacity = postretro_foundation::seat::SESSION_PLAYER_CAPACITY;
    let rows = typescript["rows"].as_array().unwrap();
    assert_eq!(rows.len(), capacity);
    let row = &rows[3];
    assert_eq!(row["id"], "row.3");
    assert_eq!(
        row["visibleWhen"],
        serde_json::json!({ "slot": "session.players.3.present", "equals": true })
    );
    assert_eq!(
        row["focusNeighbors"],
        serde_json::json!({ "down": "kick.3", "up": "outside" })
    );
    assert_eq!(row["children"][0]["id"], "name.3");
    assert_eq!(row["children"][0]["bind"]["slot"], "session.players.3.name");
    assert_eq!(row["children"][1]["id"], "kick.3");
    assert_eq!(row["children"][1]["labelledBy"], "name.3");

    let gated = typescript["gated"].as_array().unwrap();
    assert_eq!(gated.len(), capacity);
    assert_eq!(gated[5]["kind"], "vstack");
    assert_eq!(
        gated[5]["visibleWhen"],
        serde_json::json!({ "slot": "session.players.5.present", "equals": true })
    );
    assert_eq!(
        gated[5]["children"][0]["visibleWhen"],
        serde_json::json!({ "slot": "session.players.5.status", "equals": "down" })
    );
    assert!(
        typescript["rejected"]
            .as_str()
            .unwrap()
            .starts_with("forEach: `list` must be an engine state list")
    );

    // Every row crosses the Rust descriptor boundary.
    for widget in rows.iter().chain(gated) {
        let parsed: crate::ui::descriptor::Widget = serde_json::from_value(widget.clone())
            .unwrap_or_else(|error| panic!("{widget} is not a widget: {error}"));
        assert_eq!(serde_json::to_value(&parsed).unwrap()["kind"], widget["kind"]);
    }
}
