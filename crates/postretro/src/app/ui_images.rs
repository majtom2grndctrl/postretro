// UI image registry producers other than glyph art: the mod's `uiImages` and
// the engine's own images. Loaded before glyph art, which re-registers after
// them so a glyph wins any key both claim. Loading-only mod images are split
// off here and load with their loading screen instead.
// See: context/lib/ui.md §5 · context/lib/boot_sequence.md §1

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use postretro_scripting_core::data_descriptors::{PresentationTemplate, RegisteredUiTree};
use postretro_scripting_core::runtime::ModMapEntry;

use crate::App;
use crate::startup::loading_screen::LOADING_SCREEN_NAME;

/// Engine image registry keys start here. A mod may not declare one; the
/// manifest parse already drops such names, and the loader refuses them too.
pub(crate) const ENGINE_IMAGE_PREFIX: &str = "engine/";

/// The boot splash logo, registered so UI trees (the engine fallback loading
/// screen first) can draw the same art the splash shows.
pub(crate) const SPLASH_LOGO_IMAGE: &str = "engine/splashLogo";

/// Every image the engine registers itself.
const ENGINE_IMAGES: &[&str] = &[SPLASH_LOGO_IMAGE];

/// The image keys one manifest's UI names, split by whether a loading
/// candidate names them. Built when a manifest commits, before its trees and
/// catalog drain elsewhere.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct ManifestImageRefs {
    /// Keys some loading-candidate tree names.
    loading: BTreeSet<String>,
    /// Keys some other mod tree or a presentation template names.
    other: BTreeSet<String>,
    /// Every tree background, as `(tree name, image key)`.
    backgrounds: Vec<(String, String)>,
}

impl ManifestImageRefs {
    /// Loading candidates are every name in a catalog entry's `loadingTree`
    /// pool, in the mod's `loading.tree` pool, and a mod tree registered as
    /// the engine's `loadingScreen`.
    pub(crate) fn from_manifest(
        trees: &[RegisteredUiTree],
        templates: &[PresentationTemplate],
        maps: &[ModMapEntry],
        mod_pool: &[String],
    ) -> Self {
        let candidates: HashSet<&str> = maps
            .iter()
            .flat_map(|map| map.loading_tree.iter())
            .chain(mod_pool)
            .map(String::as_str)
            .chain([LOADING_SCREEN_NAME])
            .collect();
        let mut refs = Self::default();
        for registered in trees {
            let keys = if candidates.contains(registered.name.as_str()) {
                &mut refs.loading
            } else {
                &mut refs.other
            };
            keys.extend(registered.tree.image_keys().into_iter().map(String::from));
            if let Some(background) = &registered.tree.background {
                refs.backgrounds
                    .push((registered.name.clone(), background.image.clone()));
            }
        }
        for template in templates {
            refs.other
                .extend(template.root.image_keys().into_iter().map(String::from));
        }
        refs
    }
}

/// What the renderer's image registry holds from the mod and the engine.
#[derive(Debug, Default)]
pub(crate) struct ModUiImages {
    /// The committed manifest's `uiImages`: key → mod-relative PNG path.
    committed: BTreeMap<String, String>,
    /// The committed entries that load at mod init and on staged reload:
    /// everything but `deferred`.
    eager: BTreeMap<String, String>,
    /// Loading-only keys: named by a loading candidate and by nothing else.
    /// They load with the loading screen that shows them.
    deferred: BTreeSet<String>,
    /// Tree backgrounds still to check for an unknown key, once per commit.
    unchecked_backgrounds: Option<Vec<(String, String)>>,
    /// The eager map and staged-reload generation the uploaded images were
    /// read at. `None` until the first load and after the renderer is lost.
    loaded: Option<(BTreeMap<String, String>, Option<u64>)>,
    /// Keys the mod's eager images registered.
    keys: HashSet<String>,
    /// Whether the engine's own images are uploaded to the current renderer.
    engine_loaded: bool,
    /// Keys already reported as shadowed by glyph art.
    warned_glyph_collisions: HashSet<String>,
}

impl ModUiImages {
    /// Commit a manifest's `uiImages` and the image keys its UI names (mod
    /// init, or a committed staged reload). The next sync reloads the eager
    /// images when they differ.
    pub(crate) fn commit(&mut self, images: BTreeMap<String, String>, refs: ManifestImageRefs) {
        self.deferred = images
            .keys()
            .filter(|key| refs.loading.contains(*key) && !refs.other.contains(*key))
            .cloned()
            .collect();
        self.eager = images
            .iter()
            .filter(|(key, _)| !self.deferred.contains(*key))
            .map(|(key, path)| (key.clone(), path.clone()))
            .collect();
        self.committed = images;
        self.unchecked_backgrounds = Some(refs.backgrounds);
    }

