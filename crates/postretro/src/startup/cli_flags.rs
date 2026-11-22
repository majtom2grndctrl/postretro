// Engine command-line flag registry: every flag's name, value shape, `=` form,
// feature gate and help line; `--help` and the unknown-flag check come from it.
// See: context/lib/development_guide.md §Build and run (Command-line flags)

use std::io::Write as _;

use super::app_dirs::APP_NAME_FLAG;
use super::camera_route_flags::{
    CAMERA_ROUTE_FEATURE, CAMERA_ROUTE_FLAG, CAMERA_ROUTE_HOLD_FLAG, CAMERA_ROUTE_PACING_FLAG,
    CAMERA_ROUTE_REPORT_FLAG, CAMERA_ROUTE_STALL_TIMEOUT_FLAG, PRESENT_MODE_FLAG,
    RECORD_CAMERA_ROUTE_FLAG,
};
use super::start_pose::START_POSE_FLAG;

pub(crate) const HELP_FLAG: &str = "--help";
pub(crate) const HELP_SHORT_FLAG: &str = "-h";
pub(crate) const MOD_FLAG: &str = "--mod";
pub(crate) const BAKED_ROOT_FLAG: &str = "--baked-root";
pub(crate) const CORE_ROOT_FLAG: &str = "--core-root";
pub(crate) const WINDOWED_FLAG: &str = "--windowed";
pub(crate) const POOL_SEED_FLAG: &str = "--pool-seed";
pub(crate) const HEADLESS_FLAG: &str = "--headless";
pub(crate) const CAPTURE_FLAG: &str = "--capture";
pub(crate) const OBSERVE_LIVE_FLAG: &str = "--observe-live";
/// Retired: refused by name with a pointer to `--mod`, so it stays registered
/// (hidden from help) to reach that message instead of the unknown-flag one.
pub(crate) const CONTENT_ROOT_FLAG: &str = "--content-root";
// Parsed by `postretro_netcode::parse_net_config`, which cannot depend on this
// crate; the source-scan test keeps its literals registered here.
pub(crate) const HOST_FLAG: &str = "--host";
pub(crate) const CONNECT_FLAG: &str = "--connect";
pub(crate) const PASSWORD_FLAG: &str = "--password";

