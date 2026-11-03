// Dual-runtime parity for the UI `stateCompare` predicate constructor: the
// TypeScript and Luau SDKs lower it to byte-identical descriptor wire and
// reject the same misuse with the same message.
// See: context/lib/ui.md §3

use super::parity_tests::{luau_fixture_value, quickjs_fixture_value};

const TYPESCRIPT_FIXTURE: &str = r#"
    import { Text, VStack, bindState, getGameState, stateCompare } from "postretro/ui";

    const { player } = getGameState();
    const attempt = (build: () => unknown): string => {
      try {
        build();
        return "accepted";
      } catch (error) {
        return (error as Error).message;
      }
    };
    JSON.stringify({
      above: stateCompare(player.assistProgress, "greaterThan", 0),
      below: stateCompare(player.health, "lessThan", 25),
      atLeast: stateCompare(bindState(player.assistReceivedProgress), "atLeast", 0.5),
      atMost: stateCompare(player.health, "atMost", 100),
      nonNumber: attempt(() => stateCompare(player.lifeState as never, "greaterThan", 0)),
      badOp: attempt(() => stateCompare(player.health, "gt" as never, 0)),
      nan: attempt(() => stateCompare(player.health, "greaterThan", 0 / 0)),
      infinite: attempt(() => stateCompare(player.health, "greaterThan", 1 / 0)),
      stack: VStack({ visibleWhen: stateCompare(player.assistProgress, "greaterThan", 0) }, [Text({ content: "REVIVING" })]),
      label: Text({ content: "REVIVING", visibleWhen: stateCompare(player.assistProgress, "greaterThan", 0) }),
      factoryInfinite: attempt(() =>
        VStack({ visibleWhen: { slot: "player.health", compare: { op: "atLeast", value: 1 / 0 } } as never }, [Text({ content: "x" })]),
      ),
      factoryBadOp: attempt(() =>
        Text({ content: "x", visibleWhen: { slot: "player.health", compare: { op: "gt", value: 0 } } as never }),
      ),
      factoryBoth: attempt(() =>
        Text({ content: "x", visibleWhen: { slot: "player.health", equals: 1, compare: { op: "atLeast", value: 0 } } as never }),
      ),
    });
"#;

const LUAU_FIXTURE: &str = r#"
    local UI = require("postretro/ui")

    local player = UI.getGameState().player
    local function attempt(build)
      local ok, message = pcall(build)
      if ok then
        return "accepted"
      end
      -- Strip Luau's `chunk:line: ` position prefix; the message text is the contract.
      return (string.gsub(tostring(message), "^[^:]*:%d+: ", ""))
    end
    return {
      above = UI.stateCompare(player.assistProgress, "greaterThan", 0),
      below = UI.stateCompare(player.health, "lessThan", 25),
      atLeast = UI.stateCompare(UI.bindState(player.assistReceivedProgress), "atLeast", 0.5),
      atMost = UI.stateCompare(player.health, "atMost", 100),
      nonNumber = attempt(function() return UI.stateCompare(player.lifeState, "greaterThan", 0) end),
      badOp = attempt(function() return UI.stateCompare(player.health, "gt", 0) end),
      nan = attempt(function() return UI.stateCompare(player.health, "greaterThan", 0 / 0) end),
      infinite = attempt(function() return UI.stateCompare(player.health, "greaterThan", 1 / 0) end),
      stack = UI.VStack({ visibleWhen = UI.stateCompare(player.assistProgress, "greaterThan", 0) }, { UI.Text({ content = "REVIVING" }) }),
      label = UI.Text({ content = "REVIVING", visibleWhen = UI.stateCompare(player.assistProgress, "greaterThan", 0) }),
      factoryInfinite = attempt(function()
        return UI.VStack({ visibleWhen = { slot = "player.health", compare = { op = "atLeast", value = 1 / 0 } } }, { UI.Text({ content = "x" }) })
      end),
      factoryBadOp = attempt(function()
        return UI.Text({ content = "x", visibleWhen = { slot = "player.health", compare = { op = "gt", value = 0 } } })
      end),
      factoryBoth = attempt(function()
        return UI.Text({ content = "x", visibleWhen = { slot = "player.health", equals = 1, compare = { op = "atLeast", value = 0 } } })
      end),
    }
"#;

#[test]
fn state_compare_lowers_and_rejects_identically_across_authoring_runtimes() {
    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(
        typescript, luau,
        "stateCompare wire diverged across runtimes"
    );

    assert_eq!(
        typescript["above"],
        serde_json::json!({
            "slot": "player.assistProgress",
            "compare": { "op": "greaterThan", "value": 0 },
        }),
        "the predicate wire is the slot plus one `compare` object; SDK `kind` never crosses",
    );
    assert_eq!(
        typescript["atLeast"]["slot"], "player.assistReceivedProgress",
        "a bindState-decorated number ref keeps its kind for stateCompare",
    );
    assert_eq!(
        typescript["nonNumber"],
        "stateCompare: `player.lifeState` is not a number state reference"
    );
    assert_eq!(
        typescript["badOp"],
        "stateCompare: `op` must be one of greaterThan, lessThan, atLeast, atMost"
    );
    let gate = serde_json::json!({
        "slot": "player.assistProgress",
        "compare": { "op": "greaterThan", "value": 0 },
    });
    assert_eq!(
        typescript["stack"]["visibleWhen"], gate,
        "container factories keep `compare`"
    );
    assert_eq!(
        typescript["label"]["visibleWhen"], gate,
        "widget factories keep `compare`"
    );
    assert_eq!(
        typescript["factoryInfinite"],
        "VStack: `visibleWhen.compare.value` must be a finite number"
    );
    assert_eq!(
        typescript["factoryBadOp"],
        "Text: `visibleWhen.compare.op` must be one of greaterThan, lessThan, atLeast, atMost"
    );
    assert_eq!(
        typescript["factoryBoth"],
        "Text: `visibleWhen` cannot carry both `equals` and `compare`"
    );
    for key in ["nan", "infinite"] {
        assert_eq!(
            typescript[key], "stateCompare: `value` must be a finite number",
            "{key}"
        );
    }
}