    /// The mod-relative path of a loading-only image, or `None` when `key` is
    /// not one under the committed manifest.
    pub(crate) fn deferred_path(&self, key: &str) -> Option<&str> {
        if !self.deferred.contains(key) {
            return None;
        }
        self.committed.get(key).map(String::as_str)
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
            *images == self.eager && *generation == reload_generation
        })
    }

    /// Warn once per mod image key glyph art also registered; the glyph's art
    /// is what the key draws. A loading-only key counts: glyph art keeps it
    /// when its loading screen shows.
    pub(crate) fn warn_glyph_collisions(&mut self, glyph_keys: &HashSet<String>) {
        let mod_keys: HashSet<String> = self.keys.iter().chain(&self.deferred).cloned().collect();
        for key in glyph_collisions(&mod_keys, glyph_keys, &mut self.warned_glyph_collisions) {
            log::warn!("[UI] uiImages.{key} shares its key with a glyph image; the glyph art wins");
        }
    }

    /// Warn, once per committed manifest, for each tree background naming no
    /// `uiImages` entry, engine image, or glyph art key: such a background
    /// draws nothing, and nothing else reports it.
    pub(crate) fn warn_unknown_backgrounds(&mut self, glyph_keys: &HashSet<String>) {
        let Some(backgrounds) = self.unchecked_backgrounds.take() else {
            return;
        };
        for (tree, key) in unknown_backgrounds(backgrounds, &self.committed, glyph_keys) {
            log::warn!(
                "[UI] tree `{tree}` background names `{key}`, which is no uiImages entry, engine image, or glyph; it draws nothing"
            );
        }
    }
}

/// Backgrounds whose key nothing registers, in commit order.
fn unknown_backgrounds(
    backgrounds: Vec<(String, String)>,
    images: &BTreeMap<String, String>,
    glyph_keys: &HashSet<String>,
) -> Vec<(String, String)> {
    backgrounds
        .into_iter()
        .filter(|(_, key)| {
            !images.contains_key(key)
                && !ENGINE_IMAGES.contains(&key.as_str())
                && !glyph_keys.contains(key)
        })
        .collect()
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

/// Decode one `uiImages` entry under `mod_root`. The error names the file and
/// the cause; the caller's warning names the entry. Runs on whichever thread
/// calls it: the main thread for eager images, a worker for loading-only ones.
pub(crate) fn decode_ui_image(
    mod_root: &Path,
    key: &str,
    relative: &str,
) -> Result<DecodedUiImage, String> {
    if key.starts_with(ENGINE_IMAGE_PREFIX) {
        return Err(format!("uses the reserved `{ENGINE_IMAGE_PREFIX}` prefix"));
    }
    let path = mod_root.join(relative);
    let image = image::open(&path).map_err(|err| format!("at {} ({err})", path.display()))?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(DecodedUiImage {
        key: key.to_string(),
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

/// Decode every entry of `images` under `mod_root`. An entry with a reserved
/// `engine/` key, a missing file, or a PNG that does not decode warns naming
/// the entry and is skipped; the rest still load.
pub(crate) fn decode_mod_ui_images(
    mod_root: &Path,
    images: &BTreeMap<String, String>,
) -> Vec<DecodedUiImage> {
    images
        .iter()
        .filter_map(
            |(key, relative)| match decode_ui_image(mod_root, key, relative) {
                Ok(image) => Some(image),
                Err(err) => {
                    log::warn!("[UI] uiImages.{key} did not load: {err}; skipping it");
                    None
                }
            },
        )
        .collect()
}

impl App {
    /// Upload the engine's images once per renderer, and the mod's eager
    /// `uiImages` at mod init and after each committed staged reload. Returns
    /// whether the mod's images were (re)registered, so glyph art can
    /// re-register after them. Cheap when nothing changed. Loading-only images
    /// are not touched here; their loading screen loads them.
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
        for image in decode_mod_ui_images(&self.content_root, &images.eager) {
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
                "[UI] loaded {} of {} eager mod UI image(s); deferred {} loading-only image(s) until a loading screen shows them",
                keys.len(),
                images.eager.len(),
                images.deferred.len()
            );
        }
        images.loaded = Some((images.eager.clone(), reload_generation));
        // A key a reload made eager now belongs to the mod's eager set; the
        // load that uploaded it must not release it.
        session.loading_screen.disown_images(&keys);
        images.keys = keys;
        true
    }
}

#[cfg(test)]
#[path = "ui_images_tests.rs"]
pub(crate) mod tests;
