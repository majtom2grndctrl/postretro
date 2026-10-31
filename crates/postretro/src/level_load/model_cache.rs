//! The shared model cache's app side. Each predicted model's prep task reads
//! the model's files once, keys them, and looks the renderer's cache up: a
//! hit hands the cached entry to the load's set and the cached CPU products
//! to the commit, so the commit cannot tell it from a parse; a miss parses
//! and uploads from the bytes it already read and offers its entry for the
//! commit to enter. Also the commit's synchronous fallback for a model the
//! prediction lacked, with its per-load count.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Model cache, Predict models, then fall back exactly) · research.md §9 (P11, P29)

use std::collections::HashSet;
use std::path::Path;

use postretro_model::gltf_loader::{LoadedModel, load_model_from_sources, read_model_sources};
use postretro_render_cpu::texture_prep::{
    ModelSidecars, PreparedModelMaterials, prepare_model_materials,
};

use super::model_key::{ModelInstallParams, model_content_key};
use super::products::{ParsedModelProducts, PreparedModel};
use crate::render::{CachedModel, ModelCacheHandle, ModelCacheOffer, ModelContentKey, Renderer};
use crate::scripting_systems::hit_zones::ModelHitZones;
use crate::startup::lifecycle::{ModelClipEntry, model_clip_entry};

/// How a prepared model met the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelCacheUse {
    /// The cache held its key: no parse, no upload.
    Hit,
    /// The cache lacked its key: parsed, and uploaded by the set.
    Miss,
    /// Not keyed: the load had no cache (no renderer), or the model's files
    /// could not be read, so the parse fails as the oracle's does.
    Unkeyed,
}

/// What a prepared model's set needs, beside the products the commit reads.
pub(crate) enum ModelPart {
    /// Share the cached entry.
    Cached(CachedModel),
    /// Upload the parse; with an offer, the entry joins the cache when the
    /// set commits.
    Upload {
        model: Box<LoadedModel>,
        materials: PreparedModelMaterials,
        offer: Option<ModelCacheOffer>,
    },
}

/// One predicted model through the cache: read its files once, key them and
/// look the key up when the load has a cache, then either take the hit's
/// products or parse and prepare from the bytes already read. The
/// presentation products are the oracle sweep's either way: the clip entry
/// and hit zones a parse would build (a hit's were built from the parse that
/// made the entry). A failed read or parse is carried, never raised, exactly
/// as `parse_sweep_model` carries it.
pub(crate) fn prep_model(
    handle: &str,
    content_root: &Path,
    prm_cache_root: &Path,
    cache: Option<&ModelCacheHandle>,
    params: ModelInstallParams,
) -> (PreparedModel, Option<ModelPart>) {
    let open_path = content_root.join(handle);
    let sources = match read_model_sources(&open_path) {
        Ok(sources) => sources,
        Err(error) => return (failed(handle, &open_path, error), None),
    };
    let keyed = cache.map(|cache| {
        let sidecars = ModelSidecars::read(sources.material_keys(), prm_cache_root);
        let key = model_content_key(&sources, &sidecars, params);
        (cache.lookup(&key), key, sidecars)
    });
    let (sidecars, key) = match keyed {
        Some((Some(cached), key, sidecars)) => match hit_products(handle, &open_path, &cached) {
            Some(model) => return (model, Some(ModelPart::Cached(cached))),
            // An entry without this app's companion cannot stand in for a
            // parse: parse it.
            None => (Some(sidecars), Some(key)),
        },
        Some((None, key, sidecars)) => (Some(sidecars), Some(key)),
        None => (None, None),
    };
    let result = load_model_from_sources(&open_path, sources);
    let clip_entry = model_clip_entry(&result);
    let model = match result {
        Ok(model) => model,
        Err(error) => return (failed(handle, &open_path, error), None),
    };
    let hit_zones = ModelHitZones::build(&model);
    let materials = match sidecars {
        Some(sidecars) => sidecars.prepare(&model.submeshes, prm_cache_root),
        None => prepare_model_materials(&model.submeshes, prm_cache_root),
    };
    let offer = key.map(|key| ModelCacheOffer::new(key, hit_zones.clone()));
    let prepared = PreparedModel {
        handle: handle.to_string(),
        open_path,
        clip_entry,
        parsed: Ok(ParsedModelProducts {
            hit_zones,
            upload: None,
        }),
        cache: if key.is_some() {
            ModelCacheUse::Miss
        } else {
            ModelCacheUse::Unkeyed
        },
    };
    (
        prepared,
        Some(ModelPart::Upload {
            model: Box::new(model),
            materials,
            offer,
        }),
    )
}

