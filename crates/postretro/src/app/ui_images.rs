// UI image registry producers other than glyph art: the mod's `uiImages` and
// the engine's own images. Loaded before glyph art, which re-registers after
// them so a glyph wins any key both claim.
// See: context/lib/ui.md §5

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::App;

/// Engine image registry keys start here. A mod may not declare one; the
/// manifest parse already drops such names, and the loader refuses them too.
pub(crate) const ENGINE_IMAGE_PREFIX: &str = "engine/";

/// The boot splash logo, registered so UI trees (the engine fallback loading
/// screen first) can draw the same art the splash shows.
pub(crate) const SPLASH_LOGO_IMAGE: &str = "engine/splashLogo";

/// What the renderer's image registry holds from the mod and the engine.
#[derive(Debug, Default)]
pub(crate) struct ModUiImages {
    /// The committed manifest's `uiImages`: key → mod-relative PNG path.
    committed: BTreeMap<String, String>,
    /// The map and staged-reload generation the uploaded images were read at.
    /// `None` until the first load and after the renderer is lost.
    loaded: Option<(BTreeMap<String, String>, Option<u64>)>,
    /// Keys the mod's images registered.
    keys: HashSet<String>,
    /// Whether the engine's own images are uploaded to the current renderer.
    engine_loaded: bool,
    /// Keys already reported as shadowed by glyph art.
    warned_glyph_collisions: HashSet<String>,
}

impl ModUiImages {
    /// Commit a manifest's `uiImages` (mod init, or a committed staged reload).
    /// The next sync reloads when the map differs.
    pub(crate) fn commit(&mut self, images: BTreeMap<String, String>) {
        self.committed = images;
    }

    /// The renderer holding the images is gone; upload everything again to the
    /// next one.
    pub(crate) fn forget_uploads(&mut self) {
        self.loaded = None;
        self.engine_loaded = false;
        self.keys.clear();
    }

    fn is_current(&self, reload_generation: Option<u64>) -> bool {
        self.loaded.as_ref().is_some_and(|(images, generation)| {
            *images == self.committed && *generation == reload_generation
        })
    }

    /// Warn once per mod image key glyph art also registered; the glyph's art
    /// is what the key draws.
    pub(crate) fn warn_glyph_collisions(&mut self, glyph_keys: &HashSet<String>) {
        for key in glyph_collisions(&self.keys, glyph_keys, &mut self.warned_glyph_collisions) {
            log::warn!("[UI] uiImages.{key} shares its key with a glyph image; the glyph art wins");
        }
    }
}

/// Mod image keys glyph art also claims, not yet reported, in sorted order.
fn glyph_collisions(
    mod_keys: &HashSet<String>,
    glyph_keys: &HashSet<String>,
    warned: &mut HashSet<String>,
) -> Vec<String> {
    let mut collisions: Vec<String> = mod_keys
        .intersection(glyph_keys)
        .filter(|key| warned.insert((*key).clone()))
        .cloned()
        .collect();
    collisions.sort();
    collisions
}

