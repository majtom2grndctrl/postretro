// Loading-only UI images: decoded on a worker when a load chooses its tree,
// uploaded by the Loading and Settling frames that poll, released when the
// loading screen ends. A level tree naming one promotes it to eager instead.
// See: context/lib/boot_sequence.md §1 · context/lib/ui.md §5

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use postretro_ui::descriptor::AnchoredTree;

use crate::App;
use crate::app::ui_images::{DecodedUiImage, ModUiImages, decode_ui_image};

use super::LoadingScreenState;

/// Where loading-only images upload to and release from: the renderer in
/// play, a plain map in tests.
pub(crate) trait LoadingImageRegistry {
    fn contains(&self, key: &str) -> bool;
    fn register(&mut self, image: DecodedUiImage) -> Result<(), String>;
    fn unregister(&mut self, key: &str);
}

impl LoadingImageRegistry for crate::render::Renderer {
    fn contains(&self, key: &str) -> bool {
        self.has_ui_image(key)
    }

    fn register(&mut self, image: DecodedUiImage) -> Result<(), String> {
        if !self.is_full_ready() {
            return Err("the renderer is not ready".to_string());
        }
        self.register_ui_image(&image.key, image.rgba, image.width, image.height)
    }

    fn unregister(&mut self, key: &str) {
        self.unregister_ui_image(key);
    }
}

/// One finished decode, sent from the worker.
struct LoadingImageDecode {
    key: String,
    /// The mod-relative path the worker read, so a reload that moved the
    /// entry can be told apart on arrival.
    path: String,
    result: Result<DecodedUiImage, String>,
    decode_ms: f64,
}

/// One load's loading-only images: the decodes in flight and the keys it
/// uploaded. Dropping it discards every decode still in flight: nothing
/// receives it, and the worker stops at its next send.
#[derive(Default)]
pub(crate) struct LoadingImages {
    /// Decodes started and not yet arrived: key → mod-relative path.
    pending: BTreeMap<String, String>,
    arrivals: Option<Receiver<LoadingImageDecode>>,
    /// Keys this load registered. The load releases exactly these.
    uploaded: BTreeSet<String>,
}

impl LoadingImages {
    /// Decode `wanted` (`(key, mod-relative path)`, in order) under
    /// `mod_root` on one worker thread. The main thread never waits on it.
    pub(crate) fn start(mod_root: &Path, wanted: Vec<(String, String)>) -> Self {
        if wanted.is_empty() {
            return Self::default();
        }
        let pending: BTreeMap<String, String> = wanted.iter().cloned().collect();
        let mod_root = mod_root.to_path_buf();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("loading-image-decode".to_string())
            .spawn(move || {
                for (key, path) in wanted {
                    let started = Instant::now();
                    let result = decode_ui_image(&mod_root, &key, &path);
                    let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
                    let decode = LoadingImageDecode {
                        key,
                        path,
                        result,
                        decode_ms,
                    };
                    // A closed channel means the load ended: skip the rest.
                    if tx.send(decode).is_err() {
                        return;
                    }
                }
            });
        match spawned {
            Ok(_) => Self {
                pending,
                arrivals: Some(rx),
                uploaded: BTreeSet::new(),
            },
            Err(err) => {
                log::warn!(
                    "[UI] could not start decoding loading-screen images ({err}); this load draws none"
                );
                Self::default()
            }
        }
    }

