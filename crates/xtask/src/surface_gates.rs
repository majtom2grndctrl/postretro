// Repository grep gates for the scripting addressing surface: retired
// spellings stay out of authored content, the SDK and the docs, and
// `NetworkId` stays out of every scripting layer. Run by preflight's
// `cargo test`, beside `layering_invariants_hold`.
// See: context/lib/scripting.md §12 (Entity addressing)

use std::path::{Path, PathBuf};
use std::process::Command;

/// Text files a gate reads. Binary assets (textures, sounds, baked levels)
/// are skipped by extension, so a walk over `content/` stays cheap.
const TEXT_EXTENSIONS: &[&str] = &[
    "rs", "ts", "js", "luau", "md", "map", "fgd", "json", "toml", "txt", "cfg",
];

/// Spellings the SDK addressing model retired: the `world` namespace, the
/// tag-keyed group handles, the old NPC state primitive, the tag-keyed level
/// trigger-event builder, and the free trigger verbs. Raw wire descriptors
/// name `"armTrigger"` as a string, without a call, and stay legal.
const RETIRED_SPELLINGS: &[&str] = &[
    "world.query",
    "world:query",
    "world.getGravity",
    "world.setGravity",
    "world:getGravity",
    "world:setGravity",
    "enemies(",
    "spawner(",
    "updateEnemyState",
    "onTriggerEvent",
    "armTrigger(",
    "disarmTrigger(",
];

/// Free target-taking verbs: retired as free calls with a string or token
/// target. The same verbs as methods (`players().grantAmmo(…)`,
/// `on.activators.damage(…)`, `impact.source.grantAmmo(…)`) are the surface.
const RETIRED_FREE_VERBS: &[&str] = &["damage", "grantHealth", "grantAmmo", "addSlot"];

/// Directories the retired-spelling gate covers, relative to the workspace.
const AUTHORED_ROOTS: &[&str] = &["content", "sdk", "docs", "context/lib"];

pub(crate) fn workspace_root() -> PathBuf {
    crate::workspace_root().expect("workspace root")
}

fn is_text_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| TEXT_EXTENSIONS.contains(&ext))
}

