// Flag registry tests: help and rejection, value skipping shared with the map
// scan, and a source scan that keeps every parsed flag registered.
// See: context/lib/development_guide.md §Build and run (Command-line flags)

use std::path::{Path, PathBuf};

use super::cli_flags::*;
use super::session::resolve_map_path;

fn argv(list: &[&str]) -> Vec<String> {
    std::iter::once("postretro")
        .chain(list.iter().copied())
        .map(str::to_string)
        .collect()
}

fn value_flags() -> impl Iterator<Item = &'static FlagSpec> {
    FLAGS.iter().filter(|spec| spec.value != Value::None)
}

#[test]
fn help_flags_ask_for_help_wherever_they_appear() {
    for launch in [
        &["--help"][..],
        &["-h"][..],
        &["--mod", "dev", "--help"][..],
        &["maps/x.prl", "-h"][..],
        &["--hepl", "--help"][..],
    ] {
        assert_eq!(check(&argv(launch)), Verdict::Help, "{launch:?}");
    }
}

#[test]
fn a_help_token_consumed_as_a_value_is_not_a_help_request() {
    // `--mod -h` hands `-h` to `--mod`, which refuses it as a name.
    assert_eq!(check(&argv(&["--mod", "-h"])), Verdict::Run);
}

#[test]
fn a_bare_separator_is_stepped_over_not_rejected() {
    // `xtask run <map> -- --host` forwards the second `--` to the engine.
    assert_eq!(check(&argv(&["maps/x.prl", "--", "--host"])), Verdict::Run);
    assert_eq!(
        check(&argv(&["--", "--hepl"])),
        Verdict::Unknown("--hepl".into())
    );
    assert_eq!(check(&argv(&["--", "--help"])), Verdict::Help);
}

#[test]
fn unregistered_flag_tokens_are_rejected_by_their_first_token() {
    for (launch, token) in [
        (&["--hepl"][..], "--hepl"),
        (&["--version"][..], "--version"),
        (&["maps/x.prl", "-x"][..], "-x"),
        (&["--mod", "dev", "--modd", "x", "--nope"][..], "--modd"),
        (&["-"][..], "-"),
        // A switch's parser compares the whole token: the `=` form is dropped.
        (&["--windowed=1"][..], "--windowed=1"),
        (&["--help=yes"][..], "--help=yes"),
        // A prefix of a registered flag is not that flag.
        (&["--camera-route-holdx=5"][..], "--camera-route-holdx=5"),
    ] {
        assert_eq!(
            check(&argv(launch)),
            Verdict::Unknown(token.to_string()),
            "{launch:?}"
        );
    }
}

#[test]
fn unknown_flag_message_names_the_token_and_the_help_flag() {
    assert_eq!(
        unknown_flag_message("--hepl"),
        "postretro: unknown flag `--hepl`; run `postretro --help` for usage"
    );
}

#[test]
fn real_launches_pass_the_check() {
    for launch in [
        &[][..],
        &["maps/campaign-test.prl"][..],
        &["--mod", "dev", "maps/e1m1.prl", "--windowed"][..],
        &[
            "--mod",
            "my-game",
            "--app-name",
            "my-game",
            "--baked-root",
            "/p/baked",
            "--core-root=/install/core",
            "maps/e1m1.prl",
        ][..],
        &["--host"][..],
        &["--host", "30000", "--password", ""][..],
        &["--connect", "127.0.0.1:27015", "--password=-p"][..],
        &["--start-pose", "-1,2,3,0,0", "maps/x.prl"][..],
        &["--headless", "runspec.json", "--pool-seed=17"][..],
        &["--capture=scene.json"][..],
        &["--observe-live", "8998"][..],
        &[
            "maps/x.prl",
            "--camera-route",
            "r.txt",
            "--camera-route-hold=0",
            "--camera-route-pacing",
            "wall-clock",
            "--present-mode",
            "no-vsync",
        ][..],
        &["--content-root", "content/example"][..],
    ] {
        assert_eq!(check(&argv(launch)), Verdict::Run, "{launch:?}");
    }
}

