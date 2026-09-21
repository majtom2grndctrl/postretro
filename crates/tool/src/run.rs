//! `run`: the external authoring loop's launcher.
//!
//! The engine learns nothing about `postretro.toml` — it keeps plain flags, so
//! changing the manifest schema stays a tool change. What this removes is the
//! three paths a modder would otherwise type on every launch, none of whose
//! failure modes is an error: the wrong materials root degrades every world
//! texture to a placeholder (`build_pipeline.md` §Baked texture mips), and a
//! `core/` the engine cannot find degrades the pause menu, frontend menu,
//! on-screen keyboard and splash to warnings (`ui.md` §5).
//!
//! The two are separate lookups in both directions. The game comes from the
//! project; the engine's own assets come from the install; neither falls back
//! to the other.
//!
//! See: context/lib/build_pipeline.md §Baked texture mips · context/lib/ui.md §5

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::binaries::{Helper, Overrides, status_code};
use crate::engine_trees::{self, CORE_TREE, INSTALL_ROOT_FLAG};
use crate::project::{Project, ProjectLocation};

/// The engine flag naming the directory that holds its own `ui/` and `textures/`.
const CORE_ROOT_FLAG: &str = "--core-root";

/// The tool's own flags, and everything forwarded to the engine untouched.
struct RunArgs {
    location: ProjectLocation,
    named_install_root: Option<PathBuf>,
    binaries: Overrides,
    engine_args: Vec<OsString>,
}

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let cli = parse_args(args)?;
    let working_directory =
        std::env::current_dir().map_err(|error| format!("read the working directory: {error}"))?;
    let project = cli.location.open(&working_directory)?;
    let engine = cli.binaries.resolve(Helper::AuthoringEngine)?;
    let core_root = resolve_core_root(&cli, &cli.engine_args)?;
    let launch_args = engine_arguments(&project, core_root, cli.engine_args);

    println!(
        "Launching {} in {}",
        engine.display(),
        project.root().display()
    );

    // The working directory is the contract, not a convenience: every content
    // path the engine resolves is joined against it.
    let mut command = Command::new(&engine);
    command
        .current_dir(project.root())
        .args(launch_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    status_code(command.status())
}

/// The install's `core/`, absolute, or `None` when the caller named one itself.
///
/// This run pins the working directory to the project root, and an external
/// project correctly has no `core/` of its own — so the engine has to be told
/// where the engine's own assets are, or it boots without the pause menu,
/// frontend menu, on-screen keyboard and splash, each degraded to a `warn!`.
///
/// Absolute, because the engine resolves a relative flag against the working
/// directory this run pins to the project — which is the very directory that
/// does not hold `core/`.
///
/// A missing `core/` is an error rather than a launch with three screens gone,
/// for the same reason the packaging stages refuse one: nothing downstream
/// reports it. The message names `--install-root`, which is also the escape
/// hatch, alongside naming `--core-root` outright.
fn resolve_core_root(cli: &RunArgs, engine_args: &[OsString]) -> Result<Option<PathBuf>, String> {
    if names_flag(engine_args, &[CORE_ROOT_FLAG]) {
        return Ok(None);
    }
    let install_root = match &cli.named_install_root {
        Some(root) => root.clone(),
        None => engine_trees::default_install_root()?,
    };
    engine_trees::resolve(&install_root, CORE_TREE)
        .and_then(absolute)
        .map(Some)
}

/// Resolve against the working directory without touching the filesystem — the
/// tool's own directory-derived install root is already absolute, and a caller's
/// relative `--install-root` was relative to where they stood.
fn absolute(path: PathBuf) -> Result<PathBuf, String> {
    std::path::absolute(&path)
        .map_err(|error| format!("resolve {} to an absolute path: {error}", path.display()))
}

