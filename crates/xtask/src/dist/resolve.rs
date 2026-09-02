//! Distribution-script and map-set resolution helpers.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::manifest::{Manifest, Recipe};

/// The script extension that the release runtime must receive at the mod root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryExt {
    Js,
    Luau,
}

impl EntryExt {
    pub(crate) fn file_name(self) -> &'static str {
        match self {
            Self::Js => "start-script.js",
            Self::Luau => "start-script.luau",
        }
    }
}

/// A map bake resolved from an emitted `maps/<name>.prl` literal.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Resolved {
    /// The mod-root-relative output path, retaining its `maps/` prefix.
    pub(crate) output: String,
    /// The workspace source passed to `prl-build`.
    pub(crate) source: PathBuf,
    /// Recipe flags other than dist-owned `--release`, `--no-tui`, and `-o`.
    pub(crate) args: Vec<String>,
    /// The effective bake density used to establish deterministic bake order.
    pub(crate) lightmap_density: f32,
}

/// The level compiler's CLI default. Its defining module belongs to the
/// compiler binary target, so xtask deliberately keeps this small CLI mirror.
pub(crate) const DEFAULT_LIGHTMAP_DENSITY_METERS: f32 = 0.04;

/// The two mutually-exclusive script inputs did not identify a script to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryScriptChoiceError {
    BothPresent,
    NeitherPresent,
}

impl fmt::Display for EntryScriptChoiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BothPresent => {
                formatter.write_str("both start-script.ts and start-script.luau are present")
            }
            Self::NeitherPresent => {
                formatter.write_str("neither start-script.ts nor start-script.luau is present")
            }
        }
    }
}

/// Select the one release entry script dist is allowed to emit.
pub(crate) fn entry_script_choice(
    ts_present: bool,
    luau_present: bool,
) -> Result<EntryExt, EntryScriptChoiceError> {
    match (ts_present, luau_present) {
        (true, false) => Ok(EntryExt::Js),
        (false, true) => Ok(EntryExt::Luau),
        (true, true) => Err(EntryScriptChoiceError::BothPresent),
        (false, false) => Err(EntryScriptChoiceError::NeitherPresent),
    }
}

/// Returns whether `name` is exactly a completed material-mips cache filename.
pub(crate) fn is_prm_filename(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".prm") else {
        return false;
    };
    stem.len() == 64
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Sort bakes by their effective density, then by their output path.
pub(crate) fn bake_order(resolved: &[Resolved]) -> Vec<Resolved> {
    let mut ordered = resolved.to_vec();
    ordered.sort_by(|left, right| {
        left.lightmap_density
            .total_cmp(&right.lightmap_density)
            .then_with(|| left.output.cmp(&right.output))
    });
    ordered
}

/// Return the mod-root-relative paths still outstanding after `completed` bakes.
pub(crate) fn outstanding_outputs(ordered: &[Resolved], completed: usize) -> Vec<String> {
    ordered
        .iter()
        .skip(completed)
        .map(|resolved| resolved.output.clone())
        .collect()
}

/// Scan an emitted entry script for `maps/<name>.prl` literals.
pub(crate) fn scan_map_literals(script: &[u8]) -> BTreeSet<String> {
    const PREFIX: &[u8] = b"maps/";
    const SUFFIX: &[u8] = b".prl";

    let mut found = BTreeSet::new();
    let mut start = 0;
    while let Some(relative) = script[start..]
        .windows(PREFIX.len())
        .position(|window| window == PREFIX)
    {
        let literal_start = start + relative;
        let mut end = literal_start + PREFIX.len();
        while end < script.len() && is_map_literal_byte(script[end]) {
            end += 1;
        }

        let body = &script[literal_start + PREFIX.len()..end];
        if !body.is_empty() {
            for suffix_start in 1..body.len() {
                if body[suffix_start..].starts_with(SUFFIX) {
                    let literal_end = literal_start + PREFIX.len() + suffix_start + SUFFIX.len();
                    // Both the prefix and the accepted body alphabet are ASCII.
                    let literal = std::str::from_utf8(&script[literal_start..literal_end])
                        .expect("map literal scanner accepts only ASCII bytes");
                    found.insert(literal.to_string());
                    break;
                }
            }
        }

        start = literal_start + PREFIX.len();
    }
    found
}

fn is_map_literal_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
}