    /// Whether a started decode has not arrived yet.
    #[cfg(test)]
    fn is_decoding(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Upload every decode that has arrived. One the committed manifest no
    /// longer matches — the key is no longer loading-only, or its path moved —
    /// is discarded, as is one whose key something else registered meanwhile
    /// (glyph art, or an eager image a reload kept).
    pub(crate) fn upload_arrived(
        &mut self,
        images: &ModUiImages,
        registry: &mut impl LoadingImageRegistry,
    ) {
        let Some(arrivals) = &self.arrivals else {
            return;
        };
        let mut arrived = Vec::new();
        let worker_gone = loop {
            match arrivals.try_recv() {
                Ok(decode) => arrived.push(decode),
                Err(TryRecvError::Empty) => break false,
                Err(TryRecvError::Disconnected) => break true,
            }
        };
        for decode in arrived {
            let LoadingImageDecode {
                key,
                path,
                result,
                decode_ms,
            } = decode;
            let started_for = self.pending.remove(&key);
            if started_for.as_deref() != Some(path.as_str())
                || images.deferred_path(&key) != Some(path.as_str())
            {
                log::debug!("[UI] discarded a stale decode of uiImages.{key}");
                continue;
            }
            let image = match result {
                Ok(image) => image,
                Err(err) => {
                    log::warn!("[UI] uiImages.{key} did not load: {err}; skipping it");
                    continue;
                }
            };
            if registry.contains(&key) {
                log::debug!("[UI] uiImages.{key} is already registered; keeping that one");
                continue;
            }
            let (width, height) = (image.width, image.height);
            match registry.register(image) {
                Ok(()) => {
                    log::info!(
                        "[UI] loading-screen image `{key}` uploaded: {width}x{height} px, decoded in {decode_ms:.1} ms"
                    );
                    self.uploaded.insert(key);
                }
                Err(err) => log::warn!("[UI] uiImages.{key} did not load ({err}); skipping it"),
            }
        }
        // The worker sends every decode before it exits, so a closed channel
        // with decodes still owed means it panicked partway.
        if worker_gone {
            for key in std::mem::take(&mut self.pending).into_keys() {
                log::warn!(
                    "[UI] uiImages.{key} did not load: its decode worker stopped; skipping it"
                );
            }
        }
        if self.pending.is_empty() {
            self.arrivals = None;
        }
    }

    /// Unregister every image this load uploaded. Decodes still in flight are
    /// discarded as `self` drops.
    pub(crate) fn release(self, registry: &mut impl LoadingImageRegistry) {
        for key in &self.uploaded {
            registry.unregister(key);
        }
    }

    /// Hand `keys` to whoever just re-registered them; this load no longer
    /// releases them.
    pub(crate) fn disown(&mut self, keys: &HashSet<String>) {
        self.uploaded.retain(|key| !keys.contains(key));
    }
}

/// The loading-only images `tree` names that are not registered yet, once
/// each, as `(key, mod-relative path)` in tree order.
pub(crate) fn wanted_loading_images(
    tree: &AnchoredTree,
    images: &ModUiImages,
    registered: impl Fn(&str) -> bool,
) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    tree.image_keys()
        .into_iter()
        .filter(|key| seen.insert(*key))
        .filter_map(|key| {
            let path = images.deferred_path(key)?;
            (!registered(key)).then(|| (key.to_string(), path.to_string()))
        })
        .collect()
}

/// Promote the loading-only images among `level_keys` (the image keys the
/// installed level's trees name) to eager, registering each now unless
/// something already holds its key. The active load disowns them, so its end
/// releases none, and a decode of one still in flight is discarded on
/// arrival. Level install is already a blocking frame, so the decode runs
/// here, on the main thread.
pub(crate) fn promote_level_images<'a>(
    level_keys: impl IntoIterator<Item = &'a str>,
    mod_root: &Path,
    images: &mut ModUiImages,
    loading: &mut LoadingScreenState,
    registry: &mut impl LoadingImageRegistry,
) {
    let mut registered = HashSet::new();
    for (key, path) in images.promote(level_keys) {
        if registry.contains(&key) {
            // This load's upload, or glyph art that wins the key.
            registered.insert(key);
            continue;
        }
        let started = Instant::now();
        let image = match decode_ui_image(mod_root, &key, &path) {
            Ok(image) => image,
            Err(err) => {
                log::warn!("[UI] uiImages.{key} did not load: {err}; skipping it");
                continue;
            }
        };
        let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
        let (width, height) = (image.width, image.height);
        match registry.register(image) {
            Ok(()) => {
                log::info!(
                    "[UI] level tree promoted loading-screen image `{key}` to eager: {width}x{height} px, decoded in {decode_ms:.1} ms"
                );
                registered.insert(key);
            }
            Err(err) => log::warn!("[UI] uiImages.{key} did not load ({err}); skipping it"),
        }
    }
    loading.disown_images(&registered);
    images.note_promoted_registered(&registered);
}

/// Every image key the level-tier trees name.
fn level_tree_image_keys(stack: &postretro_ui::modal_stack::ModalStack) -> Vec<&str> {
    stack
        .resolved_trees()
        .filter(|(tier, _)| *tier == postretro_ui::modal_stack::ScopeTier::Level)
        .flat_map(|(_, tree)| tree.image_keys())
        .collect()
}