/// What follows a flag. Each predicate is the one its parser and
/// `resolve_map_path` apply to the next token; a single-dash token
/// (`--start-pose -1,2,3,0,0`) is a value under all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Value {
    /// A switch: the next token is never consumed.
    None,
    /// Consumes the next token unless it starts with `--`; an empty token is a
    /// value. Meta names the value in help.
    Required(&'static str),
    /// As `Required`, but an empty next token is left unconsumed: the
    /// predicate `parse_net_config` uses for `--connect`.
    RequiredNonEmpty(&'static str),
    /// Consumes the next token only when it is non-empty and not `--`-prefixed;
    /// otherwise the flag stands bare (`--host` with no port).
    OptionalNonEmpty(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    Launch,
    Networking,
    Diagnostics,
    Info,
    /// Registered so its parser can refuse it with a pointer; never in help.
    Retired,
}

impl Group {
    const SHOWN: [Group; 4] = [
        Group::Launch,
        Group::Networking,
        Group::Diagnostics,
        Group::Info,
    ];

    fn title(self) -> &'static str {
        match self {
            Group::Launch => "Content and launch",
            Group::Networking => "Networking (co-op)",
            Group::Diagnostics => "Dev and diagnostics",
            Group::Info => "Info",
            Group::Retired => "Retired",
        }
    }
}

#[derive(Debug)]
pub(crate) struct FlagSpec {
    pub(crate) name: &'static str,
    pub(crate) short: Option<&'static str>,
    pub(crate) value: Value,
    /// The owning parser also reads `--flag=value`. Never set it for a parser
    /// that ignores that form: the check would pass a token nothing reads.
    pub(crate) equals_form: bool,
    /// Cargo feature the flag's behaviour builds behind. Every build registers
    /// the flag; a build without the feature warns or exits as it always has.
    pub(crate) feature: Option<&'static str>,
    pub(crate) group: Group,
    pub(crate) help: &'static str,
}

const fn flag(name: &'static str, value: Value, group: Group, help: &'static str) -> FlagSpec {
    FlagSpec {
        name,
        short: None,
        value,
        equals_form: !matches!(value, Value::None),
        feature: None,
        group,
        help,
    }
}

const fn gated(
    name: &'static str,
    value: Value,
    feature: &'static str,
    help: &'static str,
) -> FlagSpec {
    FlagSpec {
        feature: Some(feature),
        ..flag(name, value, Group::Diagnostics, help)
    }
}

/// Every flag the engine reads, in help order within each group.
pub(crate) static FLAGS: &[FlagSpec] = &[
    flag(
        MOD_FLAG,
        Value::Required("NAME"),
        Group::Launch,
        "Load the mod in content/NAME (a folder name, not a path). A relative MAP is then read inside that folder.",
    ),
    flag(
        APP_NAME_FLAG,
        Value::Required("NAME"),
        Group::Launch,
        "Names the per-user settings and saves directory: the game's package name. Default: postretro.",
    ),
    flag(
        BAKED_ROOT_FLAG,
        Value::Required("DIR"),
        Group::Launch,
        "The directory that contains the baked materials/ folder: the one given to prl-build. Absent, the engine finds it from its own layout.",
    ),
    flag(
        CORE_ROOT_FLAG,
        Value::Required("DIR"),
        Group::Launch,
        "The engine's core/ directory itself (holds ui/ and textures/). Default: core/ under the working directory.",
    ),
    flag(
        WINDOWED_FLAG,
        Value::None,
        Group::Launch,
        "Start in a window, overriding the saved fullscreen setting for this run without changing it.",
    ),
    flag(
        START_POSE_FLAG,
        Value::Required("X,Y,Z,YAW,PITCH"),
        Group::Launch,
        "Start the first level at this pose instead of the map spawn: capsule centre in engine meters, yaw and pitch in degrees. A malformed value warns and is ignored.",
    ),
    flag(
        POOL_SEED_FLAG,
        Value::Required("SEED"),
        Group::Launch,
        "Pin the trigger-pool seed (a u64) so pool picks repeat. Without it a windowed run rolls a new seed each install and a headless run arms every pool.",
    ),
    flag(
        HOST_FLAG,
        Value::OptionalNonEmpty("PORT"),
        Group::Networking,
        "Host a co-op session (listen server) on PORT. Bare --host uses port 27015.",
    ),
    flag(
        CONNECT_FLAG,
        Value::RequiredNonEmpty("IP[:PORT]"),
        Group::Networking,
        "Join a host at an IPv4 address; PORT defaults to 27015. Needs no map: the client loads the host's level.",
    ),
    flag(
        PASSWORD_FLAG,
        Value::Required("TEXT"),
        Group::Networking,
        "Session password: --host challenges for it, --connect answers with it. Never logged; an empty value means none.",
    ),
    gated(
        HEADLESS_FLAG,
        Value::Required("RUNSPEC"),
        "observability",
        "Run an observability runspec with no window, print its JSON result to stdout, and exit.",
    ),
    gated(
        CAPTURE_FLAG,
        Value::Required("SCENE"),
        "capture",
        "Render a capture scene spec offscreen and exit; no window opens.",
    ),
    gated(
        OBSERVE_LIVE_FLAG,
        Value::Required("PORT"),
        "observe-live",
        "Open the live introspection channel on this localhost TCP port.",
    ),
    gated(
        CAMERA_ROUTE_FLAG,
        Value::Required("FILE"),
        CAMERA_ROUTE_FEATURE,
        "Fly the camera along a route file, write a playback report, and exit.",
    ),
    gated(
        CAMERA_ROUTE_PACING_FLAG,
        Value::Required("frame-locked|wall-clock"),
        CAMERA_ROUTE_FEATURE,
        "How route frames map to poses. Default: frame-locked.",
    ),
    gated(
        CAMERA_ROUTE_HOLD_FLAG,
        Value::Required("FRAMES"),
        CAMERA_ROUTE_FEATURE,
        "Frames to hold the first pose before flying, 0 to 100000. Default: 600.",
    ),
    gated(
        CAMERA_ROUTE_REPORT_FLAG,
        Value::Required("FILE"),
        CAMERA_ROUTE_FEATURE,
        "Where the playback report goes. Default: a JSON file in the working directory named after the route file's stem.",
    ),
    gated(
        CAMERA_ROUTE_STALL_TIMEOUT_FLAG,
        Value::Required("SECONDS"),
        CAMERA_ROUTE_FEATURE,
        "Give up when no hold, route or recorded frame shows in time, up to 86400. Default: 300.",
    ),
    gated(
        RECORD_CAMERA_ROUTE_FLAG,
        Value::Required("FILE"),
        CAMERA_ROUTE_FEATURE,
        "Record the camera through the first level played to a route file.",
    ),
    gated(
        PRESENT_MODE_FLAG,
        Value::Required("vsync|no-vsync"),
        CAMERA_ROUTE_FEATURE,
        "Swapchain present mode from the first frame. Default: vsync.",
    ),
    FlagSpec {
        short: Some(HELP_SHORT_FLAG),
        ..flag(
            HELP_FLAG,
            Value::None,
            Group::Info,
            "Print this help and exit.",
        )
    },
    flag(
        CONTENT_ROOT_FLAG,
        Value::Required("PATH"),
        Group::Retired,
        "Removed; select a mod with --mod NAME.",
    ),
];

/// The registry entry a whole token names, by full or short name.
pub(crate) fn lookup(token: &str) -> Option<&'static FlagSpec> {
    FLAGS
        .iter()
        .find(|spec| spec.name == token || spec.short == Some(token))
}

