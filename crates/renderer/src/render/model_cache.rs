// The shared model cache: uploaded skinned models with their CPU companions,
// keyed by content, outliving the load that made them and shared by every
// resident level set that draws them.
// See: context/plans/done/level-load-pipeline/index.md (Model cache) · research.md §9 (P7, P11, P26, P28, P29, P30)

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use postretro_model::ModelHandle;
use postretro_model::gltf_loader::LoadedModel;
use postretro_render_cpu::mesh_pass::ClipMetadata;
use postretro_render_cpu::texture_prep::PreparedModelMaterials;

use super::Renderer;
use super::level_resource_set::{LevelSetId, SamplerMaps};
use super::mesh_pass::ModelEntry;
use super::renderer_models::build_model_entry;

/// One model's key in the shared cache: the key format's version and a
/// digest over everything that shapes the model's upload. The renderer only
/// compares keys; the level load derives them on its pool from the model's
/// file content and install-time parameters (the app's `model_key`), and
/// bumps the version whenever that derivation changes, so every older entry
/// misses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelContentKey {
    version: u32,
    digest: [u8; 32],
}

impl ModelContentKey {
    pub fn new(version: u32, digest: [u8; 32]) -> Self {
        Self { version, digest }
    }

    /// The key format version it was derived under.
    pub fn version(&self) -> u32 {
        self.version
    }
}

/// One shared model: its GPU entry (buffers, material bind groups, skeleton
/// and clips) and the CPU companion the app built from the same parse (its
/// hit zones). Cloning shares both. A set that draws the model holds the
/// entry, so the entry lives as long as any set, any uncommitted build, or
/// the cache itself holds it.
#[derive(Clone)]
pub struct CachedModel {
    pub(in crate::render) entry: Arc<ModelEntry>,
    companion: Arc<dyn Any + Send + Sync>,
}

impl std::fmt::Debug for CachedModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedModel")
            .field("identity", &self.identity())
            .field("clips", &self.entry.clips.len())
            .finish_non_exhaustive()
    }
}

impl CachedModel {
    /// The model's clips' names and durations, in glTF order: what a parse's
    /// clip-table entry holds.
    pub fn clip_metadata(&self) -> Vec<ClipMetadata> {
        self.entry
            .clips
            .iter()
            .map(|clip| ClipMetadata {
                name: clip.name.clone(),
                duration: clip.duration,
            })
            .collect()
    }

    /// The mesh's conservative local-space pose envelope: what a parse's
    /// clip-table entry holds as its bounds.
    pub fn local_bounds(&self) -> postretro_render_data::cone_frustum::Aabb {
        self.entry.bounds
    }

    /// The companion, if it is a `T`.
    pub fn companion<T: Any>(&self) -> Option<&T> {
        self.companion.downcast_ref()
    }

    /// The GPU entry's identity: equal for two handles on one entry.
    pub fn identity(&self) -> ModelEntryIdentity {
        ModelEntryIdentity::of(&self.entry)
    }
}

/// Which uploaded model entry a set or the cache holds, compared by
/// identity: two equal identities are one upload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModelEntryIdentity(usize);

impl ModelEntryIdentity {
    pub(in crate::render) fn of(entry: &Arc<ModelEntry>) -> Self {
        Self(Arc::as_ptr(entry) as usize)
    }
}

/// What a missed model's build part offers the cache: its key and the CPU
/// companion a later hit hands back. The entry joins the cache only when the
/// set that built it commits ([`Renderer::enter_model_cache_offers`]); a set that
/// never commits drops its offers with it.
pub struct ModelCacheOffer {
    key: ModelContentKey,
    companion: Arc<dyn Any + Send + Sync>,
}

impl ModelCacheOffer {
    pub fn new<T: Any + Send + Sync>(key: ModelContentKey, companion: T) -> Self {
        Self {
            key,
            companion: Arc::new(companion),
        }
    }

    pub(in crate::render) fn into_cached(
        self,
        entry: &Arc<ModelEntry>,
    ) -> (ModelContentKey, CachedModel) {
        (
            self.key,
            CachedModel {
                entry: Arc::clone(entry),
                companion: self.companion,
            },
        )
    }
}

type CacheEntries = HashMap<ModelContentKey, CachedModel>;

/// The renderer's model cache. Its entries hold GPU objects of this
/// renderer's device, so it lives and dies with the full renderer: a suspend
/// drops it with every entry no in-flight build still holds.
///
/// Only the main thread changes it, and only at a commit: a set's new
/// entries join when that set commits, and the same commit frees every entry
/// nothing but the cache holds. Lookups run anywhere, through a
/// [`ModelCacheHandle`].
#[derive(Default)]
pub(in crate::render) struct ModelCache {
    entries: Arc<Mutex<CacheEntries>>,
}

