// Per-user config and data directories, resolved once from the app name at boot stage 1.
// See: context/lib/build_pipeline.md §Distribution packaging (§Player-data directory)

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::options::SETTINGS_FILENAME;

/// Names the per-user directories. Whoever knows the project — a payload
/// launcher, `postretro-tool run`, an SDK bundle's launcher — passes its
/// package name here, as it passes `--mod`; the engine never reads the manifest.
pub(crate) const APP_NAME_FLAG: &str = "--app-name";

/// A bare engine launch, `cargo run -p xtask -- run` included, keeps the
/// directory every build before per-game names used.
const DEFAULT_APP_NAME: &str = "postretro";

/// The one place the platform's per-user directories are resolved.
///
/// Settings and saved state resolve together because `state.json`'s per-player
/// rows are keyed by the claim id derived from `settings.toml`'s `player_id`
/// (`player_options.md` §2): two resolutions that disagreed would orphan every
/// per-player save. Each inner `Option` is genuine runtime absence — a platform
/// with no such directory runs without that persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppDirs {
    config_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
}

impl AppDirs {
    /// Resolve the directories for a validated app name, or for
    /// [`DEFAULT_APP_NAME`] when none was given. Each platform normalizes the
    /// name its own way through `ProjectDirs`, as it did for `postretro`, so
    /// existing dev data stays where it was.
    pub(crate) fn resolve(app_name: Option<&str>) -> Self {
        let dirs = ProjectDirs::from("", "", app_name.unwrap_or(DEFAULT_APP_NAME));
        Self {
            config_dir: dirs.as_ref().map(|dirs| dirs.config_dir().to_path_buf()),
            data_dir: dirs.as_ref().map(|dirs| dirs.data_dir().to_path_buf()),
        }
    }

    /// Directories chosen by a test rather than the platform.
    #[cfg(test)]
    pub(crate) fn at(config_dir: &Path, data_dir: &Path) -> Self {
        Self {
            config_dir: Some(config_dir.to_path_buf()),
            data_dir: Some(data_dir.to_path_buf()),
        }
    }

    /// `<config dir>/settings.toml`: the one path settings load from and save to.
    pub(crate) fn settings_path(&self) -> Option<PathBuf> {
        self.config_dir
            .as_ref()
            .map(|config_dir| config_dir.join(SETTINGS_FILENAME))
    }

    /// The directory each mod's `state.json` lives under.
    pub(crate) fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }

    /// One line naming where this run keeps player data, so a launch under the
    /// wrong name is attributable from the log.
    pub(crate) fn describe(&self) -> String {
        let show = |dir: &Option<PathBuf>| {
            dir.as_ref().map_or_else(
                || "unavailable".to_string(),
                |dir| dir.display().to_string(),
            )
        };
        format!(
            "config {}, data {}",
            show(&self.config_dir),
            show(&self.data_dir)
        )
    }
}

