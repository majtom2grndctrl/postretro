// Repository grep gates for enemy attacks and stances: shipped scripts, the
// human docs and the generated typedefs pass attacks as handles, never as
// strings; no wire record carries a behavior roll or hold release draw; and
// no guard name exposes the release draw. Run by preflight's `cargo test`,
// beside the scripting surface gates.
// See: context/lib/scripting.md §2 (Attack and stance declarations) · context/lib/networking.md

use std::path::Path;

use crate::surface_gates::{is_identifier_char, line_of, relative, text_files, workspace_root};

/// Where shipped attack references live: authored content, the SDK's
/// reference behaviors and templates, the human docs, and the generated
/// typedefs whose comments modders read.
const ATTACK_SURFACE_ROOTS: &[&str] = &[
    "content",
    "sdk/behaviors",
    "sdk/templates",
    "docs",
    "sdk/types",
];

/// Extensions the attack gate reads. A compiled `.js` bundle is build output
/// of the `.ts` beside it, so it is skipped.
const ATTACK_SURFACE_EXTENSIONS: &[&str] = &["ts", "luau", "md", "json"];

/// Wire crates: every record and message a peer sends is declared here.
const WIRE_SOURCE_ROOTS: &[&str] = &["crates/net/src", "crates/netcode/src"];

/// Stems of the host-only draw state, matched case-blind with `_` removed,
/// so `activity_roll`, `activityRoll` and `BehaviorDraws` all hit.
const HOST_DRAW_STEMS: &[&str] = &["activityroll", "behaviordraw", "holdrelease", "drawseed"];

/// Identifiers in the wire crates that share a stem but name something else.
const HOST_DRAW_EXCLUSIONS: &[&str] = &[
    // The player weapon gate's charge release, held over a vault
    // (`crates/sim/src/weapon/pawn_gate.rs`): a pawn's input, not an
    // enemy's hold.
    "hold_release_over_gate",
];

/// Declares `BRAIN_INPUTS` and the guard input prefixes.
const BRAIN_SOURCE: &str = "crates/foundation/src/brain.rs";

/// The SDK guard leaves: `brain.*`, `candidate.*` and the guard functions.
const SDK_GUARD_SOURCES: &[&str] = &["sdk/lib/brain.ts", "sdk/lib/brain.luau"];

/// Generated typedefs.
const SDK_TYPEDEFS: &str = "sdk/types";

/// Words no guard name may contain: a guard with one would read, or hint at,
/// the hold's hidden release draw.
const RELEASE_DRAW_WORDS: &[&str] = &[
    "release", "released", "releases", "draw", "draws", "drawn", "hold", "holds",
];

/// Stems of the engine's own release-draw names, which no SDK guard source or
/// typedef may spell, matched case-blind with `_` removed.
const RELEASE_DRAW_INTERNALS: &[&str] =
    &["holdrelease", "releaseafter", "behaviordraw", "drawseed"];

/// Byte offsets where `word` stands as a whole identifier.
fn identifier_at(text: &str, word: &str) -> Vec<usize> {
    text.match_indices(word)
        .filter(|(at, _)| {
            let before = text[..*at].chars().next_back();
            let after = text[at + word.len()..].chars().next();
            !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
        })
        .map(|(at, _)| at)
        .collect()
}

