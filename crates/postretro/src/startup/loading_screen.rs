// Loading screen: which UI tree a level load shows, the two `loading.*`
// slots it publishes, the world-less frame it draws on, and the one frame of
// delay between the worker's delivery and the install.
// See: context/lib/boot_sequence.md §1, §4 · context/lib/ui.md §1, §3, §5

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use postretro_entities::slot_table::SlotValue;
use postretro_level_loader::LoadProgress;
use postretro_scripting_core::runtime::ModLoading;
use winit::event_loop::ActiveEventLoop;

use crate::App;
use crate::app::ui_images::ManifestImageRefs;
use crate::startup::LevelLoadEntry;
use crate::startup::worker::LevelPayload;

#[path = "loading_screen_images.rs"]
mod images;
use images::{LoadingImages, wanted_loading_images};

/// Registry name of the engine fallback loading tree (`core/ui/loadingScreen.json`).
/// A mod tree under the same name shadows it.
pub(crate) const LOADING_SCREEN_NAME: &str = "loadingScreen";

/// Share of the bar the worker's parse fills. The rest stands for the
/// main-thread install and the Settling hold: the bar holds here for the frame
/// painted between delivery and install, then rises with the settle to 1.0.
pub(crate) const LOAD_PARSE_SHARE: f32 = 0.85;

const PROGRESS_SLOT: &str = "loading.progress";
const LEVEL_NAME_SLOT: &str = "loading.levelName";

/// Session-owned loading-screen state: the committed mod-wide pool and the
/// load in progress, if any.
#[derive(Default)]
pub(crate) struct LoadingScreenState {
    /// The committed manifest's `loading.tree` pool.
    mod_pool: Vec<String>,
    /// Pool names already reported as unregistered.
    warned_unregistered: HashSet<String>,
    active: Option<ActiveLoad>,
}

struct ActiveLoad {
    /// The tree this load shows, chosen once when it began. `None` when no
    /// candidate, not even the engine fallback, is registered.
    tree: Option<String>,
    progress: Arc<LoadProgress>,
    /// The last value written to `loading.progress`; it never decreases.
    shown: f32,
    /// UI time for this load's frames, so tweens on the loading tree run.
    ui_time: f64,
    /// A delivered payload waiting one painted frame before it installs.
    delivered: Option<LevelPayload>,
    /// The chosen tree's loading-only images: decoding, then uploaded until
    /// this load ends.
    images: LoadingImages,
}

impl LoadingScreenState {
    /// Commit the manifest's `loading` block (mod init or a committed staged
    /// reload). A load already showing keeps its tree.
    pub(crate) fn commit(&mut self, loading: ModLoading) {
        self.mod_pool = loading.tree;
    }

    /// `keys` were just registered by their own owner (eager mod images or
    /// glyph art); the active load must not release them.
    pub(crate) fn disown_images(&mut self, keys: &HashSet<String>) {
        if let Some(load) = self.active.as_mut() {
            load.images.disown(keys);
        }
    }
}

/// What one Loading frame does next.
pub(crate) enum LoadingStep {
    /// Keep painting the loading screen.
    Paint,
    /// Install this payload now.
    Install(Box<LevelPayload>),
    /// The load failed for this reason.
    Fail(String),
}

/// Pick a load's tree: a uniform pick among the registered names of the
/// catalog entry's pool, else of the mod pool, else the engine fallback name
/// when it is registered. Unregistered names warn once each and are skipped.
/// `pick(n)` returns an index below `n`.
pub(crate) fn choose_loading_tree(
    catalog_pool: &[String],
    mod_pool: &[String],
    is_registered: impl Fn(&str) -> bool,
    warned_unregistered: &mut HashSet<String>,
    mut pick: impl FnMut(usize) -> usize,
) -> Option<String> {
    for pool in [catalog_pool, mod_pool] {
        let candidates: Vec<&String> = pool
            .iter()
            .filter(|name| {
                let registered = is_registered(name);
                if !registered && warned_unregistered.insert((*name).clone()) {
                    log::warn!(
                        "[UI] loading tree `{name}` is not a registered UI tree; skipping it"
                    );
                }
                registered
            })
            .collect();
        if !candidates.is_empty() {
            let index = pick(candidates.len()).min(candidates.len() - 1);
            return Some(candidates[index].clone());
        }
    }
    is_registered(LOADING_SCREEN_NAME).then(|| LOADING_SCREEN_NAME.to_string())
}