impl ModelCache {
    fn lock(&self) -> MutexGuard<'_, CacheEntries> {
        lock_entries(&self.entries)
    }

    pub(in crate::render) fn handle(&self) -> ModelCacheHandle {
        ModelCacheHandle {
            entries: Arc::downgrade(&self.entries),
        }
    }

    pub(in crate::render) fn lookup(&self, key: &ModelContentKey) -> Option<CachedModel> {
        self.lock().get(key).cloned()
    }

    /// Enter `cached` under `key` unless an entry holds the key already;
    /// whether it was entered.
    pub(in crate::render) fn insert(&self, key: ModelContentKey, cached: CachedModel) -> bool {
        let mut entries = self.lock();
        if entries.contains_key(&key) {
            return false;
        }
        entries.insert(key, cached);
        true
    }

    /// Free the cache's hold on every entry it alone holds; how many it
    /// freed. A set (resident or active), an uncommitted build or a task
    /// still holding a looked-up model keeps its entry.
    pub(in crate::render) fn sweep(&self) -> usize {
        let mut entries = self.lock();
        let before = entries.len();
        entries.retain(|_, cached| Arc::strong_count(&cached.entry) > 1);
        before - entries.len()
    }

    pub(in crate::render) fn keys(&self) -> Vec<ModelContentKey> {
        let mut keys: Vec<ModelContentKey> = self.lock().keys().copied().collect();
        keys.sort();
        keys
    }
}

/// Every lock here guards a plain map no panic leaves half-written.
fn lock_entries(entries: &Mutex<CacheEntries>) -> MutexGuard<'_, CacheEntries> {
    entries.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A load's way to look the cache up from its pool tasks, captured on the
/// main thread at dispatch. It keeps nothing alive: once its renderer is
/// gone (a suspend), every lookup misses, so no entry of the old device can
/// reach a load the resumed renderer serves.
#[derive(Clone)]
pub struct ModelCacheHandle {
    entries: Weak<Mutex<CacheEntries>>,
}

impl std::fmt::Debug for ModelCacheHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelCacheHandle")
            .field("live", &(self.entries.strong_count() > 0))
            .finish()
    }
}

impl ModelCacheHandle {
    /// The cached model at `key`, if any. A hit shares the entry: the
    /// caller hands it to its set (`LevelSetBuilder::cached_model`) instead
    /// of parsing and uploading.
    pub fn lookup(&self, key: &ModelContentKey) -> Option<CachedModel> {
        let entries = self.entries.upgrade()?;
        lock_entries(&entries).get(key).cloned()
    }
}

// Compile-time guard: the handle crosses to pool tasks, and looked-up models
// cross to the upload thread.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ModelCacheHandle>();
    assert_send_sync::<CachedModel>();
    assert_send_sync::<ModelCacheOffer>();
};

/// What one commit's cache step did: its set's offers entered
/// ([`Renderer::enter_model_cache_offers`]), then one sweep
/// ([`Renderer::sweep_model_cache`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelCacheCommit {
    /// The committed set's new entries that joined the cache.
    pub inserted: usize,
    /// Entries freed because nothing but the cache held them.
    pub freed: usize,
    /// Entries the cache holds after the commit.
    pub retained: usize,
}

impl Renderer {
    /// A handle pool tasks look the model cache up through, `None` before
    /// full init. Capture it at dispatch, beside the set build.
    pub fn model_cache_handle(&self) -> Option<ModelCacheHandle> {
        self.full.as_ref().map(|full| full.model_cache.handle())
    }

    /// The cached model at `key`, looked up on the main thread.
    pub fn cached_model(&self, key: &ModelContentKey) -> Option<CachedModel> {
        self.full
            .as_ref()
            .and_then(|full| full.model_cache.lookup(key))
    }

    /// The tests' one-call stand-in for a commit's cache step: the set
    /// `committed`'s offers enter ([`Self::enter_model_cache_offers`]), then
    /// one sweep ([`Self::sweep_model_cache`]). The level commit itself calls
    /// the two apart, around its model install, so its miss fallback can
    /// still hit an entry only the level it released held. Creates nothing.
    /// `None` (changing nothing) for an unknown set.
    #[cfg(any(test, feature = "test-seams"))]
    pub fn commit_model_cache(&mut self, committed: LevelSetId) -> Option<ModelCacheCommit> {
        let inserted = self.enter_model_cache_offers(committed)?;
        let freed = self.sweep_model_cache()?;
        Some(ModelCacheCommit {
            inserted,
            freed,
            retained: self.model_cache_keys().len(),
        })
    }

