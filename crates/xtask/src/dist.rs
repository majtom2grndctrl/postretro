//! `sdk-dist`: build the SDK bundle's binaries, then hand the assembly over.
//!
//! Building binaries needs cargo; assembling a distribution is pure content
//! work and does not, which is why it lives in `postretro-tool` — a binary that
//! ships inside the SDK bundle and can therefore run on a machine with no
//! repository and no toolchain. `xtask` owns exactly the half that cannot
//! travel: it compiles the binaries and tells the tool where they are. A player
//! payload has no xtask half at all: a game developer runs the bundle's
//! `bin/postretro-tool dist`, which `sdk-check` scripts.
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

/// How stage 1 builds a binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profile {
    /// `--release` with default features: no dev-tools, observability or
    /// capture surface, so no camera-route playback, recording or cap seed.
    Release,
    /// The SDK's authoring engine: debug, for TS auto-compile and hot reload,
    /// with `dev-tools`, which carries the inspector and camera-route
    /// playback, recording and the drain-cap seed.
    DebugDevTools,
}

impl Profile {
    /// The cargo target subdirectory the binary lands in.
    fn target_subdir(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::DebugDevTools => "debug",
        }
    }

    /// Non-default cargo features the binary builds with.
    fn features(self) -> &'static [&'static str] {
        match self {
            Self::Release => &[],
            Self::DebugDevTools => &["dev-tools"],
        }
    }
}

/// One binary stage 1 builds: its cargo package and binary, the tool flag
/// naming the result, and how it is built.
#[derive(Debug, Clone, Copy)]
struct Build {
    package: &'static str,
    binary: &'static str,
    flag: &'static str,
    profile: Profile,
}

impl Build {
    /// The `cargo` arguments that build this binary.
    fn cargo_args(&self) -> Vec<&'static str> {
        let mut args = vec!["build"];
        if self.profile == Profile::Release {
            args.push("--release");
        }
        args.extend(["-p", self.package, "--bin", self.binary]);
        for feature in self.profile.features() {
            args.extend(["--features", *feature]);
        }
        args
    }

    fn describe(&self) -> String {
        match self.profile {
            Profile::Release => format!("release {}", self.binary),
            Profile::DebugDevTools => format!("debug dev-tools {}", self.binary),
        }
    }
}

const RELEASE_ENGINE: Build = Build {
    package: "postretro",
    binary: "postretro",
    flag: "--release-engine",
    profile: Profile::Release,
};
const AUTHORING_ENGINE: Build = Build {
    package: "postretro",
    binary: "postretro",
    flag: "--engine",
    profile: Profile::DebugDevTools,
};
const PRL_BUILD: Build = Build {
    package: "postretro-level-compiler",
    binary: "prl-build",
    flag: "--prl-build",
    profile: Profile::Release,
};
const SCRIPTS_BUILD: Build = Build {
    package: "postretro-script-compiler",
    binary: "scripts-build",
    flag: "--scripts-build",
    profile: Profile::Release,
};
// Kept a separate binary the tool execs rather than linked code: it pulls in
// the scripting runtime, so linking it would drag rquickjs and mlua into a
// tool that needs neither.
const MINT_IDENTITY: Build = Build {
    package: "postretro-sim",
    binary: "mint-identity",
    flag: "--mint-identity",
    profile: Profile::Release,
};
/// The tool, which runs the assembly and carries `generate-camera-route`.
const TOOL: Build = Build {
    package: "postretro-tool",
    binary: "postretro-tool",
    flag: "--tool",
    profile: Profile::Release,
};

/// The SDK bundle's binaries, handed to `postretro-tool sdk-dist`. The bundle
/// ships the tool itself, so it is both what runs the assembly and one of the
/// binaries being copied; naming it keeps that from resting on the tool
/// happening to find itself beside itself.
const SDK_DIST_BUILDS: [Build; 6] = [
    AUTHORING_ENGINE,
    RELEASE_ENGINE,
    PRL_BUILD,
    SCRIPTS_BUILD,
    MINT_IDENTITY,
    TOOL,
];