/// A uniform index below `len`. Client-local presentation needs no
/// determinism; a failed entropy read falls back to the clock.
fn random_index(len: usize) -> usize {
    let mut bytes = [0u8; 8];
    let seed = match getrandom::fill(&mut bytes) {
        Ok(()) => u64::from_le_bytes(bytes),
        Err(_) => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos() as u64),
    };
    (seed % len.max(1) as u64) as usize
}

impl App {
    /// Start the loading screen for `entry`: pick its tree, start decoding
    /// its loading-only images, publish the level name and zero progress.
    /// Returns the counter the worker reports through.
    pub(crate) fn begin_loading_screen(&mut self, entry: &LevelLoadEntry) -> Arc<LoadProgress> {
        let progress = Arc::new(LoadProgress::new());
        // A load replacing one still showing releases that load's images.
        self.end_active_load();
        let renderer = self.renderer.as_ref();
        let Some(session) = self.session.as_mut() else {
            return progress;
        };
        let modal_stack = &session.modal_stack;
        let state = &mut session.loading_screen;
        // The outgoing level's tier is cleared before a load begins, so a
        // level-tier tree is never a candidate.
        let tree = choose_loading_tree(
            &entry.loading_tree,
            &state.mod_pool,
            |name| {
                modal_stack
                    .resolve_with_tier(name)
                    .is_some_and(|(tier, _)| tier != postretro_ui::modal_stack::ScopeTier::Level)
            },
            &mut state.warned_unregistered,
            random_index,
        );
        if tree.is_none() {
            log::warn!(
                "[UI] no loading tree is registered, not even `{LOADING_SCREEN_NAME}`; loads show the boot splash"
            );
        }
        let wanted = tree
            .as_deref()
            .and_then(|name| modal_stack.resolve_with_tier(name))
            .map(|(_, descriptor)| {
                wanted_loading_images(descriptor, &session.mod_ui_images, |key| {
                    renderer.is_some_and(|renderer| renderer.has_ui_image(key))
                })
            })
            .unwrap_or_default();
        state.active = Some(ActiveLoad {
            tree,
            progress: progress.clone(),
            shown: 0.0,
            ui_time: 0.0,
            delivered: None,
            images: LoadingImages::start(&self.content_root, wanted),
        });
        let ctx = session.scripting.script_ctx.clone();
        write_loading_slot(&ctx, LEVEL_NAME_SLOT, SlotValue::String(entry.name.clone()));
        write_loading_slot(&ctx, PROGRESS_SLOT, SlotValue::Number(0.0));
        progress
    }