/// Prepend the flags the project already answers, unless the caller set them.
///
/// An explicit `--mod`, `--content-root`, `--baked-root`, or `--core-root` wins
/// outright rather than being shadowed: the engine reads the first occurrence of
/// each, so supplying ours unconditionally would silently discard the caller's.
///
/// `--core-root` and the content flags are independent by design. The engine's
/// own assets come from the install; the game comes from the project; neither
/// lookup falls back to the other.
fn engine_arguments(
    project: &Project,
    core_root: Option<PathBuf>,
    engine_args: Vec<OsString>,
) -> Vec<OsString> {
    let mut launch_args = Vec::with_capacity(engine_args.len() + 6);
    if !names_flag(&engine_args, &["--mod", "--content-root"]) {
        launch_args.push(OsString::from("--mod"));
        launch_args.push(OsString::from(&project.manifest().package.mod_root));
    }
    if !names_flag(&engine_args, &["--baked-root"]) {
        launch_args.push(OsString::from("--baked-root"));
        launch_args.push(project.baked_root().into_os_string());
    }
    if let Some(core_root) = core_root {
        launch_args.push(OsString::from(CORE_ROOT_FLAG));
        launch_args.push(core_root.into_os_string());
    }
    launch_args.extend(engine_args);
    launch_args
}

/// Whether any of `flags` appears, in either `--flag value` or `--flag=value` form.
fn names_flag(args: &[OsString], flags: &[&str]) -> bool {
    args.iter().any(|argument| {
        let Some(argument) = argument.to_str() else {
            return false;
        };
        flags
            .iter()
            .any(|flag| argument == *flag || argument.starts_with(&format!("{flag}=")))
    })
}

/// Consume the tool's flags; everything else forwards verbatim, so an engine
/// flag the tool has never heard of needs no change here.
fn parse_args(args: Vec<OsString>) -> Result<RunArgs, String> {
    let mut location = ProjectLocation::default();
    let mut named_install_root = None;
    let mut binaries = Overrides::default();
    let mut engine_args = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let flag = args[index].to_str().unwrap_or_default();
        let value = args.get(index + 1);
        if binaries.absorb(flag, value)? {
            index += 2;
            continue;
        }
        if location
            .absorb(flag, value)
            .map_err(|error| usage(&error))?
        {
            index += 2;
            continue;
        }
        if flag == INSTALL_ROOT_FLAG {
            let value = value
                .ok_or_else(|| usage(&format!("{INSTALL_ROOT_FLAG} requires a path")))?
                .clone();
            if named_install_root.is_some() {
                return Err(usage(&format!("{INSTALL_ROOT_FLAG} given twice")));
            }
            named_install_root = Some(PathBuf::from(value));
            index += 2;
            continue;
        }
        engine_args.push(args[index].clone());
        index += 1;
    }

    Ok(RunArgs {
        location,
        named_install_root,
        binaries,
        engine_args,
    })
}