/// Resolve every scanned literal and report all manifest/source mistakes together.
pub(crate) fn resolve_map_set(
    scanned: &BTreeSet<String>,
    manifest: &Manifest,
    workspace: &Path,
) -> Result<Vec<Resolved>, String> {
    let recipes: BTreeMap<&str, &Recipe> = manifest
        .recipes
        .iter()
        .map(|recipe| (recipe.output.as_str(), recipe))
        .collect();
    let mod_root = workspace.join(&manifest.package.mod_root);
    let mut problems = Vec::new();

    if scanned.is_empty() {
        problems.push("emitted entry script contains no maps/*.prl literals".to_string());
    }

    for recipe in &manifest.recipes {
        if !scanned.contains(&recipe.output) {
            problems.push(format!(
                "recipe `{}` is orphaned: no emitted maps literal matches it",
                recipe.output
            ));
        }
    }

    let mut resolved = Vec::with_capacity(scanned.len());
    for output in scanned {
        let recipe = recipes.get(output.as_str()).copied();
        let source = recipe
            .and_then(|recipe| recipe.source.as_deref())
            .map(|source| workspace.join(source))
            .unwrap_or_else(|| default_map_source(&mod_root, output));

        if !source.is_file() {
            let label = recipe
                .map(|recipe| format!("recipe `{}`", recipe.output))
                .unwrap_or_else(|| format!("default source for `{output}`"));
            problems.push(format!("{label}: missing map source {}", source.display()));
            continue;
        }

        resolved.push(Resolved {
            output: output.clone(),
            source,
            args: recipe.map_or_else(Vec::new, |recipe| recipe.args.clone()),
            lightmap_density: recipe
                .and_then(|recipe| recipe.lightmap_density)
                .unwrap_or(DEFAULT_LIGHTMAP_DENSITY_METERS),
        });
    }

    if problems.is_empty() {
        Ok(resolved)
    } else {
        Err(format!("resolve shipped map set: {}", problems.join("; ")))
    }
}

fn default_map_source(mod_root: &Path, output: &str) -> PathBuf {
    let stem = output
        .strip_prefix("maps/")
        .and_then(|path| path.strip_suffix(".prl"))
        .expect("manifest outputs and scanner literals retain maps/*.prl shape");
    mod_root.join("maps").join(format!("{stem}.map"))
}

/// Why a payload root cannot be safely removed by a distribution run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuardRefusal {
    NotUnderDist(PathBuf),
    NotADirectory(PathBuf),
    NoProvenance(PathBuf),
}

impl fmt::Display for GuardRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUnderDist(path) => write!(
                formatter,
                "payload root {} violates the containment rule: it must lie strictly under workspace dist/",
                path.display()
            ),
            Self::NotADirectory(path) => write!(
                formatter,
                "payload root {} is not a directory in its own right",
                path.display()
            ),
            Self::NoProvenance(path) => write!(
                formatter,
                "payload root {} has no dist provenance marker or engine binary",
                path.display()
            ),
        }
    }
}

/// Prove that `payload_root` is a removable root strictly under `workspace/dist`.
pub(crate) fn guard_payload_root(
    payload_root: &Path,
    workspace: &Path,
) -> Result<(), GuardRefusal> {
    let dist_root = workspace.join("dist");
    if !is_strictly_under(payload_root, &dist_root).unwrap_or(false) {
        return Err(GuardRefusal::NotUnderDist(payload_root.to_path_buf()));
    }

    match std::fs::symlink_metadata(payload_root) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(GuardRefusal::NotADirectory(payload_root.to_path_buf()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(GuardRefusal::NotADirectory(payload_root.to_path_buf())),
    }

    let entries = std::fs::read_dir(payload_root)
        .map_err(|_| GuardRefusal::NoProvenance(payload_root.to_path_buf()))?;
    let is_empty = entries
        .into_iter()
        .next()
        .transpose()
        .map_err(|_| GuardRefusal::NoProvenance(payload_root.to_path_buf()))?
        .is_none();
    if is_empty {
        return Ok(());
    }

    let engine_binary = if cfg!(windows) {
        "postretro.exe"
    } else {
        "postretro"
    };
    if payload_root.join(".dist-incomplete").exists() || payload_root.join(engine_binary).exists() {
        Ok(())
    } else {
        Err(GuardRefusal::NoProvenance(payload_root.to_path_buf()))
    }
}

/// Compare a candidate and ancestor after canonicalizing each nearest existing
/// ancestor and then applying the missing tail to that canonical location.
pub(crate) fn is_at_or_under(path: &Path, ancestor: &Path) -> io::Result<bool> {
    let path = canonicalize_nearest_existing(path)?;
    let ancestor = canonicalize_nearest_existing(ancestor)?;
    Ok(path.starts_with(&ancestor))
}

fn is_strictly_under(path: &Path, ancestor: &Path) -> io::Result<bool> {
    let path = canonicalize_nearest_existing(path)?;
    let ancestor = canonicalize_nearest_existing(ancestor)?;
    Ok(path != ancestor && path.starts_with(&ancestor))
}

fn canonicalize_nearest_existing(path: &Path) -> io::Result<PathBuf> {
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
                let canonical = std::fs::canonicalize(candidate)?;
                return Ok(rejoin_tail(canonical, missing_tail));
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

fn rejoin_tail(mut canonical: PathBuf, mut tail: Vec<std::ffi::OsString>) -> PathBuf {
    tail.reverse();
    for component in tail {
        match Path::new(&component).components().next() {
            Some(Component::ParentDir) => {
                canonical.pop();
            }
            Some(Component::CurDir) | None => {}
            Some(Component::Normal(component)) => canonical.push(component),
            Some(Component::RootDir | Component::Prefix(_)) => {
                // A filename collected from an existing absolute path cannot be rooted.
                unreachable!("missing path tail contains only filename components");
            }
        }
    }
    canonical
}
