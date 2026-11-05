// Source gate: a defense layer's value changes only at its named call sites —
// the damage chokepoint's drain, the tick-start recharge, the clamp and
// lifecycle refills (`settle`), the spawn fill, the hot-reload rebuild, the
// carry restore and the grant chokepoint. A new writer must join this list on
// purpose.
// See: context/lib/scripting.md §11 (Defense layers) · plans E16 §Acceptance (G15)

use std::path::{Path, PathBuf};

/// Every `(file, enclosing fn)` that may write a layer value. Each must still
/// hold a write, so the gate cannot pass by matching nothing.
const NAMED_CALL_SITES: &[(&str, &str)] = &[
    ("entities/src/components/defense_layers.rs", "fill"),
    ("entities/src/components/defense_layers.rs", "settle"),
    ("entities/src/components/defense_layers.rs", "refreshed"),
    ("entities/src/components/defense_layers.rs", "drain_hit"),
    ("entities/src/components/grant.rs", "grant_layer"),
    (
        "combat-model/src/carried_loadout.rs",
        "restore_carried_layers",
    ),
    ("sim/src/sim/defense_layers.rs", "recharge_defense_layers"),
    (
        "sim/src/scripting/builtins/data_archetype.rs",
        "attach_descriptor_components",
    ),
];

fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn is_test_file(relative: &str) -> bool {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    relative.contains("/tests/")
        || name == "tests.rs"
        || name.ends_with("_tests.rs")
        || name.ends_with("_test.rs")
}

/// Every non-test `.rs` file under `crates/*/src`, relative to `crates/`.
fn sources() -> Vec<(String, String)> {
    let root = crates_root();
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|entry| entry.path().join("src"))
        .filter(|path| path.is_dir())
        .collect();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let relative = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if !is_test_file(&relative) {
                    out.push((relative, std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
    }
    out
}

/// Whether `line` writes a layer value: an assignment to a `.value` place, a
/// `fill()`, or a `drain_layers` call, which drains the `&mut f32` values it
/// is handed. Only files that name a layer type are scanned, so other
/// `.value` fields pass.
fn writes_layer_value(line: &str) -> bool {
    let code = line.split("//").next().unwrap_or("");
    let assigns = |place: &str| {
        code.match_indices(place).any(|(at, _)| {
            // A resistance's `.value` is a parameter, not a pool's value.
            if code[..at].trim_end().ends_with("resist") {
                return false;
            }
            let rest = code[at + place.len()..].trim_start();
            ["=", "+=", "-=", "*=", "/="]
                .iter()
                .any(|op| rest.starts_with(op) && !rest.starts_with("=="))
        })
    };
    assigns(".value") || code.contains(".fill()") || code.contains("drain_layers(")
}

fn names_a_layer(source: &str) -> bool {
    [
        "DefenseLayer",
        "drain_layers",
        "PoolMultiplier",
        "recharged_value",
        "for_each_defense_layers_lent",
    ]
    .iter()
    .any(|name| source.contains(name))
}

/// The name of the last `fn` opened at or before `line`.
fn enclosing_fn(lines: &[&str], line: usize) -> Option<String> {
    lines[..=line].iter().rev().find_map(|text| {
        let at = text.find("fn ")?;
        let before = &text[..at];
        if !before.trim().is_empty()
            && !before.trim_end().ends_with("pub")
            && !before.trim_end().ends_with(')')
            && !before.trim_end().ends_with("const")
        {
            return None;
        }
        let name: String = text[at + 3..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    })
}

#[test]
fn layer_value_writes_stay_behind_named_call_sites() {
    let mut found = Vec::new();
    let mut strays = Vec::new();
    for (relative, source) in sources() {
        if !names_a_layer(&source) {
            continue;
        }
        let lines: Vec<&str> = source.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            // Inline test modules close each file; nothing past them ships.
            if line.trim() == "mod tests {"
                && index > 0
                && lines[index - 1].trim() == "#[cfg(test)]"
            {
                break;
            }
            if !writes_layer_value(line) {
                continue;
            }
            let function = enclosing_fn(&lines, index).unwrap_or_default();
            let site = NAMED_CALL_SITES
                .iter()
                .find(|(file, name)| relative.ends_with(file) && function == *name);
            match site {
                Some(site) => found.push(*site),
                None => strays.push(format!(
                    "{relative}:{} in `{function}`: {}",
                    index + 1,
                    line.trim()
                )),
            }
        }
    }
    assert!(
        strays.is_empty(),
        "layer values change only at the named call sites; add a new writer to \
         NAMED_CALL_SITES on purpose:\n{}",
        strays.join("\n")
    );
    for site in NAMED_CALL_SITES {
        assert!(
            found.contains(site),
            "named call site {site:?} no longer writes a layer value; update the gate"
        );
    }
}

#[test]
fn the_write_gate_flags_a_stray_writer() {
    assert!(writes_layer_value("    layer.value = 0.0;"));
    assert!(writes_layer_value("component.layers[0].value += 5.0;"));
    assert!(writes_layer_value(
        "    let outcome = drain_layers(raw, pools, 1.0);"
    ));
    assert!(writes_layer_value("    layers.fill();"));
    assert!(!writes_layer_value("    if layer.value == 0.0 {"));
    assert!(!writes_layer_value("    let value = layer.value;"));
    assert!(!writes_layer_value("    // layer.value = 0.0;"));
    assert!(!writes_layer_value("            resist.value = old.value;"));
    let lines = [
        "pub(crate) fn drain_hit(",
        "    let x = 1;",
        "    layer.value = 0.0;",
    ];
    assert_eq!(enclosing_fn(&lines, 2).as_deref(), Some("drain_hit"));
}