/// Every identifier in `text`, with its byte offset.
fn identifiers(text: &str) -> Vec<(usize, &str)> {
    let mut found = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices() {
        match (is_identifier_char(c), start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                found.push((from, &text[from..at]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        found.push((from, &text[from..]));
    }
    found
}

/// An identifier lowercased with `_` removed, so naming styles compare alike.
fn normalized(identifier: &str) -> String {
    identifier.to_lowercase().replace('_', "")
}

/// The lowercase words of a name: split at `_`, `.`, `@` and other
/// punctuation, and at each lower-to-upper camelCase boundary.
fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            words.push(std::mem::take(&mut word));
            previous_lower = false;
            continue;
        }
        if c.is_uppercase() && previous_lower {
            words.push(std::mem::take(&mut word));
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        word.extend(c.to_lowercase());
    }
    words.push(word);
    words.retain(|word| !word.is_empty());
    words
}

/// How a file spells code: its own language, or Markdown prose around fenced
/// TypeScript and Luau.
#[derive(Clone, Copy, PartialEq)]
enum Dialect {
    TypeScript,
    Luau,
    Markdown,
}

impl Dialect {
    fn of(path: &Path) -> Self {
        if has_extension(path, &["luau"]) {
            Self::Luau
        } else if has_extension(path, &["md"]) {
            Self::Markdown
        } else {
            Self::TypeScript
        }
    }

    /// Whether `text` opens a string literal. A backtick opens a TypeScript
    /// template literal, but in Markdown prose it opens inline code, which
    /// quotes a name rather than passing one.
    fn starts_string(self, text: &str) -> bool {
        text.starts_with(['"', '\'']) || (self != Self::Markdown && text.starts_with('`'))
    }
}

/// `text` without its leading spaces and tabs: what follows on the same line.
fn same_line(text: &str) -> &str {
    text.trim_start_matches([' ', '\t'])
}

/// After a key ending at `end`: an optional closing quote (a JSON key), then
/// `:` or a lone `=` on the same line. Returns the operator and the offset
/// just past it. A colon ending a line of prose ("the attack:") is no key.
fn assignment_after(text: &str, end: usize) -> Option<(char, usize)> {
    let rest = &text[end..];
    let rest = same_line(rest.strip_prefix(['"', '\'']).unwrap_or(rest));
    let operator = rest.chars().next()?;
    let after = &rest[operator.len_utf8()..];
    match operator {
        ':' if !after.starts_with(':') => {}
        '=' if !after.starts_with('=') => {}
        _ => return None,
    }
    Some((operator, text.len() - after.len()))
}

/// Whether a table's first entry is keyed: `jab = ...` or `["jab"] = ...`.
fn keyed_first_entry(inside: &str) -> bool {
    if inside.starts_with('[') {
        return true;
    }
    let key_len = inside
        .char_indices()
        .find(|&(_, c)| !is_identifier_char(c))
        .map_or(inside.len(), |(at, _)| at);
    if key_len == 0 {
        return false;
    }
    let after = inside[key_len..].trim_start();
    after.starts_with('=') && !after.starts_with("==")
}

/// Byte offsets, with a reason, of every place `text` passes or shows an
/// attack as a string: `attack: "x"` or `attack = "x"` (an action's attack),
/// `attackTurn: "x"`, `attackTurnFree("x")` and `attackTurnHeld("x")` (with
/// or without parentheses in Luau), an `attacks` list of strings, and the
/// retired name-keyed `attacks` map (`attacks: { jab: ... }` in TypeScript,
/// `attacks = { jab = ... }` in Luau). Luau reads `attacks: { ... }` as a
/// type annotation, where `{Attack}` is the handle list type and only a map
/// type, `{[string]: ...}`, hits.
///
/// Not flagged, each by design:
/// - a row's string `to`: it names an activity, which is legal, and a string
///   naming an attack rejects load;
/// - prose that quotes an attack's name ("names attack \"slam\""), since no
///   key and operator precede the string;
/// - a colon ending a line of prose ("the attack:"), since a key's value
///   starts on the key's own line;
/// - Markdown inline code after a key (the attack: `slam`), since a backtick
///   there quotes a name rather than passing one.
fn string_attack_references(text: &str, dialect: Dialect) -> Vec<(usize, &'static str)> {
    let mut hits = Vec::new();
    for key in ["attack", "attackTurn"] {
        for at in identifier_at(text, key) {
            if let Some((_, value)) = assignment_after(text, at + key.len())
                && dialect.starts_string(same_line(&text[value..]))
            {
                hits.push((at, "an attack reference is a string"));
            }
        }
    }
    for fact in ["attackTurnFree", "attackTurnHeld"] {
        for at in identifier_at(text, fact) {
            let rest = same_line(&text[at + fact.len()..]);
            let string_argument = match rest.strip_prefix('(') {
                Some(arguments) => dialect.starts_string(arguments.trim_start()),
                // Luau call sugar: `attackTurnFree "slam"`.
                None => rest.starts_with(['"', '\'']),
            };
            if string_argument {
                hits.push((at, "a turn fact reads a string"));
            }
        }
    }
    for at in identifier_at(text, "attacks") {
        let Some((operator, value)) = assignment_after(text, at + "attacks".len()) else {
            continue;
        };
        let value = same_line(&text[value..]);
        let inside = value
            .strip_prefix(['{', '['])
            .map(str::trim_start)
            .unwrap_or("");
        let reason = if value.starts_with(['[', '{']) && dialect.starts_string(inside) {
            Some("the `attacks` list holds strings")
        } else if !value.starts_with('{') {
            None
        } else if operator == '=' {
            keyed_first_entry(inside).then_some("a name-keyed `attacks` map")
        } else if dialect == Dialect::Luau {
            inside
                .starts_with('[')
                .then_some("a name-keyed `attacks` map type")
        } else {
            Some("a name-keyed `attacks` map")
        };
        if let Some(reason) = reason {
            hits.push((at, reason));
        }
    }
    hits.sort_unstable();
    hits
}

/// Byte offsets of identifiers naming the host-only roll or release draw.
fn host_draw_identifiers(text: &str) -> Vec<(usize, &str)> {
    identifiers(text)
        .into_iter()
        .filter(|(_, identifier)| !HOST_DRAW_EXCLUSIONS.contains(identifier))
        .filter(|(_, identifier)| {
            let normalized = normalized(identifier);
            HOST_DRAW_STEMS.iter().any(|stem| normalized.contains(stem))
        })
        .collect()
}

/// The guard input names `BRAIN_INPUTS` lists, resolved through their
/// constants, plus every guard input prefix.
fn brain_input_names(source: &str) -> Vec<String> {
    let literal = |constant: &str| -> Option<String> {
        let declaration = format!("pub const {constant}: &str = \"");
        let start = source.find(&declaration)? + declaration.len();
        let end = source[start..].find('"')?;
        Some(source[start..start + end].to_string())
    };
    let table_start = source
        .find("pub const BRAIN_INPUTS")
        .expect("brain.rs declares BRAIN_INPUTS");
    let table_end = table_start
        + source[table_start..]
            .find("];")
            .expect("BRAIN_INPUTS closes");
    let mut names: Vec<String> = identifiers(&source[table_start..table_end])
        .into_iter()
        .filter(|(_, identifier)| identifier.ends_with("_INPUT"))
        .map(|(_, constant)| {
            literal(constant)
                .unwrap_or_else(|| panic!("brain.rs declares `{constant}` as a literal"))
        })
        .collect();
    assert!(
        !names.is_empty(),
        "BRAIN_INPUTS lists no `*_INPUT` constants; repoint this gate"
    );
    for (_, constant) in identifiers(source) {
        if constant.ends_with("_INPUT_PREFIX")
            && let Some(prefix) = literal(constant)
        {
            names.push(prefix);
        }
    }
    names
}

/// Every `"@..."` input literal an SDK guard source reads.
fn sdk_input_literals(text: &str) -> Vec<(usize, String)> {
    text.match_indices("\"@")
        .filter_map(|(at, _)| {
            let start = at + 1;
            let end = text[start..].find('"')?;
            Some((at, text[start..start + end].to_string()))
        })
        .collect()
}

/// Every identifier on a typedef line that declares or returns a guard leaf
/// (`RuntimeGuardNode`): members of `brain` and `candidate`, the guard
/// functions, and their parameters.
fn typedef_guard_names(text: &str) -> Vec<(usize, &str)> {
    let mut names = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.contains("RuntimeGuardNode") {
            names.extend(
                identifiers(line)
                    .into_iter()
                    .map(|(at, identifier)| (offset + at, identifier)),
            );
        }
        offset += line.len();
    }
    names
}

