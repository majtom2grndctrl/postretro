//! `dist` and `sdk-dist`: build the binaries, then hand the content work over.
//!
//! Stage 1 of `build_pipeline.md` §Distribution packaging builds release
//! binaries and needs cargo. Stages 2 through 7 are pure content work and do
//! not, which is why they live in `postretro-tool` — a binary that ships inside
//! the SDK bundle and can therefore run on a machine with no repository and no
//! toolchain. `xtask` owns exactly the half that cannot travel: it compiles the
//! binaries and tells the tool where they are.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::{run_checked, status_code, workspace_root};

/// One cargo-built binary a distribution run needs, and the flag naming it.
struct BuiltBinary {
    flag: &'static str,
    path: PathBuf,
}

pub(crate) fn dist(args: Vec<OsString>) -> Result<i32, String> {
    let context = Context::prepare("dist")?;

    println!("Stage 1: build release postretro, prl-build, scripts-build, and postretro-tool");
    let engine = context.build_release("postretro", "postretro", "--release-engine")?;
    let prl_build =
        context.build_release("postretro-level-compiler", "prl-build", "--prl-build")?;
    let scripts_build = context.build_release(
        "postretro-script-compiler",
        "scripts-build",
        "--scripts-build",
    )?;
    let tool = context.tool_binary()?;

    context.delegate(&tool, "dist", &[engine, prl_build, scripts_build], args)
}

pub(crate) fn sdk_dist(args: Vec<OsString>) -> Result<i32, String> {
    let context = Context::prepare("sdk-dist")?;

    println!(
        "Stage 1: build the debug dev-tools engine, the release engine, the compilers, \
         mint-identity, and postretro-tool"
    );
    let authoring_engine = context.build_authoring_engine()?;
    let release_engine = context.build_release("postretro", "postretro", "--release-engine")?;
    let prl_build =
        context.build_release("postretro-level-compiler", "prl-build", "--prl-build")?;
    let scripts_build = context.build_release(
        "postretro-script-compiler",
        "scripts-build",
        "--scripts-build",
    )?;
    // Kept a separate binary the tool execs rather than linked code: it pulls in
    // the scripting runtime, so linking it would drag rquickjs and mlua into a
    // tool that needs neither.
    let mint_identity =
        context.build_release("postretro-sim", "mint-identity", "--mint-identity")?;
    let tool = context.tool_binary()?;

    // The bundle ships the tool itself, so it is both what runs the assembly and
    // one of the binaries being copied. Naming it explicitly keeps that from
    // resting on the tool happening to find itself beside itself.
    context.delegate(
        &tool,
        "sdk-dist",
        &[
            authoring_engine,
            release_engine,
            prl_build,
            scripts_build,
            mint_identity,
            BuiltBinary {
                flag: "--tool",
                path: tool.clone(),
            },
        ],
        args,
    )
}

/// The workspace, its cargo, and its target directory — established once and
/// proven safe before anything is built.
struct Context {
    label: &'static str,
    workspace: PathBuf,
    cargo: OsString,
    target_dir: PathBuf,
}

impl Context {
    fn prepare(label: &'static str) -> Result<Self, String> {
        let workspace = workspace_root()?;
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
        let target_dir = cargo_target_dir(&cargo, &workspace)?;

        // `<target-dir>/release` holds the engine binary at its top level, so a
        // target directory under `dist/` would read as a payload root to the
        // tool's provenance check and permit a delete of the binaries the later
        // stages read.
        let workspace_dist = workspace.join("dist");
        let target_inside_dist = is_at_or_under(&target_dir, &workspace_dist).map_err(|error| {
            format!(
                "{label} stage 1: compare cargo target directory {} with {}: {error}",
                target_dir.display(),
                workspace_dist.display()
            )
        })?;
        if target_inside_dist {
            return Err(format!(
                "{label} stage 1: refuse cargo target directory {} at or under workspace dist/",
                target_dir.display()
            ));
        }

        Ok(Self {
            label,
            workspace,
            cargo,
            target_dir,
        })
    }

    fn build_release(
        &self,
        package: &str,
        binary: &str,
        flag: &'static str,
    ) -> Result<BuiltBinary, String> {
        let mut command = Command::new(&self.cargo);
        command
            .current_dir(&self.workspace)
            .arg("build")
            .arg("--release")
            .arg("-p")
            .arg(package)
            .arg("--bin")
            .arg(binary);
        run_checked(
            &mut command,
            &format!("{} stage 1 build {binary}", self.label),
        )?;
        self.built(flag, "release", binary, &format!("release {binary}"))
    }

    /// The authoring engine: DEBUG (no `--release`) with `--features dev-tools`.
    /// TS auto-compile and hot reload are gated on debug assertions, not on the
    /// feature, so the bundle needs both bits set.
    fn build_authoring_engine(&self) -> Result<BuiltBinary, String> {
        let mut command = Command::new(&self.cargo);
        command
            .current_dir(&self.workspace)
            .arg("build")
            .arg("-p")
            .arg("postretro")
            .arg("--bin")
            .arg("postretro")
            .arg("--features")
            .arg("dev-tools");
        run_checked(
            &mut command,
            &format!("{} stage 1 build postretro (debug, dev-tools)", self.label),
        )?;
        self.built(
            "--engine",
            "debug",
            "postretro",
            "debug dev-tools postretro",
        )
    }

    /// The tool itself. It is named by its own path rather than run through
    /// `cargo run` so the child's stdio and exit code stay the tool's own.
    fn tool_binary(&self) -> Result<PathBuf, String> {
        let mut command = Command::new(&self.cargo);
        command
            .current_dir(&self.workspace)
            .arg("build")
            .arg("--release")
            .arg("-p")
            .arg("postretro-tool")
            .arg("--bin")
            .arg("postretro-tool");
        run_checked(
            &mut command,
            &format!("{} stage 1 build postretro-tool", self.label),
        )?;
        Ok(self
            .built("--tool", "release", "postretro-tool", "postretro-tool")?
            .path)
    }