impl App {
    /// Promote the loading-only images the just-installed level's trees name
    /// (`promote_level_images`). Runs once per level install.
    pub(crate) fn promote_level_tree_images(&mut self) {
        let (Some(session), Some(renderer)) = (self.session.as_mut(), self.renderer.as_mut())
        else {
            return;
        };
        if !renderer.is_full_ready() {
            return;
        }
        promote_level_images(
            level_tree_image_keys(&session.modal_stack),
            &self.content_root,
            &mut session.mod_ui_images,
            &mut session.loading_screen,
            renderer,
        );
    }

    /// Upload the active load's loading-only images that finished decoding.
    /// Loading and Settling frames call this; it never waits on the worker.
    pub(crate) fn poll_loading_images(&mut self) {
        let (Some(session), Some(renderer)) = (self.session.as_mut(), self.renderer.as_mut())
        else {
            return;
        };
        if !renderer.is_full_ready() {
            return;
        }
        let Some(load) = session.loading_screen.active.as_mut() else {
            return;
        };
        load.images.upload_arrived(&session.mod_ui_images, renderer);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use postretro_scripting_core::runtime::ModLoading;
    use postretro_ui::modal_stack::ScopeTier;

    use super::*;
    use crate::app::ui_images::ManifestImageRefs;
    use crate::startup::LevelLoadEntry;
    use crate::startup::lifecycle::tests::test_app;

    const SHOT: &str = "shots/e1m1";
    const TREE: &str = "load.e1m1";

    /// The registry side as a map of key → size.
    #[derive(Default)]
    struct FakeRegistry(HashMap<String, [u32; 2]>);

    impl LoadingImageRegistry for FakeRegistry {
        fn contains(&self, key: &str) -> bool {
            self.0.contains_key(key)
        }
        fn register(&mut self, image: DecodedUiImage) -> Result<(), String> {
            self.0.insert(image.key, [image.width, image.height]);
            Ok(())
        }
        fn unregister(&mut self, key: &str) {
            self.0.remove(key);
        }
    }

    /// A mod root holding a 4×2 screenshot at `ui/e1m1.png` and at
    /// `ui/moved.png`.
    fn mod_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("ui")).unwrap();
        for file in ["ui/e1m1.png", "ui/moved.png"] {
            image::RgbaImage::from_pixel(4, 2, image::Rgba([90, 60, 30, 255]))
                .save(root.path().join(file))
                .unwrap();
        }
        root
    }

    fn ui_images(path: &str) -> BTreeMap<String, String> {
        [(SHOT.to_string(), path.to_string())].into()
    }

    /// The manifest image references of a mod whose catalog map `e1m1` shows
    /// `TREE`, which draws `SHOT` as its background.
    fn refs(
        trees: &[postretro_scripting_core::data_descriptors::RegisteredUiTree],
    ) -> ManifestImageRefs {
        ManifestImageRefs::from_manifest(
            trees,
            &[],
            &[crate::app::ui_images::tests::map("e1m1", &[TREE])],
            &[],
        )
    }

    /// An app whose mod registers `TREE` and commits `SHOT` at `path`.
    fn app_with_loading_tree(mod_root: &Path, path: &str) -> App {
        let mut app = test_app();
        app.content_root = mod_root.to_path_buf();
        let tree = crate::app::ui_images::tests::tree(TREE, Some(SHOT), &[]);
        app.session
            .as_mut()
            .unwrap()
            .modal_stack
            .register_script_trees(vec![tree.clone()], ScopeTier::Mod);
        app.commit_loading_manifest(ui_images(path), ModLoading::default(), refs(&[tree]));
        app
    }

    fn entry() -> LevelLoadEntry {
        LevelLoadEntry {
            catalog_id: Some("e1m1".to_string()),
            path: "maps/e1m1.prl".to_string(),
            name: "Entryway".to_string(),
            tags: Vec::new(),
            loading_tree: vec![TREE.to_string()],
        }
    }

    fn active_images(app: &mut App) -> &mut LoadingImages {
        &mut app
            .session
            .as_mut()
            .unwrap()
            .loading_screen
            .active
            .as_mut()
            .expect("a load is showing")
            .images
    }

    /// Poll the active load's arrivals into `registry` until every started
    /// decode has arrived, as successive Loading frames would.
    fn poll_until_decoded(app: &mut App, registry: &mut FakeRegistry) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let session = app.session.as_mut().unwrap();
            let load = session.loading_screen.active.as_mut().unwrap();
            load.images.upload_arrived(&session.mod_ui_images, registry);
            if !load.images.is_decoding() {
                return;
            }
            assert!(Instant::now() < deadline, "the decode never arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn loading_images_begin_decode_upload_registers_the_chosen_trees_image() {
        let root = mod_root();
        let mut app = app_with_loading_tree(root.path(), "ui/e1m1.png");
        let capture = postretro_test_log_capture::LogCapture::start();

        app.begin_loading_screen(&entry());
        assert!(
            active_images(&mut app).is_decoding(),
            "choosing the tree starts its decode"
        );
        let mut registry = FakeRegistry::default();
        poll_until_decoded(&mut app, &mut registry);

        assert_eq!(registry.0.get(SHOT), Some(&[4, 2]));
        capture.assert_logged_once(
            log::Level::Info,
            "loading-screen image `shots/e1m1` uploaded: 4x2 px",
        );
    }

    #[test]
    fn loading_images_release_unregisters_what_the_load_uploaded() {
        let root = mod_root();
        let mut app = app_with_loading_tree(root.path(), "ui/e1m1.png");
        app.begin_loading_screen(&entry());
        let mut registry = FakeRegistry::default();
        registry.0.insert("art/logo".to_string(), [1, 1]);
        poll_until_decoded(&mut app, &mut registry);

        let images = std::mem::take(active_images(&mut app));
        images.release(&mut registry);
        assert!(!registry.contains(SHOT), "the load's image is released");
        assert!(registry.contains("art/logo"), "nothing else is touched");
    }

    #[test]
    fn loading_images_end_before_the_decode_lands_discards_it() {
        let root = mod_root();
        let mut app = app_with_loading_tree(root.path(), "ui/e1m1.png");
        app.begin_loading_screen(&entry());
        app.end_loading_screen();
        assert!(
            app.session
                .as_ref()
                .unwrap()
                .loading_screen
                .active
                .is_none()
        );

        // The next load shows a tree with no images. The first load's
        // channel closed with it, so nothing its decode produces, whenever it
        // finishes, can reach the registry.
        let mut plain = entry();
        plain.loading_tree.clear();
        app.begin_loading_screen(&plain);
        let mut registry = FakeRegistry::default();
        poll_until_decoded(&mut app, &mut registry);
        assert!(registry.0.is_empty());
    }

    #[test]
    fn loading_images_reload_that_changes_the_entry_discards_the_stale_decode() {
        let root = mod_root();
        let tree = crate::app::ui_images::tests::tree(TREE, Some(SHOT), &[]);
        let other_tree = crate::app::ui_images::tests::tree("hud", Some(SHOT), &[]);
        for (label, path, trees) in [
            ("the entry moved", "ui/moved.png", vec![tree.clone()]),
            (
                "the image became eager",
                "ui/e1m1.png",
                vec![tree.clone(), other_tree],
            ),
        ] {
            let mut app = app_with_loading_tree(root.path(), "ui/e1m1.png");
            app.begin_loading_screen(&entry());
            app.commit_loading_manifest(ui_images(path), ModLoading::default(), refs(&trees));

            let mut registry = FakeRegistry::default();
            poll_until_decoded(&mut app, &mut registry);
            assert!(
                registry.0.is_empty(),
                "{label}: the stale decode is discarded"
            );
        }
    }

    #[test]
    fn loading_images_wanted_skips_registered_and_non_loading_keys() {
        let mut images = ModUiImages::default();
        let tree = crate::app::ui_images::tests::tree(TREE, Some(SHOT), &[SHOT, "art/logo"]);
        images.commit(
            [
                (SHOT.to_string(), "ui/e1m1.png".to_string()),
                ("art/logo".to_string(), "ui/logo.png".to_string()),
                ("shots/other".to_string(), "ui/other.png".to_string()),
            ]
            .into(),
            ManifestImageRefs::from_manifest(
                &[
                    tree.clone(),
                    crate::app::ui_images::tests::tree("hud", None, &["art/logo"]),
                ],
                &[],
                &[crate::app::ui_images::tests::map("e1m1", &[TREE])],
                &[],
            ),
        );

        assert_eq!(
            wanted_loading_images(&tree.tree, &images, |_| false),
            [(SHOT.to_string(), "ui/e1m1.png".to_string())],
            "a key named twice decodes once; an eager key is not wanted"
        );
        assert!(wanted_loading_images(&tree.tree, &images, |key| key == SHOT).is_empty());
    }

    #[test]
    fn loading_images_disowned_by_a_later_registration_are_not_released() {
        let root = mod_root();
        let mut app = app_with_loading_tree(root.path(), "ui/e1m1.png");
        app.begin_loading_screen(&entry());
        let mut registry = FakeRegistry::default();
        poll_until_decoded(&mut app, &mut registry);

        app.session
            .as_mut()
            .unwrap()
            .loading_screen
            .disown_images(&[SHOT.to_string()].into());
        let images = std::mem::take(active_images(&mut app));
        images.release(&mut registry);
        assert!(
            registry.contains(SHOT),
            "glyph art or an eager reload owns it now"
        );
    }

    /// Register a level-tier tree drawing `SHOT` and promote what the level's
    /// trees name into `registry`, as level install does.
    fn install_level_tree(app: &mut App, registry: &mut FakeRegistry) {
        let session = app.session.as_mut().unwrap();
        session.modal_stack.register_script_trees(
            vec![crate::app::ui_images::tests::tree(
                "level.intro",
                Some(SHOT),
                &[],
            )],
            ScopeTier::Level,
        );
        promote_level_images(
            level_tree_image_keys(&session.modal_stack),
            &app.content_root,
            &mut session.mod_ui_images,
            &mut session.loading_screen,
            registry,
        );
    }

    #[test]
    fn loading_images_level_tree_promotes_a_loading_only_image_to_eager() {
        let root = mod_root();
        let mut app = app_with_loading_tree(root.path(), "ui/e1m1.png");
        let mut registry = FakeRegistry::default();
        let capture = postretro_test_log_capture::LogCapture::start();

        install_level_tree(&mut app, &mut registry);

        assert_eq!(
            registry.0.get(SHOT),
            Some(&[4, 2]),
            "install registers it at once"
        );
        let images = &app.session.as_ref().unwrap().mod_ui_images;
        assert_eq!(images.deferred_path(SHOT), None, "no longer loading-only");
        capture.assert_logged_once(
            log::Level::Info,
            "level tree promoted loading-screen image `shots/e1m1` to eager: 4x2 px",
        );

        // A later load showing the same tree wants nothing: the key is eager.
        app.begin_loading_screen(&entry());
        assert!(!active_images(&mut app).is_decoding());
    }

    #[test]
    fn loading_images_end_does_not_release_a_promoted_image() {
        let root = mod_root();
        for uploaded_first in [true, false] {
            let mut app = app_with_loading_tree(root.path(), "ui/e1m1.png");
            app.begin_loading_screen(&entry());
            let mut registry = FakeRegistry::default();
            if uploaded_first {
                // The load uploaded the image before the level installed.
                poll_until_decoded(&mut app, &mut registry);
                assert!(registry.contains(SHOT));
            }
            install_level_tree(&mut app, &mut registry);
            // A decode still in flight lands after the promotion; it is stale.
            poll_until_decoded(&mut app, &mut registry);

            let images = std::mem::take(active_images(&mut app));
            images.release(&mut registry);
            assert!(
                registry.contains(SHOT),
                "uploaded first: {uploaded_first}; the level tree keeps it"
            );
        }
    }

    #[test]
    fn loading_images_next_commit_recomputes_the_promoted_set() {
        let root = mod_root();
        let mut app = app_with_loading_tree(root.path(), "ui/e1m1.png");
        let mut registry = FakeRegistry::default();
        install_level_tree(&mut app, &mut registry);
        let deferred = |app: &App| {
            app.session
                .as_ref()
                .unwrap()
                .mod_ui_images
                .deferred_path(SHOT)
                .map(str::to_string)
        };
        assert_eq!(deferred(&app), None);

        let tree = crate::app::ui_images::tests::tree(TREE, Some(SHOT), &[]);
        app.commit_loading_manifest(
            ui_images("ui/e1m1.png"),
            ModLoading::default(),
            refs(&[tree]),
        );
        assert_eq!(
            deferred(&app).as_deref(),
            Some("ui/e1m1.png"),
            "the commit's own trees decide again; the level tier is not one"
        );
    }

    #[test]
    fn loading_images_worker_that_stops_early_warns_for_what_it_owed() {
        let (tx, rx) = mpsc::channel::<LoadingImageDecode>();
        drop(tx);
        let mut images = LoadingImages {
            pending: [(SHOT.to_string(), "ui/e1m1.png".to_string())].into(),
            arrivals: Some(rx),
            uploaded: BTreeSet::new(),
        };
        let capture = postretro_test_log_capture::LogCapture::start();
        images.upload_arrived(&ModUiImages::default(), &mut FakeRegistry::default());
        assert!(!images.is_decoding());
        capture.assert_logged_once(
            log::Level::Warn,
            "uiImages.shots/e1m1 did not load: its decode worker stopped",
        );
    }
}