fn exposes_release_draw(name: &str) -> bool {
    name_words(name)
        .iter()
        .any(|word| RELEASE_DRAW_WORDS.contains(&word.as_str()))
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| extensions.contains(&ext))
}

// T8: no shipped script, `docs/` page or generated typedef comment passes or
// shows an attack as a string. Attacks are `defineAttack` handles everywhere
// one is referenced.
#[test]
fn shipped_scripts_docs_and_typedefs_pass_attacks_as_handles() {
    let root = workspace_root();
    let mut violations = Vec::new();
    let mut scanned = 0;
    for surface in ATTACK_SURFACE_ROOTS {
        for path in text_files(&root.join(surface)) {
            if !has_extension(&path, ATTACK_SURFACE_EXTENSIONS) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            scanned += 1;
            for (at, reason) in string_attack_references(&text, Dialect::of(&path)) {
                let line: String = text[at..].chars().take_while(|&c| c != '\n').collect();
                violations.push(format!(
                    "{}:{}: {reason}: `{}`",
                    relative(&root, &path),
                    line_of(&text, at),
                    line.trim()
                ));
            }
        }
    }
    assert!(
        scanned > 0,
        "no shipped script or doc was scanned; repoint this gate"
    );
    assert!(
        violations.is_empty(),
        "attacks must be `defineAttack` handles, never strings (scripting.md §2):\n{}",
        violations.join("\n")
    );
}