#[test]
fn every_value_flag_accepts_its_equals_form_and_switches_refuse_it() {
    for spec in FLAGS {
        let token = format!("{}=v", spec.name);
        let expected = if spec.equals_form {
            Verdict::Run
        } else {
            Verdict::Unknown(token.clone())
        };
        assert_eq!(check(&argv(&[&token])), expected, "{}", spec.name);
    }
}

/// The single rule both scans share: a value-taking flag consumes the next
/// token unless it starts with `--`; `--host`/`--connect` also leave an empty
/// one; a single-dash token is a value.
#[test]
fn every_value_flags_value_is_skipped_by_the_map_scan_and_the_check() {
    let mut checked = 0;
    for spec in value_flags() {
        let name = spec.name;
        for value in ["plain", "-1,2,3,0,0", "-h", "-"] {
            let launch = argv(&[name, value, "maps/x.prl"]);
            assert_eq!(check(&launch), Verdict::Run, "{launch:?}");
            assert_eq!(
                resolve_map_path(&launch).as_deref(),
                Some("maps/x.prl"),
                "{launch:?}"
            );
        }
        // A `--` token is the next flag, never the value.
        let launch = argv(&[name, "--windowed", "maps/x.prl"]);
        assert_eq!(check(&launch), Verdict::Run, "{launch:?}");
        assert_eq!(resolve_map_path(&launch).as_deref(), Some("maps/x.prl"));
        let launch = argv(&[name, "--nope"]);
        assert_eq!(check(&launch), Verdict::Unknown("--nope".into()));

        let empty = argv(&[name, "", "maps/x.prl"]);
        let leaves_empty = matches!(
            spec.value,
            Value::RequiredNonEmpty(_) | Value::OptionalNonEmpty(_)
        );
        let expected_map = if leaves_empty { "" } else { "maps/x.prl" };
        assert_eq!(
            resolve_map_path(&empty).as_deref(),
            Some(expected_map),
            "{name}"
        );
        // An empty leftover is a positional, not a flag; `-x` after it is.
        let after_empty = argv(&[name, "", "-x"]);
        assert_eq!(check(&after_empty), Verdict::Unknown("-x".into()), "{name}");
        checked += 1;
    }
    assert!(checked >= 18, "only {checked} value flags checked");
}

#[test]
fn net_flags_carry_netcodes_value_predicates() {
    assert_eq!(
        lookup(HOST_FLAG).unwrap().value,
        Value::OptionalNonEmpty("PORT")
    );
    assert!(matches!(
        lookup(CONNECT_FLAG).unwrap().value,
        Value::RequiredNonEmpty(_)
    ));
    assert!(matches!(
        lookup(PASSWORD_FLAG).unwrap().value,
        Value::Required(_)
    ));
    // A `--password` value is never the map, even one shaped like a flag value.
    let launch = argv(&["--host", "--password", "-hunter2", "maps/x.prl"]);
    assert_eq!(resolve_map_path(&launch).as_deref(), Some("maps/x.prl"));
    assert_eq!(check(&launch), Verdict::Run);
}

/// The registry's `=` flag mirrors each parser it can reach from every build.
#[test]
fn equals_forms_the_registry_accepts_are_read_by_their_parsers() {
    use super::session::{capture_arg, headless_arg};
    assert_eq!(
        headless_arg(&argv(&["--headless=r.json"])),
        Some(Some("r.json"))
    );
    assert_eq!(
        capture_arg(&argv(&["--capture=s.json"])),
        Some(Some("s.json"))
    );
    assert!(super::start_pose::start_pose_arg(&argv(&["--start-pose=1,2,3,0,0"])).is_some());
    let net =
        postretro_netcode::parse_net_config(&argv(&["--host=30000", "--password=p"])).unwrap();
    assert_eq!(net.role, postretro_netcode::NetRole::Host { port: 30000 });
    assert_eq!(net.password.as_deref(), Some("p"));
    assert!(matches!(
        postretro_netcode::parse_net_config(&argv(&["--connect=10.0.0.2"]))
            .unwrap()
            .role,
        postretro_netcode::NetRole::Connect { .. }
    ));
}

