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
use crate::project::Project;

pub(crate) mod launcher;
pub(crate) mod payload;
pub(crate) mod resolve;
pub(crate) mod stages;

use payload::{
    MARKER_NAME, PAYLOAD_MOD_ROOT, copy_filtered_tree, count_payload, remove_if_exists,
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
    assemble_payload(&project, &output_root, &payload_root, &engine, &state)?;
    stages::bake_levels(
        &project,
        &BakeTarget {
            output_root: &output_root,
            payload_root: &payload_root,
            marker_name: &project.manifest().package.name,
            payload_mod_root: PAYLOAD_MOD_ROOT,
        },
        &prl_build,
        &state.resolved,
    )?;
    stages::copy_materials(&project, &payload_root)?;
    sweep_payload(
        &payload_root,
        Path::new(PAYLOAD_MOD_ROOT),
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
/// engine-owned `core/` tree, and the mod tree published at `content/base`.
fn assemble_payload(
    project: &Project,
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
    let ordered = bake_order(&state.resolved);
    replace_payload_root(
        output_root,
        payload_root,
        package_name,
        &outstanding_outputs(&ordered, 0),
    )?;

    fs::copy(engine, payload_root.join(binary_name("postretro")))
        .map_err(|error| format!("stage 5: copy release engine {}: {error}", engine.display()))?;
    launcher::emit_launcher(payload_root, package_name, PAYLOAD_MOD_ROOT)?;

    let source_mod_root = project.mod_root();
    // Engine-owned assets (UI descriptors, splash, font licences). `core/` sits
    // at the payload root beside `content/` and `baked/`: it is not a mod root,
    // so `--mod` never redirects it.
    copy_filtered_tree(
        &project.join("core"),
        &payload_root.join("core"),
        &source_mod_root,
        state.entry_ext,
    )?;
    copy_filtered_tree(
        &source_mod_root,
        &payload_root.join(PAYLOAD_MOD_ROOT),
        &source_mod_root,
        state.entry_ext,
    )?;

    let payload_mod_root = payload_root.join(PAYLOAD_MOD_ROOT);
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
pub(crate) struct DistArgs {
    pub(crate) manifest_path: Option<PathBuf>,
    pub(crate) output_root: Option<PathBuf>,
    pub(crate) binaries: Overrides,
}

impl DistArgs {
    /// Open the project: an explicit `--manifest`, else the marker walk.
    pub(crate) fn project(&self) -> Result<Project, String> {
        match &self.manifest_path {
            Some(path) => Project::open(path),
            None => {
                let working_directory = std::env::current_dir()
                    .map_err(|error| format!("read the working directory: {error}"))?;
                Project::discover(&working_directory)
            }
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
    let mut manifest_path = None;
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
        if binaries.absorb(flag, value)? {
            index += 2;
            continue;
        }

        let value = match flag {
            "--manifest" | "--out" => value
                .ok_or_else(|| format!("{flag} requires a path\n\n{}", usage(command)))?
                .clone(),
            _ => return Err(format!("unknown argument `{flag}`\n\n{}", usage(command))),
        };
        let path = absolute_from(&invocation_dir, PathBuf::from(value));
        match flag {
            "--manifest" => set_once(&mut manifest_path, path, flag)?,
            "--out" => set_once(&mut output_root, path, flag)?,
            _ => unreachable!("only recognized flags reach this branch"),
        }
        index += 2;
    }

    Ok(DistArgs {
        manifest_path,
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
        "{command} usage:\n  postretro-tool {command} [--manifest <path>] [--out <dir>]\n{}",
        crate::binaries::HELPER_USAGE
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The payload's mod root is the thing D6 changes, and the runtime's
    /// materials derivation depends on its shape rather than its name.
    #[test]
    fn payload_mod_root_keeps_the_two_component_shape_a_mod_root_must_have() {
        let components: Vec<&str> = PAYLOAD_MOD_ROOT.split('/').collect();
        assert_eq!(components.len(), 2, "{PAYLOAD_MOD_ROOT}");
        assert!(components.iter().all(|component| !component.is_empty()));
        assert_ne!(
            components[0], "dist",
            "the payload tree is where the delete works"
        );
    }
}
