// Background level-load worker: PRL parse. Texture decode + GPU upload now
// run on the main thread from baked `.prm` sidecars (which only the renderer
// can address). The worker emits the compiled-material root path so the main
// thread can locate the sidecars without re-deriving the layout.
// See: context/lib/boot_sequence.md · context/lib/build_pipeline.md §PRL section IDs · §Baked texture mips

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use postretro_level_loader::LevelWorld;

/// Delivered to the main thread after the worker completes. All fields are plain
/// `Send` — no GPU handles.
pub(crate) struct LevelPayload {
    pub level: Option<LevelWorld>,
    /// Compiled-material output directory holding the per-texture `.prm` mip
    /// sidecars (`<workspace>/baked/materials/<hex(blake3)>.prm`). Runtime-
    /// required compiled output, not a disposable cache. Always populated;
    /// absent or unusable directories surface per-texture warnings from
    /// `load_textures` and degrade those entries to placeholders.
    pub prm_cache_root: PathBuf,
    /// Spliced into level-load `StartupTimings` between `worker_dispatch` and `worker_delivered`.
    pub timings: Vec<(&'static str, Duration)>,
}

// Compile-time guard: catches non-Send fields (Rc, Cell, etc.) before thread::spawn.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<LevelPayload>();
};

pub(crate) type LoadOutcome = Result<LevelPayload, anyhow::Error>;

/// Send errors are ignored — dropped receiver means the window closed during load.
pub(crate) fn spawn_level_worker(
    map_path: PathBuf,
    content_root: PathBuf,
    baked_root: Option<PathBuf>,
    sender: mpsc::Sender<LoadOutcome>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let outcome = run_worker(&map_path, &content_root, baked_root.as_deref());
        // Receiver dropped = window closed during load; nothing to do.
        let _ = sender.send(outcome);
    })
}