#[test]
fn names_are_unique_and_flag_shaped() {
    let mut names: Vec<&str> = FLAGS
        .iter()
        .flat_map(|spec| std::iter::once(spec.name).chain(spec.short))
        .collect();
    for name in &names {
        assert!(name.starts_with('-') && !name.contains('='), "{name}");
    }
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "a flag is registered twice");
}

#[test]
fn help_lists_every_shown_flag_with_its_feature() {
    let help = help_text();
    assert!(help.contains("USAGE:\n    postretro [OPTIONS] [MAP.prl]"));
    let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
    let gated = FLAGS.iter().filter(|spec| spec.feature.is_some()).count();
    assert_eq!(flat.matches("Requires a build with the").count(), gated);
    for spec in FLAGS {
        if spec.group == Group::Retired {
            assert!(!help.contains(spec.name), "{} is retired", spec.name);
            continue;
        }
        assert!(help.contains(spec.name), "{}", spec.name);
        if let Some(feature) = spec.feature {
            assert!(
                flat.contains(&format!("`{feature}` feature")),
                "{}",
                spec.name
            );
        }
    }
    assert!(help.contains("-h, --help"));
    let port = postretro_netcode::DEFAULT_HOST_PORT.to_string();
    for flag in [HOST_FLAG, CONNECT_FLAG] {
        assert!(lookup(flag).unwrap().help.contains(&port), "{flag}");
    }
    for line in help.lines() {
        assert!(line.chars().count() <= 80, "help line too wide: {line:?}");
    }
}

// ---- Source scan: every flag literal the engine parses is registered. ----

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The engine's argv-reading source, relative to the workspace's `crates/`.
/// Adding an argv reader means adding it here; the walk test below fails
/// until it is listed.
const ARGV_READERS: &[&str] = &[
    "postretro/src/main.rs",
    "postretro/src/startup/session.rs",
    "postretro/src/startup/cli_flags.rs",
    "postretro/src/startup/app_dirs.rs",
    "postretro/src/startup/start_pose.rs",
    "postretro/src/startup/camera_route_flags.rs",
    "postretro/src/playback/args.rs",
    "postretro/src/capture/driver.rs",
    "postretro/src/session/net_entry.rs",
    "postretro/src/telemetry/mod.rs",
    // `parse_net_config` and `NetArgError::failed_role_flag`.
    "netcode/src/lib.rs",
];

/// Flag-shaped literals in those files that are arguments to other programs.
const OTHER_PROGRAM_LITERALS: &[(&str, &str, &str)] = &[(
    "postretro/src/capture/driver.rs",
    "--verify",
    "`git rev-parse --verify HEAD` names the capture's commit",
)];

/// Files outside [`ARGV_READERS`] whose production code holds a flag-shaped
/// literal, none read from the engine's argv.
const NOT_ARGV_READERS: &[(&str, &str)] = &[(
    "postretro/src/playback/record/header.rs",
    "`git rev-parse --verify HEAD` names the record's commit",
)];

/// Production text: up to the file's test module.
fn production(source: &str) -> &str {
    source
        .split_once("#[cfg(test)]\nmod tests")
        .map_or(source, |(production, _)| production)
}