/// The value of `arg` in `--flag=value` form, empty included.
pub(crate) fn equals_value<'a>(arg: &'a str, flag: &str) -> Option<&'a str> {
    arg.strip_prefix(flag)?.strip_prefix('=')
}

impl FlagSpec {
    /// Whether this flag, given as its own token, consumes `next` as its value.
    pub(crate) fn takes_next(&self, next: Option<&str>) -> bool {
        let Some(next) = next else {
            return false;
        };
        match self.value {
            Value::None => false,
            Value::Required(_) => !next.starts_with("--"),
            Value::RequiredNonEmpty(_) | Value::OptionalNonEmpty(_) => {
                !next.is_empty() && !next.starts_with("--")
            }
        }
    }

    fn usage(&self) -> String {
        let names = match self.short {
            Some(short) => format!("{short}, {}", self.name),
            None => self.name.to_string(),
        };
        match self.value {
            Value::None => names,
            Value::Required(meta) | Value::RequiredNonEmpty(meta) => format!("{names} <{meta}>"),
            Value::OptionalNonEmpty(meta) => format!("{names} [{meta}]"),
        }
    }
}

/// What the command line asks for before anything else runs.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Run,
    Help,
    /// The first flag token no registry entry claims.
    Unknown(String),
}

/// Classify argv (including `argv[0]`). A flag token is any `-`-prefixed token
/// not consumed as a preceding flag's value. Help wins wherever it appears.
/// A bare `--` is a separator, not a flag: launchers forward it (`xtask run
/// <map> -- --host`), and the engine has always stepped over it.
pub(crate) fn check(args: &[String]) -> Verdict {
    let mut help = false;
    let mut unknown = None;
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if !arg.starts_with('-') || arg == "--" {
            continue;
        }
        if let Some(spec) = lookup(arg) {
            help |= spec.group == Group::Info;
            if spec.takes_next(iter.peek().map(|next| next.as_str())) {
                let _ = iter.next();
            }
            continue;
        }
        let equals_known = FLAGS
            .iter()
            .any(|spec| spec.equals_form && equals_value(arg, spec.name).is_some());
        if !equals_known && unknown.is_none() {
            unknown = Some(arg.clone());
        }
    }
    match (help, unknown) {
        (true, _) => Verdict::Help,
        (false, Some(token)) => Verdict::Unknown(token),
        (false, None) => Verdict::Run,
    }
}