fn run_worker(map_path: &Path, content_root: &Path, baked_root: Option<&Path>) -> LoadOutcome {
    let mut timings: Vec<(&'static str, Duration)> = Vec::with_capacity(2);
    let cursor = Instant::now();

    let prm_cache_root = resolve_prm_root(content_root, baked_root);

    let path_str = map_path.to_string_lossy().into_owned();
    let level = match postretro_level_loader::load_prl(&path_str) {
        Ok(world) => {
            log::info!("[Loader] PRL loaded successfully from {path_str}");
            Some(world)
        }
        Err(postretro_level_loader::PrlLoadError::FileNotFound(p)) => {
            log::warn!("[Loader] PRL file not found: {p} — starting without map");
            let now = Instant::now();
            timings.push(("prl_parse", now.duration_since(cursor)));
            return Ok(LevelPayload {
                level: None,
                prm_cache_root,
                timings,
            });
        }
        Err(err) => {
            return Err(anyhow::anyhow!("failed to load PRL: {err}"));
        }
    };
    {
        let now = Instant::now();
        timings.push(("prl_parse", now.duration_since(cursor)));
    }

    Ok(LevelPayload {
        level,
        prm_cache_root,
        timings,
    })
}

/// Compiled-material output root: `<workspace>/baked/materials/`. `.prm` mip
/// sidecars are runtime-*required* compiled output (not a disposable cache), so
/// they live in the top-level `baked/` tree, sibling to `.build-caches/`. The
/// dev layout points `content_root` at `content/<mod>/`, so two parents up lands
/// on the workspace root. Unusual layouts that don't have two ancestors fall
/// back to the content root itself; `load_textures` then surfaces per-texture
/// warnings when the directory turns out not to hold any `.prm` files.
///
/// The fallback to `content_root` keeps the engine runnable in
/// shipping/standalone layouts where the two-parent dev-layout assumption
/// doesn't hold; unusual layouts surface as per-texture placeholder warnings
/// rather than a startup panic. Shipping layouts are out of scope for now, so
/// the fallback is deliberately conservative.
///
/// Note: the level compiler's `resolve_prm_root_via_cargo` locates the
/// workspace via a `Cargo.toml` ancestor walk, while this function uses a
/// fixed two-parent walk. They MUST coincide in the dev layout — both resolve to
/// `<workspace>/baked/materials` — or every world texture silently degrades to a
/// placeholder. The divergence in *how* they walk is intentional: collapsing
/// them to share an implementation would break shipping layouts where no
/// `Cargo.toml` exists at all.
///
/// This is the derivation used when `--baked-root` is absent; see
/// [`resolve_prm_root`] for the override that lets content live outside the
/// layout both walks depend on.
pub(crate) fn derive_prm_root_dev_layout(content_root: &Path) -> PathBuf {
    let workspace = content_root
        .parent()
        .and_then(|c| c.parent())
        .unwrap_or(content_root);
    workspace.join("baked").join("materials")
}

/// Resolve the `.prm` root, honouring `--baked-root`.
///
/// `baked_root` names the directory that *contains* `materials/` — the same
/// thing `<workspace>/baked` is in the dev layout — so `--baked-root /p/baked`
/// reads `/p/baked/materials/<hex>.prm`. `prl-build` takes the same flag with
/// the same meaning and writes into that identical directory; the two flags are
/// one contract, and reading the value as `materials/` itself on either side
/// puts reader and writer in different directories, which is the silent
/// every-texture-is-a-placeholder failure the flag exists to prevent.
///
/// `None` keeps [`derive_prm_root_dev_layout`] untouched, so the dev run, the
/// tests, and every packaged layout resolve exactly what they resolved before
/// the flag existed. The override deliberately does not consult the content
/// root: outside the two-component mod-root shape the grandparent walk has
/// nothing to derive from, which is the whole reason a caller passes the flag.
pub(crate) fn resolve_prm_root(content_root: &Path, baked_root: Option<&Path>) -> PathBuf {
    match baked_root {
        Some(root) => root.join("materials"),
        None => derive_prm_root_dev_layout(content_root),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prm_root_dev_layout_resolves_to_baked_materials() {
        // Dev layout: content_root is `<workspace>/content/<mod>`; two parents
        // up is the workspace, then `baked/materials`. This is the exact dir the
        // compiler's `resolve_prm_root_via_cargo` writes to (cross-checked by the
        // `prm_root_writer_and_reader_agree_in_dev_layout` test in the compiler's
        // `main.rs`). Keep both pointing at `baked/materials` together.
        let content_root = Path::new("/ws/content/example");
        assert_eq!(
            derive_prm_root_dev_layout(content_root),
            Path::new("/ws/baked/materials"),
        );
    }

    /// The engine's `--baked-root` and `prl-build`'s `--baked-root` are one
    /// contract: one directory value, and both binaries must land on the same
    /// `materials/`. The compiler side is reproduced inline — it lives in
    /// another crate's binary target — and its matching assertion is
    /// `baked_root_agrees_with_the_runtime_reader` in
    /// `crates/level-compiler/src/main.rs`. If these two ever disagree, every
    /// world material degrades to a placeholder with only a warning.
    #[test]
    fn prm_root_override_agrees_with_the_compiler_writer() {
        let baked_root = Path::new("/project/baked");
        // A content repository: the mod root is not two components under a tree
        // root, so the grandparent walk has nothing correct to derive.
        let content_root = Path::new("/project/levels");

        let reader_root = resolve_prm_root(content_root, Some(baked_root));
        // Writer side: `prl-build` joins `materials` onto the same flag value.
        let writer_root = baked_root.join("materials");

        assert_eq!(reader_root, writer_root);
        assert_eq!(reader_root, Path::new("/project/baked/materials"));
    }

    /// The flag value is the parent of `materials/`, so a sidecar is read from
    /// `<flag>/materials/<hex>.prm` — never `<flag>/<hex>.prm`.
    #[test]
    fn prm_root_override_treats_the_value_as_the_parent_of_materials() {
        let resolved = resolve_prm_root(Path::new("/project/levels"), Some(Path::new("/p/baked")));
        assert_eq!(resolved.file_name().unwrap(), "materials");
        assert_eq!(resolved.parent().unwrap(), Path::new("/p/baked"));
        assert_eq!(
            resolved.join("deadbeef.prm"),
            Path::new("/p/baked/materials/deadbeef.prm"),
        );
    }

    /// Flag absent means the pre-flag grandparent walk, unchanged.
    #[test]
    fn prm_root_without_an_override_keeps_the_dev_layout_walk() {
        for content_root in [
            Path::new("/ws/content/dev"),
            Path::new("/ws/content/example"),
            // Shallow layout: the conservative content-root fallback.
            Path::new("standalone"),
        ] {
            assert_eq!(
                resolve_prm_root(content_root, None),
                derive_prm_root_dev_layout(content_root),
            );
        }
        assert_eq!(
            resolve_prm_root(Path::new("/ws/content/dev"), None),
            Path::new("/ws/baked/materials"),
        );
    }

    #[test]
    fn worker_does_not_panic_when_file_missing() {
        let (tx, rx) = mpsc::channel();
        let handle = spawn_level_worker(
            PathBuf::from("does-not-exist.prl"),
            PathBuf::from("."),
            None,
            tx,
        );
        // Drop receiver before worker sends — send returns Err(SendError), which is swallowed.
        drop(rx);
        handle
            .join()
            .expect("worker thread must not panic when receiver is dropped");
    }
}