/// Flag-shaped string literals (`"--name"`, `"-h"`, `"--name="`), skipping
/// comments, char literals and raw strings.
fn flag_literals(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'\'' if bytes.get(i + 1) == Some(&b'"') && bytes.get(i + 2) == Some(&b'\'') => {
                i += 3;
            }
            b'\'' if bytes.get(i + 1..i + 4) == Some(b"\\\"'") => i += 4,
            b'r' if bytes.get(i + 1) == Some(&b'#') => {
                let hashes = bytes[i + 1..].iter().take_while(|b| **b == b'#').count();
                if bytes.get(i + 1 + hashes) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let close = format!("\"{}", "#".repeat(hashes));
                let body = i + 2 + hashes;
                i = source[body..]
                    .find(&close)
                    .map_or(bytes.len(), |end| body + end + close.len());
            }
            b'"' => {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                let text = &source[start..j.min(bytes.len())];
                if is_flag_shaped(text) {
                    found.push(text.to_string());
                }
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    found
}

fn is_flag_shaped(text: &str) -> bool {
    let text = text.strip_suffix('=').unwrap_or(text);
    let body = text.strip_prefix("--").or_else(|| text.strip_prefix('-'));
    body.is_some_and(|body| {
        body.starts_with(|c: char| c.is_ascii_alphabetic())
            && body.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

fn read(relative: &str) -> String {
    let path = crate_dir().join("..").join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// Unregistered literals, or `=` literals for flags registered without the
/// `=` form, in one file's production text.
fn unregistered_literals(relative: &str, source: &str) -> Vec<String> {
    flag_literals(production(source))
        .into_iter()
        .filter(|literal| {
            !OTHER_PROGRAM_LITERALS
                .iter()
                .any(|(file, excluded, _)| *file == relative && excluded == literal)
        })
        .filter(|literal| match literal.strip_suffix('=') {
            Some(name) => !lookup(name).is_some_and(|spec| spec.equals_form),
            None => lookup(literal).is_none(),
        })
        .collect()
}

#[test]
fn every_flag_literal_in_argv_readers_is_registered() {
    let mut missing = Vec::new();
    let mut seen = 0;
    for relative in ARGV_READERS {
        let source = read(relative);
        seen += flag_literals(production(&source)).len();
        for literal in unregistered_literals(relative, &source) {
            missing.push(format!("{relative}: \"{literal}\""));
        }
    }
    assert!(seen > 10, "the scan found only {seen} literals");
    assert!(
        missing.is_empty(),
        "flag literals with no registry entry (add one to cli_flags::FLAGS, or \
         exclude an argument to another program by name): {missing:#?}"
    );
}

#[test]
fn the_scan_catches_an_unregistered_literal_and_a_dropped_equals_form() {
    let source = "fn f(arg: &str) { if arg == \"--not-registered\" {} \
                  if arg.strip_prefix(\"--windowed=\").is_some() {} \
                  if arg == \"--mod\" {} let c = '\"'; let s = \"--mod-x\"; }\n\
                  // \"--in-a-comment\"\n\
                  #[cfg(test)]\nmod tests { const T: &str = \"--test-only\"; }";
    assert_eq!(
        unregistered_literals("x.rs", source),
        ["--not-registered", "--windowed=", "--mod-x"]
    );
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

/// Keeps [`ARGV_READERS`] honest: production code outside it that reads argv
/// or holds a flag literal must be listed, or excluded with a reason.
#[test]
fn no_unlisted_engine_source_reads_argv_or_holds_flag_literals() {
    let crate_dir = crate_dir().canonicalize().unwrap();
    let crates = crate_dir.parent().unwrap();
    let mut files = Vec::new();
    walk(&crate_dir.join("src"), &mut files);
    let mut unlisted = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(crates)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let name = path.file_name().unwrap().to_string_lossy();
        if ARGV_READERS.contains(&relative.as_str())
            || NOT_ARGV_READERS.iter().any(|(file, _)| *file == relative)
            || name.ends_with("tests.rs")
            || name.ends_with("_fixtures.rs")
        {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        if source.contains("#![cfg(test)]") {
            continue;
        }
        let code = production(&source);
        if code.contains("env::args") || !flag_literals(code).is_empty() {
            unlisted.push(relative);
        }
    }
    assert!(
        unlisted.is_empty(),
        "argv readers or flag literals outside ARGV_READERS: {unlisted:#?}"
    );
}