/// A cached model's presentation products: the clip entry from its entry's
/// clips and bounds, which the parse that made it supplied, and its hit
/// zones. `None` when its companion is not this app's hit zones.
fn hit_products(handle: &str, open_path: &Path, cached: &CachedModel) -> Option<PreparedModel> {
    let hit_zones = cached.companion::<ModelHitZones>()?.clone();
    Some(PreparedModel {
        handle: handle.to_string(),
        open_path: open_path.to_path_buf(),
        clip_entry: ModelClipEntry {
            clips: cached.clip_metadata(),
            bounds: cached.local_bounds(),
        },
        parsed: Ok(ParsedModelProducts {
            hit_zones,
            upload: None,
        }),
        cache: ModelCacheUse::Hit,
    })
}

fn failed(
    handle: &str,
    open_path: &Path,
    error: postretro_model::gltf_loader::ModelLoadError,
) -> PreparedModel {
    PreparedModel {
        handle: handle.to_string(),
        open_path: open_path.to_path_buf(),
        clip_entry: ModelClipEntry::default(),
        parsed: Err(error),
        cache: ModelCacheUse::Unkeyed,
    }
}

/// The commit's fallback for models the prediction lacked, for one load: a
/// handle the commit's real sweep holds that prep never prepared. Each one
/// is looked up by key, else parsed and uploaded into the active set and
/// entered in the cache (this runs on the main thread at the commit, where
/// entries join), and counted and logged once per load however often the
/// commit asks for it. A model that fails warns at the commit as any failed
/// model does; the fallback adds no way for the commit to fail.
#[derive(Debug, Default)]
pub(crate) struct ModelMissFallback {
    missed: HashSet<String>,
}

impl ModelMissFallback {
    /// Distinct handles that took the fallback this load.
    pub(crate) fn misses(&self) -> usize {
        self.missed.len()
    }

    /// Load `handle` into `renderer`'s active set, synchronously, and return
    /// the products the commit installs, as a prepared model's.
    pub(crate) fn load(
        &mut self,
        renderer: &mut Renderer,
        handle: &str,
        content_root: &Path,
        prm_cache_root: &Path,
        params: ModelInstallParams,
    ) -> PreparedModel {
        if self.missed.insert(handle.to_string()) {
            log::warn!(
                "[Model] model prediction missed '{handle}': loading it synchronously at commit"
            );
        }
        let open_path = content_root.join(handle);
        let sources = match read_model_sources(&open_path) {
            Ok(sources) => sources,
            Err(error) => return failed(handle, &open_path, error),
        };
        let sidecars = ModelSidecars::read(sources.material_keys(), prm_cache_root);
        let key: ModelContentKey = model_content_key(&sources, &sidecars, params);
        if let Some(cached) = renderer.cached_model(&key)
            && let Some(model) = hit_products(handle, &open_path, &cached)
        {
            renderer.install_cached_model_into_active_set(handle, &cached);
            return model;
        }
        let result = load_model_from_sources(&open_path, sources);
        let clip_entry = model_clip_entry(&result);
        let model = match result {
            Ok(model) => model,
            Err(error) => return failed(handle, &open_path, error),
        };
        let hit_zones = ModelHitZones::build(&model);
        let materials = sidecars.prepare(&model.submeshes, prm_cache_root);
        renderer.upload_missed_model_into_active_set(
            handle,
            &model,
            materials,
            Some(ModelCacheOffer::new(key, hit_zones.clone())),
        );
        PreparedModel {
            handle: handle.to_string(),
            open_path,
            clip_entry,
            parsed: Ok(ParsedModelProducts {
                hit_zones,
                upload: None,
            }),
            cache: ModelCacheUse::Miss,
        }
    }
}
