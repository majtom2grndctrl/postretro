// The `prl-build` command the integration suites bake through, with the
// lightmap reach cull's verify mode on as in every test bake, and the
// directories those bakes write into.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

#![allow(dead_code)] // Each suite uses a different subset.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

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
/// `<test target tmpdir>/<test binary>/<process id>/<name>`. A fixture a
/// `static` holds is never dropped, so a `TempDir` there would leak a full
/// bake per test run; one path per (process, fixture) keeps one copy. Each
/// process holds an advisory lock on `<process id>.lock` beside its
/// directory for its life, so two runs of one test binary at once each get
/// their own, and the first call reaps every sibling directory whose lock no
/// live process holds: leftovers stay bounded to the runs alive at once.
pub fn fixture_dir(name: &str) -> PathBuf {
    let dir = process_dir().join(name);
    clear_dir(&dir);
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|error| panic!("create fixture directory {}: {error}", dir.display()));
    dir
}

/// This process's fixture directory, locked and with stale siblings reaped
/// on first use.
fn process_dir() -> &'static Path {
    static DIR: OnceLock<(PathBuf, File)> = OnceLock::new();
    &DIR.get_or_init(|| {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(env!("CARGO_CRATE_NAME"));
        std::fs::create_dir_all(&root)
            .unwrap_or_else(|error| panic!("create fixture root {}: {error}", root.display()));
        let id = std::process::id().to_string();
        let lock_path = root.join(format!("{id}.lock"));
        let lock = File::create(&lock_path)
            .unwrap_or_else(|error| panic!("create {}: {error}", lock_path.display()));
        lock.lock()
            .unwrap_or_else(|error| panic!("lock {}: {error}", lock_path.display()));
        // The directory is made only under the lock, so a reaper that took
        // the lock first found nothing of this process's to remove.
        let dir = root.join(&id);
        clear_dir(&dir);
        reap_stale(&root, &id);
        (dir, lock)
    })
    .0
}

/// How long a dead process's lock file stays before it is removed: long past
/// the instant between a live process creating it and locking it.
const STALE_LOCK_AGE: Duration = Duration::from_secs(60);

/// Remove every directory under `root` but `own` that no live process holds
/// the lock for: a dead process's, or one left by an older layout without
/// locks. A dead process's lock file goes too, once old.
fn reap_stale(root: &Path, own: &str) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path.is_dir() {
            let live = name == own
                || (name.parse::<u32>().is_ok() && lock_held(&root.join(format!("{name}.lock"))));
            if !live {
                // Best effort: another reaper may be removing it too.
                let _ = std::fs::remove_dir_all(&path);
            }
            continue;
        }
        if let Some(id) = name.strip_suffix(".lock")
            && id != own
            && !lock_held(&path)
            && entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > STALE_LOCK_AGE))
        {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether a live process holds the lock at `path`; a missing file is held
/// by nobody.
fn lock_held(path: &Path) -> bool {
    let Ok(file) = OpenOptions::new().write(true).open(path) else {
        return false;
    };
    // A lock that cannot be read is treated as held: nothing is removed.
    !matches!(file.try_lock(), Ok(()))
}

fn clear_dir(dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("clear fixture directory {}: {error}", dir.display()),
    }
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

/// Copy the directory tree `from` into `to`, as a warm build's cache copy.
pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create cache copy");
    for entry in std::fs::read_dir(from).expect("list cache") {
        let entry = entry.expect("cache entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("cache entry type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy cache entry");
        }
    }
}
