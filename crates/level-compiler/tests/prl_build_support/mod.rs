// The `prl-build` command the integration suites bake through, with the
// lightmap reach cull's verify mode on as in every test bake, and the
// directories those bakes write into.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

#![allow(dead_code)] // Each suite uses a different subset.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `1` turns the reach cull's verify mode on, `0` off. Verify mode reads
/// neither the lightmap layer cache nor the `lightmap_section` memo, so a
/// bake whose test asserts their hits sets it to `0`.
pub const REACH_VERIFY_ENV: &str = "POSTRETRO_LIGHTMAP_REACH_VERIFY";

/// `prl-build` with reach verify mode on, whatever the caller's shell sets.
pub fn prl_build() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prl-build"));
    command.env(REACH_VERIFY_ENV, "1");
    command
}

/// A fixture's own directory, emptied and recreated on each call:
/// `<test target tmpdir>/<test binary>/<name>`. A fixture a `static` holds is
/// never dropped, so a `TempDir` there would leak a full bake per test run;
/// one fixed path per (test binary, fixture) keeps one copy, which the next
/// run replaces. Two runs of one test binary at once share it.
pub fn fixture_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(env!("CARGO_CRATE_NAME"))
        .join(name);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("clear fixture directory {}: {error}", dir.display()),
    }
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|error| panic!("create fixture directory {}: {error}", dir.display()));
    dir
}

/// Where one build writes: a fresh temporary directory, removed with its
/// owner, or a [`fixture_dir`] for a build a `static` holds.
pub enum BuildDir {
    Temp(tempfile::TempDir),
    Fixture(PathBuf),
}

impl BuildDir {
    pub fn temp() -> Self {
        Self::Temp(tempfile::tempdir().expect("create build directory"))
    }

    pub fn fixture(name: &str) -> Self {
        Self::Fixture(fixture_dir(name))
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Temp(dir) => dir.path(),
            Self::Fixture(dir) => dir,
        }
    }
}
