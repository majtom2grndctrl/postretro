// Player-event authoring surface gates: seats never reach the SDK, and the
// engine player state tree stays a tree of refs with no methods.
// See: context/lib/scripting.md §12 (Player events)

use super::*;
use postretro_scripting_core::game_state_refs::{
    GAME_STATE_BRIDGE_GLOBAL, install_luau_bridge, install_quickjs_bridge,
};
use std::path::{Path, PathBuf};

/// Seat identifiers the authoring surface must never name. Prose uses the
/// English word "seat"; only these identifier spellings, and `seat` as a
/// field (`seat:`), are seat identities.
const SEAT_IDENTIFIERS: &[&str] = &["Seat", "SeatId", "seatId"];

/// Every seat identifier in `source`, as `line: token`.
fn seat_identifiers(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (number, line) in source.lines().enumerate() {
        let bytes = line.as_bytes();
        let mut start = 0;
        while start < bytes.len() {
            if !(bytes[start].is_ascii_alphanumeric() || bytes[start] == b'_') {
                start += 1;
                continue;
            }
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            let token = &line[start..end];
            let field = token == "seat" && line[end..].trim_start().starts_with(':');
            if field || SEAT_IDENTIFIERS.contains(&token) {
                found.push(format!("{}: {}", number + 1, line.trim()));
            }
            start = end;
        }
    }
    found
}

fn sdk_lib_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("sdk/lib reads") {
        let path = entry.expect("sdk/lib entry").path();
        if path.is_dir() {
            sdk_lib_sources(&path, out);
        } else if path
            .extension()
            .is_some_and(|ext| ext == "ts" || ext == "luau")
        {
            out.push(path);
        }
    }
}

#[test]
fn seat_identifiers_stay_off_the_sdk_surface_and_the_player_state_tree_has_no_methods() {
    use crate::scripting::typedef::register_all;
    use postretro_entities::ctx::ScriptCtx;

    // The scanner catches what it must, and leaves prose and `openSeats` alone.
    assert_eq!(
        seat_identifiers("let s: Seat = x;\n{ seat: 1 }\nseatId").len(),
        3
    );
    assert!(seat_identifiers("one value per player seat. session.openSeats: x").is_empty());

    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ScriptCtx::new());
    let mut surfaces = vec![
        (
            "generated postretro.d.ts".to_string(),
            generate_typescript(&registry),
        ),
        (
            "generated postretro.d.luau".to_string(),
            generate_luau(&registry),
        ),
    ];
    let mut sources = Vec::new();
    sdk_lib_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/lib"),
        &mut sources,
    );
    assert!(
        sources
            .iter()
            .any(|path| path.ends_with("data_script/player_events.ts"))
            && sources
                .iter()
                .any(|path| path.ends_with("data_script/player_events.luau")),
        "the scan covers both runtimes' player-event surface"
    );
    for path in sources {
        let text = std::fs::read_to_string(&path).expect("sdk source reads");
        surfaces.push((path.display().to_string(), text));
    }
    for (name, text) in &surfaces {
        let hits = seat_identifiers(text);
        assert!(hits.is_empty(), "{name} names a seat identifier: {hits:?}");
    }

    // QuickJS: interior nodes hold no functions (enumerable or not); a leaf
    // enumerates exactly `slot` and `kind`.
    let runtime = rquickjs::Runtime::new().unwrap();
    let context = rquickjs::Context::full(&runtime).unwrap();
    context.with(|ctx| {
        install_quickjs_bridge(&ctx).unwrap();
        let violations: Vec<String> = ctx
            .eval(format!(
                r#"
                (() => {{
                  const out = [];
                  function walk(node, path) {{
                    if (typeof node.slot === "string") {{
                      const keys = Object.keys(node).sort().join(",");
                      if (keys !== "kind,slot") out.push(`${{path}} enumerates ${{keys}}`);
                      return;
                    }}
                    for (const key of Object.getOwnPropertyNames(node)) {{
                      const child = node[key];
                      if (typeof child === "function") out.push(`${{path}}.${{key}} is a method`);
                      else walk(child, path ? `${{path}}.${{key}}` : key);
                    }}
                  }}
                  walk(globalThis.{GAME_STATE_BRIDGE_GLOBAL}, "");
                  return out;
                }})()
                "#
            ))
            .unwrap();
        assert!(violations.is_empty(), "QuickJS state tree: {violations:?}");
    });

    // Luau: interior tables hold no functions and carry no metatable; a leaf
    // iterates exactly `slot` and `kind`.
    let lua = mlua::Lua::new();
    install_luau_bridge(&lua).unwrap();
    let violations: Vec<String> = lua
        .load(format!(
            r#"
            local out = {{}}
            local function walk(node, path)
              if type(rawget(node, "slot")) == "string" then
                local keys = {{}}
                for key in pairs(node) do table.insert(keys, key) end
                table.sort(keys)
                local joined = table.concat(keys, ",")
                if joined ~= "kind,slot" then table.insert(out, path .. " iterates " .. joined) end
                return
              end
              if getmetatable(node) ~= nil then table.insert(out, path .. " has a metatable") end
              for key, child in pairs(node) do
                local childPath = if path == "" then key else path .. "." .. key
                if type(child) == "function" then
                  table.insert(out, childPath .. " is a method")
                else
                  walk(child, childPath)
                end
              end
            end
            walk({GAME_STATE_BRIDGE_GLOBAL}, "")
            return out
            "#
        ))
        .eval()
        .unwrap();
    assert!(violations.is_empty(), "Luau state tree: {violations:?}");
}