/// Every text file under `root`, recursively, in a stable order.
pub(crate) fn text_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if is_text_file(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

pub(crate) fn line_of(text: &str, byte: usize) -> usize {
    text[..byte].matches('\n').count() + 1
}

pub(crate) fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte offsets of free calls `verb(` whose first argument is a string or a
/// token sentinel: `(damage|grantHealth|grantAmmo|addSlot)\(\s*["'@]`, with no
/// `.`, `:` or identifier character before the verb. The preceding-character
/// rule exempts the mandated method forms.
fn free_verb_calls(text: &str) -> Vec<usize> {
    let mut hits = Vec::new();
    for verb in RETIRED_FREE_VERBS {
        let call = format!("{verb}(");
        for (at, _) in text.match_indices(&call) {
            let preceded_by_receiver = text[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c == '.' || c == ':' || is_identifier_char(c));
            if preceded_by_receiver {
                continue;
            }
            let argument = text[at + call.len()..].trim_start();
            if argument.starts_with(['"', '\'', '@']) {
                hits.push(at);
            }
        }
    }
    hits.sort_unstable();
    hits
}

/// Byte offsets of `"applyDamage"` string literals whose enclosing descriptor
/// object names `player` — a hand-written player-damage descriptor that
/// `players().damage(…)` or `on.activators.damage(…)` now spells.
fn player_damage_descriptors(text: &str) -> Vec<usize> {
    text.match_indices("\"applyDamage\"")
        .filter(|(at, _)| {
            let start = text[..*at].rfind('{').map_or(0, |open| open + 1);
            let end = text[*at..].find('}').map_or(text.len(), |close| at + close);
            text[start..end].contains("player")
        })
        .map(|(at, _)| at)
        .collect()
}

pub(crate) fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

// None of the retired spellings remains in authored content, the SDK, the
// human docs or the context library. Raw `armTrigger` wire descriptors
// (string, no call) pass.
#[test]
fn retired_addressing_spellings_stay_out_of_content_sdk_and_docs() {
    let root = workspace_root();
    let mut violations = Vec::new();
    for authored in AUTHORED_ROOTS {
        for path in text_files(&root.join(authored)) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let file = relative(&root, &path);
            for spelling in RETIRED_SPELLINGS {
                for (at, _) in text.match_indices(spelling) {
                    violations.push(format!("{file}:{}: `{spelling}`", line_of(&text, at)));
                }
            }
            for at in free_verb_calls(&text) {
                let call: String = text[at..].chars().take_while(|&c| c != '\n').collect();
                violations.push(format!(
                    "{file}:{}: free verb call `{call}`",
                    line_of(&text, at)
                ));
            }
            if *authored == "content" && !file.ends_with(".md") {
                for at in player_damage_descriptors(&text) {
                    violations.push(format!(
                        "{file}:{}: hand-written `applyDamage` descriptor targeting players",
                        line_of(&text, at)
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "retired addressing spellings remain (scripting.md §12):\n{}",
        violations.join("\n")
    );
}

// Half one: `NetworkId` never reaches scripts — no scripting crate, no
// `sdk/` file and no generated typedef (`sdk/types/`) names it.
#[test]
fn network_id_stays_out_of_every_scripting_layer() {
    let root = workspace_root();
    let mut scanned: Vec<PathBuf> = vec![
        root.join("crates/scripting-core"),
        root.join("crates/script-compiler"),
        root.join("sdk"),
    ];
    let sim_src = root.join("crates/sim/src");
    for entry in std::fs::read_dir(&sim_src)
        .expect("sim sources")
        .filter_map(Result::ok)
    {
        if entry.file_name().to_string_lossy().starts_with("scripting") {
            scanned.push(entry.path());
        }
    }
    assert!(
        scanned.iter().any(|path| path.ends_with("scripting")),
        "the sim scripting tree moved; repoint this gate"
    );

    let mut violations = Vec::new();
    for scope in &scanned {
        let files = if scope.is_dir() {
            text_files(scope)
        } else {
            vec![scope.clone()]
        };
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (at, _) in text.match_indices("NetworkId") {
                violations.push(format!("{}:{}", relative(&root, &path), line_of(&text, at)));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "`NetworkId` reached a scripting layer:\n{}",
        violations.join("\n")
    );
}

/// The dependency names a `Cargo.toml` section lists, in order.
fn section_dependencies(manifest: &str, section: &str) -> Vec<String> {
    let header = format!("[{section}]");
    let mut in_section = false;
    let mut names = Vec::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == header;
            continue;
        }
        if !in_section || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            names.push(name.trim().trim_matches('"').to_owned());
        }
    }
    names
}

/// Each `(field, type)` of `pub struct <name>` in `source`, comments and
/// attributes skipped.
fn struct_fields(source: &str, name: &str) -> Vec<(String, String)> {
    let opening = format!("pub struct {name} {{");
    let start = source
        .find(&opening)
        .unwrap_or_else(|| panic!("`{opening}` is in the source"))
        + opening.len();
    let body = &source[start..start + source[start..].find("\n}").expect("struct closes")];
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with("#["))
        .map(|line| {
            let field = line.trim_start_matches("pub ").trim_end_matches(',');
            let (name, ty) = field
                .split_once(':')
                .expect("a `name: Type` field per line");
            (name.trim().to_owned(), ty.trim().to_owned())
        })
        .collect()
}

/// Whether a field type is built only from strings and integers, through
/// `Option`, `Vec` and tuples.
fn is_names_and_integers(ty: &str) -> bool {
    const ALLOWED: &[&str] = &[
        "String", "Option", "Vec", "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64",
    ];
    ty.split(|c: char| !is_identifier_char(c))
        .filter(|word| !word.is_empty())
        .all(|word| ALLOWED.contains(&word))
        && ty
            .chars()
            .all(|c| is_identifier_char(c) || "<>(), ".contains(c))
}

// The carried loadout crosses the wire as a names-only mirror: `crates/net`
// takes no engine crate as a dependency, and `CarriedLoadoutWire` holds only
// strings and integers, so no engine id or float layout reaches the wire.
// Dev-dependencies are test-only and never enter the layering.
#[test]
fn net_loadout_wire_is_names_only() {
    let root = workspace_root();
    let manifest =
        std::fs::read_to_string(root.join("crates/net/Cargo.toml")).expect("net manifest");
    for section in ["dependencies", "build-dependencies"] {
        let engine: Vec<String> = section_dependencies(&manifest, section)
            .into_iter()
            .filter(|name| name.starts_with("postretro") || name == "glam")
            .collect();
        assert!(
            engine.is_empty(),
            "crates/net [{section}] names an engine crate: {engine:?}"
        );
    }
    assert!(
        section_dependencies(&manifest, "dependencies").contains(&"bitcode".to_owned()),
        "the manifest parse found net's dependencies"
    );

    let source = std::fs::read_to_string(root.join("crates/net/src/wire/control.rs"))
        .expect("net control wire");
    let fields = struct_fields(&source, "CarriedLoadoutWire");
    assert_eq!(
        fields.len(),
        7,
        "every carried loadout field is checked: {fields:?}"
    );
    let offending: Vec<&(String, String)> = fields
        .iter()
        .filter(|(_, ty)| !is_names_and_integers(ty))
        .collect();
    assert!(
        offending.is_empty(),
        "CarriedLoadoutWire fields must hold only strings and integers: {offending:?}"
    );
}

#[test]
fn names_and_integers_gate_refuses_floats_and_engine_types() {
    assert!(is_names_and_integers("Vec<(String, u32)>"));
    assert!(is_names_and_integers("Option<i32>"));
    assert!(!is_names_and_integers("Option<f32>"));
    assert!(!is_names_and_integers("Vec<EntityId>"));
    assert!(!is_names_and_integers("[u8; 16]"));
}

fn wire_version(handshake_source: &str) -> u32 {
    let line = handshake_source
        .lines()
        .find(|line| {
            line.trim_start()
                .starts_with("pub const WIRE_VERSION: u32 =")
        })
        .expect("handshake.rs declares WIRE_VERSION");
    line.split('=')
        .nth(1)
        .and_then(|value| value.trim().trim_end_matches(';').parse().ok())
        .expect("WIRE_VERSION is an integer literal")
}

// Half two: the addressing model changes manifest JSON only, so the net
// wire version matches `main`. Branch-scoped — a later brief may bump the
// wire on purpose — so it runs on demand:
// `cargo test -p xtask -- --ignored wire_version_matches_main`.
#[test]
#[ignore = "branch gate: compares against git `main`"]
fn wire_version_matches_main() {
    const HANDSHAKE: &str = "crates/net/src/handshake.rs";
    let root = workspace_root();
    let output = Command::new("git")
        .args(["show", &format!("main:{HANDSHAKE}")])
        .current_dir(&root)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git show main:{HANDSHAKE} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let on_main = wire_version(&String::from_utf8(output.stdout).expect("utf-8 source"));
    let here = wire_version(&std::fs::read_to_string(root.join(HANDSHAKE)).expect("handshake"));
    assert_eq!(here, on_main, "WIRE_VERSION moved relative to main");
}

#[test]
fn free_verb_gate_matches_free_calls_only() {
    let source = r#"
        damage("boss", 10);
        grantAmmo( 'players', 8);
        addSlot(@activators);
        players().grantAmmo("shells.buck", 8);
        on.activators:grantHealth("x");
        impact.source.grantAmmo("shells", 2);
        Postretro["damage"]("boss", 10);
        applyDamage("x");
        damage(5);
    "#;
    let hits: Vec<&str> = free_verb_calls(source)
        .into_iter()
        .map(|at| source[at..].split('(').next().unwrap())
        .collect();
    assert_eq!(hits, vec!["damage", "grantAmmo", "addSlot"]);
}

#[test]
fn player_damage_gate_flags_player_targets_only() {
    let source = r#"
        { primitive: "applyDamage", tag: "player", args: { amount: 5 } }
        { "primitive": "applyDamage", "kind": "player" }
        { primitive: "applyDamage", tag: "dummy", args: { amount: 5 } }
    "#;
    assert_eq!(player_damage_descriptors(source).len(), 2);
}
