//! Locating the helper binaries the tool drives.
//!
//! The tool never compiles Rust (`build_pipeline.md` §Distribution packaging):
//! it runs binaries somebody else built. Each one is named by an explicit flag,
//! and each flag defaults to a conventional path beside the tool's own
//! executable — `bin/prl-build` next to `bin/postretro-tool` in a shipped SDK
//! bundle, `target/release/prl-build` next to `target/release/postretro-tool` in
//! a workspace checkout. `xtask` passes the paths explicitly; a bundle recipient
//! relies on the defaults.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Run a child to completion, treating a non-zero status as an error.
pub(crate) fn run_checked(command: &mut Command, label: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("{label}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label}: exited with {status}"))
    }
}

/// Forward a child's exit code as the tool's own.
pub(crate) fn status_code(
    status: std::io::Result<std::process::ExitStatus>,
) -> Result<i32, String> {
    Ok(status
        .map_err(|error| format!("run child process: {error}"))?
        .code()
        .unwrap_or(1))
}

/// Append the host's executable suffix to a binary stem.
pub(crate) fn binary_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// Which helper a command needs. The variants differ in their default search
/// order, not in how an explicit flag is honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Helper {
    /// The engine a payload ships: optimized, no dev-tools surface.
    ReleaseEngine,
    /// The engine an authoring run launches: the SDK bundle's debug dev-tools
    /// build at the bundle root, or a workspace build beside the tool.
    AuthoringEngine,
    PrlBuild,
    ScriptsBuild,
    MintIdentity,
    /// The tool itself, as shipped into an SDK bundle.
    Tool,
}

impl Helper {
    pub(crate) fn flag(self) -> &'static str {
        match self {
            Self::ReleaseEngine => "--release-engine",
            Self::AuthoringEngine => "--engine",
            Self::PrlBuild => "--prl-build",
            Self::ScriptsBuild => "--scripts-build",
            Self::MintIdentity => "--mint-identity",
            Self::Tool => "--tool",
        }
    }

    /// Paths tried in order, relative to the directory holding the tool.
    fn candidates(self) -> &'static [&'static str] {
        match self {
            Self::ReleaseEngine => &["postretro-release", "postretro"],
            // A bundle's authoring engine sits at the bundle root, one level up
            // from `bin/`; a workspace build sits beside the tool.
            //
            // `postretro-release` is deliberately *not* a fallback here, unlike
            // the mirrored preference above. The authoring engine must be a
            // debug build with `--features dev-tools` — TS auto-compile and hot
            // reload are gated on debug assertions, and a release engine links
            // no TypeScript compiler at all — so substituting the release binary
            // would hand `sdk-dist` an engine that cannot serve the loop its own
            // README promises, and hand `run` one that silently stops reloading.
            // Nothing downstream can tell the two apart, so an absent authoring
            // engine is an error naming `--engine` instead.
            Self::AuthoringEngine => &["postretro", "../postretro"],
            Self::PrlBuild => &["prl-build"],
            Self::ScriptsBuild => &["scripts-build"],
            Self::MintIdentity => &["mint-identity"],
            Self::Tool => &["postretro-tool"],
        }
    }

    /// How this helper is produced, for the error a missing one raises.
    fn build_hint(self) -> &'static str {
        match self {
            Self::ReleaseEngine | Self::AuthoringEngine => {
                "cargo build --release -p postretro --bin postretro"
            }
            Self::PrlBuild => "cargo build --release -p postretro-level-compiler --bin prl-build",
            Self::ScriptsBuild => {
                "cargo build --release -p postretro-script-compiler --bin scripts-build"
            }
            Self::MintIdentity => "cargo build --release -p postretro-sim --bin mint-identity",
            Self::Tool => "cargo build --release -p postretro-tool --bin postretro-tool",
        }
    }
}

