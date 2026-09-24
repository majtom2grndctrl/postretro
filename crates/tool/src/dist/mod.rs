//! Player payload assembly: the lean, runnable distribution of a project.
//!
//! Stage 1 — building the release binaries — belongs to whoever hands this tool
//! their paths (`binaries.rs`). What remains is stages 2 through 7, which need
//! no compiler and no repository, only the project marker and the helper
//! binaries. Stages 2 through 4 write nothing into the payload root; that
//! ordering, not any claim about which inputs they read, is what makes stage 5's
//! delete safe.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::binaries::{Helper, Overrides, binary_name};
use crate::engine_trees;
use crate::flag::match_flag;
use crate::project::{Project, ProjectLocation};

pub(crate) mod launcher;
pub(crate) mod payload;
pub(crate) mod resolve;
pub(crate) mod stages;

use payload::{
    MARKER_NAME, copy_filtered_tree, count_payload, remove_if_exists, remove_pack_locks,
    replace_payload_root, sweep_payload,
};
use resolve::{EntryExt, Resolved, bake_order, guard_payload_root, outstanding_outputs};
use stages::BakeTarget;

/// What stages 2 and 3 establish for every stage after them.
struct RunState {
    entry_ext: EntryExt,
    entry_script: PathBuf,
    resolved: Vec<Resolved>,
}

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let cli = parse_args(args, "dist")?;
    let project = cli.project()?;
    let install_root = cli.install_root()?;
    let output_root = cli.output_root(&project);
    let payload_root = output_root.join(&project.manifest().package.name);

    guard_payload_root(&payload_root, project.root()).map_err(|error| error.to_string())?;

    let scripts_build = cli.binaries.resolve(Helper::ScriptsBuild)?;
    let prl_build = cli.binaries.resolve(Helper::PrlBuild)?;
    let engine = cli.binaries.resolve(Helper::ReleaseEngine)?;

    let (entry_ext, entry_script) = stages::emit_entry_script(&project, &scripts_build)?;
    let resolved = stages::resolve_levels(&entry_script, &project)?;
    let state = RunState {
        entry_ext,
        entry_script,
        resolved,
    };

    stages::bake_model_textures(&project)?;
    assemble_payload(
        &project,
        &install_root,
        &output_root,
        &payload_root,
        &engine,
        &state,
    )?;
    stages::bake_levels(
        &project,
        &BakeTarget {
            output_root: &output_root,
            payload_root: &payload_root,
            marker_name: &project.manifest().package.name,
            payload_mod_root: project.mod_root_rel(),
        },
        &prl_build,
        &state.resolved,
    )?;
    stages::copy_materials(&project, &payload_root)?;
    let locks = remove_pack_locks(&payload_root)?;
    if locks > 0 {
        println!("  removed {locks} publication locks left by the level bakes");
    }
    sweep_payload(
        &payload_root,
        Path::new(project.mod_root_rel()),
        state.entry_ext,
        &state.resolved,
    )?;
    fs::remove_file(payload_root.join(MARKER_NAME)).map_err(|error| {
        format!(
            "payload sweep: remove completion marker {}: {error}",
            payload_root.join(MARKER_NAME).display()
        )
    })?;

    let (files, bytes) = count_payload(&payload_root)?;
    println!(
        "Distribution complete: {files} files, {bytes} bytes at {}",
        payload_root.display()
    );
    Ok(0)
}

