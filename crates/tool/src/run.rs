//! `run`: the external authoring loop's launcher.
//!
//! The engine learns nothing about `postretro.toml` — it keeps plain flags, so
//! changing the manifest schema stays a tool change. What this removes is the
//! two paths a modder would otherwise type on every launch, whose failure mode
//! is not an error but every world texture quietly degrading to a placeholder
//! (`build_pipeline.md` §Baked texture mips).
//!
//! See: context/lib/build_pipeline.md §Baked texture mips

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::binaries::{Helper, Overrides, status_code};
use crate::project::Project;

/// The tool's own flags, and everything forwarded to the engine untouched.
struct RunArgs {
    manifest_path: Option<PathBuf>,
    binaries: Overrides,
    engine_args: Vec<OsString>,
}

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let cli = parse_args(args)?;
    let project = match &cli.manifest_path {
        Some(manifest) => Project::open(manifest)?,
        None => Project::discover(
            &std::env::current_dir()
                .map_err(|error| format!("read the working directory: {error}"))?,
        )?,
    };
    let engine = cli.binaries.resolve(Helper::AuthoringEngine)?;
    let launch_args = engine_arguments(&project, cli.engine_args);

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

/// Prepend the flags the project already answers, unless the caller set them.
///
/// An explicit `--mod`, `--content-root`, or `--baked-root` wins outright rather
/// than being shadowed: the engine reads the first occurrence of each, so
/// supplying ours unconditionally would silently discard the caller's.
fn engine_arguments(project: &Project, engine_args: Vec<OsString>) -> Vec<OsString> {
    let mut launch_args = Vec::with_capacity(engine_args.len() + 4);
    if !names_flag(&engine_args, &["--mod", "--content-root"]) {
        launch_args.push(OsString::from("--mod"));
        launch_args.push(OsString::from(&project.manifest().package.mod_root));
    }
    if !names_flag(&engine_args, &["--baked-root"]) {
        launch_args.push(OsString::from("--baked-root"));
        launch_args.push(project.baked_root().into_os_string());
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
    let mut manifest_path = None;
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
        if flag == "--manifest" {
            let value = value.ok_or_else(|| usage("--manifest requires a path"))?;
            if manifest_path.replace(PathBuf::from(value)).is_some() {
                return Err(usage("--manifest may be given only once"));
            }
            index += 2;
            continue;
        }
        engine_args.push(args[index].clone());
        index += 1;
    }

    Ok(RunArgs {
        manifest_path,
        binaries,
        engine_args,
    })
}

fn usage(message: &str) -> String {
    format!(
        "{message}\n\nUsage: postretro-tool run [--manifest <path>] [--engine <path>] \
         [engine args...]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn baked_root() -> String {
        project().baked_root().to_string_lossy().into_owned()
    }

    fn project() -> Project {
        Project::for_test("/projects/game", "game", "levels/core")
    }

    /// The two flags travel together or not at all: the baked root without the
    /// mod root launches against the wrong content, and the mod root without the
    /// baked root is the silent-placeholder defect itself.
    #[test]
    fn launch_supplies_both_the_mod_root_and_the_baked_root() {
        assert_eq!(
            engine_arguments(&project(), os_args(&["maps/e1m1.prl"])),
            os_args(&[
                "--mod",
                "levels/core",
                "--baked-root",
                &baked_root(),
                "maps/e1m1.prl",
            ])
        );
    }

    #[test]
    fn an_explicit_flag_wins_rather_than_being_shadowed() {
        assert_eq!(
            engine_arguments(
                &project(),
                os_args(&["--mod", "levels/expansion", "--baked-root=/elsewhere/baked"]),
            ),
            os_args(&["--mod", "levels/expansion", "--baked-root=/elsewhere/baked"])
        );

        // `--content-root` selects the same thing `--mod` does, so it suppresses
        // the tool's mod root too — but not its baked root.
        assert_eq!(
            engine_arguments(&project(), os_args(&["--content-root", "levels/x"])),
            os_args(&["--baked-root", &baked_root(), "--content-root", "levels/x",])
        );
    }

    #[test]
    fn unknown_engine_flags_forward_verbatim() {
        let parsed = parse_args(os_args(&["--headless", "runspec.json", "--pool-seed=17"]))
            .expect("engine arguments forward");
        assert_eq!(parsed.manifest_path, None);
        assert_eq!(
            parsed.engine_args,
            os_args(&["--headless", "runspec.json", "--pool-seed=17"])
        );
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
            parsed.manifest_path,
            Some(PathBuf::from("/projects/game/postretro.toml"))
        );
        assert_eq!(parsed.engine_args, os_args(&["maps/e1m1.prl"]));
    }
}
