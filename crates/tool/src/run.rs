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
use crate::flag::match_flag;
use crate::project::{Project, ProjectLocation};

/// The engine flag naming the directory that holds its own `ui/` and `textures/`.
const CORE_ROOT_FLAG: &str = "--core-root";

/// The only helper a launch drives is the debug authoring engine it starts. The
/// compilers, the mint, and the tool belong to the packaging commands, not to a
/// launch — so their flags are rejected here rather than swallowed and forwarded
/// to an engine that has never heard of them.
const RUN_HELPERS: &[Helper] = &[Helper::AuthoringEngine];

/// The tool's own flags, and everything forwarded to the engine untouched.
struct RunArgs {
    location: ProjectLocation,
    named_install_root: Option<PathBuf>,
    binaries: Overrides,
    engine_args: Vec<OsString>,
}

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let mut cli = parse_args(args)?;
    let working_directory =
        std::env::current_dir().map_err(|error| format!("read the working directory: {error}"))?;
    // Absolute before anything derives a path from it. This command re-roots the
    // engine's working directory at the project, and `--baked-root` is passed as
    // a path the engine then resolves against *that* directory — so a project
    // root left relative would send the engine looking for
    // `<project>/<project>/baked/materials` and degrade every world texture to a
    // placeholder with a warning. Same class as D3 itself.
    cli.location.rebase(&working_directory);
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
/// An explicit `--mod`, `--baked-root`, or `--core-root` wins outright rather
/// than being shadowed: the engine reads the first occurrence of each, so
/// supplying ours unconditionally would silently discard the caller's.
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
    if !names_flag(&engine_args, &["--mod"]) {
        launch_args.push(OsString::from("--mod"));
        launch_args.push(OsString::from(project.mod_name()));
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
        let consumed = binaries.absorb_only(flag, value, RUN_HELPERS)?;
        if consumed > 0 {
            index += consumed;
            continue;
        }
        let consumed = location
            .absorb(flag, value)
            .map_err(|error| usage(&error))?;
        if consumed > 0 {
            index += consumed;
            continue;
        }
        if let Some(matched) = match_flag(flag, value, INSTALL_ROOT_FLAG) {
            let value = matched
                .value
                .ok_or_else(|| usage(&format!("{INSTALL_ROOT_FLAG} requires a path")))?;
            if named_install_root.is_some() {
                return Err(usage(&format!("{INSTALL_ROOT_FLAG} given twice")));
            }
            named_install_root = Some(PathBuf::from(value.into_owned()));
            index += matched.tokens;
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
        Project::for_test("/projects/game", "game", "core")
    }

    fn core_root() -> PathBuf {
        PathBuf::from("/install/core")
    }

    /// The three flags travel together or not at all: the baked root without the
    /// mod root launches against the wrong content, the mod root without the
    /// baked root is the silent-placeholder defect itself, and without the core
    /// root the pause menu, frontend menu, keyboard and splash are all absent.
    #[test]
    fn launch_supplies_the_mod_the_baked_root_and_the_core_root() {
        assert_eq!(
            engine_arguments(&project(), Some(core_root()), os_args(&["maps/e1m1.prl"])),
            os_args(&[
                "--mod",
                "core",
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
                os_args(&["--mod", "expansion", "--baked-root=/elsewhere/baked"]),
            ),
            os_args(&["--mod", "expansion", "--baked-root=/elsewhere/baked"])
        );

        // The equals form suppresses the tool's mod too — but not its baked
        // root or its core root.
        assert_eq!(
            engine_arguments(&project(), Some(core_root()), os_args(&["--mod=expansion"])),
            os_args(&[
                "--baked-root",
                &baked_root(),
                "--core-root",
                "/install/core",
                "--mod=expansion",
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
                "core",
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

    /// Regression: `ProjectLocation::absorb` and `Overrides::absorb_only`
    /// recognized only the split form, and `run` forwards any token it does
    /// not itself recognize straight to the engine (by design, for genuinely
    /// unknown engine flags). So `--project=../other`, `--manifest=…`, and
    /// `--engine=…` all fell through the tool's own parsing and reached the
    /// engine verbatim — which ignores them — while the tool silently fell
    /// back to discovering the project from the working directory. The equals
    /// form must be consumed here, the same as the split form.
    #[test]
    fn equals_form_tool_flags_are_consumed_and_never_forwarded_to_the_engine() {
        let parsed = parse_args(os_args(&[
            "--project=/projects/game",
            "--engine=/build/postretro",
            "maps/e1m1.prl",
        ]))
        .expect("the equals form of every tool flag parses");

        assert_eq!(
            parsed.location.directory(),
            Some(Path::new("/projects/game"))
        );
        assert_eq!(
            parsed.engine_args,
            os_args(&["maps/e1m1.prl"]),
            "neither --project= nor --engine= reached the engine"
        );
    }

    /// `--install-root` is a tool flag, not an engine one, in either spelling.
    #[test]
    fn the_install_roots_equals_form_is_also_consumed_and_never_forwarded() {
        let equals_flag = format!("{INSTALL_ROOT_FLAG}=/bundle");
        let parsed = parse_args(os_args(&[&equals_flag, "maps/e1m1.prl"]))
            .expect("the equals form of --install-root parses");

        assert_eq!(parsed.named_install_root, Some(PathBuf::from("/bundle")));
        assert_eq!(parsed.engine_args, os_args(&["maps/e1m1.prl"]));
    }

    /// A repeat is a repeat whichever form supplied the first value.
    #[test]
    fn install_root_given_twice_errors_across_split_and_equals_forms() {
        let equals_flag = format!("{INSTALL_ROOT_FLAG}=/other");
        let error = parse_args(os_args(&[INSTALL_ROOT_FLAG, "/bundle", &equals_flag]))
            .err()
            .expect("a second --install-root in the other form is still a repeat");
        assert!(error.contains(INSTALL_ROOT_FLAG), "{error}");
        assert!(error.contains("given twice"), "{error}");
    }

    /// Regression: `run` opened the project without rebasing, so a relative
    /// `--project` left the project root relative. This command pins the
    /// engine's working directory to that root and then hands the engine
    /// `--baked-root <project>/baked`, which the engine resolves against the
    /// directory just pinned — doubling the relative segment and pointing the
    /// materials lookup at a directory no compiler ever wrote. The engine warns
    /// per texture and exits zero, so only the resolved path shows it.
    #[test]
    fn a_relative_project_flag_becomes_absolute_before_any_path_is_derived() {
        let temp = std::env::temp_dir().join(format!(
            "postretro-run-relative-project-{}",
            std::process::id()
        ));
        let project_dir = temp.join("game");
        std::fs::create_dir_all(&project_dir).expect("temporary project created");
        std::fs::write(
            project_dir.join(crate::project::MARKER_FILE),
            "[package]\nname = \"game\"\nmod = \"base\"\n",
        )
        .expect("marker written");

        let mut cli = parse_args(os_args(&["--project", "game"])).expect("tool arguments parse");
        cli.location.rebase(&temp);
        let project = cli.location.open(&temp).expect("the named project opens");

        assert!(
            project.root().is_absolute(),
            "project root stayed relative: {}",
            project.root().display(),
        );
        let launch = engine_arguments(&project, None, cli.engine_args);
        let baked = launch
            .iter()
            .position(|argument| argument == "--baked-root")
            .and_then(|index| launch.get(index + 1))
            .map(PathBuf::from)
            .expect("run supplies a baked root");
        assert_eq!(baked, project_dir.join("baked"));
        assert!(baked.is_absolute(), "{}", baked.display());

        let _ = std::fs::remove_dir_all(&temp);
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

    /// A launch drives only the authoring engine, so its own `--engine` override
    /// is consumed and never forwarded — the flag `run` genuinely uses.
    #[test]
    fn run_accepts_the_authoring_engine_flag_it_drives() {
        let parsed = parse_args(os_args(&["--engine", "/build/postretro", "maps/e1m1.prl"]))
            .expect("the one helper run drives is accepted");
        assert_eq!(parsed.engine_args, os_args(&["maps/e1m1.prl"]));
    }

    /// A helper flag `run` does not drive is rejected outright rather than
    /// swallowed — and, crucially, rather than forwarded to the engine, which is
    /// how a genuinely unknown flag is treated.
    #[test]
    fn run_rejects_a_helper_flag_it_never_consumes() {
        let error = parse_args(os_args(&[
            "--scripts-build",
            "/build/scripts-build",
            "maps/e1m1.prl",
        ]))
        .err()
        .expect("a helper flag a launch never drives is rejected");
        assert!(error.contains("--scripts-build"), "{error}");
    }
}