/// Stage 5: replace the payload root, then write the engine, the launcher, the
/// engine-owned `core/` tree, and the mod tree published at the project's own
/// declared mod root.
fn assemble_payload(
    project: &Project,
    install_root: &Path,
    output_root: &Path,
    payload_root: &Path,
    engine: &Path,
    state: &RunState,
) -> Result<(), String> {
    println!("Stage 5: assemble payload tree");
    // Re-proven immediately before the delete: a multi-minute run gives the
    // filesystem time to change under the first check.
    guard_payload_root(payload_root, project.root())
        .map_err(|error| format!("stage 5: {error}"))?;
    let package_name = &project.manifest().package.name;
    // Engine-owned assets (UI descriptors, splash, font licences). `core/` sits
    // at the payload root beside `content/` and `baked/`: it is not a mod root,
    // so `--mod` never redirects it — and it comes from the install, never from
    // the project, so a game cannot shadow it by having a directory of that name.
    //
    // Resolved *before* the delete below. It is one `is_dir` check, and a wrong
    // or missing install root would otherwise cost the previous good payload
    // before anything diagnosed it.
    let core = engine_trees::resolve(install_root, engine_trees::CORE_TREE)?;
    println!("  core/ from the install at {}", core.display());

    let ordered = bake_order(&state.resolved);
    replace_payload_root(
        output_root,
        payload_root,
        package_name,
        &outstanding_outputs(&ordered, 0),
    )?;

    fs::copy(engine, payload_root.join(binary_name("postretro")))
        .map_err(|error| format!("stage 5: copy release engine {}: {error}", engine.display()))?;
    launcher::emit_launcher(payload_root, package_name, project.mod_name())?;

    let source_mod_root = project.mod_root();
    copy_filtered_tree(
        &core,
        &payload_root.join(engine_trees::CORE_TREE),
        &source_mod_root,
        state.entry_ext,
    )?;
    copy_filtered_tree(
        &source_mod_root,
        &payload_root.join(project.mod_root_rel()),
        &source_mod_root,
        state.entry_ext,
    )?;

    let payload_mod_root = payload_root.join(project.mod_root_rel());
    remove_if_exists(&payload_mod_root.join("start-script.js"))?;
    remove_if_exists(&payload_mod_root.join("start-script.luau"))?;
    fs::copy(
        &state.entry_script,
        payload_mod_root.join(state.entry_ext.file_name()),
    )
    .map_err(|error| {
        format!(
            "stage 5: install entry script {}: {error}",
            state.entry_script.display()
        )
    })?;
    Ok(())
}

/// Arguments shared by `dist` and, minus `--out`, every other project command.
///
/// The project and the install are two independent lookups. Neither falls back
/// to the other: a game never supplies engine-owned trees, and an install never
/// supplies a game.
pub(crate) struct DistArgs {
    pub(crate) location: ProjectLocation,
    pub(crate) named_install_root: Option<PathBuf>,
    pub(crate) output_root: Option<PathBuf>,
    pub(crate) binaries: Overrides,
}

impl DistArgs {
    pub(crate) fn project(&self) -> Result<Project, String> {
        let working_directory = std::env::current_dir()
            .map_err(|error| format!("read the working directory: {error}"))?;
        self.location.open(&working_directory)
    }

    /// The install root: named outright, else derived from this executable.
    pub(crate) fn install_root(&self) -> Result<PathBuf, String> {
        match &self.named_install_root {
            Some(root) => Ok(root.clone()),
            None => engine_trees::default_install_root(),
        }
    }

    pub(crate) fn output_root(&self, project: &Project) -> PathBuf {
        self.output_root
            .clone()
            .unwrap_or_else(|| project.output_root())
    }
}

/// Parse `[--manifest <path>] [--out <dir>]` plus the helper-binary overrides.
///
/// Relative paths resolve against the working directory, which is the only
/// anchor available before the project is found.
pub(crate) fn parse_args(args: Vec<OsString>, command: &str) -> Result<DistArgs, String> {
    let invocation_dir =
        std::env::current_dir().map_err(|error| format!("read the working directory: {error}"))?;
    let mut location = ProjectLocation::default();
    let mut named_install_root = None;
    let mut output_root = None;
    let mut binaries = Overrides::default();
    let mut index = 0;

    while index < args.len() {
        let flag = args[index].to_str().ok_or_else(|| {
            format!(
                "argument {} is not valid UTF-8",
                args[index].to_string_lossy()
            )
        })?;
        let value = args.get(index + 1);
        let consumed = binaries.absorb(flag, value)?;
        if consumed > 0 {
            index += consumed;
            continue;
        }
        let consumed = location.absorb(flag, value)?;
        if consumed > 0 {
            index += consumed;
            continue;
        }

        let mut matched_own_flag = false;
        for (name, slot) in [
            (engine_trees::INSTALL_ROOT_FLAG, &mut named_install_root),
            ("--out", &mut output_root),
        ] {
            let Some(matched) = match_flag(flag, value, name) else {
                continue;
            };
            let value = matched
                .value
                .ok_or_else(|| format!("{name} requires a path\n\n{}", usage(command)))?;
            let path = absolute_from(&invocation_dir, PathBuf::from(value.into_owned()));
            set_once(slot, path, name)?;
            index += matched.tokens;
            matched_own_flag = true;
            break;
        }
        if matched_own_flag {
            continue;
        }
        return Err(format!("unknown argument `{flag}`\n\n{}", usage(command)));
    }
    location.rebase(&invocation_dir);

    Ok(DistArgs {
        location,
        named_install_root,
        output_root,
        binaries,
    })
}