    /// End the loading screen — reveal, failure, abandon, relevel or suspend:
    /// drop any undelivered payload, release the load's loading-only images
    /// and discard decodes still in flight, and reset both slots to their
    /// defaults.
    pub(crate) fn end_loading_screen(&mut self) {
        if !self.end_active_load() {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let ctx = session.scripting.script_ctx.clone();
        write_loading_slot(&ctx, LEVEL_NAME_SLOT, SlotValue::String(String::new()));
        write_loading_slot(&ctx, PROGRESS_SLOT, SlotValue::Number(0.0));
    }

    /// Drop the active load, if any, unregistering the images it uploaded.
    /// Without a renderer (suspend) there is nothing left to unregister.
    /// Returns whether a load was active.
    fn end_active_load(&mut self) -> bool {
        let Some(load) = self
            .session
            .as_mut()
            .and_then(|session| session.loading_screen.active.take())
        else {
            return false;
        };
        if let Some(renderer) = self.renderer.as_mut() {
            load.images.release(renderer);
        }
        true
    }

    /// Whether a delivered payload is waiting for its install frame. Counts as
    /// a load in flight: no other request may start until it installs.
    pub(crate) fn has_deferred_level_payload(&self) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session
                .loading_screen
                .active
                .as_ref()
                .is_some_and(|load| load.delivered.is_some())
        })
    }

    /// The payload delivered on the previous Loading frame, ready to install.
    pub(crate) fn take_deferred_level_payload(&mut self) -> Option<LevelPayload> {
        self.session
            .as_mut()?
            .loading_screen
            .active
            .as_mut()?
            .delivered
            .take()
    }

    /// Hold a just-delivered payload for one more loading frame, with the bar
    /// at [`LOAD_PARSE_SHARE`], so the bar visibly arrives before the install
    /// hitch. Hands the payload back when no loading screen is showing.
    pub(crate) fn defer_level_payload(&mut self, payload: LevelPayload) -> Option<LevelPayload> {
        let Some(session) = self.session.as_mut() else {
            return Some(payload);
        };
        let Some(load) = session.loading_screen.active.as_mut() else {
            return Some(payload);
        };
        load.delivered = Some(payload);
        load.shown = load.shown.max(LOAD_PARSE_SHARE);
        let shown = load.shown;
        write_loading_slot(
            &session.scripting.script_ctx,
            PROGRESS_SLOT,
            SlotValue::Number(shown),
        );
        None
    }

    /// Advance the load's presentation by `frame_dt`: UI time, and
    /// `loading.progress` from the worker's counter (never decreasing).
    pub(crate) fn advance_loading_screen(&mut self, frame_dt: f32) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(load) = session.loading_screen.active.as_mut() else {
            return;
        };
        load.ui_time += f64::from(frame_dt);
        if load.delivered.is_some() {
            return;
        }
        let target = LOAD_PARSE_SHARE * load.progress.fraction().clamp(0.0, 1.0);
        if target > load.shown {
            load.shown = target;
            let shown = load.shown;
            write_loading_slot(
                &session.scripting.script_ctx,
                PROGRESS_SLOT,
                SlotValue::Number(shown),
            );
        }
    }

    /// Raise `loading.progress` to `progress` past the parse share, through
    /// Settling. Never lowers the bar.
    pub(crate) fn raise_loading_progress(&mut self, progress: f32) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(load) = session.loading_screen.active.as_mut() else {
            return;
        };
        if progress > load.shown {
            load.shown = progress.min(1.0);
            let shown = load.shown;
            write_loading_slot(
                &session.scripting.script_ctx,
                PROGRESS_SLOT,
                SlotValue::Number(shown),
            );
        }
    }

    /// Paint one Loading frame: the chosen loading tree as the frame's only UI
    /// layer, world-less, over the splash background. Without a session, a
    /// full-ready renderer, or any registered loading tree, paint the boot
    /// splash as before.
    pub(crate) fn paint_loading_frame(&mut self, event_loop: &ActiveEventLoop, frame_dt: f32) {
        let frame_start = Instant::now();
        self.advance_loading_screen(frame_dt);
        self.sync_glyph_art();
        self.poll_loading_images();
        match self.loading_screen_snapshot() {
            Some(snapshot) => {
                if !self.present_world_less_frame(
                    event_loop,
                    frame_start,
                    snapshot,
                    crate::render::SPLASH_CLEAR_COLOR,
                    false,
                ) {
                    let _ = self.paint_splash(event_loop);
                }
            }
            None => {
                let _ = self.paint_splash(event_loop);
            }
        }
    }

    /// The Loading frame's UI snapshot: the chosen tree alone, at the load's
    /// UI time. No HUD, no modal layers, no focus ring.
    pub(crate) fn loading_screen_snapshot(&mut self) -> Option<postretro_ui::UiReadSnapshot> {
        let session = self.session.as_mut()?;
        let load = session.loading_screen.active.as_ref()?;
        let name = load.tree.as_deref()?;
        let (tier, descriptor) = session.modal_stack.resolve_with_tier(name)?;
        let entry = postretro_ui::UiTreeEntry {
            name: name.to_string(),
            tier,
            capture_mode: descriptor.capture_mode,
            descriptor: descriptor.clone(),
            on_commit: None,
        };
        // Reconcile against every tree the frontend would retain as well, so
        // the menu under a backdrop load keeps its local state.
        let always_on = session.modal_stack.always_on_layers();
        let retained: Vec<&postretro_ui::descriptor::AnchoredTree> =
            std::iter::once(&entry.descriptor)
                .chain(always_on.iter().map(|layer| &layer.descriptor))
                .chain(session.modal_stack.retained_descriptors())
                .collect();
        session.presentation_cells.reconcile(&retained);
        let cell_values = session.presentation_cells.snapshot();
        let slot_values =
            Self::build_ui_slot_snapshot(&session.scripting.script_ctx.slot_table.borrow());
        let reduce_motion = crate::options::reduce_motion_from_slots(&slot_values);
        let mut snapshot = postretro_ui::UiReadSnapshot::with_trees(
            vec![entry],
            slot_values,
            cell_values,
            load.ui_time,
            None,
        );
        snapshot.reduce_motion = reduce_motion;
        crate::app::glyph_art::resolve_snapshot_glyphs(&mut snapshot, session);
        Some(snapshot)
    }

    /// Commit a manifest's loading-screen fields: the mod pool, `uiImages`,
    /// and the image keys its UI names, which decide the loading-only split.
    /// Eager images upload on the next sync.
    pub(crate) fn commit_loading_manifest(
        &mut self,
        ui_images: std::collections::BTreeMap<String, String>,
        loading: ModLoading,
        image_refs: ManifestImageRefs,
    ) {
        if let Some(session) = self.session.as_mut() {
            session.loading_screen.commit(loading);
            session.mod_ui_images.commit(ui_images, image_refs);
        }
    }

    /// Commit the loading-screen fields of a staged mod-init result, on the
    /// same boundary as the rest of its UI: only a committed result moves
    /// them, and a committed result without a start script clears them.
    pub(crate) fn commit_staged_loading_manifest(
        &mut self,
        result: &postretro_scripting_core::staged_manifest::StagedManifestBuildResult,
        outcome: &postretro_scripting_core::runtime::StagedManifestCommitOutcome,
    ) {
        use postretro_scripting_core::runtime::StagedManifestCommitOutcome;
        use postretro_scripting_core::staged_manifest::StagedManifestBuildStatus;
        if !matches!(outcome, StagedManifestCommitOutcome::Committed { .. }) {
            return;
        }
        let (ui_images, loading, image_refs) = match &result.status {
            StagedManifestBuildStatus::Built(manifest) => (
                manifest.ui_images.clone(),
                manifest.loading.clone(),
                ManifestImageRefs::from_manifest(
                    &manifest.ui_trees,
                    &manifest.presentation_templates,
                    &manifest.maps,
                    &manifest.loading.tree,
                ),
            ),
            StagedManifestBuildStatus::NoStartScript => Default::default(),
            StagedManifestBuildStatus::Failed => return,
        };
        self.commit_loading_manifest(ui_images, loading, image_refs);
    }
}

/// Write an engine-owned loading slot through the engine write path. The slots
/// are declared by the engine catalog, so a failure is an engine bug.
fn write_loading_slot(ctx: &postretro_entities::ctx::ScriptCtx, name: &str, value: SlotValue) {
    if let Err(err) = postretro_scripting_core::store_bridge::write_store_slot(ctx, name, value) {
        log::warn!("[UI] failed to write `{name}`: {err}");
    }
}

#[cfg(test)]
#[path = "loading_screen_tests.rs"]
mod tests;