/// Explicit `--engine`/`--prl-build`/… overrides collected from the command line.
#[derive(Debug, Default, Clone)]
pub(crate) struct Overrides {
    release_engine: Option<PathBuf>,
    authoring_engine: Option<PathBuf>,
    prl_build: Option<PathBuf>,
    scripts_build: Option<PathBuf>,
    mint_identity: Option<PathBuf>,
    tool: Option<PathBuf>,
}

impl Overrides {
    fn slot(&mut self, helper: Helper) -> &mut Option<PathBuf> {
        match helper {
            Helper::ReleaseEngine => &mut self.release_engine,
            Helper::AuthoringEngine => &mut self.authoring_engine,
            Helper::PrlBuild => &mut self.prl_build,
            Helper::ScriptsBuild => &mut self.scripts_build,
            Helper::MintIdentity => &mut self.mint_identity,
            Helper::Tool => &mut self.tool,
        }
    }

    fn get(&self, helper: Helper) -> Option<&Path> {
        match helper {
            Helper::ReleaseEngine => self.release_engine.as_deref(),
            Helper::AuthoringEngine => self.authoring_engine.as_deref(),
            Helper::PrlBuild => self.prl_build.as_deref(),
            Helper::ScriptsBuild => self.scripts_build.as_deref(),
            Helper::MintIdentity => self.mint_identity.as_deref(),
            Helper::Tool => self.tool.as_deref(),
        }
    }

    /// Record a recognized override flag. Returns `false` when `flag` names no
    /// helper, leaving the caller's own flag handling to run.
    ///
    /// `value` is the token after the flag; its absence is this function's error
    /// to report, since it is the one that knows the flag takes a path.
    pub(crate) fn absorb(&mut self, flag: &str, value: Option<&OsString>) -> Result<bool, String> {
        let helper = match flag {
            "--release-engine" => Helper::ReleaseEngine,
            "--engine" => Helper::AuthoringEngine,
            "--prl-build" => Helper::PrlBuild,
            "--scripts-build" => Helper::ScriptsBuild,
            "--mint-identity" => Helper::MintIdentity,
            "--tool" => Helper::Tool,
            _ => return Ok(false),
        };
        let value = value.ok_or_else(|| format!("{flag} requires a path to the binary"))?;
        if self.slot(helper).replace(PathBuf::from(value)).is_some() {
            return Err(format!("{flag} may be given only once"));
        }
        Ok(true)
    }

    /// Resolve one helper: the explicit flag if given, else the first candidate
    /// beside the tool's own executable that exists.
    pub(crate) fn resolve(&self, helper: Helper) -> Result<PathBuf, String> {
        if let Some(explicit) = self.get(helper) {
            if !explicit.is_file() {
                return Err(format!(
                    "{} names {}, which is not a file",
                    helper.flag(),
                    explicit.display()
                ));
            }
            return Ok(explicit.to_path_buf());
        }

        let directory = tool_directory()?;
        let candidates: Vec<PathBuf> = helper
            .candidates()
            .iter()
            .map(|candidate| resolve_candidate(&directory, candidate))
            .collect();
        candidates
            .iter()
            .find(|candidate| candidate.is_file())
            .cloned()
            .ok_or_else(|| missing_helper_error(helper, &candidates))
    }
}

fn resolve_candidate(directory: &Path, candidate: &str) -> PathBuf {
    match candidate.strip_prefix("../") {
        Some(stem) => directory
            .parent()
            .unwrap_or(directory)
            .join(binary_name(stem)),
        None => directory.join(binary_name(candidate)),
    }
}

fn missing_helper_error(helper: Helper, candidates: &[PathBuf]) -> String {
    let searched = candidates
        .iter()
        .map(|candidate| candidate.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "no {} binary found (looked for {searched}).\n\n\
         Pass {} <path>, or build it with `{}`.",
        helper_label(helper),
        helper.flag(),
        helper.build_hint()
    )
}