// R9: no wire record or message carries a behavior roll, a hold release
// draw, or their seed. Both draws stay on the host; clients follow poses
// through replicated animation state.
#[test]
fn no_wire_record_carries_a_roll_or_release_draw() {
    let root = workspace_root();
    let mut violations = Vec::new();
    for crate_root in WIRE_SOURCE_ROOTS {
        let files = text_files(&root.join(crate_root));
        assert!(!files.is_empty(), "`{crate_root}` moved; repoint this gate");
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (at, identifier) in host_draw_identifiers(&text) {
                violations.push(format!(
                    "{}:{}: `{identifier}`",
                    relative(&root, &path),
                    line_of(&text, at)
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "a wire crate names the host-only roll or release draw (networking.md, randomness posture):\n{}",
        violations.join("\n")
    );
}

// P4: no brain input, typedef or guard name exposes the hold's release draw.
// A guard reads only names `BRAIN_INPUTS`, its prefixes and the SDK's guard
// leaves declare, so the draw stays unreadable as long as none names it.
#[test]
fn no_guard_name_exposes_the_release_draw() {
    let root = workspace_root();
    let mut violations = Vec::new();

    let brain = std::fs::read_to_string(root.join(BRAIN_SOURCE)).expect("brain.rs reads");
    let inputs = brain_input_names(&brain);
    assert!(
        inputs.iter().any(|name| name == "@brain.activityRoll"),
        "BRAIN_INPUTS resolved without `@brain.activityRoll`; repoint this gate"
    );
    for name in inputs {
        if exposes_release_draw(&name) {
            violations.push(format!("{BRAIN_SOURCE}: brain input `{name}`"));
        }
    }

    let mut sdk_files: Vec<_> = SDK_GUARD_SOURCES
        .iter()
        .map(|file| root.join(file))
        .collect();
    let typedefs = text_files(&root.join(SDK_TYPEDEFS));
    assert!(
        !typedefs.is_empty(),
        "`{SDK_TYPEDEFS}` moved; repoint this gate"
    );
    sdk_files.extend(typedefs.iter().cloned());
    for path in &sdk_files {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|_| panic!("`{}` moved; repoint this gate", relative(&root, path)));
        let file = relative(&root, path);
        let mut names: Vec<(usize, String)> = sdk_input_literals(&text);
        if typedefs.contains(path) {
            names.extend(
                typedef_guard_names(&text)
                    .into_iter()
                    .map(|(at, name)| (at, name.to_string())),
            );
        }
        assert!(
            names.iter().any(|(_, name)| name.contains("activityRoll")),
            "`{file}` declares no `activityRoll` guard leaf; repoint this gate"
        );
        for (at, name) in names {
            if exposes_release_draw(&name) {
                violations.push(format!(
                    "{file}:{}: guard name `{name}`",
                    line_of(&text, at)
                ));
            }
        }
        for (at, identifier) in identifiers(&text) {
            let normalized = normalized(identifier);
            if RELEASE_DRAW_INTERNALS
                .iter()
                .any(|stem| normalized.contains(stem))
            {
                violations.push(format!(
                    "{file}:{}: engine release-draw name `{identifier}`",
                    line_of(&text, at)
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "a guard name exposes the hold's hidden release draw (entity_model.md §7c, Stances and attacks):\n{}",
        violations.join("\n")
    );
}

#[test]
fn string_attack_gate_matches_string_forms_only() {
    let typescript = r#"
        action: { attack: "slam", at: "hit" },
        attackTurn: 'slam',
        when: attackTurnFree("slam").and(attackTurnHeld(`jab`)),
        attacks: ["jab"],
        attacks: {
          jab: { damage: 8 },
        },
        "attack": "slam",
        action: { attack: slam, at: "hit" },
        attackTurn: slam,
        when: attackTurnFree(slam),
        attacks: [jab, slam],
        attack?: string;
        stalk: [{ to: "jab" }],
        names attack "slam", which the list leaves out
    "#;
    let hits: Vec<&str> = string_attack_references(typescript, Dialect::TypeScript)
        .into_iter()
        .map(|(at, _)| {
            typescript[at..]
                .split(|c: char| !is_identifier_char(c))
                .next()
                .unwrap()
        })
        .collect();
    assert_eq!(
        hits,
        [
            "attack",
            "attackTurn",
            "attackTurnFree",
            "attackTurnHeld",
            "attacks",
            "attacks",
            "attack"
        ]
    );

    let luau = r#"
        action = { attack = "slam" },
        when = attackTurnFree "slam",
        attacks = {
          jab = { damage = 8 },
        },
        attacks = { ["jab"] = { damage = 8 } },
        attacks = { "jab" },
        attacks: {[string]: AttackParams}?,
        action = { attack = slam },
        attacks = { jab, slam },
        attacks = {},
        attacks: {Attack}?,
        when = attackTurnFree(slam):eq(true),
    "#;
    assert_eq!(string_attack_references(luau, Dialect::Luau).len(), 6);

    let markdown = "
        Declare the attack:
        `const slam = defineAttack({ ... })`. Pass the attack: `slam`, not a name.
        It fires the attack: \"slam\" here.
        ```typescript
        action: { attack: \"slam\" },
        attacks: { jab: { damage: 8 } },
        ```
    ";
    assert_eq!(
        string_attack_references(markdown, Dialect::Markdown).len(),
        3
    );
}

#[test]
fn host_draw_gate_matches_draw_names_only() {
    let source = "activity_roll activityRoll BehaviorDraws behavior_draw_seed \
                  hold_release_ms draw_hold_release hold_release_over_gate activity_rate";
    let hits: Vec<&str> = host_draw_identifiers(source)
        .into_iter()
        .map(|(_, identifier)| identifier)
        .collect();
    assert_eq!(
        hits,
        [
            "activity_roll",
            "activityRoll",
            "BehaviorDraws",
            "behavior_draw_seed",
            "hold_release_ms",
            "draw_hold_release"
        ]
    );
}

#[test]
fn release_draw_word_gate_reads_whole_words() {
    for exposed in [
        "@brain.releaseDue",
        "holdMsLeft",
        "drawnReleaseMs",
        "@brain.timeUntilRelease",
    ] {
        assert!(exposes_release_draw(exposed), "{exposed}");
    }
    for clean in [
        "@brain.activityRoll",
        "@attackTurnHeld.",
        "speedThreshold",
        "hasAttackTurn",
        "damageInActivity",
    ] {
        assert!(!exposes_release_draw(clean), "{clean}");
    }
}