fn absolute_from(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn set_once(slot: &mut Option<PathBuf>, value: PathBuf, flag: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("{flag} may be given only once"));
    }
    Ok(())
}

pub(crate) fn usage(command: &str) -> String {
    format!(
        "{command} usage:\n  postretro-tool {command} [--project <dir> | --manifest <path>] \
         [--install-root <dir>] [--out <dir>]\n{}",
        crate::binaries::HELPER_USAGE
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A distribution publishes under the project's *own* declared mod, so the
    /// runtime's `baked/` grandparent derivation depends on the published path's
    /// shape. The manifest names only the mod and the tool places it under
    /// `content/`, which is what guarantees the two components the derivation
    /// needs; here we pin that a project hands its declared mod straight through
    /// to the payload with no fixed rename.
    #[test]
    fn the_payload_mod_root_is_the_projects_own_declared_mod() {
        let project = Project::for_test("/projects/game", "game", "dev");
        assert_eq!(project.mod_root_rel(), "content/dev");
        let components: Vec<&str> = project.mod_root_rel().split('/').collect();
        assert_eq!(components.len(), 2, "{}", project.mod_root_rel());
        assert!(components.iter().all(|component| !component.is_empty()));
    }

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    /// Regression: `--install-root` and `--out` were matched only in their
    /// split form, so `dist`/`sdk-dist` refused the equals form outright as
    /// an unknown argument — loud, unlike `run`'s silent forwarding, but still
    /// inconsistent with every other flag these commands accept.
    ///
    /// Expected values go through `absolute_from` too, the same as production:
    /// a bare `/install` is not an absolute path on every platform (Windows
    /// needs a drive prefix), so the parser's own join against the invocation
    /// directory is part of the contract being pinned, not a detail to hide.
    #[test]
    fn install_root_and_out_accept_the_equals_form() {
        let invocation_dir = std::env::current_dir().expect("read the working directory");
        let install_root_flag = format!("{}=/install", engine_trees::INSTALL_ROOT_FLAG);
        let parsed = parse_args(
            os_args(&[&install_root_flag, "--out=/somewhere/out"]),
            "dist",
        )
        .expect("the equals form of both flags parses");

        assert_eq!(
            parsed.named_install_root,
            Some(absolute_from(&invocation_dir, PathBuf::from("/install")))
        );
        assert_eq!(
            parsed.output_root,
            Some(absolute_from(
                &invocation_dir,
                PathBuf::from("/somewhere/out")
            ))
        );
    }

    /// A repeat is a repeat whichever form supplied the first value.
    #[test]
    fn out_given_twice_errors_across_split_and_equals_forms() {
        let error = parse_args(os_args(&["--out", "/first", "--out=/second"]), "dist")
            .err()
            .expect("a second --out in the other form is still a repeat");
        assert!(error.contains("--out"), "{error}");
        assert!(error.contains("only once"), "{error}");
    }

    /// The project and helper-binary flags this command shares with `run`
    /// must accept the equals form here too — the same parser recognizes
    /// them via `Overrides::absorb`/`ProjectLocation::absorb`.
    ///
    /// The expected project directory is derived the same way `parse_args`
    /// derives its own — `ProjectLocation::absorb` then `rebase` against the
    /// invocation directory — rather than a literal, since a bare
    /// `/projects/game` is not absolute on every platform and `rebase` joins
    /// it against the working directory when it isn't.
    #[test]
    fn project_and_helper_flags_accept_the_equals_form() {
        let invocation_dir = std::env::current_dir().expect("read the working directory");
        let parsed = parse_args(
            os_args(&["--project=/projects/game", "--prl-build=/build/prl-build"]),
            "dist",
        )
        .expect("the equals form of the shared flags parses");

        let mut expected_location = ProjectLocation::default();
        expected_location
            .absorb("--project", Some(&OsString::from("/projects/game")))
            .expect("the split form records the same project");
        expected_location.rebase(&invocation_dir);

        assert_eq!(parsed.location.directory(), expected_location.directory());
        // The override path does not exist on this machine, so `resolve` still
        // errors — but the error names the path this parse recorded, which is
        // what proves the equals form was absorbed rather than left unset (an
        // unset override resolves by search instead, and would not mention it).
        let error = parsed
            .binaries
            .resolve(Helper::PrlBuild)
            .expect_err("the fixture path does not exist on this machine");
        assert!(error.contains("/build/prl-build"), "{error}");
    }
}