    /// The commit's cache entry step, after the set `committed` was
    /// published: the set's new entries (its cache misses) join the cache,
    /// each under its key unless an entry holds that key already; how many
    /// joined. Frees nothing: the commit sweeps once, after its model
    /// install ([`Self::sweep_model_cache`]). Creates nothing. `None`
    /// (changing nothing) for an unknown set.
    ///
    /// The one place entries join (beside the commit's own synchronous miss
    /// path, [`Self::upload_missed_model_into_active_set`]): a failed or
    /// cancelled load never commits, so its uploads never become visible to
    /// a later lookup, and nothing needs rolling back (pins P11, P29).
    pub fn enter_model_cache_offers(&mut self, committed: LevelSetId) -> Option<usize> {
        let full = self.full.as_mut()?;
        let level = match full.level.as_mut() {
            Some(level) if level.id == committed => level,
            _ => full
                .resident_levels
                .iter_mut()
                .find(|level| level.id == committed)?,
        };
        let offers = level.models.take_offers();
        Some(
            offers
                .into_iter()
                .filter(|(key, cached)| full.model_cache.insert(*key, cached.clone()))
                .count(),
        )
    }

    /// Free every cache entry no resident or active set, no uncommitted
    /// build and no task still holding a looked-up model references; how
    /// many it freed. A commit runs it once, after its model install, so
    /// the entries only a released level or an over-prediction held go
    /// then, and not before the miss fallback could hit them. Releasing a
    /// set frees no entry; the next commit's sweep does (P30). Creates
    /// nothing; `None` before full init.
    pub fn sweep_model_cache(&mut self) -> Option<usize> {
        self.full.as_ref().map(|full| full.model_cache.sweep())
    }

    /// Every key the model cache holds, sorted (diagnostics and tests).
    pub fn model_cache_keys(&self) -> Vec<ModelContentKey> {
        self.full
            .as_ref()
            .map_or_else(Vec::new, |full| full.model_cache.keys())
    }

    /// The identity of the entry set `id` draws `handle` with, `None` when
    /// the set or the model is absent.
    pub fn level_set_model_identity(
        &self,
        id: LevelSetId,
        handle: &str,
    ) -> Option<ModelEntryIdentity> {
        let full = self.full.as_ref()?;
        full.level
            .iter()
            .chain(&full.resident_levels)
            .find(|level| level.id == id)?
            .models
            .entry(&ModelHandle::from(handle.to_string()))
            .map(ModelEntryIdentity::of)
    }

    /// The commit's synchronous hit: draw `handle` in the active set with
    /// the cached entry, creating nothing. `false` with no active set.
    pub fn install_cached_model_into_active_set(
        &mut self,
        handle: &str,
        cached: &CachedModel,
    ) -> bool {
        let Some(level) = self.full.as_mut().and_then(|full| full.level.as_mut()) else {
            return false;
        };
        level.models.insert(
            ModelHandle::from(handle.to_string()),
            Arc::clone(&cached.entry),
        );
        true
    }

    /// The commit's over-prediction release, after its real sweep: drop each
    /// of `handles` from the active set's models (a model the load predicted
    /// and built that the sweep does not draw); how many it held. Frees no
    /// cache entry: the commit's one sweep after its model install
    /// ([`Self::sweep_model_cache`]) frees each entry nothing references
    /// any more, while an entry another handle, another resident set or an
    /// uncommitted build still holds stays. Creates nothing; `None` with no
    /// active set.
    pub fn release_unused_active_set_models(&mut self, handles: &[&str]) -> Option<usize> {
        let level = self.full.as_mut()?.level.as_mut()?;
        Some(
            handles
                .iter()
                .filter(|handle| level.models.remove(&ModelHandle::from(handle.to_string())))
                .count(),
        )
    }

    /// The commit's synchronous miss: upload one parsed model into the
    /// active set (its new samplers join the renderer's maps, as a publish
    /// merges them) and, with an `offer`, enter it in the cache at once:
    /// this runs on the main thread at the commit, where a set's entries
    /// join. Its materials bind at `surface_depth_quality`, the tier the
    /// offer's key was derived at (the load's dispatch tier, which the set
    /// it joins was built at), never the live one, so the entry's content
    /// matches its key. Returns the cached model it entered, `None` with no
    /// offer, with the key already taken, or with no active set (which
    /// uploads nothing).
    pub fn upload_missed_model_into_active_set(
        &mut self,
        handle: &str,
        model: &LoadedModel,
        materials: PreparedModelMaterials,
        surface_depth_quality: postretro_render_cpu::surface_depth::SurfaceDepthQuality,
        offer: Option<ModelCacheOffer>,
    ) -> Option<CachedModel> {
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full.as_mut()?;
        full.level.as_ref()?;
        let mut samplers = SamplerMaps::default();
        let entry = Arc::new(build_model_entry(
            device,
            queue.raw(),
            &full.material_bind(surface_depth_quality),
            &mut samplers,
            model,
            materials,
        ));
        full.samplers.merge(samplers);
        let level = full.level.as_mut()?;
        level
            .models
            .insert(ModelHandle::from(handle.to_string()), Arc::clone(&entry));
        let (key, cached) = offer?.into_cached(&entry);
        full.model_cache
            .insert(key, cached.clone())
            .then_some(cached)
    }
}