pub(crate) fn sdk_dist(args: Vec<OsString>) -> Result<i32, String> {
    let context = Context::prepare("sdk-dist")?;

    println!(
        "Stage 1: build the debug dev-tools engine, the release engine, the compilers, \
         mint-identity, and postretro-tool"
    );
    let binaries = SDK_DIST_BUILDS
        .iter()
        .map(|build| context.build(build))
        .collect::<Result<Vec<_>, _>>()?;
    let tool = binaries
        .iter()
        .find(|binary| binary.flag == TOOL.flag)
        .map(|binary| binary.path.clone())
        .expect("SDK_DIST_BUILDS names the tool");

    context.delegate(&tool, "sdk-dist", &binaries, args)
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

    /// Builds one binary from the workspace and finds it. The tool is then
    /// named by its own path rather than run through `cargo run`, so the
    /// child's stdio and exit code stay the tool's own.
    fn build(&self, build: &Build) -> Result<BuiltBinary, String> {
        let mut command = Command::new(&self.cargo);
        command
            .current_dir(&self.workspace)
            .args(build.cargo_args());
        let label = build.describe();
        run_checked(
            &mut command,
            &format!("{} stage 1 build {label}", self.label),
        )?;
        self.built(
            build.flag,
            build.profile.target_subdir(),
            build.binary,
            &label,
        )
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
        let forwarded = forwarded_args(args, &self.workspace)?;
        let mut command = Command::new(tool);
        command.current_dir(&self.workspace).arg(subcommand);
        // The engine-owned trees resolve under the install root and never under
        // the project. A checkout is the one layout the tool cannot derive that
        // from — it sits in `target/debug` with no `core/` beside it — but the
        // workspace *is* the install, and this crate is the one entitled to know
        // that. Naming it here keeps the tool's rule single and unconditional.
        //
        // Skipped when the caller named one: the tool accepts the flag once, so
        // adding ours unconditionally would turn every deliberate override into
        // `--install-root may be given only once`.
        if !names_install_root(&forwarded) {
            command.arg("--install-root").arg(&self.workspace);
        }
        for binary in binaries {
            command.arg(binary.flag).arg(&binary.path);
        }
        command
            .args(forwarded)
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

/// Whether the caller already named the install root, in either spelling.
fn names_install_root(args: &[OsString]) -> bool {
    args.iter().any(|argument| {
        argument.to_str().is_some_and(|argument| {
            argument == "--install-root" || argument.starts_with("--install-root=")
        })
    })
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
            "--manifest" | "--project" | "--install-root" => Some(invocation_dir),
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

pub(crate) fn binary_name(name: &str) -> String {
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

    /// A checkout is the one layout whose install root the tool cannot derive —
    /// it sits in `target/debug` with no `core/` beside it — so this crate names
    /// the workspace. A caller may still override it, and that override is a
    /// path like any other.
    #[test]
    fn a_caller_supplied_install_root_rebases_like_the_other_path_flags() {
        assert_eq!(
            rebase_path_flags(
                os_args(&["--install-root", "other-install", "--project", "game"]),
                Path::new("/work/here"),
                Path::new("/work/repo"),
            ),
            vec![
                OsString::from("--install-root"),
                Path::new("/work/here").join("other-install").into(),
                OsString::from("--project"),
                Path::new("/work/here").join("game").into(),
            ]
        );
    }

    /// Regression: the workspace install root was prepended unconditionally, so
    /// the override the test above rebases could never take effect — the tool
    /// accepts the flag once and refused the pair with
    /// `--install-root may be given only once`.
    #[test]
    fn a_caller_supplied_install_root_suppresses_the_workspace_default() {
        assert!(names_install_root(&os_args(&[
            "--install-root",
            "/elsewhere"
        ])));
        assert!(names_install_root(&os_args(&["--install-root=/elsewhere"])));
        assert!(!names_install_root(&os_args(&["--out", "ship"])));
        assert!(
            !names_install_root(&os_args(&["--project", "install-root"])),
            "a value that merely resembles the flag is not the flag"
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

    /// The feature camera-route playback, recording and the drain-cap seed
    /// build behind, read from the engine's own declaration of it.
    fn camera_route_feature() -> String {
        let flags = workspace_root()
            .expect("workspace root")
            .join("crates/postretro/src/startup/camera_route_flags.rs");
        let source = std::fs::read_to_string(&flags).expect("the engine's camera-route flags");
        let declaration = "pub(crate) const CAMERA_ROUTE_FEATURE: &str = \"";
        let start = source
            .find(declaration)
            .expect("the engine names the feature playback builds behind")
            + declaration.len();
        let end = start + source[start..].find('"').expect("a closing quote");
        source[start..end].to_string()
    }

    fn features_of(args: &[&str]) -> Vec<String> {
        assert!(
            !args.contains(&"--all-features") && !args.contains(&"--no-default-features"),
            "{args:?}"
        );
        args.windows(2)
            .filter(|pair| pair[0] == "--features")
            .map(|pair| pair[1].to_string())
            .collect()
    }

    // The SDK bundle's authoring engine builds debug with the feature
    // playback, recording and the cap seed ride. Every other binary builds
    // release with default features — the release engine is the one every
    // player payload built with the bundle ships, so a payload carries no
    // camera-route playback, recording or cap seed. The bundle ships the
    // tool, which carries `generate-camera-route`.
    #[test]
    fn sdk_bundle_authoring_engine_carries_playback_and_its_release_engine_does_not() {
        let feature = camera_route_feature();
        let by_flag = |flag: &str| -> Vec<&'static str> {
            let builds: Vec<&Build> = SDK_DIST_BUILDS
                .iter()
                .filter(|build| build.flag == flag)
                .collect();
            assert_eq!(builds.len(), 1, "one {flag} build");
            builds[0].cargo_args()
        };

        let authoring = by_flag("--engine");
        assert_eq!(
            authoring,
            [
                "build",
                "-p",
                "postretro",
                "--bin",
                "postretro",
                "--features",
                "dev-tools"
            ]
        );
        assert!(
            features_of(&authoring).contains(&feature),
            "the authoring engine builds with `{feature}`"
        );
        assert_eq!(AUTHORING_ENGINE.profile.target_subdir(), "debug");

        let release = by_flag("--release-engine");
        assert!(release.contains(&"--release"), "{release:?}");
        assert!(release.contains(&"postretro"), "{release:?}");
        for build in SDK_DIST_BUILDS
            .iter()
            .filter(|build| build.flag != "--engine")
        {
            let args = build.cargo_args();
            assert!(
                features_of(&args).is_empty(),
                "{} builds with default features",
                build.flag
            );
            assert!(args.contains(&"--release"), "{args:?}");
            assert_eq!(build.profile.target_subdir(), "release");
        }

        let tool = by_flag("--tool");
        assert!(tool.contains(&"postretro-tool"), "{tool:?}");
        let root = workspace_root().expect("workspace root");
        let read = |path: &str| std::fs::read_to_string(root.join(path)).expect(path);
        // These pin the two call sites by their source text, not their
        // behaviour: a reformat of either line trips them, so update the
        // strings with it.
        assert!(
            read("crates/tool/src/main.rs")
                .contains("\"generate-camera-route\" => camera_route::run(rest)"),
            "the tool dispatches the generate verb"
        );
        assert!(
            read("crates/tool/src/sdk_dist/assemble.rs")
                .contains("(&binaries.tool, \"postretro-tool\")"),
            "sdk-dist copies the tool into the bundle's bin/"
        );
    }
}