fn helper_label(helper: Helper) -> &'static str {
    match helper {
        Helper::ReleaseEngine => "release engine",
        Helper::AuthoringEngine => "authoring engine",
        Helper::PrlBuild => "prl-build",
        Helper::ScriptsBuild => "scripts-build",
        Helper::MintIdentity => "mint-identity",
        Helper::Tool => "postretro-tool",
    }
}

/// The directory holding this executable — the only anchor a shipped binary has
/// for its siblings, and for the engine-owned trees beside them
/// (`engine_trees.rs`).
pub(crate) fn tool_directory() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("locate this executable to find its sibling binaries: {error}"))?;
    executable.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "executable {} has no parent directory",
            executable.display()
        )
    })
}

/// The usage fragment every command that drives helpers shares.
pub(crate) const HELPER_USAGE: &str = "  [--engine <path>] [--release-engine <path>] \
[--prl-build <path>] [--scripts-build <path>]\n  [--mint-identity <path>] [--tool <path>]";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absorb_records_each_helper_flag_and_ignores_unrelated_ones() {
        let mut overrides = Overrides::default();
        let value = OsString::from("/build/prl-build");

        assert_eq!(
            overrides.absorb("--prl-build", Some(&value)),
            Ok(true),
            "a helper flag is consumed here"
        );
        assert_eq!(
            overrides.get(Helper::PrlBuild),
            Some(Path::new("/build/prl-build"))
        );
        assert_eq!(
            overrides.absorb("--out", Some(&value)),
            Ok(false),
            "an unrelated flag is left to the command's own parser"
        );
    }

    #[test]
    fn absorb_rejects_a_missing_value_and_a_repeated_flag() {
        let mut overrides = Overrides::default();
        assert!(overrides.absorb("--engine", None).is_err());

        let value = OsString::from("/build/postretro");
        assert_eq!(overrides.absorb("--engine", Some(&value)), Ok(true));
        assert!(overrides.absorb("--engine", Some(&value)).is_err());
    }

    #[test]
    fn candidate_paths_reach_a_bundle_root_engine_from_the_bin_directory() {
        let bin = Path::new("/bundle/bin");
        let candidates: Vec<PathBuf> = Helper::AuthoringEngine
            .candidates()
            .iter()
            .map(|candidate| resolve_candidate(bin, candidate))
            .collect();

        assert_eq!(
            candidates,
            [
                PathBuf::from("/bundle/bin").join(binary_name("postretro")),
                PathBuf::from("/bundle").join(binary_name("postretro")),
            ]
        );
    }

    /// The mirror of `release_engine_prefers_the_bundled_release_name`, and the
    /// direction that used to be unguarded. An authoring engine is a debug
    /// `--features dev-tools` build; the release binary links no TypeScript
    /// compiler, so a bundle assembled from it would promise a hot-reload loop
    /// it cannot serve, with nothing downstream able to tell.
    #[test]
    fn the_authoring_engine_never_falls_back_to_a_release_build() {
        assert!(
            !Helper::AuthoringEngine
                .candidates()
                .contains(&"postretro-release"),
            "a release engine cannot serve the authoring loop"
        );
    }

    /// A payload must carry an optimized engine. The release search order puts
    /// `postretro-release` — the name an SDK bundle ships it under — first, so a
    /// bundle's debug authoring engine never becomes a player payload's engine
    /// while a release build sits beside it.
    #[test]
    fn release_engine_prefers_the_bundled_release_name() {
        assert_eq!(
            Helper::ReleaseEngine.candidates(),
            &["postretro-release", "postretro"]
        );
    }

    #[test]
    fn missing_helper_error_names_the_flag_the_search_and_the_build_command() {
        let error =
            missing_helper_error(Helper::PrlBuild, &[PathBuf::from("/bundle/bin/prl-build")]);
        assert!(error.contains("--prl-build"), "{error}");
        assert!(error.contains("/bundle/bin/prl-build"), "{error}");
        assert!(error.contains("postretro-level-compiler"), "{error}");
    }
}
