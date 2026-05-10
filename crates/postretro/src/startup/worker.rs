// Background level-load worker: PRL parse + texture decode + UV normalize.
// See: context/lib/boot_sequence.md

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::prl::{self, LevelWorld};
use crate::texture::{self, TextureSet};

/// Result of one level-load worker run delivered back to the main thread.
///
/// All fields are plain `Send` data — no engine handles, no GPU resources —
/// per the boot-phase concurrency model in `context/lib/boot_sequence.md §8`.
pub(crate) struct LevelPayload {
    pub level: Option<LevelWorld>,
    pub textures: Option<TextureSet>,
    /// Worker-thread stage entries (`prl_parse`, `texture_decode`,
    /// `uv_normalize`). The main thread splices these into the level-load
    /// `StartupTimings` instance between `worker_dispatch` and
    /// `worker_delivered`.
    pub timings: Vec<(&'static str, Duration)>,
}

// `LevelPayload` must be `Send`: worker outputs cross the thread boundary via
// mpsc, per the boot-phase concurrency model in `boot_sequence.md` §8.
// This assertion catches accidental addition of non-Send fields (e.g. Rc, Cell)
// at compile time rather than at the thread::spawn call site.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<LevelPayload>();
};

pub(crate) type LoadOutcome = Result<LevelPayload, anyhow::Error>;

/// Spawn the level-load worker. Reads the PRL from `map_path`, decodes its
/// referenced textures from `<content_root>/textures`, and normalizes UVs
/// against the resulting texture dimensions.
///
/// Send errors are ignored: a dropped receiver means the window closed during
/// load, which is a clean shutdown path.
pub(crate) fn spawn_level_worker(
    map_path: PathBuf,
    content_root: PathBuf,
    sender: mpsc::Sender<LoadOutcome>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let outcome = run_worker(&map_path, &content_root);
        // Receiver dropped = window closed during load; nothing to do.
        let _ = sender.send(outcome);
    })
}

fn run_worker(map_path: &Path, content_root: &Path) -> LoadOutcome {
    let mut timings: Vec<(&'static str, Duration)> = Vec::with_capacity(3);
    let mut cursor = Instant::now();

    let path_str = map_path.to_string_lossy().into_owned();
    let level = match prl::load_prl(&path_str) {
        Ok(world) => {
            log::info!("[Loader] PRL loaded successfully from {path_str}");
            Some(world)
        }
        Err(prl::PrlLoadError::FileNotFound(p)) => {
            log::warn!("[Loader] PRL file not found: {p} — starting without map");
            let now = Instant::now();
            timings.push(("prl_parse", now.duration_since(cursor)));
            // Fewer entries than the success path — texture decode and UV
            // normalize don't run when the PRL is missing.
            return Ok(LevelPayload {
                level: None,
                textures: None,
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
        cursor = now;
    }

    let textures = match level.as_ref() {
        Some(world) if !world.texture_names.is_empty() => {
            let texture_root = content_root.join("textures");
            log::info!(
                "[Loader] Loading PRL textures from {}",
                texture_root.display()
            );
            let texture_names: Vec<Option<String>> = world
                .texture_names
                .iter()
                .map(|n| Some(n.clone()))
                .collect();
            Some(texture::load_textures(&texture_names, &texture_root))
        }
        _ => None,
    };
    {
        let now = Instant::now();
        timings.push(("texture_decode", now.duration_since(cursor)));
        cursor = now;
    }

    // UV normalize mutates the world in-place; move into a binding so we
    // can take `&mut` without borrowing through the `Option`.
    let mut level = level;
    if let (Some(world), Some(tex_set)) = (level.as_mut(), textures.as_ref()) {
        normalize_prl_uvs(world, tex_set);
    }
    {
        let now = Instant::now();
        timings.push(("uv_normalize", now.duration_since(cursor)));
    }

    Ok(LevelPayload {
        level,
        textures,
        timings,
    })
}

/// Divides each face vertex's `base_uv` by its texture's pixel dimensions so
/// the renderer receives normalized [0,1] UVs regardless of source texture size.
/// Runs off the main thread; UV normalization requires decoded texture dimensions
/// available only after `texture_decode` completes.
fn normalize_prl_uvs(world: &mut LevelWorld, texture_set: &TextureSet) {
    let mut normalized = vec![false; world.vertices.len()];

    for leaf in &world.bvh.leaves {
        let tex_idx = leaf.material_bucket_id as usize;
        let (w, h) = match texture_set.textures.get(tex_idx) {
            Some(tex) => (tex.width, tex.height),
            None => continue,
        };
        if w == 0 || h == 0 {
            continue;
        }

        let start = leaf.index_offset as usize;
        let count = leaf.index_count as usize;
        for i in start..start + count {
            if let Some(&idx) = world.indices.get(i) {
                let vi = idx as usize;
                // The compiler emits a fresh vertex copy per face, so sharing
                // across leaves is not expected — this guard is defensive
                // against future vertex deduplication.
                if vi < normalized.len() && !normalized[vi] {
                    if let Some(vert) = world.vertices.get_mut(vi) {
                        vert.base_uv[0] /= w as f32;
                        vert.base_uv[1] /= h as f32;
                        normalized[vi] = true;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the worker completes without panicking when pointed at a
    /// non-existent map. PRL parse short-circuits on `FileNotFound`; the worker
    /// swallows the error and returns a partial payload. A panicking worker
    /// would poison the join.
    #[test]
    fn worker_does_not_panic_when_file_missing() {
        let (tx, rx) = mpsc::channel();
        let handle =
            spawn_level_worker(PathBuf::from("does-not-exist.prl"), PathBuf::from("."), tx);
        // Drop the receiver before the worker sends. The worker's `send` will
        // return `Err(SendError)`, which `spawn_level_worker` swallows with
        // `let _ = sender.send(outcome);`.
        drop(rx);
        handle
            .join()
            .expect("worker thread must not panic when receiver is dropped");
    }
}
