// Tests: descriptor `ghost` and weapon `projectile.ghost` opt-ins through mod init in both
// runtimes — defaults, values, and the shared rejection of a non-boolean.
// See: context/lib/entity_model.md §7d · context/lib/scripting.md §1 (parser twins)

use std::path::Path;

use super::{run_mod_init_luau, run_mod_init_quickjs};
use crate::error::ScriptError;
use crate::primitives_registry::PrimitiveRegistry;
use crate::quickjs::{QuickJsConfig, QuickJsSubsystem};
use crate::runtime::ModManifestResult;

fn quickjs() -> QuickJsSubsystem {
    QuickJsSubsystem::new(&PrimitiveRegistry::new(), &QuickJsConfig::default())
        .expect("QuickJS subsystem should initialize")
}

/// A player descriptor and a projectile weapon. `player_ghost` / `projectile_ghost`
/// are each runtime's spelling of the field's value, or `None` to omit it.
fn sources(
    player_ghost: Option<(&str, &str)>,
    projectile_ghost: Option<(&str, &str)>,
) -> (String, String) {
    let field = |value: Option<(&str, &str)>, js: bool| match value {
        Some((js_value, _)) if js => format!("ghost: {js_value}, "),
        Some((_, luau_value)) => format!("ghost = {luau_value}, "),
        None => String::new(),
    };
    let js = format!(
        r#"globalThis.__postretroModManifest = {{ name: "GhostMod", id: "ghost-mod", version: "1",
          entities: [
            {{ canonicalName: "player", {player}components: {{ health: {{ max: 100 }} }} }},
            {{ canonicalName: "rocket_launcher", components: {{ weapon: {{
              damage: 12, range: 64, resolution: "projectile",
              primary: {{ trigger: "press", recoveryMs: 180, steps: [{{ kind: "shot" }}] }},
              projectile: {{ {projectile}speed: 24, radius: 0.1, lifetimeMs: 1500,
                visual: {{ body: {{ kind: "sprite", sprite: "sprites/bolt.png" }} }} }}
            }} }} }},
          ] }};"#,
        player = field(player_ghost, true),
        projectile = field(projectile_ghost, true),
    );
    let luau = format!(
        r#"return {{ name = "GhostMod", id = "ghost-mod", version = "1",
          entities = {{
            {{ canonicalName = "player", {player}components = {{ health = {{ max = 100 }} }} }},
            {{ canonicalName = "rocket_launcher", components = {{ weapon = {{
              damage = 12, range = 64, resolution = "projectile",
              primary = {{ trigger = "press", recoveryMs = 180, steps = {{ {{ kind = "shot" }} }} }},
              projectile = {{ {projectile}speed = 24, radius = 0.1, lifetimeMs = 1500,
                visual = {{ body = {{ kind = "sprite", sprite = "sprites/bolt.png" }} }} }}
            }} }} }},
          }} }}"#,
        player = field(player_ghost, false),
        projectile = field(projectile_ghost, false),
    );
    (js, luau)
}

fn run_both(
    player_ghost: Option<(&str, &str)>,
    projectile_ghost: Option<(&str, &str)>,
) -> (
    Result<ModManifestResult, ScriptError>,
    Result<ModManifestResult, ScriptError>,
) {
    let (js, luau) = sources(player_ghost, projectile_ghost);
    (
        run_mod_init_quickjs(&quickjs(), &js, "ghost-mod.js"),
        run_mod_init_luau(&[], &luau, "ghost-mod.luau", Path::new(".")),
    )
}

/// `(player ghost, launcher projectile ghost)` as a manifest committed them.
fn opt_ins(manifest: &ModManifestResult) -> (bool, bool) {
    let player = manifest
        .entities
        .iter()
        .find(|entity| entity.canonical_name.as_deref() == Some("player"))
        .expect("player descriptor");
    let launcher = manifest
        .entities
        .iter()
        .find(|entity| entity.canonical_name.as_deref() == Some("rocket_launcher"))
        .expect("launcher descriptor");
    let projectile = launcher
        .weapon
        .as_ref()
        .and_then(|weapon| weapon.projectile.as_ref())
        .expect("launcher fires a projectile");
    (player.ghost, projectile.ghost)
}

// Both opt-ins default to false for every descriptor, a player's included, and
// read an authored boolean as written; `null` / `nil` reads as omitted.
#[test]
fn ghost_opt_ins_default_false_and_read_authored_booleans_in_both_runtimes() {
    for (player, projectile, expected) in [
        (None, None, (false, false)),
        (Some(("true", "true")), None, (true, false)),
        (None, Some(("true", "true")), (false, true)),
        (
            Some(("false", "false")),
            Some(("false", "false")),
            (false, false),
        ),
        (Some(("null", "nil")), Some(("null", "nil")), (false, false)),
        (Some(("true", "true")), Some(("true", "true")), (true, true)),
    ] {
        let (js, luau) = run_both(player, projectile);
        let js = js.expect("a boolean or omitted opt-in commits");
        let luau = luau.expect("a boolean or omitted opt-in commits");
        assert_eq!(opt_ins(&js), expected, "QuickJS {player:?} {projectile:?}");
        assert_eq!(opt_ins(&luau), expected, "Luau {player:?} {projectile:?}");
    }
}

/// The descriptor diagnostic inside a mod-init rejection, without the
/// runtime-specific framing (`default mod manifest export` vs `returned mod
/// manifest`).
fn descriptor_diagnostic(error: ScriptError) -> String {
    let message = error.to_string();
    message
        .split_once("`entities[")
        .and_then(|(_, rest)| rest.split_once("invalid: "))
        .map(|(_, diagnostic)| diagnostic.to_string())
        .unwrap_or_else(|| panic!("not a descriptor rejection: {message}"))
}

// A non-boolean `ghost` or `projectile.ghost` rejects mod
// init, with the same descriptor error in both runtimes.
#[test]
fn a_non_boolean_ghost_or_projectile_ghost_rejects_mod_init_alike_in_both_runtimes() {
    let malformed = [
        ("1", "1"),
        ("'yes'", "'yes'"),
        ("{}", "{}"),
        ("[true]", "{ true }"),
    ];
    for value in malformed {
        // Each runtime names the entry in its own index base.
        for (player, projectile, path, js_entry, luau_entry) in [
            (Some(value), None, "`ghost`", "entities[0]", "entities[1]"),
            (
                None,
                Some(value),
                "`components.weapon.projectile.ghost`",
                "entities[1]",
                "entities[2]",
            ),
        ] {
            let (js, luau) = run_both(player, projectile);
            let js = js.expect_err("a non-boolean opt-in rejects QuickJS mod init");
            let luau = luau.expect_err("a non-boolean opt-in rejects Luau mod init");
            assert!(js.to_string().contains(js_entry), "{js}");
            assert!(luau.to_string().contains(luau_entry), "{luau}");
            let js = descriptor_diagnostic(js);
            let luau = descriptor_diagnostic(luau);
            assert_eq!(js, luau, "the runtimes' diagnostics diverged for {value:?}");
            assert!(
                js.contains(&format!("{path} must be a boolean when supplied")),
                "{js}"
            );
        }
    }
}