fn usage(message: &str) -> String {
    format!(
        "{message}\n\nUsage: postretro-tool run [--project <dir> | --manifest <path>] \
         [{INSTALL_ROOT_FLAG} <dir>] [--engine <path>] [engine args...]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn baked_root() -> String {
        project().baked_root().to_string_lossy().into_owned()
    }

    fn project() -> Project {
        Project::for_test("/projects/game", "game", "levels/core")
    }

    fn core_root() -> PathBuf {
        PathBuf::from("/install/core")
    }

    /// The three flags travel together or not at all: the baked root without the
    /// mod root launches against the wrong content, the mod root without the
    /// baked root is the silent-placeholder defect itself, and without the core
    /// root the pause menu, frontend menu, keyboard and splash are all absent.
    #[test]
    fn launch_supplies_the_mod_root_the_baked_root_and_the_core_root() {
        assert_eq!(
            engine_arguments(&project(), Some(core_root()), os_args(&["maps/e1m1.prl"])),
            os_args(&[
                "--mod",
                "levels/core",
                "--baked-root",
                &baked_root(),
                "--core-root",
                "/install/core",
                "maps/e1m1.prl",
            ])
        );
    }

    #[test]
    fn an_explicit_flag_wins_rather_than_being_shadowed() {
        assert_eq!(
            engine_arguments(
                &project(),
                None,
                os_args(&["--mod", "levels/expansion", "--baked-root=/elsewhere/baked"]),
            ),
            os_args(&["--mod", "levels/expansion", "--baked-root=/elsewhere/baked"])
        );

        // `--content-root` selects the same thing `--mod` does, so it suppresses
        // the tool's mod root too — but not its baked root or its core root.
        assert_eq!(
            engine_arguments(
                &project(),
                Some(core_root()),
                os_args(&["--content-root", "levels/x"])
            ),
            os_args(&[
                "--baked-root",
                &baked_root(),
                "--core-root",
                "/install/core",
                "--content-root",
                "levels/x",
            ])
        );
    }

    /// A caller naming `--core-root` themselves suppresses the install lookup
    /// entirely — the engine reads the first occurrence, so a second copy would
    /// be dead weight, and resolving an install root we will not use turns an
    /// explicit override into a failure.
    #[test]
    fn a_caller_supplied_core_root_suppresses_the_install_lookup() {
        let cli =
            parse_args(os_args(&["--core-root", "/elsewhere/core"])).expect("engine flags forward");
        assert_eq!(
            resolve_core_root(&cli, &cli.engine_args),
            Ok(None),
            "the caller's own core root stands, and nothing is derived",
        );
        assert_eq!(
            engine_arguments(&project(), None, cli.engine_args),
            os_args(&[
                "--mod",
                "levels/core",
                "--baked-root",
                &baked_root(),
                "--core-root",
                "/elsewhere/core",
            ])
        );

        let equals =
            parse_args(os_args(&["--core-root=/elsewhere/core"])).expect("engine flags forward");
        assert_eq!(resolve_core_root(&equals, &equals.engine_args), Ok(None));
    }

    /// An install with no `core/` is an error rather than a launch with the
    /// screens silently gone, and the message names both ways out.
    #[test]
    fn an_install_without_core_refuses_to_launch_and_names_the_flags() {
        let cli = parse_args(os_args(&[INSTALL_ROOT_FLAG, "/no/such/install"]))
            .expect("the install root parses");
        let error = resolve_core_root(&cli, &cli.engine_args)
            .expect_err("a missing core tree cannot pass silently");
        assert!(error.contains(CORE_TREE), "{error}");
        assert!(error.contains(INSTALL_ROOT_FLAG), "{error}");
    }

    #[test]
    fn unknown_engine_flags_forward_verbatim() {
        let parsed = parse_args(os_args(&["--headless", "runspec.json", "--pool-seed=17"]))
            .expect("engine arguments forward");
        assert_eq!(parsed.location, ProjectLocation::default());
        assert_eq!(parsed.named_install_root, None);
        assert_eq!(
            parsed.engine_args,
            os_args(&["--headless", "runspec.json", "--pool-seed=17"])
        );
    }

    /// `--install-root` is the tool's, not the engine's: it is consumed here and
    /// never reaches the engine, which has never heard of it.
    #[test]
    fn the_install_root_is_consumed_and_never_forwarded() {
        let parsed = parse_args(os_args(&[INSTALL_ROOT_FLAG, "/bundle", "maps/e1m1.prl"]))
            .expect("tool arguments parse");

        assert_eq!(parsed.named_install_root, Some(PathBuf::from("/bundle")));
        assert_eq!(parsed.engine_args, os_args(&["maps/e1m1.prl"]));
    }

    #[test]
    fn tool_flags_are_consumed_before_forwarding() {
        let parsed = parse_args(os_args(&[
            "--manifest",
            "/projects/game/postretro.toml",
            "--engine",
            "/build/postretro",
            "maps/e1m1.prl",
        ]))
        .expect("tool arguments parse");

        assert_eq!(
            parsed.location.manifest(),
            Some(Path::new("/projects/game/postretro.toml"))
        );
        assert_eq!(parsed.engine_args, os_args(&["maps/e1m1.prl"]));
    }
}