/// Prints help (stdout, exit 0) or the unknown-flag error (stderr, exit 2) and
/// exits; returns only for a launch every flag of which is registered. Runs
/// first in `main`, so neither path writes telemetry or opens a window.
pub(crate) fn exit_on_help_or_unknown_flag(args: &[String]) {
    match check(args) {
        Verdict::Run => {}
        Verdict::Help => {
            // A closed pipe (`postretro --help | head`) is not an error.
            let _ = std::io::stdout().lock().write_all(help_text().as_bytes());
            std::process::exit(0);
        }
        Verdict::Unknown(token) => {
            eprintln!("{}", unknown_flag_message(&token));
            std::process::exit(2);
        }
    }
}

pub(crate) fn unknown_flag_message(token: &str) -> String {
    format!("postretro: unknown flag `{token}`; run `postretro --help` for usage")
}

const WIDTH: usize = 80;
const HELP_COLUMN: usize = 30;

/// The full `--help` text, generated from [`FLAGS`].
pub(crate) fn help_text() -> String {
    let mut out = String::new();
    out.push_str("Postretro engine.\n\nUSAGE:\n    postretro [OPTIONS] [MAP.prl]\n\nARGS:\n");
    push_entry(
        &mut out,
        "MAP.prl",
        "Level to load. With --mod, a path inside the mod folder (maps/e1m1.prl); otherwise a path from the working directory, whose mod is the map's grandparent folder (content/<mod>/maps/e1m1.prl). Without a map, the frontend menu of the mod (content/dev by default) opens.",
    );
    for group in Group::SHOWN {
        out.push('\n');
        out.push_str(group.title());
        out.push_str(":\n");
        for spec in FLAGS.iter().filter(|spec| spec.group == group) {
            let help = match spec.feature {
                Some(feature) => {
                    format!(
                        "{} Requires a build with the `{feature}` feature.",
                        spec.help
                    )
                }
                None => spec.help.to_string(),
            };
            push_entry(&mut out, &spec.usage(), &help);
        }
    }
    let every_value_takes_equals = FLAGS
        .iter()
        .filter(|spec| spec.value != Value::None)
        .all(|spec| spec.equals_form);
    if every_value_takes_equals {
        out.push_str("\nEvery flag that takes a value also accepts --flag=VALUE.\n");
    }
    out
}

/// One `  usage   help` entry, help wrapped at [`WIDTH`] from [`HELP_COLUMN`].
fn push_entry(out: &mut String, usage: &str, help: &str) {
    let lead = format!("  {usage}");
    let mut line = if lead.len() + 2 <= HELP_COLUMN {
        format!("{lead:HELP_COLUMN$}")
    } else {
        out.push_str(&lead);
        out.push('\n');
        " ".repeat(HELP_COLUMN)
    };
    let mut line_has_word = false;
    for word in help.split_whitespace() {
        if line_has_word && line.len() + 1 + word.len() > WIDTH {
            out.push_str(&line);
            out.push('\n');
            line = " ".repeat(HELP_COLUMN);
            line_has_word = false;
        }
        if line_has_word {
            line.push(' ');
        }
        line.push_str(word);
        line_has_word = true;
    }
    out.push_str(&line);
    out.push('\n');
}
