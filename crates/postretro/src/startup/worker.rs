// Background level-load worker: PRL parse + texture decode + UV normalize.
// See: context/lib/boot_sequence.md

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::prl::{self, LevelWorld};
use crate::texture::{self, TextureSet};

/// Delivered to the main thread after the worker completes. All fields are plain
/// `Send` — no GPU handles.
pub(crate) struct LevelPayload {
    pub level: Option<LevelWorld>,
    pub textures: Option<TextureSet>,
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
            // texture_decode and uv_normalize don't run when PRL is missing.
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

    // Move into a binding to take `&mut` without borrowing through the `Option`.
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

/// Normalizes texel-space UVs to [0,1] using decoded texture dimensions.
/// Runs off-thread: decoded dimensions aren't available until after texture_decode.
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
                // Guard against future vertex deduplication — compiler currently emits a fresh copy per face.
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

    #[test]
    fn worker_does_not_panic_when_file_missing() {
        let (tx, rx) = mpsc::channel();
        let handle =
            spawn_level_worker(PathBuf::from("does-not-exist.prl"), PathBuf::from("."), tx);
        // Drop receiver before worker sends — send returns Err(SendError), which is swallowed.
        drop(rx);
        handle
            .join()
            .expect("worker thread must not panic when receiver is dropped");
    }
}