/// Refuse an app name that cannot be one directory name, or that a launcher
/// would pass as something other than the flag's value.
///
/// The same values `--mod` refuses, plus a whitespace-only one, which some
/// platforms would normalize to an empty directory name. `postretro-tool`
/// mirrors this for package names; `app_name_cases.toml` holds both to one
/// verdict per case.
pub(crate) fn validate_app_name(name: &str) -> Result<(), String> {
    let is_plain_name = !name.trim().is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && !name.contains(['/', '\\', ':']);
    if is_plain_name {
        Ok(())
    } else {
        Err(format!(
            "{APP_NAME_FLAG} takes a name for this game's settings and save directory — not a \
             path, not blank, and not starting with `-`; got `{name}`"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct AppNameCases {
        accepted: Vec<String>,
        rejected: Vec<String>,
    }

    /// The table `postretro-tool`'s package-name check asserts too.
    fn app_name_cases() -> AppNameCases {
        toml::from_str(include_str!("app_name_cases.toml")).expect("the shared case table parses")
    }

    #[test]
    fn app_name_resolves_both_directories_through_project_dirs() {
        let dirs = AppDirs::resolve(Some("my-game"));
        let expected =
            ProjectDirs::from("", "", "my-game").expect("this host has a home directory");
        assert_eq!(
            dirs.settings_path(),
            Some(expected.config_dir().join(SETTINGS_FILENAME))
        );
        assert_eq!(dirs.data_dir(), Some(expected.data_dir()));
    }

    #[test]
    fn absent_app_name_resolves_both_directories_under_postretro() {
        let dirs = AppDirs::resolve(None);
        let expected =
            ProjectDirs::from("", "", "postretro").expect("this host has a home directory");
        assert_eq!(
            dirs.settings_path(),
            Some(expected.config_dir().join(SETTINGS_FILENAME))
        );
        assert_eq!(dirs.data_dir(), Some(expected.data_dir()));
    }

    /// A game's first launch must not read or write the bare-launch directory:
    /// every path it resolves sits outside `postretro`'s.
    #[test]
    fn a_game_app_name_shares_no_directory_with_postretro() {
        let game = AppDirs::resolve(Some("my-game"));
        let bare = AppDirs::resolve(None);
        let game_paths = [game.config_dir.clone(), game.data_dir.clone()];
        let bare_paths = [bare.config_dir.clone(), bare.data_dir.clone()];
        for game_path in game_paths.iter().flatten() {
            for bare_path in bare_paths.iter().flatten() {
                assert!(
                    !game_path.starts_with(bare_path) && !bare_path.starts_with(game_path),
                    "{} overlaps {}",
                    game_path.display(),
                    bare_path.display()
                );
            }
        }
    }

    #[test]
    fn app_name_check_matches_the_shared_case_table() {
        let cases = app_name_cases();
        for name in &cases.accepted {
            assert_eq!(
                validate_app_name(name),
                Ok(()),
                "`{name}` should be accepted"
            );
        }
        for name in &cases.rejected {
            let error = validate_app_name(name).expect_err(&format!("`{name}` should be refused"));
            assert!(error.contains(APP_NAME_FLAG), "{error}");
        }
    }

    /// Source of every `.rs` file under `dir`, recursively, by path.
    fn rust_sources(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in std::fs::read_dir(dir).expect("source directory reads") {
            let path = entry.expect("directory entry reads").path();
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = std::fs::read_to_string(&path).expect("source is UTF-8");
                out.push((path, source));
            }
        }
    }

    /// Code lines of a file's non-test part: comment lines dropped, and the
    /// file cut at its `#[cfg(test)] mod …` block. A whole test file yields none.
    fn non_test_code_lines(path: &Path, source: &str) -> Vec<(usize, String)> {
        let is_test_file = path
            .components()
            .any(|component| component.as_os_str() == "tests")
            || path
                .file_stem()
                .is_some_and(|stem| stem.to_string_lossy().ends_with("tests"));
        if is_test_file {
            return Vec::new();
        }
        let lines: Vec<&str> = source.lines().collect();
        let test_module_start = lines
            .iter()
            .enumerate()
            .position(|(index, line)| {
                line.trim() == "#[cfg(test)]"
                    && lines[index + 1..]
                        .iter()
                        .find(|next| !next.trim().is_empty())
                        .is_some_and(|next| next.trim_start().starts_with("mod "))
            })
            .unwrap_or(lines.len());
        lines[..test_module_start]
            .iter()
            .enumerate()
            .filter(|(_, line)| !line.trim_start().starts_with("//"))
            .map(|(index, line)| (index + 1, line.to_string()))
            .collect()
    }

    fn crates_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the engine crate sits under crates/")
            .to_path_buf()
    }

    fn is_chokepoint(path: &Path) -> bool {
        path.ends_with("postretro/src/startup/app_dirs.rs")
    }

    /// Grep gate: the platform's per-user directories resolve in this file
    /// alone, so settings and saved state can never disagree on the app name.
    #[test]
    fn no_crate_resolves_project_dirs_outside_the_chokepoint() {
        let mut sources = Vec::new();
        rust_sources(&crates_dir(), &mut sources);
        let offenders: Vec<String> = sources
            .iter()
            .filter(|(path, _)| !is_chokepoint(path))
            .flat_map(|(path, source)| {
                source
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| !line.trim_start().starts_with("//"))
                    .filter(|(_, line)| line.contains("ProjectDirs::from"))
                    .map(move |(index, _)| format!("{}:{}", path.display(), index + 1))
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "ProjectDirs::from outside app_dirs.rs: {offenders:?}"
        );
    }

    /// Grep gate: the bare-launch default names a directory only here. The
    /// engine and sim crates — every settings and `state.json` consumer — take
    /// the directories resolved once at stage 1 instead of naming one.
    #[test]
    fn the_default_app_name_literal_appears_only_in_the_chokepoint() {
        let crates = crates_dir();
        let mut sources = Vec::new();
        rust_sources(&crates.join("postretro/src"), &mut sources);
        rust_sources(&crates.join("sim/src"), &mut sources);
        let literal = format!("\"{DEFAULT_APP_NAME}\"");
        let offenders: Vec<String> = sources
            .iter()
            .filter(|(path, _)| !is_chokepoint(path))
            .flat_map(|(path, source)| {
                non_test_code_lines(path, source)
                    .into_iter()
                    .filter(|(_, line)| line.contains(&literal))
                    .map(move |(number, _)| format!("{}:{number}", path.display()))
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "default app name outside app_dirs.rs: {offenders:?}"
        );
    }
}