/// One decoded image ready for upload.
pub(crate) struct DecodedUiImage {
    pub(crate) key: String,
    pub(crate) rgba: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Decode every `uiImages` entry under `mod_root`. An entry with a reserved
/// `engine/` key, a missing file, or a PNG that does not decode warns naming
/// the entry and is skipped; the rest still load.
pub(crate) fn decode_mod_ui_images(
    mod_root: &Path,
    images: &BTreeMap<String, String>,
) -> Vec<DecodedUiImage> {
    let mut decoded = Vec::with_capacity(images.len());
    for (key, relative) in images {
        if key.starts_with(ENGINE_IMAGE_PREFIX) {
            log::warn!(
                "[UI] uiImages.{key} uses the reserved `{ENGINE_IMAGE_PREFIX}` prefix; skipping it"
            );
            continue;
        }
        let path = mod_root.join(relative);
        match image::open(&path) {
            Ok(image) => {
                let rgba = image.to_rgba8();
                let (width, height) = rgba.dimensions();
                decoded.push(DecodedUiImage {
                    key: key.clone(),
                    rgba: rgba.into_raw(),
                    width,
                    height,
                });
            }
            Err(err) => log::warn!(
                "[UI] uiImages.{key} at {} did not load ({err}); skipping it",
                path.display()
            ),
        }
    }
    decoded
}

impl App {
    /// Upload the engine's images once per renderer, and the mod's `uiImages`
    /// at mod init and after each committed staged reload. Returns whether the
    /// mod's images were (re)registered, so glyph art can re-register after
    /// them. Cheap when nothing changed.
    pub(crate) fn sync_ui_images(&mut self) -> bool {
        let (Some(session), Some(renderer)) = (self.session.as_mut(), self.renderer.as_mut())
        else {
            return false;
        };
        if !renderer.is_full_ready() {
            return false;
        }
        let images = &mut session.mod_ui_images;
        if !images.engine_loaded {
            images.engine_loaded = true;
            let registered = crate::render::splash::load_splash(
                &crate::startup::SplashSource::Base,
                &self.core_root,
            )
            .map_err(|err| format!("{err:#}"))
            .and_then(|logo| {
                renderer.register_ui_image(SPLASH_LOGO_IMAGE, logo.data, logo.width, logo.height)
            });
            if let Err(err) = registered {
                log::warn!(
                    "[UI] engine image `{SPLASH_LOGO_IMAGE}` did not load ({err}); trees drawing it show nothing there"
                );
            }
        }
        let reload_generation = session
            .scripting
            .script_runtime
            .committed_staged_manifest_generation();
        if images.is_current(reload_generation) {
            return false;
        }
        let mut keys = HashSet::new();
        for image in decode_mod_ui_images(&self.content_root, &images.committed) {
            match renderer.register_ui_image(&image.key, image.rgba, image.width, image.height) {
                Ok(()) => {
                    keys.insert(image.key);
                }
                Err(err) => log::warn!(
                    "[UI] uiImages.{} did not load ({err}); skipping it",
                    image.key
                ),
            }
        }
        if !images.committed.is_empty() {
            log::info!(
                "[UI] loaded {} of {} mod UI image(s)",
                keys.len(),
                images.committed.len()
            );
        }
        images.loaded = Some((images.committed.clone(), reload_generation));
        images.keys = keys;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_test_log_capture::LogCapture;

    /// A mod root holding one 3×2 PNG at `ui/good.png` and one file that is not
    /// a PNG at `ui/broken.png`.
    fn mod_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("ui")).unwrap();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]))
            .save(root.path().join("ui/good.png"))
            .unwrap();
        std::fs::write(root.path().join("ui/broken.png"), b"not a png").unwrap();
        root
    }

    fn images(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(key, path)| (key.to_string(), path.to_string()))
            .collect()
    }

    #[test]
    fn ui_images_decode_good_entries_and_warn_naming_the_bad_ones() {
        let root = mod_root();
        let capture = LogCapture::start();
        let decoded = decode_mod_ui_images(
            root.path(),
            &images(&[
                ("art/good", "ui/good.png"),
                ("art/missing", "ui/missing.png"),
                ("art/broken", "ui/broken.png"),
            ]),
        );

        assert_eq!(decoded.len(), 1, "only the decodable entry loads");
        assert_eq!(decoded[0].key, "art/good");
        assert_eq!((decoded[0].width, decoded[0].height), (3, 2));
        assert_eq!(decoded[0].rgba.len(), 3 * 2 * 4);
        capture.assert_logged_once(log::Level::Warn, "uiImages.art/missing");
        capture.assert_logged_once(log::Level::Warn, "uiImages.art/broken");
    }

    /// The manifest parse already drops `engine/` names; the loader refuses
    /// them again, so no mod entry can replace an engine image.
    #[test]
    fn ui_images_never_load_an_engine_prefixed_key() {
        let root = mod_root();
        let capture = LogCapture::start();
        let decoded = decode_mod_ui_images(
            root.path(),
            &images(&[
                (SPLASH_LOGO_IMAGE, "ui/good.png"),
                ("art/good", "ui/good.png"),
            ]),
        );
        let keys: Vec<&str> = decoded.iter().map(|image| image.key.as_str()).collect();
        assert_eq!(keys, ["art/good"]);
        capture.assert_logged_once(log::Level::Warn, "reserved `engine/` prefix");
    }

    #[test]
    fn ui_images_shadowed_by_glyph_art_warn_once_per_key() {
        let mod_keys: HashSet<String> = ["ui/glyphs/kbm/space", "art/logo"]
            .into_iter()
            .map(String::from)
            .collect();
        let glyph_keys: HashSet<String> = ["ui/glyphs/kbm/space", "ui/glyphs/kbm/e"]
            .into_iter()
            .map(String::from)
            .collect();
        let mut warned = HashSet::new();
        assert_eq!(
            glyph_collisions(&mod_keys, &glyph_keys, &mut warned),
            ["ui/glyphs/kbm/space"]
        );
        assert!(
            glyph_collisions(&mod_keys, &glyph_keys, &mut warned).is_empty(),
            "a reload that keeps the collision does not warn again"
        );
    }

    #[test]
    fn ui_images_reload_when_the_map_or_staged_generation_changes() {
        let mut state = ModUiImages::default();
        assert!(!state.is_current(None), "nothing has loaded yet");
        state.commit(images(&[("art/a", "ui/a.png")]));
        state.loaded = Some((state.committed.clone(), None));
        assert!(state.is_current(None));
        assert!(!state.is_current(Some(1)), "a committed reload reloads");
        state.commit(images(&[("art/b", "ui/b.png")]));
        assert!(!state.is_current(None), "a changed map reloads");
        state.forget_uploads();
        assert!(state.loaded.is_none() && !state.engine_loaded);
    }
}