    fn built(
        &self,
        flag: &'static str,
        profile: &str,
        binary: &str,
        label: &str,
    ) -> Result<BuiltBinary, String> {
        let path = self.target_dir.join(profile).join(binary_name(binary));
        if !path.is_file() {
            return Err(format!(
                "{} stage 1: {label} not found at {}",
                self.label,
                path.display()
            ));
        }
        Ok(BuiltBinary { flag, path })
    }

    /// Run the tool from the workspace, so its own `postretro.toml` walk finds
    /// the repository's project exactly as a bundle recipient's finds theirs.
    fn delegate(
        &self,
        tool: &Path,
        subcommand: &str,
        binaries: &[BuiltBinary],
        args: Vec<OsString>,
    ) -> Result<i32, String> {
        let mut command = Command::new(tool);
        command.current_dir(&self.workspace).arg(subcommand);
        for binary in binaries {
            command.arg(binary.flag).arg(&binary.path);
        }
        command
            .args(forwarded_args(args, &self.workspace)?)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        status_code(
            command
                .status()
                .map_err(|error| format!("launch postretro-tool {subcommand}: {error}")),
        )
    }
}

/// Re-root the path flags so they keep meaning what they meant when the tool
/// ran in-process: `--manifest` relative to where the developer stood,
/// `--out` relative to the workspace.
fn forwarded_args(args: Vec<OsString>, workspace: &Path) -> Result<Vec<OsString>, String> {
    let invocation_dir =
        std::env::current_dir().map_err(|error| format!("read invoking directory: {error}"))?;
    Ok(rebase_path_flags(args, &invocation_dir, workspace))
}

fn rebase_path_flags(
    args: Vec<OsString>,
    invocation_dir: &Path,
    workspace: &Path,
) -> Vec<OsString> {
    let mut forwarded = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_str().unwrap_or_default();
        let base = match flag {
            "--manifest" => Some(invocation_dir),
            "--out" => Some(workspace),
            _ => None,
        };
        forwarded.push(args[index].clone());
        index += 1;

        let (Some(base), Some(value)) = (base, args.get(index)) else {
            continue;
        };
        let path = PathBuf::from(value);
        forwarded.push(if path.is_absolute() {
            value.clone()
        } else {
            base.join(path).into_os_string()
        });
        index += 1;
    }
    forwarded
}

fn cargo_target_dir(cargo: &OsStr, workspace: &Path) -> Result<PathBuf, String> {
    let output = Command::new(cargo)
        .current_dir(workspace)
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--no-deps")
        .output()
        .map_err(|error| format!("stage 1: run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "stage 1: cargo metadata exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("stage 1: parse cargo metadata: {error}"))?;
    let target_dir = metadata["target_directory"]
        .as_str()
        .ok_or("stage 1: cargo metadata omitted target_directory")?;
    Ok(PathBuf::from(target_dir))
}

fn binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Compare a candidate and ancestor after canonicalizing each nearest existing
/// ancestor and then applying the missing tail to that canonical location.
fn is_at_or_under(path: &Path, ancestor: &Path) -> std::io::Result<bool> {
    let path = canonicalize_nearest_existing(path)?;
    let ancestor = canonicalize_nearest_existing(ancestor)?;
    Ok(path.starts_with(&ancestor))
}

fn canonicalize_nearest_existing(path: &Path) -> std::io::Result<PathBuf> {
    use std::io;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut candidate = absolute.as_path();
    let mut missing_tail = Vec::new();

    loop {
        match std::fs::symlink_metadata(candidate) {
            Ok(_) => {
                let mut canonical = std::fs::canonicalize(candidate)?;
                missing_tail.reverse();
                for component in missing_tail {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = candidate.file_name().ok_or(error)?;
                missing_tail.push(name.to_os_string());
                candidate = candidate.parent().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no existing ancestor for {}", absolute.display()),
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    /// The tool resolves relative paths against its own working directory, which
    /// `delegate` pins to the workspace. Rebasing here is what keeps a developer
    /// standing in a subdirectory naming the same manifest they always did.
    #[test]
    fn path_flags_rebase_onto_the_directories_they_were_always_relative_to() {
        let invocation = Path::new("/work/repo/content/dev");
        let workspace = Path::new("/work/repo");

        assert_eq!(
            rebase_path_flags(
                os_args(&["--manifest", "other.toml", "--out", "ship"]),
                invocation,
                workspace,
            ),
            vec![
                OsString::from("--manifest"),
                invocation.join("other.toml").into_os_string(),
                OsString::from("--out"),
                workspace.join("ship").into_os_string(),
            ]
        );
    }

    #[test]
    fn absolute_path_flags_and_unknown_flags_forward_untouched() {
        let forwarded = rebase_path_flags(
            os_args(&["--out", "/ship/here", "--engine", "relative/engine"]),
            Path::new("/work/repo/sub"),
            Path::new("/work/repo"),
        );
        assert_eq!(
            forwarded,
            os_args(&["--out", "/ship/here", "--engine", "relative/engine"]),
            "only the two path flags xtask used to resolve are rebased"
        );
    }

    #[test]
    fn a_trailing_path_flag_forwards_without_panicking() {
        assert_eq!(
            rebase_path_flags(
                os_args(&["--manifest"]),
                Path::new("/work"),
                Path::new("/work")
            ),
            os_args(&["--manifest"]),
            "the tool owns the missing-value diagnostic"
        );
    }
}
